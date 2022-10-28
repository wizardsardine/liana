pub use minisafe::{
    commands::{
        CreateSpendResult, GetAddressResult, GetInfoResult, ListCoinsEntry, ListCoinsResult,
        ListSpendEntry, ListSpendResult,
    },
    miniscript::bitcoin::{util::psbt::Psbt, Amount},
};

pub type Coin = ListCoinsEntry;

#[derive(Debug, Clone)]
pub struct SpendTx {
    pub coins: Vec<Coin>,
    pub psbt: Psbt,
    pub change_index: Option<usize>,
    pub spend_amount: Amount,
    pub fee_amount: Amount,
    pub status: SpendStatus,
}

#[derive(Debug, Clone)]
pub enum SpendStatus {
    Pending,
    Deprecated,
    Broadcasted,
}

impl SpendTx {
    pub fn new(psbt: Psbt, change_index: Option<usize>, coins: Vec<Coin>) -> Self {
        let (change_amount, spend_amount) = psbt.unsigned_tx.output.iter().enumerate().fold(
            (Amount::from_sat(0), Amount::from_sat(0)),
            |(change, spend), (i, output)| {
                if Some(i) == change_index {
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
                    status = SpendStatus::Broadcasted
                } else {
                    status = SpendStatus::Deprecated
                }
            }
        }

        Self {
            coins,
            psbt,
            change_index,
            spend_amount,
            fee_amount: inputs_amount - spend_amount - change_amount,
            status,
        }
    }
}
