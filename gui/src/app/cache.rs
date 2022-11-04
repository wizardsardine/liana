use crate::daemon::model::{Coin, SpendTx};
use minisafe::miniscript::bitcoin::Network;

pub struct Cache {
    pub network: Network,
    pub blockheight: i32,
    pub coins: Vec<Coin>,
    pub spend_txs: Vec<SpendTx>,
}

impl std::default::Default for Cache {
    fn default() -> Self {
        Self {
            network: Network::Bitcoin,
            blockheight: 0,
            coins: Vec::new(),
            spend_txs: Vec::new(),
        }
    }
}
