use std::{fmt, sync::Arc};

use async_hwi::HWI;
use liana::miniscript::{
    bitcoin::{
        bip32::{ChildNumber, Fingerprint},
        Network,
    },
    Descriptor, DescriptorPublicKey,
};
use liana_ui::component::{form::Value, panels::map::modals::ImportMode};
use lianad::config::BitcoinBackend;
use tokio::task::JoinError;

use crate::{
    app::{
        error::Error,
        state::map::external::{
            device_descriptors, remember_electrum, remove, scan, standard_paths, ExternalError,
            ExternalWallet, ImportError, ScanError,
        },
    },
    dir::NetworkDirectory,
    hw::HardwareWallets,
    t, utils,
};

/// The import form of an external wallet.
#[derive(Debug)]
pub struct ImportForm {
    pub mode: ImportMode,
    pub name: Value<String>,
    pub descriptor: Value<String>,
    pub account: Value<String>,
    /// Asked only when the current wallet has no Electrum server.
    pub electrum: Option<Value<String>>,
    pub error: Option<String>,
    pub scanning: bool,
    /// The device whose xpubs are being fetched.
    pub device: Option<Fingerprint>,
    pub hws: HardwareWallets,
}

fn value(value: String) -> Value<String> {
    Value {
        value,
        warning: None,
        valid: true,
    }
}

impl ImportForm {
    pub fn new(hws: HardwareWallets, electrum: Option<String>) -> Self {
        Self {
            mode: ImportMode::Descriptor,
            name: Value::default(),
            descriptor: Value::default(),
            account: value("0".to_string()),
            electrum: electrum.map(value),
            error: None,
            scanning: false,
            device: None,
            hws,
        }
    }

    /// The name and the asked Electrum server are filled.
    pub fn ready(&self) -> bool {
        !self.name.value.trim().is_empty()
            && self
                .electrum
                .as_ref()
                .is_none_or(|electrum| !electrum.value.trim().is_empty())
    }

    pub fn can_import(&self) -> bool {
        self.ready()
            && self.mode == ImportMode::Descriptor
            && !self.descriptor.value.trim().is_empty()
    }
}

/// The Electrum server of the current wallet, `None` when it uses bitcoind.
pub fn daemon_electrum(backend: Option<&BitcoinBackend>) -> Option<String> {
    match backend? {
        BitcoinBackend::Electrum(config) => Some(config.addr.clone()),
        BitcoinBackend::Bitcoind(_) => None,
    }
}

/// An account number below the hardened range.
pub fn parse_account(s: &str) -> Result<u32, ImportFailure> {
    let account = s.trim().parse().map_err(|_| ImportFailure::Account)?;
    ChildNumber::from_hardened_idx(account).map_err(|_| ImportFailure::Account)?;
    Ok(account)
}

#[derive(Debug)]
pub enum ImportFailure {
    Descriptor(ImportError),
    Account,
    Device(async_hwi::Error),
    Scan(ScanError),
    Wallet(ExternalError),
    NoHistory,
    NoElectrum,
    Join(JoinError),
}

impl fmt::Display for ImportFailure {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Descriptor(e) => write!(f, "{e}"),
            Self::Account => write!(f, "{}", t!("map-import-invalid-account")),
            Self::Device(e) => write!(f, "{e}"),
            Self::Scan(e) => write!(f, "{e}"),
            Self::Wallet(e) => write!(f, "{e}"),
            Self::NoHistory => write!(f, "{}", t!("map-import-no-history")),
            Self::NoElectrum => write!(f, "{}", t!("map-import-no-electrum")),
            Self::Join(e) => write!(f, "{e}"),
        }
    }
}

impl From<ImportError> for ImportFailure {
    fn from(e: ImportError) -> Self {
        Self::Descriptor(e)
    }
}

impl From<async_hwi::Error> for ImportFailure {
    fn from(e: async_hwi::Error) -> Self {
        Self::Device(e)
    }
}

impl From<ScanError> for ImportFailure {
    fn from(e: ScanError) -> Self {
        Self::Scan(e)
    }
}

impl From<ExternalError> for ImportFailure {
    fn from(e: ExternalError) -> Self {
        Self::Wallet(e)
    }
}

impl From<JoinError> for ImportFailure {
    fn from(e: JoinError) -> Self {
        Self::Join(e)
    }
}

impl From<ImportFailure> for Error {
    fn from(e: ImportFailure) -> Self {
        Error::Unexpected(e.to_string())
    }
}

pub enum ImportSource {
    Descriptor(Box<Descriptor<DescriptorPublicKey>>),
    Device {
        device: Arc<dyn HWI + Send + Sync>,
        fingerprint: Fingerprint,
        account: u32,
    },
}

/// The single key descriptors of the device xpubs at the standard paths of `account`.
async fn fetch_device_descriptors(
    device: Arc<dyn HWI + Send + Sync>,
    fingerprint: Fingerprint,
    network: Network,
    account: u32,
) -> Result<Vec<Descriptor<DescriptorPublicKey>>, ImportFailure> {
    let mut xpubs = Vec::new();
    for (single_sig, path) in standard_paths(network, account) {
        let xpub = device.get_extended_pubkey(&path).await?;
        xpubs.push((single_sig, path, xpub));
    }
    Ok(device_descriptors(fingerprint, xpubs, network)?)
}

fn now() -> u32 {
    utils::now().as_secs() as u32
}

/// Scans a new external wallet and saves it. A device import keeps only the descriptors with
/// history, a pasted descriptor is kept even without. `remember`: the Electrum server was asked.
pub async fn import(
    network_dir: NetworkDirectory,
    network: Network,
    name: String,
    source: ImportSource,
    electrum: String,
    remember: bool,
) -> Result<ExternalWallet, ImportFailure> {
    let (descriptors, keep_empty) = match source {
        ImportSource::Descriptor(descriptor) => (vec![*descriptor], true),
        ImportSource::Device {
            device,
            fingerprint,
            account,
        } => (
            fetch_device_descriptors(device, fingerprint, network, account).await?,
            false,
        ),
    };
    let wallet = ExternalWallet::new(name, descriptors, now(), remember.then(|| electrum.clone()))?;
    tokio::task::spawn_blocking(move || {
        let result = scan_new(wallet.clone(), &network_dir, network, &electrum, keep_empty);
        if result.is_err() {
            if let Err(e) = remove(&network_dir, &wallet.id) {
                log::warn!(
                    "Cannot delete the scan of external wallet {}: {e}",
                    wallet.id
                );
            }
        }
        let wallet = result?;
        if remember {
            remember_electrum(&network_dir, &electrum)?;
        }
        Ok(wallet)
    })
    .await?
}

fn scan_new(
    mut wallet: ExternalWallet,
    network_dir: &NetworkDirectory,
    network: Network,
    electrum: &str,
    keep_empty: bool,
) -> Result<ExternalWallet, ImportFailure> {
    let scanned = scan(
        &wallet.descriptors,
        electrum,
        &wallet.dir(network_dir),
        network,
    )?;
    wallet.descriptors = scanned
        .into_iter()
        .filter(|result| keep_empty || result.has_history)
        .map(|result| result.descriptor)
        .collect();
    if wallet.descriptors.is_empty() {
        return Err(ImportFailure::NoHistory);
    }
    wallet.last_scan = Some(now());
    wallet.save(network_dir)?;
    Ok(wallet)
}

/// Scans `wallet` again in place, blocking.
pub fn rescan(
    mut wallet: ExternalWallet,
    network_dir: &NetworkDirectory,
    network: Network,
    electrum: &str,
) -> Result<ExternalWallet, ImportFailure> {
    scan(
        &wallet.descriptors,
        electrum,
        &wallet.dir(network_dir),
        network,
    )?;
    wallet.last_scan = Some(now());
    wallet.save(network_dir)?;
    Ok(wallet)
}

#[cfg(test)]
mod tests {
    use lianad::config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth, ElectrumConfig};

    use crate::app::state::map::import::{daemon_electrum, parse_account, ImportFailure};

    #[test]
    fn account_numbers() {
        assert_eq!(parse_account("0").unwrap(), 0);
        assert_eq!(parse_account(" 7 ").unwrap(), 7);
        assert_eq!(parse_account("2147483647").unwrap(), 2147483647);
        for invalid in ["", "-1", "1.5", "a", "2147483648", "4294967296"] {
            assert!(
                matches!(parse_account(invalid), Err(ImportFailure::Account)),
                "{}",
                invalid
            );
        }
    }

    #[test]
    fn electrum_of_the_current_wallet() {
        assert_eq!(daemon_electrum(None), None);
        let electrum = BitcoinBackend::Electrum(ElectrumConfig {
            addr: "ssl://electrum.example.com:50002".to_string(),
            validate_domain: true,
        });
        assert_eq!(
            daemon_electrum(Some(&electrum)),
            Some("ssl://electrum.example.com:50002".to_string())
        );
        let bitcoind = BitcoinBackend::Bitcoind(BitcoindConfig {
            rpc_auth: BitcoindRpcAuth::UserPass("user".to_string(), "pass".to_string()),
            addr: "127.0.0.1:8332".parse().unwrap(),
        });
        assert_eq!(daemon_electrum(Some(&bitcoind)), None);
    }
}
