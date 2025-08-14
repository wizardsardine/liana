//! Implementation of the Bitcoin interface using bitcoind.
//!
//! We use the RPC interface and a watchonly descriptor wallet.

mod utils;
use crate::{
    bitcoin::{Block, BlockChainTip},
    config,
};
use liana::descriptors::LianaDescriptor;
use utils::{block_before_date, roundup_progress};

use std::{
    cmp,
    collections::{HashMap, HashSet},
    convert::TryInto,
    fs, io,
    str::FromStr,
    thread,
    time::Duration,
};

use jsonrpc::{
    arg,
    client::Client,
    minreq,
    minreq_http::{self, MinreqHttpTransport},
};

use miniscript::{
    bitcoin::{self, address, hashes::hex::FromHex},
    descriptor::{self, Descriptor, DescriptorPublicKey},
};

use serde_json::Value as Json;

// If bitcoind takes more than 3 minutes to answer one of our queries, fail.
const RPC_SOCKET_TIMEOUT: u64 = 180;

// Number of retries the client is allowed to do in case of timeout or i/o error
// while communicating with the bitcoin daemon.
// A retry happens every 1 second, this makes us give up after one minute.
const BITCOIND_RETRY_LIMIT: usize = 60;

// The minimum bitcoind version that can be used with lianad.
const MIN_BITCOIND_VERSION: u64 = 240000;

// The minimum bitcoind version that can be used with lianad and a Taproot descriptor.
const MIN_TAPROOT_BITCOIND_VERSION: u64 = 260000;

/// An error in the bitcoind interface.
#[derive(Debug)]
pub enum BitcoindError {
    CookieFile(io::Error),
    /// Bitcoind server error.
    Server(jsonrpc::error::Error),
    /// They replied to a batch request omitting some responses.
    BatchMissingResponse,
    /// Error while managing wallet.
    Wallet(String /* watchonly wallet path */, WalletError),
    InvalidVersion(u64),
    NetworkMismatch(String /*config*/, String /*bitcoind*/),
    StartRescan,
    RescanPastPruneHeight,
}

impl BitcoindError {
    /// Is bitcoind just starting ?
    pub fn is_warming_up(&self) -> bool {
        match self {
            // https://github.com/bitcoin/bitcoin/blob/dca80ffb45fcc8e6eedb6dc481d500dedab4248b/src/rpc/protocol.h#L49
            BitcoindError::Server(jsonrpc::error::Error::Rpc(jsonrpc::error::RpcError {
                code,
                ..
            })) => *code == -28,
            _ => false,
        }
    }

    /// Is it a timeout of any kind?
    pub fn is_timeout(&self) -> bool {
        if let BitcoindError::Server(jsonrpc::Error::Transport(ref e)) = self {
            if let Some(minreq_http::Error::Minreq(minreq::Error::IoError(e))) =
                e.downcast_ref::<minreq_http::Error>()
            {
                return e.kind() == io::ErrorKind::TimedOut;
            }
        }
        false
    }

    /// Is it an error that can be recovered from?
    pub fn is_transient(&self) -> bool {
        if let BitcoindError::Server(jsonrpc::Error::Transport(ref e)) = self {
            if let Some(ref e) = e.downcast_ref::<minreq_http::Error>() {
                // Bitcoind is overloaded
                if let minreq_http::Error::Http(minreq_http::HttpError { status_code, .. }) = e {
                    return status_code == &503;
                }
                // Bitcoind may have been restarted
                return matches!(e, minreq_http::Error::Minreq(minreq::Error::IoError(_)));
            }
        }
        false
    }

    /// Is it an error that has to do with our credentials?
    pub fn is_unauthorized(&self) -> bool {
        if let BitcoindError::Server(jsonrpc::Error::Transport(ref e)) = self {
            if let Some(minreq_http::Error::Http(minreq_http::HttpError { status_code, .. })) =
                e.downcast_ref::<minreq_http::Error>()
            {
                return status_code == &402;
            }
        }
        false
    }
}

impl std::fmt::Display for BitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BitcoindError::CookieFile(e) => write!(f, "Reading bitcoind cookie file: {}", e),
            BitcoindError::Server(ref e) => write!(f, "Bitcoind RPC server error: {}", e),
            BitcoindError::BatchMissingResponse => write!(
                f,
                "Bitcoind server replied without enough responses to our batched request"
            ),
            BitcoindError::Wallet(path, e) => {
                write!(f, "Watchonly wallet (path: {}) error: {}", path, e)
            }
            BitcoindError::InvalidVersion(v) => {
                write!(
                    f,
                    "Invalid bitcoind version '{}', minimum supported is '{}' and minimum supported if using Taproot is '{}'.",
                    v, MIN_BITCOIND_VERSION, MIN_TAPROOT_BITCOIND_VERSION
                )
            }
            BitcoindError::NetworkMismatch(conf_net, bitcoind_net) => {
                write!(
                    f,
                    "Network mismatch. We are supposed to run on '{}' but bitcoind is on '{}'.",
                    conf_net, bitcoind_net
                )
            }
            BitcoindError::StartRescan => {
                write!(
                    f,
                    "Error while triggering the rescan for the bitcoind watchonly wallet."
                )
            }
            BitcoindError::RescanPastPruneHeight => {
                write!(
                    f,
                    "Trying to rescan the block chain past the prune block height."
                )
            }
        }
    }
}

impl std::error::Error for BitcoindError {}

impl From<jsonrpc::error::Error> for BitcoindError {
    fn from(e: jsonrpc::error::Error) -> Self {
        Self::Server(e)
    }
}

impl From<minreq_http::Error> for BitcoindError {
    fn from(e: minreq_http::Error) -> Self {
        jsonrpc::error::Error::Transport(Box::new(e)).into()
    }
}

#[derive(Debug)]
pub enum WalletError {
    Creating(String),
    ImportingDescriptor(String),
    Loading(String),
    MissingOrTooManyWallet,
    MissingDescriptor,
}

impl std::fmt::Display for WalletError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            WalletError::Creating(s) => {
                write!(f, "Error creating watchonly wallet: {}", s)
            }
            WalletError::ImportingDescriptor(s) => write!(
                f,
                "Error importing descriptor. Response from bitcoind: '{}'",
                s
            ),
            WalletError::Loading(s) => {
                write!(f, "Error when loading watchonly wallet: '{}'.", s)
            }
            WalletError::MissingOrTooManyWallet => {
                write!(
                    f,
                    "No, or too many, watchonly wallet(s) loaded on bitcoind."
                )
            }
            WalletError::MissingDescriptor => {
                write!(f, "The watchonly wallet loaded on bitcoind does not have the main descriptor imported.")
            }
        }
    }
}

pub struct BitcoinD {
    /// Client for generalistic calls.
    node_client: Client,
    /// A client that will disregard responses to the queries it makes.
    sendonly_client: Client,
    /// A client for calls related to the wallet.
    watchonly_client: Client,
    watchonly_wallet_path: String,
    /// How many times we'll retry upon failure to send a request.
    retries: usize,
}

macro_rules! params {
    ($($param:expr),* $(,)?) => {
        // FIXME: is there a way to avoid the allocation of an unnecessary Box?
        Some(&*arg(Json::Array(vec![
            $(
                $param,
            )*
        ])))
    };
}

impl BitcoinD {
    /// Create a new bitcoind interface. This tests the connection to bitcoind and disables retries
    /// on failure to send a request.
    pub fn new(
        config: &config::BitcoindConfig,
        watchonly_wallet_path: String,
    ) -> Result<BitcoinD, BitcoindError> {
        let node_url = format!("http://{}", config.addr);
        let watchonly_url = format!("http://{}/wallet/{}", config.addr, watchonly_wallet_path);

        let builder = match &config.rpc_auth {
            config::BitcoindRpcAuth::CookieFile(cookie_path) => {
                let cookie_string =
                    fs::read_to_string(cookie_path).map_err(BitcoindError::CookieFile)?;
                MinreqHttpTransport::builder().cookie_auth(cookie_string)
            }
            config::BitcoindRpcAuth::UserPass(user, pass) => {
                MinreqHttpTransport::builder().basic_auth(user.clone(), Some(pass.clone()))
            }
        };

        // Create a dummy bitcoind with clients using a low timeout to sanity check the connection.
        let dummy_node_client = Client::with_transport(
            builder
                .clone()
                .url(&node_url)
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(3))
                .build(),
        );
        let sendonly_client = Client::with_transport(
            builder
                .clone()
                .url(&watchonly_url)
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(1))
                .build(),
        );
        let dummy_wo_client = Client::with_transport(
            builder
                .clone()
                .url(&watchonly_url)
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(3))
                .build(),
        );
        let dummy_bitcoind = BitcoinD {
            node_client: dummy_node_client,
            sendonly_client,
            watchonly_client: dummy_wo_client,
            watchonly_wallet_path: watchonly_wallet_path.clone(),
            retries: 0,
        };
        log::info!("Checking the connection to bitcoind.");
        dummy_bitcoind.check_connection()?;
        log::info!("Connection to bitcoind checked.");

        // Now the connection is checked, create the clients with an appropriate timeout.
        let node_client = Client::with_transport(
            builder
                .clone()
                .url(&node_url)
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(RPC_SOCKET_TIMEOUT))
                .build(),
        );
        let sendonly_client = Client::with_transport(
            builder
                .clone()
                .url(&watchonly_url)
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(1))
                .build(),
        );
        let watchonly_client = Client::with_transport(
            builder
                .url(&watchonly_url)
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(RPC_SOCKET_TIMEOUT))
                .build(),
        );
        Ok(BitcoinD {
            node_client,
            sendonly_client,
            watchonly_client,
            watchonly_wallet_path,
            retries: BITCOIND_RETRY_LIMIT,
        })
    }

    fn check_client(&self, client: &Client) -> Result<(), BitcoindError> {
        if let Err(e) = self.make_request(client, "echo", None) {
            if e.is_warming_up() {
                log::info!("bitcoind is warming up. Retrying connection sanity check in 1 second.");
                thread::sleep(Duration::from_secs(1));
                return self.check_client(client);
            } else {
                return Err(e);
            }
        }
        Ok(())
    }

    // Make sure bitcoind is reachable through all clients. Note we don't check the sendonly client
    // since it has precisely a very low timeout for the purpose of ignoring responses.
    fn check_connection(&self) -> Result<(), BitcoindError> {
        self.check_client(&self.node_client)?;
        self.check_client(&self.watchonly_client)?;
        Ok(())
    }

    /// Wrapper to retry a request sent to bitcoind upon IO failure
    /// according to the configured number of retries.
    fn retry<T, R: Fn() -> Result<T, BitcoindError>>(
        &self,
        request: R,
    ) -> Result<T, BitcoindError> {
        let mut error: Option<BitcoindError> = None;
        for i in 0..self.retries + 1 {
            match request() {
                Ok(res) => return Ok(res),
                Err(e) => {
                    if e.is_warming_up() {
                        // Always retry when bitcoind is warming up, it'll be available eventually.
                        std::thread::sleep(Duration::from_secs(1));
                        error = Some(e)
                    } else if e.is_unauthorized() {
                        // FIXME: it should be trivial for us to cache the cookie path and simply
                        // refresh the credentials when this happens. Unfortunately this means
                        // making the BitcoinD struct mutable...
                        log::error!("Denied access to bitcoind. Most likely bitcoind was restarted from under us and the cookie changed.");
                        return Err(e);
                    } else if e.is_transient() {
                        // If we start hitting transient errors retry requests for a limited time.
                        log::warn!("Transient error when sending request to bitcoind: {}", e);
                        if i <= self.retries {
                            std::thread::sleep(Duration::from_secs(1));
                            log::debug!("Retrying RPC request to bitcoind: attempt #{}", i);
                        }
                        error = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(error.expect("Always set if we reach this point"))
    }

    fn try_request(&self, client: &Client, req: jsonrpc::Request) -> Result<Json, BitcoindError> {
        log::trace!("Sending to bitcoind: {:#?}", req);
        match client.send_request(req) {
            Ok(resp) => {
                let res = resp.result().map_err(BitcoindError::Server)?;
                log::trace!("Got from bitcoind: {:#?}", res);

                Ok(res)
            }
            Err(e) => Err(BitcoindError::Server(e)),
        }
    }

    fn make_request_inner(
        &self,
        client: &Client,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
        retry: bool,
    ) -> Result<Json, BitcoindError> {
        let req = client.build_request(method, params);
        if retry {
            self.retry(|| self.try_request(client, req.clone()))
        } else {
            self.try_request(client, req)
        }
    }

    fn make_request(
        &self,
        client: &Client,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<Json, BitcoindError> {
        self.make_request_inner(client, method, params, true)
    }

    // Make a request for which you don't expect a response. This is achieved by setting a very low
    // timeout on the connection.
    fn make_noreply_request(
        &self,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<(), BitcoindError> {
        match self.make_request_inner(&self.sendonly_client, method, params, false) {
            Ok(_) => Ok(()),
            Err(e) => {
                // A timeout error is expected, as that's our workaround to avoid blocking
                if e.is_timeout() {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    fn make_fallible_node_request(
        &self,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<Json, BitcoindError> {
        self.make_request(&self.node_client, method, params)
    }

    fn make_node_request(
        &self,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Json {
        self.make_request(&self.node_client, method, params)
            .expect("We must not fail to make a request for more than a minute")
    }

    fn make_wallet_request(
        &self,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Json {
        self.make_request(&self.watchonly_client, method, params)
            .expect("We must not fail to make a request for more than a minute")
    }

    fn make_faillible_wallet_request(
        &self,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<Json, BitcoindError> {
        self.make_request(&self.watchonly_client, method, params)
    }

    fn get_bitcoind_version(&self) -> u64 {
        self.make_node_request("getnetworkinfo", None)
            .get("version")
            .and_then(Json::as_u64)
            .expect("Missing or invalid 'version' in 'getnetworkinfo' result?")
    }

    fn get_network_bip70(&self) -> String {
        self.make_node_request("getblockchaininfo", None)
            .get("chain")
            .and_then(Json::as_str)
            .expect("Missing or invalid 'chain' in 'getblockchaininfo' result?")
            .to_string()
    }

    fn list_wallets(&self) -> Vec<String> {
        self.make_node_request("listwallets", None)
            .as_array()
            .expect("API break, 'listwallets' didn't return an array.")
            .iter()
            .map(|json_str| {
                json_str
                    .as_str()
                    .expect("API break: 'listwallets' contains a non-string value")
                    .to_string()
            })
            .collect()
    }

    // Get a warning from the result of a wallet command. It was modified in v25 so it's a bit
    // messy...
    fn warning_from_res(&self, res: &Json) -> Option<String> {
        // In v24, it's a "warning" field.
        if let Some(warning) = res.get("warning").and_then(Json::as_str) {
            if !warning.is_empty() {
                return Some(warning.to_string());
            }
        }

        // In v25 it becomes a "warnings" field...
        if let Some(warnings) = res.get("warnings").and_then(Json::as_array) {
            // FIXME: don't drop the other warnings if there are more than one.
            let first_actual_warning = warnings.iter().find_map(|w| {
                if let Some(w) = w.as_str() {
                    if !w.is_empty() {
                        return Some(w);
                    }
                }
                None
            });
            if let Some(warning) = first_actual_warning {
                return Some(warning.to_string());
            }
        }

        None
    }

    fn unload_wallet(&self, wallet_path: String) -> Option<String> {
        let res = self.make_node_request("unloadwallet", params!(Json::String(wallet_path),));
        self.warning_from_res(&res)
    }

    fn create_wallet(&self, wallet_path: String) -> Result<(), String> {
        // NOTE: we set load_on_startup to make sure the wallet will get updated before the
        // historical blocks are deleted in case the bitcoind is pruned.
        let res = self
            .make_fallible_node_request(
                "createwallet",
                params!(
                    Json::String(wallet_path),
                    Json::Bool(true),  // watchonly
                    Json::Bool(true),  // blank
                    Json::Null,        // passphrase
                    Json::Bool(false), // avoid_reuse
                    Json::Bool(true),  // descriptors
                    Json::Bool(true)   // load_on_startup
                ),
            )
            .map_err(|e| e.to_string())?;

        if let Some(warning) = self.warning_from_res(&res) {
            return Err(warning);
        }
        if res.get("name").is_none() {
            return Err("Unknown error when create watchonly wallet".to_string());
        }

        Ok(())
    }

    // Import the receive and change descriptors from the multipath descriptor to bitcoind.
    fn import_descriptor(&self, desc: &LianaDescriptor) -> Option<String> {
        let descriptors = [desc.receive_descriptor(), desc.change_descriptor()]
            .iter()
            .map(|desc| {
                serde_json::json!({
                    "desc": desc.to_string(),
                    "timestamp": "now",
                    "active": false,
                })
            })
            .collect();

        let res = self.make_wallet_request("importdescriptors", params!(Json::Array(descriptors)));
        let all_succeeded = res
            .as_array()
            .map(|results| {
                results
                    .iter()
                    .all(|res| res.get("success").and_then(Json::as_bool).unwrap_or(false))
            })
            .unwrap_or(false);
        if all_succeeded {
            None
        } else {
            Some(res.to_string())
        }
    }

    fn list_descriptors(&self) -> Vec<ListDescEntry> {
        self.make_wallet_request("listdescriptors", None)
            .get("descriptors")
            .and_then(Json::as_array)
            .expect("Missing or invalid 'descriptors' field in 'listdescriptors' response")
            .iter()
            .map(|elem| {
                let desc = elem
                    .get("desc")
                    .and_then(Json::as_str)
                    .expect(
                        "Missing or invalid 'desc' field in 'listdescriptors' response's entries",
                    )
                    .to_string();
                let range = elem.get("range").and_then(Json::as_array).map(|a| {
                    a.iter()
                        .map(|e| e.as_u64().expect("Invalid range index") as u32)
                        .collect::<Vec<_>>()
                        .try_into()
                        .expect("Range is always an array of size 2")
                });
                let timestamp = elem
                    .get("timestamp")
                    .and_then(Json::as_u64)
                    .expect("A valid timestamp is always present")
                    .try_into()
                    .expect("timestamp must fit");

                ListDescEntry {
                    desc,
                    range,
                    timestamp,
                }
            })
            .collect()
    }

    fn maybe_unload_watchonly_wallet(&self, watchonly_wallet_path: String) {
        while self.list_wallets().contains(&watchonly_wallet_path) {
            log::info!("Found a leftover watchonly wallet loaded on bitcoind. Removing it.");
            if let Some(e) = self.unload_wallet(watchonly_wallet_path.clone()) {
                log::error!(
                    "Unloading wallet '{}': '{}'",
                    &self.watchonly_wallet_path,
                    e
                );
            }
        }
    }

    /// Create the watchonly wallet on bitcoind, and import it the main descriptor.
    pub fn create_watchonly_wallet(
        &self,
        main_descriptor: &LianaDescriptor,
    ) -> Result<(), BitcoindError> {
        // Remove any leftover. This can happen if we delete the watchonly wallet but don't restart
        // bitcoind.
        self.maybe_unload_watchonly_wallet(self.watchonly_wallet_path.clone());

        // Now create the wallet and import the main descriptor.
        self.create_wallet(self.watchonly_wallet_path.clone())
            .map_err(|e| {
                BitcoindError::Wallet(self.watchonly_wallet_path.clone(), WalletError::Creating(e))
            })?;
        // TODO: make it return an error instead of an option.
        if let Some(err) = self.import_descriptor(main_descriptor) {
            return Err(BitcoindError::Wallet(
                self.watchonly_wallet_path.clone(),
                WalletError::ImportingDescriptor(err),
            ));
        }

        Ok(())
    }

    /// Load the watchonly wallet on bitcoind, if it isn't already.
    pub fn maybe_load_watchonly_wallet(&self) -> Result<(), BitcoindError> {
        if self.list_wallets().contains(&self.watchonly_wallet_path) {
            return Ok(());
        }
        let res = self.make_fallible_node_request(
            "loadwallet",
            params!(Json::String(self.watchonly_wallet_path.clone()),),
        );
        match res {
            Err(BitcoindError::Server(jsonrpc::Error::Rpc(ref e))) => {
                if e.code == -4 && e.message.to_lowercase().contains("wallet already loading") {
                    log::warn!("The watchonly wallet is already loading on bitcoind. Waiting for completion.");
                    loop {
                        thread::sleep(Duration::from_secs(3));
                        if self.list_wallets().contains(&self.watchonly_wallet_path) {
                            log::warn!("Watchonly wallet now loaded. Continuing.");
                            return Ok(());
                        }
                        log::debug!(
                            "Watchonly wallet loading still not complete. Waiting 3 more seconds."
                        );
                    }
                }
                res
            }
            r => r,
        }.map(|_| ())
    }

    /// Perform various non-wallet-related sanity checks on the bitcoind instance.
    pub fn node_sanity_checks(
        &self,
        config_network: bitcoin::Network,
        is_taproot: bool,
    ) -> Result<(), BitcoindError> {
        // Check the minimum supported bitcoind version
        let version = self.get_bitcoind_version();
        if version < MIN_BITCOIND_VERSION {
            return Err(BitcoindError::InvalidVersion(version));
        }
        if is_taproot && version < MIN_TAPROOT_BITCOIND_VERSION {
            return Err(BitcoindError::InvalidVersion(version));
        }

        // Check bitcoind is running on the right network
        let bitcoind_net = self.get_network_bip70();
        let bip70_net = match config_network {
            bitcoin::Network::Bitcoin => "main",
            bitcoin::Network::Testnet => "test",
            bitcoin::Network::Regtest => "regtest",
            bitcoin::Network::Signet => "signet",
            _ => "Unknown network, undefined at the time of writing",
        };
        if bitcoind_net != bip70_net {
            return Err(BitcoindError::NetworkMismatch(
                bip70_net.to_string(),
                bitcoind_net,
            ));
        }

        Ok(())
    }

    /// Perform various sanity checks of our watchonly wallet.
    pub fn wallet_sanity_checks(
        &self,
        main_descriptor: &LianaDescriptor,
    ) -> Result<(), BitcoindError> {
        // Check our watchonly wallet is loaded
        if self
            .list_wallets()
            .iter()
            .filter(|s| s == &&self.watchonly_wallet_path)
            .count()
            != 1
        {
            return Err(BitcoindError::Wallet(
                self.watchonly_wallet_path.clone(),
                WalletError::MissingOrTooManyWallet,
            ));
        }

        // Check our main descriptor is imported in this wallet.
        let receive_desc = main_descriptor.receive_descriptor();
        let change_desc = main_descriptor.change_descriptor();
        let desc_list: Vec<_> = self
            .list_descriptors()
            .into_iter()
            .filter_map(|entry| {
                match descriptor::Descriptor::<descriptor::DescriptorPublicKey>::from_str(
                    &entry.desc,
                ) {
                    Ok(desc) => Some(desc),
                    Err(e) => {
                        log::error!(
                            "Error deserializing descriptor: {}. Descriptor: {}.",
                            e,
                            entry.desc
                        );
                        None
                    }
                }
            })
            .collect();
        if !desc_list.iter().any(|desc| *receive_desc == *desc)
            || !desc_list.iter().any(|desc| *change_desc == *desc)
        {
            return Err(BitcoindError::Wallet(
                self.watchonly_wallet_path.clone(),
                WalletError::MissingDescriptor,
            ));
        }

        Ok(())
    }

    fn block_chain_info(&self) -> Json {
        self.make_node_request("getblockchaininfo", None)
    }

    pub fn sync_progress(&self) -> SyncProgress {
        // TODO: don't harass lianad, be smarter like in revaultd.
        let chain_info = self.block_chain_info();
        let percentage = chain_info
            .get("verificationprogress")
            .and_then(Json::as_f64)
            .expect("No valid 'verificationprogress' in getblockchaininfo response?");
        let headers = chain_info
            .get("headers")
            .and_then(Json::as_u64)
            .expect("No valid 'verificationprogress' in getblockchaininfo response?");
        let blocks = chain_info
            .get("blocks")
            .and_then(Json::as_u64)
            .expect("No valid 'blocks' in getblockchaininfo response?");
        SyncProgress {
            percentage,
            headers,
            blocks,
        }
    }

    pub fn chain_tip(&self) -> BlockChainTip {
        // We use getblockchaininfo to avoid a race between getblockcount and getblockhash
        let chain_info = self.block_chain_info();
        let hash = bitcoin::BlockHash::from_str(
            chain_info
                .get("bestblockhash")
                .and_then(Json::as_str)
                .expect("No valid 'bestblockhash' in 'getblockchaininfo' response?"),
        )
        .expect("Invalid blockhash from bitcoind?");
        let height: i32 = chain_info
            .get("blocks")
            .and_then(Json::as_i64)
            .expect("No valid 'blocks' in 'getblockchaininfo' response?")
            .try_into()
            .expect("Must fit by Bitcoin consensus");

        BlockChainTip { hash, height }
    }

    pub fn get_block_hash(&self, height: i32) -> Option<bitcoin::BlockHash> {
        Some(
            self.make_fallible_node_request("getblockhash", params!(Json::Number(height.into()),))
                .ok()?
                .as_str()
                .and_then(|s| bitcoin::BlockHash::from_str(s).ok())
                .expect("bitcoind must send valid block hashes"),
        )
    }

    pub fn list_since_block(&self, block_hash: &bitcoin::BlockHash) -> LSBlockRes {
        self.make_wallet_request(
            "listsinceblock",
            params!(
                Json::String(block_hash.to_string()),
                Json::Number(1.into()), // Default for min_confirmations for the returned
                Json::Bool(true),       // Whether to include watchonly
                Json::Bool(false), // Whether to include an array of txs that were removed in reorgs
                Json::Bool(true)   // Whether to include UTxOs treated as change.
            ),
        )
        .into()
    }

    pub fn get_transaction(&self, txid: &bitcoin::Txid) -> Option<GetTxRes> {
        // TODO: Maybe assert we got a -5 error, and not any other kind of error?
        self.make_faillible_wallet_request(
            "gettransaction",
            params!(Json::String(txid.to_string())),
        )
        .ok()
        .map(|res| res.into())
    }

    /// Efficient check that a coin is spent.
    pub fn is_spent(&self, op: &bitcoin::OutPoint) -> bool {
        // The result of gettxout is empty if the outpoint is spent.
        self.make_node_request(
            "gettxout",
            params!(
                Json::String(op.txid.to_string()),
                Json::Number(op.vout.into())
            ),
        )
        .get("bestblock")
        .is_none()
    }

    /// So, bitcoind has no API for getting the transaction spending a wallet UTXO. Instead we are
    /// therefore using a rather convoluted way to get it the other way around, since the spending
    /// transaction is actually *part of the wallet transactions*.
    /// So, what we do there is listing all outgoing transactions of the wallet since the last poll
    /// and iterating through each of those to check if it spends the transaction we are interested
    /// in (requiring an other RPC call for each!!).
    pub fn get_spender_txid(&self, spent_outpoint: &bitcoin::OutPoint) -> Option<bitcoin::Txid> {
        // Get the hash of the spent transaction's block parent. If the spent transaction is still
        // unconfirmed, just use the tip.
        let req = self.make_wallet_request(
            "gettransaction",
            params!(Json::String(spent_outpoint.txid.to_string())),
        );
        let list_since_height = match req.get("blockheight").and_then(Json::as_i64) {
            Some(h) => h as i32,
            None => self.chain_tip().height,
        };
        let block_hash = if let Ok(res) = self.make_fallible_node_request(
            "getblockhash",
            params!(Json::Number((list_since_height - 1).into())),
        ) {
            res.as_str()
                .expect("'getblockhash' result isn't a string")
                .to_string()
        } else {
            // Possibly a race.
            return None;
        };

        // Now we can get all transactions related to us since the spent transaction confirmed.
        // We'll use it to locate the spender.
        // TODO: merge this with the existing list_since_block method.
        let lsb_res = self.make_wallet_request(
            "listsinceblock",
            params!(
                Json::String(block_hash),
                Json::Number(1.into()), // Default for min_confirmations for the returned
                Json::Bool(true),       // Whether to include watchonly
                Json::Bool(false), // Whether to include an array of txs that were removed in reorgs
                Json::Bool(true)   // Whether to include UTxOs treated as change.
            ),
        );
        let transactions = lsb_res
            .get("transactions")
            .and_then(Json::as_array)
            .expect("tx array must be there");

        // Get the spent txid to ignore the entries about this transaction
        let spent_txid = spent_outpoint.txid.to_string();
        // We use a cache to avoid needless iterations, since listsinceblock returns an entry
        // per transaction output, not per transaction.
        let mut visited_txs = HashSet::with_capacity(transactions.len());
        for transaction in transactions {
            if transaction.get("category").and_then(Json::as_str) != Some("send") {
                continue;
            }

            let spending_txid = transaction
                .get("txid")
                .and_then(Json::as_str)
                .expect("A valid txid must be present");
            if visited_txs.contains(&spending_txid) || spent_txid == spending_txid {
                continue;
            } else {
                visited_txs.insert(spending_txid);
            }

            let gettx_res = self.make_wallet_request(
                "gettransaction",
                params!(
                    Json::String(spending_txid.to_string()),
                    Json::Bool(true), // watchonly
                    Json::Bool(true)  // verbose
                ),
            );
            let vin = gettx_res
                .get("decoded")
                .and_then(|d| d.get("vin").and_then(Json::as_array))
                .expect("A valid vin array must be present");

            for input in vin {
                let txid = input
                    .get("txid")
                    .and_then(Json::as_str)
                    .and_then(|t| bitcoin::Txid::from_str(t).ok())
                    .expect("A valid txid must be present");
                let vout = input
                    .get("vout")
                    .and_then(Json::as_u64)
                    .expect("A valid vout must be present") as u32;
                let input_outpoint = bitcoin::OutPoint { txid, vout };

                if spent_outpoint == &input_outpoint {
                    let spending_txid =
                        bitcoin::Txid::from_str(spending_txid).expect("Must be a valid txid");

                    // If the spending transaction is unconfirmed, there may more than one of them.
                    // Make sure to not return one that RBF'd.
                    let confs = gettx_res
                        .get("confirmations")
                        .and_then(Json::as_i64)
                        .expect("A valid number of confirmations must always be present.");
                    let conflicts = gettx_res
                        .get("walletconflicts")
                        .and_then(Json::as_array)
                        .expect("A valid list of wallet conflicts must always be present.");
                    if confs == 0 && !conflicts.is_empty() && !self.is_in_mempool(&spending_txid) {
                        log::debug!("Noticed '{}' as spending '{}', but is unconfirmed with conflicts and is not in mempool anymore. Discarding it.", &spending_txid, &spent_outpoint);
                        break;
                    }

                    return Some(spending_txid);
                }
            }
        }

        None
    }

    pub fn get_block_stats(&self, blockhash: bitcoin::BlockHash) -> Option<BlockStats> {
        let res = match self.make_fallible_node_request(
            "getblockheader",
            params!(Json::String(blockhash.to_string()),),
        ) {
            Ok(res) => res,
            Err(e) => {
                log::warn!("Error when fetching block header {}: {}", &blockhash, e);
                return None;
            }
        };
        let confirmations = res
            .get("confirmations")
            .and_then(Json::as_i64)
            .expect("Invalid confirmations in `getblockheader` response: not an i64")
            as i32;
        let previous_blockhash = res
            .get("previousblockhash")
            .and_then(Json::as_str)
            .map(|s| {
                bitcoin::BlockHash::from_str(s)
                    .expect("Invalid previousblockhash in `getblockheader` response")
            });
        let height = res
            .get("height")
            .and_then(Json::as_i64)
            .expect("Invalid height in `getblockheader` response: not an i64")
            as i32;
        let time = res
            .get("time")
            .and_then(Json::as_u64)
            .expect("Invalid timestamp in `getblockheader` response: not an u64")
            as u32;
        let median_time_past = res
            .get("mediantime")
            .and_then(Json::as_u64)
            .expect("Invalid median timestamp in `getblockheader` response: not an u64")
            as u32;
        Some(BlockStats {
            confirmations,
            previous_blockhash,
            height,
            blockhash,
            time,
            median_time_past,
        })
    }

    pub fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), BitcoindError> {
        self.make_fallible_node_request(
            "sendrawtransaction",
            params!(bitcoin::consensus::encode::serialize_hex(tx).into()),
        )?;
        Ok(())
    }

    // For the given descriptor strings check if they are imported at this timestamp in the
    // watchonly wallet.
    fn check_descs_timestamp(
        &self,
        descs: &[&Descriptor<DescriptorPublicKey>],
        timestamp: u32,
    ) -> bool {
        let current_descs = self.list_descriptors();

        for desc in descs {
            let present = current_descs_contain_desc_timestamp(&current_descs, desc, timestamp);
            if !present {
                return false;
            }
        }

        true
    }

    // Make sure the bitcoind has enough blocks to rescan up to this timestamp.
    fn check_prune_height(&self, timestamp: u32) -> Result<(), BitcoindError> {
        let chain_info = self.block_chain_info();
        let first_block_height = if let Some(h) = chain_info.get("pruneheight") {
            h
        } else {
            // The node isn't pruned
            return Ok(());
        };
        let prune_height: i32 = first_block_height
            .as_i64()
            .expect("Height must be an integer")
            .try_into()
            .expect("Height must fit in a i32");
        if let Some(tip) = self.tip_before_timestamp(timestamp) {
            if tip.height >= prune_height {
                return Ok(());
            }
        }
        Err(BitcoindError::RescanPastPruneHeight)
    }

    pub fn start_rescan(
        &mut self,
        desc: &LianaDescriptor,
        timestamp: u32,
    ) -> Result<(), BitcoindError> {
        // Re-import the receive and change descriptors to the watchonly wallet for the purpose of
        // rescanning.
        // The range of the newly imported descriptors supposed to update the existing ones must
        // have a range inclusive of the existing ones. We always use 0 as the initial index so
        // this is just determining the maximum index to use.
        let max_range = self
            .list_descriptors()
            .into_iter()
            // 1_000 is bitcoind's default and what we use at initial import.
            .fold(1_000, |range, entry| {
                cmp::max(range, entry.range.map(|r| r[1]).unwrap_or(0))
            });
        let descs = [
            desc.receive_descriptor().as_descriptor_public_key(),
            desc.change_descriptor().as_descriptor_public_key(),
        ];
        let desc_json: Vec<Json> = descs
            .iter()
            .map(|desc| {
                serde_json::json!({
                    "desc": desc.to_string(),
                    "timestamp": timestamp,
                    "active": false,
                    "range": max_range,
                })
            })
            .collect();

        // Have we pruned the blocks necessary to rescan down to this timestamp?
        // This check is necessary racy since bitcoind may prune these blocks in-between the check
        // here and the import below.
        self.check_prune_height(timestamp)?;

        // Since we don't wait for a response (which would make us block for the entire duration of
        // the rescan), we can't know for sure whether it was started successfully. So what we do
        // here is retrying a few times (since the noreply_request disables our generalistic retry
        // logic) until we notice the descriptors are successfully imported at this timestamp on
        // the watchonly wallet.
        // NOTE: if the rescan gets aborted through the 'abortrescan' RPC we won't see the
        // error and bitcoind will keep the new timestamps for the descriptors as if it had
        // successfully rescanned them.
        const NUM_RETRIES: usize = 10;
        let mut i = 0;
        loop {
            if let Err(e) = self
                .make_noreply_request("importdescriptors", params!(Json::Array(desc_json.clone())))
            {
                log::error!(
                    "Error when calling 'importdescriptors' for rescanning: {}",
                    e
                );
            }

            i += 1;
            if self.check_descs_timestamp(&descs, timestamp) {
                return Ok(());
            } else if i >= NUM_RETRIES {
                return Err(BitcoindError::StartRescan);
            } else {
                log::debug!("Sleeping a second before retrying to trigger the rescan");
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }

    /// Get the progress of the ongoing rescan, if there is any.
    pub fn rescan_progress(&self) -> Option<f64> {
        self.make_wallet_request("getwalletinfo", None)
            .get("scanning")
            // If no rescan is ongoing, it will fail cause it would be 'false'
            .and_then(Json::as_object)
            .and_then(|map| map.get("progress"))
            .and_then(Json::as_f64)
    }

    /// Get the height and hash of the last block with a timestamp below the given one.
    pub fn tip_before_timestamp(&self, timestamp: u32) -> Option<BlockChainTip> {
        block_before_date(
            timestamp,
            self.chain_tip(),
            |h| self.get_block_hash(h),
            |h| self.get_block_stats(h),
        )
    }

    /// Whether this transaction is in the mempool.
    pub fn is_in_mempool(&self, txid: &bitcoin::Txid) -> bool {
        self.mempool_entry(txid).is_some()
    }

    /// Get mempool entry of the given transaction.
    /// Returns `None` if it is not in the mempool.
    pub fn mempool_entry(&self, txid: &bitcoin::Txid) -> Option<MempoolEntry> {
        match self
            .make_fallible_node_request("getmempoolentry", params!(Json::String(txid.to_string())))
        {
            Ok(json) => Some(MempoolEntry::from(json)),
            Err(BitcoindError::Server(jsonrpc::Error::Rpc(jsonrpc::error::RpcError {
                code: -5,
                ..
            }))) => None,
            Err(e) => {
                panic!("Unexpected error returned by bitcoind {}", e);
            }
        }
    }

    /// Get the list of txids spending those outpoints in mempool.
    pub fn mempool_txs_spending_prevouts(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Vec<bitcoin::Txid> {
        let prevouts: Json = outpoints
            .iter()
            .map(|op| serde_json::json!({"txid": op.txid.to_string(), "vout": op.vout}))
            .collect();
        self.make_node_request("gettxspendingprevout", params!(prevouts))
            .as_array()
            .expect("Always returns an array")
            .iter()
            .filter_map(|e| {
                e.get("spendingtxid").map(|e| {
                    e.as_str()
                        .and_then(|s| bitcoin::Txid::from_str(s).ok())
                        .expect("Must be a valid txid if present")
                })
            })
            .collect()
    }

    /// Test whether raw transactions would be accepted by the mempool.
    pub fn test_mempool_accept(&self, rawtxs: Vec<String>) -> Vec<bool> {
        let hex_txs: Json = rawtxs.into_iter().map(|tx| serde_json::json!(tx)).collect();
        self.make_node_request("testmempoolaccept", params!(hex_txs))
            .as_array()
            .expect("Always returns an array")
            .iter()
            .map(|e| {
                e.get("allowed")
                    .and_then(|v| v.as_bool())
                    .expect("Each result must have an 'allowed' boolean")
            })
            .collect()
    }

    /// Stop bitcoind.
    pub fn stop(&self) {
        self.make_node_request("stop", None);
    }
}

/// Information about the block chain verification progress.
#[derive(Debug, Clone, Copy)]
pub struct SyncProgress {
    /// Chain verification progress as a percentage between 0 and 1.
    percentage: f64,
    /// Headers count for the best known tip.
    pub headers: u64,
    /// Number of blocks validated toward the best known tip.
    pub blocks: u64,
}

impl SyncProgress {
    pub fn new(percentage: f64, headers: u64, blocks: u64) -> Self {
        Self {
            percentage,
            headers,
            blocks,
        }
    }

    /// Get the verification progress, roundup up to four decimal places. This will not return
    /// 1.0 (ie 100% verification progress) until the verification is complete.
    pub fn rounded_up_progress(&self) -> f64 {
        let progress = roundup_progress(self.percentage);
        if progress == 1.0 && self.blocks != self.headers {
            // Don't return a 100% progress until we are actually done syncing.
            0.9999
        } else {
            progress
        }
    }

    pub fn is_complete(&self) -> bool {
        self.rounded_up_progress() == 1.0
    }
}

/// An entry in the 'listdescriptors' result.
#[derive(Debug, Clone)]
pub struct ListDescEntry {
    pub desc: String,
    pub range: Option<[u32; 2]>,
    pub timestamp: u32,
}

/// Whether `current_descs` contain the descriptor `desc` at `timestamp`.
///
/// Any descriptors in `current_descs` that cannot be parsed as
/// `Descriptor::<DescriptorPublicKey>` will be ignored.
fn current_descs_contain_desc_timestamp(
    current_descs: &[ListDescEntry],
    desc: &Descriptor<DescriptorPublicKey>,
    timestamp: u32,
) -> bool {
    current_descs
        .iter()
        .filter_map(|entry| {
            if let Ok(entry_desc) = Descriptor::<DescriptorPublicKey>::from_str(&entry.desc) {
                Some((entry_desc, entry.timestamp))
            } else {
                None
            }
        })
        .find(|(entry_desc, _)| entry_desc.to_string() == desc.to_string())
        .map(|(_, entry_timestamp)| entry_timestamp == timestamp)
        .unwrap_or(false)
}

/// A 'received' entry in the 'listsinceblock' result.
#[derive(Debug, Clone)]
pub struct LSBlockEntry {
    pub outpoint: bitcoin::OutPoint,
    pub amount: bitcoin::Amount,
    pub block_height: Option<i32>,
    pub address: bitcoin::Address<address::NetworkUnchecked>,
    pub parent_descs: Vec<descriptor::Descriptor<descriptor::DescriptorPublicKey>>,
    pub is_immature: bool,
}

impl From<&Json> for LSBlockEntry {
    fn from(json: &Json) -> LSBlockEntry {
        let txid = json
            .get("txid")
            .and_then(Json::as_str)
            .and_then(|s| bitcoin::Txid::from_str(s).ok())
            .expect("bitcoind can't give a bad block hash");
        let vout = json
            .get("vout")
            .and_then(Json::as_u64)
            .expect("bitcoind can't give a bad vout") as u32;
        let outpoint = bitcoin::OutPoint { txid, vout };

        // Must be a received entry, hence not negative.
        let amount = json
            .get("amount")
            .and_then(Json::as_f64)
            .and_then(|a| bitcoin::Amount::from_btc(a).ok())
            .expect("bitcoind won't give us a bad amount");
        let block_height = json
            .get("blockheight")
            .and_then(Json::as_i64)
            .map(|bh| bh as i32);

        let address = json
            .get("address")
            .and_then(Json::as_str)
            .and_then(|s| bitcoin::Address::from_str(s).ok())
            .expect("bitcoind can't give a bad address");
        let parent_descs = json
            .get("parent_descs")
            .and_then(Json::as_array)
            .and_then(|descs| {
                descs
                    .iter()
                    .map(|desc| {
                        desc.as_str()
                            .and_then(|s| descriptor::Descriptor::<_>::from_str(s).ok())
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .expect("bitcoind can't give invalid descriptors");

        let is_immature = json
            .get("category")
            .and_then(Json::as_str)
            .expect("must be present")
            == "immature";

        LSBlockEntry {
            outpoint,
            amount,
            block_height,
            address,
            parent_descs,
            is_immature,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LSBlockRes {
    pub received_coins: Vec<LSBlockEntry>,
}

impl From<Json> for LSBlockRes {
    fn from(json: Json) -> LSBlockRes {
        let received_coins = json
            .get("transactions")
            .and_then(Json::as_array)
            .expect("Array must be present")
            .iter()
            .filter_map(|j| {
                // From 'listunspent' help:
                //   "send"                  Transactions sent.
                //   "receive"               Non-coinbase transactions received.
                //   "generate"              Coinbase transactions received with more than 100 confirmations.
                //   "immature"              Coinbase transactions received with 100 or fewer confirmations.
                //   "orphan"                Orphaned coinbase transactions received.
                let category = j
                    .get("category")
                    .and_then(Json::as_str)
                    .expect("must be present");
                if ["receive", "generate", "immature"].contains(&category) {
                    let lsb_entry: LSBlockEntry = j.into();
                    Some(lsb_entry)
                } else {
                    None
                }
            })
            .collect();

        LSBlockRes { received_coins }
    }
}

#[derive(Debug, Clone)]
pub struct GetTxRes {
    pub conflicting_txs: Vec<bitcoin::Txid>,
    pub block: Option<Block>,
    pub tx: bitcoin::Transaction,
    pub is_coinbase: bool,
    pub confirmations: i32,
}

impl From<Json> for GetTxRes {
    fn from(json: Json) -> GetTxRes {
        let block_hash = json.get("blockhash").and_then(Json::as_str).map(|s| {
            bitcoin::BlockHash::from_str(s).expect("Invalid blockhash in `gettransaction` response")
        });
        let block_height = json
            .get("blockheight")
            .and_then(Json::as_i64)
            .map(|bh| bh as i32);
        let block_time = json
            .get("blocktime")
            .and_then(Json::as_u64)
            .map(|bt| bt as u32);
        let conflicting_txs = json
            .get("walletconflicts")
            .and_then(Json::as_array)
            .map(|array| {
                array
                    .iter()
                    .map(|v| {
                        bitcoin::Txid::from_str(v.as_str().expect("wrong json format")).unwrap()
                    })
                    .collect()
            });
        let block = match (block_hash, block_height, block_time) {
            (Some(hash), Some(height), Some(time)) => Some(Block { hash, time, height }),
            _ => None,
        };
        let hex = json
            .get("hex")
            .and_then(Json::as_str)
            .expect("Must be present in bitcoind response");
        let bytes = Vec::from_hex(hex).expect("bitcoind returned a wrong transaction format");
        let tx: bitcoin::Transaction = bitcoin::consensus::encode::deserialize(&bytes)
            .expect("bitcoind returned a wrong transaction format");
        let is_coinbase = json
            .get("generated")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let confirmations = json
            .get("confirmations")
            .and_then(Json::as_i64)
            .expect("Must be present in the response") as i32;

        GetTxRes {
            conflicting_txs: conflicting_txs.unwrap_or_default(),
            block,
            tx,
            is_coinbase,
            confirmations,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlockStats {
    pub confirmations: i32,
    pub previous_blockhash: Option<bitcoin::BlockHash>,
    pub blockhash: bitcoin::BlockHash,
    pub height: i32,
    pub time: u32,
    pub median_time_past: u32,
}

/// Make cached calls to bitcoind's `gettransaction`. It's useful for instance when coins have been
/// created or spent in a single transaction.
pub struct CachedTxGetter<'a> {
    bitcoind: &'a BitcoinD,
    cache: HashMap<bitcoin::Txid, GetTxRes>,
}

impl<'a> CachedTxGetter<'a> {
    pub fn new(bitcoind: &'a BitcoinD) -> Self {
        Self {
            bitcoind,
            cache: HashMap::new(),
        }
    }

    /// Query a transaction. Tries to get it from the cache and falls back to calling
    /// `gettransaction` on bitcoind. If both fail, returns None.
    pub fn get_transaction(&mut self, txid: &bitcoin::Txid) -> Option<GetTxRes> {
        // TODO: work around the borrow checker to avoid having to clone.
        if let Some(res) = self.cache.get(txid) {
            Some(res.clone())
        } else if let Some(res) = self.bitcoind.get_transaction(txid) {
            self.cache.insert(*txid, res);
            self.cache.get(txid).cloned()
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct MempoolEntry {
    pub vsize: u64,
    pub ancestor_vsize: u64,
    pub fees: MempoolEntryFees,
}

impl From<Json> for MempoolEntry {
    fn from(json: Json) -> MempoolEntry {
        let vsize = json
            .get("vsize")
            .and_then(Json::as_u64)
            .expect("Must be present in bitcoind response");
        let ancestor_vsize = json
            .get("ancestorsize")
            .and_then(Json::as_u64)
            .expect("Must be present in bitcoind response");
        let fees = json
            .get("fees")
            .as_ref()
            .expect("Must be present in bitcoind response")
            .into();

        MempoolEntry {
            vsize,
            ancestor_vsize,
            fees,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MempoolEntryFees {
    pub base: bitcoin::Amount,
    pub ancestor: bitcoin::Amount,
    pub descendant: bitcoin::Amount,
}

impl From<&&Json> for MempoolEntryFees {
    fn from(json: &&Json) -> MempoolEntryFees {
        let json = json.as_object().expect("fees must be an object");
        let base = json
            .get("base")
            .and_then(Json::as_f64)
            .and_then(|a| bitcoin::Amount::from_btc(a).ok())
            .expect("Must be present and a valid amount");
        let ancestor = json
            .get("ancestor")
            .and_then(Json::as_f64)
            .and_then(|a| bitcoin::Amount::from_btc(a).ok())
            .expect("Must be present and a valid amount");
        let descendant = json
            .get("descendant")
            .and_then(Json::as_f64)
            .and_then(|a| bitcoin::Amount::from_btc(a).ok())
            .expect("Must be present and a valid amount");
        MempoolEntryFees {
            base,
            ancestor,
            descendant,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rounded_up_progress() {
        assert_eq!(
            SyncProgress::new(0.6, 1_000, 1_000).rounded_up_progress(),
            0.6
        );
        assert_eq!(
            SyncProgress::new(0.67891, 1_000, 1_000).rounded_up_progress(),
            0.6789
        );
        assert_eq!(
            SyncProgress::new(0.99991, 1_000, 1_000).rounded_up_progress(),
            1.0
        );
        assert_eq!(
            SyncProgress::new(1.2, 1_000, 1_000).rounded_up_progress(),
            1.0
        );
        assert_eq!(
            SyncProgress::new(1.0, 1_000, 999).rounded_up_progress(),
            0.9999
        );
        // approximately year 2198
        assert_eq!(
            SyncProgress::new(1.0, 9999999, 9999998).rounded_up_progress(),
            0.9999
        );
        //bug or corrupted bitcond (blocks > headers)
        assert_eq!(
            SyncProgress::new(1.0, 999998, 999999).rounded_up_progress(),
            0.9999
        );
    }

    #[test]
    fn test_current_descs_contain_desc_timestamp() {
        // For simplicity, I've removed the checksums in the following descriptors as these will change
        // depending on whether `h` or `'` is used.

        // Simulate that `listdescriptors` returns an entry for receive and change descriptors, respectively.
        // Include one of the descriptors with two different timestamps and another invalid descriptor.
        let current_descs = vec![
            ListDescEntry {
                desc: "this is not a descriptor and will be ignored".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918410,
            },
            ListDescEntry {
                desc: "tr([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/0/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/2/*),older(65535)))".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918400,
            },
            ListDescEntry {
                desc: "tr([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/1/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/3/*),older(65535)))".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918380,
            },
            // same as receive descriptor above but with different timestamp.
            ListDescEntry {
                desc: "tr([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/0/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/2/*),older(65535)))".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918410,
            },
        ];

        // Create the Liana wallet descriptor:
        let desc = LianaDescriptor::from_str("tr([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/<0;1>/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/<2;3>/*),older(65535)))").unwrap();

        // The receive and change descriptors contain only `'`:
        assert_eq!(desc.receive_descriptor().to_string(), "tr([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/0/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/2/*),older(65535)))#xhrh0cvn".to_string());
        assert_eq!(desc.change_descriptor().to_string(), "tr([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/1/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/3/*),older(65535)))#6yyu2dsu".to_string());

        let recv_desc = desc.receive_descriptor().as_descriptor_public_key();
        let change_desc = desc.change_descriptor().as_descriptor_public_key();
        // For the receive descriptor, we don't get a match unless the timestamp matches the first occurrence.
        assert!(!current_descs_contain_desc_timestamp(
            &current_descs,
            recv_desc,
            1598918399
        ));
        assert!(!current_descs_contain_desc_timestamp(
            &current_descs,
            recv_desc,
            1598918401
        ));
        assert!(!current_descs_contain_desc_timestamp(
            &current_descs,
            change_desc,
            1598918381
        ));
        assert!(!current_descs_contain_desc_timestamp(
            &current_descs,
            recv_desc,
            1598918410 // this is the second timestamp for this descriptor
        ));
        // We only get a match when we use the first timestamp for each descriptor.
        assert!(current_descs_contain_desc_timestamp(
            &current_descs,
            recv_desc,
            1598918400
        ));
        assert!(current_descs_contain_desc_timestamp(
            &current_descs,
            change_desc,
            1598918380
        ));

        // If the `listdescriptors` response contains a mix of `h` and `'`, then there is still a match.
        let current_descs = vec![
            ListDescEntry {
                desc: "this is not a descriptor and will be ignored".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918410,
            },
            ListDescEntry {
                desc: "tr([1dce71b2/48h/1h/0h/2h]tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/0/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/2/*),older(65535)))".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918400,
            },
            ListDescEntry {
                desc: "tr([1dce71b2/48h/1h/0h/2h]tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/1/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/3/*),older(65535)))".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918380,
            },
        ];
        assert!(current_descs_contain_desc_timestamp(
            &current_descs,
            recv_desc,
            1598918400
        ));
        assert!(current_descs_contain_desc_timestamp(
            &current_descs,
            change_desc,
            1598918380
        ));

        // There is still a match if the checksum is included in the `listdescriptors` response.
        let current_descs = vec![
            ListDescEntry {
                desc: "this is not a descriptor and will be ignored".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918410,
            },
            ListDescEntry {
                desc: "tr([1dce71b2/48h/1h/0h/2h]tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/0/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/2/*),older(65535)))#2h7g2wme".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918400,
            },
            ListDescEntry {
                desc: "tr([1dce71b2/48h/1h/0h/2h]tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/1/*,and_v(v:pk([1dce71b2/48'/1'/0'/2']tpubDEeP3GefjqbaDTTaVAF5JkXWhoFxFDXQ9KuhVrMBViFXXNR2B3Lvme2d2AoyiKfzRFZChq2AGMNbU1qTbkBMfNv7WGVXLt2pnYXY87gXqcs/3/*),older(65535)))#kyer0m8k".to_string(),
                range: Some([0, 999]),
                timestamp: 1598918380,
            },
        ];
        assert!(current_descs_contain_desc_timestamp(
            &current_descs,
            recv_desc,
            1598918400
        ));
        assert!(current_descs_contain_desc_timestamp(
            &current_descs,
            change_desc,
            1598918380
        ));
    }
}
