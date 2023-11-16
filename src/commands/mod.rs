//! # Liana commands
//!
//! External interface to the Liana daemon.

mod utils;

use crate::{
    bitcoin::BitcoinInterface,
    database::{Coin, DatabaseInterface},
    descriptors, DaemonControl, VERSION,
};

pub use crate::database::{CoinStatus, LabelItem};

use bdk_coin_select::InsufficientFunds;
use utils::{
    deser_addr_assume_checked, deser_amount_from_sats, deser_fromstr, deser_hex,
    select_coins_for_spend, ser_amount, ser_hex, ser_to_string,
};

use std::{
    collections::{hash_map, BTreeMap, HashMap, HashSet},
    convert::TryInto,
    fmt,
};

use miniscript::{
    bitcoin::{
        self, address, bip32,
        locktime::absolute,
        psbt::{Input as PsbtIn, Output as PsbtOut, PartiallySignedTransaction as Psbt},
    },
    psbt::PsbtExt,
};
use serde::{Deserialize, Serialize};

// We would never create a transaction with an output worth less than this.
// That's 1$ at 20_000$ per BTC.
const DUST_OUTPUT_SATS: u64 = 5_000;

// Long-term feerate (sats/vb) used for coin selection considerations.
const LONG_TERM_FEERATE_VB: f32 = 10.0;

// Assume that paying more than 1BTC in fee is a bug.
const MAX_FEE: u64 = bitcoin::blockdata::constants::COIN_VALUE;

// Assume that paying more than 1000sat/vb in feerate is a bug.
const MAX_FEERATE: u64 = 1_000;

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
    InvalidOutputValue(bitcoin::Amount),
    InsufficientFunds(
        /* in value */ bitcoin::Amount,
        /* out value */ Option<bitcoin::Amount>,
        /* target feerate */ u64,
    ),
    InsaneFees(InsaneFeeInfo),
    FetchingTransaction(bitcoin::OutPoint),
    SanityCheckFailure(Psbt),
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
    CoinSelectionError(InsufficientFunds),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoOutpointForSelfSend => write!(f, "No provided outpoint for self-send. Need at least one."),
            Self::InvalidFeerate(sats_vb) => write!(f, "Invalid feerate: {} sats/vb.", sats_vb),
            Self::AlreadySpent(op) => write!(f, "Coin at '{}' is already spent.", op),
            Self::ImmatureCoinbase(op) => write!(f, "Coin at '{}' is from an immature coinbase transaction.", op),
            Self::UnknownOutpoint(op) => write!(f, "Unknown outpoint '{}'.", op),
            Self::Address(e) => write!(
                f,
                "Address error: {}", e
            ),
            Self::InvalidOutputValue(amount) => write!(f, "Invalid output value '{}'.", amount),
            Self::InsufficientFunds(in_val, out_val, feerate) => if let Some(out_val) = out_val {
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
            },
            Self::InsaneFees(info) => write!(
                f,
                "We assume transactions with a fee larger than {} sats or a feerate larger than {} sats/vb are a mistake. \
                The created transaction {}.",
                MAX_FEE,
                MAX_FEERATE,
                match info {
                    InsaneFeeInfo::NegativeFee => "would have a negative fee".to_string(),
                    InsaneFeeInfo::TooHighFee(f) => format!("{} sats in fees", f),
                    InsaneFeeInfo::InvalidFeerate => "would have an invalid feerate".to_string(),
                    InsaneFeeInfo::TooHighFeerate(r) => format!("has a feerate of {} sats/vb", r),
                },
            ),
            Self::FetchingTransaction(op) => {
                write!(f, "Could not fetch transaction for coin {}", op)
            }
            Self::SanityCheckFailure(psbt) => write!(
                f,
                "BUG! Please report this. Failed sanity checks for PSBT '{}'.",
                psbt
            ),
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
            Self::InvalidDerivationIndex => write!(f, "Unhardened or overflowing BIP32 derivation index."),
           Self::CoinSelectionError(e) => write!(f, "Coin selection error: '{}'", e),
        }
    }
}

impl std::error::Error for CommandError {}

// Sanity check the value of a transaction output.
fn check_output_value(value: bitcoin::Amount) -> Result<(), CommandError> {
    // NOTE: the network parameter isn't used upstream
    if value.to_sat() > bitcoin::blockdata::constants::MAX_MONEY
        || value.to_sat() < DUST_OUTPUT_SATS
    {
        Err(CommandError::InvalidOutputValue(value))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsaneFeeInfo {
    NegativeFee,
    InvalidFeerate,
    TooHighFee(u64),
    TooHighFeerate(u64),
}

/// A candidate for coin selection when creating a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CandidateCoin {
    /// The candidate coin.
    coin: Coin,
    /// Whether or not this coin must be selected by the coin selection algorithm.
    must_select: bool,
}

// Apply some sanity checks on a created transaction's PSBT.
// TODO: add more sanity checks from revault_tx
fn sanity_check_psbt(
    spent_desc: &descriptors::LianaDescriptor,
    psbt: &Psbt,
) -> Result<(), CommandError> {
    let tx = &psbt.unsigned_tx;

    // Must have as many in/out in the PSBT and Bitcoin tx.
    if psbt.inputs.len() != tx.input.len()
        || psbt.outputs.len() != tx.output.len()
        || tx.output.is_empty()
    {
        return Err(CommandError::SanityCheckFailure(psbt.clone()));
    }

    // Compute the transaction input value, checking all PSBT inputs have the derivation
    // index set for signing devices to recognize them as ours.
    let mut value_in = 0;
    for psbtin in psbt.inputs.iter() {
        if psbtin.bip32_derivation.is_empty() {
            return Err(CommandError::SanityCheckFailure(psbt.clone()));
        }
        value_in += psbtin
            .witness_utxo
            .as_ref()
            .ok_or_else(|| CommandError::SanityCheckFailure(psbt.clone()))?
            .value;
    }

    // Compute the output value and check the absolute fee isn't insane.
    let value_out: u64 = tx.output.iter().map(|o| o.value).sum();
    let abs_fee = value_in
        .checked_sub(value_out)
        .ok_or(CommandError::InsaneFees(InsaneFeeInfo::NegativeFee))?;
    if abs_fee > MAX_FEE {
        return Err(CommandError::InsaneFees(InsaneFeeInfo::TooHighFee(abs_fee)));
    }

    // Check the feerate isn't insane.
    // Add weights together before converting to vbytes to avoid rounding up multiple times
    // and increasing the result, which could lead to the feerate in sats/vb falling below 1.
    let tx_wu = tx.weight().to_wu() + (spent_desc.max_sat_weight() * tx.input.len()) as u64;
    let tx_vb = tx_wu
        .checked_add(descriptors::WITNESS_FACTOR as u64 - 1)
        .unwrap()
        .checked_div(descriptors::WITNESS_FACTOR as u64)
        .unwrap();
    let feerate_sats_vb = abs_fee
        .checked_div(tx_vb)
        .ok_or(CommandError::InsaneFees(InsaneFeeInfo::InvalidFeerate))?;
    if !(1..=MAX_FEERATE).contains(&feerate_sats_vb) {
        return Err(CommandError::InsaneFees(InsaneFeeInfo::TooHighFeerate(
            feerate_sats_vb,
        )));
    }

    // Check for dust outputs
    for txo in psbt.unsigned_tx.output.iter() {
        if txo.value < txo.script_pubkey.dust_value().to_sat() {
            return Err(CommandError::SanityCheckFailure(psbt.clone()));
        }
    }

    Ok(())
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
        let addresses: Result<Vec<AddressInfo>, _> = (start_index_u32..end_index)
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
                    outpoint,
                    block_height,
                    spend_info,
                    is_immature,
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
    ) -> Result<CreateSpendResult, CommandError> {
        // This method is a bit convoluted, but it's the nature of creating a Bitcoin transaction
        // with a target feerate and outputs. In addition, we support different modes (coin control
        // vs automated coin selection, self-spend, etc..) which make the logic a bit more
        // intricate. Here is a brief overview of what we're doing here:
        // 1. Create a transaction with all the target outputs (if this is a self-send, none are
        //    added at this step the only output will be added as a change output).
        // 2. Automatically select the coins if necessary and determine whether a change output
        //    will be necessary for this transaction from the set of (automatically or manually)
        //    selected coins. The output for a self-send is added there.
        // 3. Fetch the selected coins from database and add them as inputs to the transaction.
        // 4. Finalize the PSBT and sanity check it before returning it.

        let is_self_send = destinations.is_empty();
        // For self-send, the coins must be specified.
        if is_self_send && coins_outpoints.is_empty() {
            return Err(CommandError::NoOutpointForSelfSend);
        }
        if feerate_vb < 1 {
            return Err(CommandError::InvalidFeerate(feerate_vb));
        }
        let mut db_conn = self.db.connection();

        // Create transaction with no inputs and no outputs.
        let mut tx = bitcoin::Transaction {
            version: 2,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO), // TODO: randomized anti fee sniping
            input: Vec::with_capacity(coins_outpoints.len()), // Will be zero capacity for coin selection.
            output: Vec::with_capacity(destinations.len()),
        };
        // Add the destinations outputs to the transaction and PSBT. At the same time
        // sanity check each output's value.
        let mut psbt_outs = Vec::with_capacity(destinations.len());
        for (address, value_sat) in destinations {
            let address = self.validate_address(address.clone())?;

            let amount = bitcoin::Amount::from_sat(*value_sat);
            check_output_value(amount)?;

            tx.output.push(bitcoin::TxOut {
                value: amount.to_sat(),
                script_pubkey: address.script_pubkey(),
            });
            // If it's an address of ours, signal it as change to signing devices by adding the
            // BIP32 derivation path to the PSBT output.
            let bip32_derivation =
                if let Some((index, is_change)) = db_conn.derivation_index_by_address(&address) {
                    let desc = if is_change {
                        self.config.main_descriptor.change_descriptor()
                    } else {
                        self.config.main_descriptor.receive_descriptor()
                    };
                    desc.derive(index, &self.secp).bip32_derivations()
                } else {
                    Default::default()
                };
            psbt_outs.push(PsbtOut {
                bip32_derivation,
                ..PsbtOut::default()
            });
        }
        assert_eq!(tx.output.is_empty(), is_self_send);

        // Now compute whether we'll need a change output while automatically selecting coins to be
        // used as input if necessary.
        // We need to get the size of a potential change output to select coins / determine whether
        // we should include one, so get a change address and create a dummy txo for this purpose.
        let change_index = db_conn.change_index();
        let change_desc = self
            .config
            .main_descriptor
            .change_descriptor()
            .derive(change_index, &self.secp);
        let mut change_txo = bitcoin::TxOut {
            value: std::u64::MAX,
            script_pubkey: change_desc.script_pubkey(),
        };
        // Now, either select the coins necessary or use the ones provided (verifying they do in
        // fact exist and are still unspent) and determine whether there is any leftover to create a
        // change output.
        let (selected_coins, change_amount) = {
            let candidate_coins: Vec<CandidateCoin> = if coins_outpoints.is_empty() {
                // We only select confirmed coins for now. Including unconfirmed ones as well would
                // introduce a whole bunch of additional complexity.
                db_conn
                    .coins(&[CoinStatus::Confirmed], &[])
                    .into_values()
                    .map(|c| CandidateCoin {
                        coin: c,
                        must_select: false, // No coin is mandatory.
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
                    .map(|c| CandidateCoin {
                        coin: c,
                        must_select: true, // All coins must be selected.
                    })
                    .collect()
            };
            // At this point the transaction still has no input and no change output, as expected
            // by the coins selection helper function.
            assert!(tx.input.is_empty());
            assert_eq!(tx.output.len(), destinations.len());
            // TODO: Introduce general conversion error type.
            let feerate_vb: f32 = {
                let fr: u16 = feerate_vb.try_into().map_err(|_| {
                    CommandError::InsaneFees(InsaneFeeInfo::TooHighFeerate(feerate_vb))
                })?;
                fr
            }
            .try_into()
            .expect("u16 must fit in f32");
            let max_sat_wu = self
                .config
                .main_descriptor
                .max_sat_weight()
                .try_into()
                .expect("Weight must fit in a u32");
            select_coins_for_spend(
                &candidate_coins,
                tx.clone(),
                change_txo.clone(),
                feerate_vb,
                0, // We only constrain the feerate.
                max_sat_wu,
            )
            .map_err(CommandError::CoinSelectionError)?
        };
        // If necessary, add a change output.
        if change_amount.to_sat() > 0 {
            // Don't forget to update our next change index!
            let next_index = change_index
                .increment()
                .expect("Must not get into hardened territory");
            db_conn.set_change_index(next_index, &self.secp);
            check_output_value(change_amount)?;

            // TODO: shuffle once we have Taproot
            change_txo.value = change_amount.to_sat();
            tx.output.push(change_txo);
            psbt_outs.push(PsbtOut {
                bip32_derivation: change_desc.bip32_derivations(),
                ..PsbtOut::default()
            });
        } else if is_self_send {
            return Err(CommandError::InsufficientFunds(
                selected_coins.iter().map(|c| c.amount).sum(),
                None,
                feerate_vb,
            ));
        }

        // Iterate through selected coins and add necessary information to the PSBT inputs.
        let mut psbt_ins = Vec::with_capacity(selected_coins.len());
        let mut spent_txs = HashMap::with_capacity(selected_coins.len());
        for coin in &selected_coins {
            // Fetch the transaction that created it if necessary
            if let hash_map::Entry::Vacant(e) = spent_txs.entry(coin.outpoint) {
                let tx = self
                    .bitcoin
                    .wallet_transaction(&coin.outpoint.txid)
                    .ok_or(CommandError::FetchingTransaction(coin.outpoint))?;
                e.insert(tx.0);
            }

            tx.input.push(bitcoin::TxIn {
                previous_output: coin.outpoint,
                sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                // TODO: once we move to Taproot, anti-fee-sniping using nSequence
                ..bitcoin::TxIn::default()
            });

            // Populate the PSBT input with the information needed by signers.
            let coin_desc = self.derived_desc(coin);
            let witness_script = Some(coin_desc.witness_script());
            let witness_utxo = Some(bitcoin::TxOut {
                value: coin.amount.to_sat(),
                script_pubkey: coin_desc.script_pubkey(),
            });
            let non_witness_utxo = spent_txs.get(&coin.outpoint).cloned();
            let bip32_derivation = coin_desc.bip32_derivations();
            psbt_ins.push(PsbtIn {
                witness_script,
                witness_utxo,
                bip32_derivation,
                non_witness_utxo,
                ..PsbtIn::default()
            });
        }

        // Finally, create the PSBT with all inputs and outputs, sanity check it and return it.
        let psbt = Psbt {
            unsigned_tx: tx,
            version: 0,
            xpub: BTreeMap::new(),
            proprietary: BTreeMap::new(),
            unknown: BTreeMap::new(),
            inputs: psbt_ins,
            outputs: psbt_outs,
        };
        sanity_check_psbt(&self.config.main_descriptor, &psbt)?;
        // TODO: maybe check for common standardness rules (max size, ..)?

        Ok(CreateSpendResult { psbt })
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
        let final_tx = spend_psbt.extract_tx();
        self.bitcoin
            .broadcast_tx(&final_tx)
            .map_err(CommandError::TxBroadcast)
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
        let address = self.validate_address(address)?;
        let mut db_conn = self.db.connection();

        // The transaction template. We'll fill-in the inputs afterward.
        let mut psbt = Psbt {
            unsigned_tx: bitcoin::Transaction {
                version: 2,
                lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO), // TODO: anti-fee sniping
                input: Vec::new(),
                output: vec![bitcoin::TxOut {
                    script_pubkey: address.script_pubkey(),
                    value: 0xFF_FF_FF_FF,
                }],
            },
            version: 0,
            xpub: BTreeMap::new(),
            proprietary: BTreeMap::new(),
            unknown: BTreeMap::new(),
            inputs: Vec::new(),
            outputs: vec![PsbtOut::default()],
        };

        // Query the coins that we can spend through the specified recovery path (if no recovery
        // path specified, use the first available one) from the database.
        let current_height = self.bitcoin.chain_tip().height;
        let timelock =
            timelock.unwrap_or_else(|| self.config.main_descriptor.first_timelock_value());
        let height_delta: i32 = timelock.try_into().expect("Must fit, it's a u16");
        let sweepable_coins = db_conn
            .coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
            .into_values()
            .filter(|c| {
                // We are interested in coins available at the *next* block
                c.block_info
                    .map(|b| current_height + 1 >= b.height + height_delta)
                    .unwrap_or(false)
            });

        // Fill-in the transaction inputs and PSBT inputs information. Record the value
        // that is fed to the transaction while doing so, to compute the fees afterward.
        let mut in_value = bitcoin::Amount::from_sat(0);
        let txin_sat_vb = self.config.main_descriptor.max_sat_vbytes();
        let mut sat_vb = 1; // Start at 1 for the segwit marker size, rounded up.
        let mut spent_txs = HashMap::new();
        for coin in sweepable_coins {
            in_value += coin.amount;
            psbt.unsigned_tx.input.push(bitcoin::TxIn {
                previous_output: coin.outpoint,
                sequence: bitcoin::Sequence::from_height(timelock),
                // TODO: once we move to Taproot, anti-fee-sniping using nSequence
                ..bitcoin::TxIn::default()
            });

            // Fetch the transaction that created this coin if necessary
            if let hash_map::Entry::Vacant(e) = spent_txs.entry(coin.outpoint) {
                let tx = self
                    .bitcoin
                    .wallet_transaction(&coin.outpoint.txid)
                    .ok_or(CommandError::FetchingTransaction(coin.outpoint))?;
                e.insert(tx.0);
            }

            let coin_desc = self.derived_desc(&coin);
            sat_vb += txin_sat_vb;
            let witness_script = Some(coin_desc.witness_script());
            let witness_utxo = Some(bitcoin::TxOut {
                value: coin.amount.to_sat(),
                script_pubkey: coin_desc.script_pubkey(),
            });
            let non_witness_utxo = spent_txs.get(&coin.outpoint).cloned();
            let bip32_derivation = coin_desc.bip32_derivations();
            psbt.inputs.push(PsbtIn {
                witness_script,
                witness_utxo,
                non_witness_utxo,
                bip32_derivation,
                ..PsbtIn::default()
            });
        }

        // The sweepable_coins iterator may have been empty.
        if psbt.unsigned_tx.input.is_empty() {
            return Err(CommandError::RecoveryNotAvailable);
        }

        // Compute the value of the single output based on the requested feerate.
        let tx_vbytes = (psbt.unsigned_tx.vsize() + sat_vb) as u64;
        let absolute_fee = bitcoin::Amount::from_sat(tx_vbytes.checked_mul(feerate_vb).unwrap());
        let output_value = in_value
            .checked_sub(absolute_fee)
            .ok_or(CommandError::InsufficientFunds(in_value, None, feerate_vb))?;
        psbt.unsigned_tx.output[0].value = output_value.to_sat();

        sanity_check_psbt(&self.config.main_descriptor, &psbt)?;

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
    /// Information about the transaction spending this coin.
    pub spend_info: Option<LCSpendInfo>,
    /// Whether this coin was created by a coinbase transaction that is still immature.
    pub is_immature: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsResult {
    pub coins: Vec<ListCoinsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSpendResult {
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    pub psbt: Psbt,
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

    use bitcoin::{
        bip32::{self, ChildNumber},
        blockdata::transaction::{TxIn, TxOut},
        locktime::absolute,
        OutPoint, ScriptBuf, Sequence, Transaction, Txid, Witness,
    };
    use std::str::FromStr;

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
                    version: 2,
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
            control.create_spend(&destinations, &[], 1),
            Err(CommandError::NoOutpointForSelfSend)
        );
        destinations = [(dummy_addr.clone(), dummy_value)]
            .iter()
            .cloned()
            .collect();
        // Insufficient funds for coin selection.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1),
            Err(CommandError::CoinSelectionError(..))
        ));
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 0),
            Err(CommandError::InvalidFeerate(0))
        );

        // The coin doesn't exist. If we create a new unspent one at this outpoint with a much
        // higher value, we'll get a Spend transaction with a change output.
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1),
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
            control.create_spend(&destinations, &[], 1),
            Err(CommandError::CoinSelectionError(..))
        ));
        let res = control.create_spend(&destinations, &[dummy_op], 1).unwrap();
        assert!(res.psbt.inputs[0].non_witness_utxo.is_some());
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(
            tx.output[0].script_pubkey,
            dummy_addr.payload.script_pubkey()
        );
        assert_eq!(tx.output[0].value, dummy_value);

        // Transaction is 1 in (P2WSH satisfaction), 2 outs. At 1sat/vb, it's 170 sats fees.
        // At 2sats/vb, it's twice that.
        assert_eq!(tx.output[1].value, 89_830);
        let res = control.create_spend(&destinations, &[dummy_op], 2).unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.output[1].value, 89_660);

        // A feerate of 555 won't trigger the sanity checks (they were previously not taking the
        // satisfaction size into account and overestimating the feerate).
        control
            .create_spend(&destinations, &[dummy_op], 555)
            .unwrap();

        // If we ask for a too high feerate, or a too large/too small output, it'll fail.
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 10_000),
            Err(CommandError::CoinSelectionError(..))
        ));
        *destinations.get_mut(&dummy_addr).unwrap() = 100_001;
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 1),
            Err(CommandError::CoinSelectionError(..))
        ));
        *destinations.get_mut(&dummy_addr).unwrap() = 4_500;
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1),
            Err(CommandError::InvalidOutputValue(bitcoin::Amount::from_sat(
                4_500
            )))
        );

        // If we ask to create an output for an address from another network, it will fail.
        let invalid_addr =
            bitcoin::Address::new(bitcoin::Network::Testnet, dummy_addr.payload.clone());
        let invalid_destinations: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(invalid_addr, dummy_value)].iter().cloned().collect();
        assert!(matches!(
            control.create_spend(&invalid_destinations, &[dummy_op], 1),
            Err(CommandError::Address(
                address::Error::NetworkValidation { .. }
            ))
        ));

        // If we ask for a large, but valid, output we won't get a change output. 95_000 because we
        // won't create an output lower than 5k sats.
        *destinations.get_mut(&dummy_addr).unwrap() = 95_000;
        let res = control.create_spend(&destinations, &[dummy_op], 1).unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 1);
        assert_eq!(
            tx.output[0].script_pubkey,
            dummy_addr.payload.script_pubkey()
        );
        assert_eq!(tx.output[0].value, 95_000);

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
            control.create_spend(&destinations, &[dummy_op], 1),
            Err(CommandError::AlreadySpent(dummy_op))
        );
        // If we try to use coin selection, the spent coin will not be used as a candidate
        // and so we get a coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1),
            Err(CommandError::CoinSelectionError(..))
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
            control.create_spend(&destinations, &[dummy_op_dup], 1_003),
            Err(CommandError::InsaneFees(InsaneFeeInfo::TooHighFeerate(
                1_001
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
            control.create_spend(&destinations, &[], 1),
            Err(CommandError::CoinSelectionError(..))
        ));
        // Set destination amount equal to value of confirmed coins.
        *destinations.get_mut(&dummy_addr).unwrap() = 80_000;
        // Coin selection error occurs due to insufficient funds to pay fee.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1),
            Err(CommandError::CoinSelectionError(..))
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
        let res_auto = control.create_spend(&destinations, &[], 1).unwrap();
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
            dummy_addr.payload.script_pubkey()
        );
        assert_eq!(tx_auto.output[0].value, 80_000);

        // Create a second transaction using manual coin selection.
        let res_manual = control
            .create_spend(&destinations, &[confirmed_op_1, confirmed_op_2], 1)
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
        assert_eq!(
            control.create_spend(empty_dest, &[confirmed_op_3], 5),
            Err(CommandError::InsufficientFunds(
                bitcoin::Amount::from_sat(5_250),
                None,
                5
            ))
        );
        // If we use a lower fee, the self-send will succeed.
        let res = control
            .create_spend(empty_dest, &[confirmed_op_3], 1)
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
            control.create_spend(&destinations, &[imma_op], 1_001),
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
            version: 2,
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
            .create_spend(&destinations_a, &[dummy_op_a], 1)
            .unwrap()
            .psbt;
        let txid_a = psbt_a.unsigned_tx.txid();
        let psbt_b = control
            .create_spend(&destinations_b, &[dummy_op_b], 10)
            .unwrap()
            .psbt;
        let txid_b = psbt_b.unsigned_tx.txid();
        let psbt_c = control
            .create_spend(&destinations_c, &[dummy_op_a, dummy_op_b], 100)
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
    fn list_confirmed_transactions() {
        let outpoint = OutPoint::new(
            Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                .unwrap(),
            0,
        );

        let deposit1: Transaction = Transaction {
            version: 1,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: 100_000_000,
            }],
        };

        let deposit2: Transaction = Transaction {
            version: 1,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: 2000,
            }],
        };

        let deposit3: Transaction = Transaction {
            version: 1,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: 3000,
            }],
        };

        let spend_tx: Transaction = Transaction {
            version: 1,
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
                    value: 4000,
                },
                TxOut {
                    script_pubkey: ScriptBuf::new(),
                    value: 100_000_000 - 4000 - 1000,
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
            version: 1,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: 100_000_000,
            }],
        };

        let tx2: Transaction = Transaction {
            version: 1,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: 2000,
            }],
        };

        let tx3: Transaction = Transaction {
            version: 1,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: 3000,
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
