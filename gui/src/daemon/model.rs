pub use liana::{
    commands::{
        CreateSpendResult, GetAddressResult, GetInfoResult, ListCoinsEntry, ListCoinsResult,
        ListSpendEntry, ListSpendResult, ListTransactionsResult, TransactionInfo,
    },
    descriptors::PartialSpendInfo,
    miniscript::bitcoin::{util::psbt::Psbt, Amount, Transaction},
};

pub type Coin = ListCoinsEntry;

pub fn remaining_sequence(coin: &Coin, blockheight: u32, timelock: u32) -> u32 {
    if let Some(coin_blockheight) = coin.block_height {
        if blockheight > coin_blockheight as u32 + timelock {
            0
        } else {
            coin_blockheight as u32 + timelock - blockheight
        }
    } else {
        timelock
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendStatus {
    Pending,
    Deprecated,
    Broadcast,
}

impl SpendTx {
    pub fn new(psbt: Psbt, coins: Vec<Coin>, sigs: PartialSpendInfo) -> Self {
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
                    status = SpendStatus::Broadcast
                } else {
                    status = SpendStatus::Deprecated
                }
            }
        }

        Self {
            coins,
            psbt,
            change_indexes,
            spend_amount,
            fee_amount: inputs_amount - spend_amount - change_amount,
            status,
            sigs,
        }
    }

    pub fn is_signed(&self) -> bool {
        !self.psbt.inputs.first().unwrap().partial_sigs.is_empty()
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
}
