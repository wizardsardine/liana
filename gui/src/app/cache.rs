use crate::daemon::model::{Coin, SpendTx};

#[derive(Default)]
pub struct Cache {
    pub blockheight: i32,
    pub coins: Vec<Coin>,
    pub spend_txs: Vec<SpendTx>,
}
