//! Database interface for Liana.
//!
//! Record wallet metadata, spent and unspent coins, ongoing transactions.

pub mod sqlite;

use crate::{
    bitcoin::BlockChainTip,
    database::sqlite::{
        schema::{
            DbBlockInfo, DbCoin, DbGraphLayoutEntry, DbGraphWallet, DbTip, DbWalletTransaction,
        },
        SqliteConn, SqliteDb,
    },
};

use std::{
    collections::{HashMap, HashSet},
    fmt,
    iter::FromIterator,
    str::FromStr,
    sync,
};

use bip329::Labels;
use liana::label::Label;
pub use liana::label::LabelItem;
use miniscript::bitcoin::{self, bip32, psbt::Psbt, secp256k1};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Information about the wallet.
///
/// All timestamps are the number of seconds since the UNIX epoch.
#[derive(Clone, Debug)]
pub struct Wallet {
    /// Timestamp at wallet creation time.
    pub timestamp: u32,
    /// Derivation index for the last used/revealed receiving address.
    pub receive_index: bip32::ChildNumber,
    /// Derivation index for the last used/revealed change address.
    pub change_index: bip32::ChildNumber,
    /// Timestamp to start rescanning from, if any.
    pub rescan_timestamp: Option<u32>,
    /// Timestamp at which the last poll of the blockchain completed, if any,
    pub last_poll_timestamp: Option<u32>,
}

pub trait DatabaseInterface: Send {
    fn connection(&self) -> Box<dyn DatabaseConnection>;
}

impl DatabaseInterface for SqliteDb {
    fn connection(&self) -> Box<dyn DatabaseConnection> {
        Box::new(self.connection().expect("Database must be available"))
    }
}

// FIXME: do we need to repeat the entire trait implementation? Isn't there a nicer way?
impl DatabaseInterface for sync::Arc<sync::Mutex<dyn DatabaseInterface>> {
    fn connection(&self) -> Box<dyn DatabaseConnection> {
        self.lock().unwrap().connection()
    }
}

pub trait DatabaseConnection {
    /// Get the tip of the best chain we've seen.
    fn chain_tip(&mut self) -> Option<BlockChainTip>;

    /// The network we are operating on.
    fn network(&mut self) -> bitcoin::Network;

    /// Get the `Wallet`.
    fn wallet(&mut self) -> Wallet;

    /// The timestamp at wallet creation time
    fn timestamp(&mut self) -> u32;

    /// Update our best chain seen.
    fn update_tip(&mut self, tip: &BlockChainTip);

    /// Get the derivation index for the last used/revealed receiving address
    fn receive_index(&mut self) -> bip32::ChildNumber;

    /// Set the derivation index for the last used/revealed receiving address
    fn set_receive_index(
        &mut self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    );

    /// Get the derivation index for the last used/revealed change address
    fn change_index(&mut self) -> bip32::ChildNumber;

    /// Set the derivation index for the last used/revealed change address
    fn set_change_index(
        &mut self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    );

    /// Get the timestamp at which to start rescaning from, if any.
    fn rescan_timestamp(&mut self) -> Option<u32>;

    /// Set a timestamp at which to start rescaning the block chain from.
    fn set_rescan(&mut self, timestamp: u32);

    /// Mark the rescan as complete.
    fn complete_rescan(&mut self);

    /// Get the timestamp at which the last poll of the blockchain completed, if any,
    /// as the number of seconds since the UNIX epoch.
    fn last_poll_timestamp(&mut self) -> Option<u32>;

    /// Set the timestamp at which the last poll of the blockchain completed,
    /// where `timestamp` should be given as the number of seconds since the UNIX epoch.
    fn set_last_poll(&mut self, timestamp: u32);

    /// Get the derivation index for this address, as well as whether this address is change.
    fn derivation_index_by_address(
        &mut self,
        address: &bitcoin::Address,
    ) -> Option<(bip32::ChildNumber, bool)>;

    /// Get all our coins, past or present, spent or not.
    fn coins(
        &mut self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin>;

    /// Get our coins as [`DatabaseConnection::coins`] does, along with their default label.
    fn coins_with_default_label(
        &mut self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, CoinWithDefaultLabel>;

    /// List coins that are being spent and whose spending transaction is still unconfirmed.
    fn list_spending_coins(&mut self) -> HashMap<bitcoin::OutPoint, Coin>;

    /// Store new UTxOs. Coins must not already be in database.
    fn new_unspent_coins(&mut self, coins: &[Coin]);

    /// Remove some UTxOs from the database.
    fn remove_coins(&mut self, coins: &[bitcoin::OutPoint]);

    /// Mark a set of coins as being confirmed at a specified height and block time.
    /// NOTE: if the coin comes from an immature coinbase transaction, this will mark it as mature.
    /// Immature coinbase deposits must not be confirmed before they are 100 blocks deep in the
    /// chain.
    fn confirm_coins(&mut self, outpoints: &[(bitcoin::OutPoint, i32, u32)]);

    /// Mark a set of coins as being spent by a specified txid of a pending transaction.
    fn spend_coins(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)]);

    /// Mark a set of coins as not being spent anymore.
    fn unspend_coins(&mut self, outpoints: &[bitcoin::OutPoint]);

    /// Mark a set of coins as spent by a specified txid at a specified block time.
    fn confirm_spend(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid, i32, u32)]);

    /// Get specific coins from the database.
    fn coins_by_outpoints(
        &mut self,
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin>;

    fn spend_tx(&mut self, txid: &bitcoin::Txid) -> Option<Psbt>;

    /// Insert a new Spend transaction or replace an existing one.
    fn store_spend(&mut self, psbt: &Psbt);

    /// List all existing Spend transactions, along with an optional last update timestamp.
    fn list_spend(&mut self) -> Vec<(Psbt, Option<u32>)>;

    /// Delete a Spend transaction from database.
    fn delete_spend(&mut self, txid: &bitcoin::Txid);

    /// Update, for a set of items (as key), their label (as value). A `None` value deletes the
    /// label.
    fn update_labels(&mut self, items: &HashMap<LabelItem, Option<String>>);

    fn labels(&mut self, labels: &HashSet<LabelItem>) -> HashMap<String, String>;

    /// Mark the given tip as the new best seen block. Update stored data accordingly.
    fn rollback_tip(&mut self, new_tip: &BlockChainTip);

    /// Retrieve a limited list of txids that where deposited or spent between the start and end timestamps (inclusive bounds)
    fn list_txids(&mut self, start: u32, end: u32, limit: u64) -> Vec<bitcoin::Txid>;

    /// Retrieves all txids from the transactions table whether or not they are referenced by a coin.
    fn list_saved_txids(&mut self) -> Vec<bitcoin::Txid>;

    /// Store transactions in database, ignoring any that already exist.
    fn new_txs(&mut self, txs: &[bitcoin::Transaction]);

    /// For all unconfirmed coins and those confirmed after `prev_tip_height`,
    /// update whether the coin is from self or not.
    fn update_coins_from_self(&mut self, prev_tip_height: i32);

    /// Retrieve a list of transactions and their corresponding block heights, times and default
    /// labels.
    fn list_wallet_transactions(&mut self, txids: &[bitcoin::Txid]) -> Vec<WalletTransaction>;

    fn list_txs_without_default_label(&mut self) -> Vec<bitcoin::Transaction>;

    fn list_coins_without_default_label(&mut self) -> Vec<bitcoin::OutPoint>;

    /// Store the default label of transactions and coins, keeping the ones already stored.
    fn store_default_labels(
        &mut self,
        txs: &HashMap<bitcoin::Txid, Label>,
        coins: &HashMap<bitcoin::OutPoint, Label>,
    );

    /// Dump all labels
    fn get_labels_bip329(&mut self, offset: u32, limit: u32) -> Labels;

    /// Stored transaction map layout entries, in insertion order.
    fn graph_layout(&mut self) -> Vec<GraphLayoutEntry>;

    /// Replace the `set` entries, then delete the `remove` items, atomically.
    fn update_graph_layout(&mut self, set: &[GraphLayoutEntry], remove: &[GraphItem]);

    /// Other wallets shown on the transaction map, in insertion order.
    fn graph_wallets(&mut self) -> Vec<GraphWallet>;

    /// Replace the stored entries of these wallets, creating the missing ones, atomically.
    fn update_graph_wallets(&mut self, wallets: &[GraphWallet]);
}

impl DatabaseConnection for SqliteConn {
    fn chain_tip(&mut self) -> Option<BlockChainTip> {
        match self.db_tip() {
            DbTip {
                block_height: Some(height),
                block_hash: Some(hash),
                ..
            } => Some(BlockChainTip { height, hash }),
            _ => None,
        }
    }

    fn network(&mut self) -> bitcoin::Network {
        self.db_tip().network
    }

    fn wallet(&mut self) -> Wallet {
        let db_wallet = self.db_wallet();
        Wallet {
            timestamp: db_wallet.timestamp,
            receive_index: db_wallet.deposit_derivation_index,
            change_index: db_wallet.change_derivation_index,
            rescan_timestamp: db_wallet.rescan_timestamp,
            last_poll_timestamp: db_wallet.last_poll_timestamp,
        }
    }

    fn timestamp(&mut self) -> u32 {
        self.wallet().timestamp
    }

    fn update_tip(&mut self, tip: &BlockChainTip) {
        self.update_tip(tip)
    }

    fn receive_index(&mut self) -> bip32::ChildNumber {
        self.wallet().receive_index
    }

    fn set_receive_index(
        &mut self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) {
        self.set_derivation_index(index, false, secp)
    }

    fn change_index(&mut self) -> bip32::ChildNumber {
        self.wallet().change_index
    }

    fn set_change_index(
        &mut self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    ) {
        self.set_derivation_index(index, true, secp)
    }

    fn rescan_timestamp(&mut self) -> Option<u32> {
        self.wallet().rescan_timestamp
    }

    fn set_rescan(&mut self, timestamp: u32) {
        self.set_wallet_rescan_timestamp(timestamp)
    }

    fn complete_rescan(&mut self) {
        self.complete_wallet_rescan()
    }

    fn last_poll_timestamp(&mut self) -> Option<u32> {
        self.wallet().last_poll_timestamp
    }

    fn set_last_poll(&mut self, timestamp: u32) {
        self.set_wallet_last_poll_timestamp(timestamp)
            .expect("database must be available")
    }

    fn coins(
        &mut self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin> {
        self.coins(statuses, outpoints)
            .into_iter()
            .map(|db_coin| (db_coin.outpoint, db_coin.into()))
            .collect()
    }

    fn coins_with_default_label(
        &mut self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, CoinWithDefaultLabel> {
        self.coins(statuses, outpoints)
            .into_iter()
            .map(|db_coin| {
                let coin = CoinWithDefaultLabel::from(db_coin);
                (coin.coin.outpoint, coin)
            })
            .collect()
    }

    fn list_spending_coins(&mut self) -> HashMap<bitcoin::OutPoint, Coin> {
        self.list_spending_coins()
            .into_iter()
            .map(|db_coin| (db_coin.outpoint, db_coin.into()))
            .collect()
    }

    fn new_unspent_coins<'a>(&mut self, coins: &[Coin]) {
        self.new_unspent_coins(coins)
    }

    fn remove_coins(&mut self, outpoints: &[bitcoin::OutPoint]) {
        self.remove_coins(outpoints)
    }

    fn confirm_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, i32, u32)]) {
        self.confirm_coins(outpoints)
    }

    fn spend_coins<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid)]) {
        self.spend_coins(outpoints)
    }

    fn unspend_coins(&mut self, outpoints: &[bitcoin::OutPoint]) {
        self.unspend_coins(outpoints)
    }

    fn confirm_spend<'a>(&mut self, outpoints: &[(bitcoin::OutPoint, bitcoin::Txid, i32, u32)]) {
        self.confirm_spend(outpoints)
    }

    fn derivation_index_by_address(
        &mut self,
        address: &bitcoin::Address,
    ) -> Option<(bip32::ChildNumber, bool)> {
        self.db_address(address).map(|db_addr| {
            (
                db_addr.derivation_index,
                // We only compare address strings in case `assume_checked()` uses a different network.
                // E.g. An unchecked signet address would have its network set to testnet and so comparing
                // to a signet `Address` would never match.
                address.to_string() == db_addr.change_address.assume_checked().to_string(),
            )
        })
    }

    fn coins_by_outpoints(
        &mut self,
        outpoints: &[bitcoin::OutPoint],
    ) -> HashMap<bitcoin::OutPoint, Coin> {
        self.db_coins(outpoints)
            .into_iter()
            .map(|db_coin| (db_coin.outpoint, db_coin.into()))
            .collect()
    }

    fn spend_tx(&mut self, txid: &bitcoin::Txid) -> Option<Psbt> {
        self.db_spend(txid).map(|db_spend| db_spend.psbt)
    }

    fn store_spend(&mut self, psbt: &Psbt) {
        self.store_spend(psbt)
    }

    fn list_spend(&mut self) -> Vec<(Psbt, Option<u32>)> {
        self.list_spend()
            .into_iter()
            .map(|db_spend| (db_spend.psbt, db_spend.updated_at))
            .collect()
    }

    fn delete_spend(&mut self, txid: &bitcoin::Txid) {
        self.delete_spend(txid)
    }

    fn update_labels(&mut self, items: &HashMap<LabelItem, Option<String>>) {
        self.update_labels(items)
    }

    fn labels(&mut self, items: &HashSet<LabelItem>) -> HashMap<String, String> {
        let labels = self.db_labels(items);
        HashMap::from_iter(labels.into_iter().map(|label| (label.item, label.value)))
    }

    fn get_labels_bip329(&mut self, offset: u32, limit: u32) -> Labels {
        let labels = self
            .labels_bip329(offset, limit)
            .into_iter()
            .map(|l| l.into())
            .collect();
        Labels::new(labels)
    }

    fn graph_layout(&mut self) -> Vec<GraphLayoutEntry> {
        self.graph_layout()
            .into_iter()
            .map(GraphLayoutEntry::from)
            .collect()
    }

    fn update_graph_layout(&mut self, set: &[GraphLayoutEntry], remove: &[GraphItem]) {
        self.update_graph_layout(set, remove)
    }

    fn graph_wallets(&mut self) -> Vec<GraphWallet> {
        self.graph_wallets()
            .into_iter()
            .map(GraphWallet::from)
            .collect()
    }

    fn update_graph_wallets(&mut self, wallets: &[GraphWallet]) {
        self.update_graph_wallets(wallets)
    }

    fn rollback_tip(&mut self, new_tip: &BlockChainTip) {
        self.rollback_tip(new_tip)
    }

    fn list_txids(&mut self, start: u32, end: u32, limit: u64) -> Vec<bitcoin::Txid> {
        self.db_list_txids(start, end, limit)
    }

    fn list_saved_txids(&mut self) -> Vec<bitcoin::Txid> {
        self.db_list_saved_txids()
    }

    fn new_txs<'a>(&mut self, txs: &[bitcoin::Transaction]) {
        self.new_txs(txs)
    }

    fn update_coins_from_self(&mut self, prev_tip_height: i32) {
        self.update_coins_from_self(prev_tip_height)
            .expect("must not fail")
    }

    fn list_wallet_transactions(&mut self, txids: &[bitcoin::Txid]) -> Vec<WalletTransaction> {
        self.list_wallet_transactions(txids)
            .into_iter()
            .map(WalletTransaction::from)
            .collect()
    }

    fn list_txs_without_default_label(&mut self) -> Vec<bitcoin::Transaction> {
        self.list_txs_without_default_label()
    }

    fn list_coins_without_default_label(&mut self) -> Vec<bitcoin::OutPoint> {
        self.list_coins_without_default_label()
    }

    fn store_default_labels(
        &mut self,
        txs: &HashMap<bitcoin::Txid, Label>,
        coins: &HashMap<bitcoin::OutPoint, Label>,
    ) {
        self.store_default_labels(txs, coins)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockInfo {
    pub height: i32,
    pub time: u32,
}

impl From<DbBlockInfo> for BlockInfo {
    fn from(b: DbBlockInfo) -> BlockInfo {
        BlockInfo {
            height: b.height,
            time: b.time,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Coin {
    pub outpoint: bitcoin::OutPoint,
    pub is_immature: bool,
    pub block_info: Option<BlockInfo>,
    pub amount: bitcoin::Amount,
    pub derivation_index: bip32::ChildNumber,
    pub is_change: bool,
    pub spend_txid: Option<bitcoin::Txid>,
    pub spend_block: Option<BlockInfo>,
    pub is_from_self: bool,
}

impl std::convert::From<DbCoin> for Coin {
    fn from(db_coin: DbCoin) -> Coin {
        let DbCoin {
            outpoint,
            is_immature,
            block_info,
            amount,
            derivation_index,
            is_change,
            spend_txid,
            spend_block,
            is_from_self,
            ..
        } = db_coin;
        Coin {
            outpoint,
            is_immature,
            block_info: block_info.map(BlockInfo::from),
            amount,
            derivation_index,
            is_change,
            spend_txid,
            spend_block: spend_block.map(BlockInfo::from),
            is_from_self,
        }
    }
}

impl From<DbGraphLayoutEntry> for GraphLayoutEntry {
    fn from(db_entry: DbGraphLayoutEntry) -> GraphLayoutEntry {
        let DbGraphLayoutEntry {
            item,
            position,
            input_order,
            output_order,
        } = db_entry;
        GraphLayoutEntry {
            item,
            position,
            input_order,
            output_order,
        }
    }
}

impl From<DbGraphWallet> for GraphWallet {
    fn from(db_wallet: DbGraphWallet) -> GraphWallet {
        let DbGraphWallet {
            wallet,
            selected,
            offset,
        } = db_wallet;
        GraphWallet {
            wallet,
            selected,
            offset,
        }
    }
}

impl Coin {
    pub fn is_confirmed(&self) -> bool {
        self.block_info.is_some()
    }

    pub fn is_spent(&self) -> bool {
        self.spend_txid.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoinWithDefaultLabel {
    pub coin: Coin,
    pub default_label: Label,
}

impl From<DbCoin> for CoinWithDefaultLabel {
    fn from(db_coin: DbCoin) -> Self {
        let default_label = db_coin.default_label.clone().unwrap_or_default();
        Self {
            coin: db_coin.into(),
            default_label,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletTransaction {
    pub tx: bitcoin::Transaction,
    pub block_height: Option<i32>,
    pub block_time: Option<u32>,
    pub default_label: Label,
}

impl From<DbWalletTransaction> for WalletTransaction {
    fn from(wtx: DbWalletTransaction) -> Self {
        Self {
            tx: wtx.transaction,
            block_height: wtx.block_info.map(|b| b.height),
            block_time: wtx.block_info.map(|b| b.time),
            default_label: wtx.default_label.unwrap_or_default(),
        }
    }
}

/// Possible (mutually exclusive) status of a coin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoinStatus {
    /// Has not yet been included in a block and has no spend transaction.
    Unconfirmed,
    /// Has been included in a block and has no spend transaction.
    Confirmed,
    /// Has an unconfirmed spend transaction, but coin itself may not yet have been included in a block.
    Spending,
    /// Has a confirmed spend transaction.
    Spent,
}

impl CoinStatus {
    pub fn from_arg(s: &str) -> Option<CoinStatus> {
        match s {
            "unconfirmed" => Some(CoinStatus::Unconfirmed),
            "confirmed" => Some(CoinStatus::Confirmed),
            "spending" => Some(CoinStatus::Spending),
            "spent" => Some(CoinStatus::Spent),
            _ => None,
        }
    }

    /// Converts a `CoinStatus` to its equivalent argument name
    /// as used in the `listcoins` RPC command.
    pub fn to_arg(&self) -> &'static str {
        match self {
            CoinStatus::Unconfirmed => "unconfirmed",
            CoinStatus::Confirmed => "confirmed",
            CoinStatus::Spending => "spending",
            CoinStatus::Spent => "spent",
        }
    }
}

/// An item of the transaction map.
///
/// The same outpoint can be a payment leaf of one transaction and a counterparty coin leaf of
/// another, hence the two leaf kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GraphItem {
    Tx(bitcoin::Txid),
    OutputLeaf(bitcoin::OutPoint),
    InputLeaf(bitcoin::OutPoint),
}

impl fmt::Display for GraphItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphItem::Tx(txid) => write!(f, "tx:{txid}"),
            GraphItem::OutputLeaf(outpoint) => write!(f, "out:{outpoint}"),
            GraphItem::InputLeaf(outpoint) => write!(f, "in:{outpoint}"),
        }
    }
}

impl FromStr for GraphItem {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || format!("Invalid graph item '{s}'");
        let (kind, value) = s.split_once(':').ok_or_else(err)?;
        match kind {
            "tx" => bitcoin::Txid::from_str(value)
                .map(GraphItem::Tx)
                .map_err(|_| err()),
            "out" => bitcoin::OutPoint::from_str(value)
                .map(GraphItem::OutputLeaf)
                .map_err(|_| err()),
            "in" => bitcoin::OutPoint::from_str(value)
                .map(GraphItem::InputLeaf)
                .map_err(|_| err()),
            _ => Err(err()),
        }
    }
}

impl Serialize for GraphItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for GraphItem {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        GraphItem::from_str(&s).map_err(de::Error::custom)
    }
}

/// Stored layout of a transaction map item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphLayoutEntry {
    pub item: GraphItem,
    /// Top-left corner in graph coordinates.
    pub position: Option<(f64, f64)>,
    /// Display order of the input slots: `input_order[row]` is the true input index shown at
    /// display row `row`. `None` means the true transaction order. Only used for `GraphItem::Tx`.
    pub input_order: Option<Vec<u32>>,
    /// Same as `input_order`, for the output slots.
    pub output_order: Option<Vec<u32>>,
}

/// Another wallet shown on the transaction map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphWallet {
    /// Identifier of the other wallet.
    pub wallet: String,
    pub selected: bool,
    /// Offset of the other wallet's items in graph coordinates.
    pub offset: Option<(f64, f64)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coin_status_as_arg() {
        assert_eq!(
            CoinStatus::from_arg(CoinStatus::Unconfirmed.to_arg()),
            Some(CoinStatus::Unconfirmed)
        );
        assert_eq!(
            CoinStatus::from_arg(CoinStatus::Confirmed.to_arg()),
            Some(CoinStatus::Confirmed)
        );
        assert_eq!(
            CoinStatus::from_arg(CoinStatus::Spending.to_arg()),
            Some(CoinStatus::Spending)
        );
        assert_eq!(
            CoinStatus::from_arg(CoinStatus::Spent.to_arg()),
            Some(CoinStatus::Spent)
        );
    }

    #[test]
    fn graph_item_string_form() {
        let txid = bitcoin::Txid::from_str(
            "0b7d8cd7a8ff5e2f4a9c1f6c3ad4b26a5e9d3c2b1a0f9e8d7c6b5a4938271605",
        )
        .unwrap();
        let outpoint = bitcoin::OutPoint { txid, vout: 3 };
        let cases = [
            (GraphItem::Tx(txid), format!("tx:{txid}")),
            (GraphItem::OutputLeaf(outpoint), format!("out:{txid}:3")),
            (GraphItem::InputLeaf(outpoint), format!("in:{txid}:3")),
        ];
        for (item, string) in cases {
            assert_eq!(item.to_string(), string);
            assert_eq!(GraphItem::from_str(&string), Ok(item));
            let json = serde_json::to_string(&item).unwrap();
            assert_eq!(json, format!("\"{string}\""));
            assert_eq!(serde_json::from_str::<GraphItem>(&json).unwrap(), item);
        }

        for invalid in [
            txid.to_string(),
            format!("foo:{txid}"),
            format!("tx:{txid}:0"),
            format!("out:{txid}"),
        ] {
            assert!(GraphItem::from_str(&invalid).is_err());
        }

        let entry = GraphLayoutEntry {
            item: GraphItem::Tx(txid),
            position: Some((12.0, -24.5)),
            input_order: Some(vec![1, 0]),
            output_order: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert_eq!(
            serde_json::from_str::<GraphLayoutEntry>(&json).unwrap(),
            entry
        );

        let bare: GraphLayoutEntry =
            serde_json::from_str(&format!("{{\"item\": \"tx:{txid}\"}}")).unwrap();
        assert_eq!(
            bare,
            GraphLayoutEntry {
                item: GraphItem::Tx(txid),
                position: None,
                input_order: None,
                output_order: None,
            }
        );
    }
}
