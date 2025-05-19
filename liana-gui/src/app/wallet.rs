use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::dir::LianaDirectory;
use crate::{
    app::settings, daemon::DaemonBackend, hw::HardwareWalletConfig, node::NodeType, signer::Signer,
};

use liana::{miniscript::bitcoin, signer::HotSigner};

use liana::descriptors::LianaDescriptor;
use liana::miniscript::bitcoin::bip32::Fingerprint;

use super::settings::{WalletId, WalletSettings};

const DEFAULT_WALLET_NAME: &str = "Liana";

pub fn wallet_name(main_descriptor: &LianaDescriptor) -> String {
    let desc = main_descriptor.to_string();
    let checksum = desc
        .split_once('#')
        .map(|(_, checksum)| checksum)
        .unwrap_or("");
    format!(
        "{}{}{}",
        DEFAULT_WALLET_NAME,
        if checksum.is_empty() { "" } else { "-" },
        checksum
    )
}

#[derive(Debug, Clone)]
pub struct Wallet {
    pub name: String,
    pub alias: Option<String>,
    pub main_descriptor: LianaDescriptor,
    pub descriptor_checksum: String,
    pub pinned_at: Option<i64>,
    // TODO: We could replace these two fields with `keys: HashMap<Fingerprint, settings::KeySetting>`.
    pub keys_aliases: HashMap<Fingerprint, String>,
    pub provider_keys: HashMap<Fingerprint, settings::ProviderKey>,
    pub hardware_wallets: Vec<HardwareWalletConfig>,
    pub signer: Option<Arc<Signer>>,
}

impl Wallet {
    pub fn new(main_descriptor: LianaDescriptor) -> Self {
        Self {
            name: wallet_name(&main_descriptor),
            alias: None,
            descriptor_checksum: main_descriptor
                .to_string()
                .split_once('#')
                .map(|(_, checksum)| checksum)
                .unwrap()
                .to_string(),
            pinned_at: None,
            main_descriptor,
            keys_aliases: HashMap::new(),
            provider_keys: HashMap::new(),
            hardware_wallets: Vec::new(),
            signer: None,
        }
    }

    pub fn with_name(mut self, name: String) -> Self {
        self.name = name;
        self
    }

    pub fn with_alias(mut self, alias: Option<String>) -> Self {
        self.alias = alias;
        self
    }

    // To match with WalletSettings.wallet_id
    pub fn id(&self) -> WalletId {
        WalletId::new(self.descriptor_checksum.clone(), self.pinned_at)
    }

    pub fn with_pinned_at(mut self, pinned_at: Option<i64>) -> Self {
        self.pinned_at = pinned_at;
        self
    }

    pub fn with_key_aliases(mut self, aliases: HashMap<Fingerprint, String>) -> Self {
        self.keys_aliases = aliases;
        self
    }

    pub fn with_provider_keys(
        mut self,
        provider_keys: HashMap<Fingerprint, settings::ProviderKey>,
    ) -> Self {
        self.provider_keys = provider_keys;
        self
    }

    pub fn with_hardware_wallets(mut self, hardware_wallets: Vec<HardwareWalletConfig>) -> Self {
        self.hardware_wallets = hardware_wallets;
        self
    }

    pub fn with_signer(mut self, signer: Signer) -> Self {
        self.signer = Some(Arc::new(signer));
        self
    }

    pub fn descriptor_keys(&self) -> HashSet<Fingerprint> {
        let info = self.main_descriptor.policy();
        let mut descriptor_keys = HashSet::new();
        for (fingerprint, _) in info.primary_path().thresh_origins().1.iter() {
            descriptor_keys.insert(*fingerprint);
        }
        for path in info.recovery_paths().values() {
            for (fingerprint, _) in path.thresh_origins().1.iter() {
                descriptor_keys.insert(*fingerprint);
            }
        }
        descriptor_keys
    }

    pub fn load_from_settings(self, wallet_settings: WalletSettings) -> Result<Self, WalletError> {
        if wallet_settings.descriptor_checksum != self.descriptor_checksum {
            Err(WalletError::WrongWalletLoaded)
        } else {
            Ok(self
                .with_key_aliases(wallet_settings.keys_aliases())
                .with_provider_keys(wallet_settings.provider_keys())
                .with_alias(wallet_settings.alias)
                .with_name(wallet_settings.name)
                .with_pinned_at(wallet_settings.pinned_at)
                .with_hardware_wallets(wallet_settings.hardware_wallets))
        }
    }

    pub fn load_hotsigners(
        self,
        datadir_path: &LianaDirectory,
        network: bitcoin::Network,
    ) -> Result<Self, WalletError> {
        let hot_signers = match HotSigner::from_datadir(datadir_path.path(), network) {
            Ok(signers) => signers,
            Err(e) => match e {
                liana::signer::SignerError::MnemonicStorage(e) => {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Vec::new()
                    } else {
                        return Err(WalletError::HotSigner(e.to_string()));
                    }
                }
                _ => return Err(WalletError::HotSigner(e.to_string())),
            },
        };

        let curve = bitcoin::secp256k1::Secp256k1::signing_only();
        let keys = self.descriptor_keys();
        if let Some(hot_signer) = hot_signers
            .into_iter()
            .find(|s| keys.contains(&s.fingerprint(&curve)))
        {
            Ok(self.with_signer(Signer::new(hot_signer)))
        } else {
            Ok(self)
        }
    }

    pub fn keys(&self) -> HashMap<Fingerprint, settings::KeySetting> {
        let mut map = HashMap::new();
        self.keys_aliases.iter().for_each(|(fg, alias)| {
            map.insert(
                *fg,
                settings::KeySetting {
                    name: alias.clone(),
                    master_fingerprint: *fg,
                    provider_key: None,
                },
            );
        });

        self.provider_keys.iter().for_each(|(fg, key)| {
            if let Some(entry) = map.get_mut(fg) {
                entry.provider_key = Some(key.clone())
            }
        });

        map
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum WalletError {
    WrongWalletLoaded,
    Settings(settings::SettingsError),
    HotSigner(String),
}

impl std::fmt::Display for WalletError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::WrongWalletLoaded => write!(f, "Wrong wallet was loaded"),
            Self::Settings(e) => write!(f, "Failed to load settings: {}", e),
            Self::HotSigner(e) => write!(f, "Failed to load hot signer: {}", e),
        }
    }
}

impl From<settings::SettingsError> for WalletError {
    fn from(error: settings::SettingsError) -> Self {
        WalletError::Settings(error)
    }
}

/// The sync status of a wallet with respect to the blockchain.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncStatus {
    /// Wallet and blockchain are fully synced.
    Synced,
    /// Wallet is performing a full scan of the blockchain.
    WalletFullScan,
    /// Wallet is syncing with latest transactions.
    LatestWalletSync,
    /// Blockchain is syncing with given progress between 0.0 and 1.0.
    BlockchainSync(f64),
}

impl SyncStatus {
    pub fn is_synced(&self) -> bool {
        self == &SyncStatus::Synced
    }

    /// Whether the wallet itself, and not the blockchain, is syncing.
    pub fn wallet_is_syncing(&self) -> bool {
        self == &SyncStatus::WalletFullScan || self == &SyncStatus::LatestWalletSync
    }
}

/// Get the [`SyncStatus`].
///
/// The `last_poll_at_startup` is the timestamp of the last poll
/// of the blockchain when the application was first loaded, while
/// `last_poll` refers to the most recent poll.
///
/// `sync_progress` is the blockchain synchronization progress as
/// a number between `0.0` and `1.0`.
pub fn sync_status(
    daemon_backend: DaemonBackend,
    blockheight: i32,
    sync_progress: f64,
    last_poll: Option<u32>,
    last_poll_at_startup: Option<u32>,
) -> SyncStatus {
    if sync_progress < 1.0 {
        return SyncStatus::BlockchainSync(sync_progress);
    } else if blockheight <= 0 {
        // If blockheight <= 0, then this is a newly created wallet.
        // If user imported descriptor and is using a local bitcoind, a rescan
        // will need to be performed in order to see past transactions and so the
        // syncing status could be misleading as it could suggest the rescan is
        // being performed.
        // For external daemon or if we otherwise don't know the node type,
        // treat it the same as bitcoind to be sure we don't mislead the user.
        if daemon_backend == DaemonBackend::RemoteBackend
            || daemon_backend == DaemonBackend::EmbeddedLianad(Some(NodeType::Electrum))
        {
            return SyncStatus::WalletFullScan;
        }
    }
    // For an existing wallet with any local node type, if the first poll has
    // not completed, then the wallet has not yet caught up with the tip.
    // An existing wallet with remote backend remains synced so we can ignore it.
    // If external daemon, we cannot be sure it will return last poll as it
    // depends on the version, so assume it won't unless the last poll at
    // startup is set.
    // TODO: should we check the daemon version at GUI startup?
    else if last_poll <= last_poll_at_startup
        && (daemon_backend.is_embedded()
            || (daemon_backend == DaemonBackend::ExternalLianad && last_poll_at_startup.is_some()))
    {
        return SyncStatus::LatestWalletSync;
    }
    SyncStatus::Synced
}
