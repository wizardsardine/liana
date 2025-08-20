use crate::{
    bitcoin::{BitcoinInterface, Block, BlockChainTip, MempoolEntry, SyncProgress, UTxO},
    config::{BitcoinConfig, Config},
    database::{
        BlockInfo, Coin, CoinStatus, DatabaseConnection, DatabaseInterface, LabelItem, Wallet,
    },
    datadir::DataDirectory,
    payjoin::db::SessionId,
    DaemonControl, DaemonHandle,
};
use liana::descriptors;
use payjoin::OhttpKeys;

use std::convert::TryInto;
use std::{
    collections::{HashMap, HashSet},
    env, fs, path, process,
    str::FromStr,
    sync, thread, time,
    time::{SystemTime, UNIX_EPOCH},
};

use miniscript::{
    bitcoin::{self, bip32, psbt::Psbt, secp256k1, Transaction, Txid},
    descriptor,
};

pub struct DummyBitcoind {
    pub txs: HashMap<Txid, (Transaction, Option<Block>)>,
}

impl DummyBitcoind {}

impl DummyBitcoind {
    pub fn new() -> Self {
        Self {
            txs: HashMap::new(),
        }
    }
}

impl BitcoinInterface for DummyBitcoind {
    fn genesis_block_timestamp(&self) -> u32 {
        1231006505
    }

    fn genesis_block(&self) -> BlockChainTip {
        let hash = bitcoin::BlockHash::from_str(
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
        )
        .unwrap();
        BlockChainTip { hash, height: 0 }
    }

    fn sync_progress(&self) -> SyncProgress {
        SyncProgress::new(1.0, 1_000, 1_000)
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

    fn sync_wallet(
        &mut self,
        _receive_index: bip32::ChildNumber,
        _change_index: bip32::ChildNumber,
    ) -> Result<Option<BlockChainTip>, String> {
        Ok(None)
    }

    fn received_coins(
        &self,
        _: &BlockChainTip,
        _: &[descriptors::SinglePathLianaDesc],
    ) -> Vec<UTxO> {
        Vec::new()
    }

    fn confirmed_coins(
        &self,
        _: &[bitcoin::OutPoint],
    ) -> (Vec<(bitcoin::OutPoint, i32, u32)>, Vec<bitcoin::OutPoint>) {
        (Vec::new(), Vec::new())
    }

    fn spending_coins(&self, _: &[bitcoin::OutPoint]) -> Vec<(bitcoin::OutPoint, bitcoin::Txid)> {
        Vec::new()
    }

    fn spent_coins(
        &self,
        _: &[(bitcoin::OutPoint, bitcoin::Txid)],
    ) -> (
        Vec<(bitcoin::OutPoint, bitcoin::Txid, i32, u32)>,
        Vec<bitcoin::OutPoint>,
    ) {
        (Vec::new(), Vec::new())
    }

    fn common_ancestor(&self, _: &BlockChainTip) -> Option<BlockChainTip> {
        todo!()
    }

    fn broadcast_tx(&self, _: &bitcoin::Transaction) -> Result<(), String> {
        todo!()
    }

    fn start_rescan(&mut self, _: &descriptors::LianaDescriptor, _: u32) -> Result<(), String> {
        todo!()
    }

    fn rescan_progress(&self) -> Option<f64> {
        None
    }

    fn block_before_date(&self, _: u32) -> Option<BlockChainTip> {
        todo!()
    }

    fn tip_time(&self) -> Option<u32> {
        None
    }

    fn wallet_transaction(
        &self,
        txid: &bitcoin::Txid,
    ) -> Option<(bitcoin::Transaction, Option<Block>)> {
        self.txs.get(txid).cloned()
    }

    fn mempool_spenders(&self, _: &[bitcoin::OutPoint]) -> Vec<MempoolEntry> {
        Vec::new()
    }

    fn mempool_entry(&self, _: &bitcoin::Txid) -> Option<MempoolEntry> {
        None
    }

    fn test_mempool_accept(&self, _rawtxs: Vec<String>) -> Vec<bool> {
        todo!()
    }
}

struct PayjoinSession {
    completed: bool,
}

struct PayjoinSessionEvent {
    events: Vec<Vec<u8>>,
}

struct DummyDbState {
    deposit_index: bip32::ChildNumber,
    change_index: bip32::ChildNumber,
    curr_tip: Option<BlockChainTip>,
    coins: HashMap<bitcoin::OutPoint, Coin>,
    txs: HashMap<bitcoin::Txid, bitcoin::Transaction>,
    spend_txs: HashMap<bitcoin::Txid, (Psbt, Option<u32>)>,
    labels: HashMap<LabelItem, String>,
    timestamp: u32,
    rescan_timestamp: Option<u32>,
    last_poll_timestamp: Option<u32>,
    payjoin_sender_sessions: HashMap<i64, PayjoinSession>,
    payjoin_receiver_sessions: HashMap<i64, PayjoinSession>,
    payjoin_session_events: HashMap<i64, PayjoinSessionEvent>,
}

pub struct DummyDatabase {
    db: sync::Arc<sync::RwLock<DummyDbState>>,
}

impl DatabaseInterface for DummyDatabase {
    fn connection(&self) -> Box<dyn DatabaseConnection> {
        Box::new(DummyDatabase {
            db: self.db.clone(),
        })
    }
}

impl DummyDatabase {
    pub fn new() -> DummyDatabase {
        let now: u32 = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .try_into()
            .unwrap();

        DummyDatabase {
            db: sync::Arc::new(sync::RwLock::new(DummyDbState {
                deposit_index: 0.into(),
                change_index: 0.into(),
                curr_tip: None,
                coins: HashMap::new(),
                txs: HashMap::new(),
                spend_txs: HashMap::new(),
                labels: HashMap::new(),
                timestamp: now,
                rescan_timestamp: None,
                last_poll_timestamp: None,
                payjoin_sender_sessions: HashMap::new(),
                payjoin_receiver_sessions: HashMap::new(),
                payjoin_session_events: HashMap::new(),
            })),
        }
    }

    pub fn insert_coins(&mut self, coins: Vec<Coin>) {
        for coin in coins {
            self.db.write().unwrap().coins.insert(coin.outpoint, coin);
        }
    }
}

impl DatabaseConnection for DummyDatabase {
    fn network(&mut self) -> bitcoin::Network {
        bitcoin::Network::Bitcoin
    }

    fn chain_tip(&mut self) -> Option<BlockChainTip> {
        self.db.read().unwrap().curr_tip
    }

    fn wallet(&mut self) -> Wallet {
        let db_wallet = self.db.read().unwrap();
        Wallet {
            timestamp: db_wallet.timestamp,
            receive_index: db_wallet.deposit_index,
            change_index: db_wallet.change_index,
            rescan_timestamp: db_wallet.rescan_timestamp,
            last_poll_timestamp: db_wallet.last_poll_timestamp,
        }
    }

    fn timestamp(&mut self) -> u32 {
        self.db.read().unwrap().timestamp
    }

    fn update_tip(&mut self, tip: &BlockChainTip) {
        self.db.write().unwrap().curr_tip = Some(*tip);
    }

    fn receive_index(&mut self) -> bip32::ChildNumber {
        self.db.read().unwrap().deposit_index
    }

    fn set_receive_index(
        &mut self,
        index: bip32::ChildNumber,
        _: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) {
        self.db.write().unwrap().deposit_index = index;
    }

    fn change_index(&mut self) -> bip32::ChildNumber {
        self.db.read().unwrap().change_index
    }

    fn set_change_index(
        &mut self,
        index: bip32::ChildNumber,
        _: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) {
        self.db.write().unwrap().change_index = index;
    }

    fn coins(
        &mut self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin> {
        self.db
            .read()
            .unwrap()
            .coins
            .clone()
            .into_iter()
            .filter_map(|(op, c)| {
                if (c.block_info.is_none()
                    && c.spend_txid.is_none()
                    && statuses.contains(&CoinStatus::Unconfirmed))
                    || (c.block_info.is_some()
                        && c.spend_txid.is_none()
                        && statuses.contains(&CoinStatus::Confirmed))
                    || (c.spend_txid.is_some()
                        && c.spend_block.is_none()
                        && statuses.contains(&CoinStatus::Spending))
                    || (c.spend_block.is_some() && statuses.contains(&CoinStatus::Spent))
                    || statuses.is_empty()
                {
                    Some((op, c))
                } else {
                    None
                }
            })
            .filter_map(|(op, c)| {
                if outpoints.contains(&op) || outpoints.is_empty() {
                    Some((op, c))
                } else {
                    None
                }
            })
            .collect()
    }

    fn list_spending_coins(&mut self) -> HashMap<bitcoin::OutPoint, Coin> {
        let mut result = HashMap::new();
        for (k, v) in self.db.read().unwrap().coins.iter() {
            if v.spend_txid.is_some() {
                result.insert(*k, *v);
            }
        }
        result
    }

    fn new_unspent_coins<'a>(&mut self, coins: &[Coin]) {
        for coin in coins {
            self.db.write().unwrap().coins.insert(coin.outpoint, *coin);
        }
    }

    fn remove_coins(&mut self, outpoints: &[bitcoin::OutPoint]) {
        for op in outpoints {
            self.db.write().unwrap().coins.remove(op);
        }
    }

    fn confirm_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, i32, u32)]) {
        for (op, height, time) in outpoints {
            let mut db = self.db.write().unwrap();
            let coin = &mut db.coins.get_mut(op).unwrap();
            assert!(coin.block_info.is_none());
            coin.block_info = Some(BlockInfo {
                height: *height,
                time: *time,
            });
        }
    }

    fn spend_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)]) {
        for (op, spend_txid) in outpoints {
            let mut db = self.db.write().unwrap();
            let spent = &mut db.coins.get_mut(op).unwrap();
            assert!(spent.spend_txid.is_none());
            assert!(spent.spend_block.is_none());
            spent.spend_txid = Some(*spend_txid);
        }
    }

    fn unspend_coins<'a>(&mut self, outpoints: &[bitcoin::OutPoint]) {
        for op in outpoints {
            let mut db = self.db.write().unwrap();
            let spent = &mut db.coins.get_mut(op).unwrap();
            assert!(spent.spend_txid.is_some());
            spent.spend_txid = None;
            spent.spend_block = None;
        }
    }

    fn confirm_spend<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid, i32, u32)]) {
        for (op, spend_txid, height, time) in outpoints {
            let mut db = self.db.write().unwrap();
            let spent = &mut db.coins.get_mut(op).unwrap();
            assert!(spent.spend_txid.is_some());
            assert!(spent.spend_block.is_none());
            spent.spend_txid = Some(*spend_txid);
            spent.spend_block = Some(BlockInfo {
                height: *height,
                time: *time,
            });
        }
    }

    fn derivation_index_by_address(
        &mut self,
        _: &bitcoin::Address,
    ) -> Option<(bip32::ChildNumber, bool)> {
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
            .filter(|(op, _)| outpoints.contains(op))
            .collect()
    }

    fn store_spend(&mut self, psbt: &Psbt) {
        let txid = psbt.unsigned_tx.compute_txid();
        self.db
            .write()
            .unwrap()
            .spend_txs
            .insert(txid, (psbt.clone(), None));
    }

    fn spend_tx(&mut self, txid: &bitcoin::Txid) -> Option<Psbt> {
        self.db
            .read()
            .unwrap()
            .spend_txs
            .get(txid)
            .cloned()
            .map(|x| x.0)
    }

    fn list_spend(&mut self) -> Vec<(Psbt, Option<u32>)> {
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

    fn rollback_tip(&mut self, _: &BlockChainTip) {
        todo!()
    }

    fn rescan_timestamp(&mut self) -> Option<u32> {
        self.db.read().unwrap().rescan_timestamp
    }

    fn set_rescan(&mut self, _: u32) {
        todo!()
    }

    fn complete_rescan(&mut self) {
        todo!()
    }

    fn last_poll_timestamp(&mut self) -> Option<u32> {
        self.db.read().unwrap().last_poll_timestamp
    }

    fn set_last_poll(&mut self, timestamp: u32) {
        self.db.write().unwrap().last_poll_timestamp = Some(timestamp);
    }

    fn update_labels(&mut self, items: &HashMap<LabelItem, Option<String>>) {
        for (lab_item, lab_val) in items {
            if let Some(val) = lab_val {
                self.db
                    .write()
                    .unwrap()
                    .labels
                    .insert(lab_item.clone(), val.clone());
            } else {
                self.db.write().unwrap().labels.remove_entry(lab_item);
            }
        }
    }

    fn labels(&mut self, items: &HashSet<LabelItem>) -> HashMap<String, String> {
        self.db
            .read()
            .unwrap()
            .labels
            .iter()
            .filter_map(|(lab_item, lab_val)| {
                items
                    .contains(lab_item)
                    .then_some((lab_item.to_string(), lab_val.clone()))
            })
            .collect()
    }

    fn list_txids(&mut self, start: u32, end: u32, limit: u64) -> Vec<bitcoin::Txid> {
        let mut txids_and_time = Vec::new();
        let coins = &self.db.read().unwrap().coins;
        // Get txid and block time of every transactions that happened between start and end
        // timestamps.
        for coin in coins.values() {
            if let Some(time) = coin.block_info.map(|b| b.time) {
                if time >= start && time <= end {
                    let row = (coin.outpoint.txid, time);
                    if !txids_and_time.contains(&row) {
                        txids_and_time.push(row);
                    }
                }
            }
            if let Some(time) = coin.spend_block.map(|b| b.time) {
                if time >= start && time <= end {
                    let row = (coin.spend_txid.expect("spent_at is not none"), time);
                    if !txids_and_time.contains(&row) {
                        txids_and_time.push(row);
                    }
                }
            }
        }
        // Apply order and limit
        txids_and_time.sort_by(|(_, t1), (_, t2)| t2.cmp(t1));
        txids_and_time.truncate(limit as usize);
        txids_and_time.into_iter().map(|(txid, _)| txid).collect()
    }

    fn list_saved_txids(&mut self) -> Vec<bitcoin::Txid> {
        self.db.read().unwrap().txs.keys().cloned().collect()
    }

    fn new_txs(&mut self, txs: &[bitcoin::Transaction]) {
        for tx in txs {
            self.db
                .write()
                .unwrap()
                .txs
                .insert(tx.compute_txid(), tx.clone());
        }
    }

    fn update_coins_from_self(&mut self, _prev_tip_height: i32) {
        // noop
    }

    fn list_wallet_transactions(
        &mut self,
        txids: &[bitcoin::Txid],
    ) -> Vec<(bitcoin::Transaction, Option<i32>, Option<u32>)> {
        let txs: HashMap<_, _> = self
            .db
            .read()
            .unwrap()
            .txs
            .clone()
            .into_iter()
            .filter(|(txid, _tx)| txids.contains(txid))
            .collect();
        let coins = self.coins(&[], &[]);
        let mut wallet_txs = Vec::with_capacity(txs.len());
        for (txid, tx) in txs {
            let first_block_info = coins.values().find_map(|c| {
                if c.outpoint.txid == txid {
                    Some(c.block_info)
                } else if c.spend_txid == Some(txid) {
                    Some(c.spend_block)
                } else {
                    None
                }
            });
            if let Some(block_info) = first_block_info {
                wallet_txs.push((tx, block_info.map(|b| b.height), block_info.map(|b| b.time)));
            }
        }
        wallet_txs
    }

    fn insert_input_seen_before(&mut self, _outpoints: &[bitcoin::OutPoint]) -> bool {
        todo!()
    }

    fn get_labels_bip329(&mut self, _offset: u32, _limit: u32) -> bip329::Labels {
        todo!()
    }
    fn payjoin_get_ohttp_keys(&mut self, _ohttp_relay: &str) -> Option<(u32, OhttpKeys)> {
        todo!()
    }

    fn payjoin_save_ohttp_keys(&mut self, _ohttp_relay: &str, _ohttp_keys: payjoin::OhttpKeys) {
        todo!()
    }

    fn get_all_active_receiver_session_ids(&mut self) -> Vec<SessionId> {
        self.db
            .read()
            .expect("lock should not be poisoned")
            .payjoin_receiver_sessions
            .keys()
            .map(|id| SessionId(*id))
            .collect()
    }
    fn save_new_payjoin_sender_session(&mut self, _txid: &bitcoin::Txid) -> i64 {
        let id = self
            .db
            .read()
            .expect("lock should not be poisoned")
            .payjoin_sender_sessions
            .len() as i64
            + 1;
        self.db
            .write()
            .expect("lock should not be poisoned")
            .payjoin_sender_sessions
            .insert(id, PayjoinSession { completed: false });
        id
    }

    fn get_all_active_sender_session_ids(&mut self) -> Vec<SessionId> {
        self.db
            .read()
            .expect("lock should not be poisoned")
            .payjoin_sender_sessions
            .keys()
            .map(|id| SessionId(*id))
            .collect()
    }

    fn save_new_payjoin_receiver_session(&mut self) -> i64 {
        let id = self
            .db
            .read()
            .expect("lock should not be poisoned")
            .payjoin_receiver_sessions
            .len() as i64
            + 1;
        self.db
            .write()
            .expect("lock should not be poisoned")
            .payjoin_receiver_sessions
            .insert(id, PayjoinSession { completed: false });
        id
    }

    fn save_receiver_session_event(&mut self, session_id: &SessionId, event: Vec<u8>) {
        self.db
            .write()
            .expect("lock should not be poisoned")
            .payjoin_session_events
            .entry(session_id.0)
            .or_insert(PayjoinSessionEvent { events: Vec::new() })
            .events
            .push(event);
    }

    fn update_receiver_session_completed_at(&mut self, session_id: &SessionId) {
        self.db
            .write()
            .expect("lock should not be poisoned")
            .payjoin_receiver_sessions
            .entry(session_id.0)
            .or_insert(PayjoinSession { completed: false })
            .completed = true;
    }

    fn load_receiver_session_events(&mut self, session_id: &SessionId) -> Vec<Vec<u8>> {
        self.db
            .read()
            .expect("lock should not be poisoned")
            .payjoin_session_events
            .get(&session_id.0)
            .map(|e| e.events.clone())
            .unwrap_or_default()
    }

    fn save_sender_session_event(&mut self, session_id: &SessionId, event: Vec<u8>) {
        self.db
            .write()
            .expect("lock should not be poisoned")
            .payjoin_session_events
            .entry(session_id.0)
            .or_insert(PayjoinSessionEvent { events: Vec::new() })
            .events
            .push(event);
    }

    fn get_all_sender_session_events(&mut self, session_id: &SessionId) -> Vec<Vec<u8>> {
        self.db
            .read()
            .expect("lock should not be poisoned")
            .payjoin_session_events
            .get(&session_id.0)
            .map(|e| e.events.clone())
            .unwrap_or_default()
    }

    fn update_sender_session_completed_at(&mut self, session_id: &SessionId) {
        self.db
            .write()
            .expect("lock should not be poisoned")
            .payjoin_receiver_sessions
            .entry(session_id.0)
            .or_insert(PayjoinSession { completed: false })
            .completed = true;
    }

    fn save_receiver_session_original_txid(
        &mut self,
        _session_id: &SessionId,
        _original_txid: &bitcoin::Txid,
    ) {
        todo!()
    }

    fn save_receiver_session_proposed_txid(
        &mut self,
        _session_id: &SessionId,
        _proposed_txid: &bitcoin::Txid,
    ) {
        todo!()
    }

    fn get_payjoin_receiver_session_id_from_txid(
        &mut self,
        _txid: &bitcoin::Txid,
    ) -> Option<SessionId> {
        todo!()
    }

    fn save_proposed_payjoin_txid(
        &mut self,
        _session_id: &SessionId,
        _proposed_txid: &bitcoin::Txid,
    ) {
        todo!()
    }

    fn get_payjoin_sender_session_id_from_txid(
        &mut self,
        _txid: &bitcoin::Txid,
    ) -> Option<SessionId> {
        todo!()
    }
}

pub struct DummyLiana {
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
        "lianad-{}-{:?}-{}",
        process::id(),
        thread::current().id(),
        uid(),
    ))
}

impl DummyLiana {
    /// Creates a new DummyLiana interface
    pub fn _new(
        bitcoin_interface: impl BitcoinInterface + 'static,
        database: impl DatabaseInterface + 'static,
        rpc_server: bool,
        timelock: u16,
    ) -> DummyLiana {
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        // Use a shorthand for 'datadir', to avoid overflowing SUN_LEN on MacOS.
        let root_directory: path::PathBuf =
            [tmp_dir.as_path(), path::Path::new("d")].iter().collect();
        fs::create_dir_all(&root_directory).unwrap();
        let mut data_directory = root_directory.clone();
        data_directory.push("bitcoin");

        let network = bitcoin::Network::Bitcoin;
        let bitcoin_config = BitcoinConfig {
            network,
            poll_interval_secs: time::Duration::from_secs(2),
        };

        let owner_key = descriptors::PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*").unwrap());
        let heir_key = descriptors::PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/<0;1>/*").unwrap());
        let policy = descriptors::LianaPolicy::new_legacy(
            owner_key,
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap();
        let desc = descriptors::LianaDescriptor::new(policy);
        let config = Config::new(
            bitcoin_config,
            None,
            log::LevelFilter::Debug,
            desc,
            DataDirectory::new(data_directory),
        );

        let handle =
            DaemonHandle::start(config, Some(bitcoin_interface), Some(database), rpc_server)
                .unwrap();
        DummyLiana { tmp_dir, handle }
    }

    /// Creates a new DummyLiana interface
    pub fn new(
        bitcoin_interface: impl BitcoinInterface + 'static,
        database: impl DatabaseInterface + 'static,
    ) -> DummyLiana {
        Self::_new(bitcoin_interface, database, false, 10_000)
    }

    /// Creates a new DummyLiana interface with the specified recovery path timelock.
    pub fn new_timelock(
        bitcoin_interface: impl BitcoinInterface + 'static,
        database: impl DatabaseInterface + 'static,
        timelock: u16,
    ) -> DummyLiana {
        Self::_new(bitcoin_interface, database, false, timelock)
    }

    /// Creates a new DummyLiana interface which also spins up an RPC server.
    pub fn new_server(
        bitcoin_interface: impl BitcoinInterface + 'static,
        database: impl DatabaseInterface + 'static,
    ) -> DummyLiana {
        Self::_new(bitcoin_interface, database, true, 10_000)
    }

    pub fn control(&self) -> &DaemonControl {
        match self.handle {
            DaemonHandle::Controller { ref control, .. } => control,
            DaemonHandle::Server { .. } => unreachable!(),
        }
    }

    pub fn shutdown(self) {
        self.handle.stop().unwrap();
        fs::remove_dir_all(self.tmp_dir).unwrap();
    }
}
