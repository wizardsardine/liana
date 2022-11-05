//! # Minisafe commands
//!
//! External interface to the Minisafe daemon.

mod utils;

use crate::{
    bitcoin::{BitcoinError, BitcoinInterface},
    database::{Coin, DatabaseInterface},
    descriptors, DaemonControl, VERSION,
};
use utils::{change_index, deser_amount_from_sats, deser_psbt_base64, ser_amount, ser_base64};

use std::{
    collections::{BTreeMap, HashMap},
    convert::TryInto,
    fmt,
};

use miniscript::{
    bitcoin::{
        self,
        util::psbt::{Input as PsbtIn, Output as PsbtOut, PartiallySignedTransaction as Psbt},
    },
    psbt::PsbtExt,
};
use serde::{Deserialize, Serialize};

const WITNESS_FACTOR: usize = 4;

// We would never create a transaction with an output worth less than this.
// That's 1$ at 20_000$ per BTC.
const DUST_OUTPUT_SATS: u64 = 5_000;

// Assume that paying more than 1BTC in fee is a bug.
const MAX_FEE: u64 = bitcoin::blockdata::constants::COIN_VALUE;

// Assume that paying more than 1000sat/vb in feerate is a bug.
const MAX_FEERATE: u64 = bitcoin::blockdata::constants::COIN_VALUE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    NoOutpoint,
    NoDestination,
    InvalidFeerate(/* sats/vb */ u64),
    UnknownOutpoint(bitcoin::OutPoint),
    AlreadySpent(bitcoin::OutPoint),
    InvalidOutputValue(bitcoin::Amount),
    InsufficientFunds(
        /* in value */ bitcoin::Amount,
        /* out value */ bitcoin::Amount,
        /* target feerate */ u64,
    ),
    SanityCheckFailure(Psbt),
    UnknownSpend(bitcoin::Txid),
    // FIXME: when upgrading Miniscript put the actual error there
    SpendFinalization(String),
    TxBroadcast(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoOutpoint => write!(f, "No provided outpoint. Need at least one."),
            Self::NoDestination => write!(f, "No provided destination. Need at least one."),
            Self::InvalidFeerate(sats_vb) => write!(f, "Invalid feerate: {} sats/vb.", sats_vb),
            Self::AlreadySpent(op) => write!(f, "Coin at '{}' is already spent.", op),
            Self::UnknownOutpoint(op) => write!(f, "Unknown outpoint '{}'.", op),
            Self::InvalidOutputValue(amount) => write!(f, "Invalid output value '{}'.", amount),
            Self::InsufficientFunds(in_val, out_val, feerate) => write!(
                f,
                "Cannot create a {} sat/vb transaction with input value {} and output value {}",
                feerate, in_val, out_val
            ),
            Self::SanityCheckFailure(psbt) => write!(
                f,
                "BUG! Please report this. Failed sanity checks for PSBT '{:?}'.",
                psbt
            ),
            Self::UnknownSpend(txid) => write!(f, "Unknown spend transaction '{}'.", txid),
            Self::SpendFinalization(e) => {
                write!(f, "Failed to finalize the spend transaction PSBT: '{}'.", e)
            }
            Self::TxBroadcast(e) => write!(f, "Failed to broadcast transaction: '{}'.", e),
        }
    }
}

impl std::error::Error for CommandError {}

// Sanity check the value of a transaction output.
fn check_output_value(value: bitcoin::Amount) -> Result<(), CommandError> {
    // NOTE: the network parameter isn't used upstream
    if value.to_sat() > bitcoin::blockdata::constants::max_money(bitcoin::Network::Bitcoin)
        || value.to_sat() < DUST_OUTPUT_SATS
    {
        Err(CommandError::InvalidOutputValue(value))
    } else {
        Ok(())
    }
}

// Apply some sanity checks on a created transaction's PSBT.
// TODO: add more sanity checks from revault_tx
fn sanity_check_psbt(psbt: &Psbt) -> Result<(), CommandError> {
    let tx = &psbt.unsigned_tx;

    // Must have as many in/out in the PSBT and Bitcoin tx.
    if psbt.inputs.len() != tx.input.len() || psbt.outputs.len() != tx.output.len() {
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
        .ok_or_else(|| CommandError::SanityCheckFailure(psbt.clone()))?;
    if abs_fee > MAX_FEE {
        return Err(CommandError::SanityCheckFailure(psbt.clone()));
    }

    // Check the feerate isn't insane.
    let tx_vb: u64 = tx_vbytes(tx);
    let feerate_sats_vb = abs_fee
        .checked_div(tx_vb)
        .ok_or_else(|| CommandError::SanityCheckFailure(psbt.clone()))?;
    if !(1..=MAX_FEERATE).contains(&feerate_sats_vb) {
        return Err(CommandError::SanityCheckFailure(psbt.clone()));
    }

    Ok(())
}

// Get the maximum satisfaction size in vbytes for this descriptor
fn desc_sat_vb(desc: &descriptors::DerivedInheritanceDescriptor) -> u64 {
    desc.max_sat_weight()
        .checked_div(WITNESS_FACTOR)
        .unwrap()
        .try_into()
        .unwrap()
}

// Get the virtual size of this transaction
fn tx_vbytes(tx: &bitcoin::Transaction) -> u64 {
    tx.weight()
        .checked_div(WITNESS_FACTOR)
        .unwrap()
        .try_into()
        .unwrap()
}

// Get the size of a type that can be serialized (txos, transactions, ..)
fn serializable_size<T: bitcoin::consensus::Encodable + ?Sized>(t: &T) -> u64 {
    bitcoin::consensus::serialize(t).len().try_into().unwrap()
}

impl DaemonControl {
    // Get the derived descriptor for this coin
    fn derived_desc(&self, coin: &Coin) -> descriptors::DerivedInheritanceDescriptor {
        let desc = if coin.is_change {
            self.config.main_descriptor.change_descriptor()
        } else {
            self.config.main_descriptor.receive_descriptor()
        };
        desc.derive(coin.derivation_index, &self.secp)
    }
}

impl DaemonControl {
    /// Get information about the current state of the daemon
    pub fn get_info(&self) -> GetInfoResult {
        let mut db_conn = self.db.connection();

        let blockheight = db_conn.chain_tip().map(|tip| tip.height).unwrap_or(0);
        GetInfoResult {
            version: VERSION.to_string(),
            network: self.config.bitcoin_config.network,
            blockheight,
            sync: self.bitcoin.sync_progress(),
            descriptors: GetInfoDescriptors {
                main: self.config.main_descriptor.clone(),
            },
        }
    }

    /// Get a new deposit address. This will always generate a new deposit address, regardless of
    /// whether it was actually used.
    pub fn get_new_address(&self) -> GetAddressResult {
        let mut db_conn = self.db.connection();
        let index = db_conn.receive_index();
        // TODO: should we wrap around instead of failing?
        db_conn.increment_receive_index(&self.secp);
        let address = self
            .config
            .main_descriptor
            .receive_descriptor()
            .derive(index, &self.secp)
            .address(self.config.bitcoin_config.network);
        GetAddressResult { address }
    }

    /// Get a list of all known coins.
    pub fn list_coins(&self) -> ListCoinsResult {
        let mut db_conn = self.db.connection();
        let coins: Vec<ListCoinsEntry> = db_conn
            .coins()
            // Can't use into_values as of Rust 1.48
            .into_iter()
            .map(|(_, coin)| {
                let Coin {
                    amount,
                    outpoint,
                    block_height,
                    spend_txid,
                    spend_block,
                    ..
                } = coin;
                let spend_info = spend_txid.map(|txid| LCSpendInfo {
                    txid,
                    height: spend_block.map(|b| b.height),
                });
                ListCoinsEntry {
                    amount,
                    outpoint,
                    block_height,
                    spend_info,
                }
            })
            .collect();
        ListCoinsResult { coins }
    }

    pub fn create_spend(
        &self,
        coins_outpoints: &[bitcoin::OutPoint],
        destinations: &HashMap<bitcoin::Address, u64>,
        feerate_vb: u64,
    ) -> Result<CreateSpendResult, CommandError> {
        if coins_outpoints.is_empty() {
            return Err(CommandError::NoOutpoint);
        }
        if destinations.is_empty() {
            return Err(CommandError::NoDestination);
        }
        if feerate_vb < 1 {
            return Err(CommandError::InvalidFeerate(feerate_vb));
        }
        let mut db_conn = self.db.connection();

        // Iterate through given outpoints to fetch the coins (hence checking there existence
        // at the same time). We checked there is at least one, therefore after this loop the
        // list of coins is not empty.
        // While doing so, we record the total input value of the transaction to later compute
        // fees, and add necessary information to the PSBT inputs.
        let mut in_value = bitcoin::Amount::from_sat(0);
        let mut sat_vb = 0;
        let mut txins = Vec::with_capacity(destinations.len());
        let mut psbt_ins = Vec::with_capacity(destinations.len());
        let coins = db_conn.coins_by_outpoints(coins_outpoints);
        for op in coins_outpoints {
            let coin = coins.get(op).ok_or(CommandError::UnknownOutpoint(*op))?;
            if coin.is_spent() {
                return Err(CommandError::AlreadySpent(*op));
            }
            in_value += coin.amount;
            txins.push(bitcoin::TxIn {
                previous_output: *op,
                // TODO: once we move to Taproot, anti-fee-sniping using nSequence
                ..bitcoin::TxIn::default()
            });

            let coin_desc = self.derived_desc(coin);
            sat_vb += desc_sat_vb(&coin_desc);
            let witness_script = Some(coin_desc.witness_script());
            let witness_utxo = Some(bitcoin::TxOut {
                value: coin.amount.to_sat(),
                script_pubkey: coin_desc.script_pubkey(),
            });
            let bip32_derivation = coin_desc.bip32_derivations();
            psbt_ins.push(PsbtIn {
                witness_script,
                witness_utxo,
                bip32_derivation,
                ..PsbtIn::default()
            });
        }

        // Add the destinations outputs to the transaction and PSBT. At the same time record the
        // total output value to later compute fees, and sanity check each output's value.
        let mut out_value = bitcoin::Amount::from_sat(0);
        let mut txouts = Vec::with_capacity(destinations.len());
        let mut psbt_outs = Vec::with_capacity(destinations.len());
        for (address, value_sat) in destinations {
            let amount = bitcoin::Amount::from_sat(*value_sat);
            check_output_value(amount)?;
            out_value = out_value.checked_add(amount).unwrap();

            txouts.push(bitcoin::TxOut {
                value: amount.to_sat(),
                script_pubkey: address.script_pubkey(),
            });
            // TODO: if it's an address of ours, signal it as change to signing devices by adding
            // the BIP32 derivation path to the PSBT output.
            psbt_outs.push(PsbtOut::default());
        }

        // Now create the transaction, compute its fees and already sanity check if its feerate
        // isn't much less than what was asked (and obviously that fees aren't negative).
        let mut tx = bitcoin::Transaction {
            version: 2,
            lock_time: bitcoin::PackedLockTime(0), // TODO: randomized anti fee sniping
            input: txins,
            output: txouts,
        };
        let nochange_vb = tx_vbytes(&tx) + sat_vb;
        let absolute_fee =
            in_value
                .checked_sub(out_value)
                .ok_or(CommandError::InsufficientFunds(
                    in_value, out_value, feerate_vb,
                ))?;
        let nochange_feerate_vb = absolute_fee.to_sat().checked_div(nochange_vb).unwrap();
        if nochange_feerate_vb.checked_mul(10).unwrap() < feerate_vb.checked_mul(9).unwrap() {
            return Err(CommandError::InsufficientFunds(
                in_value, out_value, feerate_vb,
            ));
        }

        // If necessary, add a change output. The computation here is a bit convoluted: we infer
        // the needed change value from the target feerate and the size of the transaction *with
        // an added output* (for the change).
        if nochange_feerate_vb > feerate_vb {
            // Get the change address to create a dummy change txo.
            let change_desc = self
                .config
                .main_descriptor
                .change_descriptor()
                .derive(db_conn.change_index(), &self.secp);
            db_conn.increment_change_index(&self.secp);
            let mut change_txo = bitcoin::TxOut {
                value: std::u64::MAX,
                script_pubkey: change_desc.script_pubkey(),
            };
            // Serialized size is equal to the virtual size for an output.
            let change_vb: u64 = serializable_size(&change_txo);
            // We assume the added output does not increase the size of the varint for
            // the output count.
            let with_change_vb = nochange_vb.checked_add(change_vb).unwrap();
            let with_change_feerate_vb = absolute_fee.to_sat().checked_div(with_change_vb).unwrap();

            if with_change_feerate_vb > feerate_vb {
                let target_fee = with_change_vb.checked_mul(feerate_vb).unwrap();
                let change_amount = absolute_fee
                    .checked_sub(bitcoin::Amount::from_sat(target_fee))
                    .unwrap();
                if change_amount.to_sat() >= DUST_OUTPUT_SATS {
                    check_output_value(change_amount)?;

                    // TODO: shuffle once we have Taproot
                    change_txo.value = change_amount.to_sat();
                    tx.output.push(change_txo);
                    psbt_outs.push(PsbtOut::default());
                }
            }
        }

        let psbt = Psbt {
            unsigned_tx: tx,
            version: 0,
            xpub: BTreeMap::new(),
            proprietary: BTreeMap::new(),
            unknown: BTreeMap::new(),
            inputs: psbt_ins,
            outputs: psbt_outs,
        };
        sanity_check_psbt(&psbt)?;
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

    pub fn list_spend(&self) -> ListSpendResult {
        let mut db_conn = self.db.connection();
        let spend_txs = db_conn
            .list_spend()
            .into_iter()
            .map(|psbt| {
                let change_index =
                    change_index(&psbt, &mut db_conn).map(|i| i.try_into().expect("insane usize"));
                ListSpendEntry { psbt, change_index }
            })
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
        match self.bitcoin.broadcast_tx(&final_tx) {
            Ok(()) => Ok(()),
            Err(BitcoinError::Broadcast(e)) => Err(CommandError::TxBroadcast(e)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoDescriptors {
    pub main: descriptors::MultipathDescriptor,
}

/// Information about the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoResult {
    pub version: String,
    pub network: bitcoin::Network,
    pub blockheight: i32,
    pub sync: f64,
    pub descriptors: GetInfoDescriptors,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAddressResult {
    pub address: bitcoin::Address,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LCSpendInfo {
    pub txid: bitcoin::Txid,
    /// The block height this spending transaction was confirmed at.
    pub height: Option<i32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ListCoinsEntry {
    #[serde(
        serialize_with = "ser_amount",
        deserialize_with = "deser_amount_from_sats"
    )]
    pub amount: bitcoin::Amount,
    pub outpoint: bitcoin::OutPoint,
    pub block_height: Option<i32>,
    /// Information about the transaction spending this coin.
    pub spend_info: Option<LCSpendInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsResult {
    pub coins: Vec<ListCoinsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateSpendResult {
    #[serde(serialize_with = "ser_base64", deserialize_with = "deser_psbt_base64")]
    pub psbt: Psbt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpendEntry {
    #[serde(serialize_with = "ser_base64", deserialize_with = "deser_psbt_base64")]
    pub psbt: Psbt,
    pub change_index: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpendResult {
    pub spend_txs: Vec<ListSpendEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::*;
    use std::str::FromStr;

    use bitcoin::util::bip32;

    #[test]
    fn getinfo() {
        let ms = DummyMinisafe::new();
        // We can query getinfo
        ms.handle.control.get_info();
        ms.shutdown();
    }

    #[test]
    fn getnewaddress() {
        let ms = DummyMinisafe::new();

        let control = &ms.handle.control;
        // We can get an address
        let addr = control.get_new_address().address;
        assert_eq!(
            addr,
            bitcoin::Address::from_str(
                "bc1q9ksrc647hx8zp2cewl8p5f487dgux3777yees8rjcx46t4daqzzqt7yga8"
            )
            .unwrap()
        );
        // We won't get the same twice.
        let addr2 = control.get_new_address().address;
        assert_ne!(addr, addr2);

        ms.shutdown();
    }

    #[test]
    fn create_spend() {
        let ms = DummyMinisafe::new();
        let control = &ms.handle.control;

        // Arguments sanity checking
        let dummy_op = bitcoin::OutPoint::from_str(
            "3753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:0",
        )
        .unwrap();
        let dummy_addr =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let dummy_value = 10_000;
        let mut destinations: HashMap<bitcoin::Address, u64> = [(dummy_addr.clone(), dummy_value)]
            .iter()
            .cloned()
            .collect();
        assert_eq!(
            control.create_spend(&[], &destinations, 1),
            Err(CommandError::NoOutpoint)
        );
        assert_eq!(
            control.create_spend(&[dummy_op], &HashMap::new(), 1),
            Err(CommandError::NoDestination)
        );
        assert_eq!(
            control.create_spend(&[dummy_op], &destinations, 0),
            Err(CommandError::InvalidFeerate(0))
        );

        // The coin doesn't exist. If we create a new unspent one at this outpoint with a much
        // higher value, we'll get a Spend transaction with a change output.
        assert_eq!(
            control.create_spend(&[dummy_op], &destinations, 1),
            Err(CommandError::UnknownOutpoint(dummy_op))
        );
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op,
            block_height: None,
            block_time: None,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
        }]);
        let res = control.create_spend(&[dummy_op], &destinations, 1).unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 2);
        assert_eq!(tx.output[0].script_pubkey, dummy_addr.script_pubkey());
        assert_eq!(tx.output[0].value, dummy_value);

        // Transaction is 1 in (P2WSH satisfaction), 2 outs. At 1sat/vb, it's 170 sats fees.
        // At 2sats/vb, it's twice that.
        assert_eq!(tx.output[1].value, 89_830);
        let res = control.create_spend(&[dummy_op], &destinations, 2).unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.output[1].value, 89_660);

        // If we ask for a too high feerate, or a too large/too small output, it'll fail.
        assert_eq!(
            control.create_spend(&[dummy_op], &destinations, 10_000),
            Err(CommandError::InsufficientFunds(
                bitcoin::Amount::from_sat(100_000),
                bitcoin::Amount::from_sat(10_000),
                10_000
            ))
        );
        *destinations.get_mut(&dummy_addr).unwrap() = 100_001;
        assert_eq!(
            control.create_spend(&[dummy_op], &destinations, 1),
            Err(CommandError::InsufficientFunds(
                bitcoin::Amount::from_sat(100_000),
                bitcoin::Amount::from_sat(100_001),
                1
            ))
        );
        *destinations.get_mut(&dummy_addr).unwrap() = 4_500;
        assert_eq!(
            control.create_spend(&[dummy_op], &destinations, 1),
            Err(CommandError::InvalidOutputValue(bitcoin::Amount::from_sat(
                4_500
            )))
        );

        // If we ask for a large, but valid, output we won't get a change output. 95_000 because we
        // won't create an output lower than 5k sats.
        *destinations.get_mut(&dummy_addr).unwrap() = 95_000;
        let res = control.create_spend(&[dummy_op], &destinations, 1).unwrap();
        let tx = res.psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].script_pubkey, dummy_addr.script_pubkey());
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
            control.create_spend(&[dummy_op], &destinations, 1),
            Err(CommandError::AlreadySpent(dummy_op))
        );

        ms.shutdown();
    }

    #[test]
    fn update_spend() {
        let ms = DummyMinisafe::new();
        let control = &ms.handle.control;
        let mut db_conn = control.db().lock().unwrap().connection();

        // Add two (unconfirmed) coins in DB
        let dummy_op_a = bitcoin::OutPoint::from_str(
            "3753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:0",
        )
        .unwrap();
        let dummy_op_b = bitcoin::OutPoint::from_str(
            "4753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:1",
        )
        .unwrap();
        db_conn.new_unspent_coins(&[
            Coin {
                outpoint: dummy_op_a,
                block_height: None,
                block_time: None,
                amount: bitcoin::Amount::from_sat(100_000),
                derivation_index: bip32::ChildNumber::from(13),
                is_change: false,
                spend_txid: None,
                spend_block: None,
            },
            Coin {
                outpoint: dummy_op_b,
                block_height: None,
                block_time: None,
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
        let destinations_a: HashMap<bitcoin::Address, u64> =
            [(dummy_addr_a.clone(), dummy_value_a)]
                .iter()
                .cloned()
                .collect();
        let destinations_b: HashMap<bitcoin::Address, u64> =
            [(dummy_addr_b.clone(), dummy_value_b)]
                .iter()
                .cloned()
                .collect();
        let destinations_c: HashMap<bitcoin::Address, u64> =
            [(dummy_addr_a, dummy_value_a), (dummy_addr_b, dummy_value_b)]
                .iter()
                .cloned()
                .collect();
        let mut psbt_a = control
            .create_spend(&[dummy_op_a], &destinations_a, 1)
            .unwrap()
            .psbt;
        let txid_a = psbt_a.unsigned_tx.txid();
        let psbt_b = control
            .create_spend(&[dummy_op_b], &destinations_b, 10)
            .unwrap()
            .psbt;
        let txid_b = psbt_b.unsigned_tx.txid();
        let psbt_c = control
            .create_spend(&[dummy_op_a, dummy_op_b], &destinations_c, 100)
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
        let sig = bitcoin::EcdsaSig::from_str("304402204004fcdbb9c0d0cbf585f58cee34dccb012efbd8fc2b0d5e97760045ae35803802201a0bd7ec2383e0b93748abc9946c8e17a8312e314dab85982aeba650e738cbf401").unwrap();
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
}
