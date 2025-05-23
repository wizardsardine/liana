use chrono::{Duration, Utc};
use liana::miniscript::{
    self,
    bitcoin::{bip32::Fingerprint, Network, Txid},
};
use lianad::{
    bip329,
    commands::{CoinStatus, ListCoinsEntry},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::{Debug, Display},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    app::{
        settings::{Settings, WalletSettings},
        wallet::{wallet_name, Wallet},
        Config,
    },
    daemon::{model::HistoryTransaction, Daemon, DaemonBackend, DaemonError},
    dir::LianaDirectory,
    export::Progress,
    installer::Context,
    services::connect::client::backend::api::DEFAULT_LIMIT,
    VERSION,
};

const CONFIG_KEY: &str = "config";
const SETTINGS_KEY: &str = "settings";
const LIANA_VERSION_KEY: &str = "liana_version";

pub fn liana_version() -> String {
    format!("{}.{}.{}", VERSION.major, VERSION.minor, VERSION.patch)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("cannot fail")
        .as_secs()
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Backup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub accounts: Vec<Account>,
    pub network: Network,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<u64>,
    /// App proprietary metadata (settings, configuration, etc..)
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub proprietary: serde_json::Map<String, serde_json::Value>,
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_version() -> u32 {
    0
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

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::DescriptorMissing => write!(f, "Backup: descriptor missing"),
            Error::NotSingleWallet => write!(f, "Backup: Zero or several wallets"),
            Error::Json => write!(f, "Backup: json error"),
            Error::SettingsFromFile => write!(f, "Backup: fail to parse setting from file"),
            Error::Daemon(e) => write!(f, "Backup daemon error: {e}"),
            Error::TxTimeMissing => write!(f, "Backup: transaction block height missing"),
        }
    }
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
    pub async fn from_installer_descriptor_step(ctx: Context) -> Result<Self, Error> {
        let descriptor = ctx.descriptor.clone().ok_or(Error::DescriptorMissing)?;

        let now = now();
        let name = Some(wallet_name(&descriptor));

        let mut account = Account::new(descriptor.to_string());
        account.name = name.clone();
        account.timestamp = Some(now);
        account
            .proprietary
            .insert(LIANA_VERSION_KEY.to_string(), liana_version().into());

        ctx.keys.iter().for_each(|(k, s)| {
            account.keys.insert(*k, s.to_backup());
        });

        Ok(Backup {
            name,
            alias: None,
            accounts: vec![account],
            network: ctx.network,
            proprietary: serde_json::Map::new(),
            date: Some(now),
            version: 0,
        })
    }

    /// Create a Backup from the Liana App context
    pub async fn from_app(
        datadir: LianaDirectory,
        network: Network,
        config: Arc<Config>,
        wallet: Arc<Wallet>,
        daemon: Arc<dyn Daemon + Sync + Send>,
        sender: &UnboundedSender<Progress>,
    ) -> Result<Self, Error> {
        let mut proprietary = serde_json::Map::new();
        proprietary.insert(LIANA_VERSION_KEY.to_string(), liana_version().into());

        let name = wallet.name.clone();
        let descriptor = wallet.main_descriptor.to_string();
        let keys = wallet.keys();

        let network_dir = datadir.network_directory(network);
        let mut wallet_alias = wallet.alias.clone();
        if let Some(settings) =
            WalletSettings::from_file(&network_dir, |settings| wallet.id() == settings.wallet_id())
                .map_err(|_| Error::SettingsFromFile)?
        {
            if let Ok(settings) = serde_json::to_value(&settings) {
                proprietary.insert(SETTINGS_KEY.to_string(), settings);
            }
            wallet_alias = settings.alias;
        };

        if let Ok(config) = serde_json::to_value((*config).clone()) {
            proprietary.insert(CONFIG_KEY.to_string(), config);
        }

        let info = daemon.get_info().await?;

        let _ = sender.send(Progress::Progress(20.0));

        let mut account = Account::new(descriptor);

        account.chain_tip = Some(ChainTip {
            block_height: info.block_height,
            block_hash: None,
        });
        account.proprietary = proprietary;
        account.name = Some(name.clone());
        account.timestamp = Some(info.timestamp as u64);
        account.change_index = Some(info.change_index);
        account.receive_index = Some(info.receive_index);
        for (fg, setting) in keys {
            account.keys.insert(fg, setting.to_backup());
        }

        const MAX_LABEL_BIP329: u32 = 100;

        let labels = {
            let mut buff = Vec::new();
            let mut start = 0;
            loop {
                let mut fetched = daemon.get_labels_bip329(start, 100).await?.into_vec();

                if fetched.len() < MAX_LABEL_BIP329 as usize {
                    buff.append(&mut fetched);
                    break;
                } else {
                    buff.append(&mut fetched);
                    start += MAX_LABEL_BIP329;
                }
            }
            bip329::Labels::new(buff)
        };

        let _ = sender.send(Progress::Progress(30.0));

        account.labels = Some(labels);
        account.transactions = get_transactions(&daemon)
            .await?
            .into_iter()
            .map(|tx| miniscript::bitcoin::consensus::encode::serialize_hex(&tx.tx))
            .collect();

        let _ = sender.send(Progress::Progress(40.0));

        account.psbts = daemon
            .list_spend_transactions(None)
            .await?
            .into_iter()
            .map(|tx| tx.psbt.to_string())
            .collect();

        let _ = sender.send(Progress::Progress(50.0));

        let statuses = [
            CoinStatus::Unconfirmed,
            CoinStatus::Confirmed,
            CoinStatus::Spending,
        ];
        account.coins = daemon
            .list_coins(&statuses, &[])
            .await?
            .coins
            .into_iter()
            .map(|c| (c.outpoint.clone().to_string(), Coin::from(c)))
            .collect();

        let _ = sender.send(Progress::Progress(60.0));

        Ok(Backup {
            name: Some(name),
            alias: wallet_alias,
            accounts: vec![account],
            network,
            proprietary: serde_json::Map::new(),
            date: Some(now()),
            version: 0,
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Account {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub descriptor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<Fingerprint, Key>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<bip329::Labels>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transactions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub psbts: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub coins: BTreeMap<String, Coin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_tip: Option<ChainTip>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub proprietary: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ChainTip {
    pub block_height: i32,
    pub block_hash: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Coin {
    amount: u64,
    outpoint: String,
    address: String,
    block_height: Option<i32>,
    account: u32,
    derivation_index: u32,
    is_coinbase: Option<bool>,
    is_from_self: Option<bool>,
}

impl From<ListCoinsEntry> for Coin {
    fn from(value: ListCoinsEntry) -> Self {
        Self {
            amount: value.amount.to_sat(),
            outpoint: value.outpoint.to_string(),
            address: value.address.to_string(),
            block_height: value.block_height,
            account: if value.is_change { 1 } else { 0 },
            derivation_index: value.derivation_index.into(),
            is_coinbase: if value.is_immature { Some(true) } else { None },
            is_from_self: Some(value.is_from_self),
        }
    }
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
            coins: BTreeMap::new(),
            proprietary: serde_json::Map::new(),
            chain_tip: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Key {
    pub key: Fingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<KeyRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_type: Option<KeyType>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub proprietary: Value,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub enum KeyType {
    /// Main user
    Internal,
    /// Heirs or friends
    External,
    /// Service the user pay for
    ThirdParty,
}

#[cfg(test)]
mod test {
    use super::*;

    fn round_trip(backup: &Backup) -> bool {
        let serialized = serde_json::to_string(backup).unwrap();
        let parsed: Backup = serde_json::from_str(&serialized).unwrap();
        *backup == parsed
    }

    #[test]
    fn backup_serde() {
        let mut backup = Backup {
            name: None,
            alias: None,
            accounts: Vec::new(),
            network: Network::Signet,
            date: Some(0),
            proprietary: serde_json::Map::new(),
            version: 0,
        };
        let serialized = serde_json::to_string(&backup).unwrap();
        let expected = r#"{"accounts":[],"network":"signet","date":0,"version":0}"#;
        assert_eq!(expected, serialized);
        assert!(round_trip(&backup));

        backup.name = Some("Liana".into());

        let serialized = serde_json::to_string(&backup).unwrap();
        let expected = r#"{"name":"Liana","accounts":[],"network":"signet","date":0,"version":0}"#;
        assert_eq!(expected, serialized);
        assert!(round_trip(&backup));

        let descr_str = r#"wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej"#.to_string();

        let account = Account::new(descr_str);
        backup.accounts.push(account);

        let serialized = serde_json::to_string(&backup).unwrap();
        println!("{serialized}");
        let expected = r#"{"name":"Liana","accounts":[{"descriptor":"wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej"}],"network":"signet","date":0,"version":0}"#;
        assert_eq!(expected, serialized);
        assert!(round_trip(&backup));

        // if there is no version, the default is 0
        let no_version = r#"{"name":"Liana","accounts":[{"descriptor":"wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej"}],"network":"signet","date":0}"#;
        let parsed: Backup = serde_json::from_str(no_version).unwrap();
        assert_eq!(parsed.version, 0);

        // Network is mandatory for an account
        let no_network = r#"{"name":"Liana","accounts":[{"descriptor":"wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej"}],"date":0,"version":0}"#;
        let parsed: Result<Backup, _> = serde_json::from_str(no_network);
        assert!(parsed.is_err());

        // But it's the only mandatory field,  w/ accounts array
        let minimal = r#"{"network":"signet","accounts":[]}"#;
        let _parsed: Backup = serde_json::from_str(minimal).unwrap();
    }
}
