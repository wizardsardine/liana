use crate::{
    bitcoin::{
        AncestorSearch, BackendId, BitcoinInterface, Block, BlockChainTip, MempoolEntry,
        SyncProgress, UTxO,
    },
    config::{BitcoinConfig, Config},
    database::{
        BlockInfo, Coin, CoinStatus, DatabaseConnection, DatabaseInterface, LabelItem, Wallet,
    },
    datadir::DataDirectory,
    DaemonControl, DaemonHandle,
};
use coincube_core::descriptors;

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
    pub(crate) record_replay: Option<sync::Arc<sync::Mutex<crate::bitcoin::WalletRecordReplay>>>,
    pub(crate) request_replay_during_received: bool,
    pub rescan_start: Option<BlockChainTip>,
    pub poll_failure: Option<&'static str>,
    pub received: Vec<UTxO>,
    pub broadcasted: sync::Mutex<Vec<Transaction>>,
    pub broadcast_error: Option<String>,
    pub rescan_requests: Vec<u32>,
    pub tip_timestamp: Option<u32>,
    pub genesis_error: Option<crate::connect::AdmissionError>,
    pub genesis_timestamp_override: Option<u32>,
    pub txs: HashMap<Txid, (Transaction, Option<Block>)>,
    /// What `chain_tip` reports. Defaults to the historical fixed value (height 100).
    pub tip: BlockChainTip,
    /// What `is_in_chain` reports. Defaults to `true`, i.e. no reorg.
    pub in_chain: bool,
    /// What `common_ancestor` reports. `None` models a lookup that keeps failing.
    pub ancestor: Option<BlockChainTip>,
    /// Blocks this backend's chain contains regardless of [`Self::in_chain`], so a
    /// forked backend can still be asked about a block deeper than the fork point.
    pub also_in_chain: Vec<BlockChainTip>,
    /// Which node this backend claims to be, for scoping a sanctioned rollback.
    pub backend_id: Option<BackendId>,
    /// What `walks_common_ancestor` reports. Defaults to `true`, i.e. a bitcoind-like
    /// backend. Set to `false` to model Electrum/Esplora, which report reorgs from
    /// `sync_wallet` and cannot be asked for a fork point afterwards.
    pub walks_ancestors: bool,
    /// The history-sync record the daemon installed, shared so a test can check
    /// which one it was after handing this backend over.
    pub history_sync: sync::Arc<sync::Mutex<Option<sync::Arc<crate::bitcoin::HistorySyncCache>>>>,
    /// What `sees_mempool` reports. Defaults to `true`, i.e. a backend whose
    /// (always empty) mempool answers are trusted. Set to `false` to model
    /// Esplora, whose mempool reads are always empty whatever is in it.
    pub sees_mempool: bool,
    /// What the next `sync_wallet` reports as a reorg's common ancestor, the way
    /// Electrum/Esplora hand it over. Reported once, then `None`.
    pub sync_reorg_ancestor: Option<BlockChainTip>,
}

/// The endpoint [`DummyBitcoind`] reports by default.
pub const DUMMY_RPC_ADDR: &str = "127.0.0.1:8332";

/// The credential descriptor [`DummyBitcoind`] reports by default.
pub const DUMMY_CREDENTIALS: &str = "cookie:/dummy/.cookie";

/// The identity [`DummyBitcoind`] reports by default.
pub fn dummy_backend_id(addr: &str, credentials: &str) -> BackendId {
    BackendId {
        addr: addr.parse().expect("valid socket address"),
        credentials: BackendId::fingerprint(credentials),
    }
}

impl DummyBitcoind {
    pub fn new() -> Self {
        let hash = bitcoin::BlockHash::from_str(
            "000000007bc154e0fa7ea32218a72fe2c1bb9f86cf8c9ebf9a715ed27fdb229a",
        )
        .unwrap();
        Self {
            record_replay: None,
            request_replay_during_received: false,
            rescan_start: None,
            poll_failure: None,
            received: Vec::new(),
            broadcasted: sync::Mutex::new(Vec::new()),
            broadcast_error: None,
            rescan_requests: Vec::new(),
            genesis_error: None,
            genesis_timestamp_override: None,
            tip_timestamp: None,
            txs: HashMap::new(),
            tip: BlockChainTip { hash, height: 100 },
            in_chain: true,
            ancestor: None,
            also_in_chain: Vec::new(),
            backend_id: Some(dummy_backend_id(DUMMY_RPC_ADDR, DUMMY_CREDENTIALS)),
            walks_ancestors: true,
            history_sync: Default::default(),
            sees_mempool: true,
            sync_reorg_ancestor: None,
        }
    }
}

impl BitcoinInterface for DummyBitcoind {
    fn wallet_record_replay_pending(&self) -> Option<u64> {
        self.record_replay
            .as_ref()
            .and_then(|state| state.lock().unwrap().pending())
    }
    fn acknowledge_wallet_record_replay(&mut self, ticket: u64) {
        if let Some(state) = &self.record_replay {
            state.lock().unwrap().acknowledge(ticket);
        }
    }
    fn set_history_sync_cache(&mut self, cache: sync::Arc<crate::bitcoin::HistorySyncCache>) {
        *self.history_sync.lock().unwrap() = Some(cache);
    }

    fn try_received_coins(
        &self,
        _: &BlockChainTip,
        _: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
    ) -> Result<Vec<UTxO>, String> {
        if self.request_replay_during_received {
            self.record_replay
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .request()
                .unwrap();
        }
        Ok(self.received.clone())
    }
    fn try_confirmed_coins(
        &self,
        outpoints: &[bitcoin::OutPoint],
    ) -> Result<crate::bitcoin::ConfirmedCoins, String> {
        if self.poll_failure == Some("confirmed") {
            return Err("injected transport failure".into());
        }
        Ok(self.confirmed_coins(outpoints))
    }
    fn try_rescan_progress(&self) -> Result<Option<f64>, String> {
        if self.poll_failure == Some("rescan") {
            return Err("injected rescan transport failure".into());
        }
        Ok(self.rescan_progress())
    }

    fn genesis_block_timestamp(&self) -> Result<u32, crate::bitcoin::GenesisError> {
        self.genesis_block()?;
        Ok(self.genesis_timestamp_override.unwrap_or(1231006505))
    }

    fn genesis_block(&self) -> Result<BlockChainTip, crate::bitcoin::GenesisError> {
        if let Some(error) = self.genesis_error {
            return Err(crate::bitcoin::GenesisError::Esplora(Box::new(
                crate::bitcoin::esplora::client::Error::Admission(error),
            )));
        }
        let hash = bitcoin::BlockHash::from_str(
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f",
        )
        .unwrap();
        Ok(BlockChainTip { hash, height: 0 })
    }

    fn sync_progress(&self) -> SyncProgress {
        SyncProgress::new(1.0, 1_000, 1_000)
    }

    fn chain_tip(&self) -> BlockChainTip {
        self.tip
    }

    fn is_in_chain(&self, tip: &BlockChainTip) -> bool {
        self.in_chain || self.also_in_chain.contains(tip)
    }

    fn backend_id(&self) -> Option<BackendId> {
        self.backend_id.clone()
    }

    fn sync_wallet(
        &mut self,
        _receive_index: bip32::ChildNumber,
        _change_index: bip32::ChildNumber,
    ) -> Result<Option<BlockChainTip>, String> {
        Ok(self.sync_reorg_ancestor.take())
    }

    fn received_coins(
        &self,
        _: &BlockChainTip,
        _: &[miniscript::Descriptor<miniscript::DescriptorPublicKey>],
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

    fn common_ancestor(&self, tip: &BlockChainTip, max_depth: i32) -> AncestorSearch {
        match self.ancestor {
            // An ancestor above our tip is a nonsensical fixture; a real backend
            // could never return one, so reject it before considering the bound.
            Some(ancestor) if ancestor.height > tip.height => AncestorSearch::Failed,
            // Honour the bound like a real backend would, so callers are exercised
            // against the truncated case and not just the found one.
            Some(ancestor) if tip.height.saturating_sub(ancestor.height) > max_depth => {
                AncestorSearch::TooDeep
            }
            Some(ancestor) => AncestorSearch::Found(ancestor),
            None => AncestorSearch::Failed,
        }
    }

    fn walks_common_ancestor(&self) -> bool {
        self.walks_ancestors
    }

    fn broadcast_tx(&self, transaction: &bitcoin::Transaction) -> Result<(), String> {
        self.broadcasted.lock().unwrap().push(transaction.clone());
        match &self.broadcast_error {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    fn start_rescan(
        &mut self,
        _: &descriptors::CoincubeDescriptor,
        timestamp: u32,
    ) -> Result<(), String> {
        self.rescan_requests.push(timestamp);
        Ok(())
    }

    fn rescan_progress(&self) -> Option<f64> {
        None
    }

    fn block_before_date(&self, _: u32) -> Option<BlockChainTip> {
        self.rescan_start
    }

    fn tip_time(&self) -> Option<u32> {
        self.tip_timestamp
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

    fn sees_mempool(&self) -> bool {
        self.sees_mempool
    }
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
    rollbacks: Vec<BlockChainTip>,
}

pub struct DummyDatabase {
    db: sync::Arc<sync::RwLock<DummyDbState>>,
}

impl DatabaseInterface for DummyDatabase {
    fn reserve_change(
        &self,
        chain: coincube_core::chain::ChainId,
        descriptor: &coincube_core::descriptors::CoincubeDescriptor,
        _: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) -> Result<crate::database::ChangeReservation, crate::database::ReservationError> {
        let mut state = self.db.write().unwrap();
        let index = state
            .change_index
            .increment()
            .map_err(|_| crate::database::ReservationError::Exhausted)?;
        if index.is_hardened() {
            return Err(crate::database::ReservationError::Exhausted);
        }
        state.change_index = index;
        Ok(crate::database::ChangeReservation {
            chain,
            descriptor: descriptor.clone(),
            index,
        })
    }

    fn commit_change_if_next(
        &self,
        _: coincube_core::chain::ChainId,
        _: &coincube_core::descriptors::CoincubeDescriptor,
        _: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
        index: bip32::ChildNumber,
    ) -> Result<bool, crate::database::ReservationError> {
        if index.is_hardened() {
            return Err(crate::database::ReservationError::Exhausted);
        }
        let mut state = self.db.write().unwrap();
        let next = state
            .change_index
            .increment()
            .map_err(|_| crate::database::ReservationError::Exhausted)?;
        if next != index {
            return Ok(false);
        }
        state.change_index = index;
        Ok(true)
    }

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
                rollbacks: Vec::new(),
            })),
        }
    }

    /// Every tip this database was asked to roll back to, in order.
    pub fn rollbacks(&self) -> Vec<BlockChainTip> {
        self.db.read().unwrap().rollbacks.clone()
    }

    /// The outpoints currently stored, for asserting that a poll did or did not
    /// remove coins.
    pub fn coin_outpoints(&self) -> Vec<bitcoin::OutPoint> {
        self.db.read().unwrap().coins.keys().copied().collect()
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
        let mut state = self.db.write().unwrap();
        state.change_index = state.change_index.max(index);
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

    fn list_spend(&mut self) -> Vec<(Psbt, Option<u32>, Option<bitcoin::Txid>)> {
        self.db
            .read()
            .unwrap()
            .spend_txs
            .values()
            .cloned()
            .map(|(psbt, updated_at)| (psbt, updated_at, None))
            .collect()
    }

    fn delete_spend(&mut self, txid: &bitcoin::Txid) {
        self.db.write().unwrap().spend_txs.remove(txid);
    }

    fn rollback_tip(&mut self, new_tip: &BlockChainTip) {
        let mut db = self.db.write().unwrap();
        db.rollbacks.push(*new_tip);
        db.curr_tip = Some(*new_tip);
    }

    fn rescan_timestamp(&mut self) -> Option<u32> {
        self.db.read().unwrap().rescan_timestamp
    }

    fn set_rescan(&mut self, timestamp: u32) {
        self.db.write().unwrap().rescan_timestamp = Some(timestamp);
    }

    fn complete_rescan(&mut self) {
        self.db.write().unwrap().rescan_timestamp = None;
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

    fn get_labels_bip329(&mut self, _offset: u32, _limit: u32) -> bip329::Labels {
        todo!()
    }
}

pub struct DummyCoincube {
    pub tmp_dir: path::PathBuf,
    pub handle: DaemonHandle,
}

fn uid() -> usize {
    static COUNTER: sync::atomic::AtomicUsize = sync::atomic::AtomicUsize::new(0);
    COUNTER.fetch_add(1, sync::atomic::Ordering::Relaxed)
}

// The schema of database version 8, frozen verbatim from before the v9 chain-identity
// migration so the v8 fixtures below are genuine (the version-8 `tip` has no `chain`).
pub const V8_SCHEMA: &str = "\
CREATE TABLE version (
version INTEGER NOT NULL
);

/* About the Bitcoin network. */
CREATE TABLE tip (
network TEXT NOT NULL,
blockheight INTEGER,
blockhash BLOB
);

/* This stores metadata about our wallet. We only support single wallet for
 * now (and the foreseeable future).
 *
 * The 'timestamp' field is the creation date of the wallet. We guarantee to have seen all
 * information related to our descriptor(s) that occurred after this date.
 * The optional 'rescan_timestamp' field is a the timestamp we need to rescan the chain
 * for events related to our descriptor(s) from.
 */
CREATE TABLE wallets (
id INTEGER PRIMARY KEY NOT NULL,
timestamp INTEGER NOT NULL,
main_descriptor TEXT NOT NULL,
deposit_derivation_index INTEGER NOT NULL,
change_derivation_index INTEGER NOT NULL,
rescan_timestamp INTEGER,
last_poll_timestamp INTEGER
);

/* Our (U)TxOs.
 *
 * The 'spend_block_height' and 'spend_block.time' are only present if the spending
 * transaction for this coin exists and was confirmed.
 *
 * The 'is_immature' field is for coinbase deposits that are not yet buried under 100
 * blocks. Note coinbase deposits can't technically be unconfirmed but we keep them
 * as such until they become mature.
 *
 * The `is_from_self` field indicates if the coin is the output of a transaction whose
 * inputs are all from the same wallet as the coin. For an unconfirmed coin, this also
 * means that all unconfirmed ancestors, if any, are from self.
 */
CREATE TABLE coins (
id INTEGER PRIMARY KEY NOT NULL,
wallet_id INTEGER NOT NULL,
blockheight INTEGER,
blocktime INTEGER,
txid BLOB NOT NULL,
vout INTEGER NOT NULL,
amount_sat INTEGER NOT NULL,
derivation_index INTEGER NOT NULL,
is_change BOOLEAN NOT NULL CHECK (is_change IN (0,1)),
spend_txid BLOB,
spend_block_height INTEGER,
spend_block_time INTEGER,
is_immature BOOLEAN NOT NULL CHECK (is_immature IN (0,1)),
is_from_self BOOLEAN NOT NULL DEFAULT 0 CHECK (is_from_self IN (0,1)),
UNIQUE (txid, vout),
FOREIGN KEY (wallet_id) REFERENCES wallets (id)
    ON UPDATE RESTRICT
    ON DELETE RESTRICT,
FOREIGN KEY (txid) REFERENCES transactions (txid)
    ON UPDATE RESTRICT
    ON DELETE RESTRICT,
FOREIGN KEY (spend_txid) REFERENCES transactions (txid)
    ON UPDATE RESTRICT
    ON DELETE RESTRICT
);

/* A mapping from descriptor address to derivation index. Necessary until
 * we can get the derivation index from the parent descriptor from bitcoind.
 */
CREATE TABLE addresses (
receive_address TEXT NOT NULL UNIQUE,
change_address TEXT NOT NULL UNIQUE,
derivation_index INTEGER NOT NULL UNIQUE
);

/* Transactions for all wallets. */
CREATE TABLE transactions (
id INTEGER PRIMARY KEY NOT NULL,
txid BLOB UNIQUE NOT NULL,
tx BLOB UNIQUE NOT NULL,
num_inputs INTEGER CHECK (num_inputs IS NULL OR num_inputs > 0),
num_outputs INTEGER CHECK (num_outputs IS NULL OR num_outputs > 0),
is_coinbase BOOLEAN NOT NULL DEFAULT 0 CHECK (is_coinbase IN (0,1))
);

/* Transactions we created that spend some of our coins. */
CREATE TABLE spend_transactions (
id INTEGER PRIMARY KEY NOT NULL,
psbt BLOB UNIQUE NOT NULL,
txid BLOB UNIQUE NOT NULL,
updated_at INTEGER
);

/* Labels applied on addresses (0), outpoints (1), txids (2) */
CREATE TABLE labels (
id INTEGER PRIMARY KEY NOT NULL,
wallet_id INTEGER NOT NULL,
item_kind INTEGER NOT NULL CHECK (item_kind IN (0,1,2)),
item TEXT UNIQUE NOT NULL,
value TEXT NOT NULL
);
";

pub fn tmp_dir() -> path::PathBuf {
    env::temp_dir().join(format!(
        "coincubed-{}-{:?}-{}",
        process::id(),
        thread::current().id(),
        uid(),
    ))
}

impl DummyCoincube {
    /// Creates a new DummyCoincube interface
    pub fn _new(
        bitcoin_interface: impl BitcoinInterface + 'static,
        database: impl DatabaseInterface + 'static,
        rpc_server: bool,
        timelock: u16,
    ) -> DummyCoincube {
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        // Use a shorthand for 'datadir', to avoid overflowing SUN_LEN on MacOS.
        let root_directory: path::PathBuf =
            [tmp_dir.as_path(), path::Path::new("d")].iter().collect();
        fs::create_dir_all(&root_directory).unwrap();
        let mut data_directory = root_directory.clone();
        data_directory.push("bitcoin");

        let network = bitcoin::Network::Bitcoin;
        let bitcoin_config = BitcoinConfig::new(
            coincube_core::chain::ChainId::from(network),
            time::Duration::from_secs(2),
        );

        let owner_key = descriptors::PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*").unwrap());
        let heir_key = descriptors::PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/<0;1>/*").unwrap());
        let policy = descriptors::CoincubePolicy::new_legacy(
            owner_key,
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap();
        let desc = descriptors::CoincubeDescriptor::new(policy);
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
        DummyCoincube { tmp_dir, handle }
    }

    /// Creates a new DummyCoincube interface
    pub fn new(
        bitcoin_interface: impl BitcoinInterface + 'static,
        database: impl DatabaseInterface + 'static,
    ) -> DummyCoincube {
        Self::_new(bitcoin_interface, database, false, 10_000)
    }

    /// Creates a new DummyCoincube interface with the specified recovery path timelock.
    pub fn new_timelock(
        bitcoin_interface: impl BitcoinInterface + 'static,
        database: impl DatabaseInterface + 'static,
        timelock: u16,
    ) -> DummyCoincube {
        Self::_new(bitcoin_interface, database, false, timelock)
    }

    /// Creates a new DummyCoincube interface which also spins up an RPC server.
    #[allow(dead_code)]
    pub fn new_server(
        bitcoin_interface: impl BitcoinInterface + 'static,
        database: impl DatabaseInterface + 'static,
    ) -> DummyCoincube {
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

/// Capture the log records emitted on the current thread while running `f`.
///
/// The logger is process-wide and installed once; it keeps only the records of
/// threads inside a `capture_logs` call, so tests running in parallel do not see
/// each other's records. Only warnings and errors are captured; see
/// [`capture_logs_at`] for a lower level. Returns `f`'s result and the
/// `(level, message)` pairs.
pub fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, Vec<(log::Level, String)>) {
    capture_logs_at(log::Level::Warn, f)
}

/// [`capture_logs`], keeping the records at `level` and above.
///
/// The `log` crate's maximum level is process-wide, so this raises it to
/// `level` for every thread, for the rest of the test binary: it is never
/// lowered again, since a concurrent capture may still need it. Every test
/// running at the same time — and after — therefore evaluates the arguments of
/// log calls down to `level`, which is what makes it a check that a log call
/// has no side effect (#628).
pub fn capture_logs_at<T>(
    level: log::Level,
    f: impl FnOnce() -> T,
) -> (T, Vec<(log::Level, String)>) {
    use std::cell::RefCell;

    /// The capturing thread's level and the records kept so far.
    type Capturing = Option<(log::Level, Vec<(log::Level, String)>)>;
    thread_local! {
        static CAPTURED: RefCell<Capturing> = const { RefCell::new(None) };
    }
    struct Capture;
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            // Format only on a capturing thread: every other test's records
            // pass through here too, and must not be slowed down.
            CAPTURED.with(|captured| {
                if let Some((level, records)) = captured.borrow_mut().as_mut() {
                    // The global level may be lower than this capture's, if
                    // another test raised it.
                    if record.level() <= *level {
                        records.push((record.level(), record.args().to_string()));
                    }
                }
            });
        }
        fn flush(&self) {}
    }
    static LOGGER: Capture = Capture;
    // Held while installing or raising the level, so two captures racing to
    // raise it cannot leave it at the lower of the two.
    static MAX_LEVEL: sync::Mutex<()> = sync::Mutex::new(());
    {
        let _guard = MAX_LEVEL.lock().unwrap_or_else(|e| e.into_inner());
        static INSTALL: sync::Once = sync::Once::new();
        INSTALL.call_once(|| {
            log::set_logger(&LOGGER).expect("no other logger in the coincubed unit tests");
            // Warn and above by default: the records most tests assert on. A
            // lower level makes every `info!`/`debug!` in the parallel tests
            // reach here, so it is only set by a capture that asks for it.
            log::set_max_level(log::LevelFilter::Warn);
        });
        if log::max_level() < level.to_level_filter() {
            log::set_max_level(level.to_level_filter());
        }
    }

    CAPTURED.with(|captured| *captured.borrow_mut() = Some((level, Vec::new())));
    let result = f();
    let records = CAPTURED.with(|captured| {
        captured
            .borrow_mut()
            .take()
            .map(|(_, records)| records)
            .unwrap_or_default()
    });
    (result, records)
}
