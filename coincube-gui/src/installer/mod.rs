pub(crate) mod connect_vault;
mod context;
mod decrypt;
mod descriptor;
mod message;
pub(crate) mod migration;
mod prompt;
pub(crate) mod step;
mod view;

// Shared node-resources control (prune + mempool presets/fields + disk
// estimate), reused by the Vault node settings so the two surfaces never drift.
pub(crate) use view::node_resources_controls;

pub(crate) fn connect_url(network: bitcoin::Network) -> String {
    let network_path = match network {
        bitcoin::Network::Bitcoin => "bitcoin/mainnet",
        bitcoin::Network::Testnet => "bitcoin/testnet",
        bitcoin::Network::Signet => "bitcoin/signet",
        bitcoin::Network::Testnet4 => "bitcoin/testnet4",
        _ => "bitcoin/regtest",
    };
    let base = crate::services::coincube_api_base_url();
    format!("{}/api/v1/esplora/{}", base, network_path)
}

/// Public-Esplora URL for the given network. On non-mainnet networks this is
/// the daemon's primary endpoint so wallet sync traffic distributes across
/// users' IPs instead of consolidating onto coincube-api's IP (which is where
/// the per-IP rate limits bite). The chain in that flow goes
/// `public_esplora_url` (mempool.space) → [`public_esplora_fallback_url`]
/// (blockstream.info) → [`connect_url`] (authenticated backstop), so a
/// throttled mempool doesn't immediately push the user to the metered
/// Connect URL.
///
/// Mainnet inverts this: [`connect_esplora_config`] makes [`connect_url`]
/// (COINCUBE API) the primary and demotes these public URLs to fallbacks, so
/// mainnet address lookups stay on COINCUBE infrastructure. See
/// [`connect_esplora_config`] for the assembled chain.
///
/// Regtest has no public counterpart — callers in regtest builds should
/// configure their own endpoint via the installer's manual-Esplora step.
pub(crate) fn public_esplora_url(network: bitcoin::Network) -> String {
    match network {
        bitcoin::Network::Bitcoin => "https://mempool.space/api".to_string(),
        bitcoin::Network::Testnet => "https://mempool.space/testnet/api".to_string(),
        bitcoin::Network::Testnet4 => "https://mempool.space/testnet4/api".to_string(),
        bitcoin::Network::Signet => "https://mempool.space/signet/api".to_string(),
        _ => "http://localhost:3000/api".to_string(),
    }
}

/// Second public-Esplora URL, used as the daemon's *first* fallback
/// between [`public_esplora_url`] (mempool.space) and [`connect_url`]
/// (the metered backstop). Using a different provider for this slot —
/// `blockstream.info` — means a mempool 429 doesn't imply this
/// endpoint is also throttled.
///
/// Returns `None` for networks where blockstream doesn't publish a
/// public Esplora (signet, testnet4, regtest); those flows just skip
/// straight from mempool to the Connect backstop.
pub(crate) fn public_esplora_fallback_url(network: bitcoin::Network) -> Option<String> {
    match network {
        bitcoin::Network::Bitcoin => Some("https://blockstream.info/api".to_string()),
        bitcoin::Network::Testnet => Some("https://blockstream.info/testnet/api".to_string()),
        // Blockstream doesn't host signet / testnet4 / regtest. Returning
        // None means the daemon's provider chain shrinks to
        // mempool → Connect for these networks, which matches the
        // previous behaviour exactly.
        _ => None,
    }
}

/// Build the Esplora provider chain for a Connect-backed install.
///
/// Mainnet routes primary traffic through COINCUBE API (self-hosted node —
/// keeps mainnet addresses off public providers) with mempool.space and
/// blockstream.info demoted to fallbacks. Every other network keeps the
/// public-primary chain (mempool.space → blockstream.info → Connect) so sync
/// load stays distributed across user IPs.
pub(crate) fn connect_esplora_config(network: bitcoin::Network, jwt: &str) -> EsploraConfig {
    if network == bitcoin::Network::Bitcoin {
        EsploraConfig {
            addr: connect_url(network),
            token: Some(jwt.to_owned()),
            fallback_addr: Some(public_esplora_url(network)),
            fallback_token: None,
            // Some(blockstream.info) for mainnet.
            secondary_fallback_addr: public_esplora_fallback_url(network),
            secondary_fallback_token: None,
        }
    } else {
        // Unchanged public-primary chain for testnet/testnet4/signet/regtest.
        let (fallback_addr, fallback_token, secondary_fallback_addr, secondary_fallback_token) =
            match public_esplora_fallback_url(network) {
                Some(public_fallback) => (
                    Some(public_fallback),
                    None,
                    Some(connect_url(network)),
                    Some(jwt.to_owned()),
                ),
                None => (Some(connect_url(network)), Some(jwt.to_owned()), None, None),
            };
        EsploraConfig {
            addr: public_esplora_url(network),
            token: None,
            fallback_addr,
            fallback_token,
            secondary_fallback_addr,
            secondary_fallback_token,
        }
    }
}

use coincube_core::miniscript::bitcoin::{self, Network};
use coincube_ui::{
    component::network_banner,
    widget::{Column, Element},
};
use coincubed::config::EsploraConfig;
use coincubed::config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth, Config};
pub use context::{Context, RemoteBackend, RestoreSource};
use iced::{clipboard, Subscription, Task};
use std::{collections::HashMap, ops::Deref};
use tokio::runtime::Handle;
use tracing::{error, info, warn};

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{
    app::{
        config as gui_config,
        settings::{update_settings_file, AuthConfig, SettingsError, WalletId, WalletSettings},
        wallet::wallet_name,
    },
    daemon::{Daemon, DaemonError},
    delete,
    dir::{CoincubeDirectory, NetworkDirectory},
    hw::{HardwareWalletConfig, HardwareWalletMessage, HardwareWallets},
    services::{
        self,
        connect::client::{
            auth::AuthError,
            backend::{
                api::payload::{Provider, ProviderKey},
                BackendClient, BackendWalletClient,
            },
            cache::update_connect_cache,
        },
    },
    signer::Signer,
};

pub use descriptor::{KeySource, KeySourceKind, KeychainKeyOwner, PathKind, PathSequence};
pub use message::Message;
use step::{
    BackupDescriptor, BackupMnemonic, ChooseBackend, ChooseDescriptorTemplate, CoincubeConnectStep,
    DefineDescriptor, DefineNode, DescriptorTemplateDescription, Final, ImportDescriptor,
    ImportRemoteWallet, InheritanceRestoreStep, InternalBitcoindStep, OwnerKeychainRestoreStep,
    RecoverMnemonic, RecoveryKitRestoreStep, RegisterDescriptor, RemoteBackendLogin,
    RestorePinSetupStep, RestoreScope, SelectBitcoindTypeStep, Step, WalletAlias,
};

#[derive(Debug, Clone)]
pub enum UserFlow {
    CreateWallet,
    AddWallet,
    /// W13 — full install restore from a Cube Recovery Kit.
    /// Skips the descriptor-editor, backup-mnemonic, and register-
    /// descriptor steps because the kit provides both the seed and
    /// the descriptor. The flow is: RecoveryKitRestore(Full) →
    /// CoincubeConnect (noop — already authed) → Node setup →
    /// WalletAlias → Final.
    ///
    /// `cube_uuid` preselects a specific remote cube (set when launched
    /// from a home "Your Cubes" row) so the step skips its picker; `None`
    /// falls back to the picker.
    RestoreFromRecoveryKit {
        cube_uuid: Option<String>,
    },
    /// W15 — restore the Wallet Descriptor only, from a running
    /// Cube. Assumes the Cube's seed is already on disk (the user
    /// just needs to rehydrate the vault). Launched from the
    /// running app's "Create Vault" menu.
    RestoreVaultFromRecoveryKit,
    /// Heir inheritance recovery (ECIES pivot, COIN-377 PR 3). Launched from
    /// the pre-Cube "Recover a Vault" surface. `InheritanceRestoreStep`
    /// fetches + relay-decrypts the heir's escrowed envelope(s) and stages the
    /// seed/descriptor; the rest reuses the existing restore machinery.
    /// `full_cube` picks the scope: Full (seed + descriptor → a real Cube, with
    /// a PIN-setup step) vs descriptor-only (watch-only Vault recovery).
    RecoverInheritedVault {
        cube_id: u64,
        full_cube: bool,
    },
    /// Owner self-recovery via Keychain (PLAN-owner-keychain-recovery PR 3).
    /// Launched from the "Recover a Cube I own → with my phone" surface. Same
    /// shape as `RecoverInheritedVault` but `OwnerKeychainRestoreStep` is the
    /// decrypt source: it pulls the owner's *own* `recovery-kit/envelope` set and
    /// relay-decrypts it via the owner's Keychain — no recovery password.
    /// `full_cube` picks the scope: Full (seed + descriptor → a real Cube, with a
    /// PIN-setup step) vs descriptor-only (watch-only Vault recovery).
    RecoverOwnCubeWithPhone {
        cube_id: u64,
        full_cube: bool,
    },
}

pub struct Installer {
    pub network: bitcoin::Network,
    pub datadir: CoincubeDirectory,

    current: usize,
    steps: Vec<Box<dyn Step>>,
    hws: HardwareWallets,
    signer: Arc<Mutex<Signer>>,

    /// Context is data passed through each step.
    pub context: Context,

    /// Track if installer was launched from an app without vault (true) or from home (false)
    pub launched_from_app: bool,

    /// Cube settings when launched from app (for returning to the same cube)
    pub cube_settings: Option<crate::app::settings::CubeSettings>,

    /// Pre-loaded BreezClient when launched from app (avoids re-entering PIN)
    pub breez_client: Option<std::sync::Arc<crate::app::breez_liquid::BreezClient>>,

    /// Pre-loaded SparkBackend when launched from app — preserved
    /// across the vault-setup round-trip so the Spark bridge
    /// subprocess isn't killed and re-spawned.
    pub spark_backend: Option<std::sync::Arc<crate::app::wallets::SparkBackend>>,
    pub developer_mode: bool,
}

impl Installer {
    fn previous(&mut self) -> Task<Message> {
        self.hws.reset_watch_list();
        let network = self.network;
        if self.current > 0 {
            self.current -= 1;
        } else {
            // At first step - return to App (installer only launched from App for Vault setup)
            return Task::done(Message::BackToApp(network));
        }
        // skip the previous step according to the current context.
        while self
            .steps
            .get(self.current)
            .expect("There is always a step")
            .skip(&self.context)
        {
            if self.current > 0 {
                self.current -= 1;
            } else {
                // At first step - return to App (installer only launched from App for Vault setup)
                return Task::done(Message::BackToApp(network));
            }
        }

        if let Some(step) = self.steps.get(self.current) {
            step.revert(&mut self.context)
        }
        Task::none()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        destination_path: CoincubeDirectory,
        network: bitcoin::Network,
        remote_backend: Option<BackendClient>,
        user_flow: UserFlow,
        launched_from_app: bool,
        cube_settings: Option<crate::app::settings::CubeSettings>,
        breez_client: Option<std::sync::Arc<crate::app::breez_liquid::BreezClient>>,
        spark_backend: Option<std::sync::Arc<crate::app::wallets::SparkBackend>>,
        mut developer_mode: bool,
        coincube_client: Option<crate::services::coincube::CoincubeClient>,
    ) -> (Installer, Task<Message>) {
        let signer = if developer_mode {
            let master_signer = breez_client
                .as_ref()
                .and_then(|bc| bc.liquid_signer())
                .and_then(|arc_hs| {
                    arc_hs
                        .lock()
                        .ok()
                        .and_then(|hs_guard| hs_guard.try_clone().ok())
                        .map(|hs| Arc::new(Mutex::new(Signer::new(hs))))
                });
            if let Some(ms) = master_signer {
                ms
            } else {
                tracing::warn!(
                    "developer_mode=true but master signer unavailable; \
                     downgrading to normal mode"
                );
                developer_mode = false;
                Arc::new(Mutex::new(Signer::generate(network).unwrap()))
            }
        } else {
            Arc::new(Mutex::new(Signer::generate(network).unwrap()))
        };
        let mut context = Context::new(
            network,
            destination_path.clone(),
            remote_backend
                .map(RemoteBackend::WithoutWallet)
                .unwrap_or_else(|| {
                    match (&user_flow, network) {
                        // CreateWallet no longer has a ChooseBackend step; always local.
                        (UserFlow::CreateWallet, _) => RemoteBackend::None,
                        // Restore flows also have no ChooseBackend step — the
                        // user is restoring a descriptor (and optionally a
                        // seed) into a fresh local install. Connect auth still
                        // happens via the `CoincubeConnectStep` further down
                        // the flow, which sets `ctx.connect_jwt` and lets
                        // Final idempotently re-register the Cube. Without
                        // this match arm `ctx.remote_backend` stays at
                        // `Undefined` and the `Message::Install` match panics
                        // with `unreachable!("Must be defined at this point")`.
                        (UserFlow::RestoreFromRecoveryKit { .. }, _)
                        | (UserFlow::RestoreVaultFromRecoveryKit, _)
                        | (UserFlow::RecoverInheritedVault { .. }, _)
                        | (UserFlow::RecoverOwnCubeWithPhone { .. }, _) => RemoteBackend::None,
                        // AddWallet still has ChooseBackend which transitions away from Undefined.
                        (_, Network::Bitcoin | Network::Signet) => RemoteBackend::Undefined,
                        // Non-mainnet/signet AddWallet skips backend choice.
                        _ => RemoteBackend::None,
                    }
                }),
            cube_settings.as_ref(),
            coincube_client,
        );
        // Inherit the open Cube's PIN when the installer was launched from
        // inside one (`SetupVault` from the app or the loader). Every seed the
        // installer writes is encrypted, so without this there is nothing to
        // encrypt under — see `Context::cube_pin`. Scoped by Cube id so a stale
        // session can't supply credentials for a different Cube; the restore
        // flows have no Cube yet and get their PIN from `RestorePinSetupStep`
        // instead.
        context.cube_pin = cube_settings
            .as_ref()
            .and_then(|cs| crate::app::session::pin_for(&cs.id));
        // A passkey Cube has no PIN, so the line above is `None` for it *by
        // design* — `session::pin_for` refuses rather than hand back an empty
        // string. Its seed files are encrypted under a key derived from the
        // master seed the unlock assertion produced instead, which the session
        // is already holding and which needs no second authenticator prompt.
        //
        // Deliberately the *same* resolver the read side uses
        // (`Wallet::load_hotsigners`). The two have to agree exactly or the
        // installer writes a file nothing can open again, and one shared
        // definition is the only way to keep that true. Its own guards — right
        // Cube, right master fingerprint — are what stop a stale session from
        // supplying another Cube's seed here.
        //
        // Filtered on shape so a PIN Cube whose session has gone still fails
        // loudly at the seed write, rather than quietly taking some other path.
        context.passkey_seed_password = cube_settings
            .as_ref()
            .filter(|cs| cs.is_passkey_cube())
            .and_then(crate::app::session::seed_file_password);
        // Connect blinding (PR D3): the Vault builder opens Contacts' xpub
        // envelopes with the Cube's seed-derived encryption key. The master
        // signer behind the Breez client is that seed, already unlocked — same
        // source `App` uses for `Cache::cube_encryption_key`.
        context.cube_encryption_key = breez_client
            .as_ref()
            .and_then(|bc| bc.liquid_signer())
            .and_then(|arc| {
                arc.lock().ok().map(|s| {
                    std::sync::Arc::new(
                        crate::services::connect::crypto::CubeEncryptionKey::derive(&s, network),
                    )
                })
            });
        let context = context;

        let mut installer = Installer {
            network,
            datadir: destination_path.clone(),
            current: 0,
            hws: HardwareWallets::new(destination_path.clone(), network),
            launched_from_app,
            cube_settings,
            breez_client,
            spark_backend,
            steps: {
                // Network string used as a filter by the restore step's
                // cube-picker. Must match the Connect API's canonical
                // form (`"mainnet"` for Bitcoin mainnet) — `CubeResponse.network`
                // comes straight from the backend which uses that shape.
                // Using any other form here silently filters out every
                // server-side cube on mainnet.
                let network_str = crate::app::settings::network_to_api_string(network);
                match user_flow {
                    UserFlow::CreateWallet => vec![
                        ChooseDescriptorTemplate::default().into(),
                        DescriptorTemplateDescription::default().into(),
                        DefineDescriptor::new(network, signer.clone()).into(),
                        BackupMnemonic::new(signer.clone()).into(),
                        BackupDescriptor::default().into(),
                        RegisterDescriptor::new_create_wallet().into(),
                        CoincubeConnectStep::new().into(),
                        SelectBitcoindTypeStep::new().into(),
                        InternalBitcoindStep::new(&context.coincube_directory).into(),
                        DefineNode::new(crate::node::NodeType::Esplora).into(),
                        WalletAlias::default().into(),
                        Final::new().into(),
                    ],
                    UserFlow::AddWallet => vec![
                        ChooseBackend::new(network).into(),
                        RemoteBackendLogin::new(
                            network,
                            destination_path.network_directory(network),
                        )
                        .into(),
                        ImportRemoteWallet::new(network).into(),
                        ImportDescriptor::new(network).into(),
                        RecoverMnemonic::default().into(),
                        // W14 — offer to pull the descriptor from Connect
                        // after the user confirms their mnemonic. The step
                        // is skippable; `apply(skipped)` leaves the
                        // context alone so the file-imported descriptor
                        // (from `ImportDescriptor` above) stays in place.
                        RecoveryKitRestoreStep::new(
                            RestoreScope::DescriptorOnly,
                            network_str.clone(),
                            None,
                        )
                        .into(),
                        RegisterDescriptor::new_import_wallet().into(),
                        CoincubeConnectStep::new().into(),
                        SelectBitcoindTypeStep::new().into(),
                        InternalBitcoindStep::new(&context.coincube_directory).into(),
                        DefineNode::default().into(),
                        WalletAlias::default().into(),
                        Final::new().into(),
                    ],
                    // W13 — full restore: the kit carries both the seed
                    // and the descriptor, so the flow skips the editor +
                    // register-descriptor + backup-mnemonic steps that
                    // only make sense for a fresh install.
                    UserFlow::RestoreFromRecoveryKit { cube_uuid } => vec![
                        RecoveryKitRestoreStep::new(
                            RestoreScope::Full,
                            network_str.clone(),
                            cube_uuid,
                        )
                        .into(),
                        // Collect a PIN *after* the kit is decrypted
                        // (so `ctx.recovered_signer` is populated and
                        // the step's `skip()` doesn't short-circuit)
                        // but *before* node setup — the PIN ends up
                        // encrypting the restored mnemonic in
                        // `install_local_wallet` and seeding
                        // `CubeSettings.security_pin_hash` in the
                        // tab-level `CubeSaved` handler, matching the
                        // fresh-install Cube layout so BreezClient
                        // decrypt-on-open works.
                        RestorePinSetupStep::new().into(),
                        CoincubeConnectStep::new().into(),
                        SelectBitcoindTypeStep::new().into(),
                        InternalBitcoindStep::new(&context.coincube_directory).into(),
                        DefineNode::new(crate::node::NodeType::Esplora).into(),
                        WalletAlias::default().into(),
                        Final::new().into(),
                    ],
                    // W15 — running-app Vault restore: the current Cube
                    // already has its seed on disk, we just need the
                    // descriptor. Launched with `launched_from_app = true`
                    // so `previous()` at step 0 returns to App rather
                    // than exiting the installer.
                    UserFlow::RestoreVaultFromRecoveryKit => vec![
                        RecoveryKitRestoreStep::new(
                            RestoreScope::DescriptorOnly,
                            network_str.clone(),
                            None,
                        )
                        .into(),
                        RegisterDescriptor::new_import_wallet().into(),
                        SelectBitcoindTypeStep::new().into(),
                        InternalBitcoindStep::new(&context.coincube_directory).into(),
                        DefineNode::default().into(),
                        WalletAlias::default().into(),
                        Final::new().into(),
                    ],
                    // Heir inheritance recovery (COIN-377 PR 3). Same shape as
                    // the owner restore flows, but `InheritanceRestoreStep`
                    // (ECIES relay) is the decrypt source instead of the
                    // owner-password Recovery-Kit step.
                    UserFlow::RecoverInheritedVault { cube_id, full_cube } => {
                        if full_cube {
                            // Full-Cube: seed + descriptor → a real Cube. Mirror
                            // RestoreFromRecoveryKit (incl. PIN setup for the
                            // restored seed).
                            vec![
                                InheritanceRestoreStep::new(RestoreScope::Full, cube_id).into(),
                                RestorePinSetupStep::new().into(),
                                CoincubeConnectStep::new().into(),
                                SelectBitcoindTypeStep::new().into(),
                                InternalBitcoindStep::new(&context.coincube_directory).into(),
                                DefineNode::new(crate::node::NodeType::Esplora).into(),
                                WalletAlias::default().into(),
                                Final::new().into(),
                            ]
                        } else {
                            // Vault-only: descriptor → watch-only Vault. Mirror
                            // RestoreVaultFromRecoveryKit (descriptor import, no
                            // seed on disk).
                            vec![
                                InheritanceRestoreStep::new(RestoreScope::DescriptorOnly, cube_id)
                                    .into(),
                                RegisterDescriptor::new_import_wallet().into(),
                                SelectBitcoindTypeStep::new().into(),
                                InternalBitcoindStep::new(&context.coincube_directory).into(),
                                DefineNode::default().into(),
                                WalletAlias::default().into(),
                                Final::new().into(),
                            ]
                        }
                    }
                    // Owner self-recovery via Keychain (PLAN-owner-keychain-recovery
                    // PR 3). Same shape as `RecoverInheritedVault`, but
                    // `OwnerKeychainRestoreStep` (owner's own envelope set +
                    // Keychain relay) is the decrypt source instead of the heir
                    // release. No recovery password.
                    UserFlow::RecoverOwnCubeWithPhone { cube_id, full_cube } => {
                        if full_cube {
                            // Full-Cube: seed + descriptor → a real Cube. Mirror
                            // RestoreFromRecoveryKit (incl. PIN setup for the
                            // restored seed).
                            vec![
                                OwnerKeychainRestoreStep::new(RestoreScope::Full, cube_id).into(),
                                RestorePinSetupStep::new().into(),
                                CoincubeConnectStep::new().into(),
                                SelectBitcoindTypeStep::new().into(),
                                InternalBitcoindStep::new(&context.coincube_directory).into(),
                                DefineNode::new(crate::node::NodeType::Esplora).into(),
                                WalletAlias::default().into(),
                                Final::new().into(),
                            ]
                        } else {
                            // Vault-only: descriptor → watch-only Vault. Mirror
                            // RestoreVaultFromRecoveryKit (descriptor import, no
                            // seed on disk).
                            vec![
                                OwnerKeychainRestoreStep::new(
                                    RestoreScope::DescriptorOnly,
                                    cube_id,
                                )
                                .into(),
                                RegisterDescriptor::new_import_wallet().into(),
                                SelectBitcoindTypeStep::new().into(),
                                InternalBitcoindStep::new(&context.coincube_directory).into(),
                                DefineNode::default().into(),
                                WalletAlias::default().into(),
                                Final::new().into(),
                            ]
                        }
                    }
                }
            },
            context,
            signer,
            developer_mode,
        };
        // skip the step according to the current context.
        installer.skip_steps();

        let current_step = installer
            .steps
            .get_mut(installer.current)
            .expect("There is always a step");
        current_step.load_context(&installer.context);
        let command = current_step.load();
        (installer, command)
    }

    pub fn destination_path(&self) -> CoincubeDirectory {
        self.context.coincube_directory.clone()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        self.steps
            .get(self.current)
            .expect("There is always a step")
            .subscription(&self.hws)
    }

    pub fn stop(&mut self) {
        // Use current step's `stop()` method for any changes not yet written to context.
        self.steps
            .get_mut(self.current)
            .expect("There is always a step")
            .stop();
        // Now use context to determine what to stop.
        if let Some(bitcoind) = self.context.internal_bitcoind.take() {
            bitcoind.stop();
        }
    }

    fn skip_steps(&mut self) {
        while self
            .steps
            .get(self.current)
            .expect("There is always a step")
            .skip(&self.context)
        {
            if self.current < self.steps.len() - 1 {
                self.current += 1;
            }
        }
    }

    fn next(&mut self) -> Task<Message> {
        self.hws.reset_watch_list();
        let current_step = self
            .steps
            .get_mut(self.current)
            .expect("There is always a step");
        if current_step.apply(&mut self.context) {
            if self.current < self.steps.len() - 1 {
                self.current += 1;
            } else {
                // The step is already the last current step.
                // No need to reload the current step.
                return Task::none();
            }
            // skip the step according to the current context.
            self.skip_steps();

            // calculate new current_step.
            let current_step = self
                .steps
                .get_mut(self.current)
                .expect("There is always a step");
            current_step.load_context(&self.context);
            return current_step.load();
        }
        Task::none()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::HardwareWallets(msg) => {
                let update = matches!(&msg, &HardwareWalletMessage::List(_));
                match self.hws.update(msg) {
                    Ok(cmd) => {
                        let task_1 = cmd.map(Message::HardwareWallets);
                        let mut task_2 = Task::none();
                        if update {
                            task_2 = self
                                .steps
                                .get_mut(self.current)
                                .expect("There is always a step")
                                .update(
                                    &mut self.hws,
                                    // We notify downstream that the the list have been updated
                                    Message::HardwareWalletUpdate,
                                );
                        }
                        Task::batch(vec![task_1, task_2])
                    }
                    Err(e) => {
                        error!("{}", e);
                        Task::none()
                    }
                }
            }
            Message::Clipboard(s) => clipboard::write(s),
            Message::OpenUrl(url) => {
                if let Err(e) = open::that_detached(&url) {
                    tracing::error!("Error opening '{}': {}", url, e);
                }
                Task::none()
            }
            Message::Next => self.next(),
            Message::Previous => self.previous(),
            Message::Install => {
                let _cmd = self
                    .steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(&mut self.hws, message);
                if let Some(descriptor) = self.context.descriptor.as_ref() {
                    let wallet_id = WalletId::generate(descriptor);
                    let context = self.context.clone();
                    let signer = self.signer.clone();
                    match &self.context.remote_backend {
                        RemoteBackend::WithoutWallet(backend) => Task::perform(
                            with_wallet_id(
                                wallet_id.clone(),
                                create_remote_wallet(context, wallet_id, signer, backend.clone()),
                            ),
                            |(id, res)| Message::Installed(Some(id), res.map(Some)),
                        ),
                        RemoteBackend::WithWallet(backend) => Task::perform(
                            with_wallet_id(
                                wallet_id.clone(),
                                import_remote_wallet(context, wallet_id, backend.clone()),
                            ),
                            |(id, res)| Message::Installed(Some(id), res.map(Some)),
                        ),
                        RemoteBackend::None => Task::perform(
                            with_wallet_id(
                                wallet_id.clone(),
                                install_local_wallet(context, wallet_id, signer),
                            ),
                            |(id, res)| Message::Installed(Some(id), res.map(Some)),
                        ),
                        RemoteBackend::Undefined => unreachable!("Must be defined at this point"),
                    }
                } else {
                    let ctx = self.context.clone();
                    Task::perform(
                        async move {
                            // We must persist the recovered signer so Breez Liquid/Spark
                            // can load it from the datadir on next startup.
                            // Even though there is no vault descriptor, the seed is needed.
                            let recovered = ctx.recovered_signer.as_ref().ok_or_else(|| {
                                Error::Unexpected(
                                    "Seed-only install is missing the recovered signer".into(),
                                )
                            })?;
                            let password = ctx.restore_pin.as_ref().ok_or_else(|| {
                                Error::Unexpected(
                                    "Seed-only install is missing the restore PIN".into(),
                                )
                            })?;

                            persist_seed_only_install(
                                recovered,
                                &ctx.coincube_directory,
                                ctx.bitcoin_config.network,
                                password.as_str(),
                                ctx.seed_cube_id(),
                                seed_device_secret(&ctx)?.as_ref(),
                            )
                        },
                        |res| match res {
                            Ok(_) => Message::Installed(None, Ok(None)),
                            Err(e) => Message::Installed(None, Err(e)),
                        },
                    )
                }
            }
            Message::Installed(wallet_id, Err(e)) => {
                if let Some(wallet_id) = &wallet_id {
                    let network_directory = self
                        .context
                        .coincube_directory
                        .network_directory(self.context.bitcoin_config.network);
                    // In case of failure during install, block the thread to
                    // deleted the data_dir/network directory in order to start clean again.
                    warn!("Installation failed. Cleaning up the network directory.");
                    if let Err(e) = Handle::current()
                        .block_on(delete::delete_failed_install(&network_directory, wallet_id))
                    {
                        error!(
                            "Failed to completely clean the network directory (path: '{}'): {}",
                            network_directory.path().to_string_lossy(),
                            e
                        );
                    } else {
                        warn!(
                            "Successfully cleaned network directory at '{}'.",
                            network_directory.path().to_string_lossy()
                        );
                    }
                }
                self.steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(&mut self.hws, Message::Installed(wallet_id, Err(e)))
            }
            _ => self
                .steps
                .get_mut(self.current)
                .expect("There is always a step")
                .update(&mut self.hws, message),
        }
    }

    /// Some steps are skipped because of contextual choice of the user, this
    /// code is giving a correct progress summary to the user.
    fn progress(&self) -> (usize, usize) {
        let mut current = self.current;
        let mut total = 0;
        for (i, step) in self.steps.iter().enumerate() {
            if step.skip(&self.context) {
                if i < self.current {
                    current -= 1;
                }
            } else {
                total += 1
            }
        }
        (current, total - 1)
    }

    pub fn view(&self) -> Element<Message> {
        let content = self
            .steps
            .get(self.current)
            .expect("There is always a step")
            .view(
                &self.hws,
                self.progress(),
                self.context.remote_backend.user_email(),
            );

        if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        }
    }
}

pub fn daemon_check(cfg: coincubed::config::Config) -> Result<(), Error> {
    // Start Daemon to check correctness of installation
    match coincubed::DaemonHandle::start_default(cfg, false) {
        Ok(daemon) => daemon
            .stop()
            .map_err(|e| Error::Unexpected(format!("Failed to stop Tenshu daemon: {}", e))),
        Err(e) => Err(Error::Unexpected(format!(
            "Failed to start Tenshu daemon: {}",
            e
        ))),
    }
}

async fn with_wallet_id<F>(wallet_id: WalletId, res: F) -> (WalletId, Result<WalletSettings, Error>)
where
    F: std::future::Future<Output = Result<WalletSettings, Error>>,
{
    (wallet_id, res.await)
}

/// User-facing message when the installer has nothing to encrypt a seed under.
///
/// This should be unreachable: every flow that reaches a seed write either ran
/// `RestorePinSetupStep` or was launched from an unlocked Cube, and an unlocked
/// Cube always has one of the two — a session PIN, or (for a passkey Cube,
/// which has no PIN at all) the seed-derived password in
/// [`Context::passkey_seed_password`]. It fails loudly rather than falling
/// back, because the fallback it replaces was writing the mnemonic to disk in
/// the clear (I5).
///
/// Deliberately says "unlock credentials" rather than "PIN": a passkey user
/// told their PIN is missing would go looking for a PIN they never set, and
/// **I12** says a passkey failure must never read as something being wrong with
/// their Cube.
const NO_SEED_PASSWORD_MSG: &str =
    "Can't save this wallet's seed: this Cube's unlock credentials aren't available in \
     this session. Close and re-open the Cube, then try again.";

fn seed_password(ctx: &Context) -> Result<zeroize::Zeroizing<String>, Error> {
    ctx.seed_password()
        .cloned()
        .ok_or_else(|| Error::Unexpected(NO_SEED_PASSWORD_MSG.to_string()))
}

/// This Cube's device secret, so an installer-written seed file is sealed at
/// the same wire version as the Cube's master seed.
///
/// `None` where the Cube has no entry (a pre-v3 Cube, or one being created on a
/// platform without a keystore) — those Cubes stay on v2 and the startup
/// migration upgrades them once a secret exists. A keystore that is present but
/// *unreachable* is a hard error: writing a v2 file next to a v3 one would
/// silently downgrade this Cube's protection.
fn seed_device_secret(
    ctx: &Context,
) -> Result<Option<coincube_core::seed_crypt::DeviceSecret>, Error> {
    let cube_id = ctx.seed_cube_id();
    if cube_id.is_empty() {
        return Ok(None);
    }
    crate::services::unlock::device_secret::load_optional(cube_id).map_err(|e| {
        Error::Unexpected(format!(
            "Couldn't reach your system keychain to save this wallet's seed: {e}"
        ))
    })
}

/// The rescan a restored Vault owes, if any.
///
/// # Why a restore owes a rescan
///
/// `import_descriptor` imports at `timestamp: "now"`. For a Vault being created
/// that is right — there is no history behind it. For one being *restored* it
/// leaves bitcoind scanning from today forward, so every transaction that
/// funded the wallet before the restore is invisible to it while our own
/// database, restored alongside, knows about those coins. The two never
/// reconcile: `get_spender_txid` asks the wallet about a coin's funding
/// transaction, gets `-5`, and the coin can never be resolved as spent — so it
/// stays selectable and the user can build transactions conflicting with their
/// own pending ones.
///
/// # Where the date comes from
///
/// The Recovery Kit already carries it. `backup::Account::timestamp` is the
/// source wallet's birthday, written at backup time from that machine's
/// `get_info().timestamp`, and it is exactly the point the new node has to scan
/// from.
///
/// The **earliest** timestamp across the kit's accounts, because scanning from
/// too early only costs time while scanning from too late is the bug this
/// exists to prevent. `None` when there is no kit (a fresh install) or the kit
/// predates the field — the caller surfaces a rescan prompt rather than
/// inventing a date, since a wrong one would look like a completed rescan that
/// found nothing.
fn pending_rescan(ctx: &Context) -> Option<crate::app::settings::PendingRescan> {
    use crate::app::settings::PendingRescan;

    // No descriptor, no wallet, so no scan window that could fall short. True
    // of a seed-only restore — a Cube that had no Vault when it was backed up —
    // whichever way its birthday arrived, so it is asked first and once.
    ctx.descriptor.as_ref()?;

    // A backup import records the source wallet's birthday, so the scan can
    // start unattended.
    let from_backup = ctx.backup.as_ref().and_then(|backup| {
        backup
            .accounts
            .iter()
            .filter_map(|account| account.timestamp)
            .min()
            .map(|t| <u32 as std::convert::TryFrom<u64>>::try_from(t).unwrap_or(u32::MAX))
    });
    if let Some(t) = from_backup {
        return Some(PendingRescan::From(t));
    }

    // A fresh install has a descriptor too, but no history behind it.
    ctx.restore_source?;

    // A Recovery Kit written since `DescriptorBlobVault::birthday` existed
    // carries the Vault's creation time and can start unattended too.
    //
    // An older kit carries nothing that dates the wallet, so the rescan is
    // recorded as owed *without* a date and the user supplies one. Inventing a
    // date would be worse than asking: too late and the scan finds nothing
    // while presenting as complete.
    Some(
        ctx.restored_wallet_birthday
            .map(PendingRescan::From)
            .unwrap_or(PendingRescan::DateUnknown),
    )
}

pub async fn install_local_wallet(
    ctx: Context,
    wallet_id: WalletId,
    signer: Arc<Mutex<Signer>>,
) -> Result<WalletSettings, Error> {
    let network_datadir = ctx
        .coincube_directory
        .network_directory(ctx.bitcoin_config.network);
    network_datadir
        .init()
        .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;

    let descriptor = ctx
        .descriptor
        .as_ref()
        .expect("Context must have a descriptor at this point");

    let hardware_wallets = ctx
        .hws
        .iter()
        .filter_map(|(kind, fingerprint, token)| {
            token
                .as_ref()
                .map(|token| HardwareWalletConfig::new(kind, *fingerprint, token))
        })
        .collect();

    let wallet_settings = WalletSettings {
        name: wallet_name(descriptor),
        alias: Some(ctx.wallet_alias.clone()),
        pinned_at: wallet_id.timestamp,
        descriptor_checksum: wallet_id.descriptor_checksum.clone(),
        keys: ctx.keys.values().cloned().collect(),
        hardware_wallets,
        remote_backend_auth: None,
        start_internal_bitcoind: Some(ctx.internal_bitcoind.is_some()),
        // A Recovery-Kit restore lands its descriptors in a watchonly wallet
        // that has never seen them; `App` turns this into an actual rescan once
        // the daemon is up, and clears it when the daemon accepts one.
        pending_rescan: pending_rescan(&ctx),
    };

    let cfg: coincubed::config::Config = extract_daemon_config(&ctx, &wallet_settings)?;

    daemon_check(cfg.clone())?;

    info!("daemon checked");

    // Step needed because of ValueAfterTable error in the toml serialize implementation.
    let daemon_config_toml = toml::to_string_pretty(&cfg)
        .map_err(|e| Error::Unexpected(format!("Failed to serialize daemon config: {}", e)))?;

    // create coincubed configuration file
    create_and_write_file(
        &network_datadir
            .coincubed_data_directory(&wallet_settings.wallet_id())
            .path()
            .join("daemon.toml"),
        daemon_config_toml.as_bytes(),
    )?;

    info!("Daemon configuration file created");

    if cfg
        .main_descriptor
        .to_string()
        .contains(&signer.lock().unwrap().fingerprint().to_string())
    {
        // In developer mode this signer is a *clone of the Cube master signer*
        // (see `Installer::new`), so the pre-hardening `store(...)` here was
        // writing the master seed itself to disk in the clear. It gets the same
        // treatment as every other seed, not an exemption.
        let password = seed_password(&ctx)?;
        signer
            .lock()
            .unwrap()
            .store_encrypted(
                &ctx.coincube_directory,
                cfg.bitcoin_config.network,
                &wallet_id.descriptor_checksum,
                wallet_id
                    .timestamp
                    .expect("Every new wallet have now a timestamp"),
                password.as_str(),
                ctx.seed_cube_id(),
                seed_device_secret(&ctx)?.as_ref(),
            )
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Master signer mnemonic stored (encrypted)");
    }

    if let Some(signer) = &ctx.recovered_signer {
        let timestamp = wallet_id
            .timestamp
            .expect("Every new wallet have now a timestamp");
        // Recovery Kit restore: encrypt with the PIN the user chose in
        // `RestorePinSetupStep` so the on-disk layout matches what a
        // fresh-install Cube produces. The `RestoreVaultFromRecoveryKit` /
        // legacy AddWallet flows have no `restore_pin` but do run inside an
        // already-open Cube, so they fall through to the session PIN — which is
        // the same PIN that Cube's other seed files already use.
        let password = seed_password(&ctx)?;
        signer
            .store_encrypted(
                &ctx.coincube_directory,
                cfg.bitcoin_config.network,
                &wallet_id.descriptor_checksum,
                timestamp,
                password.as_str(),
                ctx.seed_cube_id(),
                seed_device_secret(&ctx)?.as_ref(),
            )
            .map_err(|e| Error::Unexpected(format!("Failed to store encrypted mnemonic: {}", e)))?;
        info!("Recovered signer mnemonic stored (PIN-encrypted)");
    }

    // create coincube GUI configuration file
    // Installer started a bitcoind, it is expected that gui will start it on startup
    ensure_gui_config(&network_datadir, ctx.internal_bitcoind.is_some())?;

    // create coincube GUI settings file
    update_settings_file(&network_datadir, |mut settings| {
        settings.wallets.push(wallet_settings.clone());
        Some(settings)
    })
    .await
    .map_err(|e| Error::Unexpected(e.to_string()))?;

    info!("Settings file created");

    Ok(wallet_settings)
}

pub async fn create_remote_wallet(
    ctx: Context,
    wallet_id: WalletId,
    signer: Arc<Mutex<Signer>>,
    remote_backend: BackendClient,
) -> Result<WalletSettings, Error> {
    let network_datadir = ctx.coincube_directory.network_directory(ctx.network);
    network_datadir
        .init()
        .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;

    let descriptor = ctx
        .descriptor
        .as_ref()
        .expect("There must be a descriptor at this point");

    if descriptor
        .to_string()
        .contains(&signer.lock().unwrap().fingerprint().to_string())
    {
        let password = seed_password(&ctx)?;
        signer
            .lock()
            .unwrap()
            .store_encrypted(
                &ctx.coincube_directory,
                ctx.network,
                &wallet_id.descriptor_checksum,
                wallet_id
                    .timestamp
                    .expect("Every new wallet have now a timestamp"),
                password.as_str(),
                ctx.seed_cube_id(),
                seed_device_secret(&ctx)?.as_ref(),
            )
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Master signer mnemonic stored (encrypted)");
    }

    if let Some(signer) = &ctx.recovered_signer {
        let password = seed_password(&ctx)?;
        signer
            .store_encrypted(
                &ctx.coincube_directory,
                ctx.network,
                &wallet_id.descriptor_checksum,
                wallet_id
                    .timestamp
                    .expect("Every new wallet have now a timestamp"),
                password.as_str(),
                ctx.seed_cube_id(),
                seed_device_secret(&ctx)?.as_ref(),
            )
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Recovered signer mnemonic stored (encrypted)");
    }

    // create coincube GUI configuration file
    ensure_gui_config(&network_datadir, false)?;

    let pks: Vec<_> = ctx
        .keys
        .values()
        .filter_map(|key| {
            key.provider_key.as_ref().map(|pk| ProviderKey {
                fingerprint: key.master_fingerprint.to_string(),
                uuid: pk.uuid.clone(),
                token: pk.token.clone(),
                provider: Provider {
                    uuid: pk.provider.uuid.clone(),
                    name: pk.provider.name.clone(),
                },
            })
        })
        .collect();
    let wallet = remote_backend
        .create_wallet(&wallet_name(descriptor), descriptor, &pks)
        .await
        .map_err(|e| Error::Unexpected(e.to_string()))?;

    let hws: Vec<HardwareWalletConfig> = ctx
        .hws
        .iter()
        .filter_map(|(kind, fingerprint, token)| {
            token
                .as_ref()
                .map(|token| HardwareWalletConfig::new(kind, *fingerprint, token))
        })
        .collect();
    let descriptor_str = descriptor.to_string();
    let aliases = ctx
        .keys
        .values()
        .filter_map(|k| {
            if descriptor_str.contains(&k.master_fingerprint.to_string()) {
                Some((k.master_fingerprint, k.name.to_string()))
            } else {
                None
            }
        })
        .collect();
    remote_backend
        .update_wallet_metadata(&wallet.id, Some(ctx.wallet_alias.clone()), &aliases, &hws)
        .await
        .map_err(|e| Error::Unexpected(e.to_string()))?;

    let remote_backend = remote_backend.connect_wallet(wallet).0;

    // create coincube GUI settings file
    // if the wallet is using the remote backend, then the hardware wallet settings and
    // keys will be store on the remote backend side and not in the settings file.
    let wallet_settings = WalletSettings {
        name: wallet_name(descriptor),
        alias: Some(ctx.wallet_alias.clone()),
        descriptor_checksum: wallet_id.descriptor_checksum,
        pinned_at: wallet_id.timestamp,
        keys: Vec::new(),
        hardware_wallets: Vec::new(),
        remote_backend_auth: Some(AuthConfig::new(
            remote_backend.user_email().to_string(),
            remote_backend.wallet_id(),
        )),
        start_internal_bitcoind: None,
        // Remote backend: no local node, so no scan window to fall short.
        pending_rescan: None,
    };
    update_settings_file(&network_datadir, |mut settings| {
        settings.wallets.push(wallet_settings.clone());
        Some(settings)
    })
    .await
    .map_err(|e| Error::Unexpected(e.to_string()))?;

    info!("Settings file created");

    let backend = remote_backend.inner_client();
    if let Err(e) = update_connect_cache(
        &network_datadir,
        backend.auth.read().await.deref(),
        backend.auth_client(),
        false,
        true,
    )
    .await
    {
        // this error is not critical, the liana-connect backend stored the wallet
        // and user can reauthenticate.
        tracing::error!("Failed to update Liana-Connect cache: {}", e);
    } else {
        info!("Liana-Connect cache updated");
    }

    Ok(wallet_settings)
}

pub async fn import_remote_wallet(
    ctx: Context,
    wallet_id: WalletId,
    backend: BackendWalletClient,
) -> Result<WalletSettings, Error> {
    tracing::info!("Importing wallet from remote backend");

    if let Some(signer) = &ctx.recovered_signer {
        let password = seed_password(&ctx)?;
        signer
            .store_encrypted(
                &ctx.coincube_directory,
                ctx.network,
                &wallet_id.descriptor_checksum,
                wallet_id
                    .timestamp
                    .expect("Every new wallet have now a timestamp"),
                password.as_str(),
                ctx.seed_cube_id(),
                seed_device_secret(&ctx)?.as_ref(),
            )
            .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;

        info!("Recovered signer mnemonic stored (encrypted)");
    }

    let network_datadir = ctx.coincube_directory.network_directory(ctx.network);
    network_datadir
        .init()
        .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;

    backend
        .update_wallet_metadata(Some(ctx.wallet_alias.clone()), &HashMap::new(), &[])
        .await?;

    // create coincube GUI settings file
    // if the wallet is using the remote backend, then the hardware wallet settings and
    // keys will be store on the remote backend side and not in the settings file.
    let wallet_settings = WalletSettings {
        name: wallet_name(
            ctx.descriptor
                .as_ref()
                .expect("Context must have a descriptor at this point"),
        ),
        alias: Some(ctx.wallet_alias.clone()),
        descriptor_checksum: wallet_id.descriptor_checksum,
        pinned_at: wallet_id.timestamp,
        keys: Vec::new(),
        hardware_wallets: Vec::new(),
        remote_backend_auth: Some(AuthConfig::new(
            backend.user_email().to_string(),
            backend.wallet_id(),
        )),
        start_internal_bitcoind: None,
        // Remote backend: no local node, so no scan window to fall short.
        pending_rescan: None,
    };
    update_settings_file(&network_datadir, |mut settings| {
        settings.wallets.push(wallet_settings.clone());
        Some(settings)
    })
    .await
    .map_err(|e| Error::Unexpected(e.to_string()))?;

    info!("Settings file created");

    // create coincube GUI configuration file
    ensure_gui_config(&network_datadir, false)?;

    let backend = backend.inner_client();
    if let Err(e) = update_connect_cache(
        &network_datadir,
        backend.auth.read().await.deref(),
        backend.auth_client(),
        false,
        true,
    )
    .await
    {
        // this error is not critical, the liana-connect backend stored the wallet
        // and user can reauthenticate.
        tracing::error!("Failed to update Liana-Connect cache: {}", e);
    } else {
        info!("Liana-Connect cache updated");
    }

    Ok(wallet_settings)
}

/// Persist a seed-only (Vault-less) install: store the recovered signer's
/// mnemonic encrypted under the restore PIN, then make sure `gui.toml` exists
/// so the `CubeSaved` finish line has a config to load.
///
/// On a retried restore the encrypted seed file may already be on disk
/// (`AlreadyExists`). That's only acceptable when the existing file decrypts to
/// the *same* master-signer fingerprint under the *same* PIN — verified via
/// `from_datadir_by_fingerprint`. A mismatch means the on-disk seed conflicts
/// with the new recovery credentials, so we surface an error rather than
/// silently continuing against a seed the new PIN can't open.
///
/// Extracted from the inline installer closure so the seed-conflict arm is unit
/// testable.
fn persist_seed_only_install(
    recovered: &Signer,
    coincube_directory: &CoincubeDirectory,
    network: Network,
    password: &str,
    cube_id: &str,
    device_secret: Option<&coincube_core::seed_crypt::DeviceSecret>,
) -> Result<(), Error> {
    if let Err(e) = recovered.store_encrypted_seed_only(
        coincube_directory,
        network,
        password,
        cube_id,
        device_secret,
    ) {
        match e {
            coincube_core::signer::SignerError::MnemonicStorage(ref io_err)
                if io_err.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                if let Err(verify_err) =
                    coincube_core::signer::MasterSigner::from_datadir_by_fingerprint(
                        coincube_directory.path(),
                        network,
                        recovered.fingerprint(),
                        Some(password),
                        cube_id,
                    )
                {
                    return Err(Error::Unexpected(format!(
                        "Existing seed file conflicted with new recovery PIN or was invalid: {}",
                        verify_err
                    )));
                }
                log::info!(
                    "Seed already exists on disk from a previous attempt and matches. Continuing."
                );
            }
            _ => {
                return Err(Error::Unexpected(format!("Failed to store seed: {}", e)));
            }
        }
    }

    // Write `gui.toml` ourselves — the three wallet installers create it, but
    // the seed-only path never did, so a fresh (non-post-wipe) datadir reached
    // the `CubeSaved` finish line with no config and panicked. Seed-only cubes
    // run no managed bitcoind, so `start_internal_bitcoind` is false.
    let network_datadir = coincube_directory.network_directory(network);
    network_datadir
        .init()
        .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;
    ensure_gui_config(&network_datadir, false)?;

    Ok(())
}

/// Write the COINCUBE GUI configuration file (`gui.toml`) into the network
/// datadir if it isn't already present. Shared by every install path — the
/// three wallet installers and the seed-only restore — so a missing `gui.toml`
/// can never abort the finish line (see the seed-only `CubeSaved` panic).
///
/// `start_internal_bitcoind` records whether the installer launched a managed
/// bitcoind that the GUI is expected to start on boot; seed-only and remote
/// installs pass `false`.
pub fn ensure_gui_config(
    network_datadir: &NetworkDirectory,
    start_internal_bitcoind: bool,
) -> Result<(), Error> {
    let gui_config_path = network_datadir
        .path()
        .join(gui_config::DEFAULT_FILE_NAME)
        .to_path_buf();
    if !gui_config_path.exists() {
        create_and_write_file(
            &gui_config_path,
            toml::to_string(&gui_config::Config::new(start_internal_bitcoind))
                .map_err(|e| Error::Unexpected(format!("Failed to serialize gui config: {}", e)))?
                .as_bytes(),
        )?;
        info!("Gui configuration file created");
    }
    Ok(())
}

pub fn create_and_write_file(path: &Path, data: &[u8]) -> Result<(), Error> {
    let mut file =
        std::fs::File::create(path).map_err(|e| Error::CannotCreateFile(e.to_string()))?;
    file.write_all(data)
        .map_err(|e| Error::CannotWriteToFile(e.to_string()))?;
    Ok(())
}

pub fn extract_daemon_config(ctx: &Context, settings: &WalletSettings) -> Result<Config, Error> {
    let data_directory = ctx
        .coincube_directory
        .network_directory(ctx.bitcoin_config.network)
        .coincubed_data_directory(&settings.wallet_id());
    data_directory
        .init()
        .map_err(|e| Error::CannotCreateDatadir(e.to_string()))?;

    let data_directory = data_directory
        .path()
        .to_path_buf()
        .canonicalize()
        .map_err(|e| Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e)))?;
    let bitcoin_backend = if let Some(BitcoinBackend::Bitcoind(BitcoindConfig {
        rpc_auth: BitcoindRpcAuth::CookieFile(cookie_path),
        addr,
    })) = &ctx.bitcoin_backend
    {
        // The cookie path must exist for this canonicalization to succeed, which means bitcoind must be running.
        // We already checked in the installer that bitcoind is running.
        let cookie_path = cookie_path
            .canonicalize()
            .map_err(|e| Error::Unexpected(format!("Failed to canonicalize cookie path: {}", e)))?;
        Some(BitcoinBackend::Bitcoind(BitcoindConfig {
            rpc_auth: BitcoindRpcAuth::CookieFile(cookie_path),
            addr: *addr,
        }))
    } else {
        ctx.bitcoin_backend.clone()
    };
    let mut cfg = Config::new(
        ctx.bitcoin_config.clone(),
        bitcoin_backend,
        log::LevelFilter::Info,
        ctx.descriptor
            .clone()
            .expect("Context must have a descriptor at this point"),
        coincubed::datadir::DataDirectory::new(data_directory),
    );
    cfg.pending_bitcoind = ctx.pending_bitcoind_config.clone();
    // The user installed a node alongside Connect: adopt it (auto-switch) once
    // it's synced, even if it reused a chainstate and never entered IBD.
    cfg.auto_switch_to_pending = Some(ctx.install_node_alongside_connect);
    Ok(cfg)
}

#[derive(Debug, Clone)]
pub enum Error {
    Auth(AuthError),
    // DaemonError does not implement Clone.
    // TODO: maybe Arc is overkill
    Backend(Arc<DaemonError>),
    Services(services::keys::Error),
    Settings(SettingsError),
    Bitcoind(String),
    Electrum(String),
    Esplora(String),
    CannotCreateDatadir(String),
    CannotCreateFile(String),
    CannotWriteToFile(String),
    CannotGetAvailablePort(String),
    Unexpected(String),
    HardwareWallet(async_hwi::Error),
    Backup(encrypted_backup::Error),
}

impl From<jsonrpc::simple_http::Error> for Error {
    fn from(error: jsonrpc::simple_http::Error) -> Self {
        Error::Bitcoind(error.to_string())
    }
}

impl From<jsonrpc::Error> for Error {
    fn from(error: jsonrpc::Error) -> Self {
        Error::Bitcoind(error.to_string())
    }
}

impl From<async_hwi::Error> for Error {
    fn from(error: async_hwi::Error) -> Self {
        Error::HardwareWallet(error)
    }
}

impl From<DaemonError> for Error {
    fn from(value: DaemonError) -> Self {
        Self::Backend(Arc::new(value))
    }
}

impl From<AuthError> for Error {
    fn from(value: AuthError) -> Self {
        Self::Auth(value)
    }
}

impl From<SettingsError> for Error {
    fn from(value: SettingsError) -> Self {
        Self::Settings(value)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Auth(e) => write!(f, "Authentication error: {}", e),
            Self::Backend(e) => write!(f, "Remote backend error: {}", e),
            Self::Services(e) => write!(f, "Services error: {}", e),
            Self::Settings(e) => write!(f, "Settings file error: {}", e),
            Self::Bitcoind(e) => write!(f, "Failed to ping bitcoind: {}", e),
            Self::Electrum(e) => write!(f, "Failed to ping Electrum: {}", e),
            Self::Esplora(e) => write!(f, "Failed to ping Esplora: {}", e),
            Self::CannotCreateDatadir(e) => write!(f, "Failed to create datadir: {}", e),
            Self::CannotGetAvailablePort(e) => write!(f, "Failed to get available port: {}", e),
            Self::CannotWriteToFile(e) => write!(f, "Failed to write to file: {}", e),
            Self::CannotCreateFile(e) => write!(f, "Failed to create file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected: {}", e),
            Self::HardwareWallet(e) => write!(f, "Hardware Wallet: {}", e),
            Self::Backup(e) => write!(f, "Backup: {:?}", e),
        }
    }
}

#[cfg(test)]
mod pending_rescan_tests {
    //! What rescan, if any, a newly installed Vault owes.

    use super::*;
    use crate::app::settings::PendingRescan;
    use crate::backup::{Account, Backup};

    fn ctx() -> Context {
        Context::new(
            bitcoin::Network::Bitcoin,
            CoincubeDirectory::new(std::path::PathBuf::from("/nonexistent")),
            RemoteBackend::None,
            None,
            None,
        )
    }

    /// A backup-import context. The descriptor is part of the fixture because
    /// the import step always sets one — there is no way to import a backup
    /// without the descriptor it describes — and `pending_rescan` requires it.
    fn with_backup(accounts: Vec<Account>) -> Context {
        let mut ctx = ctx();
        ctx.descriptor = staged_with_descriptor(None).descriptor;
        ctx.backup = Some(Backup {
            name: None,
            alias: None,
            accounts,
            network: bitcoin::Network::Bitcoin,
            date: None,
            proprietary: serde_json::Map::new(),
            version: 0,
        });
        ctx
    }

    fn staged_with_descriptor(
        birthday: Option<u32>,
    ) -> crate::installer::step::recovery_kit_restore::StagedRestore {
        crate::installer::step::recovery_kit_restore::StagedRestore {
            descriptor: Some(
                "wsh(or_d(multi(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[de6eb005/48'/1'/0'/2']tpubDFGuYfS2JwiUSEXiQuNGdT3R7WTDhbaE6jbUhgYSSdhmfQcSx7ZntMPPv7nrkvAqjpj3jX9wbhSGMeKVao4qAzhbNyBi7iQmv5xxQk6H6jz/<0;1>/*),and_v(v:pkh([ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*),older(3))))#p9ax3xxp"
                    .parse()
                    .expect("fixture descriptor must parse"),
            ),
            signer: None,
            birthday,
        }
    }

    fn account(timestamp: Option<u64>) -> Account {
        let mut a = Account::new("wsh(pk(xpub))".to_string());
        a.timestamp = timestamp;
        a
    }

    /// A fresh install owes nothing — scanning from now is correct there, and a
    /// spurious rescan is a long, pointless wait.
    #[test]
    fn a_fresh_install_owes_no_rescan() {
        assert_eq!(pending_rescan(&ctx()), None);
    }

    /// A backup import records the source wallet's birthday, so the scan can
    /// start unattended.
    #[test]
    fn a_backup_import_owes_a_dated_rescan() {
        assert_eq!(
            pending_rescan(&with_backup(vec![account(Some(1_784_953_848))])),
            Some(PendingRescan::From(1_784_953_848))
        );
    }

    /// Earliest wins. Too early only costs scan time; too late silently misses
    /// history, which is the whole bug.
    #[test]
    fn the_earliest_account_birthday_wins() {
        assert_eq!(
            pending_rescan(&with_backup(vec![
                account(Some(1_787_372_770)),
                account(Some(1_784_953_848)),
                account(None),
            ])),
            Some(PendingRescan::From(1_784_953_848))
        );
    }

    /// The flow that actually caused this: a Recovery Kit carries no birthday
    /// anywhere in `DescriptorBlobVault`, so the rescan is owed with no date
    /// rather than not owed at all. Reading "no date" as "nothing to do" is the
    /// bug — it leaves a wallet that silently cannot see its own history.
    #[test]
    fn a_recovery_kit_restore_owes_an_undated_rescan() {
        let mut ctx = ctx();
        staged_with_descriptor(None).commit(&mut ctx, RestoreSource::PasswordKit);
        assert_eq!(pending_rescan(&ctx), Some(PendingRescan::DateUnknown));
        assert_eq!(pending_rescan(&ctx).unwrap().timestamp(), None);
    }

    /// Every restore path must record the rescan, not just the Recovery Kit
    /// one.
    ///
    /// `owner_keychain_restore` and `inheritance_restore` install a descriptor
    /// that already has on-chain history, exactly as a kit restore does, but
    /// committed it by hand and recorded nothing — so those Vaults came up
    /// unable to see their own past with no prompt. All three now commit
    /// through `StagedRestore::commit`, which is what this pins.
    #[test]
    fn every_restore_path_records_the_rescan_it_owes() {
        use crate::installer::step::recovery_kit_restore::StagedRestore;

        let stage_descriptor = |ctx: &mut Context| {
            staged_with_descriptor(Some(1_784_953_848)).commit(ctx, RestoreSource::PasswordKit);
        };

        let mut dated = ctx();
        assert_eq!(pending_rescan(&dated), None, "nothing staged yet");

        stage_descriptor(&mut dated);
        assert!(
            dated.restore_source.is_some(),
            "the descriptor carries history"
        );
        assert_eq!(
            pending_rescan(&dated),
            Some(PendingRescan::From(1_784_953_848))
        );

        // The same commit with no birthday still owes a rescan — just an
        // undated one. Reading "no date" as "nothing to do" is the bug.
        let mut empty = ctx();
        StagedRestore {
            descriptor: None,
            signer: None,
            birthday: None,
        }
        .commit(&mut empty, RestoreSource::PasswordKit);
        assert!(
            empty.restore_source.is_none(),
            "no descriptor means nothing was restored"
        );
        assert_eq!(pending_rescan(&empty), None);
    }

    /// Backing out of a restore must take the rescan with it.
    ///
    /// Every restore step's `revert` drops `ctx.descriptor`; the rescan
    /// metadata is derived from that same descriptor and has to go with it.
    /// Left behind, a user who backs out and completes a *fresh* install gets a
    /// Vault marked as owing a rescan it does not owe — and an inherited
    /// birthday would start a multi-hour scan of a chain the new wallet has no
    /// history on.
    #[test]
    fn backing_out_of_a_restore_clears_the_rescan_it_staged() {
        use crate::installer::step::recovery_kit_restore::StagedRestore;

        let mut ctx = ctx();
        staged_with_descriptor(Some(1_784_953_848)).commit(&mut ctx, RestoreSource::PasswordKit);
        assert_eq!(
            pending_rescan(&ctx),
            Some(PendingRescan::From(1_784_953_848))
        );

        StagedRestore::revert_commit(&mut ctx);
        assert!(ctx.descriptor.is_none());
        assert_eq!(
            pending_rescan(&ctx),
            None,
            "a discarded restore leaves no rescan behind"
        );
    }

    /// `commit` assigns the rescan fields rather than only setting them when
    /// present, so re-applying with nothing staged cannot leave a mixture.
    ///
    /// `restored_wallet_birthday` was already unconditional while
    /// `restore_source` was not — a re-apply staging nothing kept the source
    /// and dropped the date, turning a dated rescan into an undated one.
    #[test]
    fn re_committing_with_nothing_staged_clears_rather_than_mixes() {
        use crate::installer::step::recovery_kit_restore::StagedRestore;

        let mut ctx = ctx();
        staged_with_descriptor(Some(1_784_953_848)).commit(&mut ctx, RestoreSource::PasswordKit);
        assert!(ctx.restore_source.is_some());

        StagedRestore {
            descriptor: None,
            signer: None,
            birthday: None,
        }
        .commit(&mut ctx, RestoreSource::PasswordKit);

        assert_eq!(ctx.restore_source, None, "no descriptor, no restore");
        assert_eq!(ctx.restored_wallet_birthday, None);
        assert_eq!(pending_rescan(&ctx), None);
    }

    /// A seed-only kit restores a Cube with no Vault: provenance yes, rescan no.
    ///
    /// `RestoreScope::Full` needs only `has_encrypted_seed`, so a Cube that had
    /// no Vault when it was backed up is a legitimate Full restore that stages a
    /// signer and no descriptor. Two things follow, and they pull opposite ways:
    /// the Home card must know a Recovery Kit exists (it does — the restore came
    /// out of one), while nothing owes a rescan, because there is no descriptor
    /// and so no wallet whose scan window could fall short. Recording an undated
    /// rescan for a Vault that does not exist would raise a prompt the user
    /// cannot act on.
    #[test]
    fn a_seed_only_restore_keeps_provenance_without_owing_a_rescan() {
        use crate::installer::step::recovery_kit_restore::StagedRestore;

        let mut ctx = ctx();
        StagedRestore {
            descriptor: None,
            signer: Some(crate::signer::Signer::generate(bitcoin::Network::Bitcoin).unwrap()),
            birthday: None,
        }
        .commit(&mut ctx, RestoreSource::PasswordKit);

        assert_eq!(
            ctx.restore_source,
            Some(RestoreSource::PasswordKit),
            "a seed-only kit is still a Recovery Kit"
        );
        assert!(ctx.recovered_signer.is_some());
        assert!(ctx.descriptor.is_none());
        assert_eq!(
            pending_rescan(&ctx),
            None,
            "no descriptor means no wallet to rescan"
        );
    }

    /// A stray birthday must not survive a restore that brought no descriptor.
    ///
    /// Birthdays come off the descriptor blob, so this cannot happen through
    /// `stage_restore` today — the assertion is what keeps that true if it ever
    /// could, since a birthday with no descriptor would be a dated rescan for a
    /// Vault that does not exist.
    #[test]
    fn a_seed_only_restore_drops_any_birthday() {
        use crate::installer::step::recovery_kit_restore::StagedRestore;

        let mut ctx = ctx();
        StagedRestore {
            descriptor: None,
            signer: None,
            birthday: Some(1_784_953_848),
        }
        .commit(&mut ctx, RestoreSource::PasswordKit);

        assert_eq!(ctx.restored_wallet_birthday, None);
        assert_eq!(pending_rescan(&ctx), None);
    }

    /// Every restore owes a rescan; only one of them evidences a password
    /// Recovery Kit.
    ///
    /// These are separate questions with different answers, and collapsing them
    /// to one bool made an owner-keychain restore — and, worse, an heir
    /// restoring somebody else's Vault — set
    /// `recovery_kit_password_backed_up`, claiming a password kit that does not
    /// exist. The keychain seal keeps its own record; an heir has no kit at all.
    #[test]
    fn only_a_password_kit_evidences_a_password_kit() {
        for source in [
            RestoreSource::PasswordKit,
            RestoreSource::KeychainSeal,
            RestoreSource::Inheritance,
        ] {
            let mut ctx = ctx();
            staged_with_descriptor(None).commit(&mut ctx, source);
            assert!(
                pending_rescan(&ctx).is_some(),
                "{:?} restores a descriptor with history, so it owes a rescan",
                source
            );
        }

        // The badge gate is the narrow one. Mirrors the `matches!` in the
        // installer-exit handler, so a future widening of `restore_source` has
        // to come past this.
        let evidences_password_kit = |s: RestoreSource| matches!(s, RestoreSource::PasswordKit);
        assert!(evidences_password_kit(RestoreSource::PasswordKit));
        assert!(!evidences_password_kit(RestoreSource::KeychainSeal));
        assert!(!evidences_password_kit(RestoreSource::Inheritance));
    }

    /// A kit written since `DescriptorBlobVault::birthday` existed dates its
    /// own restore, so the scan starts unattended like a backup import does.
    #[test]
    fn a_kit_with_a_birthday_dates_its_own_rescan() {
        let mut ctx = ctx();
        staged_with_descriptor(Some(1_784_953_848)).commit(&mut ctx, RestoreSource::PasswordKit);
        assert_eq!(
            pending_rescan(&ctx),
            Some(PendingRescan::From(1_784_953_848))
        );
    }

    /// A kit whose install also carried a dated backup prefers the date — it
    /// can start unattended, which is strictly better than prompting.
    #[test]
    fn a_known_birthday_beats_an_undated_kit() {
        let mut ctx = with_backup(vec![account(Some(1_784_953_848))]);
        staged_with_descriptor(None).commit(&mut ctx, RestoreSource::PasswordKit);
        assert_eq!(
            pending_rescan(&ctx),
            Some(PendingRescan::From(1_784_953_848))
        );
    }

    /// A backup with no birthday and no kit flag is not a restore we can speak
    /// to: no date, and no evidence a rescan is owed.
    #[test]
    fn a_backup_without_a_birthday_owes_nothing_on_its_own() {
        assert_eq!(pending_rescan(&with_backup(vec![account(None)])), None);
        assert_eq!(pending_rescan(&with_backup(vec![])), None);
    }
}

#[cfg(test)]
mod seed_only_install_tests {
    //! Unit tests for the seed-only (Vault-less) restore persistence:
    //! `gui.toml` is written on every path (the missing-config finish-line
    //! panic), and a retried restore reconciles the on-disk seed against the
    //! recovery PIN's fingerprint rather than clobbering or blindly trusting it.
    use super::*;

    fn temp_coincube_dir(tag: &str) -> CoincubeDirectory {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "coincube-seed-only-{}-{}-{}",
            tag,
            std::process::id(),
            seq
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        CoincubeDirectory::new(path)
    }

    #[test]
    fn ensure_gui_config_writes_and_is_idempotent() {
        let dir = temp_coincube_dir("ensure-gui");
        let nd = dir.network_directory(Network::Bitcoin);
        nd.init().unwrap();

        // Fresh network dir → file created, parses, and (seed-only) does not
        // start an internal bitcoind.
        ensure_gui_config(&nd, false).unwrap();
        let cfg_path = nd.path().join(gui_config::DEFAULT_FILE_NAME);
        assert!(cfg_path.exists(), "gui.toml is written");
        let cfg = gui_config::Config::from_file(&cfg_path).expect("gui.toml parses");
        assert!(!cfg.start_internal_bitcoind);

        // Existing file → left untouched, even when called with a different
        // flag (mirrors the `if !exists` guard).
        ensure_gui_config(&nd, true).unwrap();
        let cfg = gui_config::Config::from_file(&cfg_path).expect("gui.toml still parses");
        assert!(
            !cfg.start_internal_bitcoind,
            "existing gui.toml must not be overwritten"
        );
    }

    #[test]
    fn persist_seed_only_install_stores_seed_and_writes_gui_config() {
        let dir = temp_coincube_dir("persist");
        let signer = Signer::generate(Network::Bitcoin).unwrap();

        persist_seed_only_install(&signer, &dir, Network::Bitcoin, "246810", "cube-a", None)
            .expect("seed-only persistence should succeed on a fresh datadir");

        let nd = dir.network_directory(Network::Bitcoin);
        assert!(
            nd.path().join(gui_config::DEFAULT_FILE_NAME).exists(),
            "gui.toml is written on the seed-only path"
        );
        // The encrypted seed is on disk and decrypts under the restore PIN.
        coincube_core::signer::MasterSigner::from_datadir_by_fingerprint(
            dir.path(),
            Network::Bitcoin,
            signer.fingerprint(),
            Some("246810"),
            "cube-a",
        )
        .expect("stored seed decrypts under the restore PIN");
    }

    #[test]
    fn persist_seed_only_install_retry_with_matching_pin_continues() {
        let dir = temp_coincube_dir("retry-match");
        let signer = Signer::generate(Network::Bitcoin).unwrap();

        // First attempt stores the seed; a retry (same signer + PIN, e.g. after
        // a mid-flow kill) hits `AlreadyExists` and must succeed because the
        // on-disk seed verifies against the recovery fingerprint.
        persist_seed_only_install(&signer, &dir, Network::Bitcoin, "246810", "cube-a", None)
            .unwrap();
        persist_seed_only_install(&signer, &dir, Network::Bitcoin, "246810", "cube-a", None)
            .expect("retry with the matching PIN continues past AlreadyExists");
    }

    #[test]
    fn persist_seed_only_install_conflicting_pin_errors() {
        let dir = temp_coincube_dir("conflict");
        let signer = Signer::generate(Network::Bitcoin).unwrap();

        // Seed stored under one PIN; retry under a *different* PIN hits
        // `AlreadyExists`, the fingerprint verification fails to decrypt, and we
        // surface an actionable conflict error rather than continuing.
        persist_seed_only_install(&signer, &dir, Network::Bitcoin, "246810", "cube-a", None)
            .unwrap();
        let err =
            persist_seed_only_install(&signer, &dir, Network::Bitcoin, "999999", "cube-a", None)
                .expect_err("a conflicting recovery PIN must error");
        match err {
            Error::Unexpected(msg) => assert!(
                msg.contains("Existing seed file conflicted"),
                "unexpected error message: {}",
                msg
            ),
            other => panic!("expected Error::Unexpected, got {:?}", other),
        }
    }
}
