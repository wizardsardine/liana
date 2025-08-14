//! # Liana commands
//!
//! External interface to the Liana daemon.

mod utils;

use crate::{
    bitcoin::BitcoinInterface,
    database::{Coin, DatabaseConnection, DatabaseInterface},
    miniscript::bitcoin::absolute::LockTime,
    payjoin::{
        db::{ReceiverPersister, SenderPersister},
        helpers::{fetch_ohttp_keys, FetchOhttpKeysError, OHTTP_RELAY, PAYJOIN_DIRECTORY},
        types::PayjoinStatus,
    },
    poller::PollerMessage,
    DaemonControl, VERSION,
};

pub use crate::database::{CoinStatus, LabelItem};

use liana::{
    descriptors,
    spend::{
        self, create_spend, AddrInfo, AncestorInfo, CandidateCoin, CreateSpendRes,
        SpendCreationError, SpendOutputAddress, SpendTxFees, TxGetter,
    },
};

use log::info;
use utils::{
    deser_addr_assume_checked, deser_amount_from_sats, deser_fromstr, deser_hex, ser_amount,
    ser_hex, ser_to_string,
};

use std::{
    collections::{hash_map, HashMap, HashSet},
    convert::{TryFrom, TryInto},
    fmt,
    str::FromStr,
    sync::{self, mpsc, Arc},
    time::SystemTime,
};

use miniscript::{
    bitcoin::{
        self, address,
        bip32::{self, ChildNumber},
        psbt::Psbt,
    },
    psbt::PsbtExt,
};
use payjoin::{
    bitcoin::{key::Secp256k1, FeeRate},
    receive::v2::{replay_event_log as replay_receiver_event_log, Receiver, UninitializedReceiver},
    send::v2::{replay_event_log as replay_sender_event_log, SenderBuilder},
    Uri, UriExt, Url,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    NoOutpointForSelfSend,
    InvalidFeerate(/* sats/vb */ u64),
    UnknownOutpoint(bitcoin::OutPoint),
    AlreadySpent(bitcoin::OutPoint),
    ImmatureCoinbase(bitcoin::OutPoint),
    Address(bitcoin::address::ParseError),
    SpendCreation(SpendCreationError),
    InsufficientFunds(
        /* in value */ bitcoin::Amount,
        /* out value */ Option<bitcoin::Amount>,
        /* target feerate */ u64,
    ),
    UnknownSpend(bitcoin::Txid),
    // FIXME: when upgrading Miniscript put the actual error there
    SpendFinalization(String),
    TxBroadcast(String),
    AlreadyRescanning,
    InsaneRescanTimestamp(u32),
    /// An error that might occur in the racy rescan triggering logic.
    RescanTrigger(String),
    RecoveryNotAvailable,
    // Include timelock in error as it may not have been set explicitly by the user.
    OutpointNotRecoverable(bitcoin::OutPoint, /* timelock */ u16),
    /// Overflowing or unhardened derivation index.
    InvalidDerivationIndex,
    RbfError(RbfErrorInfo),
    EmptyFilterList,
    FailedToFetchOhttpKeys(FetchOhttpKeysError),
    // Same FIXME as `SpendFinalization`
    FailedToPostOriginalPayjoinProposal(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoOutpointForSelfSend => {
                write!(f, "No provided outpoint for self-send. Need at least one.")
            }
            Self::InvalidFeerate(sats_vb) => write!(f, "Invalid feerate: {} sats/vb.", sats_vb),
            Self::AlreadySpent(op) => write!(f, "Coin at '{}' is already spent.", op),
            Self::ImmatureCoinbase(op) => write!(
                f,
                "Coin at '{}' is from an immature coinbase transaction.",
                op
            ),
            Self::UnknownOutpoint(op) => write!(f, "Unknown outpoint '{}'.", op),
            Self::Address(e) => write!(f, "Address error: {}", e),
            Self::SpendCreation(e) => write!(f, "Creating spend: {}", e),
            Self::InsufficientFunds(in_val, out_val, feerate) => {
                if let Some(out_val) = out_val {
                    write!(
                    f,
                    "Cannot create a {} sat/vb transaction with input value {} and output value {}",
                    feerate, in_val, out_val
                )
                } else {
                    write!(
                        f,
                        "Not enough fund to create a {} sat/vb transaction with input value {}",
                        feerate, in_val
                    )
                }
            }
            Self::UnknownSpend(txid) => write!(f, "Unknown spend transaction '{}'.", txid),
            Self::SpendFinalization(e) => {
                write!(f, "Failed to finalize the spend transaction PSBT: '{}'.", e)
            }
            Self::TxBroadcast(e) => write!(f, "Failed to broadcast transaction: '{}'.", e),
            Self::AlreadyRescanning => write!(
                f,
                "There is already a rescan ongoing. Please wait for it to complete first."
            ),
            Self::InsaneRescanTimestamp(t) => write!(f, "Insane timestamp '{}'.", t),
            Self::RescanTrigger(s) => write!(f, "Error while starting rescan: '{}'", s),
            Self::RecoveryNotAvailable => write!(
                f,
                "No coin currently spendable through this timelocked recovery path."
            ),
            Self::OutpointNotRecoverable(op, t) => write!(
                f,
                "Coin at '{}' is not recoverable with timelock '{}'",
                op, t
            ),
            Self::InvalidDerivationIndex => {
                write!(f, "Unhardened or overflowing BIP32 derivation index.")
            }
            Self::RbfError(e) => write!(f, "RBF error: '{}'.", e),
            Self::EmptyFilterList => write!(f, "Filter list is empty, should supply None instead."),
            Self::FailedToFetchOhttpKeys(e) => write!(f, "Failed to fetch OHTTP keys: '{}'.", e),
            Self::FailedToPostOriginalPayjoinProposal(e) => {
                write!(f, "Failed to post original payjoin proposal: '{}'.", e)
            }
        }
    }
}

impl std::error::Error for CommandError {}

impl From<SpendCreationError> for CommandError {
    fn from(e: SpendCreationError) -> Self {
        CommandError::SpendCreation(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RbfErrorInfo {
    MissingFeerate,
    SuperfluousFeerate,
    TooLowFeerate(u64, u64),
    NotSignaling,
}

impl fmt::Display for RbfErrorInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Self::MissingFeerate => {
                write!(f, "A feerate must be provided if not creating a cancel.")
            }
            Self::SuperfluousFeerate => {
                write!(f, "A feerate must not be provided if creating a cancel. We'll always use the smallest one which satisfies the RBF rules.")
            }
            Self::TooLowFeerate(r, m) => {
                write!(f, "Feerate {} too low for minimum feerate {}.", r, m)
            }
            Self::NotSignaling => write!(f, "Replacement candidate does not signal for RBF."),
        }
    }
}

/// A wallet transaction getter which fetches the transaction from our database backend with a cache
/// to avoid needless redundant calls. Note the cache holds an Option<> so we also avoid redundant
/// calls when the txid isn't known by our database backend.
struct DbTxGetter<'a> {
    db: &'a sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    cache: HashMap<bitcoin::Txid, Option<bitcoin::Transaction>>,
}

impl<'a> DbTxGetter<'a> {
    pub fn new(db: &'a sync::Arc<sync::Mutex<dyn DatabaseInterface>>) -> Self {
        Self {
            db,
            cache: HashMap::new(),
        }
    }
}

impl<'a> TxGetter for DbTxGetter<'a> {
    fn get_tx(&mut self, txid: &bitcoin::Txid) -> Option<bitcoin::Transaction> {
        if let hash_map::Entry::Vacant(entry) = self.cache.entry(*txid) {
            let tx = self
                .db
                .connection()
                .list_wallet_transactions(&[*txid])
                .pop()
                .map(|(tx, _, _)| tx);
            entry.insert(tx);
        }
        self.cache.get(txid).cloned().flatten()
    }
}

fn coin_to_candidate(
    coin: &Coin,
    must_select: bool,
    sequence: Option<bitcoin::Sequence>,
    ancestor_info: Option<AncestorInfo>,
) -> CandidateCoin {
    CandidateCoin {
        outpoint: coin.outpoint,
        amount: coin.amount,
        deriv_index: coin.derivation_index,
        is_change: coin.is_change,
        must_select,
        sequence,
        ancestor_info,
    }
}

impl DaemonControl {
    // Get the derived descriptor for this coin
    fn derived_desc(&self, coin: &Coin) -> descriptors::DerivedSinglePathLianaDesc {
        let desc = if coin.is_change {
            self.config.main_descriptor.change_descriptor()
        } else {
            self.config.main_descriptor.receive_descriptor()
        };
        desc.derive(coin.derivation_index, &self.secp)
    }

    // Check whether this address is valid for the network we are operating on.
    fn validate_address(
        &self,
        addr: bitcoin::Address<address::NetworkUnchecked>,
    ) -> Result<bitcoin::Address, CommandError> {
        // NOTE: signet uses testnet addresses, and legacy addresses on regtest use testnet
        // encoding.
        addr.require_network(self.config.bitcoin_config.network)
            .map_err(CommandError::Address)
    }

    // Get details about this address, if we know about it.
    fn addr_info(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
        addr: &bitcoin::Address,
    ) -> Option<AddrInfo> {
        db_conn
            .derivation_index_by_address(addr)
            .map(|(index, is_change)| AddrInfo { index, is_change })
    }

    // Create an address to be used in an output of a spend transaction.
    fn spend_addr(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
        addr: bitcoin::Address,
    ) -> SpendOutputAddress {
        SpendOutputAddress {
            info: self.addr_info(db_conn, &addr),
            addr,
        }
    }

    // Get the change address for the next derivation index.
    // The spend may not have a change output, so we don't update the DB value yet.
    fn next_change_addr(&self, db_conn: &mut Box<dyn DatabaseConnection>) -> SpendOutputAddress {
        let index = db_conn.change_index();
        let next_index = index
            .increment()
            .expect("Must not get into hardened territory");
        let desc = self
            .config
            .main_descriptor
            .change_descriptor()
            .derive(next_index, &self.secp);
        let addr = desc.address(self.config.bitcoin_config.network);
        SpendOutputAddress {
            addr,
            info: Some(AddrInfo {
                index: next_index,
                is_change: true,
            }),
        }
    }

    // If we detect the given address as ours, and it has a higher derivation index than our last
    // derivation index, update our last derivation index to the given value.
    fn maybe_increase_last_deriv_index(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
        addr_info: &Option<AddrInfo>,
    ) {
        if let Some(AddrInfo { index, is_change }) = addr_info {
            if *is_change && db_conn.change_index() < *index {
                db_conn.set_change_index(*index, &self.secp);
            } else if !is_change && db_conn.receive_index() < *index {
                db_conn.set_receive_index(*index, &self.secp);
            }
        }
    }

    // Pass relevant values to the spend module function of same name.
    fn anti_fee_sniping_locktime(&self) -> LockTime {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("time measured now cannot be before unix epoch");
        let tip_time = self.bitcoin.tip_time();
        let tip_height: u32 = self
            .bitcoin
            .chain_tip()
            .height
            .try_into()
            .expect("block height must fit in u32");
        spend::anti_fee_sniping_locktime(now, tip_height, tip_time)
    }
}

impl DaemonControl {
    /// Get information about the current state of the daemon
    pub fn get_info(&self) -> GetInfoResult {
        let mut db_conn = self.db.connection();
        let block_height = db_conn.chain_tip().map(|tip| tip.height).unwrap_or(0);
        let wallet = db_conn.wallet();
        let receive_index: u32 = db_conn.receive_index().into();
        let change_index: u32 = db_conn.change_index().into();
        let rescan_progress = wallet
            .rescan_timestamp
            .map(|_| self.bitcoin.rescan_progress().unwrap_or(1.0));
        GetInfoResult {
            version: VERSION.to_string(),
            network: self.config.bitcoin_config.network,
            block_height,
            sync: self.bitcoin.sync_progress().rounded_up_progress(),
            descriptors: GetInfoDescriptors {
                main: self.config.main_descriptor.clone(),
            },
            rescan_progress,
            timestamp: wallet.timestamp,
            last_poll_timestamp: wallet.last_poll_timestamp,
            receive_index,
            change_index,
        }
    }

    /// Get a new deposit address. This will always generate a new deposit address, regardless of
    /// whether it was actually used.
    pub fn get_new_address(&self) -> GetAddressResult {
        let mut db_conn = self.db.connection();
        let index = db_conn.receive_index();
        let new_index = index
            .increment()
            .expect("Can't get into hardened territory");
        db_conn.set_receive_index(new_index, &self.secp);
        let address = self
            .config
            .main_descriptor
            .receive_descriptor()
            .derive(new_index, &self.secp)
            .address(self.config.bitcoin_config.network);
        GetAddressResult::new(address, new_index, None)
    }

    pub fn receive_payjoin(&self) -> Result<GetAddressResult, CommandError> {
        let mut db_conn = self.db.connection();

        let ohttp_keys = if let Some(entry) = db_conn.payjoin_get_ohttp_keys(OHTTP_RELAY) {
            entry.1
        } else {
            let ohttp_keys =
                std::thread::spawn(move || fetch_ohttp_keys(OHTTP_RELAY, PAYJOIN_DIRECTORY))
                    .join()
                    .unwrap()
                    .map_err(CommandError::FailedToFetchOhttpKeys)?;
            db_conn.payjoin_save_ohttp_keys(OHTTP_RELAY, ohttp_keys.clone());
            ohttp_keys
        };

        let index = db_conn.receive_index();
        let new_index = index
            .increment()
            .expect("Can't get into hardened territory");
        db_conn.set_receive_index(new_index, &self.secp);
        let address = self
            .config
            .main_descriptor
            .receive_descriptor()
            .derive(new_index, &self.secp)
            .address(self.config.bitcoin_config.network);

        let persister = ReceiverPersister::new(Arc::new(self.db.clone()));
        let session = Receiver::<UninitializedReceiver>::create_session(
            address.clone(),
            PAYJOIN_DIRECTORY,
            ohttp_keys.clone(),
            None,
        )
        .save(&persister)
        .unwrap();

        Ok(GetAddressResult::new(
            address,
            new_index,
            Some(Url::from_str(session.pj_uri().to_string().as_str()).expect("Should be valid")),
        ))
    }

    /// Initiate a payjoin sender
    // TODO bip21 should be a uri not a string
    // TODO: min fee rate should be a param
    pub fn init_payjoin_sender(&self, bip21: String, psbt: &Psbt) -> Result<(), CommandError> {
        // TODO: validate bip21 in uri
        let uri = Uri::try_from(bip21.clone())
            .map_err(|e| format!("Failed to create URI from BIP21: {}", e))
            .unwrap();
        let uri = uri.assume_checked();
        let uri = uri
            .check_pj_supported()
            .map_err(|_| "URI does not support Payjoin".to_string())
            .unwrap();

        let mut signed_psbt = psbt.clone();
        signed_psbt
            .finalize_mut(&Secp256k1::verification_only())
            // Just display the first error
            .map_err(|e| CommandError::FailedToPostOriginalPayjoinProposal(e[0].to_string()))?;

        let mut original_psbt = psbt.clone();
        for (index, input) in original_psbt.inputs.iter_mut().enumerate() {
            input.partial_sigs = Default::default();
            input.final_script_witness = signed_psbt.inputs[index].final_script_witness.clone();
        }

        let original_txid = original_psbt.unsigned_tx.compute_txid();
        let persister = SenderPersister::new(Arc::new(self.db.clone()), &original_txid);
        let _sender = SenderBuilder::new(original_psbt.clone(), uri)
            .build_recommended(FeeRate::BROADCAST_MIN)
            .save(&persister)
            .unwrap();

        Ok(())
    }

    /// Get Payjoin URI (BIP21) and its sender/receiver status by txid
    pub fn get_payjoin_info(&self, txid: &bitcoin::Txid) -> Result<PayjoinStatus, CommandError> {
        let mut db_conn = self.db.connection();
        info!("Getting payjoin info for txid: {:?}", txid);
        if let Some(session_id) = db_conn.get_payjoin_receiver_session_id_from_txid(txid) {
            let persister =
                ReceiverPersister::from_id(Arc::new(self.db.clone()), session_id.clone());
            let (state, _) = replay_receiver_event_log(&persister).unwrap();
            return Ok(state.into());
        }

        if let Some(session_id) = db_conn.get_payjoin_sender_session_id_from_txid(txid) {
            log::info!("Checking sender session: {:?}", session_id);
            let persister = SenderPersister::from_id(Arc::new(self.db.clone()), session_id.clone());
            let (state, _) = replay_sender_event_log(&persister).unwrap();
            log::info!("Sender state: {:?}", state);
            return Ok(state.into());
        }

        Ok(PayjoinStatus::Unknown)
    }

    /// Update derivation indexes
    pub fn update_deriv_indexes(
        &self,
        receive: Option<u32>,
        change: Option<u32>,
    ) -> Result<UpdateDerivIndexesResult, CommandError> {
        let mut db_conn = self.db.connection();

        const MAX_INCREMENT_GAP: u32 = 1_000;

        let db_receive = db_conn.receive_index().into();
        let mut final_receive = db_receive;

        let db_change = db_conn.change_index().into();
        let mut final_change = db_change;

        if let Some(index) = receive {
            ChildNumber::from_normal_idx(index)
                .map_err(|_| CommandError::InvalidDerivationIndex)?;
            if index > db_receive {
                let delta = (index - db_receive).min(MAX_INCREMENT_GAP);
                let index = db_receive + delta;
                final_receive = index;
                match ChildNumber::from_normal_idx(index) {
                    Ok(i) => {
                        db_conn.set_receive_index(i, &self.secp);
                    }
                    Err(_) => return Err(CommandError::InvalidDerivationIndex),
                };
            }
        }

        if let Some(index) = change {
            ChildNumber::from_normal_idx(index)
                .map_err(|_| CommandError::InvalidDerivationIndex)?;
            if index > db_change {
                let delta = (index - db_change).min(MAX_INCREMENT_GAP);
                let index = db_change + delta;
                final_change = index;
                match ChildNumber::from_normal_idx(index) {
                    Ok(i) => {
                        db_conn.set_change_index(i, &self.secp);
                    }
                    Err(_) => return Err(CommandError::InvalidDerivationIndex),
                };
            }
        }

        Ok(UpdateDerivIndexesResult {
            receive: final_receive,
            change: final_change,
        })
    }

    /// list addresses
    pub fn list_addresses(
        &self,
        start_index: Option<u32>,
        count: Option<u32>,
    ) -> Result<ListAddressesResult, CommandError> {
        let mut db_conn = self.db.connection();
        let receive_index: u32 = db_conn.receive_index().into();
        let change_index: u32 = db_conn.change_index().into();

        // If a start index isn't provided, we derive from index 0. Make sure the provided index is
        // unhardened.
        let start_index = bip32::ChildNumber::from_normal_idx(start_index.unwrap_or(0))
            .map_err(|_| CommandError::InvalidDerivationIndex)?;
        let start_index_u32: u32 = start_index.into();

        // Derive the end index (ie, the first index to not be returned) from the count of
        // addresses to provide. If no count was passed, use the next derivation index between
        // change and receive as end index.
        let end_index = if let Some(c) = count {
            start_index_u32
                .checked_add(c)
                .ok_or(CommandError::InvalidDerivationIndex)?
        } else {
            // `end_index` will not be included so add 1 in order to include the last used index.
            receive_index
                .max(change_index)
                .checked_add(1)
                .ok_or(CommandError::InvalidDerivationIndex)?
        };

        // Derive all receive and change addresses for the queried range.
        let addresses: Result<Vec<AddressInfo>, CommandError> = (start_index_u32..end_index)
            .map(|index| {
                let child = bip32::ChildNumber::from_normal_idx(index)
                    .map_err(|_| CommandError::InvalidDerivationIndex)?;

                let receive = self
                    .config
                    .main_descriptor
                    .receive_descriptor()
                    .derive(child, &self.secp)
                    .address(self.config.bitcoin_config.network);

                let change = self
                    .config
                    .main_descriptor
                    .change_descriptor()
                    .derive(child, &self.secp)
                    .address(self.config.bitcoin_config.network);

                Ok(AddressInfo {
                    index,
                    receive,
                    change,
                })
            })
            .collect();
        Ok(ListAddressesResult::new(addresses?))
    }

    /// List revealed addresses. Addresses will be returned in order of
    /// descending derivation index.
    ///
    /// # Parameters
    ///
    /// - `is_change`: set to `false` to return receive addresses and `true` for change addresses.
    ///
    /// - `exclude_used`:  set to `true` to return only those revealed addresses that
    ///   are unused by any coins in the wallet.
    ///
    /// - `limit`: the maximum number of addresses to return.
    ///
    /// - `start_index`: the derivation index from which to start listing addresses. As addresses are
    ///   returned in descending order, `start_index` is the highest index that can be returned.
    ///   If set to `None`, then addresses will be returned starting from the last revealed index.
    ///   As there are no revealed addresses with a derivation index higher than the last revealed index,
    ///   setting this parameter to a higher value will be the same as setting it to `None`.
    pub fn list_revealed_addresses(
        &self,
        is_change: bool,
        exclude_used: bool,
        limit: usize,
        start_index: Option<ChildNumber>,
    ) -> Result<ListRevealedAddressesResult, CommandError> {
        let mut db_conn = self.db.connection();

        let (desc, last_revealed) = if is_change {
            (
                self.config.main_descriptor.change_descriptor(),
                db_conn.change_index(),
            )
        } else {
            (
                self.config.main_descriptor.receive_descriptor(),
                db_conn.receive_index(),
            )
        };

        // Determine the index to start deriving addresses from, ensuring it is not higher than the last revealed.
        let start_index = start_index.unwrap_or(last_revealed).min(last_revealed);

        // Count how many times each (used) address has been used.
        let mut used_counts = HashMap::<ChildNumber, u32>::new();
        // TODO: consider adding DB method to get coins or used indices by index range.
        for coin in db_conn.coins(&[], &[]).values() {
            if coin.is_change == is_change && coin.derivation_index <= start_index {
                *used_counts.entry(coin.derivation_index).or_insert(0) += 1;
            }
        }

        let mut addresses = Vec::<_>::with_capacity(limit);
        let mut continue_from = None;
        // This will store (index, address) pairs.
        let mut derived_addresses = Vec::<_>::with_capacity(limit);
        // Iterate in descending order.
        for i in (0..=start_index.into()).rev() {
            let index = ChildNumber::from(i);
            if derived_addresses.len() == limit {
                // We've reached the limit. There may be more addresses to list using pagination.
                continue_from = Some(index);
                break;
            }
            if !exclude_used || !used_counts.contains_key(&index) {
                let addr = desc
                    .derive(index, &self.secp)
                    .address(self.config.bitcoin_config.network);
                derived_addresses.push((index, addr));
            }
        }
        // Now get the labels from DB (in multiple chunks).
        let mut labels = HashMap::<String, String>::with_capacity(derived_addresses.len());
        const CHUNK_SIZE: usize = 100;
        for chunk in derived_addresses.chunks(CHUNK_SIZE) {
            let items = chunk
                .iter()
                .map(|(_, addr)| LabelItem::Address(addr.clone()))
                .collect::<HashSet<_>>();
            labels.extend(db_conn.labels(&items));
        }
        for (index, address) in derived_addresses {
            let label = labels.get(&address.to_string()).cloned();
            addresses.push(ListRevealedAddressesEntry {
                index,
                address,
                label,
                used_count: *used_counts.get(&index).unwrap_or(&0),
            });
        }
        Ok(ListRevealedAddressesResult {
            addresses,
            continue_from,
        })
    }

    /// Get a list of all known coins, optionally by status and/or outpoint.
    pub fn list_coins(
        &self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> ListCoinsResult {
        let mut db_conn = self.db.connection();
        let coins: Vec<ListCoinsEntry> = db_conn
            .coins(statuses, outpoints)
            .into_values()
            .map(|coin| {
                let Coin {
                    amount,
                    outpoint,
                    block_info,
                    spend_txid,
                    spend_block,
                    is_immature,
                    is_change,
                    is_from_self,
                    derivation_index,
                    ..
                } = coin;
                let spend_info = spend_txid.map(|txid| LCSpendInfo {
                    txid,
                    height: spend_block.map(|b| b.height),
                });
                let block_height = block_info.map(|b| b.height);
                let address = self
                    .derived_desc(&coin)
                    .address(self.config.bitcoin_config.network);
                ListCoinsEntry {
                    address,
                    amount,
                    derivation_index,
                    outpoint,
                    block_height,
                    spend_info,
                    is_immature,
                    is_change,
                    is_from_self,
                }
            })
            .collect();
        ListCoinsResult { coins }
    }

    pub fn create_spend(
        &self,
        destinations: &HashMap<bitcoin::Address<bitcoin::address::NetworkUnchecked>, u64>,
        coins_outpoints: &[bitcoin::OutPoint],
        feerate_vb: u64,
        change_address: Option<bitcoin::Address<bitcoin::address::NetworkUnchecked>>,
    ) -> Result<CreateSpendResult, CommandError> {
        let is_self_send = destinations.is_empty();
        // For self-send, the coins must be specified.
        if is_self_send && coins_outpoints.is_empty() {
            return Err(CommandError::NoOutpointForSelfSend);
        }
        if feerate_vb < 1 {
            return Err(CommandError::InvalidFeerate(feerate_vb));
        }
        let mut db_conn = self.db.connection();
        let mut tx_getter = DbTxGetter::new(&self.db);

        // Prepare the destination addresses.
        let mut destinations_checked = Vec::with_capacity(destinations.len());
        for (address, value_sat) in destinations {
            let address = self.validate_address(address.clone())?;
            let amount = bitcoin::Amount::from_sat(*value_sat);
            let address = self.spend_addr(&mut db_conn, address);
            destinations_checked.push((address, amount));
        }

        // The change address to be used if a change output needs to be created. It may be
        // specified by the caller (for instance for the purpose of a sweep, or to avoid us
        // creating a new change address on every call).
        let change_address = change_address
            .map(|addr| {
                Ok::<_, CommandError>(self.spend_addr(&mut db_conn, self.validate_address(addr)?))
            })
            .transpose()?
            .unwrap_or_else(|| self.next_change_addr(&mut db_conn));

        // The candidate coins will be either all optional or all mandatory.
        // If no coins have been specified, then coins will be selected automatically for
        // the spend from a set of optional candidates.
        // Otherwise, only the specified coins will be used, all as mandatory candidates.
        let candidate_coins: Vec<CandidateCoin> = if coins_outpoints.is_empty() {
            // From our unconfirmed coins, we only include those that are from self
            // since unconfirmed external deposits are more at risk of being dropped
            // unexpectedly from the mempool as they are beyond the user's control.
            db_conn
                .coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                .into_iter()
                .filter_map(|(op, c)| {
                    if c.block_info.is_some() {
                        Some((c, None)) // confirmed coins have no ancestor info
                    } else if c.is_from_self {
                        // In case the mempool_entry is None, the coin will be included without
                        // any ancestor info.
                        Some((
                            c,
                            self.bitcoin
                                .mempool_entry(&op.txid)
                                .map(|info| AncestorInfo {
                                    vsize: info.ancestor_vsize,
                                    fee: info
                                        .fees
                                        .ancestor
                                        .to_sat()
                                        .try_into()
                                        .expect("fee in sat should fit in u32"),
                                }),
                        ))
                    } else {
                        None
                    }
                })
                .map(|(c, ancestor_info)| {
                    coin_to_candidate(
                        &c,
                        /*must_select=*/ false,
                        /*sequence=*/ None,
                        ancestor_info,
                    )
                })
                .collect()
        } else {
            // Query from DB and sanity check the provided coins to spend.
            let coins = db_conn.coins(&[], coins_outpoints);
            for op in coins_outpoints {
                let coin = coins.get(op).ok_or(CommandError::UnknownOutpoint(*op))?;
                if coin.is_spent() {
                    return Err(CommandError::AlreadySpent(*op));
                }
                if coin.is_immature {
                    return Err(CommandError::ImmatureCoinbase(*op));
                }
            }
            coins
                .into_iter()
                .map(|(op, c)| {
                    let ancestor_info = if c.block_info.is_none() {
                        // We include any non-change coins here as they have been selected by the caller.
                        // If the unconfirmed coin's transaction is no longer in the mempool, keep the
                        // coin as a candidate but without any ancestor info (same as confirmed candidate).
                        self.bitcoin
                            .mempool_entry(&op.txid)
                            .map(|info| AncestorInfo {
                                vsize: info.ancestor_vsize,
                                fee: info
                                    .fees
                                    .ancestor
                                    .to_sat()
                                    .try_into()
                                    .expect("fee in sat should fit in u32"),
                            })
                    } else {
                        None
                    };
                    coin_to_candidate(
                        &c,
                        /*must_select=*/ true,
                        /*sequence=*/ None,
                        ancestor_info,
                    )
                })
                .collect()
        };

        // Create the PSBT. If there was no error in doing so make sure to update our next
        // derivation index in case any address in the transaction outputs was ours and from the
        // future.
        let change_info = change_address.info;
        let locktime = self.anti_fee_sniping_locktime();
        let CreateSpendRes {
            psbt,
            has_change,
            warnings,
        } = match create_spend(
            &self.config.main_descriptor,
            &self.secp,
            &mut tx_getter,
            &destinations_checked,
            &candidate_coins,
            SpendTxFees::Regular(feerate_vb),
            change_address,
            locktime,
        ) {
            Ok(res) => res,
            Err(SpendCreationError::CoinSelection(e)) => {
                return Ok(CreateSpendResult::InsufficientFunds { missing: e.missing });
            }
            Err(e) => {
                return Err(e.into());
            }
        };
        for (addr, _) in destinations_checked {
            self.maybe_increase_last_deriv_index(&mut db_conn, &addr.info);
        }
        if has_change {
            self.maybe_increase_last_deriv_index(&mut db_conn, &change_info);
        }

        Ok(CreateSpendResult::Success {
            psbt,
            warnings: warnings.iter().map(|w| w.to_string()).collect(),
        })
    }

    pub fn update_spend(&self, mut psbt: Psbt) -> Result<(), CommandError> {
        let mut db_conn = self.db.connection();
        let tx = &psbt.unsigned_tx;

        // If the transaction already exists in DB, merge the signatures for each input on a best
        // effort basis.
        let txid = tx.compute_txid();
        if let Some(mut db_psbt) = db_conn.spend_tx(&txid) {
            let db_tx = db_psbt.unsigned_tx.clone();
            for i in 0..db_tx.input.len() {
                if tx
                    .input
                    .get(i)
                    .map(|tx_in| tx_in.previous_output == db_tx.input[i].previous_output)
                    != Some(true)
                {
                    continue;
                }
                let psbtin = match psbt.inputs.get(i) {
                    Some(psbtin) => psbtin,
                    None => continue,
                };
                let db_psbtin = match db_psbt.inputs.get_mut(i) {
                    Some(db_psbtin) => db_psbtin,
                    None => continue,
                };
                db_psbtin
                    .partial_sigs
                    .extend(psbtin.partial_sigs.clone().into_iter());
                db_psbtin
                    .tap_script_sigs
                    .extend(psbtin.tap_script_sigs.clone().into_iter());
                if db_psbtin.tap_key_sig.is_none() {
                    db_psbtin.tap_key_sig = psbtin.tap_key_sig;
                }
            }
            psbt = db_psbt;
        } else {
            // If the transaction doesn't exist in DB already, sanity check its inputs.
            // FIXME: should we allow for external inputs?
            let outpoints: Vec<bitcoin::OutPoint> =
                tx.input.iter().map(|txin| txin.previous_output).collect();
            let coins = db_conn.coins_by_outpoints(&outpoints);
            if coins.len() != outpoints.len() {
                for op in outpoints {
                    if !coins.contains_key(&op) {
                        return Err(CommandError::UnknownOutpoint(op));
                    }
                }
            }
        }

        // Finally, insert (or update) the PSBT in database.
        db_conn.store_spend(&psbt);

        Ok(())
    }

    pub fn update_labels(&self, items: &HashMap<LabelItem, Option<String>>) {
        let mut db_conn = self.db.connection();
        db_conn.update_labels(items);
    }

    pub fn get_labels(&self, items: &HashSet<LabelItem>) -> GetLabelsResult {
        let mut db_conn = self.db.connection();
        GetLabelsResult {
            labels: db_conn.labels(items),
        }
    }

    pub fn get_labels_bip329(&self, offset: u32, limit: u32) -> GetLabelsBip329Result {
        let mut db_conn = self.db.connection();
        GetLabelsBip329Result {
            labels: db_conn.get_labels_bip329(offset, limit),
        }
    }

    pub fn list_spend(
        &self,
        txids: Option<Vec<bitcoin::Txid>>,
    ) -> Result<ListSpendResult, CommandError> {
        if let Some(ids) = &txids {
            if ids.is_empty() {
                return Err(CommandError::EmptyFilterList);
            }
        }

        let mut db_conn = self.db.connection();
        let spend_psbts = db_conn.list_spend();

        let txids_set: Option<HashSet<_>> = txids.as_ref().map(|list| list.iter().collect());
        let spend_txs = spend_psbts
            .into_iter()
            .filter_map(|(psbt, updated_at)| {
                if let Some(set) = &txids_set {
                    if !set.contains(&psbt.unsigned_tx.compute_txid()) {
                        return None;
                    }
                }
                Some(ListSpendEntry { psbt, updated_at })
            })
            .collect();
        Ok(ListSpendResult { spend_txs })
    }

    pub fn delete_spend(&self, txid: &bitcoin::Txid) {
        let mut db_conn = self.db.connection();
        db_conn.delete_spend(txid);
    }

    /// Finalize and broadcast this stored Spend transaction.
    pub fn broadcast_spend(&self, txid: &bitcoin::Txid) -> Result<(), CommandError> {
        let mut db_conn = self.db.connection();

        // First, try to finalize the spending transaction with the elements contained
        // in the PSBT.
        let mut spend_psbt = db_conn
            .spend_tx(txid)
            .ok_or(CommandError::UnknownSpend(*txid))?;

        for index in 0..spend_psbt.inputs.len() {
            match spend_psbt.finalize_inp_mut(&self.secp, index) {
                Ok(_) => log::info!("Finalizing input at: {}", index),
                Err(e) => log::warn!("Not finalizing input at: {} | {}", index, e),
            }
        }

        // Then, broadcast it (or try to, we never know if we are not going to hit an
        // error at broadcast time).
        // These checks are already performed at Spend creation time. TODO: a belt-and-suspenders is still worth it though.
        let final_tx = spend_psbt.extract_tx_unchecked_fee_rate();
        self.bitcoin
            .broadcast_tx(&final_tx)
            .map_err(CommandError::TxBroadcast)?;

        // Finally, update our state with the changes from this transaction.
        let (tx, rx) = mpsc::sync_channel(0);
        if let Err(e) = self.poller_sender.send(PollerMessage::PollNow(tx)) {
            log::error!("Error requesting update from poller: {}", e);
        }
        if let Err(e) = rx.recv() {
            log::error!("Error receiving completion signal from poller: {}", e);
        }

        Ok(())
    }

    /// Create PSBT to replace the given transaction using RBF.
    ///
    /// `txid` must either point to a PSBT in our database (not necessarily broadcast) or an
    /// unconfirmed spend transaction (whether or not any associated PSBT is saved in our database).
    ///
    /// `is_cancel` indicates whether to "cancel" the transaction by including only a single (change)
    /// output in the replacement or otherwise to keep the same (non-change) outputs and simply
    /// bump the fee.
    /// If `true`, the only output of the RBF transaction will be change and the inputs will include
    /// at least one of the inputs from the previous transaction. If `false`, all inputs from the previous
    /// transaction will be used in the replacement.
    /// In both cases:
    /// - if the previous transaction includes a change output to one of our own change addresses,
    ///   this same address will be used for change in the RBF transaction, if required. If the previous
    ///   transaction pays to more than one of our change addresses, then the one receiving the highest
    ///   value will be used as a change address and the others will be treated as non-change outputs.
    /// - the RBF transaction may include additional confirmed coins as inputs if required
    ///   in order to pay the higher fee (this applies also when replacing a self-send).
    ///
    /// `feerate_vb` is the target feerate for the RBF transaction (in sat/vb). If `None`, it will be set
    /// to 1 sat/vb larger than the feerate of the previous transaction, which is the minimum value allowed
    /// when using RBF.
    pub fn rbf_psbt(
        &self,
        txid: &bitcoin::Txid,
        is_cancel: bool,
        feerate_vb: Option<u64>,
    ) -> Result<CreateSpendResult, CommandError> {
        let mut db_conn = self.db.connection();
        let mut tx_getter = DbTxGetter::new(&self.db);

        if is_cancel && feerate_vb.is_some() {
            return Err(CommandError::RbfError(RbfErrorInfo::SuperfluousFeerate));
        }

        let prev_tx = if let Some(psbt) = db_conn.spend_tx(txid) {
            psbt.unsigned_tx
        } else {
            db_conn
                .coins(&[CoinStatus::Spending], &[])
                .into_values()
                .find(|c| c.spend_txid == Some(*txid))
                .and_then(|_| tx_getter.get_tx(txid))
                .ok_or(CommandError::UnknownSpend(*txid))?
        };
        if !prev_tx.is_explicitly_rbf() {
            return Err(CommandError::RbfError(RbfErrorInfo::NotSignaling));
        }
        let prev_outpoints: Vec<bitcoin::OutPoint> = prev_tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect();
        let prev_coins = db_conn.coins_by_outpoints(&prev_outpoints);
        // Make sure all prev outpoints are coins in our DB.
        if let Some(op) = prev_outpoints
            .iter()
            .find(|op| !prev_coins.contains_key(op))
        {
            return Err(CommandError::UnknownOutpoint(*op));
        }
        if let Some(op) = prev_coins.iter().find_map(|(_, coin)| {
            if coin.spend_block.is_some() {
                Some(coin.outpoint)
            } else {
                None
            }
        }) {
            return Err(CommandError::AlreadySpent(op));
        }
        // Compute the minimal feerate and fee the replacement transaction must have to satisfy RBF
        // rules #3, #4 and #6 (see
        // https://github.com/bitcoin/bitcoin/blob/master/doc/policy/mempool-replacements.md). By
        // default (ie if the transaction we are replacing was dropped from the mempool) there is
        // no minimum absolute fee and the minimum feerate is 1, the minimum relay feerate.
        let (min_feerate_vb, descendant_fees) = self
            .bitcoin
            .mempool_spenders(&prev_outpoints)
            .into_iter()
            .fold(
                (1, bitcoin::Amount::from_sat(0)),
                |(min_feerate, descendant_fee), entry| {
                    let entry_feerate = entry
                        .fees
                        .base
                        .checked_div(entry.vsize)
                        .expect("Can't have a null vsize or tx would be invalid")
                        .to_sat()
                        .checked_add(1)
                        .expect("Can't overflow or tx would be invalid");
                    (
                        std::cmp::max(min_feerate, entry_feerate),
                        descendant_fee + entry.fees.descendant,
                    )
                },
            );
        // Check replacement transaction's target feerate, if set, is high enough,
        // and otherwise set it to the min feerate found above.
        let feerate_vb = if is_cancel {
            min_feerate_vb
        } else {
            feerate_vb.ok_or(CommandError::RbfError(RbfErrorInfo::MissingFeerate))?
        };
        if feerate_vb < min_feerate_vb {
            return Err(CommandError::RbfError(RbfErrorInfo::TooLowFeerate(
                feerate_vb,
                min_feerate_vb,
            )));
        }
        // Get info about prev outputs to determine replacement outputs.
        let prev_derivs: Vec<_> = prev_tx
            .output
            .iter()
            .map(|txo| {
                let address = bitcoin::Address::from_script(
                    &txo.script_pubkey,
                    self.config.bitcoin_config.network,
                )
                .expect("address already used in finalized transaction");
                (
                    address.clone(),
                    txo.value,
                    db_conn.derivation_index_by_address(&address),
                )
            })
            .collect();
        // Set the previous change address to that of the change output with the largest value
        // and then largest index.
        let prev_change_address = prev_derivs
            .iter()
            .filter_map(|(addr, amt, deriv)| {
                if let Some((ind, true)) = &deriv {
                    Some((addr, amt, ind))
                } else {
                    None
                }
            })
            .max_by(|(_, amt_1, ind_1), (_, amt_2, ind_2)| amt_1.cmp(amt_2).then(ind_1.cmp(ind_2)))
            .map(|(addr, _, _)| addr)
            .cloned();
        // If not cancel, use all previous outputs as destinations, except for
        // the output corresponding to the change address we found above.
        // If cancel, the replacement will not have any destinations, only a change output.
        let destinations = if !is_cancel {
            prev_derivs
                .into_iter()
                .filter_map(|(addr, amt, _)| {
                    if prev_change_address.as_ref() != Some(&addr) {
                        Some((self.spend_addr(&mut db_conn, addr), amt))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // If there was no previous change address, we set the change address for the replacement
        // to our next change address. This way, we won't increment the change index with each attempt
        // at creating the replacement PSBT below.
        let change_address = prev_change_address
            .map(|addr| self.spend_addr(&mut db_conn, addr))
            .unwrap_or_else(|| self.next_change_addr(&mut db_conn));
        // If `!is_cancel`, we take the previous coins as mandatory candidates and add confirmed coins as optional.
        // Otherwise, we take the previous coins as optional candidates and let coin selection find the
        // best solution that includes at least one of these. If there are insufficient funds to create the replacement
        // transaction in this way, then we set candidates in the same way as for the `!is_cancel` case.
        let mut candidate_coins: Vec<CandidateCoin> = prev_coins
            .values()
            .map(|c| {
                // In case any previous coins are unconfirmed, we don't include their ancestor info
                // in the candidate as the replacement fee and feerate will be higher and any
                // additional fee to pay for ancestors should already have been taken into account
                // when including these coins in the previous transaction.
                coin_to_candidate(
                    c, /*must_select=*/ !is_cancel, /*sequence=*/ None,
                    /*ancestor_info=*/ None,
                )
            })
            .collect();
        let confirmed_cands: Vec<CandidateCoin> = db_conn
            .coins(&[CoinStatus::Confirmed], &[])
            .into_values()
            .filter_map(|c| {
                // Make sure we don't have duplicate candidates in case any of the coins are not
                // currently set as spending in the DB (and are therefore still confirmed).
                if !prev_coins.contains_key(&c.outpoint) {
                    Some(coin_to_candidate(
                        &c, /*must_select=*/ false, /*sequence=*/ None,
                        /*ancestor_info=*/ None,
                    ))
                } else {
                    None
                }
            })
            .collect();
        if !is_cancel {
            candidate_coins.extend(&confirmed_cands);
        }
        // The replaced fee is the fee of the transaction being replaced and its descendants. Coin selection
        // will ensure that the replacement transaction additionally pays for its own weight as per
        // RBF rule 4.
        let replaced_fee = descendant_fees.to_sat();
        let locktime = self.anti_fee_sniping_locktime();
        // This loop can have up to 2 iterations in the case of cancel and otherwise only 1.
        loop {
            match create_spend(
                &self.config.main_descriptor,
                &self.secp,
                &mut tx_getter,
                &destinations,
                &candidate_coins,
                SpendTxFees::Rbf(feerate_vb, replaced_fee),
                change_address.clone(),
                locktime,
            ) {
                Ok(CreateSpendRes {
                    psbt,
                    has_change,
                    warnings,
                }) => {
                    // In case of success, make sure to update our next derivation index if any address
                    // used in the transaction outputs was from the future.
                    for (addr, _) in destinations {
                        self.maybe_increase_last_deriv_index(&mut db_conn, &addr.info);
                    }
                    if has_change {
                        self.maybe_increase_last_deriv_index(&mut db_conn, &change_address.info);
                    }

                    return Ok(CreateSpendResult::Success {
                        psbt,
                        warnings: warnings.iter().map(|w| w.to_string()).collect(),
                    });
                }
                Err(SpendCreationError::CoinSelection(e)) => {
                    // If we get a coin selection error due to insufficient funds and we want to cancel the
                    // transaction, then set all previous coins as mandatory and add confirmed coins as
                    // optional, unless we have already done this.
                    if is_cancel && candidate_coins.iter().all(|c| !c.must_select) {
                        for cand in candidate_coins.iter_mut() {
                            cand.must_select = true;
                        }
                        candidate_coins.extend(&confirmed_cands);
                        continue;
                    } else {
                        return Ok(CreateSpendResult::InsufficientFunds { missing: e.missing });
                    }
                }
                Err(e) => {
                    return Err(e.into());
                }
            };
        }
    }

    /// Trigger a rescan of the block chain for transactions involving our main descriptor between
    /// the given date and the current tip.
    /// The date must be after the genesis block time and before the current tip blocktime.
    pub fn start_rescan(&mut self, timestamp: u32) -> Result<(), CommandError> {
        let mut db_conn = self.db.connection();
        let genesis_timestamp = self.bitcoin.genesis_block_timestamp();

        let future_timestamp = self
            .bitcoin
            .tip_time()
            .map(|t| timestamp >= t)
            .unwrap_or(false);
        if timestamp < genesis_timestamp || future_timestamp {
            return Err(CommandError::InsaneRescanTimestamp(timestamp));
        }
        if db_conn.rescan_timestamp().is_some() || self.bitcoin.rescan_progress().is_some() {
            return Err(CommandError::AlreadyRescanning);
        }

        // TODO: there is a race with the above check for whether the backend is already
        // rescanning. This could make us crash with the bitcoind backend if someone triggered a
        // rescan of the wallet just after we checked above and did now.
        self.bitcoin
            .start_rescan(&self.config.main_descriptor, timestamp)
            .map_err(CommandError::RescanTrigger)?;
        db_conn.set_rescan(timestamp);

        Ok(())
    }

    /// list_confirmed_transactions retrieves a limited list of transactions which occurred between two given dates.
    pub fn list_confirmed_transactions(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> ListTransactionsResult {
        let mut db_conn = self.db.connection();
        // Note the result could in principle be retrieved in a single database query.
        let txids = db_conn.list_txids(start, end, limit);
        self.list_transactions(&txids)
    }

    /// list_transactions retrieves the transactions with the given txids.
    pub fn list_transactions(&self, txids: &[bitcoin::Txid]) -> ListTransactionsResult {
        let transactions = self
            .db
            .connection()
            .list_wallet_transactions(txids)
            .into_iter()
            .map(|(tx, height, time)| TransactionInfo { tx, height, time })
            .collect();
        ListTransactionsResult { transactions }
    }

    /// Create a transaction that sweeps coins using a timelocked recovery path to a
    /// provided address with the provided feerate.
    ///
    /// The `timelock` parameter can be used to specify which recovery path to use. By default,
    /// we'll use the first recovery path available.
    ///
    /// If `coins_outpoints` is empty, all coins for which the given recovery path is currently
    /// available will be used. Otherwise, only those specified will be considered. An error will
    /// be returned if any coins specified by `coins_outpoints` are unknown, already spent or
    /// otherwise not currently recoverable using the given recovery path.
    ///
    /// Note that not all coins may be spendable through a single recovery path at the same time.
    pub fn create_recovery(
        &self,
        address: bitcoin::Address<address::NetworkUnchecked>,
        coins_outpoints: &[bitcoin::OutPoint],
        feerate_vb: u64,
        timelock: Option<u16>,
    ) -> Result<CreateRecoveryResult, CommandError> {
        if feerate_vb < 1 {
            return Err(CommandError::InvalidFeerate(feerate_vb));
        }
        let mut tx_getter = DbTxGetter::new(&self.db);
        let mut db_conn = self.db.connection();
        let sweep_addr = self.spend_addr(&mut db_conn, self.validate_address(address)?);

        // Query the coins that we can spend through the specified recovery path (if no recovery
        // path specified, use the first available one) from the database.
        let current_height = self.bitcoin.chain_tip().height;
        let timelock =
            timelock.unwrap_or_else(|| self.config.main_descriptor.first_timelock_value());
        let height_delta: i32 = timelock.into();
        let coins = if coins_outpoints.is_empty() {
            db_conn.coins(&[CoinStatus::Confirmed], &[])
        } else {
            // We could have used the same DB call for both cases by specifying the status and outpoints,
            // but in order to give more helpful errors, we filter the DB call here only for outpoints
            // and then check for coin status separately.
            let coins_by_op = db_conn.coins(&[], coins_outpoints);
            for op in coins_outpoints {
                let coin = coins_by_op
                    .get(op)
                    .ok_or(CommandError::UnknownOutpoint(*op))?;
                // We only check for spent coins here. Unconfirmed coins (including immature)
                // will fail the check for recoverability further below.
                if coin.is_spent() {
                    return Err(CommandError::AlreadySpent(*op));
                }
            }
            coins_by_op
        };
        let mut sweepable_coins = Vec::with_capacity(coins.len());
        for (op, c) in coins {
            // We are interested in coins available at the *next* block
            if c.block_info
                .map(|b| current_height + 1 >= b.height + height_delta)
                .unwrap_or(false)
            {
                sweepable_coins.push(coin_to_candidate(
                    &c,
                    /*must_select=*/ true,
                    /*sequence=*/ Some(bitcoin::Sequence::from_height(timelock)),
                    /*ancestor_info=*/ None,
                ));
            } else if !coins_outpoints.is_empty() {
                return Err(CommandError::OutpointNotRecoverable(op, timelock));
            }
        }
        if sweepable_coins.is_empty() {
            return Err(CommandError::RecoveryNotAvailable);
        }

        let sweep_addr_info = sweep_addr.info;
        let locktime = self.anti_fee_sniping_locktime();
        let CreateSpendRes {
            psbt, has_change, ..
        } = create_spend(
            &self.config.main_descriptor,
            &self.secp,
            &mut tx_getter,
            &[], // No destination, only the change address.
            &sweepable_coins,
            SpendTxFees::Regular(feerate_vb),
            sweep_addr,
            locktime,
        )?;
        if has_change {
            self.maybe_increase_last_deriv_index(&mut db_conn, &sweep_addr_info);
        }

        Ok(CreateRecoveryResult { psbt })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoDescriptors {
    pub main: descriptors::LianaDescriptor,
}

/// Information about the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoResult {
    pub version: String,
    pub network: bitcoin::Network,
    pub block_height: i32,
    pub sync: f64,
    pub descriptors: GetInfoDescriptors,
    /// The progress as a percentage (between 0 and 1) of an ongoing rescan if there is any
    pub rescan_progress: Option<f64>,
    /// Timestamp at wallet creation date
    pub timestamp: u32,
    /// Timestamp of last poll, if any.
    pub last_poll_timestamp: Option<u32>,
    /// Last index used to generate a receive address
    pub receive_index: u32,
    /// Last index used to generate a change address
    pub change_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDerivIndexesResult {
    pub receive: u32,
    pub change: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAddressResult {
    #[serde(deserialize_with = "deser_addr_assume_checked")]
    pub address: bitcoin::Address,
    pub derivation_index: bip32::ChildNumber,
    pub bip21: Option<Url>,
}

impl GetAddressResult {
    pub fn new(
        address: bitcoin::Address,
        derivation_index: bip32::ChildNumber,
        bip21: Option<Url>,
    ) -> Self {
        Self {
            address,
            derivation_index,
            bip21,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetLabelsResult {
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetLabelsBip329Result {
    pub labels: crate::bip329::Labels,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AddressInfo {
    index: u32,
    receive: bitcoin::Address,
    change: bitcoin::Address,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ListAddressesResult {
    addresses: Vec<AddressInfo>,
}

impl ListAddressesResult {
    pub fn new(addresses: Vec<AddressInfo>) -> Self {
        ListAddressesResult { addresses }
    }
}

/// A revealed address entry in the list returned by [`DaemonControl::list_revealed_addresses`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListRevealedAddressesEntry {
    /// The address's derivation index.
    pub index: ChildNumber,
    /// The address.
    #[serde(deserialize_with = "deser_addr_assume_checked")]
    pub address: bitcoin::Address,
    /// Label assigned to the address, if any.
    pub label: Option<String>,
    /// How many coins, including those unconfirmed, that are currently in the wallet are using this address.
    ///
    /// This count does not include any coins that may have been replaced or otherwise dropped
    /// from the mempool.
    pub used_count: u32,
}

/// Result of a [`DaemonControl::list_revealed_addresses`] request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListRevealedAddressesResult {
    /// Revealed addresses in order of descending derivation index.
    pub addresses: Vec<ListRevealedAddressesEntry>,
    /// `continue_from` being set to some value indicates that there may
    /// be more addresses that can be listed with pagination. The next
    /// [`DaemonControl::list_revealed_addresses`] request can be continued
    /// with this value passed to `start_index`.
    ///
    /// If `continue_from` is `None`, then there are no further
    /// addresses to be listed.
    pub continue_from: Option<ChildNumber>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LCSpendInfo {
    pub txid: bitcoin::Txid,
    /// The block height this spending transaction was confirmed at.
    pub height: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsEntry {
    #[serde(
        serialize_with = "ser_amount",
        deserialize_with = "deser_amount_from_sats"
    )]
    pub amount: bitcoin::Amount,
    pub outpoint: bitcoin::OutPoint,
    #[serde(
        serialize_with = "ser_to_string",
        deserialize_with = "deser_addr_assume_checked"
    )]
    pub address: bitcoin::Address,
    pub block_height: Option<i32>,
    /// Derivation index used to create the coin deposit address.
    pub derivation_index: bip32::ChildNumber,
    /// Information about the transaction spending this coin.
    pub spend_info: Option<LCSpendInfo>,
    /// Whether this coin was created by a coinbase transaction that is still immature.
    pub is_immature: bool,
    /// Whether the coin deposit address was derived from the change descriptor.
    pub is_change: bool,
    /// Whether the coin is the output of a transaction whose inputs are all from
    /// this same wallet. If the coin is unconfirmed, it also means that all its
    /// unconfirmed ancestors, if any, are also from self.
    pub is_from_self: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsResult {
    pub coins: Vec<ListCoinsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum CreateSpendResult {
    Success {
        #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
        psbt: Psbt,
        warnings: Vec<String>,
    },
    InsufficientFunds {
        missing: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpendEntry {
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    pub psbt: Psbt,
    pub updated_at: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpendResult {
    pub spend_txs: Vec<ListSpendEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTransactionsResult {
    pub transactions: Vec<TransactionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionInfo {
    #[serde(serialize_with = "ser_hex", deserialize_with = "deser_hex")]
    pub tx: bitcoin::Transaction,
    pub height: Option<i32>,
    pub time: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateRecoveryResult {
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    pub psbt: Psbt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bitcoin::Block, database::BlockInfo, testutils::*};
    use liana::spend::InsaneFeeInfo;

    use bitcoin::{
        bip32::{self, ChildNumber},
        blockdata::transaction::{TxIn, TxOut, Version as TxVersion},
        locktime::absolute,
        Amount, OutPoint, ScriptBuf, Sequence, Transaction, Txid, Witness,
    };
    use spend::InsufficientFunds;
    use std::{collections::BTreeMap, str::FromStr};

    #[test]
    fn getinfo() {
        let ms = DummyLiana::new(DummyBitcoind::new(), DummyDatabase::new());
        // We can query getinfo
        ms.control().get_info();
        ms.shutdown();
    }

    #[test]
    fn getnewaddress() {
        let ms = DummyLiana::new(DummyBitcoind::new(), DummyDatabase::new());

        let control = &ms.control();
        // We can get an address (it will have index 1)
        let addr = control.get_new_address().address;
        // $ bitcoin-cli deriveaddresses "wsh(or_d(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/0/*),and_v(v:pkh([aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/0/*),older(10000))))#wx6v3mks" 1
        // [
        // "bc1q9ksrc647hx8zp2cewl8p5f487dgux3777yees8rjcx46t4daqzzqt7yga8",
        // "bc1qm06ceyghltr8v5cmeckh6cquhy9nks626pahapc04xd98kwf478qwmhqew"
        // ]
        assert_eq!(
            addr,
            bitcoin::Address::from_str(
                "bc1qm06ceyghltr8v5cmeckh6cquhy9nks626pahapc04xd98kwf478qwmhqew"
            )
            .unwrap()
            .assume_checked()
        );
        // We won't get the same twice.
        let addr2 = control.get_new_address().address;
        assert_ne!(addr, addr2);

        ms.shutdown();
    }

    #[test]
    fn listaddresses() {
        let ms = DummyLiana::new(DummyBitcoind::new(), DummyDatabase::new());

        let control = &ms.control();

        let list = control.list_addresses(Some(2), Some(5)).unwrap();

        assert_eq!(list.addresses[0].index, 2);
        assert_eq!(list.addresses.last().unwrap().index, 6);

        let addr0 = control.get_new_address().address;
        let _addr1 = control.get_new_address().address;
        let addr2 = control.get_new_address().address;
        let _addr3 = control.get_new_address().address;
        let addr4 = control.get_new_address().address;

        let list = control.list_addresses(Some(0), None).unwrap();

        assert_eq!(list.addresses[0].index, 0);
        assert_eq!(list.addresses[1].receive, addr0); // first address has index 1
        assert_eq!(list.addresses.last().unwrap().index, 5);
        assert_eq!(list.addresses.last().unwrap().receive, addr4);

        let list = control.list_addresses(None, None).unwrap();

        assert_eq!(list.addresses[0].index, 0);
        assert_eq!(list.addresses[1].index, 1);
        assert_eq!(list.addresses[1].receive, addr0);
        assert_eq!(list.addresses.last().unwrap().index, 5);
        assert_eq!(list.addresses.last().unwrap().receive, addr4);

        let list = control.list_addresses(Some(1), Some(3)).unwrap();

        assert_eq!(list.addresses[0].index, 1);
        assert_eq!(list.addresses[0].receive, addr0);
        assert_eq!(list.addresses.last().unwrap().index, 3);
        assert_eq!(list.addresses.last().unwrap().receive, addr2);

        let list = control.list_addresses(Some(5), None).unwrap();

        assert_eq!(list.addresses.len(), 1);
        assert_eq!(list.addresses[0].index, 5);
        assert_eq!(list.addresses[0].receive, addr4);

        // We can get no address for the last unhardened index.
        let max_unhardened_index = 2u32.pow(31) - 1;
        let res = control
            .list_addresses(Some(max_unhardened_index), Some(0))
            .unwrap();
        // This is equivalent to not passing a count.
        assert_eq!(
            res,
            control
                .list_addresses(Some(max_unhardened_index), None)
                .unwrap()
        );
        // We can also get the one last unhardened index.
        control
            .list_addresses(Some(max_unhardened_index), Some(1))
            .unwrap();
        // However we can't get into hardened territory.
        assert_eq!(
            control
                .list_addresses(Some(max_unhardened_index), Some(2))
                .unwrap_err(),
            CommandError::InvalidDerivationIndex
        );

        // We also can't pass a hardened start index.
        let first_hardened_index = max_unhardened_index + 1;
        assert_eq!(
            control
                .list_addresses(Some(first_hardened_index), None)
                .unwrap_err(),
            CommandError::InvalidDerivationIndex
        );
        assert_eq!(
            control
                .list_addresses(Some(first_hardened_index), Some(0))
                .unwrap_err(),
            CommandError::InvalidDerivationIndex
        );
        assert_eq!(
            control
                .list_addresses(Some(first_hardened_index), Some(1))
                .unwrap_err(),
            CommandError::InvalidDerivationIndex
        );

        // Much less so overflow.
        assert_eq!(
            control.list_addresses(Some(u32::MAX), None).unwrap_err(),
            CommandError::InvalidDerivationIndex
        );
        assert_eq!(
            control.list_addresses(Some(u32::MAX), Some(0)).unwrap_err(),
            CommandError::InvalidDerivationIndex
        );
        assert_eq!(
            control.list_addresses(Some(u32::MAX), Some(1)).unwrap_err(),
            CommandError::InvalidDerivationIndex
        );

        // We won't crash if we pass a start index larger than the next derivation index without
        // passing a count. (ie no underflow.)
        let next_deriv_index = list.addresses.last().unwrap().index + 1;
        control
            .list_addresses(Some(next_deriv_index + 1), None)
            .unwrap();

        ms.shutdown();
    }

    #[test]
    fn list_revealed_addresses() {
        let ms = DummyLiana::new(DummyBitcoind::new(), DummyDatabase::new());

        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();

        // $ bitcoin-cli deriveaddresses "wsh(or_d(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/0/*),and_v(v:pkh([aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/0/*),older(10000))))#wx6v3mks" 0
        // [
        //   "bc1q9ksrc647hx8zp2cewl8p5f487dgux3777yees8rjcx46t4daqzzqt7yga8"
        // ]
        let addr0 = bitcoin::Address::from_str(
            "bc1q9ksrc647hx8zp2cewl8p5f487dgux3777yees8rjcx46t4daqzzqt7yga8",
        )
        .unwrap()
        .assume_checked();

        // The wallet starts with index 0 already revealed:
        let list = control
            .list_revealed_addresses(false, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 1);
        assert!(list.continue_from.is_none());
        let revealed = list.addresses.first().unwrap();
        assert_eq!(revealed.index, ChildNumber::from(0));
        assert_eq!(revealed.address, addr0);
        assert_eq!(revealed.used_count, 0);
        assert!(revealed.label.is_none());

        // Generate new addresses up to and including index 7:
        let addr1 = control.get_new_address().address;
        let addr2 = control.get_new_address().address;
        let addr3 = control.get_new_address().address;
        let addr4 = control.get_new_address().address;
        let addr5 = control.get_new_address().address;
        let addr6 = control.get_new_address().address;
        let addr7 = control.get_new_address().address;
        assert_eq!(control.get_info().receive_index, 7);

        // Set some labels.
        db_conn.update_labels(&HashMap::from([
            (
                LabelItem::Address(addr1.clone()),
                Some("my test label 1".to_string()),
            ),
            (
                LabelItem::Address(addr5.clone()),
                Some("my test label 5".to_string()),
            ),
        ]));

        // If we continue_from a value above our last index, we'll start from the last index.
        let list = control
            .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(100)))
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7)); // this is our last revealed index
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(5));
        assert_eq!(list.addresses[2].address, addr5);
        assert_eq!(list.addresses[2].used_count, 0);
        assert_eq!(list.addresses[2].label, Some("my test label 5".to_string()));

        // If we start from a hardened index, we'll get the same result again:
        assert_eq!(
            list,
            control
                .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(u32::MAX)))
                .unwrap()
        );

        // Passing `None` for `continue_from` will also start from the last revealed index:
        let list = control
            .list_revealed_addresses(false, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7));
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(5));
        assert_eq!(list.addresses[2].address, addr5);
        assert_eq!(list.addresses[2].used_count, 0);
        assert_eq!(list.addresses[2].label, Some("my test label 5".to_string()));

        // Now continue pagination using the `continue_from` value from the result above:
        let list = control
            .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(4)))
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(1)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(4));
        assert_eq!(list.addresses[0].address, addr4);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(3));
        assert_eq!(list.addresses[1].address, addr3);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(2));
        assert_eq!(list.addresses[2].address, addr2);
        assert_eq!(list.addresses[2].used_count, 0);
        assert!(list.addresses[2].label.is_none());

        // This is the final page:
        let list = control
            .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(1)))
            .unwrap();
        assert_eq!(list.addresses.len(), 2); // only two addresses even though limit was 3
        assert!(list.continue_from.is_none()); // there are no more addresses to derive

        assert_eq!(list.addresses[0].index, ChildNumber::from(1));
        assert_eq!(list.addresses[0].address, addr1);
        assert_eq!(list.addresses[0].used_count, 0);
        assert_eq!(list.addresses[0].label, Some("my test label 1".to_string()));
        assert_eq!(list.addresses[1].index, ChildNumber::from(0));
        assert_eq!(list.addresses[1].address, addr0);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());

        // Add a coin so that address with index 5 is used:
        db_conn.new_unspent_coins(&[Coin {
            outpoint: OutPoint::new(
                Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                    .unwrap(),
                0,
            ),
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(80_000),
            derivation_index: ChildNumber::from(5),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: true,
        }]);

        // If we don't exclude used, results will be same as before, except index 5 is marked as used:
        let list = control
            .list_revealed_addresses(false, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7));
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(5));
        assert_eq!(list.addresses[2].address, addr5);
        assert_eq!(list.addresses[2].used_count, 1); // used
        assert_eq!(list.addresses[2].label, Some("my test label 5".to_string()));

        // If we exclude used, index 5 will be skipped:
        let list = control
            .list_revealed_addresses(false, true, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(3)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7));
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(4));
        assert_eq!(list.addresses[2].address, addr4);
        assert_eq!(list.addresses[2].used_count, 0);
        assert!(list.addresses[2].label.is_none());

        // Similar behaviour if we continue from index 5. First without excluding used:
        let list = control
            .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(5)))
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(2)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(5));
        assert_eq!(list.addresses[0].address, addr5);
        assert_eq!(list.addresses[0].used_count, 1); // used
        assert_eq!(list.addresses[0].label, Some("my test label 5".to_string()));
        assert_eq!(list.addresses[1].index, ChildNumber::from(4));
        assert_eq!(list.addresses[1].address, addr4);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(3));
        assert_eq!(list.addresses[2].address, addr3);
        assert_eq!(list.addresses[2].used_count, 0);
        assert!(list.addresses[2].label.is_none());

        // Now excluding used:
        let list = control
            .list_revealed_addresses(false, true, 3, Some(ChildNumber::from(5)))
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(1)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(4));
        assert_eq!(list.addresses[0].address, addr4);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(3));
        assert_eq!(list.addresses[1].address, addr3);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(2));
        assert_eq!(list.addresses[2].address, addr2);
        assert_eq!(list.addresses[2].used_count, 0);
        assert!(list.addresses[2].label.is_none());

        // If we add another coin using the same derivation index, the count will increase:
        db_conn.new_unspent_coins(&[Coin {
            outpoint: OutPoint::new(
                Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                    .unwrap(),
                1,
            ),
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(80_000),
            derivation_index: ChildNumber::from(5),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: true,
        }]);

        let list = control
            .list_revealed_addresses(false, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7));
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(5));
        assert_eq!(list.addresses[2].address, addr5);
        assert_eq!(list.addresses[2].used_count, 2); // count updated.
        assert_eq!(list.addresses[2].label, Some("my test label 5".to_string()));

        // Check change address
        // $ bitcoin-cli deriveaddresses "wsh(or_d(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/1/*),and_v(v:pkh([aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/1/*),older(10000))))#sqk8au7v" 0
        // [
        //   "bc1qd5m23jemr8mfj8x4q482zpspnxehg0jg0pyzrhxfg8xrrtyqewvqjrq3x6"
        // ]
        let change_addr0 = bitcoin::Address::from_str(
            "bc1qd5m23jemr8mfj8x4q482zpspnxehg0jg0pyzrhxfg8xrrtyqewvqjrq3x6",
        )
        .unwrap()
        .assume_checked();

        // If we ask for change addresses, we only have the initial one revealed:
        let list = control
            .list_revealed_addresses(true, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 1);
        assert!(list.continue_from.is_none());

        assert_eq!(list.addresses[0].index, ChildNumber::from(0));
        assert_eq!(list.addresses[0].address, change_addr0);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());

        // Finally, passing limit 0 returns an empty vector and the continue_from is same as it started with.
        // First, passing None for continue from:
        let list = control
            .list_revealed_addresses(false, false, 0, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 0);
        assert_eq!(list.continue_from, Some(ChildNumber::from(7)));

        // Now, continuing from 4:
        let list = control
            .list_revealed_addresses(false, false, 0, Some(ChildNumber::from(4)))
            .unwrap();
        assert_eq!(list.addresses.len(), 0);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        ms.shutdown();
    }

    #[test]
    fn create_spend() {
        let dummy_tx = bitcoin::Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![],
            output: vec![],
        };
        let dummy_op = bitcoin::OutPoint::new(dummy_tx.compute_txid(), 0);
        let ms = DummyLiana::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_txs(&[dummy_tx]);

        // Arguments sanity checking
        let dummy_addr =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let dummy_value = 10_000;
        let mut destinations = <HashMap<bitcoin::Address<address::NetworkUnchecked>, u64>>::new();
        assert_eq!(
            control.create_spend(&destinations, &[], 1, None),
            Err(CommandError::NoOutpointForSelfSend)
        );
        destinations = [(dummy_addr.clone(), dummy_value)]
            .iter()
            .cloned()
            .collect();
        // Insufficient funds for coin selection.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 0, None),
            Err(CommandError::InvalidFeerate(0))
        );

        // The coin doesn't exist. If we create a new unspent one at this outpoint with a much
        // higher value, we'll get a Spend transaction with a change output.
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Err(CommandError::UnknownOutpoint(dummy_op))
        );
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        // If we try to use coin selection, the unconfirmed not-from-self coin will not be used
        // as a candidate and so we get a coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        assert!(psbt.inputs[0].non_witness_utxo.is_some());
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 2);
        // It has change so no warnings expected.
        assert!(warnings.is_empty());
        assert_eq!(
            tx.output[0].script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        assert_eq!(tx.output[0].value.to_sat(), dummy_value);

        // NOTE: if you are wondering about the usefulness of these tests asserting arbitrary fixed
        // values, that's a belt-and-suspenders check to make sure size and fee calculations do not
        // change unexpectedly. For instance this specific test caught how a change in
        // rust-bitcoin's serialization of transactions with no input silently affected our fee
        // calculation.

        // Transaction is 1 in (P2WSH satisfaction), 2 outs. At 1sat/vb, it's 161 sats fees.
        // At 2sats/vb, it's twice that.
        assert_eq!(tx.output[1].value.to_sat(), 89_839);
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations, &[dummy_op], 2, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.output[1].value.to_sat(), 89_678);

        // A feerate of 555 won't trigger the sanity checks (they were previously not taking the
        // satisfaction size into account and overestimating the feerate).
        control
            .create_spend(&destinations, &[dummy_op], 555, None)
            .unwrap();

        // If we ask for a too high feerate, or a too large/too small output, it'll fail.
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 10_000, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        *destinations.get_mut(&dummy_addr).unwrap() = 100_001;
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        *destinations.get_mut(&dummy_addr).unwrap() = 4_500;
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::InvalidOutputValue(bitcoin::Amount::from_sat(4_500))
            ))
        );

        // If we ask to create an output for an address from another network, it will fail.
        let invalid_addr =
            bitcoin::Address::from_str("tb1qfufcrdyarcg5eph608c6l8vktrc9re6agu4se2").unwrap();
        let invalid_destinations: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(invalid_addr, dummy_value)].iter().cloned().collect();
        assert!(matches!(
            control.create_spend(&invalid_destinations, &[dummy_op], 1, None),
            Err(CommandError::Address(
                address::error::ParseError::NetworkValidation { .. }
            ))
        ));

        // If we ask for a large, but valid, output we won't get a change output. 95_000 because we
        // won't create an output lower than 5k sats.
        *destinations.get_mut(&dummy_addr).unwrap() = 95_000;
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 1);
        assert_eq!(
            tx.output[0].script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        assert_eq!(tx.output[0].value.to_sat(), 95_000);
        // change = 100_000 - 95_000 - /* fee without change */ 127 - /* extra fee for change output */ 43 = 4830
        assert_eq!(
            warnings,
            vec![
                "Dust UTXO. The minimal change output allowed by Liana is 5000 sats. \
                Instead of creating a change of 4839 sats, it was added to the \
                transaction fee. Select a larger input to avoid this from happening."
            ]
        );

        // Increase the target value by the change amount and the warning will disappear.
        *destinations.get_mut(&dummy_addr).unwrap() = 95_000 + 4_839;
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert!(warnings.is_empty());

        // Now increase target also by the extra fee that was paying for change and we can still create the spend.
        *destinations.get_mut(&dummy_addr).unwrap() =
            95_000 + 4_830 + /* fee for change output */ 43;
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert!(warnings.is_empty());

        // Now increase the target by 1 more sat and we will have insufficient funds.
        *destinations.get_mut(&dummy_addr).unwrap() =
            95_000 + 4_839 + /* fee for change output */ 43 + 1;
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { missing: 1 }),
        );

        // Now decrease the target so that the lost change is just 1 sat.
        *destinations.get_mut(&dummy_addr).unwrap() =
            100_000 - /* fee without change */ 118 - /* extra fee for change output */ 43 - 1;
        let warnings = if let CreateSpendResult::Success { warnings, .. } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            warnings
        } else {
            panic!("expect successful spend creation")
        };
        // Message uses "sat" instead of "sats" when value is 1.
        assert_eq!(
            warnings,
            vec![
                "Dust UTXO. The minimal change output allowed by Liana is 5000 sats. \
                Instead of creating a change of 1 sat, it was added to the \
                transaction fee. Select a larger input to avoid this from happening."
            ]
        );

        // Now decrease the target value so that we have enough for a change output.
        *destinations.get_mut(&dummy_addr).unwrap() =
            95_000 - /* fee without change */ 118 - /* extra fee for change output */ 43;
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 2);
        assert_eq!(tx.output[1].value.to_sat(), 5_000);
        assert!(warnings.is_empty());

        // Now increase the target by 1 and we'll get a warning again, this time for 1 less than the dust threshold.
        *destinations.get_mut(&dummy_addr).unwrap() =
            95_000 - /* fee without change */ 118 - /* extra fee for change output */ 43 + 1;
        let warnings = if let CreateSpendResult::Success { warnings, .. } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            warnings
        } else {
            panic!("expect successful spend creation")
        };
        assert_eq!(
            warnings,
            vec![
                "Dust UTXO. The minimal change output allowed by Liana is 5000 sats. \
                Instead of creating a change of 4999 sats, it was added to the \
                transaction fee. Select a larger input to avoid this from happening."
            ]
        );

        // Now if we mark the coin as spent, we won't create another Spend transaction containing
        // it.
        db_conn.spend_coins(&[(
            dummy_op,
            bitcoin::Txid::from_str(
                "ef78f79ba747813887747cf8582897a48f1a09f1ca04d2cd3d6fcfdcbb5e0797",
            )
            .unwrap(),
        )]);
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Err(CommandError::AlreadySpent(dummy_op))
        );
        // If we try to use coin selection, the spent coin will not be used as a candidate
        // and so we get a coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));

        // We'd bail out if they tried to create a transaction with a too high feerate.
        let dummy_op_dup = bitcoin::OutPoint {
            txid: dummy_op.txid,
            vout: dummy_op.vout + 10,
        };
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op_dup,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(400_000),
            derivation_index: bip32::ChildNumber::from(42),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op_dup], 1_001, None),
            Err(CommandError::SpendCreation(SpendCreationError::InsaneFees(
                InsaneFeeInfo::TooHighFeerate(1_001)
            )))
        );

        // Add an unconfirmed coin from self to be used for coin selection.
        let confirmed_op_1 = bitcoin::OutPoint {
            txid: dummy_op.txid,
            vout: dummy_op.vout + 100,
        };
        let unconfirmed_coin = Coin {
            outpoint: confirmed_op_1,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(80_000),
            derivation_index: bip32::ChildNumber::from(42),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: true,
        };
        db_conn.new_unspent_coins(&[unconfirmed_coin]);
        // Coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        // Set destination amount equal to value of confirmed coins.
        *destinations.get_mut(&dummy_addr).unwrap() = 80_000;
        // Coin selection error occurs due to insufficient funds to pay fee.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        let confirmed_op_2 = bitcoin::OutPoint {
            txid: confirmed_op_1.txid,
            vout: confirmed_op_1.vout + 10,
        };
        // Add new confirmed coin to cover the fee.
        db_conn.new_unspent_coins(&[Coin {
            outpoint: confirmed_op_2,
            is_immature: false,
            block_info: Some(BlockInfo {
                height: 174500,
                time: 174500,
            }),
            amount: bitcoin::Amount::from_sat(20_000),
            derivation_index: bip32::ChildNumber::from(43),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        // First, create a transaction using auto coin selection.
        let psbt = if let CreateSpendResult::Success { psbt, .. } =
            control.create_spend(&destinations, &[], 1, None).unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx_auto = psbt.unsigned_tx;
        let mut tx_prev_outpoints = tx_auto
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<Vec<OutPoint>>();
        tx_prev_outpoints.sort();
        assert_eq!(tx_auto.input.len(), 2);
        assert_eq!(tx_prev_outpoints, vec![confirmed_op_1, confirmed_op_2]);
        // Output includes change.
        assert_eq!(tx_auto.output.len(), 2);
        assert_eq!(
            tx_auto.output[0].script_pubkey,
            dummy_addr.assume_checked().script_pubkey()
        );
        assert_eq!(tx_auto.output[0].value, Amount::from_sat(80_000));

        // Create a second transaction using manual coin selection.
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations, &[confirmed_op_1, confirmed_op_2], 1, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx_manual = psbt.unsigned_tx;
        // Check that manual and auto selection give same outputs (except change address).
        assert_ne!(tx_auto.output, tx_manual.output);
        assert_eq!(tx_auto.output.len(), tx_manual.output.len());
        assert_eq!(tx_auto.output[0], tx_manual.output[0]);
        assert_eq!(tx_auto.output[1].value, tx_manual.output[1].value);
        assert_ne!(
            tx_auto.output[1].script_pubkey,
            tx_manual.output[1].script_pubkey
        );
        // Check inputs are also the same. Need to sort as order is not guaranteed by `create_spend`.
        let mut auto_input = tx_auto.clone().input;
        let mut manual_input = tx_manual.input;
        auto_input.sort();
        manual_input.sort();
        assert_eq!(auto_input, manual_input);

        // Now check that the spend created above using auto-selection only works when the unconfirmed coin
        // is from self, whether or not it is from change.
        // 1. not from self and not change
        db_conn.remove_coins(&[unconfirmed_coin.outpoint]);
        let mut unconfirmed_coin_2 = unconfirmed_coin;
        unconfirmed_coin_2.is_from_self = false;
        unconfirmed_coin_2.is_change = false;
        db_conn.new_unspent_coins(&[unconfirmed_coin_2]);
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        // 2. not from self and change
        db_conn.remove_coins(&[unconfirmed_coin_2.outpoint]);
        unconfirmed_coin_2.is_from_self = false;
        unconfirmed_coin_2.is_change = true;
        db_conn.new_unspent_coins(&[unconfirmed_coin_2]);
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));

        // Re-insert the original unconfirmed coin again.
        db_conn.remove_coins(&[unconfirmed_coin_2.outpoint]);
        db_conn.new_unspent_coins(&[unconfirmed_coin]);

        // Now do the same again, but this time specifying the change address to be the same
        // as for the auto spend.
        let change_address = bitcoin::Address::from_script(
            tx_auto.output[1].script_pubkey.as_script(),
            bitcoin::Network::Bitcoin,
        )
        .unwrap();
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(
                &destinations,
                &[confirmed_op_1, confirmed_op_2],
                1,
                Some(change_address.as_unchecked().clone()),
            )
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx_manual = psbt.unsigned_tx;
        // Now the outputs of each transaction are the same.
        assert_eq!(tx_auto.output, tx_manual.output);
        // Check again that inputs are still the same.
        let mut auto_input = tx_auto.input;
        let mut manual_input = tx_manual.input;
        auto_input.sort();
        manual_input.sort();
        assert_eq!(auto_input, manual_input);

        // Add a confirmed coin with a value near the dust limit and check that
        // `InsufficientFunds` error is returned if feerate is too high.
        let confirmed_op_3 = bitcoin::OutPoint {
            txid: confirmed_op_2.txid,
            vout: confirmed_op_2.vout + 10,
        };
        db_conn.new_unspent_coins(&[Coin {
            outpoint: confirmed_op_3,
            is_immature: false,
            block_info: Some(BlockInfo {
                height: 174500,
                time: 174500,
            }),
            amount: bitcoin::Amount::from_sat(5_250),
            derivation_index: bip32::ChildNumber::from(56),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        let empty_dest = &HashMap::<bitcoin::Address<address::NetworkUnchecked>, u64>::new();
        assert!(matches!(
            control.create_spend(empty_dest, &[confirmed_op_3], 5, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        // If we use a lower fee, the self-send will succeed.
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(empty_dest, &[confirmed_op_3], 1, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        let tx_prev_outpoints = tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<Vec<OutPoint>>();
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx_prev_outpoints, vec![confirmed_op_3]);
        assert_eq!(tx.output.len(), 1);

        // Can't create a transaction that spends an immature coinbase deposit.
        let imma_op = bitcoin::OutPoint::from_str(
            "4753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:0",
        )
        .unwrap();
        db_conn.new_unspent_coins(&[Coin {
            outpoint: imma_op,
            is_immature: true,
            block_info: None,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        assert_eq!(
            control.create_spend(&destinations, &[imma_op], 1_001, None),
            Err(CommandError::ImmatureCoinbase(imma_op))
        );

        ms.shutdown();
    }

    #[test]
    fn update_spend() {
        let dummy_op_a = bitcoin::OutPoint::from_str(
            "3753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:0",
        )
        .unwrap();
        let dummy_op_b = bitcoin::OutPoint::from_str(
            "4753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:1",
        )
        .unwrap();
        let mut dummy_bitcoind = DummyBitcoind::new();
        let dummy_tx = bitcoin::Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![],
            output: vec![],
        };
        dummy_bitcoind
            .txs
            .insert(dummy_op_a.txid, (dummy_tx.clone(), None));
        dummy_bitcoind.txs.insert(dummy_op_b.txid, (dummy_tx, None));
        let ms = DummyLiana::new(dummy_bitcoind, DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();

        // Add two (unconfirmed) coins in DB
        db_conn.new_unspent_coins(&[
            Coin {
                outpoint: dummy_op_a,
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(100_000),
                derivation_index: bip32::ChildNumber::from(13),
                is_change: false,
                spend_txid: None,
                spend_block: None,
                is_from_self: false,
            },
            Coin {
                outpoint: dummy_op_b,
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(115_680),
                derivation_index: bip32::ChildNumber::from(34),
                is_change: false,
                spend_txid: None,
                spend_block: None,
                is_from_self: false,
            },
        ]);

        // Now create three transactions spending those coins differently
        let dummy_addr_a =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let dummy_addr_b =
            bitcoin::Address::from_str("bc1q39srgatmkp6k2ne3l52yhkjprdvunvspqydmkx").unwrap();
        let dummy_value_a = 50_000;
        let dummy_value_b = 60_000;
        let destinations_a: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(dummy_addr_a.clone(), dummy_value_a)]
                .iter()
                .cloned()
                .collect();
        let destinations_b: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(dummy_addr_b.clone(), dummy_value_b)]
                .iter()
                .cloned()
                .collect();
        let destinations_c: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(dummy_addr_a, dummy_value_a), (dummy_addr_b, dummy_value_b)]
                .iter()
                .cloned()
                .collect();
        let mut psbt_a = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations_a, &[dummy_op_a], 1, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let txid_a = psbt_a.unsigned_tx.compute_txid();
        let psbt_b = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations_b, &[dummy_op_b], 10, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let txid_b = psbt_b.unsigned_tx.compute_txid();
        let psbt_c = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations_c, &[dummy_op_a, dummy_op_b], 100, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let txid_c = psbt_c.unsigned_tx.compute_txid();

        // We can store and query them all
        control.update_spend(psbt_a.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_a).unwrap(), psbt_a);
        control.update_spend(psbt_b.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_b).unwrap(), psbt_b);
        control.update_spend(psbt_c.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_c).unwrap(), psbt_c);

        // As well as update them, with or without new signatures
        let sig = bitcoin::ecdsa::Signature::from_str("304402204004fcdbb9c0d0cbf585f58cee34dccb012efbd8fc2b0d5e97760045ae35803802201a0bd7ec2383e0b93748abc9946c8e17a8312e314dab85982aeba650e738cbf401").unwrap();
        psbt_a.inputs[0].partial_sigs.insert(
            bitcoin::PublicKey::from_str(
                "023a664c5617412f0b292665b1fd9d766456a7a3b1614c7e7c5f411200ff1958ef",
            )
            .unwrap(),
            sig,
        );
        control.update_spend(psbt_a.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_a).unwrap(), psbt_a);
        control.update_spend(psbt_b.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_b).unwrap(), psbt_b);
        control.update_spend(psbt_c.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_c).unwrap(), psbt_c);

        // We can't store a PSBT spending an external coin
        let external_op = bitcoin::OutPoint::from_str(
            "8753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:2",
        )
        .unwrap();
        psbt_a.unsigned_tx.input[0].previous_output = external_op;
        assert_eq!(
            control.update_spend(psbt_a),
            Err(CommandError::UnknownOutpoint(external_op))
        );

        ms.shutdown();
    }

    #[test]
    fn rbf_psbt() {
        let dummy_op_a = bitcoin::OutPoint::from_str(
            "3753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:0",
        )
        .unwrap();
        let mut dummy_bitcoind = DummyBitcoind::new();
        // Transaction spends outpoint a.
        let dummy_tx_a = bitcoin::Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![bitcoin::TxIn {
                previous_output: dummy_op_a,
                sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                ..bitcoin::TxIn::default()
            }],
            output: vec![],
        };
        // PSBT corresponding to the above transaction.
        let dummy_psbt_a = Psbt {
            unsigned_tx: dummy_tx_a.clone(),
            version: 0,
            xpub: BTreeMap::new(),
            proprietary: BTreeMap::new(),
            unknown: BTreeMap::new(),
            inputs: vec![],
            outputs: vec![],
        };
        let dummy_txid_a = dummy_psbt_a.unsigned_tx.compute_txid();
        dummy_bitcoind.txs.insert(dummy_txid_a, (dummy_tx_a, None));
        let ms = DummyLiana::new(dummy_bitcoind, DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        // The spend needs to be in DB before using RBF.
        assert_eq!(
            control.rbf_psbt(&dummy_txid_a, true, None),
            Err(CommandError::UnknownSpend(dummy_txid_a))
        );
        // Store the spend.
        db_conn.store_spend(&dummy_psbt_a);
        // Now add the coin to DB, but as spent.
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op_a,
            is_immature: false,
            block_info: Some(BlockInfo {
                height: 174500,
                time: 174500,
            }),
            amount: bitcoin::Amount::from_sat(300_000),
            derivation_index: bip32::ChildNumber::from(11),
            is_change: false,
            spend_txid: Some(dummy_txid_a),
            spend_block: Some(BlockInfo {
                height: 184500,
                time: 184500,
            }),
            is_from_self: false,
        }]);
        // The coin is spent so we cannot RBF.
        assert_eq!(
            control.rbf_psbt(&dummy_txid_a, true, None),
            Err(CommandError::AlreadySpent(dummy_op_a))
        );
        db_conn.unspend_coins(&[dummy_op_a]);
        // Now remove the coin.
        db_conn.remove_coins(&[dummy_op_a]);
        assert_eq!(
            control.rbf_psbt(&dummy_txid_a, true, None),
            Err(CommandError::UnknownOutpoint(dummy_op_a))
        );
        // A target feerate not higher than the previous should return an error. This is tested in
        // the functional tests.

        ms.shutdown();
    }

    #[test]
    fn list_confirmed_transactions() {
        let outpoint = OutPoint::new(
            Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                .unwrap(),
            0,
        );

        let deposit1: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(100_000_000),
            }],
        };

        let deposit2: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(2000),
            }],
        };

        let deposit3: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(3000),
            }],
        };

        let spend_tx: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: OutPoint {
                    txid: deposit1.compute_txid(),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![
                TxOut {
                    script_pubkey: ScriptBuf::new(),
                    value: Amount::from_sat(4000),
                },
                TxOut {
                    script_pubkey: ScriptBuf::new(),
                    value: Amount::from_sat(100_000_000 - 4000 - 1000),
                },
            ],
        };

        let mut db = DummyDatabase::new();
        db.insert_coins(vec![
            // Deposit 1
            Coin {
                is_change: false,
                is_immature: false,
                outpoint: OutPoint {
                    txid: deposit1.compute_txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 1, time: 1 }),
                spend_block: Some(BlockInfo { height: 3, time: 3 }),
                derivation_index: ChildNumber::from(0),
                amount: bitcoin::Amount::from_sat(100_000_000),
                spend_txid: Some(spend_tx.compute_txid()),
                is_from_self: false,
            },
            // Deposit 2
            Coin {
                is_change: false,
                is_immature: false,
                outpoint: OutPoint {
                    txid: deposit2.compute_txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 2, time: 2 }),
                spend_block: None,
                derivation_index: ChildNumber::from(1),
                amount: bitcoin::Amount::from_sat(2000),
                spend_txid: None,
                is_from_self: false,
            },
            // This coin is a change output.
            Coin {
                is_change: true,
                is_immature: false,
                outpoint: OutPoint::new(spend_tx.compute_txid(), 1),
                block_info: Some(BlockInfo { height: 3, time: 3 }),
                spend_block: None,
                derivation_index: ChildNumber::from(2),
                amount: bitcoin::Amount::from_sat(100_000_000 - 4000 - 1000),
                spend_txid: None,
                is_from_self: false,
            },
            // Deposit 3
            Coin {
                is_change: false,
                is_immature: false,
                outpoint: OutPoint {
                    txid: deposit3.compute_txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 4, time: 4 }),
                spend_block: None,
                derivation_index: ChildNumber::from(3),
                amount: bitcoin::Amount::from_sat(3000),
                spend_txid: None,
                is_from_self: false,
            },
        ]);

        let mut txs_map = HashMap::new();
        txs_map.insert(
            deposit1.compute_txid(),
            (
                deposit1.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 1,
                    height: 1,
                }),
            ),
        );
        txs_map.insert(
            deposit2.compute_txid(),
            (
                deposit2.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 2,
                    height: 2,
                }),
            ),
        );
        txs_map.insert(
            spend_tx.compute_txid(),
            (
                spend_tx.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 3,
                    height: 3,
                }),
            ),
        );
        txs_map.insert(
            deposit3.compute_txid(),
            (
                deposit3.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 4,
                    height: 4,
                }),
            ),
        );

        let ms = DummyLiana::new(DummyBitcoind::new(), db);

        let control = &ms.control();
        let mut db_conn = control.db.connection();
        let txs: Vec<_> = txs_map.values().map(|(tx, _)| tx.clone()).collect();
        db_conn.new_txs(&txs);

        let mut transactions = control.list_confirmed_transactions(0, 4, 10).transactions;
        transactions.sort_by(|tx1, tx2| tx2.height.cmp(&tx1.height));
        assert_eq!(transactions.len(), 4);

        assert_eq!(transactions[0].time, Some(4));
        assert_eq!(transactions[0].tx, deposit3);

        assert_eq!(transactions[1].time, Some(3));
        assert_eq!(transactions[1].tx, spend_tx);

        assert_eq!(transactions[2].time, Some(2));
        assert_eq!(transactions[2].tx, deposit2);

        assert_eq!(transactions[3].time, Some(1));
        assert_eq!(transactions[3].tx, deposit1);

        let mut transactions = control.list_confirmed_transactions(2, 3, 10).transactions;
        transactions.sort_by(|tx1, tx2| tx2.height.cmp(&tx1.height));
        assert_eq!(transactions.len(), 2);

        assert_eq!(transactions[0].time, Some(3));
        assert_eq!(transactions[1].time, Some(2));
        assert_eq!(transactions[1].tx, deposit2);

        let transactions = control.list_confirmed_transactions(2, 3, 1).transactions;
        assert_eq!(transactions.len(), 1);

        assert_eq!(transactions[0].time, Some(3));
        assert_eq!(transactions[0].tx, spend_tx);

        ms.shutdown();
    }

    #[test]
    fn list_transactions() {
        let outpoint = OutPoint::new(
            Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                .unwrap(),
            0,
        );

        let tx1: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(100_000_000),
            }],
        };

        let tx2: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(2000),
            }],
        };

        let tx3: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(3000),
            }],
        };

        let mut txs_map = HashMap::new();
        txs_map.insert(
            tx1.compute_txid(),
            (
                tx1.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 1,
                    height: 1,
                }),
            ),
        );
        txs_map.insert(
            tx2.compute_txid(),
            (
                tx2.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 2,
                    height: 2,
                }),
            ),
        );
        txs_map.insert(
            tx3.compute_txid(),
            (
                tx3.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 4,
                    height: 4,
                }),
            ),
        );

        let ms = DummyLiana::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db.connection();
        let txs: Vec<_> = txs_map.values().map(|(tx, _)| tx.clone()).collect();
        db_conn.new_txs(&txs);
        // We need coins in the DB in order to get the block info for the transactions.
        for (txid, (_tx, block)) in txs_map {
            // Insert more than one coin per transaction to check that the command does not
            // return duplicate transactions.
            for vout in 0..4 {
                db_conn.new_unspent_coins(&[Coin {
                    outpoint: bitcoin::OutPoint::new(txid, vout),
                    is_immature: false,
                    block_info: block.map(|b| BlockInfo {
                        height: b.height,
                        time: b.time,
                    }),
                    amount: bitcoin::Amount::from_sat(100_000),
                    derivation_index: bip32::ChildNumber::from(13),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            }
        }

        let transactions = control
            .list_transactions(&[tx1.compute_txid()])
            .transactions;
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].tx, tx1);

        let transactions = control
            .list_transactions(&[tx1.compute_txid(), tx2.compute_txid(), tx3.compute_txid()])
            .transactions;
        assert_eq!(transactions.len(), 3);

        let txs: Vec<Transaction> = transactions
            .iter()
            .map(|transaction| transaction.tx.clone())
            .collect();

        assert!(txs.contains(&tx1));
        assert!(txs.contains(&tx2));
        assert!(txs.contains(&tx3));

        ms.shutdown();
    }

    #[test]
    fn create_recovery() {
        let dummy_tx = bitcoin::Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![],
            output: vec![],
        };
        let dummy_txid = dummy_tx.compute_txid();
        let dummy_op = bitcoin::OutPoint::new(dummy_txid, 0);
        let ms = DummyLiana::new_timelock(DummyBitcoind::new(), DummyDatabase::new(), 10);
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_txs(&[dummy_tx]);

        // Arguments sanity checking
        let dummy_addr =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        // Feerate cannot be less than 1.
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[], 0, None),
            Err(CommandError::InvalidFeerate(0))
        );
        // If we ask to sweep to an address from another network, it will fail.
        let invalid_addr =
            bitcoin::Address::from_str("tb1qfufcrdyarcg5eph608c6l8vktrc9re6agu4se2").unwrap();
        assert!(matches!(
            control.create_recovery(invalid_addr, &[], 1, None),
            Err(CommandError::Address(
                address::error::ParseError::NetworkValidation { .. }
            ))
        ));

        // We have no coins to create recovery.
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::RecoveryNotAvailable),
        ));
        // Coin is unknown.
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::UnknownOutpoint(dummy_op)),
        );

        // Add unconfirmed coin.
        let dummy_coin = Coin {
            outpoint: dummy_op,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        };
        db_conn.new_unspent_coins(&[dummy_coin]);
        // Recovery not available for unconfirmed coins.
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::RecoveryNotAvailable),
        ));
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::OutpointNotRecoverable(dummy_op, 10)),
        );

        // Confirm coin such that timelock (10) has not expired at next block (101).
        db_conn.confirm_coins(&[(dummy_op, 92, 100_000)]);
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::RecoveryNotAvailable),
        ));
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::OutpointNotRecoverable(dummy_op, 10)),
        );

        // If we use a smaller timelock value it works, even though we don't have any such
        // recovery timelock (see https://github.com/wizardsardine/liana/issues/1089).
        assert!(control
            .create_recovery(dummy_addr.clone(), &[], 1, Some(9))
            .is_ok());
        assert!(control
            .create_recovery(dummy_addr.clone(), &[dummy_op], 1, Some(9))
            .is_ok());

        // Remove coin, re-add and confirm such that recovery available at next block.
        db_conn.remove_coins(&[dummy_op]);
        db_conn.new_unspent_coins(&[dummy_coin]);
        db_conn.confirm_coins(&[(dummy_op, 91, 100_000)]);
        let res = control.create_recovery(dummy_addr.clone(), &[], 1, None);
        assert!(res.is_ok());
        let psbt = res.unwrap().psbt;
        assert_eq!(psbt.outputs.len(), 1);
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        // Amount is coin value minus fee.
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().value,
            Amount::from_sat(100_000 - 127)
        );

        // If we pass a larger timelock, it no longer works:
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, Some(11)),
            Err(CommandError::RecoveryNotAvailable),
        ));
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, Some(11)),
            Err(CommandError::OutpointNotRecoverable(dummy_op, 11)),
        );

        // If the coin is spending, it is no longer recoverable.
        db_conn.spend_coins(&[(
            dummy_op,
            Txid::from_str("84f09bddfe0f036d0390edf655636ad6092c3ab8f09b2bb1503caa393463f241")
                .unwrap(),
        )]);
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::RecoveryNotAvailable),
        ));
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::AlreadySpent(dummy_op)),
        );

        // Now remove the coin and re-add, but this time with an amount that is too small to create an output.
        // This will give a coin selection error due to insufficient funds.
        db_conn.remove_coins(&[dummy_op]);
        let mut dummy_coin = dummy_coin;
        dummy_coin.amount = Amount::from_sat(5_000 + 126);
        db_conn.new_unspent_coins(&[dummy_coin]);
        db_conn.confirm_coins(&[(dummy_op, 91, 100_000)]);
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(InsufficientFunds { missing: 1 })
            )),
        );
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(InsufficientFunds { missing: 1 })
            )),
        );

        // Add a new coin so that we have enough funds for the recovery.
        let dummy_op_2 = bitcoin::OutPoint::new(dummy_txid, 1);
        let dummy_coin_2 = Coin {
            outpoint: dummy_op_2,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(10_000),
            derivation_index: bip32::ChildNumber::from(1378),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        };
        db_conn.new_unspent_coins(&[dummy_coin_2]);
        db_conn.confirm_coins(&[(dummy_op_2, 92, 200_000)]);
        // Coin cannot be used as the timelock will still be in place at the next block.
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(InsufficientFunds { missing: 1 })
            )),
        );
        // If we try to specify the new coin, we'll get an error that the coin is not recoverable.
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op, dummy_op_2], 1, None),
            Err(CommandError::OutpointNotRecoverable(dummy_op_2, 10)),
        );
        // Using a shorter timelock parameter works:
        assert!(control
            .create_recovery(dummy_addr.clone(), &[], 1, Some(9))
            .is_ok());
        assert!(control
            .create_recovery(dummy_addr.clone(), &[dummy_op, dummy_op_2], 1, Some(9))
            .is_ok());

        // Now re-add the coin with a confirmation one block earlier.
        db_conn.remove_coins(&[dummy_op_2]);
        db_conn.new_unspent_coins(&[dummy_coin_2]);
        db_conn.confirm_coins(&[(dummy_op_2, 91, 200_000)]);

        // Now both coins are used in the recovery and we have enough funds.
        let res = control.create_recovery(dummy_addr.clone(), &[], 1, None);
        assert!(res.is_ok());
        let psbt = res.unwrap().psbt;
        assert_eq!(psbt.outputs.len(), 1);
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        // Amount is coin value minus fee.
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().value,
            Amount::from_sat(/* coin 1 */ 5_000 + 126 + /* coin 2 */10_000 - /* fee */ 211)
        );

        // Do the same again, now specifying the outpoints explicitly.
        let res = control.create_recovery(dummy_addr.clone(), &[dummy_op, dummy_op_2], 1, None);
        assert!(res.is_ok());
        let psbt = res.unwrap().psbt;
        assert_eq!(psbt.outputs.len(), 1);
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().value,
            Amount::from_sat(/* coin 1 */ 5_000 + 126 + /* coin 2 */10_000 - /* fee */ 211)
        );

        // Now check that increasing the feerate increases the fee.
        let res = control.create_recovery(dummy_addr.clone(), &[], 2, None);
        assert!(res.is_ok());
        let psbt = res.unwrap().psbt;
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().value,
            Amount::from_sat(/* coin 1 */ 5_000 + 126 + /* coin 2 */10_000 - /* fee */ 2 * 211)
        );

        ms.shutdown();
    }
}
