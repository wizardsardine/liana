use std::collections::HashSet;

pub use liana::{
    commands::{
        CreateSpendResult, GetAddressResult, GetInfoResult, ListCoinsEntry, ListCoinsResult,
        ListSpendEntry, ListSpendResult, ListTransactionsResult, TransactionInfo,
    },
    descriptors::{PartialSpendInfo, PathSpendInfo},
    miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt, Amount, Transaction},
};

pub type Coin = ListCoinsEntry;

pub fn remaining_sequence(coin: &Coin, blockheight: u32, timelock: u16) -> u32 {
    if let Some(coin_blockheight) = coin.block_height {
        if blockheight > coin_blockheight as u32 + timelock as u32 {
            0
        } else {
            coin_blockheight as u32 + timelock as u32 - blockheight
        }
    } else {
        timelock as u32
    }
}

#[derive(Debug, Clone)]
pub struct SpendTx {
    pub coins: Vec<Coin>,
    pub psbt: Psbt,
    pub change_indexes: Vec<usize>,
    pub spend_amount: Amount,
    pub fee_amount: Amount,
    pub status: SpendStatus,
    pub sigs: PartialSpendInfo,
    pub updated_at: Option<u32>,
}

#[derive(PartialOrd, Ord, Debug, Clone, PartialEq, Eq)]
pub enum SpendStatus {
    Pending,
    Broadcast,
    Spent,
    Deprecated,
}

impl SpendTx {
    pub fn new(
        updated_at: Option<u32>,
        psbt: Psbt,
        coins: Vec<Coin>,
        sigs: PartialSpendInfo,
    ) -> Self {
        let mut change_indexes = Vec::new();
        let (change_amount, spend_amount) = psbt.unsigned_tx.output.iter().enumerate().fold(
            (Amount::from_sat(0), Amount::from_sat(0)),
            |(change, spend), (i, output)| {
                if !psbt.outputs[i].bip32_derivation.is_empty() {
                    change_indexes.push(i);
                    (change + Amount::from_sat(output.value), spend)
                } else {
                    (change, spend + Amount::from_sat(output.value))
                }
            },
        );

        let mut inputs_amount = Amount::from_sat(0);
        let mut status = SpendStatus::Pending;
        for coin in &coins {
            inputs_amount += coin.amount;
            if let Some(info) = coin.spend_info {
                if info.txid == psbt.unsigned_tx.txid() {
                    if info.height.is_some() {
                        status = SpendStatus::Spent
                    } else {
                        status = SpendStatus::Broadcast
                    }
                } else {
                    status = SpendStatus::Deprecated
                }
            }
        }

        Self {
            updated_at,
            coins,
            psbt,
            change_indexes,
            spend_amount,
            fee_amount: inputs_amount - spend_amount - change_amount,
            status,
            sigs,
        }
    }

    /// Returns the path ready if it exists.
    pub fn path_ready(&self) -> Option<&PathSpendInfo> {
        let path = self.sigs.primary_path();
        if path.sigs_count >= path.threshold {
            return Some(path);
        }
        self.sigs
            .recovery_paths()
            .values()
            .find(|&path| path.sigs_count >= path.threshold)
    }

    pub fn signers(&self) -> HashSet<Fingerprint> {
        let mut signers = HashSet::new();
        for fg in self.sigs.primary_path().signed_pubkeys.keys() {
            signers.insert(*fg);
        }

        for path in self.sigs.recovery_paths().values() {
            for fg in path.signed_pubkeys.keys() {
                signers.insert(*fg);
            }
        }

        signers
    }

    pub fn is_self_send(&self) -> bool {
        !self.coins.is_empty() && self.spend_amount == Amount::from_sat(0)
    }
}

#[derive(Debug, Clone)]
pub struct HistoryTransaction {
    pub coins: Vec<Coin>,
    pub change_indexes: Vec<usize>,
    pub tx: Transaction,
    pub outgoing_amount: Amount,
    pub incoming_amount: Amount,
    pub fee_amount: Option<Amount>,
    pub height: Option<i32>,
    pub time: Option<u32>,
}

impl HistoryTransaction {
    pub fn new(
        tx: Transaction,
        height: Option<i32>,
        time: Option<u32>,
        coins: Vec<Coin>,
        change_indexes: Vec<usize>,
    ) -> Self {
        let (incoming_amount, outgoing_amount) = tx.output.iter().enumerate().fold(
            (Amount::from_sat(0), Amount::from_sat(0)),
            |(change, spend), (i, output)| {
                if change_indexes.contains(&i) {
                    (change + Amount::from_sat(output.value), spend)
                } else {
                    (change, spend + Amount::from_sat(output.value))
                }
            },
        );

        let mut inputs_amount = Amount::from_sat(0);
        for coin in &coins {
            inputs_amount += coin.amount;
        }

        let fee_amount = if inputs_amount > outgoing_amount + incoming_amount {
            Some(inputs_amount - outgoing_amount - incoming_amount)
        } else {
            None
        };

        Self {
            tx,
            coins,
            change_indexes,
            outgoing_amount,
            incoming_amount,
            fee_amount,
            height,
            time,
        }
    }

    pub fn is_external(&self) -> bool {
        self.coins.is_empty()
    }

    pub fn is_self_send(&self) -> bool {
        !self.coins.is_empty() && self.outgoing_amount == Amount::from_sat(0)
    }
}
