use crate::{
    bitcoin::{BitcoinInterface, BlockChainTip},
    config::{BitcoinConfig, Config},
    database::{DatabaseConnection, DatabaseInterface},
    DaemonControl, DaemonHandle,
};

use std::{env, fs, path, process, str::FromStr, sync, thread, time};

use miniscript::{
    bitcoin::{self, util::bip32},
    descriptor,
};

pub struct DummyBitcoind {}

impl BitcoinInterface for DummyBitcoind {
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
}

pub struct DummyDb {
    curr_index: bip32::ChildNumber,
    curr_tip: Option<BlockChainTip>,
}

impl DummyDb {
    pub fn new() -> DummyDb {
        DummyDb {
            curr_index: 0.into(),
            curr_tip: None,
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
    fn chain_tip(&mut self) -> Option<BlockChainTip> {
        self.db.read().unwrap().curr_tip
    }

    fn update_tip(&mut self, tip: &BlockChainTip) {
        self.db.write().unwrap().curr_tip = Some(*tip);
    }

    fn derivation_index(&mut self) -> bip32::ChildNumber {
        self.db.read().unwrap().curr_index
    }

    fn update_derivation_index(&mut self, index: bip32::ChildNumber) {
        self.db.write().unwrap().curr_index = index;
    }
}

pub struct DummyMinisafe {
    tmp_dir: path::PathBuf,
    pub handle: DaemonHandle,
}

impl DummyMinisafe {
    pub fn new() -> DummyMinisafe {
        let tmp_dir = env::temp_dir().join(format!(
            "minisafed-unit-tests-{}-{:?}",
            process::id(),
            thread::current().id()
        ));
        fs::create_dir_all(&tmp_dir).unwrap();
        let data_dir: path::PathBuf = [tmp_dir.as_path(), path::Path::new("datadir")]
            .iter()
            .collect();

        let network = bitcoin::Network::Bitcoin;
        let bitcoin_config = BitcoinConfig {
            network,
            poll_interval_secs: time::Duration::from_secs(2),
        };

        let owner_key = descriptor::DescriptorPublicKey::from_str("xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/*").unwrap();
        let heir_key = descriptor::DescriptorPublicKey::from_str("xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/*").unwrap();
        let desc = crate::descriptors::inheritance_descriptor(owner_key, heir_key, 10_000).unwrap();
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

    pub fn shutdown(self) {
        self.handle.shutdown();
        fs::remove_dir_all(&self.tmp_dir).unwrap();
    }
}
