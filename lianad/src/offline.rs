//! Access to a wallet database without running its daemon.
//!
//! The data is as fresh as the wallet's last sync. The database is never migrated, since a
//! migration may need a Bitcoin backend: a database of another version is refused.

use crate::{
    commands::{
        list_coins_entries, list_transactions_info, GraphItem, GraphLayoutEntry, LabelItem,
        ListCoinsEntry, TransactionInfo,
    },
    database::{
        sqlite::{SqliteConn, SqliteDb, SqliteDbError, DB_VERSION},
        DatabaseConnection,
    },
    datadir::DataDirectory,
};

use liana::descriptors::LianaDescriptor;

use std::{
    collections::{HashMap, HashSet},
    error, fmt, path,
};

use miniscript::bitcoin::{self, secp256k1};

#[derive(Debug)]
pub enum OfflineError {
    NotFound(path::PathBuf),
    Version {
        found: i64,
        expected: i64,
    },
    Network {
        found: bitcoin::Network,
        expected: bitcoin::Network,
    },
    Database(SqliteDbError),
}

impl fmt::Display for OfflineError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NotFound(p) => write!(f, "No wallet database at '{}'.", p.display()),
            Self::Version { found, expected } => write!(
                f,
                "Wallet database version is '{found}', this version of Liana expects '{expected}'."
            ),
            Self::Network { found, expected } => write!(
                f,
                "Wallet database is for network '{found}', expected '{expected}'."
            ),
            Self::Database(e) => write!(f, "Error opening wallet database: '{e}'."),
        }
    }
}

impl error::Error for OfflineError {}

impl From<SqliteDbError> for OfflineError {
    fn from(e: SqliteDbError) -> Self {
        match e {
            SqliteDbError::FileNotFound(p) => Self::NotFound(p),
            e => Self::Database(e),
        }
    }
}

/// The database of a wallet whose daemon is not running.
pub struct WalletDb {
    conn: SqliteConn,
    network: bitcoin::Network,
    secp: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
}

impl WalletDb {
    pub fn open(data_dir: &DataDirectory, network: bitcoin::Network) -> Result<Self, OfflineError> {
        let secp = secp256k1::Secp256k1::verification_only();
        let mut conn = SqliteDb::new(data_dir.sqlite_db_file_path(), None, &secp)?.connection()?;
        let found = conn.db_version();
        if found != DB_VERSION {
            return Err(OfflineError::Version {
                found,
                expected: DB_VERSION,
            });
        }
        let found = conn.db_tip().network;
        if found != network {
            return Err(OfflineError::Network {
                found,
                expected: network,
            });
        }
        Ok(Self {
            conn,
            network,
            secp,
        })
    }

    pub fn main_descriptor(&mut self) -> LianaDescriptor {
        self.conn.db_wallet().main_descriptor
    }

    /// Time of the wallet's last sync with the chain, if any.
    pub fn last_poll_timestamp(&mut self) -> Option<u32> {
        self.conn.db_wallet().last_poll_timestamp
    }

    /// All the coins of the wallet, spent ones included.
    pub fn list_coins(&mut self) -> Vec<ListCoinsEntry> {
        let main_descriptor = self.main_descriptor();
        list_coins_entries(
            &mut self.conn,
            &main_descriptor,
            self.network,
            &self.secp,
            &[],
            &[],
        )
    }

    pub fn list_transactions(&mut self, txids: &[bitcoin::Txid]) -> Vec<TransactionInfo> {
        list_transactions_info(&mut self.conn, txids)
    }

    pub fn labels(&mut self, items: &HashSet<LabelItem>) -> HashMap<String, String> {
        DatabaseConnection::labels(&mut self.conn, items)
    }

    pub fn update_labels(&mut self, items: &HashMap<LabelItem, Option<String>>) {
        self.conn.update_labels(items)
    }

    pub fn graph_layout(&mut self) -> Vec<GraphLayoutEntry> {
        DatabaseConnection::graph_layout(&mut self.conn)
    }

    pub fn update_graph_layout(&mut self, set: &[GraphLayoutEntry], remove: &[GraphItem]) {
        self.conn.update_graph_layout(set, remove)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        commands::{GraphItem, GraphLayoutEntry, LCSpendInfo, LabelItem},
        database::{
            sqlite::{FreshDbOptions, SqliteDb, DB_VERSION},
            Coin,
        },
        datadir::DataDirectory,
        offline::{OfflineError, WalletDb},
        testutils::{dummy_descriptor, tmp_dir},
    };

    use std::{
        collections::{HashMap, HashSet},
        fs,
    };

    use miniscript::bitcoin::{
        self, absolute, bip32, secp256k1, transaction, Amount, OutPoint, ScriptBuf, Transaction,
        TxIn, TxOut,
    };

    const NETWORK: bitcoin::Network = bitcoin::Network::Bitcoin;

    fn fresh_data_dir() -> DataDirectory {
        let data_dir = DataDirectory::new(tmp_dir());
        fs::create_dir_all(data_dir.path()).unwrap();
        let secp = secp256k1::Secp256k1::verification_only();
        let options = FreshDbOptions::new(NETWORK, dummy_descriptor(10_000));
        SqliteDb::new(data_dir.sqlite_db_file_path(), Some(options), &secp).unwrap();
        data_dir
    }

    fn set_db_version(data_dir: &DataDirectory, version: i64) {
        rusqlite::Connection::open(data_dir.sqlite_db_file_path())
            .unwrap()
            .execute(
                "UPDATE version SET version = (?1)",
                rusqlite::params![version],
            )
            .unwrap();
    }

    fn dummy_tx(lock_height: u32, input: OutPoint) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::from_height(lock_height).unwrap(),
            input: vec![TxIn {
                previous_output: input,
                ..TxIn::default()
            }],
            output: vec![
                TxOut::minimal_non_dust(ScriptBuf::default()),
                TxOut::minimal_non_dust(ScriptBuf::default()),
            ],
        }
    }

    #[test]
    fn open_fresh_db() {
        let data_dir = fresh_data_dir();
        let mut db = WalletDb::open(&data_dir, NETWORK).unwrap();
        assert_eq!(db.main_descriptor(), dummy_descriptor(10_000));
        assert_eq!(db.last_poll_timestamp(), None);
        assert!(db.list_coins().is_empty());
        assert!(db.graph_layout().is_empty());
        fs::remove_dir_all(data_dir.path()).unwrap();
    }

    #[test]
    fn open_refuses_other_version() {
        let data_dir = fresh_data_dir();
        for version in [DB_VERSION + 1, DB_VERSION - 1] {
            set_db_version(&data_dir, version);
            match WalletDb::open(&data_dir, NETWORK) {
                Err(OfflineError::Version { found, expected }) => {
                    assert_eq!(found, version);
                    assert_eq!(expected, DB_VERSION);
                }
                _ => panic!("database of version {} must be refused", version),
            }
        }
        // The older database is not migrated.
        let version: i64 = rusqlite::Connection::open(data_dir.sqlite_db_file_path())
            .unwrap()
            .query_row("SELECT version FROM version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, DB_VERSION - 1);
        fs::remove_dir_all(data_dir.path()).unwrap();
    }

    #[test]
    fn open_refuses_other_network_or_missing_db() {
        let data_dir = fresh_data_dir();
        match WalletDb::open(&data_dir, bitcoin::Network::Testnet) {
            Err(OfflineError::Network { found, expected }) => {
                assert_eq!(found, NETWORK);
                assert_eq!(expected, bitcoin::Network::Testnet);
            }
            _ => panic!("database of another network must be refused"),
        }
        fs::remove_dir_all(data_dir.path()).unwrap();
        match WalletDb::open(&data_dir, NETWORK) {
            Err(OfflineError::NotFound(p)) => assert_eq!(p, data_dir.sqlite_db_file_path()),
            _ => panic!("missing database must be refused"),
        }
    }

    #[test]
    fn wallet_db_round_trip() {
        let data_dir = fresh_data_dir();
        let deposit = dummy_tx(1, OutPoint::null());
        let deposit_txid = deposit.compute_txid();
        let received = OutPoint::new(deposit_txid, 0);
        let spent = OutPoint::new(deposit_txid, 1);
        let spend = dummy_tx(2, spent);
        let spend_txid = spend.compute_txid();
        {
            let secp = secp256k1::Secp256k1::verification_only();
            let mut conn = SqliteDb::new(data_dir.sqlite_db_file_path(), None, &secp)
                .unwrap()
                .connection()
                .unwrap();
            conn.new_txs(&[deposit.clone(), spend.clone()]);
            let coins = [
                Coin {
                    outpoint: received,
                    is_immature: false,
                    block_info: None,
                    amount: Amount::from_sat(10_000),
                    derivation_index: bip32::ChildNumber::from_normal_idx(3).unwrap(),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                },
                Coin {
                    outpoint: spent,
                    is_immature: false,
                    block_info: None,
                    amount: Amount::from_sat(20_000),
                    derivation_index: bip32::ChildNumber::from_normal_idx(5).unwrap(),
                    is_change: true,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                },
            ];
            conn.new_unspent_coins(&coins);
            conn.confirm_coins(&[(received, 100, 1_700_000_000), (spent, 100, 1_700_000_000)]);
            conn.spend_coins(&[(spent, spend_txid)]);
            conn.confirm_spend(&[(spent, spend_txid, 105, 1_700_003_000)]);
        }

        let mut db = WalletDb::open(&data_dir, NETWORK).unwrap();
        let secp = secp256k1::Secp256k1::verification_only();
        let descriptor = dummy_descriptor(10_000);

        let mut coins = db.list_coins();
        coins.sort_by_key(|c| c.outpoint.vout);
        assert_eq!(coins.len(), 2);
        assert_eq!(coins[0].outpoint, received);
        assert_eq!(coins[0].amount, Amount::from_sat(10_000));
        assert_eq!(coins[0].block_height, Some(100));
        assert!(coins[0].spend_info.is_none());
        assert!(!coins[0].is_change);
        assert_eq!(
            coins[0].address,
            descriptor
                .receive_descriptor()
                .derive(bip32::ChildNumber::from_normal_idx(3).unwrap(), &secp)
                .address(NETWORK)
        );
        assert_eq!(coins[1].outpoint, spent);
        assert!(coins[1].is_change);
        assert!(matches!(
            coins[1].spend_info,
            Some(LCSpendInfo { txid, height: Some(105) }) if txid == spend_txid
        ));
        assert_eq!(
            coins[1].address,
            descriptor
                .change_descriptor()
                .derive(bip32::ChildNumber::from_normal_idx(5).unwrap(), &secp)
                .address(NETWORK)
        );

        let mut txs = db.list_transactions(&[deposit_txid, spend_txid]);
        txs.sort_by_key(|t| t.height);
        assert_eq!(txs.len(), 2);
        assert_eq!(txs[0].tx, deposit);
        assert_eq!(txs[0].height, Some(100));
        assert_eq!(txs[0].time, Some(1_700_000_000));
        assert_eq!(txs[1].tx, spend);
        assert_eq!(txs[1].height, Some(105));

        let tx_item = LabelItem::Txid(deposit_txid);
        let coin_item = LabelItem::OutPoint(received);
        db.update_labels(&HashMap::from([
            (tx_item.clone(), Some("deposit".to_string())),
            (coin_item.clone(), Some("received".to_string())),
        ]));
        let entry = GraphLayoutEntry {
            item: GraphItem::Tx(deposit_txid),
            position: Some((12.0, -4.5)),
            input_order: None,
            output_order: Some(vec![1, 0]),
        };
        db.update_graph_layout(&[entry.clone()], &[]);
        drop(db);

        let mut db = WalletDb::open(&data_dir, NETWORK).unwrap();
        let items = HashSet::from([tx_item.clone(), coin_item.clone()]);
        assert_eq!(
            db.labels(&items),
            HashMap::from([
                (deposit_txid.to_string(), "deposit".to_string()),
                (received.to_string(), "received".to_string()),
            ])
        );
        assert_eq!(db.graph_layout(), vec![entry.clone()]);

        db.update_labels(&HashMap::from([(tx_item, None)]));
        db.update_graph_layout(&[], &[entry.item]);
        assert_eq!(
            db.labels(&items),
            HashMap::from([(received.to_string(), "received".to_string())])
        );
        assert!(db.graph_layout().is_empty());
        fs::remove_dir_all(data_dir.path()).unwrap();
    }
}
