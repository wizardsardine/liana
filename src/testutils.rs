use crate::{
    bitcoin::{BitcoinInterface, BlockChainTip, UTxO},
    config::{BitcoinConfig, Config},
    database::{Coin, DatabaseConnection, DatabaseInterface},
    DaemonHandle,
};

use std::{collections::HashMap, env, fs, io, path, process, str::FromStr, sync, thread, time};

use miniscript::{
    bitcoin::{
        self, secp256k1,
        util::{bip32, psbt::PartiallySignedTransaction as Psbt},
    },
    descriptor,
};

pub struct DummyBitcoind {}

impl BitcoinInterface for DummyBitcoind {
    fn genesis_block(&self) -> BlockChainTip {
        let hash = bitcoin::BlockHash::from_str(
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
        )
        .unwrap();
        BlockChainTip { hash, height: 0 }
    }

    fn sync_progress(&self) -> f64 {
        1.0
    }

    fn chain_tip(&self) -> BlockChainTip {
        let hash = bitcoin::BlockHash::from_str(
            "000000007bc154e0fa7ea32218a72fe2c1bb9f86cf8c9ebf9a715ed27fdb229a",
        )
        .unwrap();
        let height = 100;
        BlockChainTip { hash, height }
    }

    fn is_in_chain(&self, _: &BlockChainTip) -> bool {
        // No reorg
        true
    }

    fn received_coins(&self, _: &BlockChainTip) -> Vec<UTxO> {
        Vec::new()
    }

    fn confirmed_coins(&self, _: &[bitcoin::OutPoint]) -> Vec<(bitcoin::OutPoint, i32)> {
        Vec::new()
    }

    fn spent_coins(&self, _: &[bitcoin::OutPoint]) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        Vec::new()
    }
}

pub struct DummyDb {
    curr_index: bip32::ChildNumber,
    curr_tip: Option<BlockChainTip>,
    coins: HashMap<bitcoin::OutPoint, Coin>,
    spend_txs: HashMap<bitcoin::Txid, Psbt>,
}

impl DummyDb {
    pub fn new() -> DummyDb {
        DummyDb {
            curr_index: 0.into(),
            curr_tip: None,
            coins: HashMap::new(),
            spend_txs: HashMap::new(),
        }
    }
}

impl DatabaseInterface for sync::Arc<sync::RwLock<DummyDb>> {
    fn connection(&self) -> Box<dyn DatabaseConnection> {
        Box::new(DummyDbConn { db: self.clone() })
    }
}

pub struct DummyDbConn {
    db: sync::Arc<sync::RwLock<DummyDb>>,
}

impl DatabaseConnection for DummyDbConn {
    fn network(&mut self) -> bitcoin::Network {
        bitcoin::Network::Bitcoin
    }

    fn chain_tip(&mut self) -> Option<BlockChainTip> {
        self.db.read().unwrap().curr_tip
    }

    fn update_tip(&mut self, tip: &BlockChainTip) {
        self.db.write().unwrap().curr_tip = Some(*tip);
    }

    fn derivation_index(&mut self) -> bip32::ChildNumber {
        self.db.read().unwrap().curr_index
    }

    fn increment_derivation_index(&mut self, _: &secp256k1::Secp256k1<secp256k1::VerifyOnly>) {
        let next_index = self.db.write().unwrap().curr_index.increment().unwrap();
        self.db.write().unwrap().curr_index = next_index;
    }

    fn unspent_coins(&mut self) -> HashMap<bitcoin::OutPoint, Coin> {
        self.db.read().unwrap().coins.clone()
    }

    fn new_unspent_coins<'a>(&mut self, coins: &[Coin]) {
        for coin in coins {
            self.db
                .write()
                .unwrap()
                .coins
                .insert(coin.outpoint, coin.clone());
        }
    }

    fn confirm_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, i32)]) {
        for (op, height) in outpoints {
            let mut db = self.db.write().unwrap();
            let h = &mut db.coins.get_mut(op).unwrap().block_height;
            assert!(h.is_none());
            *h = Some(*height);
        }
    }

    fn spend_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)]) {
        for (op, spend_txid) in outpoints {
            let mut db = self.db.write().unwrap();
            let spender = &mut db.coins.get_mut(op).unwrap().spend_txid;
            assert!(spender.is_none());
            *spender = Some(*spend_txid);
        }
    }

    fn derivation_index_by_address(&mut self, _: &bitcoin::Address) -> Option<bip32::ChildNumber> {
        None
    }

    fn coins_by_outpoints(
        &mut self,
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin> {
        // Very inefficient but hey
        self.db
            .read()
            .unwrap()
            .coins
            .clone()
            .into_iter()
            .filter(|(op, _)| outpoints.contains(&op))
            .collect()
    }

    fn store_spend(&mut self, psbt: &Psbt) {
        let txid = psbt.global.unsigned_tx.txid();
        self.db
            .write()
            .unwrap()
            .spend_txs
            .insert(txid, psbt.clone());
    }

    fn spend_tx(&mut self, txid: &bitcoin::Txid) -> Option<Psbt> {
        self.db.read().unwrap().spend_txs.get(txid).cloned()
    }

    fn list_spend(&mut self) -> Vec<Psbt> {
        self.db
            .read()
            .unwrap()
            .spend_txs
            .values()
            .cloned()
            .collect()
    }

    fn delete_spend(&mut self, txid: &bitcoin::Txid) {
        self.db.write().unwrap().spend_txs.remove(txid);
    }
}

pub struct DummyMinisafe {
    pub tmp_dir: path::PathBuf,
    pub handle: DaemonHandle,
}

static mut COUNTER: sync::atomic::AtomicUsize = sync::atomic::AtomicUsize::new(0);
fn uid() -> usize {
    unsafe {
        let uid = COUNTER.load(sync::atomic::Ordering::Relaxed);
        COUNTER.fetch_add(1, sync::atomic::Ordering::Relaxed);
        uid
    }
}

pub fn tmp_dir() -> path::PathBuf {
    env::temp_dir().join(format!(
        "minisafed-{}-{:?}-{}",
        process::id(),
        thread::current().id(),
        uid(),
    ))
}

impl DummyMinisafe {
    pub fn new() -> DummyMinisafe {
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        // Use a shorthand for 'datadir', to avoid overflowing SUN_LEN on MacOS.
        let data_dir: path::PathBuf = [tmp_dir.as_path(), path::Path::new("d")].iter().collect();

        let network = bitcoin::Network::Bitcoin;
        let bitcoin_config = BitcoinConfig {
            network,
            poll_interval_secs: time::Duration::from_secs(2),
        };

        let owner_key = descriptor::DescriptorPublicKey::from_str("xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/*").unwrap();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/*").unwrap();
        let desc =
            crate::descriptors::InheritanceDescriptor::new(owner_key, heir_key, 10_000).unwrap();
        let config = Config {
            bitcoin_config,
            bitcoind_config: None,
            data_dir: Some(data_dir.clone()),
            #[cfg(unix)]
            daemon: false,
            log_level: log::LevelFilter::Debug,
            main_descriptor: desc,
        };

        let db = sync::Arc::from(sync::RwLock::from(DummyDb::new()));
        let handle = DaemonHandle::start(config, Some(DummyBitcoind {}), Some(db)).unwrap();
        DummyMinisafe { tmp_dir, handle }
    }

    #[cfg(feature = "jsonrpc_server")]
    pub fn rpc_server(self) -> Result<(), io::Error> {
        self.handle.rpc_server()?;
        fs::remove_dir_all(&self.tmp_dir)?;
        Ok(())
    }

    pub fn shutdown(self) {
        self.handle.shutdown();
        fs::remove_dir_all(&self.tmp_dir).unwrap();
    }
}
