use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    app::settings::KeySetting,
    backup::Backup,
    dir::CoincubeDirectory,
    installer::descriptor::PathKind,
    node::bitcoind::{Bitcoind, InternalBitcoindConfig},
    services::{
        coincube::CoincubeClient,
        connect::client::backend::{BackendClient, BackendWalletClient},
    },
    signer::Signer,
};
use async_hwi::DeviceKind;
use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::bitcoin::{self, bip32::Fingerprint},
};
use coincubed::config::{BitcoinBackend, BitcoinConfig};

/// One backend `ConnectVaultMember` row to create after the descriptor
/// is installed. Only keychain-sourced descriptor keys (W8 / W3) produce
/// a payload — hardware-wallet, xpub-entered, master-signer and
/// token-sourced keys are intentionally skipped (per
/// `plans/PLAN-cube-membership-desktop.md` design decision,
/// 2026-04-18: "only keychain-sourced keys become VaultMember rows").
#[derive(Debug, Clone)]
pub struct ConnectVaultMemberPayload {
    pub fingerprint: Fingerprint,
    /// Backend `keys.id` captured when the user selected this key in the
    /// Vault Builder picker.
    pub key_id: u64,
    /// Populated when the key belongs to a contact-Keyholder, `None` when
    /// the key belongs to the vault owner themselves.
    pub contact_id: Option<u64>,
    /// Path the key participates in. Carried through for future role
    /// inference; all members currently default to `Keyholder` per the
    /// 2026-04-18 plan direction.
    pub path_kind: PathKind,
}

#[derive(Debug, Clone)]
pub enum RemoteBackend {
    Undefined,
    None,
    // The installer will have to create a wallet from the created descriptor.
    WithoutWallet(BackendClient),
    // The installer will have to fetch the wallet and only install the missing configuration files.
    WithWallet(BackendWalletClient),
}

impl RemoteBackend {
    pub fn user_email(&self) -> Option<&str> {
        match self {
            Self::WithWallet(b) => Some(b.user_email()),
            Self::WithoutWallet(b) => Some(b.user_email()),
            _ => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, RemoteBackend::None)
    }
    pub fn is_some(&self) -> bool {
        matches!(
            self,
            RemoteBackend::WithoutWallet { .. } | RemoteBackend::WithWallet { .. }
        )
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorTemplate {
    #[default]
    SimpleInheritance,
    Custom,
    MultisigSecurity,
}

#[derive(Clone)]
pub struct Context {
    pub bitcoin_config: BitcoinConfig,
    pub bitcoin_backend: Option<BitcoinBackend>,
    pub descriptor_template: DescriptorTemplate,
    pub descriptor: Option<CoincubeDescriptor>,
    pub keys: HashMap<bitcoin::bip32::Fingerprint, KeySetting>,
    pub hws: Vec<(DeviceKind, bitcoin::bip32::Fingerprint, Option<[u8; 32]>)>,
    pub coincube_directory: CoincubeDirectory,
    pub network: bitcoin::Network,
    pub hw_is_used: bool,
    // In case a user entered a mnemonic,
    // we dont want to override the generated signer with it.
    pub recovered_signer: Option<Arc<Signer>>,
    pub bitcoind_is_external: bool,
    pub use_coincube_connect: bool,
    pub connect_jwt: Option<String>,
    pub install_node_alongside_connect: bool,
    pub internal_bitcoind_config: Option<InternalBitcoindConfig>,
    pub internal_bitcoind: Option<Bitcoind>,
    /// Set when `install_node_alongside_connect` is true; holds the Bitcoind
    /// config that will become the primary backend once IBD completes.
    pub pending_bitcoind_config: Option<coincubed::config::BitcoindConfig>,
    pub remote_backend: RemoteBackend,
    pub backup: Option<Backup>,
    pub wallet_alias: String,
    /// Cube UUID (from CubeSettings.id) — present when the Vault installer
    /// is launched from inside a Cube.  Used by the key picker to fetch
    /// Cube-scoped Keychain keys from the API.
    pub cube_id: Option<String>,
    /// Authenticated coincube-api client, used by the key picker to
    /// fetch Cube-scoped Keychain keys.  `None` when launched from
    /// the Loader (user hasn't done coincube-api auth yet).
    pub coincube_client: Option<CoincubeClient>,
    /// Cube display name used when idempotently registering the cube
    /// with the backend during Final. `None` when no cube settings
    /// were passed in.
    pub cube_name: Option<String>,
    /// Vault members to fan out to the backend after the local install
    /// completes. Populated by `DefineDescriptor::apply()` for every
    /// keychain-sourced descriptor key. Empty when no such keys exist
    /// in the descriptor.
    pub connect_vault_members: Vec<ConnectVaultMemberPayload>,
    /// Approximate timelock (in days) used for the backend vault's
    /// `timelockDays` field. Derived from the longest Recovery path's
    /// `PathSequence::Recovery(blocks)` via `max(blocks / 144, 1)` —
    /// inherently approximate because block cadence varies. Surfaced
    /// with an "approximate" caveat in the Final step's success caption.
    /// `None` when the descriptor has no recovery paths.
    pub connect_vault_timelock_days: Option<i32>,
    /// PIN chosen by the user during a Recovery Kit restore. Populated
    /// by `RestorePinSetupStep` (between `RecoveryKitRestoreStep` and
    /// the node-setup step in `UserFlow::RestoreFromRecoveryKit`).
    ///
    /// Downstream consumers:
    /// - `install_local_wallet` branches on this to call
    ///   `Signer::store_encrypted(..., &pin)` rather than the
    ///   unencrypted `store(...)` so the Liquid/Spark BreezClient can
    ///   decrypt the mnemonic on subsequent Cube opens.
    /// - `gui::tab::find_or_create_cube` / the `CubeSaved` handler use
    ///   the value to populate `CubeSettings.security_pin_hash` and
    ///   `CubeSettings.master_signer_fingerprint`, matching what a
    ///   fresh-install Cube stores.
    ///
    /// Wrapped in `Zeroizing<String>` so the heap allocation is zeroed
    /// when the `Context` clone held by `Task::perform` drops after
    /// the install completes. `None` for non-restore flows.
    pub restore_pin: Option<zeroize::Zeroizing<String>>,
}

impl Context {
    pub fn new(
        network: bitcoin::Network,
        coincube_directory: CoincubeDirectory,
        remote_backend: RemoteBackend,
        cube_settings: Option<&crate::app::settings::CubeSettings>,
        coincube_client: Option<CoincubeClient>,
    ) -> Self {
        Self {
            descriptor_template: DescriptorTemplate::default(),
            bitcoin_config: BitcoinConfig {
                network,
                poll_interval_secs: Duration::from_secs(30),
            },
            hws: Vec::new(),
            keys: HashMap::new(),
            bitcoin_backend: None,
            descriptor: None,
            coincube_directory,
            network,
            hw_is_used: false,
            recovered_signer: None,
            bitcoind_is_external: true,
            use_coincube_connect: false,
            connect_jwt: None,
            install_node_alongside_connect: false,
            internal_bitcoind_config: None,
            internal_bitcoind: None,
            pending_bitcoind_config: None,
            remote_backend,
            wallet_alias: String::new(),
            backup: None,
            cube_id: cube_settings.map(|cs| cs.id.clone()),
            coincube_client,
            cube_name: cube_settings.map(|cs| cs.name.clone()),
            connect_vault_members: Vec::new(),
            connect_vault_timelock_days: None,
            restore_pin: None,
        }
    }
}
