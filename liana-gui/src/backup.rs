use chrono::{Duration, Utc};
use liana::miniscript::{
    self,
    bitcoin::{bip32::Fingerprint, Network, Txid},
};
use lianad::bip329;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::{Debug, Display},
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    app::{settings::Settings, wallet::Wallet, Config},
    daemon::{model::HistoryTransaction, Daemon, DaemonBackend, DaemonError},
    installer::{
        extract_daemon_config, extract_local_gui_settings, extract_remote_gui_settings, Context,
        RemoteBackend,
    },
    lianalite::client::backend::api::DEFAULT_LIMIT,
};

const CONFIG_KEY: &str = "config";
const SETTINGS_KEY: &str = "settings";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("cannot fail")
        .as_secs()
}

#[derive(Serialize, Deserialize)]
pub struct Backup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub accounts: Vec<Account>,
    pub network: Network,
    pub date: u64,
    /// App proprietary metadata (settings, configuration, etc..)
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    pub proprietary: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum Error {
    DescriptorMissing,
    NotSingleWallet,
    Json,
    SettingsFromFile,
    Daemon(String),
    TxTimeMissing,
}

impl From<DaemonError> for Error {
    fn from(value: DaemonError) -> Self {
        Error::Daemon(value.to_string())
    }
}

impl Display for Backup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        write!(f, "{str}")
    }
}

impl Debug for Backup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = serde_json::to_string_pretty(self).map_err(|_| std::fmt::Error)?;
        write!(f, "{str}")
    }
}

impl Backup {
    /// Create a Backup from the Installer context
    ///
    /// # Arguments
    /// * `ctx` - the installer context
    /// * `timestamp` - wether to record the current timestamp as wallet creation time
    ///   (we should want to set timestamp = false for  a wallet import for instance)
    pub async fn from_installer(ctx: Context, timestamp: bool) -> Result<Self, Error> {
        let descriptor = ctx
            .descriptor
            .clone()
            .ok_or(Error::DescriptorMissing)?
            .to_string();

        let now = now();

        let mut account = Account::new(descriptor);

        let mut proprietary = serde_json::Map::new();

        let config = extract_daemon_config(&ctx);
        if let Ok(config) = serde_json::to_value(config) {
            proprietary.insert(CONFIG_KEY.to_string(), config);
        }
        let settings = if ctx.bitcoin_backend.is_some() {
            Some(extract_local_gui_settings(&ctx))
        } else {
            match &ctx.remote_backend {
                RemoteBackend::WithWallet(backend) => {
                    Some(extract_remote_gui_settings(&ctx, backend).await)
                }
                _ => None,
            }
        };

        let name = if let Some(settings) = settings {
            assert_eq!(settings.wallets.len(), 1);
            if settings.wallets.len() != 1 {
                return Err(Error::NotSingleWallet);
            }
            let settings = settings.wallets.first().expect("only one wallet");
            let name = settings.name.clone();
            if let Ok(settings) = serde_json::to_value(settings) {
                proprietary.insert(SETTINGS_KEY.to_string(), settings);
            }
            Some(name)
        } else {
            None
        };

        ctx.keys.iter().for_each(|(k, s)| {
            account.keys.insert(*k, s.to_backup());
        });

        account.proprietary = proprietary;
        account.name = name.clone();
        if timestamp {
            account.timestamp = Some(now);
        }

        Ok(Backup {
            name,
            accounts: vec![account],
            network: ctx.network,
            proprietary: serde_json::Map::new(),
            date: now,
        })
    }

    /// Create a Backup from the Liana App context
    pub async fn from_app(
        datadir: PathBuf,
        network: Network,
        config: Arc<Config>,
        wallet: Arc<Wallet>,
        daemon: Arc<dyn Daemon + Sync + Send>,
    ) -> Result<Self, Error> {
        let mut proprietary = serde_json::Map::new();
        let name = wallet.name.clone();
        let descriptor = wallet.main_descriptor.to_string();
        let keys = wallet.keys();

        let settings =
            Settings::from_file(datadir, network).map_err(|_| Error::SettingsFromFile)?;
        if settings.wallets.len() == 1 {
            if let Ok(settings) = serde_json::to_value(settings.wallets[0].clone()) {
                proprietary.insert(SETTINGS_KEY.to_string(), settings);
            }
        }

        if let Ok(config) = serde_json::to_value((*config).clone()) {
            proprietary.insert(CONFIG_KEY.to_string(), config);
        }

        let mut account = Account::new(descriptor);
        account.proprietary = proprietary;
        account.name = Some(name.clone());
        let info = daemon.get_info().await?;
        account.timestamp = Some(info.timestamp as u64);
        account.change_index = Some(info.change_index);
        account.receive_index = Some(info.receive_index);
        for (fg, setting) in keys {
            account.keys.insert(fg, setting.to_backup());
        }

        account.labels = Some(daemon.get_labels_bip329(0, u32::MAX).await?);
        account.transactions = get_transactions(&daemon)
            .await?
            .into_iter()
            .map(|tx| miniscript::bitcoin::consensus::encode::serialize_hex(&tx.tx))
            .collect();
        account.psbts = daemon
            .list_spend_transactions(None)
            .await?
            .into_iter()
            .map(|tx| tx.psbt.serialize_hex())
            .collect();

        Ok(Backup {
            name: Some(name),
            accounts: vec![account],
            network,
            proprietary: serde_json::Map::new(),
            date: now(),
        })
    }

    fn account(&self) -> Result<&Account, Error> {
        if self.accounts.len() != 1 {
            Err(Error::NotSingleWallet)
        } else {
            Ok(self.accounts.first().expect("single account"))
        }
    }

    pub fn config(&self) -> Result<Option<Config>, Error> {
        let account = self.account()?;
        if let Some(config) = account.proprietary.get(CONFIG_KEY) {
            let config: Config = serde_json::from_value(config.clone()).map_err(|_| Error::Json)?;
            Ok(Some(config))
        } else {
            Ok(None)
        }
    }

    pub fn settings(&self) -> Result<Option<Settings>, Error> {
        let account = self.account()?;
        if let Some(settings) = account.proprietary.get(SETTINGS_KEY) {
            let settings: Settings =
                serde_json::from_value(settings.clone()).map_err(|_| Error::Json)?;
            Ok(Some(settings))
        } else {
            Ok(None)
        }
    }
}

async fn get_transactions(
    daemon: &Arc<dyn Daemon + Sync + Send>,
) -> Result<Vec<HistoryTransaction>, Error> {
    let max = match daemon.backend() {
        DaemonBackend::RemoteBackend => DEFAULT_LIMIT as u64,
        _ => u32::MAX as u64,
    };

    // look 2 hour forward
    // https://github.com/bitcoin/bitcoin/blob/62bd61de110b057cbfd6e31e4d0b727d93119c72/src/chain.h#L29
    let mut end = ((Utc::now() + Duration::hours(2)).timestamp()) as u32;

    // store txs in a map to avoid duplicates
    let mut map = HashMap::<Txid, HistoryTransaction>::new();
    let mut limit = max;

    loop {
        let history_txs = daemon.list_history_txs(0, end, limit).await?;
        // all txs have been fetched
        if history_txs.is_empty() {
            return Ok(Vec::new());
        }

        if history_txs.len() == limit as usize {
            let first = if let Some(t) = history_txs.first().expect("checked").time {
                t
            } else {
                return Err(Error::TxTimeMissing);
            };

            let last = if let Some(t) = history_txs.last().expect("checked").time {
                t
            } else {
                return Err(Error::TxTimeMissing);
            };

            // limit too low, all tx are in the same timestamp
            // we must increase limit and retry
            if first == last {
                limit += DEFAULT_LIMIT as u64;
                continue;
            } else {
                // add txs to map
                for tx in history_txs {
                    let txid = tx.txid;
                    map.insert(txid, tx);
                }
                limit = max;
                end = first.min(last);
                continue;
            }
        } else
        /* history_txs.len() < limit */
        {
            // add txs to map
            for tx in history_txs {
                let txid = tx.txid;
                map.insert(txid, tx);
            }
            break;
        }
    }
    let vec: Vec<_> = map.into_values().collect();
    Ok(vec)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Account {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub descriptor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receive_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<Fingerprint, Key>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<bip329::Labels>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transactions: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub psbts: Vec<String>,
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    pub proprietary: serde_json::Map<String, serde_json::Value>,
}

impl Account {
    pub fn new(descriptor: String) -> Self {
        Self {
            name: None,
            descriptor,
            receive_index: None,
            change_index: None,
            timestamp: None,
            keys: BTreeMap::new(),
            labels: None,
            transactions: Vec::new(),
            psbts: Vec::new(),
            proprietary: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Key {
    pub key: Fingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<KeyRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_type: Option<KeyType>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum KeyRole {
    /// Key to be used in normal spending condition
    Main,
    /// Key that will be used for recover in case loss of main key(s)
    Recovery,
    /// Key that wil inherit coins if main user disapear
    Inheritance,
    /// Key that will cosign a spend in order to enforce some policy
    Cosigning,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum KeyType {
    /// Main user
    Internal,
    /// Heirs or friends
    External,
    /// Service the user pay for
    ThirdParty,
}
