pub mod interface;
pub mod poller;
pub mod utils;

use crate::config::BitcoindConfig;
use crate::{database::DatabaseError, revaultd::RevaultD, threadmessages::BitcoindMessageOut};
use interface::{BitcoinD, WalletTransaction};
use poller::poller_main;
use revault_tx::bitcoin::{Network, Txid};

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
        Arc, RwLock,
    },
    thread,
    time::Duration,
};

use jsonrpc::{
    error::{Error, RpcError},
    simple_http,
};

/// Number of retries the client is allowed to do in case of timeout or i/o error
/// while communicating with the bitcoin daemon.
/// A retry happens every 1 second, this makes us give up after one minute.
const BITCOIND_RETRY_LIMIT: usize = 60;

/// The minimum bitcoind version that can be used with revaultd.
const MIN_BITCOIND_VERSION: u64 = 220000;

/// An error happened in the bitcoind-manager thread
#[derive(Debug)]
pub enum BitcoindError {
    /// It can be related to us..
    Custom(String),
    /// Or directly to bitcoind's RPC server
    Server(Error),
    /// They replied to a batch request omitting some responses
    BatchMissingResponse,
    RevaultTx(revault_tx::Error),
}

impl BitcoindError {
    /// Is bitcoind just starting ?
    pub fn is_warming_up(&self) -> bool {
        match self {
            // https://github.com/bitcoin/bitcoin/blob/dca80ffb45fcc8e6eedb6dc481d500dedab4248b/src/rpc/protocol.h#L49
            BitcoindError::Server(Error::Rpc(RpcError { code, .. })) => *code == -28,
            _ => false,
        }
    }
}

impl std::fmt::Display for BitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BitcoindError::Custom(ref s) => write!(f, "Bitcoind manager error: {}", s),
            BitcoindError::Server(ref e) => write!(f, "Bitcoind server error: {}", e),
            BitcoindError::BatchMissingResponse => write!(
                f,
                "Bitcoind server replied without enough responses to our batched request"
            ),
            BitcoindError::RevaultTx(ref s) => write!(f, "Bitcoind manager error: {}", s),
        }
    }
}

impl std::error::Error for BitcoindError {}

// FIXME: remove this (and probably the 'Custom' variant too. If we fail to access the DB we should
// panic.
impl From<DatabaseError> for BitcoindError {
    fn from(e: DatabaseError) -> Self {
        Self::Custom(format!("Database error in bitcoind thread: {}", e))
    }
}

impl From<simple_http::Error> for BitcoindError {
    fn from(e: simple_http::Error) -> Self {
        Self::Server(Error::Transport(Box::new(e)))
    }
}

impl From<revault_tx::Error> for BitcoindError {
    fn from(e: revault_tx::Error) -> Self {
        Self::RevaultTx(e)
    }
}

fn check_bitcoind_network(
    bitcoind: &BitcoinD,
    config_network: &Network,
) -> Result<(), BitcoindError> {
    let chaininfo = bitcoind.getblockchaininfo()?;
    let chain = chaininfo
        .get("chain")
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            BitcoindError::Custom("No valid 'chain' in getblockchaininfo response?".to_owned())
        })?;
    let bip70_net = match config_network {
        Network::Bitcoin => "main",
        Network::Testnet => "test",
        Network::Regtest => "regtest",
        Network::Signet => "signet",
    };

    if !bip70_net.eq(chain) {
        return Err(BitcoindError::Custom(format!(
            "Wrong network, bitcoind is on '{}' but our config says '{}' ({})",
            chain, bip70_net, config_network
        )));
    }

    Ok(())
}

fn check_bitcoind_version(bitcoind: &BitcoinD) -> Result<(), BitcoindError> {
    let network_info = bitcoind.getnetworkinfo()?;
    let bitcoind_version = network_info
        .get("version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            BitcoindError::Custom("No valid 'version' in getnetworkinfo response?".to_owned())
        })?;

    if bitcoind_version < MIN_BITCOIND_VERSION {
        return Err(BitcoindError::Custom(format!(
            "Revaultd needs bitcoind v{} or greater to operate but v{} was found",
            MIN_BITCOIND_VERSION, bitcoind_version
        )));
    }

    Ok(())
}

/// Some sanity checks to be done at startup to make sure our bitcoind isn't going to fail under
/// our feet for a legitimate reason.
fn bitcoind_sanity_checks(
    bitcoind: &BitcoinD,
    bitcoind_config: &BitcoindConfig,
) -> Result<(), BitcoindError> {
    check_bitcoind_version(bitcoind)?;
    check_bitcoind_network(bitcoind, &bitcoind_config.network)?;
    Ok(())
}

/// Connects to and sanity checks bitcoind.
pub fn start_bitcoind(revaultd: &mut RevaultD) -> Result<BitcoinD, BitcoindError> {
    let bitcoind = BitcoinD::new(
        &revaultd.bitcoind_config,
        revaultd
            .watchonly_wallet_file()
            .expect("Wallet id is set at startup in setup_db()"),
        revaultd
            .cpfp_wallet_file()
            .expect("Wallet id is set at startup in setup_db()"),
    )
    .map_err(|e| BitcoindError::Custom(format!("Could not connect to bitcoind: {}", e)))?;

    while let Err(e) = bitcoind_sanity_checks(&bitcoind, &revaultd.bitcoind_config) {
        if e.is_warming_up() {
            log::info!("Bitcoind is warming up. Waiting for it to be back up.");
            thread::sleep(Duration::from_secs(3))
        } else {
            return Err(e);
        }
    }

    Ok(bitcoind.with_retry_limit(BITCOIND_RETRY_LIMIT))
}

fn wallet_transaction(bitcoind: &BitcoinD, txid: Txid) -> Option<WalletTransaction> {
    bitcoind
        .get_wallet_transaction(&txid)
        .map_err(|res| {
            log::trace!(
                "Got '{:?}' from bitcoind when requesting wallet transaction '{}'",
                res,
                txid
            );
            res
        })
        .ok()
}

/// The bitcoind event loop.
/// Listens for bitcoind requests (wallet / chain) and poll bitcoind every 30 seconds,
/// updating our state accordingly.
pub fn bitcoind_main_loop(
    rx: Receiver<BitcoindMessageOut>,
    revaultd: Arc<RwLock<RevaultD>>,
    bitcoind: BitcoinD,
) -> Result<(), BitcoindError> {
    let bitcoind = Arc::new(RwLock::new(bitcoind));
    // The verification progress announced by bitcoind *at startup* thus won't be updated
    // after startup check. Should be *exactly* 1.0 when synced, but hey, floats so we are
    // careful.
    let sync_progress = Arc::new(RwLock::new(0.0f64));
    // Used to shutdown the poller thread
    let shutdown = Arc::new(AtomicBool::new(false));

    // We use a thread to 1) wait for bitcoind to be synced 2) poll listunspent
    let poller_thread = std::thread::spawn({
        let _bitcoind = bitcoind.clone();
        let _sync_progress = sync_progress.clone();
        let _shutdown = shutdown.clone();
        move || poller_main(revaultd, _bitcoind, _sync_progress, _shutdown)
    });

    for msg in rx {
        match msg {
            BitcoindMessageOut::Shutdown => {
                log::info!("Bitcoind received shutdown from main. Exiting.");
                shutdown.store(true, Ordering::Relaxed);
                poller_thread
                    .join()
                    .expect("Joining bitcoind poller thread");
                return Ok(());
            }
            BitcoindMessageOut::SyncProgress(resp_tx) => {
                resp_tx.send(*sync_progress.read().unwrap()).map_err(|e| {
                    BitcoindError::Custom(format!(
                        "Sending synchronization progress to main thread: {}",
                        e
                    ))
                })?;
            }
            BitcoindMessageOut::WalletTransaction(txid, resp_tx) => {
                log::trace!("Received 'wallettransaction' from main thread");
                // FIXME: what if bitcoind isn't synced?
                resp_tx
                    .send(wallet_transaction(&bitcoind.read().unwrap(), txid))
                    .map_err(|e| {
                        BitcoindError::Custom(format!(
                            "Sending wallet transaction to main thread: {}",
                            e
                        ))
                    })?;
            }
            BitcoindMessageOut::BroadcastTransactions(txs, resp_tx) => {
                log::trace!("Received 'broadcastransactions' from main thread");
                resp_tx
                    .send(bitcoind.read().unwrap().broadcast_transactions(&txs))
                    .map_err(|e| {
                        BitcoindError::Custom(format!(
                            "Sending transactions broadcast result to main thread: {}",
                            e
                        ))
                    })?;
            }
        }
    }

    Ok(())
}
