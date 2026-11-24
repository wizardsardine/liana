use std::{
    collections::{BTreeMap, HashMap},
    convert::TryFrom,
    str::FromStr,
    sync::Arc,
};

use bip377::ll::response::{Capabilities, FirmwareVersion, ReleaseFlag};
use chrono::{Duration, Utc};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{
        address::NetworkUnchecked, bip32::Fingerprint, consensus::encode::deserialize_hex,
        consensus::encode::serialize_hex, psbt::Psbt, Address, Network, Transaction, Txid,
    },
};
use lianad::{
    bip329,
    commands::{CoinStatus, GetInfoResult, ListCoinsEntry},
};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    airgap::device::{AirgappedDevice, Registration},
    app::{
        settings::{fiat::PriceSetting, KeySetting, LianaWalletSettings, WalletSettings},
        wallet::Wallet,
    },
    backup::{
        liana_version, Error, KeyRole, KeyType, LegacyAccount, LegacyBackup, LegacyCoin,
        SETTINGS_KEY,
    },
    daemon::{
        model::{HistoryCursor, HistoryTransaction},
        Daemon, DaemonBackend,
    },
    dir::LianaDirectory,
    export::Progress,
    hw::HardwareWalletConfig,
    services::connect::client::backend::DEFAULT_LIMIT,
    utils::{now, serde::ok_or_none},
};

pub const VERSION_PREFIX: &str = "BIP139-";
const VERSION: &str = "BIP139-1";
const BIP380: &str = "bip380";
const PROPRIETARY_KEY: &str = "liana";
const LABELS_PAGE: u32 = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bip139Backup {
    pub version: String,
    pub network: Bip139Network,
    pub accounts: Vec<Account>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exporter: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub transactions: BTreeMap<String, TransactionEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub psbts: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signers: Vec<Signer>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub proprietary: Map<String, Value>,
}

pub struct WalletData {
    pub info: GetInfoResult,
    pub labels: bip329::Labels,
    pub transactions: Vec<(Transaction, Option<i32>, Option<u32>)>,
    pub psbts: Vec<Psbt>,
    pub coins: Vec<ListCoinsEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Preferences {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_internal_bitcoind: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "ok_or_none"
    )]
    pub fiat_price: Option<PriceSetting>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_backup_privacy_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", try_from = "String")]
pub enum Bip139Network {
    Bitcoin,
    Testnet3,
    Testnet4,
    Signet,
    Regtest,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported network {0}")]
pub struct UnsupportedNetwork(String);

impl TryFrom<String> for Bip139Network {
    type Error = UnsupportedNetwork;

    fn try_from(network: String) -> Result<Self, Self::Error> {
        match network.as_str() {
            "bitcoin" => Ok(Self::Bitcoin),
            "testnet3" => Ok(Self::Testnet3),
            "testnet4" => Ok(Self::Testnet4),
            "signet" => Ok(Self::Signet),
            "regtest" => Ok(Self::Regtest),
            _ => Err(UnsupportedNetwork(network)),
        }
    }
}

impl From<Network> for Bip139Network {
    fn from(network: Network) -> Self {
        match network {
            Network::Bitcoin => Self::Bitcoin,
            Network::Testnet => Self::Testnet3,
            Network::Testnet4 => Self::Testnet4,
            Network::Signet => Self::Signet,
            _ => Self::Regtest,
        }
    }
}

impl From<Bip139Network> for Network {
    fn from(network: Bip139Network) -> Self {
        match network {
            Bip139Network::Bitcoin => Network::Bitcoin,
            Bip139Network::Testnet3 => Network::Testnet,
            Bip139Network::Testnet4 => Network::Testnet4,
            Bip139Network::Signet => Network::Signet,
            Bip139Network::Regtest => Network::Regtest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    #[serde(rename = "type")]
    pub kind: String,
    pub descriptor: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receive_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birth_block: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_height: Option<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<Fingerprint, AccountKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<bip329::Labels>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transactions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coins: Vec<Coin>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub psbts: Vec<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub proprietary: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountKey {
    pub key: Fingerprint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option"
    )]
    pub role: Option<Role>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "option"
    )]
    pub key_type: Option<OwnerType>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_of_registration: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Main,
    Recovery,
    Inheritance,
    Cosigning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerType {
    Internal,
    External,
    ThirdParty,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_time: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blockheight: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coin {
    pub outpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_change: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derivation_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_immature: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_from_self: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signer {
    pub fingerprints: Vec<Fingerprint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<Device>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Device {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_version: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "vec"
    )]
    pub transports: Vec<Transport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bip377_capabilities: Vec<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registered: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_checksum: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_stored: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Usb,
    Bluetooth,
    Nfc,
    Qr,
    Sdcard,
}

fn option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

fn vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
}

impl From<KeyRole> for Role {
    fn from(role: KeyRole) -> Self {
        match role {
            KeyRole::Main => Self::Main,
            KeyRole::Recovery => Self::Recovery,
            KeyRole::Inheritance => Self::Inheritance,
            KeyRole::Cosigning => Self::Cosigning,
        }
    }
}

impl From<KeyType> for OwnerType {
    fn from(key_type: KeyType) -> Self {
        match key_type {
            KeyType::Internal => Self::Internal,
            KeyType::External => Self::External,
            KeyType::ThirdParty => Self::ThirdParty,
        }
    }
}

impl Bip139Backup {
    fn new(network: Network, account: Account) -> Self {
        Self {
            version: VERSION.to_owned(),
            network: network.into(),
            accounts: vec![account],
            name: None,
            date: None,
            exporter: None,
            transactions: BTreeMap::new(),
            psbts: BTreeMap::new(),
            signers: Vec::new(),
            proprietary: Map::new(),
        }
    }

    pub async fn from_app(
        datadir: LianaDirectory,
        network: Network,
        wallet: Arc<Wallet>,
        daemon: Arc<dyn Daemon + Sync + Send>,
        sender: &UnboundedSender<Progress>,
    ) -> Result<Self, Error> {
        let network_dir = datadir.network_directory(network);
        let settings =
            WalletSettings::from_file(&network_dir, |settings| wallet.id() == settings.wallet_id())
                .map_err(|_| Error::SettingsFromFile)?;
        let info = daemon.get_info().await?;
        let _ = sender.send(Progress::Progress(20.0));
        let labels = labels(&daemon).await?;
        let _ = sender.send(Progress::Progress(30.0));
        let transactions = transactions(&daemon)
            .await?
            .into_iter()
            .map(|tx| (tx.tx, tx.height, tx.time))
            .collect();
        let _ = sender.send(Progress::Progress(40.0));
        let psbts = daemon
            .list_spend_transactions(None)
            .await?
            .into_iter()
            .map(|spend| spend.psbt)
            .collect();
        let _ = sender.send(Progress::Progress(50.0));
        let statuses = [
            CoinStatus::Unconfirmed,
            CoinStatus::Confirmed,
            CoinStatus::Spending,
        ];
        let coins = daemon.list_coins(&statuses, &[]).await?.coins;
        let _ = sender.send(Progress::Progress(60.0));
        let data = WalletData {
            info,
            labels,
            transactions,
            psbts,
            coins,
        };
        Ok(Self::from_parts(
            network,
            &wallet,
            settings.as_ref(),
            data,
            now().as_secs(),
        ))
    }

    pub fn from_parts(
        network: Network,
        wallet: &Wallet,
        settings: Option<&LianaWalletSettings>,
        data: WalletData,
        date: u64,
    ) -> Self {
        let info = data.info;
        let mut account = Account::new(wallet.main_descriptor.to_string());
        account.name = Some(wallet.name.clone());
        account.receive_index = Some(info.receive_index);
        account.change_index = Some(info.change_index);
        account.last_height = u32::try_from(info.block_height).ok();
        account.birth_block = birth_block(network, u64::from(info.timestamp));
        for key in wallet.main_descriptor.spendable_keys() {
            let fingerprint = key.master_fingerprint();
            let mut entry = AccountKey::new(fingerprint);
            entry.alias = wallet
                .keys_aliases
                .get(&fingerprint)
                .filter(|alias| !alias.is_empty())
                .cloned();
            if wallet.provider_keys.contains(&fingerprint) {
                entry.key_type = Some(OwnerType::ThirdParty);
            }
            account.keys.insert(fingerprint, entry);
        }
        account.labels = Some(data.labels);
        account.coins = data
            .coins
            .into_iter()
            .map(|coin| Coin {
                outpoint: coin.outpoint.to_string(),
                amount: Some(coin.amount.to_sat()),
                script: Some(coin.address.script_pubkey().to_hex_string()),
                block_height: coin
                    .block_height
                    .and_then(|height| u32::try_from(height).ok()),
                is_change: Some(coin.is_change),
                derivation_index: Some(coin.derivation_index.into()),
                is_immature: Some(coin.is_immature),
                is_from_self: Some(coin.is_from_self),
            })
            .collect();

        let mut backup = Self::new(network, account);
        backup.name = wallet
            .alias
            .clone()
            .filter(|alias| !alias.is_empty())
            .or_else(|| Some(wallet.name.clone()));
        backup.date = Some(date);
        backup.exporter = Some(format!("Liana {}", liana_version()));
        for (tx, height, time) in data.transactions {
            let txid = tx.compute_txid().to_string();
            backup.accounts[0].transactions.push(txid.clone());
            backup.transactions.insert(
                txid,
                TransactionEntry {
                    hex: Some(serialize_hex(&tx)),
                    block_time: height.and(time).map(u64::from),
                    blockheight: height.and_then(|height| u32::try_from(height).ok()),
                },
            );
        }
        for psbt in data.psbts {
            let txid = psbt.unsigned_tx.compute_txid().to_string();
            backup.accounts[0].psbts.push(txid.clone());
            backup.psbts.insert(txid, psbt.to_string());
        }
        if let Some(settings) = settings {
            backup.add_settings(settings);
        }
        backup.add_airgapped_signers(wallet);
        backup
    }

    pub fn from_legacy(legacy: LegacyBackup) -> Self {
        let mut backup = Self {
            name: legacy.alias.clone().or(legacy.name.clone()),
            date: legacy.date,
            ..Self::new(legacy.network, Account::new(String::new()))
        };
        backup.accounts.clear();
        for legacy_account in legacy.accounts {
            let settings = legacy_account
                .proprietary
                .get(SETTINGS_KEY)
                .and_then(|settings| {
                    serde_json::from_value::<LianaWalletSettings>(settings.clone()).ok()
                });
            backup.add_legacy_account(legacy_account, legacy.network);
            if let Some(settings) = settings {
                backup.add_settings(&settings);
            }
        }
        backup
    }

    fn add_legacy_account(&mut self, legacy: LegacyAccount, network: Network) {
        let mut account = Account::new(legacy.descriptor);
        account.name = legacy.name;
        account.receive_index = legacy.receive_index;
        account.change_index = legacy.change_index;
        account.last_height = legacy
            .chain_tip
            .and_then(|tip| u32::try_from(tip.block_height).ok());
        account.birth_block = legacy
            .timestamp
            .and_then(|timestamp| birth_block(network, timestamp));
        account.labels = legacy.labels;
        for (fingerprint, key) in legacy.keys {
            let mut entry = AccountKey::new(key.key);
            entry.alias = key.alias;
            entry.role = key.role.map(Role::from);
            entry.key_type = key.key_type.map(OwnerType::from);
            account.keys.insert(fingerprint, entry);
        }
        for hex in legacy.transactions {
            if let Ok(tx) = deserialize_hex::<Transaction>(&hex) {
                let txid = tx.compute_txid().to_string();
                account.transactions.push(txid.clone());
                self.transactions.insert(
                    txid,
                    TransactionEntry {
                        hex: Some(hex),
                        block_time: None,
                        blockheight: None,
                    },
                );
            }
        }
        for psbt in legacy.psbts {
            if let Ok(parsed) = Psbt::from_str(&psbt) {
                let txid = parsed.unsigned_tx.compute_txid().to_string();
                account.psbts.push(txid.clone());
                self.psbts.insert(txid, psbt);
            }
        }
        account.coins = legacy
            .coins
            .into_values()
            .map(|coin| Coin::from_legacy(coin, network))
            .collect();
        self.accounts.push(account);
    }

    fn add_settings(&mut self, settings: &LianaWalletSettings) {
        for hw in &settings.hardware_wallets {
            let device = Device {
                vendor: Some(hw.kind.clone()),
                transports: vec![Transport::Usb],
                registered: Some(true),
                descriptor_checksum: Some(settings.descriptor_checksum.clone()),
                ..Default::default()
            };
            self.add_device(hw.fingerprint, device, Some(&hw.token));
        }
        let preferences = Preferences {
            start_internal_bitcoind: settings.start_internal_bitcoind,
            fiat_price: settings.fiat_price.clone(),
            encrypted_backup_privacy_mode: Some(settings.encrypted_backup_privacy_mode),
            pinned_at: settings.pinned_at,
        };
        if let Ok(Value::Object(liana)) = serde_json::to_value(preferences) {
            self.proprietary = wrap(liana);
        }
    }

    fn add_airgapped_signers(&mut self, wallet: &Wallet) {
        for signer in &wallet.airgapped_signers {
            let registration = signer.registration(&wallet.descriptor_checksum);
            let registered = registration.is_some_and(|registration| {
                registration.stored == Some(true) || registration.proof.is_some()
            });
            let device = Device {
                vendor: Some(signer.vendor.clone()),
                model: Some(signer.model.clone()),
                firmware_version: Some(firmware_version(&signer.version)),
                transports: vec![Transport::Qr],
                bip377_capabilities: vec![signer.capabilities.0],
                registered: Some(registered),
                descriptor_checksum: registered.then(|| wallet.descriptor_checksum.clone()),
                descriptor_stored: registration
                    .and_then(|registration| registration.stored)
                    .filter(|_| registered),
            };
            let proof = registration
                .and_then(|registration| registration.proof.as_deref())
                .map(hex::encode);
            self.add_device(signer.fingerprint, device, proof.as_deref());
        }
    }

    fn add_device(&mut self, fingerprint: Fingerprint, device: Device, proof: Option<&str>) {
        if let (Some(true), Some(proof)) = (device.registered, proof.filter(|p| !p.is_empty())) {
            if let Some(account) = self.accounts.first_mut() {
                if let (Some(key), Some(descriptor)) = (
                    account.keys.get_mut(&fingerprint),
                    account.descriptor.as_str(),
                ) {
                    key.proof_of_registration
                        .push((descriptor.to_owned(), proof.to_lowercase()));
                }
            }
        }
        match self
            .signers
            .iter_mut()
            .find(|signer| signer.fingerprints == [fingerprint])
        {
            Some(signer) => signer.devices.push(device),
            None => self.signers.push(Signer {
                fingerprints: vec![fingerprint],
                devices: vec![device],
            }),
        }
    }

    pub fn preferences(&self) -> Preferences {
        serde_json::from_value(Value::Object(unwrap(&self.proprietary))).unwrap_or_default()
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.version != VERSION {
            return Err(Error::Bip139(format!(
                "unsupported BIP-139 version {}",
                self.version
            )));
        }
        if self.accounts.is_empty() {
            return Err(Error::Bip139("the backup holds no account".to_owned()));
        }
        if self
            .signers
            .iter()
            .any(|signer| signer.fingerprints.is_empty())
        {
            return Err(Error::Bip139("a signer has no fingerprint".to_owned()));
        }
        Ok(())
    }

    pub fn retain_usable_accounts(&mut self) -> Result<Vec<String>, Error> {
        let mut warnings = Vec::new();
        self.accounts.retain(|account| {
            let usable = account
                .descriptor_str()
                .is_some_and(|descriptor| LianaDescriptor::from_str(descriptor).is_ok());
            if !usable {
                warnings.push(format!(
                    "An account of type {} was skipped, Liana cannot use it.",
                    account.kind
                ));
            }
            usable
        });
        if self.accounts.is_empty() {
            return Err(Error::Bip139(format!(
                "the backup holds no {BIP380} account"
            )));
        }
        Ok(warnings)
    }

    pub fn psbts_of<'a>(&'a self, account: &'a Account) -> impl Iterator<Item = &'a String> {
        account
            .psbts
            .iter()
            .filter_map(move |txid| self.psbts.get(txid))
    }

    pub fn hardware_wallets(&self, account: &Account) -> Vec<HardwareWalletConfig> {
        self.restored_devices(account)
            .into_iter()
            .filter_map(|device| match device {
                RestoredDevice::Usb(hw) => Some(hw),
                RestoredDevice::Qr(_) => None,
            })
            .collect()
    }

    pub fn airgapped_devices(&self, account: &Account) -> Vec<AirgappedDevice> {
        self.restored_devices(account)
            .into_iter()
            .filter_map(|device| match device {
                RestoredDevice::Usb(_) => None,
                RestoredDevice::Qr(device) => Some(device),
            })
            .collect()
    }

    fn restored_devices(&self, account: &Account) -> Vec<RestoredDevice> {
        let Some(descriptor) = account.descriptor_str() else {
            return Vec::new();
        };
        let checksum = LianaDescriptor::from_str(descriptor)
            .map(|descriptor| descriptor.checksum())
            .unwrap_or_default();
        let mut restored = Vec::new();
        for signer in &self.signers {
            for fingerprint in &signer.fingerprints {
                let mut proofs = account
                    .keys
                    .get(fingerprint)
                    .into_iter()
                    .flat_map(|key| &key.proof_of_registration)
                    .filter(|(registered, _)| registered == descriptor)
                    .map(|(_, proof)| proof.clone());
                let registered: Vec<bool> = signer
                    .devices
                    .iter()
                    .map(|device| {
                        device.registered == Some(true)
                            && device.descriptor_checksum.as_deref() == Some(checksum.as_str())
                    })
                    .collect();
                let mut tokens = vec![String::new(); signer.devices.len()];
                for stores_descriptor in [false, true] {
                    for (i, device) in signer.devices.iter().enumerate() {
                        let stores = device.transports.contains(&Transport::Qr)
                            && device.descriptor_stored == Some(true);
                        if registered[i] && stores == stores_descriptor {
                            if let Some(proof) = proofs.next() {
                                tokens[i] = proof;
                            }
                        }
                    }
                }
                for ((device, registered), token) in
                    signer.devices.iter().zip(registered).zip(tokens)
                {
                    if device.transports.contains(&Transport::Qr) {
                        let registration = if registered {
                            Registration {
                                descriptor_checksum: Some(checksum.clone()),
                                stored: device.descriptor_stored,
                                proof: hex::decode(&token).ok().filter(|proof| !proof.is_empty()),
                            }
                        } else {
                            Registration {
                                descriptor_checksum: device
                                    .descriptor_checksum
                                    .clone()
                                    .filter(|other| *other != checksum),
                                stored: device.descriptor_stored,
                                proof: None,
                            }
                        };
                        let mut airgapped = AirgappedDevice {
                            fingerprint: *fingerprint,
                            alias: account
                                .keys
                                .get(fingerprint)
                                .and_then(|key| key.alias.clone()),
                            vendor: device.vendor.clone().unwrap_or_default(),
                            model: device.model.clone().unwrap_or_default(),
                            version: parse_firmware_version(device.firmware_version.as_deref()),
                            capabilities: Capabilities(
                                device.bip377_capabilities.first().copied().unwrap_or(0),
                            ),
                            registrations: Vec::new(),
                        };
                        airgapped.record(registration);
                        restored.push(RestoredDevice::Qr(airgapped));
                    } else if registered {
                        if let Some(vendor) = &device.vendor {
                            restored.push(RestoredDevice::Usb(HardwareWalletConfig {
                                kind: vendor.to_lowercase(),
                                fingerprint: *fingerprint,
                                token,
                            }));
                        }
                    }
                }
            }
        }
        restored
    }
}

enum RestoredDevice {
    Usb(HardwareWalletConfig),
    Qr(AirgappedDevice),
}

impl Account {
    fn new(descriptor: String) -> Self {
        Self {
            kind: BIP380.to_owned(),
            descriptor: descriptor.into(),
            name: None,
            receive_index: None,
            change_index: None,
            birth_block: None,
            last_height: None,
            keys: BTreeMap::new(),
            labels: None,
            transactions: Vec::new(),
            coins: Vec::new(),
            psbts: Vec::new(),
            proprietary: Map::new(),
        }
    }

    pub fn descriptor_str(&self) -> Option<&str> {
        (self.kind == BIP380)
            .then(|| self.descriptor.as_str())
            .flatten()
    }

    pub fn key_settings(&self) -> HashMap<Fingerprint, KeySetting> {
        self.keys
            .iter()
            .map(|(fingerprint, key)| {
                (
                    *fingerprint,
                    KeySetting {
                        name: key.alias.clone().unwrap_or_default(),
                        master_fingerprint: *fingerprint,
                        provider_key: key.key_type == Some(OwnerType::ThirdParty),
                    },
                )
            })
            .collect()
    }
}

impl AccountKey {
    fn new(key: Fingerprint) -> Self {
        Self {
            key,
            alias: None,
            role: None,
            key_type: None,
            proof_of_registration: Vec::new(),
        }
    }
}

impl Coin {
    fn from_legacy(coin: LegacyCoin, network: Network) -> Self {
        let script = Address::<NetworkUnchecked>::from_str(&coin.address)
            .ok()
            .and_then(|address| address.require_network(network).ok())
            .map(|address| address.script_pubkey().to_hex_string());
        Self {
            outpoint: coin.outpoint,
            amount: Some(coin.amount),
            script,
            block_height: coin
                .block_height
                .and_then(|height| u32::try_from(height).ok()),
            is_change: Some(coin.account == 1),
            derivation_index: Some(coin.derivation_index),
            is_immature: coin.is_coinbase,
            is_from_self: coin.is_from_self,
        }
    }
}

/// Estimates the birth height from the creation time, as BIP-139 allows for `birth_block`.
fn birth_block(network: Network, timestamp: u64) -> Option<u32> {
    let genesis_time: u64 = match network {
        Network::Bitcoin => 1_231_006_505,
        Network::Testnet => 1_296_688_602,
        Network::Testnet4 => 1_714_777_860,
        _ => return None,
    };
    u32::try_from(timestamp.checked_sub(genesis_time)? / 600).ok()
}

fn firmware_version(version: &FirmwareVersion) -> String {
    let base = format!("{}.{}.{}", version.major, version.minor, version.patch);
    match version.flag {
        ReleaseFlag::Stable => base,
        ReleaseFlag::Alpha(number) => format!("{base}-alpha{number}"),
        ReleaseFlag::Beta(number) => format!("{base}-beta{number}"),
        ReleaseFlag::ReleaseCandidate(number) => format!("{base}-rc{number}"),
        ReleaseFlag::Unknown { number, .. } => format!("{base}-prerelease{number}"),
    }
}

fn parse_firmware_version(version: Option<&str>) -> FirmwareVersion {
    let (numbers, prerelease) = match version.and_then(|version| version.split_once('-')) {
        Some((numbers, prerelease)) => (numbers, Some(prerelease)),
        None => (version.unwrap_or_default(), None),
    };
    let mut parts = numbers.split('.').map(|part| part.parse().unwrap_or(0));
    FirmwareVersion {
        major: parts.next().unwrap_or(0),
        minor: parts.next().unwrap_or(0),
        patch: parts.next().unwrap_or(0),
        flag: match prerelease {
            None => ReleaseFlag::Stable,
            Some(prerelease) => {
                let (tag, number) = prerelease.split_at(
                    prerelease
                        .find(|c: char| c.is_ascii_digit())
                        .unwrap_or(prerelease.len()),
                );
                let number = number.parse().unwrap_or(0);
                match tag {
                    "alpha" => ReleaseFlag::Alpha(number),
                    "beta" => ReleaseFlag::Beta(number),
                    "rc" => ReleaseFlag::ReleaseCandidate(number),
                    _ => ReleaseFlag::Unknown { channel: 0, number },
                }
            }
        },
    }
}

async fn labels(daemon: &Arc<dyn Daemon + Sync + Send>) -> Result<bip329::Labels, Error> {
    let mut labels = Vec::new();
    let mut start = 0;
    loop {
        let mut fetched = daemon
            .get_labels_bip329(start, LABELS_PAGE)
            .await?
            .into_vec();
        let last = fetched.len() < LABELS_PAGE as usize;
        labels.append(&mut fetched);
        if last {
            break;
        }
        start += LABELS_PAGE;
    }
    Ok(bip329::Labels::new(labels))
}

async fn transactions(
    daemon: &Arc<dyn Daemon + Sync + Send>,
) -> Result<Vec<HistoryTransaction>, Error> {
    let limit = match daemon.backend() {
        DaemonBackend::RemoteBackend => DEFAULT_LIMIT as u64,
        _ => u32::MAX as u64,
    };
    let end = ((Utc::now() + Duration::hours(2)).timestamp()) as u32;
    let mut transactions = HashMap::<Txid, HistoryTransaction>::new();
    let mut cursor: Option<HistoryCursor> = None;
    loop {
        let (before, before_txid) = match cursor {
            Some(cursor) => (cursor.time, Some(cursor.txid)),
            None => (end, None),
        };
        let page = daemon.list_history_page(before, before_txid, limit).await?;
        for tx in page.txs {
            transactions.insert(tx.txid, tx);
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    Ok(transactions.into_values().collect())
}

fn wrap(liana: Map<String, Value>) -> Map<String, Value> {
    let mut proprietary = Map::new();
    if !liana.is_empty() {
        proprietary.insert(PROPRIETARY_KEY.to_owned(), liana.into());
    }
    proprietary
}

fn unwrap(proprietary: &Map<String, Value>) -> Map<String, Value> {
    proprietary
        .get(PROPRIETARY_KEY)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        convert::TryFrom,
        str::FromStr,
    };

    use bip377::ll::response::{Capabilities, FirmwareVersion, ReleaseFlag};
    use liana::{
        descriptors::LianaDescriptor,
        miniscript::bitcoin::{
            bip32::{ChildNumber, Fingerprint},
            consensus::encode::{deserialize_hex, serialize_hex},
            psbt::Psbt,
            secp256k1, Network, OutPoint, Transaction,
        },
    };
    use lianad::commands::{GetInfoDescriptors, GetInfoResult, ListCoinsEntry};
    use serde_json::{json, Value};

    use crate::{
        airgap::device::{AirgappedDevice, Registration},
        app::{
            settings::{fiat::PriceSetting, LianaWalletSettings},
            wallet::Wallet,
        },
        backup::{
            bip139::{
                birth_block, firmware_version, parse_firmware_version, Bip139Backup, OwnerType,
                Role, Transport, WalletData,
            },
            liana_version, read, LegacyBackup,
        },
        hw::HardwareWalletConfig,
        services::fiat::{Currency, PriceSource},
    };

    const LIANA: &str = include_str!("../../test_assets/backup/liana.bip139");
    const FOREIGN: &str = include_str!("../../test_assets/backup/foreign.bip139");
    const LEGACY: &str = include_str!("../../test_assets/backup/legacy.json");
    const DESCRIPTOR: &str = include_str!("../../test_assets/backup/wallet.descriptor");
    const TRANSACTIONS: &str = include_str!("../../test_assets/backup/wallet.transactions");
    const PSBT: &str = include_str!("../../test_assets/backup/spend.psbt.base64");
    const ALICE: &str = "73c5da0a";
    const BOB: &str = "b8688df1";
    const CAROL: &str = "28645006";
    const LEDGER_PROOF: &str = "d2c58bc79bc7a9efc9783f469b9a09d99142ea5025d9c6fca415771dcb80f38c";
    const QR_PROOF: &str = "90f1cd9d2493294451796fb6f68b173dcca64a52560700c5ffffbc720160eb2a";
    const OTHER_CHECKSUM: &str = "u768v50p";

    fn fingerprint(hex: &str) -> Fingerprint {
        Fingerprint::from_str(hex).unwrap()
    }

    fn fixture() -> Value {
        serde_json::from_str(LIANA).unwrap()
    }

    fn descriptor() -> LianaDescriptor {
        LianaDescriptor::from_str(DESCRIPTOR.trim()).unwrap()
    }

    fn transactions() -> Vec<Transaction> {
        TRANSACTIONS
            .lines()
            .map(|hex| deserialize_hex(hex).unwrap())
            .collect()
    }

    fn qr_device(fingerprint_hex: &str, registration: Registration) -> AirgappedDevice {
        let mut device = AirgappedDevice {
            fingerprint: fingerprint(fingerprint_hex),
            alias: None,
            vendor: "test vendor".to_owned(),
            model: "test signer".to_owned(),
            version: FirmwareVersion {
                major: 1,
                minor: 2,
                patch: 0,
                flag: ReleaseFlag::Beta(1),
            },
            capabilities: Capabilities(3),
            registrations: Vec::new(),
        };
        device.record(registration);
        device
    }

    fn wallet() -> (Wallet, LianaWalletSettings, WalletData) {
        let descriptor = descriptor();
        let checksum = descriptor.checksum();
        let wallet = Wallet::new(descriptor.clone())
            .with_alias(Some("Savings".to_owned()))
            .with_key_aliases(HashMap::from([
                (fingerprint(ALICE), "Alice".to_owned()),
                (fingerprint(CAROL), "Carol".to_owned()),
            ]))
            .with_provider_keys(HashSet::from([fingerprint(CAROL)]))
            .with_airgapped_signers(&[
                qr_device(
                    ALICE,
                    Registration {
                        descriptor_checksum: Some(checksum.clone()),
                        stored: Some(true),
                        proof: None,
                    },
                ),
                qr_device(
                    BOB,
                    Registration {
                        descriptor_checksum: Some(checksum.clone()),
                        stored: Some(false),
                        proof: Some(hex::decode(QR_PROOF).unwrap()),
                    },
                ),
                qr_device(
                    CAROL,
                    Registration {
                        descriptor_checksum: Some(OTHER_CHECKSUM.to_owned()),
                        stored: Some(true),
                        proof: None,
                    },
                ),
            ]);
        let settings = LianaWalletSettings {
            name: wallet.name.clone(),
            alias: wallet.alias.clone(),
            descriptor_checksum: checksum.clone(),
            pinned_at: Some(1700000000),
            keys: Vec::new(),
            hardware_wallets: vec![HardwareWalletConfig {
                kind: "ledger".to_owned(),
                fingerprint: fingerprint(ALICE),
                token: LEDGER_PROOF.to_owned(),
            }],
            remote_backend_auth: None,
            start_internal_bitcoind: Some(false),
            fiat_price: Some(PriceSetting {
                source: PriceSource::MempoolSpace,
                currency: Currency::EUR,
                is_enabled: true,
            }),
            encrypted_backup_privacy_mode: true,
        };
        let [deposit, self_spend, coinbase, pending] =
            <[Transaction; 4]>::try_from(transactions()).unwrap();
        let secp = secp256k1::Secp256k1::verification_only();
        let receive = |index| {
            descriptor
                .receive_descriptor()
                .derive(ChildNumber::Normal { index }, &secp)
                .address(Network::Testnet)
        };
        let change = descriptor
            .change_descriptor()
            .derive(ChildNumber::Normal { index: 0 }, &secp)
            .address(Network::Testnet);
        let coin = |tx: &Transaction, vout: u32, address, index, height| ListCoinsEntry {
            amount: tx.output[vout as usize].value,
            outpoint: OutPoint {
                txid: tx.compute_txid(),
                vout,
            },
            address,
            block_height: height,
            derivation_index: ChildNumber::Normal { index },
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
            default_label: Default::default(),
        };
        let spent_by_psbt = coin(&deposit, 1, receive(2), 2, Some(2_400_000));
        let change_coin = ListCoinsEntry {
            is_change: true,
            is_from_self: true,
            ..coin(&self_spend, 1, change.clone(), 0, Some(2_450_000))
        };
        let coinbase_coin = ListCoinsEntry {
            is_immature: true,
            ..coin(&coinbase, 0, receive(1), 1, Some(2_499_950))
        };
        let pending_coin = coin(&pending, 0, receive(3), 3, None);
        let data = WalletData {
            info: GetInfoResult {
                version: "test".to_owned(),
                network: Network::Testnet,
                block_height: 2_500_000,
                sync: 1.0,
                descriptors: GetInfoDescriptors {
                    main: descriptor.clone(),
                },
                rescan_progress: None,
                timestamp: 1_700_000_000,
                last_poll_timestamp: None,
                receive_index: 4,
                change_index: 2,
            },
            labels: serde_json::from_value(json!([
                { "type": "tx", "ref": deposit.compute_txid().to_string(), "label": "salary" },
                { "type": "addr", "ref": receive(3).to_string(), "label": "invoice 42" },
                { "type": "output", "ref": format!("{}:1", deposit.compute_txid()), "label": "savings" }
            ]))
            .unwrap(),
            transactions: vec![
                (deposit, Some(2_400_000), Some(1_700_000_050)),
                (self_spend, Some(2_450_000), Some(1_700_030_000)),
                (coinbase, Some(2_499_950), Some(1_700_060_000)),
                (pending, None, None),
            ],
            psbts: vec![Psbt::from_str(PSBT.trim()).unwrap()],
            coins: vec![spent_by_psbt, change_coin, coinbase_coin, pending_coin],
        };
        (wallet, settings, data)
    }

    #[test]
    fn the_export_of_the_wallet_is_the_fixture() {
        let (wallet, settings, data) = wallet();
        let backup =
            Bip139Backup::from_parts(Network::Testnet, &wallet, Some(&settings), data, 1700000100);
        let mut export = serde_json::to_value(backup).unwrap();
        assert_eq!(
            export["exporter"],
            json!(format!("Liana {}", liana_version()))
        );
        export["exporter"] = fixture()["exporter"].clone();
        assert_eq!(export, fixture());
    }

    #[test]
    fn importing_the_fixture_restores_everything_liana_backs_up() {
        let (wallet, settings, data) = wallet();
        let (backup, warnings) = read(LIANA).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(backup.name.as_deref(), Some("Savings"));
        assert_eq!(backup.date, Some(1700000100));
        assert_eq!(
            backup.exporter,
            fixture()["exporter"].as_str().map(str::to_owned)
        );

        let account = &backup.accounts[0];
        assert_eq!(account.descriptor_str(), Some(DESCRIPTOR.trim()));
        assert_eq!(account.name.as_deref(), Some(wallet.name.as_str()));
        assert_eq!(account.receive_index, Some(4));
        assert_eq!(account.change_index, Some(2));
        assert_eq!(account.birth_block, Some(672185));
        assert_eq!(account.last_height, Some(2_500_000));

        let keys = account.key_settings();
        assert_eq!(keys.len(), 3);
        assert_eq!(keys[&fingerprint(ALICE)].name, "Alice");
        assert_eq!(keys[&fingerprint(BOB)].name, "");
        assert_eq!(keys[&fingerprint(CAROL)].name, "Carol");
        assert!(!keys[&fingerprint(ALICE)].provider_key);
        assert!(!keys[&fingerprint(BOB)].provider_key);
        assert!(keys[&fingerprint(CAROL)].provider_key);

        assert_eq!(account.labels.as_ref(), Some(&data.labels));
        let label_types: Vec<_> = serde_json::to_value(&account.labels)
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|label| label["type"].clone())
            .collect();
        assert_eq!(label_types, [json!("tx"), json!("addr"), json!("output")]);

        assert_eq!(account.transactions.len(), data.transactions.len());
        for (tx, height, time) in &data.transactions {
            let entry = &backup.transactions[&tx.compute_txid().to_string()];
            assert_eq!(entry.hex.as_deref(), Some(serialize_hex(tx).as_str()));
            assert_eq!(entry.blockheight, height.map(|h| h as u32));
            assert_eq!(entry.block_time, height.and(*time).map(u64::from));
        }
        assert!(backup
            .transactions
            .values()
            .any(|tx| tx.blockheight.is_some()));
        assert!(backup
            .transactions
            .values()
            .any(|tx| tx.blockheight.is_none()));

        assert_eq!(account.coins.len(), data.coins.len());
        for (coin, expected) in account.coins.iter().zip(&data.coins) {
            assert_eq!(coin.outpoint, expected.outpoint.to_string());
            assert_eq!(coin.amount, Some(expected.amount.to_sat()));
            assert_eq!(
                coin.script,
                Some(expected.address.script_pubkey().to_hex_string())
            );
            assert_eq!(coin.block_height, expected.block_height.map(|h| h as u32));
            assert_eq!(coin.is_change, Some(expected.is_change));
            assert_eq!(
                coin.derivation_index,
                Some(expected.derivation_index.into())
            );
            assert_eq!(coin.is_immature, Some(expected.is_immature));
            assert_eq!(coin.is_from_self, Some(expected.is_from_self));
        }
        let flags = |f: fn(&ListCoinsEntry) -> bool| data.coins.iter().filter(|c| f(c)).count();
        assert!(flags(|c| c.block_height.is_none()) > 0);
        assert!(flags(|c| c.is_change) > 0 && flags(|c| !c.is_change) > 0);
        assert!(flags(|c| c.is_immature) > 0);
        assert!(flags(|c| c.is_from_self) > 0);

        let psbts: Vec<_> = backup.psbts_of(account).collect();
        assert_eq!(psbts.len(), 1);
        let psbt = Psbt::from_str(psbts[0]).unwrap();
        assert_eq!(
            psbt.unsigned_tx.input[0].previous_output,
            data.coins[0].outpoint
        );

        let signer = |hex: &str| {
            backup
                .signers
                .iter()
                .find(|signer| signer.fingerprints == [fingerprint(hex)])
                .unwrap()
        };
        assert_eq!(backup.signers.len(), 3);
        assert_eq!(signer(ALICE).devices.len(), 2);
        assert_eq!(signer(ALICE).devices[0].transports, [Transport::Usb]);
        assert_eq!(signer(ALICE).devices[0].vendor.as_deref(), Some("ledger"));
        assert_eq!(signer(BOB).devices[0].bip377_capabilities, [3]);
        assert_eq!(signer(CAROL).devices[0].registered, Some(false));
        assert_eq!(signer(CAROL).devices[0].descriptor_checksum, None);

        let devices: Vec<_> = backup
            .hardware_wallets(account)
            .into_iter()
            .map(|hw| (hw.kind, hw.fingerprint, hw.token))
            .collect();
        let expected: Vec<_> = settings
            .hardware_wallets
            .iter()
            .map(|hw| (hw.kind.clone(), hw.fingerprint, hw.token.clone()))
            .collect();
        assert_eq!(devices, expected);
        let expected: Vec<_> = wallet
            .airgapped_signers
            .iter()
            .map(|device| AirgappedDevice {
                alias: wallet.keys_aliases.get(&device.fingerprint).cloned(),
                registrations: device
                    .registration(&wallet.descriptor_checksum)
                    .cloned()
                    .into_iter()
                    .collect(),
                ..device.clone()
            })
            .collect();
        assert_eq!(backup.airgapped_devices(account), expected);

        let preferences = backup.preferences();
        assert_eq!(
            preferences.start_internal_bitcoind,
            settings.start_internal_bitcoind
        );
        assert_eq!(preferences.fiat_price, settings.fiat_price);
        assert_eq!(
            preferences.encrypted_backup_privacy_mode,
            Some(settings.encrypted_backup_privacy_mode)
        );
        assert_eq!(preferences.pinned_at, settings.pinned_at);
    }

    #[test]
    fn the_fixture_reads_back_unchanged() {
        let (backup, _) = read(LIANA).unwrap();
        assert_eq!(serde_json::to_value(backup).unwrap(), fixture());
    }

    #[test]
    fn a_foreign_backup_restores_its_bip139_fields() {
        let (backup, warnings) = read(FOREIGN).unwrap();
        assert_eq!(
            warnings,
            ["An account of type bip392 was skipped, Liana cannot use it."]
        );
        assert_eq!(backup.accounts.len(), 1);
        assert_eq!(backup.name.as_deref(), Some("Family vault"));
        let account = &backup.accounts[0];

        let keys = account.key_settings();
        assert_eq!(keys[&fingerprint(ALICE)].name, "Mum's key");
        assert_eq!(keys[&fingerprint(BOB)].name, "Dad's key");
        assert!(keys[&fingerprint(CAROL)].provider_key);
        let mum = &account.keys[&fingerprint(ALICE)];
        assert_eq!(mum.role, Some(Role::Main));
        assert_eq!(mum.key_type, Some(OwnerType::Internal));
        assert_eq!(account.keys[&fingerprint(BOB)].role, None);

        assert_eq!(backup.signers[0].devices[0].transports, [Transport::Qr]);
        assert!(backup.hardware_wallets(account).is_empty());
        let devices = backup.airgapped_devices(account);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].version.minor, 3);
        assert_eq!(devices[0].version.flag, ReleaseFlag::Stable);
        assert_eq!(devices[0].capabilities, Capabilities(0));
        assert!(devices[0].registrations.is_empty());

        let preferences = backup.preferences();
        assert_eq!(preferences.start_internal_bitcoind, None);
        assert_eq!(preferences.pinned_at, None);
    }

    #[test]
    fn a_legacy_backup_still_reads() {
        let (backup, warnings) = read(LEGACY).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(backup.name.as_deref(), Some("Savings"));
        let account = &backup.accounts[0];
        assert_eq!(account.keys.len(), 2);
        let bob = &account.keys[&fingerprint(BOB)];
        assert_eq!(bob.alias.as_deref(), Some("Bob"));
        assert_eq!(bob.role, Some(Role::Recovery));
        assert_eq!(bob.key_type, Some(OwnerType::External));
        assert_eq!(account.transactions.len(), 1);
        assert_eq!(backup.psbts_of(account).count(), 1);
        assert_eq!(account.coins.len(), 1);
        assert!(account.coins[0].script.is_some());
        assert_eq!(account.receive_index, Some(3));
        assert_eq!(account.last_height, Some(250));
    }

    #[test]
    fn the_devices_of_a_legacy_backup_restore() {
        let mut legacy: Value = serde_json::from_str(LEGACY).unwrap();
        legacy["accounts"][0]["proprietary"]["settings"] = json!({
            "name": "Liana-uacwhmkg",
            "alias": null,
            "descriptor_checksum": "uacwhmkg",
            "pinned_at": 1700000000,
            "hardware_wallets": [
                { "kind": "ledger", "fingerprint": ALICE, "token": LEDGER_PROOF }
            ],
            "remote_backend_auth": {
                "email": "user@example.com",
                "wallet_id": "wallet-uuid",
                "refresh_token": "secret"
            },
            "start_internal_bitcoind": true,
            "encrypted_backup_privacy_mode": true
        });
        let legacy: LegacyBackup = serde_json::from_value(legacy).unwrap();
        let backup = Bip139Backup::from_legacy(legacy);
        let export = serde_json::to_value(&backup).unwrap();
        assert_eq!(
            export["proprietary"],
            json!({ "liana": {
                "start_internal_bitcoind": true,
                "encrypted_backup_privacy_mode": true,
                "pinned_at": 1700000000
            }}),
            "only the preferences with no BIP-139 field, never the backend credentials"
        );
        let devices = backup.hardware_wallets(&backup.accounts[0]);
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].kind, "ledger");
        assert_eq!(devices[0].token, LEDGER_PROOF);
    }

    #[test]
    fn an_account_liana_cannot_use_is_skipped() {
        let mut backup = fixture();
        let mut wpkh = backup["accounts"][0].clone();
        wpkh["descriptor"] = json!("wpkh([bf3891f9/48'/1'/0'/2']tpubDF8FX2wbUi7rFS4xC8BfYDu6AScYFvRQBwouJeu2DzE55gJgQk2mSa56D9VbyU9YVdWNqWdFBqhWtV9ixZGbd83SRhr7EZUvGJ8QfYazwks/<0;1>/*)");
        backup["accounts"].as_array_mut().unwrap().push(wpkh);

        let (backup, warnings) = read(&backup.to_string()).unwrap();
        assert_eq!(backup.accounts.len(), 1);
        assert_eq!(
            backup.accounts[0].descriptor_str(),
            Some(descriptor().to_string().as_str())
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_backup_missing_a_mandatory_field_is_refused() {
        let mut no_network = fixture();
        no_network.as_object_mut().unwrap().remove("network");
        assert!(read(&no_network.to_string()).is_err());

        let mut mutinynet = fixture();
        mutinynet["network"] = json!("mutinynet");
        let error = read(&mutinynet.to_string()).unwrap_err();
        assert!(error.to_string().contains("unsupported network mutinynet"));

        let mut no_account = fixture();
        no_account["accounts"] = json!([]);
        assert!(read(&no_account.to_string()).is_err());

        let mut no_descriptor = fixture();
        no_descriptor["accounts"][0]
            .as_object_mut()
            .unwrap()
            .remove("descriptor");
        assert!(read(&no_descriptor.to_string()).is_err());

        let mut next_version = fixture();
        next_version["version"] = json!("BIP139-2");
        let error = read(&next_version.to_string()).unwrap_err();
        assert!(error
            .to_string()
            .contains("unsupported BIP-139 version BIP139-2"));

        let mut only_silent_payments = fixture();
        only_silent_payments["accounts"][0]["type"] = json!("bip392");
        assert!(read(&only_silent_payments.to_string()).is_err());

        let mut negative = fixture();
        negative["accounts"][0]["coins"][0]["block_height"] = json!(-1);
        assert!(read(&negative.to_string()).is_err());

        let mut empty_signer = fixture();
        empty_signer["signers"] = json!([{ "fingerprints": [] }]);
        assert!(read(&empty_signer.to_string()).is_err());
    }

    #[test]
    fn the_firmware_version_keeps_its_pre_release_number() {
        for version in [
            "1.2.3",
            "1.2.3-alpha1",
            "1.2.3-beta3",
            "1.2.3-rc2",
            "1.2.3-prerelease4",
        ] {
            assert_eq!(
                firmware_version(&parse_firmware_version(Some(version))),
                version
            );
        }
        assert_eq!(
            parse_firmware_version(Some("1.2.3-rc2")).flag,
            ReleaseFlag::ReleaseCandidate(2)
        );
        assert_eq!(
            parse_firmware_version(Some("1.2.3-prerelease4")).flag,
            ReleaseFlag::Unknown {
                channel: 0,
                number: 4
            }
        );
        assert_eq!(
            parse_firmware_version(Some("1.2.3-rc")).flag,
            ReleaseFlag::ReleaseCandidate(0)
        );
    }

    #[test]
    fn only_the_first_capabilities_entry_is_read() {
        for (entries, expected) in [("[5, 9]", 5), ("[]", 0)] {
            let json = LIANA.replace("[3]", entries);
            let (backup, _) = read(&json).unwrap();
            let devices = backup.airgapped_devices(&backup.accounts[0]);
            assert_eq!(devices.len(), 3);
            assert!(devices
                .iter()
                .all(|device| device.capabilities == Capabilities(expected)));
        }
    }

    #[test]
    fn the_birth_block_is_estimated_from_genesis_or_left_out() {
        assert_eq!(
            birth_block(Network::Bitcoin, 1_231_006_505 + 600 * 100),
            Some(100)
        );
        assert_eq!(birth_block(Network::Bitcoin, 0), None);
        assert_eq!(birth_block(Network::Signet, 1_700_000_000), None);
        assert_eq!(birth_block(Network::Regtest, 1_700_000_000), None);
    }
}
