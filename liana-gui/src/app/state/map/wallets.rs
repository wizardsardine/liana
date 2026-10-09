use std::collections::HashMap;

use iced::Vector;
use liana::miniscript::bitcoin::Network;
use lianad::{
    commands::{GraphItem, GraphLayoutEntry, GraphWallet},
    datadir::DataDirectory,
    offline::{OfflineError, WalletDb},
};

use crate::{
    app::{
        error::Error,
        settings::{LianaSettings, SettingsError, WalletId},
        state::map::{
            external::{external_wallets, load_external, load_layout, ExternalWallet},
            graph::WalletTxs,
            offsets::WalletLayout,
            MapWallet,
        },
    },
    daemon::{
        history_txs, label_items,
        model::{Coin, HistoryTransaction, LabelItem},
        set_labels, wallet_txids,
    },
    dir::NetworkDirectory,
};

/// The wallet some map data belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum WalletKey {
    /// The wallet the map is opened from.
    Current,
    Other(WalletId),
    /// An imported wallet, by its id.
    External(String),
}

impl WalletKey {
    /// The `graph_wallets` row of the wallet. `current` cannot be a Liana wallet id.
    pub fn row(&self) -> String {
        match self {
            Self::Current => "current".to_string(),
            Self::Other(id) => id.to_string(),
            Self::External(id) => format!("external:{id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletStatus {
    Available,
    /// Its database is of another version, the wallet must be opened once to update it.
    Outdated,
    Unavailable,
}

impl WalletStatus {
    pub fn check(datadir: &DataDirectory, network: Network) -> Self {
        match WalletDb::open(datadir, network) {
            Ok(_) => Self::Available,
            Err(OfflineError::Version { .. }) => Self::Outdated,
            Err(_) => Self::Unavailable,
        }
    }
}

/// A local wallet of the network other than the current one.
#[derive(Debug, Clone)]
pub struct OtherWallet {
    pub id: WalletId,
    pub name: String,
    pub checksum: String,
    pub datadir: DataDirectory,
    pub status: WalletStatus,
}

/// Where the edits of a wallet added to the map are written.
#[derive(Debug, Clone)]
pub enum WalletStore {
    Other(OtherWallet),
    External(ExternalWallet),
}

impl WalletStore {
    pub fn name(&self) -> &str {
        match self {
            Self::Other(other) => &other.name,
            Self::External(external) => &external.name,
        }
    }

    /// The checksum the wallet color is picked from.
    pub fn checksum(&self) -> &str {
        match self {
            Self::Other(other) => &other.checksum,
            Self::External(external) => external.checksum(),
        }
    }
}

/// The wallets the wallets modal lists.
#[derive(Debug)]
pub struct ListedWallets {
    pub others: Vec<OtherWallet>,
    pub externals: Vec<ExternalWallet>,
}

#[derive(Debug)]
pub struct WalletData {
    pub key: WalletKey,
    pub name: String,
    pub checksum: String,
    pub txs: Vec<HistoryTransaction>,
    pub coins: Vec<Coin>,
    pub layout: Vec<GraphLayoutEntry>,
    /// Time of the wallet's last sync.
    pub last_sync: Option<u32>,
}

/// The wallets of the settings file other than `current`, Liana Connect ones left out.
pub fn other_wallets(
    network_dir: &NetworkDirectory,
    network: Network,
    current: &WalletId,
) -> Result<Vec<OtherWallet>, SettingsError> {
    Ok(LianaSettings::from_file(network_dir)?
        .wallets
        .into_iter()
        .filter(|settings| settings.remote_backend_auth.is_none())
        .map(|settings| (settings.wallet_id(), settings))
        .filter(|(id, _)| id != current)
        .map(|(id, settings)| {
            let datadir = network_dir.lianad_data_directory(&id);
            OtherWallet {
                status: WalletStatus::check(&datadir, network),
                name: settings
                    .alias
                    .filter(|alias| !alias.is_empty())
                    .unwrap_or(settings.name),
                checksum: settings.descriptor_checksum,
                id,
                datadir,
            }
        })
        .collect())
}

/// Reads `wallet` from its database, blocking. The data is as fresh as its last sync.
pub fn load_wallet(wallet: &OtherWallet, network: Network) -> Result<WalletData, OfflineError> {
    let mut db = WalletDb::open(&wallet.datadir, network)?;
    let coins = db.list_coins();
    let mut txs = history_txs(db.list_transactions(&wallet_txids(&coins)), &coins, network);
    let labels = db.labels(&label_items(&txs));
    set_labels(&mut txs, labels);
    Ok(WalletData {
        key: WalletKey::Other(wallet.id.clone()),
        name: wallet.name.clone(),
        checksum: wallet.checksum.clone(),
        txs,
        coins,
        layout: db.graph_layout(),
        last_sync: db.last_poll_timestamp(),
    })
}

/// Writes layout entries of `wallet` to its database, blocking.
pub fn save_wallet_layout(
    wallet: &OtherWallet,
    network: Network,
    set: &[GraphLayoutEntry],
    remove: &[GraphItem],
) -> Result<(), OfflineError> {
    WalletDb::open(&wallet.datadir, network)?.update_graph_layout(set, remove);
    Ok(())
}

/// Writes labels of `wallet` to its database, blocking.
pub fn save_wallet_labels(
    wallet: &OtherWallet,
    network: Network,
    items: &HashMap<LabelItem, Option<String>>,
) -> Result<(), OfflineError> {
    WalletDb::open(&wallet.datadir, network)?.update_labels(items);
    Ok(())
}

fn stored_offset(row: &GraphWallet) -> Option<Vector> {
    row.offset.map(|(x, y)| Vector::new(x as f32, y as f32))
}

/// The wallets selected in `rows` that can be read, each with its stored offset, blocking.
pub fn load_selected(
    network_dir: &NetworkDirectory,
    network: Network,
    current: &WalletId,
    rows: &[GraphWallet],
) -> Result<Vec<MapWallet>, Error> {
    let mut wallets = Vec::new();
    if !rows.iter().any(|row| row.selected) {
        return Ok(wallets);
    }
    let selected = |key: &WalletKey| {
        let wallet = key.row();
        rows.iter().find(|row| row.selected && row.wallet == wallet)
    };
    for other in other_wallets(network_dir, network, current)? {
        let Some(row) = selected(&WalletKey::Other(other.id.clone())) else {
            continue;
        };
        if other.status != WalletStatus::Available {
            continue;
        }
        let data = load_wallet(&other, network)?;
        wallets.push(MapWallet {
            txs: WalletTxs {
                key: data.key,
                checksum: data.checksum,
                txs: data.txs,
                coins: data.coins,
            },
            layout: WalletLayout {
                entries: data.layout,
                offset: stored_offset(row),
            },
            store: Some(WalletStore::Other(other)),
        });
    }
    for external in external_wallets(network_dir) {
        let key = WalletKey::External(external.id.clone());
        let Some(row) = selected(&key) else {
            continue;
        };
        let history = load_external(&external, network_dir, network)?;
        wallets.push(MapWallet {
            txs: WalletTxs {
                key,
                checksum: external.checksum().to_string(),
                txs: history.txs,
                coins: history.coins,
            },
            layout: WalletLayout {
                entries: load_layout(&external.dir(network_dir))?,
                offset: stored_offset(row),
            },
            store: Some(WalletStore::External(external)),
        });
    }
    Ok(wallets)
}

#[cfg(test)]
mod tests {
    use std::{env, fs, path::PathBuf, process};

    use liana::miniscript::bitcoin::Network;
    use lianad::offline::OfflineError;

    use crate::{
        app::{
            settings::{SettingsError, WalletId, SETTINGS_FILE_NAME},
            state::map::wallets::{
                load_wallet, other_wallets, OtherWallet, WalletKey, WalletStatus,
            },
        },
        dir::NetworkDirectory,
    };

    const SETTINGS: &str = r#"{
        "wallets": [
            {
                "name": "Liana-current",
                "alias": "Current",
                "descriptor_checksum": "current",
                "pinned_at": 1700000000,
                "remote_backend_auth": null
            },
            {
                "name": "Liana-aliased",
                "alias": "Savings",
                "descriptor_checksum": "aliased",
                "pinned_at": 1700000001,
                "remote_backend_auth": null
            },
            {
                "name": "Liana-unnamed",
                "alias": "",
                "descriptor_checksum": "unnamed",
                "pinned_at": 1700000002,
                "remote_backend_auth": null
            },
            {
                "name": "Liana-legacy",
                "alias": null,
                "descriptor_checksum": "legacy",
                "pinned_at": null,
                "remote_backend_auth": null
            },
            {
                "name": "Liana-remote",
                "alias": "Shared",
                "descriptor_checksum": "remote",
                "pinned_at": 1700000003,
                "remote_backend_auth": {
                    "email": "user@example.com",
                    "wallet_id": "c0ffee",
                    "refresh_token": null
                }
            }
        ]
    }"#;

    fn network_dir(test: &str) -> NetworkDirectory {
        let path = env::temp_dir().join(format!("liana-gui-map-wallets-{}-{test}", process::id()));
        fs::create_dir_all(&path).unwrap();
        NetworkDirectory::new(path)
    }

    fn summary(wallet: &OtherWallet) -> (String, String, String, PathBuf, WalletStatus) {
        (
            wallet.id.to_string(),
            wallet.name.clone(),
            wallet.checksum.clone(),
            wallet.datadir.path().to_path_buf(),
            wallet.status,
        )
    }

    #[test]
    fn other_wallets_skip_current_and_remote() {
        let dir = network_dir("discovery");
        fs::write(dir.path().join(SETTINGS_FILE_NAME), SETTINGS).unwrap();
        let current = WalletId::new("current".to_string(), Some(1700000000));

        let wallets = other_wallets(&dir, Network::Bitcoin, &current).unwrap();
        assert_eq!(
            wallets.iter().map(summary).collect::<Vec<_>>(),
            vec![
                (
                    "aliased-1700000001".to_string(),
                    "Savings".to_string(),
                    "aliased".to_string(),
                    dir.path().join("data").join("aliased-1700000001"),
                    WalletStatus::Unavailable,
                ),
                (
                    "unnamed-1700000002".to_string(),
                    "Liana-unnamed".to_string(),
                    "unnamed".to_string(),
                    dir.path().join("data").join("unnamed-1700000002"),
                    WalletStatus::Unavailable,
                ),
                (
                    "legacy".to_string(),
                    "Liana-legacy".to_string(),
                    "legacy".to_string(),
                    dir.path().to_path_buf(),
                    WalletStatus::Unavailable,
                ),
            ]
        );

        // Another wallet of the same descriptor is still listed.
        let other_pin = WalletId::new("current".to_string(), Some(1));
        let wallets = other_wallets(&dir, Network::Bitcoin, &other_pin).unwrap();
        assert_eq!(
            wallets.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
            vec!["Current", "Savings", "Liana-unnamed", "Liana-legacy"]
        );

        fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn other_wallets_without_settings_fails() {
        let dir = network_dir("no-settings");
        let current = WalletId::new("current".to_string(), None);
        assert!(matches!(
            other_wallets(&dir, Network::Bitcoin, &current),
            Err(SettingsError::NotFound)
        ));
        fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn load_wallet_without_database_fails() {
        let dir = network_dir("no-database");
        fs::write(dir.path().join(SETTINGS_FILE_NAME), SETTINGS).unwrap();
        let current = WalletId::new("current".to_string(), Some(1700000000));
        let wallets = other_wallets(&dir, Network::Bitcoin, &current).unwrap();
        assert!(matches!(
            load_wallet(&wallets[0], Network::Bitcoin),
            Err(OfflineError::NotFound(path)) if path == wallets[0].datadir.sqlite_db_file_path()
        ));
        fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn wallet_rows() {
        assert_eq!(WalletKey::Current.row(), "current");
        let other = WalletKey::Other(WalletId::new("aliased".to_string(), Some(1700000001)));
        assert_eq!(other.row(), "aliased-1700000001");
        let external = WalletKey::External("q8n5v3ka-1700000002".to_string());
        assert_eq!(external.row(), "external:q8n5v3ka-1700000002");
    }
}
