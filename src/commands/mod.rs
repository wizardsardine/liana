//! # Liana commands
//!
//! External interface to the Liana daemon.

mod utils;

use crate::{
    bitcoin::BitcoinInterface,
    database::{Coin, DatabaseConnection, DatabaseInterface},
    descriptors,
    spend::{
        create_spend, AddrInfo, CandidateCoin, CreateSpendRes, SpendCreationError,
        SpendOutputAddress, SpendTxFees, TxGetter,
    },
    DaemonControl, VERSION,
};

pub use crate::database::{CoinStatus, LabelItem};

use utils::{
    deser_addr_assume_checked, deser_amount_from_sats, deser_fromstr, deser_hex, ser_amount,
    ser_hex, ser_to_string,
};

use std::{
    collections::{hash_map, HashMap, HashSet},
    fmt, sync,
};

use miniscript::{
    bitcoin::{self, address, bip32, psbt::Psbt},
    psbt::PsbtExt,
};
use serde::{Deserialize, Serialize};

// Timestamp in the header of the genesis block. Used for sanity checks.
const MAINNET_GENESIS_TIME: u32 = 1231006505;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    NoOutpointForSelfSend,
    InvalidFeerate(/* sats/vb */ u64),
    UnknownOutpoint(bitcoin::OutPoint),
    AlreadySpent(bitcoin::OutPoint),
    ImmatureCoinbase(bitcoin::OutPoint),
    Address(bitcoin::address::Error),
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
    /// Overflowing or unhardened derivation index.
    InvalidDerivationIndex,
    RbfError(RbfErrorInfo),
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
            Self::InvalidDerivationIndex => {
                write!(f, "Unhardened or overflowing BIP32 derivation index.")
            }
            Self::RbfError(e) => write!(f, "RBF error: '{}'.", e),
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
    TooLowFeerate(u64),
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
            Self::TooLowFeerate(r) => write!(f, "Feerate too low: {}.", r),
            Self::NotSignaling => write!(f, "Replacement candidate does not signal for RBF."),
        }
    }
}

/// A wallet transaction getter which fetches the transaction from our Bitcoin backend with a cache
/// to avoid needless redundant calls. Note the cache holds an Option<> so we also avoid redundant
/// calls when the txid isn't known by our Bitcoin backend.
struct BitcoindTxGetter<'a> {
    bitcoind: &'a sync::Arc<sync::Mutex<dyn BitcoinInterface>>,
    cache: HashMap<bitcoin::Txid, Option<bitcoin::Transaction>>,
}

impl<'a> BitcoindTxGetter<'a> {
    pub fn new(bitcoind: &'a sync::Arc<sync::Mutex<dyn BitcoinInterface>>) -> Self {
        Self {
            bitcoind,
            cache: HashMap::new(),
        }
    }
}

impl<'a> TxGetter for BitcoindTxGetter<'a> {
    fn get_tx(&mut self, txid: &bitcoin::Txid) -> Option<bitcoin::Transaction> {
        if let hash_map::Entry::Vacant(entry) = self.cache.entry(*txid) {
            entry.insert(self.bitcoind.wallet_transaction(txid).map(|wtx| wtx.0));
        }
        self.cache.get(txid).cloned().flatten()
    }
}

fn coin_to_candidate(
    coin: &Coin,
    must_select: bool,
    sequence: Option<bitcoin::Sequence>,
) -> CandidateCoin {
    CandidateCoin {
        outpoint: coin.outpoint,
        amount: coin.amount,
        deriv_index: coin.derivation_index,
        is_change: coin.is_change,
        must_select,
        sequence,
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
    fn next_change_addr(&self, db_conn: &mut Box<dyn DatabaseConnection>) -> SpendOutputAddress {
        let index = db_conn.change_index();
        let desc = self
            .config
            .main_descriptor
            .change_descriptor()
            .derive(index, &self.secp);
        let addr = desc.address(self.config.bitcoin_config.network);
        SpendOutputAddress {
            addr,
            info: Some(AddrInfo {
                index,
                is_change: true,
            }),
        }
    }

    // If we detect the given address as ours, and it has a higher derivation index than our next
    // derivation index, update our next derivation index to the one after the address'.
    fn maybe_increase_next_deriv_index(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
        addr_info: &Option<AddrInfo>,
    ) {
        if let Some(AddrInfo { index, is_change }) = addr_info {
            if *is_change && db_conn.change_index() < *index {
                let next_index = index
                    .increment()
                    .expect("Must not get into hardened territory");
                db_conn.set_change_index(next_index, &self.secp);
            } else if !is_change && db_conn.receive_index() < *index {
                let next_index = index
                    .increment()
                    .expect("Must not get into hardened territory");
                db_conn.set_receive_index(next_index, &self.secp);
            }
        }
    }
}

impl DaemonControl {
    /// Get information about the current state of the daemon
    pub fn get_info(&self) -> GetInfoResult {
        let mut db_conn = self.db.connection();

        let block_height = db_conn.chain_tip().map(|tip| tip.height).unwrap_or(0);
        let rescan_progress = db_conn
            .rescan_timestamp()
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
            .derive(index, &self.secp)
            .address(self.config.bitcoin_config.network);
        GetAddressResult::new(address, index)
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
            receive_index.max(change_index)
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
        let mut tx_getter = BitcoindTxGetter::new(&self.bitcoin);

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
            // We only select confirmed coins for now. Including unconfirmed ones as well would
            // introduce a whole bunch of additional complexity.
            db_conn
                .coins(&[CoinStatus::Confirmed], &[])
                .into_values()
                .map(|c| {
                    coin_to_candidate(&c, /*must_select=*/ false, /*sequence=*/ None)
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
                .into_values()
                .map(|c| coin_to_candidate(&c, /*must_select=*/ true, /*sequence=*/ None))
                .collect()
        };

        // Create the PSBT. If there was no error in doing so make sure to update our next
        // derivation index in case any address in the transaction outputs was ours and from the
        // future.
        let change_info = change_address.info;
        let CreateSpendRes {
            psbt,
            has_change,
            warnings,
        } = create_spend(
            &self.config.main_descriptor,
            &self.secp,
            &mut tx_getter,
            &destinations_checked,
            &candidate_coins,
            SpendTxFees::Regular(feerate_vb),
            change_address,
        )?;
        for (addr, _) in destinations_checked {
            self.maybe_increase_next_deriv_index(&mut db_conn, &addr.info);
        }
        if has_change {
            self.maybe_increase_next_deriv_index(&mut db_conn, &change_info);
        }

        Ok(CreateSpendResult {
            psbt,
            warnings: warnings.iter().map(|w| w.to_string()).collect(),
        })
    }

    pub fn update_spend(&self, mut psbt: Psbt) -> Result<(), CommandError> {
        let mut db_conn = self.db.connection();
        let tx = &psbt.unsigned_tx;

        // If the transaction already exists in DB, merge the signatures for each input on a best
        // effort basis.
        // We work on the newly provided PSBT, in case its content was updated.
        let txid = tx.txid();
        if let Some(db_psbt) = db_conn.spend_tx(&txid) {
            let db_tx = db_psbt.unsigned_tx;
            for i in 0..db_tx.input.len() {
                if tx
                    .input
                    .get(i)
                    .map(|tx_in| tx_in.previous_output == db_tx.input[i].previous_output)
                    != Some(true)
                {
                    continue;
                }
                let psbtin = match psbt.inputs.get_mut(i) {
                    Some(psbtin) => psbtin,
                    None => continue,
                };
                let db_psbtin = match db_psbt.inputs.get(i) {
                    Some(db_psbtin) => db_psbtin,
                    None => continue,
                };
                psbtin
                    .partial_sigs
                    .extend(db_psbtin.partial_sigs.clone().into_iter());
            }
        } else {
            // If the transaction doesn't exist in DB already, sanity check its inputs.
            // FIXME: should we allow for external inputs?
            let outpoints: Vec<bitcoin::OutPoint> =
                tx.input.iter().map(|txin| txin.previous_output).collect();
            let coins = db_conn.coins_by_outpoints(&outpoints);
            if coins.len() != outpoints.len() {
                for op in outpoints {
                    if coins.get(&op).is_none() {
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

    pub fn list_spend(&self) -> ListSpendResult {
        let mut db_conn = self.db.connection();
        let spend_txs = db_conn
            .list_spend()
            .into_iter()
            .map(|(psbt, updated_at)| ListSpendEntry { psbt, updated_at })
            .collect();
        ListSpendResult { spend_txs }
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
        spend_psbt.finalize_mut(&self.secp).map_err(|e| {
            CommandError::SpendFinalization(
                e.into_iter()
                    .next()
                    .map(|e| e.to_string())
                    .unwrap_or_default(),
            )
        })?;

        // Then, broadcast it (or try to, we never know if we are not going to hit an
        // error at broadcast time).
        // These checks are already performed at Spend creation time. TODO: a belt-and-suspenders is still worth it though.
        let final_tx = spend_psbt.extract_tx_unchecked_fee_rate();
        self.bitcoin
            .broadcast_tx(&final_tx)
            .map_err(CommandError::TxBroadcast)
    }

    /// Create PSBT to replace the given transaction using RBF.
    ///
    /// `txid` must point to a PSBT in our database.
    ///
    /// `is_cancel` indicates whether to "cancel" the transaction by including only a single (change)
    /// output in the replacement or otherwise to keep the same (non-change) outputs and simply
    /// bump the fee.
    /// If `true`, the only output of the RBF transaction will be change and the inputs will include
    /// at least one of the inputs from the previous transaction. If `false`, all inputs from the previous
    /// transaction will be used in the replacement.
    /// In both cases:
    /// - if the previous transaction includes a change output to one of our own change addresses,
    /// this same address will be used for change in the RBF transaction, if required. If the previous
    /// transaction pays to more than one of our change addresses, then the one receiving the highest
    /// value will be used as a change address and the others will be treated as non-change outputs.
    /// - the RBF transaction may include additional confirmed coins as inputs if required
    /// in order to pay the higher fee (this applies also when replacing a self-send).
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
        let mut tx_getter = BitcoindTxGetter::new(&self.bitcoin);

        if is_cancel && feerate_vb.is_some() {
            return Err(CommandError::RbfError(RbfErrorInfo::SuperfluousFeerate));
        }

        let prev_psbt = db_conn
            .spend_tx(txid)
            .ok_or(CommandError::UnknownSpend(*txid))?;
        if !prev_psbt.unsigned_tx.is_explicitly_rbf() {
            return Err(CommandError::RbfError(RbfErrorInfo::NotSignaling));
        }
        let prev_outpoints: Vec<bitcoin::OutPoint> = prev_psbt
            .unsigned_tx
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
            )));
        }
        // Get info about prev outputs to determine replacement outputs.
        let prev_derivs: Vec<_> = prev_psbt
            .unsigned_tx
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
                coin_to_candidate(c, /*must_select=*/ !is_cancel, /*sequence=*/ None)
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
                    ))
                } else {
                    None
                }
            })
            .collect();
        if !is_cancel {
            candidate_coins.extend(&confirmed_cands);
        }
        // Try with increasing fee until fee paid by replacement transaction is high enough.
        // Replacement fee must be at least:
        // sum of fees paid by original transactions + incremental feerate * replacement size.
        // Loop will continue until either we find a suitable replacement or we have insufficient funds.
        let mut replacement_vsize = 0;
        for incremental_feerate in 0.. {
            let min_fee = descendant_fees.to_sat() + replacement_vsize * incremental_feerate;
            let CreateSpendRes {
                psbt: rbf_psbt,
                has_change,
                warnings,
            } = match create_spend(
                &self.config.main_descriptor,
                &self.secp,
                &mut tx_getter,
                &destinations,
                &candidate_coins,
                SpendTxFees::Rbf(feerate_vb, min_fee),
                change_address.clone(),
            ) {
                Ok(psbt) => psbt,
                // If we get a coin selection error due to insufficient funds and we want to cancel the
                // transaction, then set all previous coins as mandatory and add confirmed coins as
                // optional, unless we have already done this.
                Err(SpendCreationError::CoinSelection(_))
                    if is_cancel && candidate_coins.iter().all(|c| !c.must_select) =>
                {
                    for cand in candidate_coins.iter_mut() {
                        cand.must_select = true;
                    }
                    candidate_coins.extend(&confirmed_cands);
                    continue;
                }
                Err(e) => {
                    return Err(e.into());
                }
            };
            replacement_vsize = self
                .config
                .main_descriptor
                .unsigned_tx_max_vbytes(&rbf_psbt.unsigned_tx);

            // Make sure it satisfies RBF rule 4.
            if rbf_psbt.fee().expect("has already been sanity checked")
                >= descendant_fees + bitcoin::Amount::from_sat(replacement_vsize)
            {
                // In case of success, make sure to update our next derivation index if any address
                // used in the transaction outputs was from the future.
                for (addr, _) in destinations {
                    self.maybe_increase_next_deriv_index(&mut db_conn, &addr.info);
                }
                if has_change {
                    self.maybe_increase_next_deriv_index(&mut db_conn, &change_address.info);
                }

                return Ok(CreateSpendResult {
                    psbt: rbf_psbt,
                    warnings: warnings.iter().map(|w| w.to_string()).collect(),
                });
            }
        }

        unreachable!("We keep increasing the min fee until we run out of funds or satisfy rule 4.")
    }

    /// Trigger a rescan of the block chain for transactions involving our main descriptor between
    /// the given date and the current tip.
    /// The date must be after the genesis block time and before the current tip blocktime.
    pub fn start_rescan(&self, timestamp: u32) -> Result<(), CommandError> {
        let mut db_conn = self.db.connection();

        let future_timestamp = self
            .bitcoin
            .tip_time()
            .map(|t| timestamp >= t)
            .unwrap_or(false);
        if timestamp < MAINNET_GENESIS_TIME || future_timestamp {
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

    /// list_confirmed_transactions retrieves a limited list of transactions which occured between two given dates.
    pub fn list_confirmed_transactions(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> ListTransactionsResult {
        let mut db_conn = self.db.connection();
        let txids = db_conn.list_txids(start, end, limit);
        let transactions = txids
            .iter()
            .filter_map(|txid| {
                // TODO: batch those calls to the Bitcoin backend
                // so it can in turn optimize its queries.
                self.bitcoin
                    .wallet_transaction(txid)
                    .map(|(tx, block)| TransactionInfo {
                        tx,
                        height: block.map(|b| b.height),
                        time: block.map(|b| b.time),
                    })
            })
            .collect();
        ListTransactionsResult { transactions }
    }

    /// list_transactions retrieves the transactions with the given txids.
    pub fn list_transactions(&self, txids: &[bitcoin::Txid]) -> ListTransactionsResult {
        let transactions = txids
            .iter()
            .filter_map(|txid| {
                // TODO: batch those calls to the Bitcoin backend
                // so it can in turn optimize its queries.
                self.bitcoin
                    .wallet_transaction(txid)
                    .map(|(tx, block)| TransactionInfo {
                        tx,
                        height: block.map(|b| b.height),
                        time: block.map(|b| b.time),
                    })
            })
            .collect();
        ListTransactionsResult { transactions }
    }

    /// Create a transaction that sweeps all coins for which a timelocked recovery path is
    /// currently available to a provided address with the provided feerate.
    ///
    /// The `timelock` parameter can be used to specify which recovery path to use. By default,
    /// we'll use the first recovery path available.
    ///
    /// Note that not all coins may be spendable through a single recovery path at the same time.
    pub fn create_recovery(
        &self,
        address: bitcoin::Address<address::NetworkUnchecked>,
        feerate_vb: u64,
        timelock: Option<u16>,
    ) -> Result<CreateRecoveryResult, CommandError> {
        if feerate_vb < 1 {
            return Err(CommandError::InvalidFeerate(feerate_vb));
        }
        let mut tx_getter = BitcoindTxGetter::new(&self.bitcoin);
        let mut db_conn = self.db.connection();
        let sweep_addr = self.spend_addr(&mut db_conn, self.validate_address(address)?);

        // Query the coins that we can spend through the specified recovery path (if no recovery
        // path specified, use the first available one) from the database.
        let current_height = self.bitcoin.chain_tip().height;
        let timelock =
            timelock.unwrap_or_else(|| self.config.main_descriptor.first_timelock_value());
        let height_delta: i32 = timelock.into();
        let sweepable_coins: Vec<_> = db_conn
            .coins(&[CoinStatus::Confirmed], &[])
            .into_values()
            .filter_map(|c| {
                // We are interested in coins available at the *next* block
                if c.block_info
                    .map(|b| current_height + 1 >= b.height + height_delta)
                    .unwrap_or(false)
                {
                    Some(coin_to_candidate(
                        &c,
                        /*must_select=*/ true,
                        /*sequence=*/ Some(bitcoin::Sequence::from_height(timelock)),
                    ))
                } else {
                    None
                }
            })
            .collect();
        if sweepable_coins.is_empty() {
            return Err(CommandError::RecoveryNotAvailable);
        }

        let sweep_addr_info = sweep_addr.info;
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
        )?;
        if has_change {
            self.maybe_increase_next_deriv_index(&mut db_conn, &sweep_addr_info);
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAddressResult {
    #[serde(deserialize_with = "deser_addr_assume_checked")]
    pub address: bitcoin::Address,
    pub derivation_index: bip32::ChildNumber,
}

impl GetAddressResult {
    pub fn new(address: bitcoin::Address, derivation_index: bip32::ChildNumber) -> Self {
        Self {
            address,
            derivation_index,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetLabelsResult {
    pub labels: HashMap<String, String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsResult {
    pub coins: Vec<ListCoinsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSpendResult {
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    pub psbt: Psbt,
    pub warnings: Vec<String>,
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
    use crate::{bitcoin::Block, database::BlockInfo, spend::InsaneFeeInfo, testutils::*};

    use bdk_coin_select::InsufficientFunds;
    use bitcoin::{
        bip32::{self, ChildNumber},
        blockdata::transaction::{TxIn, TxOut, Version as TxVersion},
        locktime::absolute,
        Amount, OutPoint, ScriptBuf, Sequence, Transaction, Txid, Witness,
    };
    use std::{collections::BTreeMap, str::FromStr};

    #[test]
    fn getinfo() {
        let ms = DummyLiana::new(DummyBitcoind::new(), DummyDatabase::new());
        // We can query getinfo
        ms.handle.control.get_info();
        ms.shutdown();
    }

    #[test]
    fn getnewaddress() {
        let ms = DummyLiana::new(DummyBitcoind::new(), DummyDatabase::new());

        let control = &ms.handle.control;
        // We can get an address
        let addr = control.get_new_address().address;
        assert_eq!(
            addr,
            bitcoin::Address::from_str(
                "bc1q9ksrc647hx8zp2cewl8p5f487dgux3777yees8rjcx46t4daqzzqt7yga8"
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

        let control = &ms.handle.control;

        let list = control.list_addresses(Some(2), Some(5)).unwrap();

        assert_eq!(list.addresses[0].index, 2);
        assert_eq!(list.addresses.last().unwrap().index, 6);

        let addr0 = control.get_new_address().address;
        let addr1 = control.get_new_address().address;
        let _addr2 = control.get_new_address().address;
        let addr3 = control.get_new_address().address;
        let addr4 = control.get_new_address().address;

        let list = control.list_addresses(Some(0), None).unwrap();

        assert_eq!(list.addresses[0].index, 0);
        assert_eq!(list.addresses[0].receive, addr0);
        assert_eq!(list.addresses.last().unwrap().index, 4);
        assert_eq!(list.addresses.last().unwrap().receive, addr4);

        let list = control.list_addresses(None, None).unwrap();

        assert_eq!(list.addresses[0].index, 0);
        assert_eq!(list.addresses[0].receive, addr0);
        assert_eq!(list.addresses.last().unwrap().index, 4);
        assert_eq!(list.addresses.last().unwrap().receive, addr4);

        let list = control.list_addresses(Some(1), Some(3)).unwrap();

        assert_eq!(list.addresses[0].index, 1);
        assert_eq!(list.addresses[0].receive, addr1);
        assert_eq!(list.addresses.last().unwrap().index, 3);
        assert_eq!(list.addresses.last().unwrap().receive, addr3);

        let addr5 = control.get_new_address().address;
        let list = control.list_addresses(Some(5), None).unwrap();

        assert_eq!(list.addresses[0].index, 5);
        assert_eq!(list.addresses[0].receive, addr5);
        assert_eq!(list.addresses.last().unwrap().index, 5);
        assert_eq!(list.addresses.last().unwrap().receive, addr5);

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
    fn create_spend() {
        let dummy_op = bitcoin::OutPoint::from_str(
            "3753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:0",
        )
        .unwrap();
        let mut dummy_bitcoind = DummyBitcoind::new();
        dummy_bitcoind.txs.insert(
            dummy_op.txid,
            (
                bitcoin::Transaction {
                    version: TxVersion::TWO,
                    lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
                    input: vec![],
                    output: vec![],
                },
                None,
            ),
        );
        let ms = DummyLiana::new(dummy_bitcoind, DummyDatabase::new());
        let control = &ms.handle.control;

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
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(..)
            ))
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
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
        }]);
        // If we try to use coin selection, the unconfirmed coin will not be used as a candidate
        // and so we get a coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(..)
            ))
        ));
        let res = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap();
        assert!(res.psbt.inputs[0].non_witness_utxo.is_some());
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 2);
        // It has change so no warnings expected.
        assert!(res.warnings.is_empty());
        assert_eq!(
            tx.output[0].script_pubkey,
            dummy_addr.payload().script_pubkey()
        );
        assert_eq!(tx.output[0].value.to_sat(), dummy_value);

        // NOTE: if you are wondering about the usefulness of these tests asserting arbitrary fixed
        // values, that's a belt-and-suspenders check to make sure size and fee calculations do not
        // change unexpectedly. For instance this specific test caught how a change in
        // rust-bitcoin's serialization of transactions with no input silently affected our fee
        // calculation.

        // Transaction is 1 in (P2WSH satisfaction), 2 outs. At 1sat/vb, it's 170 sats fees.
        // At 2sats/vb, it's twice that.
        assert_eq!(tx.output[1].value.to_sat(), 89_830);
        let res = control
            .create_spend(&destinations, &[dummy_op], 2, None)
            .unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.output[1].value.to_sat(), 89_660);

        // A feerate of 555 won't trigger the sanity checks (they were previously not taking the
        // satisfaction size into account and overestimating the feerate).
        control
            .create_spend(&destinations, &[dummy_op], 555, None)
            .unwrap();

        // If we ask for a too high feerate, or a too large/too small output, it'll fail.
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 10_000, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(..)
            ))
        ));
        *destinations.get_mut(&dummy_addr).unwrap() = 100_001;
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(..)
            ))
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
            bitcoin::Address::new(bitcoin::Network::Testnet, dummy_addr.payload().clone());
        let invalid_destinations: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(invalid_addr, dummy_value)].iter().cloned().collect();
        assert!(matches!(
            control.create_spend(&invalid_destinations, &[dummy_op], 1, None),
            Err(CommandError::Address(
                address::Error::NetworkValidation { .. }
            ))
        ));

        // If we ask for a large, but valid, output we won't get a change output. 95_000 because we
        // won't create an output lower than 5k sats.
        *destinations.get_mut(&dummy_addr).unwrap() = 95_000;
        let res = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 1);
        assert_eq!(
            tx.output[0].script_pubkey,
            dummy_addr.payload().script_pubkey()
        );
        assert_eq!(tx.output[0].value.to_sat(), 95_000);
        // change = 100_000 - 95_000 - /* fee without change */ 127 - /* extra fee for change output */ 43 = 4830
        assert_eq!(res.warnings, vec!["Change amount of 4830 sats added to fee as it was too small to create a transaction output."]);

        // Increase the target value by the change amount and the warning will disappear.
        *destinations.get_mut(&dummy_addr).unwrap() = 95_000 + 4_830;
        let res = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert!(res.warnings.is_empty());

        // Now increase target also by the extra fee that was paying for change and we can still create the spend.
        *destinations.get_mut(&dummy_addr).unwrap() =
            95_000 + 4_830 + /* fee for change output */ 43;
        let res = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert!(res.warnings.is_empty());

        // Now increase the target by 1 more sat and we will have insufficient funds.
        *destinations.get_mut(&dummy_addr).unwrap() =
            95_000 + 4_830 + /* fee for change output */ 43 + 1;
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(InsufficientFunds { missing: 1 })
            ))
        );

        // Now decrease the target so that the lost change is just 1 sat.
        *destinations.get_mut(&dummy_addr).unwrap() =
            100_000 - /* fee without change */ 127 - /* extra fee for change output */ 43 - 1;
        let res = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap();
        // Message uses "sat" instead of "sats" when value is 1.
        assert_eq!(res.warnings, vec!["Change amount of 1 sat added to fee as it was too small to create a transaction output."]);

        // Now decrease the target value so that we have enough for a change output.
        *destinations.get_mut(&dummy_addr).unwrap() =
            95_000 - /* fee without change */ 127 - /* extra fee for change output */ 43;
        let res = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 2);
        assert_eq!(tx.output[1].value.to_sat(), 5_000);
        assert!(res.warnings.is_empty());

        // Now increase the target by 1 and we'll get a warning again, this time for 1 less than the dust threshold.
        *destinations.get_mut(&dummy_addr).unwrap() =
            95_000 - /* fee without change */ 127 - /* extra fee for change output */ 43 + 1;
        let res = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap();
        assert_eq!(res.warnings, vec!["Change amount of 4999 sats added to fee as it was too small to create a transaction output."]);

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
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(..)
            ))
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
        }]);
        // Even though 1_000 is the max feerate allowed by our sanity check, we need to
        // use 1_003 in order to exceed it and fail this test since coin selection is
        // based on a minimum feerate of `feerate_vb / 4.0` sats/wu, which can result in
        // the sats/vb feerate being lower than `feerate_vb`.
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op_dup], 1_003, None),
            Err(CommandError::SpendCreation(SpendCreationError::InsaneFees(
                InsaneFeeInfo::TooHighFeerate(1_001)
            )))
        );

        // Add a confirmed unspent coin to be used for coin selection.
        let confirmed_op_1 = bitcoin::OutPoint {
            txid: dummy_op.txid,
            vout: dummy_op.vout + 100,
        };
        db_conn.new_unspent_coins(&[Coin {
            outpoint: confirmed_op_1,
            is_immature: false,
            block_info: Some(BlockInfo {
                height: 174500,
                time: 174500,
            }),
            amount: bitcoin::Amount::from_sat(80_000),
            derivation_index: bip32::ChildNumber::from(42),
            is_change: false,
            spend_txid: None,
            spend_block: None,
        }]);
        // Coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(..)
            ))
        ));
        // Set destination amount equal to value of confirmed coins.
        *destinations.get_mut(&dummy_addr).unwrap() = 80_000;
        // Coin selection error occurs due to insufficient funds to pay fee.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(..)
            ))
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
        }]);
        // First, create a transaction using auto coin selection.
        let res_auto = control.create_spend(&destinations, &[], 1, None).unwrap();
        let tx_auto = res_auto.psbt.unsigned_tx;
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
            dummy_addr.payload().script_pubkey()
        );
        assert_eq!(tx_auto.output[0].value, Amount::from_sat(80_000));

        // Create a second transaction using manual coin selection.
        let res_manual = control
            .create_spend(&destinations, &[confirmed_op_1, confirmed_op_2], 1, None)
            .unwrap();
        let tx_manual = res_manual.psbt.unsigned_tx;
        // Check that manual and auto selection give same outputs (including change).
        assert_eq!(tx_auto.output, tx_manual.output);
        // Check inputs are also the same. Need to sort as order is not guaranteed by `create_spend`.
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
        }]);
        let empty_dest = &HashMap::<bitcoin::Address<address::NetworkUnchecked>, u64>::new();
        assert!(matches!(
            control.create_spend(empty_dest, &[confirmed_op_3], 5, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(..)
            ))
        ));
        // If we use a lower fee, the self-send will succeed.
        let res = control
            .create_spend(empty_dest, &[confirmed_op_3], 1, None)
            .unwrap();
        let tx = res.psbt.unsigned_tx;
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
        let control = &ms.handle.control;
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
        let mut psbt_a = control
            .create_spend(&destinations_a, &[dummy_op_a], 1, None)
            .unwrap()
            .psbt;
        let txid_a = psbt_a.unsigned_tx.txid();
        let psbt_b = control
            .create_spend(&destinations_b, &[dummy_op_b], 10, None)
            .unwrap()
            .psbt;
        let txid_b = psbt_b.unsigned_tx.txid();
        let psbt_c = control
            .create_spend(&destinations_c, &[dummy_op_a, dummy_op_b], 100, None)
            .unwrap()
            .psbt;
        let txid_c = psbt_c.unsigned_tx.txid();

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
        let dummy_txid_a = dummy_psbt_a.unsigned_tx.txid();
        dummy_bitcoind.txs.insert(dummy_txid_a, (dummy_tx_a, None));
        let ms = DummyLiana::new(dummy_bitcoind, DummyDatabase::new());
        let control = &ms.handle.control;
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
                    txid: deposit1.txid(),
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
                    txid: deposit1.txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 1, time: 1 }),
                spend_block: Some(BlockInfo { height: 3, time: 3 }),
                derivation_index: ChildNumber::from(0),
                amount: bitcoin::Amount::from_sat(100_000_000),
                spend_txid: Some(spend_tx.txid()),
            },
            // Deposit 2
            Coin {
                is_change: false,
                is_immature: false,
                outpoint: OutPoint {
                    txid: deposit2.txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 2, time: 2 }),
                spend_block: None,
                derivation_index: ChildNumber::from(1),
                amount: bitcoin::Amount::from_sat(2000),
                spend_txid: None,
            },
            // This coin is a change output.
            Coin {
                is_change: true,
                is_immature: false,
                outpoint: OutPoint::new(spend_tx.txid(), 1),
                block_info: Some(BlockInfo { height: 3, time: 3 }),
                spend_block: None,
                derivation_index: ChildNumber::from(2),
                amount: bitcoin::Amount::from_sat(100_000_000 - 4000 - 1000),
                spend_txid: None,
            },
            // Deposit 3
            Coin {
                is_change: false,
                is_immature: false,
                outpoint: OutPoint {
                    txid: deposit3.txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 4, time: 4 }),
                spend_block: None,
                derivation_index: ChildNumber::from(3),
                amount: bitcoin::Amount::from_sat(3000),
                spend_txid: None,
            },
        ]);

        let mut btc = DummyBitcoind::new();
        btc.txs.insert(
            deposit1.txid(),
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
        btc.txs.insert(
            deposit2.txid(),
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
        btc.txs.insert(
            spend_tx.txid(),
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
        btc.txs.insert(
            deposit3.txid(),
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

        let ms = DummyLiana::new(btc, db);

        let control = &ms.handle.control;

        let transactions = control.list_confirmed_transactions(0, 4, 10).transactions;
        assert_eq!(transactions.len(), 4);

        assert_eq!(transactions[0].time, Some(4));
        assert_eq!(transactions[0].tx, deposit3);

        assert_eq!(transactions[1].time, Some(3));
        assert_eq!(transactions[1].tx, spend_tx);

        assert_eq!(transactions[2].time, Some(2));
        assert_eq!(transactions[2].tx, deposit2);

        assert_eq!(transactions[3].time, Some(1));
        assert_eq!(transactions[3].tx, deposit1);

        let transactions = control.list_confirmed_transactions(2, 3, 10).transactions;
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

        let mut btc = DummyBitcoind::new();
        btc.txs.insert(
            tx1.txid(),
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
        btc.txs.insert(
            tx2.txid(),
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
        btc.txs.insert(
            tx3.txid(),
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

        let ms = DummyLiana::new(btc, DummyDatabase::new());

        let control = &ms.handle.control;

        let transactions = control.list_transactions(&[tx1.txid()]).transactions;
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].tx, tx1);

        let transactions = control
            .list_transactions(&[tx1.txid(), tx2.txid(), tx3.txid()])
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
}
