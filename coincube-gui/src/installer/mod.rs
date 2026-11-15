use coincube_ui::locale::copy as localized_copy;
mod claim;
pub(crate) mod connect_vault;

#[cfg(feature = "integration-tests")]
pub mod integration {
    pub use super::connect_vault::{create_connect_vault, ConnectVaultError, ConnectVaultOutcome};
    pub use super::context::{ConnectVaultMemberPayload, Context};
    pub use super::descriptor::PathKind;

    pub fn testnet4_esplora_config(jwt: &str) -> coincubed::config::EsploraConfig {
        super::connect_esplora_config(super::bitcoin::Network::Testnet4, jwt)
    }

    /// Exact product restore staging, with no caller-supplied original signer.
    pub fn stage_full_recovery(
        ctx: &mut Context,
        seed: &crate::services::recovery::SeedBlob,
        descriptor: &crate::services::recovery::DescriptorBlob,
    ) -> Result<(), String> {
        use super::step::recovery_kit_restore::{stage_restore, RestoreScope};
        if seed.cube.uuid != descriptor.cube.uuid
            || seed.cube.network != descriptor.cube.network
            || seed.cube.network != ctx.bitcoin_config.chain.api_str()
            || ctx.seed_cube_id() != seed.cube.uuid
        {
            return Err("Recovery fixture identity mismatch".into());
        }
        stage_restore(
            ctx.network,
            RestoreScope::Full,
            Some(seed),
            Some(descriptor),
        )?
        .commit(ctx, super::context::RestoreSource::PasswordKit);
        Ok(())
    }

    pub fn recovery_esplora_config(
        chain: crate::chain::ChainId,
        jwt: &str,
    ) -> coincubed::config::EsploraConfig {
        if chain.is_blake2b() {
            return super::connect_esplora_config(chain, jwt);
        }
        // Native QA never persists an ordinary-Testnet4 bearer in daemon.toml.
        // Use the product's existing public providers; Connect kit APIs keep
        // their token solely in the signed process / protected QA item.
        coincubed::config::EsploraConfig {
            addr: super::public_esplora_url(chain.bitcoin_network()),
            token: None,
            fallback_addr: super::public_esplora_fallback_url(chain.bitcoin_network()),
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        }
    }

    /// Resume only the native test runner's verified Testnet4 seed write.
    /// Normal app callers retain create-new behavior through install_local_wallet.
    pub async fn install_testnet4_wallet_resumable(
        ctx: Context,
        wallet_id: super::WalletId,
        signer: std::sync::Arc<std::sync::Mutex<crate::signer::Signer>>,
    ) -> Result<super::WalletSettings, super::Error> {
        if ctx.bitcoin_config.chain != crate::chain::ChainId::Testnet4
            || ctx.recovered_signer.is_some()
            || ctx.seed_cube_id().is_empty()
        {
            return Err(super::Error::Unexpected(
                "Invalid Testnet4 resume context".into(),
            ));
        }
        super::install_local_wallet_with_seed_mode(
            ctx,
            wallet_id,
            signer,
            super::SeedWriteMode::ResumeVerified,
        )
        .await
    }

    /// Verify the exact existing descriptor seed without changing any bytes.
    /// Exposed only with integration-tests so offline crashpoint tests can use
    /// an ephemeral device secret instead of accessing the system Keychain.
    pub fn verify_existing_testnet4_seed(
        ctx: &Context,
        wallet_id: &super::WalletId,
        signer: &crate::signer::Signer,
        secret: &coincube_core::seed_crypt::DeviceSecret,
    ) -> Result<bool, super::Error> {
        use coincube_core::{
            seed_crypt,
            signer::{MasterSigner, MnemonicFileName},
        };
        let refuse = || {
            super::Error::Unexpected(
                "Existing Testnet4 seed could not be verified; no seed was changed".into(),
            )
        };
        let descriptor = ctx.descriptor.as_ref().ok_or_else(refuse)?;
        if ctx.bitcoin_config.chain != crate::chain::ChainId::Testnet4
            || ctx.network != super::bitcoin::Network::Testnet4
            || !descriptor.all_xpubs_net_is(super::bitcoin::Network::Testnet4)
            || !descriptor.contains_fingerprint_in_path(signer.fingerprint(), None)
            || ctx.seed_cube_id().is_empty()
            || descriptor
                .to_string()
                .split_once('#')
                .map(|(_, sum)| sum.to_owned())
                .as_ref()
                != Some(&wallet_id.descriptor_checksum)
        {
            return Err(refuse());
        }
        let timestamp = wallet_id.timestamp.ok_or_else(refuse)?;
        let path = MasterSigner::mnemonics_folder_for_chain(
            ctx.coincube_directory.path(),
            ctx.bitcoin_config.chain,
        )
        .join(
            MnemonicFileName {
                fingerprint: signer.fingerprint(),
                descriptor_info: Some((wallet_id.descriptor_checksum.clone(), timestamp)),
            }
            .to_string(),
        );
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(refuse()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            return Err(refuse());
        }
        let bytes = std::fs::read(&path).map_err(|_| refuse())?;
        if !bytes.starts_with(seed_crypt::ENCRYPTED_V3_MARKER) {
            return Err(refuse());
        }
        let password = ctx.seed_password().ok_or_else(refuse)?;
        let plaintext =
            seed_crypt::decrypt_with(&bytes, password, ctx.seed_cube_id(), Some(secret))
                .map_err(|_| refuse())?;
        let phrase = std::str::from_utf8(&plaintext).map_err(|_| refuse())?;
        let recovered = MasterSigner::from_str(super::bitcoin::Network::Testnet4, phrase)
            .map_err(|_| refuse())?;
        if recovered.words() != signer.mnemonic()
            || recovered.fingerprint(&super::bitcoin::secp256k1::Secp256k1::new())
                != signer.fingerprint()
        {
            return Err(refuse());
        }
        Ok(true)
    }
}
mod context;
#[cfg(feature = "mcp-proposals")]
pub(crate) mod creation;
mod decrypt;
mod descriptor;
mod message;
pub(crate) mod migration;
mod prompt;
#[cfg(feature = "mcp-proposals")]
pub(crate) mod proposal;
#[cfg(feature = "mcp-proposals")]
pub(crate) mod proposal_handoff;
pub(crate) mod step;
mod view;

// Shared node-resources control (prune + mempool presets/fields + disk
// estimate), reused by the Vault node settings so the two surfaces never drift.

/// Connect's Esplora proxy for a chain, keyed on the chain's *identity*:
/// `/api/v1/esplora/bitcoin/<net>` for the Bitcoin family and
/// `/api/v1/esplora/bitcoin-blake2b/{mainnet,testnet4}` for Bitcoin Blake2b
/// (the namespace Connect PR 4 serves; coincube-api#281). A BTCB2 Cube must
/// never be pointed at `bitcoin/mainnet` — the two chains share an address
/// format, so a BTCB2 spend broadcast there would *be* a replay onto Bitcoin
/// executed by our own software (audit F3 on coincube-api#276).
pub(crate) fn connect_url<C: Into<crate::chain::ChainId>>(chain: C) -> String {
    let base = crate::services::coincube_api_base_url();
    format!("{}/api/v1/esplora/{}", base, connect_esplora_path(chain))
}

/// The `<family>/<net>` segment of Connect's Esplora proxy for a chain — the
/// part of [`connect_url`] that does not depend on which API base is in use,
/// so callers that already hold a client with a base URL (tests included)
/// build the same path.
pub(crate) fn connect_esplora_path<C: Into<crate::chain::ChainId>>(chain: C) -> &'static str {
    use crate::chain::ChainId;
    match chain.into() {
        ChainId::Bitcoin => "bitcoin/mainnet",
        ChainId::Testnet => "bitcoin/testnet",
        ChainId::Signet => "bitcoin/signet",
        ChainId::Testnet4 => "bitcoin/testnet4",
        ChainId::Regtest => "bitcoin/regtest",
        ChainId::BitcoinBlake2b => "bitcoin-blake2b/mainnet",
        ChainId::BitcoinBlake2bTestnet4 => "bitcoin-blake2b/testnet4",
    }
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
///
/// Bitcoin Blake2b gets a chain of exactly one entry: Connect's BTCB2 proxy.
/// No mempool.space, no blockstream.info — those index Bitcoin, and any
/// fallback to them would show Bitcoin's balances and broadcast BTCB2 spends
/// onto Bitcoin. (Prepared here for the runtime slice; in this build a BTCB2
/// Cube is refused before any `daemon.toml` is written.)
pub(crate) fn connect_esplora_config<C: Into<crate::chain::ChainId>>(
    chain: C,
    jwt: &str,
) -> EsploraConfig {
    let chain = chain.into();
    if chain.is_blake2b() {
        return EsploraConfig {
            addr: connect_url(chain),
            token: Some(jwt.to_owned()),
            fallback_addr: None,
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        };
    }
    let network = chain.bitcoin_network();
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
use coincube_ui::locale::Text as CopyText;
use coincube_ui::{
    component::network_banner,
    widget::{Column, ColumnExt, Element},
};
use coincubed::config::EsploraConfig;
use coincubed::config::{BitcoinBackend, BitcoindConfig, BitcoindRpcAuth, Config};
pub use context::{Context, RestoreSource};
use iced::{clipboard, Subscription, Task};
use tokio::runtime::Handle;
use tracing::{error, info, warn};

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{
    app::{
        config as gui_config,
        settings::{update_settings_file, SettingsError, WalletId, WalletSettings},
        wallet::wallet_name,
    },
    daemon::DaemonError,
    delete,
    dir::{CoincubeDirectory, NetworkDirectory},
    hw::{HardwareWalletConfig, HardwareWalletMessage, HardwareWallets},
    signer::Signer,
};

pub use claim::ClaimSource;
pub use descriptor::{KeySource, KeySourceKind, KeychainKeyOwner, PathKind, PathSequence};
pub use message::Message;
use step::{
    BackupDescriptor, BackupMnemonic, ChooseDescriptorTemplate, ClaimOverview, CoincubeConnectStep,
    DefineDescriptor, DefineNode, DescriptorTemplateDescription, Final, ImportDescriptor,
    InheritanceRestoreStep, InternalBitcoindStep, OwnerKeychainRestoreStep, RecoverMnemonic,
    RecoveryKitRestoreStep, RegisterDescriptor, RestorePinSetupStep, RestoreScope,
    SelectBitcoindTypeStep, Step, WalletAlias,
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
    /// Bitcoin Blake2b **Claim target** creation (Lane B1.4). Builds a fork
    /// Cube from a Bitcoin Cube's Vault descriptor so it watches — and can
    /// spend — the same addresses on the fork chain.
    ///
    /// Shares the descriptor-reuse shape of `RestoreVaultFromRecoveryKit`: the
    /// descriptor is settled before the flow starts, so there is no editor, no
    /// template picker and no mnemonic backup. It is *not* a restore — nothing
    /// is being recovered — and it creates a second Cube rather than filling in
    /// the running one, which is why it is its own flow rather than a flag on
    /// that one.
    ///
    /// Creates the copy only (Tools › Split coins → Create copy). Nothing in
    /// this flow splits, sweeps or broadcasts.
    ClaimBlake2b {
        from_cube: Box<ClaimSource>,
    },
}

/// The Import or restore paths that run through the installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePath {
    /// Restore from Cube Recovery Kit.
    Kit,
    /// Recover with phone (the owner's Keychain).
    Phone,
}

/// A row of Import or restore's path list, pressed in the installer: the
/// Launcher owns every path, so the installer hands the press back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreDestination {
    Overview,
    ImportBitcoin,
    Kit,
    Phone,
}

/// Where a restore step sits on its path's stepper. Steps with none are
/// Connect, node and finalize, which the design sums up as Finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreStage {
    SignIn,
    Cube,
    Password,
    Approve,
    Pin,
}

impl RestorePath {
    fn of(flow: &UserFlow) -> Option<Self> {
        match flow {
            UserFlow::RestoreFromRecoveryKit { .. } => Some(Self::Kit),
            UserFlow::RecoverOwnCubeWithPhone {
                full_cube: true, ..
            } => Some(Self::Phone),
            _ => None,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Kit => localized_copy("Restore from Cube Recovery Kit"),
            Self::Phone => localized_copy("Recover with phone"),
        }
    }

    pub(crate) fn steps(self) -> &'static [&'static str] {
        match self {
            Self::Kit => &["Cube", "Password", "PIN", "Done"],
            Self::Phone => &["Cube", "Approve", "PIN", "Done"],
        }
    }

    /// The stepper index for a step's stage; Done when it has none. A
    /// signed-out start lands on the Kit's own sign-in, part of its Cube
    /// step: the wizard opens Kit restores only after Connect sign-in.
    fn index(self, stage: Option<RestoreStage>) -> usize {
        let label = match stage {
            Some(RestoreStage::SignIn) => "Cube",
            Some(RestoreStage::Cube) => "Cube",
            Some(RestoreStage::Password) => "Password",
            Some(RestoreStage::Approve) => "Approve",
            Some(RestoreStage::Pin) => "PIN",
            None => "Done",
        };
        let steps = self.steps();
        steps
            .iter()
            .position(|step| *step == label)
            .unwrap_or(steps.len() - 1)
    }
}

/// The heading of the Vault wizard run from inside an open Cube.
pub(crate) const CREATE_VAULT_TITLE: &str = "Create Vault";

/// Where a step sits on the Create Vault stepper: the installer's
/// `CreateWallet` flow launched from inside an open Cube. Several steps can
/// share a stage, and the stepper shows a stage once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultStage {
    Vault,
    Policy,
    Keys,
    Backup,
    Register,
    Connect,
    Node,
    Name,
    Done,
}

impl VaultStage {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Vault => "Vault",
            Self::Policy => localized_copy("Policy"),
            Self::Keys => localized_copy("Keys"),
            Self::Backup => localized_copy("Backup"),
            Self::Register => localized_copy("Register"),
            Self::Connect => "Connect",
            Self::Node => localized_copy("Node"),
            Self::Name => localized_copy("Name"),
            Self::Done => localized_copy("Done"),
        }
    }

    /// Whether a Create Vault can pass without this stage: Register runs
    /// only for a hardware key and Connect only without a Connect session or
    /// on regtest. Every other stage runs in every Create Vault, even where
    /// the context cannot tell yet: before Keys builds the descriptor, the
    /// Node and Name steps report themselves skipped.
    fn optional(self) -> bool {
        matches!(self, Self::Register | Self::Connect)
    }
}

/// The Create Vault stepper's labels and the current step's place on it.
/// An optional stage drops out when the context skips its steps, and
/// consecutive steps of one stage share a label. A current step with no
/// stage keeps the stage before it.
fn vault_stepper(
    stages: impl IntoIterator<Item = (Option<VaultStage>, bool)>,
    current: usize,
) -> (Vec<&'static str>, usize) {
    let mut labels: Vec<&'static str> = Vec::new();
    let mut active = 0;
    for (index, (stage, skipped)) in stages.into_iter().enumerate() {
        let Some(stage) = stage else { continue };
        if skipped && stage.optional() && index != current {
            continue;
        }
        if labels.last() != Some(&stage.label()) {
            labels.push(stage.label());
        }
        if index <= current {
            active = labels.len() - 1;
        }
    }
    (labels, active)
}

/// What a claim needs to put the user back in the Cube they started from.
///
/// Deliberately **not** `Clone`: it owns `Arc`s to the source Cube's live SDK
/// clients, and the last release of the Spark one shuts its bridge subprocess
/// down. One owner at a time means the cancel path has to `take()` it — a
/// second copy left behind in the installer is a compile error rather than a
/// bridge that outlives the flow.
pub struct SourceCube {
    pub settings: crate::app::settings::CubeSettings,
    pub breez_client: Option<std::sync::Arc<crate::app::breez_liquid::BreezClient>>,
    pub spark_backend: Option<std::sync::Arc<crate::app::wallets::SparkBackend>>,
}

pub struct Installer {
    pub(crate) navigation_error: Option<String>,
    pub(crate) claim_target_saved: bool,
    pub network: bitcoin::Network,
    pub datadir: CoincubeDirectory,

    #[cfg(feature = "mcp-proposals")]
    proposal_settings: Option<crate::app::local_mcp::Settings>,
    #[cfg(feature = "mcp-proposals")]
    pub(crate) proposal_install_dispatched: bool,
    #[cfg(feature = "mcp-proposals")]
    pub(crate) proposal_finished: bool,
    #[cfg(feature = "mcp-proposals")]
    pub(crate) proposal_locked: bool,
    #[cfg(feature = "mcp-proposals")]
    pub(crate) proposal_failure: Option<String>,
    current: usize,
    steps: Vec<Box<dyn Step>>,
    /// A Cube restore drawn in Import or restore (P-Restore), and its chain.
    restore: Option<(RestorePath, crate::chain::ChainId)>,
    /// The path list row that left the restore, for the Launcher to open.
    restore_exit: Option<RestoreDestination>,
    /// Create Vault from inside an open Cube, drawn under its own heading
    /// and stepper. Bitcoin Blake2b keeps the installer's layout.
    create_vault: bool,
    hws: HardwareWallets,
    signer: Arc<Mutex<Signer>>,
    fork_install_task: Option<iced::task::Handle>,

    /// Context is data passed through each step.
    pub context: Context,

    /// Track if installer was launched from an app without vault (true) or from home (false)
    pub launched_from_app: bool,

    /// Cube settings when launched from app (for returning to the same cube)
    pub cube_settings: Option<crate::app::settings::CubeSettings>,

    /// The Cube this installer was launched *from*, when that is a different
    /// Cube from the one being built — today only a Bitcoin Blake2b claim.
    ///
    /// Two jobs, and they are deliberately separate from [`Self::breez_client`]
    /// / [`Self::spark_backend`]: those are inputs to the Cube being *built*
    /// and must stay `None` on the fork chain, while this is what the source
    /// Cube is rebuilt from if the user backs out. Parking the handles here
    /// also keeps them alive across the round trip — `State` holds one screen
    /// at a time, so the source `App` is dropped at the transition, and the
    /// last `Arc` release is what shuts the Spark bridge subprocess down.
    pub source_cube: Option<SourceCube>,
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
        #[cfg(feature = "mcp-proposals")]
        if let Some(session) = &self.context.mcp_proposal {
            session.invalidate();
        }
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

    /// The path list row that left a restore, taken once by the Launcher.
    pub fn take_restore_exit(&mut self) -> Option<RestoreDestination> {
        self.restore_exit.take()
    }

    /// Public identifier of the generated Cube master signer.
    pub fn master_signer_fingerprint(&self) -> bitcoin::bip32::Fingerprint {
        self.signer.lock().unwrap().fingerprint()
    }

    /// Chain-aware entry point. Reject dormant chains before generating a
    /// signer or constructing any backend; an encoding twin is never a fallback.
    #[allow(clippy::too_many_arguments)]
    pub fn try_new_for_chain(
        destination_path: CoincubeDirectory,
        chain: crate::chain::ChainId,
        user_flow: UserFlow,
        launched_from_app: bool,
        cube_settings: Option<crate::app::settings::CubeSettings>,
        breez_client: Option<std::sync::Arc<crate::app::breez_liquid::BreezClient>>,
        spark_backend: Option<std::sync::Arc<crate::app::wallets::SparkBackend>>,
        developer_mode: bool,
        coincube_client: Option<crate::services::coincube::CoincubeClient>,
    ) -> Result<(Installer, Task<Message>), Error> {
        if let crate::chain::RuntimeSupport::Dormant { reason } =
            crate::chain::authenticated_connect_support(chain)
        {
            return Err(Error::Unexpected(reason.to_string()));
        }
        if matches!(&user_flow, UserFlow::ClaimBlake2b { .. })
            && coincube_client.as_ref().and_then(|c| c.token()).is_none()
        {
            return Err(Error::Unexpected(
                "Connect authentication is required for Bitcoin Blake2b".into(),
            ));
        }
        if cube_settings
            .as_ref()
            .is_some_and(|cube| cube.network != chain)
        {
            return Err(Error::Unexpected(
                "Installer chain differs from the Cube chain".to_string(),
            ));
        }
        // A claim target unlocks the same way as its source Cube (decision
        // 2026-10-29): its seed file is the source Cube's mnemonic, written
        // under the source's PIN, or for a passkey source under the password
        // derived from the passkey's seed. With neither in this session the
        // target could not be opened, so refuse here rather than at the write.
        if let UserFlow::ClaimBlake2b { from_cube } = &user_flow {
            if crate::app::session::seed_file_password(&from_cube.cube).is_none() {
                return Err(Error::Unexpected(if from_cube.cube.is_passkey_cube() {
                    "Unlock this Cube with its passkey again, then create the copy.".to_string()
                } else {
                    "Reopen this Cube and try again — its PIN isn't available in this session."
                        .to_string()
                }));
            }
        }
        if chain.is_blake2b()
            && !matches!(
                user_flow,
                UserFlow::CreateWallet
                    | UserFlow::ClaimBlake2b { .. }
                    | UserFlow::RestoreFromRecoveryKit { .. }
                    | UserFlow::RecoverOwnCubeWithPhone {
                        full_cube: true,
                        ..
                    }
            )
        {
            return Err(Error::Unexpected(
                "Bitcoin Blake2b supports only a Connect-backed Vault creation, claim or \
                 Cube restore flow"
                    .to_string(),
            ));
        }
        let (parked_breez, parked_spark) = (breez_client.clone(), spark_backend.clone());
        // The invariant is that a Bitcoin Blake2b Cube is never *given* a Breez
        // or Spark client, not that the caller may not hold one. Refusing on
        // the caller's handles made the claim flow unanswerable — it launches
        // from a running Bitcoin Cube, which holds both — so the refusal moves
        // to what is handed into the fork construction path. The source Cube
        // keeps its live handles, which is also what stops the Spark bridge
        // subprocess being killed and re-spawned across the round trip.
        let (breez_client, spark_backend) = if chain.is_blake2b() {
            (None, None)
        } else {
            (breez_client, spark_backend)
        };
        if let UserFlow::ClaimBlake2b { from_cube } = &user_flow {
            // Parked, not handed in: the source Cube's own settings and SDK
            // handles, so backing out of a claim returns to that Cube rather
            // than to Home, and so the handles outlive the state transition.
            let source = SourceCube {
                settings: from_cube.cube.clone(),
                breez_client: parked_breez,
                spark_backend: parked_spark,
            };
            let (mut installer, task) = Self::build_for_chain(
                destination_path,
                chain,
                user_flow,
                launched_from_app,
                cube_settings,
                None,
                None,
                developer_mode,
                coincube_client,
            )?;
            installer.source_cube = Some(source);
            return Ok((installer, task));
        }
        Self::build_for_chain(
            destination_path,
            chain,
            user_flow,
            launched_from_app,
            cube_settings,
            breez_client,
            spark_backend,
            developer_mode,
            coincube_client,
        )
    }

    /// Bitcoin-family compatibility wrapper over [`Self::try_new_for_chain`].
    /// It goes through the same gate, so a Cube on another chain — which a
    /// `bitcoin::Network` cannot name — is refused rather than built as a
    /// Bitcoin installer carrying a fork Cube.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        destination_path: CoincubeDirectory,
        network: bitcoin::Network,
        user_flow: UserFlow,
        launched_from_app: bool,
        cube_settings: Option<crate::app::settings::CubeSettings>,
        breez_client: Option<std::sync::Arc<crate::app::breez_liquid::BreezClient>>,
        spark_backend: Option<std::sync::Arc<crate::app::wallets::SparkBackend>>,
        developer_mode: bool,
        coincube_client: Option<crate::services::coincube::CoincubeClient>,
    ) -> Result<(Installer, Task<Message>), Error> {
        Self::try_new_for_chain(
            destination_path,
            network.into(),
            user_flow,
            launched_from_app,
            cube_settings,
            breez_client,
            spark_backend,
            developer_mode,
            coincube_client,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_for_chain(
        destination_path: CoincubeDirectory,
        chain: crate::chain::ChainId,
        user_flow: UserFlow,
        launched_from_app: bool,
        cube_settings: Option<crate::app::settings::CubeSettings>,
        breez_client: Option<std::sync::Arc<crate::app::breez_liquid::BreezClient>>,
        spark_backend: Option<std::sync::Arc<crate::app::wallets::SparkBackend>>,
        developer_mode: bool,
        coincube_client: Option<crate::services::coincube::CoincubeClient>,
    ) -> Result<(Installer, Task<Message>), Error> {
        let network = chain.bitcoin_network();
        // Cube Key uses the Cube master and the descriptor's ordinary BIP32
        // paths. Developer mode changes visibility, never the signing seed.
        let master =
            installer_master(&destination_path, chain, &user_flow, cube_settings.as_ref())?;
        let cube_encryption_key = matches!(user_flow, UserFlow::CreateWallet).then(|| {
            Arc::new(crate::services::connect::crypto::CubeEncryptionKey::derive(
                &master, network,
            ))
        });
        let signer = Arc::new(Mutex::new(Signer::new(master)));
        let mut context = Context::new_for_chain(
            chain,
            destination_path.clone(),
            cube_settings.as_ref(),
            coincube_client,
        );
        context.in_cube_fork_vault = chain.is_blake2b()
            && launched_from_app
            && matches!(user_flow, UserFlow::CreateWallet)
            && cube_settings
                .as_ref()
                .is_some_and(|cube| cube.network == chain && cube.vault_wallet_id.is_none());
        // A claim target is a Cube of its own — its own id, its own Connect
        // Cube, its own datadir (I7) — built from the source Cube's descriptor
        // and seed. Both are settled here rather than collected by a step:
        // there is nothing for the user to enter that the source Cube does not
        // already answer.
        if let UserFlow::ClaimBlake2b { from_cube } = &user_flow {
            // Derived from the source Cube, not minted: an install that fails
            // after the seed write leaves a file bound to this id, and a retry
            // with a fresh one could never open it. See `claim::target_cube_id`.
            context.cube_id = Some(claim::target_cube_id(from_cube));
            // Named here as well as minted: the installer's exit seam mints the
            // Cube from `(cube_id, cube_name)` and falls back to a *fresh*
            // UUID when either is absent — which would leave the target's seed
            // file bound to a Cube id no Cube has, and nothing able to open it.
            context.cube_name = Some(from_cube.default_target_alias());
            context.descriptor = Some(from_cube.descriptor.clone());
            context.wallet_alias = from_cube.default_target_alias();
            context.claim_source = Some((**from_cube).clone());
        }
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
        // A claim runs inside the *source* Cube, not the Cube being built, so
        // the line above — which reads the Cube the installer is building —
        // finds nothing. The target's seed file is the source Cube's mnemonic
        // and must be encrypted under the source Cube's PIN, so it comes from
        // the same session resolver, keyed by the source Cube id. Admission
        // has already refused a source with no PIN in this session.
        if let UserFlow::ClaimBlake2b { from_cube } = &user_flow {
            context.cube_pin = crate::app::session::pin_for(from_cube.cube_id());
            // A passkey source has no PIN; its copy's seed file is written
            // under the password derived from the passkey's seed, the same
            // one the copy derives when it is unlocked with that passkey.
            if from_cube.cube.is_passkey_cube() {
                context.passkey_seed_password =
                    crate::app::session::seed_file_password(&from_cube.cube);
            }
        }
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
        if context.passkey_seed_password.is_none() {
            context.passkey_seed_password = cube_settings
                .as_ref()
                .filter(|cs| cs.is_passkey_cube())
                .and_then(crate::app::session::seed_file_password);
        }
        // Connect blinding (PR D3): the Vault builder opens Contacts' xpub
        // envelopes with the Cube's seed-derived encryption key. The master
        // source for a new Vault is the same verified Cube master as the hot
        // signer. Import/restore flows retain their existing SDK fallback.
        context.cube_encryption_key = cube_encryption_key.or_else(|| {
            breez_client
                .as_ref()
                .and_then(|bc| bc.liquid_signer())
                .and_then(|arc| {
                    arc.lock().ok().map(|s| {
                        std::sync::Arc::new(
                            crate::services::connect::crypto::CubeEncryptionKey::derive(
                                &s, network,
                            ),
                        )
                    })
                })
        });
        context.connect_signed_in_at_launch = context
            .coincube_client
            .as_ref()
            .and_then(|client| client.token())
            .is_some();
        let context = context;

        let restore = RestorePath::of(&user_flow).map(|path| (path, chain));
        // A Vault created inside an open Cube, Bitcoin Blake2b included: its
        // steps carry the same stages. Restore and claim flows are not framed
        // as Vault creation.
        let create_vault = launched_from_app
            && matches!(user_flow, UserFlow::CreateWallet)
            && (!chain.is_blake2b() || context.in_cube_fork_vault);
        let mut installer = Installer {
            restore,
            restore_exit: None,
            create_vault,
            navigation_error: None,
            claim_target_saved: false,
            network,
            source_cube: None,
            datadir: destination_path.clone(),
            #[cfg(feature = "mcp-proposals")]
            proposal_settings: None,
            #[cfg(feature = "mcp-proposals")]
            proposal_install_dispatched: false,
            #[cfg(feature = "mcp-proposals")]
            proposal_finished: false,
            #[cfg(feature = "mcp-proposals")]
            proposal_locked: false,
            #[cfg(feature = "mcp-proposals")]
            proposal_failure: None,
            current: 0,
            fork_install_task: None,
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
                let network_str = chain.api_str().to_string();
                match user_flow {
                    // Claim target: the descriptor is the source Cube's and is
                    // already in the context, so the editor, the template
                    // picker and `BackupMnemonic` are all absent. The mnemonic
                    // is by construction already backed up by the source Cube;
                    // showing it again would produce a second physical copy of
                    // one secret for no recovery benefit.
                    UserFlow::ClaimBlake2b { from_cube } => vec![
                        ClaimOverview {
                            source_name: from_cube.cube.name.clone(),
                            passkey: from_cube.cube.is_passkey_cube(),
                        }
                        .into(),
                        RegisterDescriptor::new_import_wallet().into(),
                        CoincubeConnectStep::new().into(),
                        SelectBitcoindTypeStep::new().into(),
                        InternalBitcoindStep::new(&context.coincube_directory).into(),
                        WalletAlias::default().into(),
                        Final::new().into(),
                    ],
                    UserFlow::CreateWallet if chain.is_blake2b() => vec![
                        ChooseDescriptorTemplate::default().into(),
                        DescriptorTemplateDescription::default().into(),
                        DefineDescriptor::new(network, signer.clone()).into(),
                        BackupMnemonic::new(signer.clone()).into(),
                        BackupDescriptor::default().into(),
                        RegisterDescriptor::new_create_wallet().into(),
                        CoincubeConnectStep::new().into(),
                        SelectBitcoindTypeStep::new().into(),
                        InternalBitcoindStep::new(&context.coincube_directory).into(),
                        WalletAlias::default().into(),
                        Final::new().into(),
                    ],
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
                        Final::restored().into(),
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
                                Final::restored().into(),
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
        Ok((installer, command))
    }

    /// Replace this proposal flow's steps after a native handoff.
    /// A mismatched target closes the session and must return to the launcher.
    #[cfg(feature = "mcp-proposals")]
    pub(crate) fn use_mcp_proposal(
        &mut self,
        settings: crate::app::local_mcp::Settings,
        proposal: proposal::Session,
    ) -> Result<Task<Message>, &'static str> {
        if proposal.chain() != self.context.bitcoin_config.chain {
            proposal.close();
            return Err("This proposal belongs to a different network than this Cube. Reopen the Cube and prepare a new proposal.");
        }
        let proposal = Arc::new(proposal);
        self.context.descriptor_template = context::DescriptorTemplate::Custom;
        self.context.mcp_proposal = Some(proposal.clone());
        // Keep the native client for managed fork feature admission.
        // Wallet installation uses the native local-node workflow.
        self.context.use_coincube_connect = false;
        self.steps = vec![
            Box::new(proposal_handoff::ProposalHandoff::new(proposal.clone())),
            DefineDescriptor::new(self.network, self.signer.clone()).into(),
            BackupMnemonic::new(self.signer.clone()).into(),
            BackupDescriptor::default().into(),
            RegisterDescriptor::new_create_wallet().into(),
        ];
        if proposal.chain().is_blake2b() {
            self.context.node_flavor = crate::node::bitcoind::NodeFlavor::KnotsBlake2b;
            self.context.bitcoind_is_external = false;
            self.context.install_node_alongside_connect = false;
            self.steps
                .push(InternalBitcoindStep::new(&self.context.coincube_directory).into());
        } else {
            self.steps
                .push(DefineNode::new(crate::node::NodeType::Bitcoind).into());
        }
        self.steps.extend(vec![
            WalletAlias::default().into(),
            Box::new(proposal::Review::new(proposal)) as Box<dyn Step>,
            Final::new().into(),
        ]);
        self.proposal_settings = Some(settings);
        // The proposal hand-off and review are not Create Vault stages.
        self.create_vault = false;
        self.current = 0;
        self.steps[0].load_context(&self.context);
        Ok(self.steps[0].load())
    }

    #[cfg(feature = "mcp-proposals")]
    pub(crate) fn expire_mcp_proposal(&mut self) {
        if self.context.mcp_proposal.is_none() {
            return;
        }
        self.proposal_locked = true;
        if let Some(proposal) = &self.context.mcp_proposal {
            proposal.close();
        }
        if let Some(settings) = &mut self.proposal_settings {
            settings.revoke();
        }
        // Existing Context password copies cannot authorize after admission is
        // revoked. Clear ours too; other installer/signers are stopped on exit.
        self.context.cube_pin = None;
        self.context.passkey_seed_password = None;
    }

    /// A terminal native failure retains partial data and permits safe exit.
    #[cfg(feature = "mcp-proposals")]
    pub(crate) fn finish_proposal_failure(&mut self, message: &str) {
        if self.context.mcp_proposal.is_none() {
            return;
        }
        self.proposal_finished = true;
        self.proposal_failure = Some(message.to_owned());
        self.expire_mcp_proposal();
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
        #[cfg(feature = "mcp-proposals")]
        self.expire_mcp_proposal();
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
        #[cfg(feature = "mcp-proposals")]
        if self
            .context
            .mcp_proposal
            .as_ref()
            .is_some_and(|p| !p.active())
            && !matches!(
                message,
                Message::Installed(..)
                    | Message::ConnectVaultCreated(..)
                    | Message::InstallFinished
                    | Message::Exit(..)
                    | Message::CubeSaved(..)
                    | Message::CubeSaveFailed(..)
                    | Message::RetryCubeSave
                    | Message::BackToApp(..)
                    | Message::FocusAction(..)
            )
        {
            self.navigation_error = Some("Proposal access ended. Reopen the Cube and start a new proposal. An already admitted native install may still finish; inspect its result before retrying.".into());
            return Task::none();
        }
        #[cfg(feature = "mcp-proposals")]
        if self.context.mcp_proposal.is_some()
            && self
                .context
                .creation_lease
                .lock()
                .map(|lease| lease.is_some())
                .unwrap_or(true)
            && matches!(
                message,
                Message::Next | Message::Previous | Message::BackToApp(..) | Message::Install
            )
        {
            return Task::none();
        }
        if matches!(&message, Message::Next | Message::Previous) {
            self.navigation_error = None;
        }
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
            Message::FocusAction(id) => iced::widget::operation::focus(id),
            Message::OpenUrl(url) => {
                if let Err(e) = crate::browser::open_url(&url) {
                    tracing::error!("Error opening '{}': {}", url, e);
                }
                Task::none()
            }
            Message::Next => self.next(),
            Message::Previous => self.previous(),
            Message::LeaveRestore(destination) => {
                self.restore_exit = Some(destination);
                Task::done(Message::BackToApp(self.network))
            }
            Message::Install => {
                #[cfg(feature = "mcp-proposals")]
                if self.context.mcp_proposal.is_some() {
                    if self.proposal_install_dispatched {
                        return Task::none();
                    }
                    self.proposal_install_dispatched = true;
                }
                // A restored Cube with no Vault is its seed alone.
                if self.context.bitcoin_config.chain.is_blake2b()
                    && self.context.descriptor.is_none()
                    && self.context.recovered_signer.is_none()
                {
                    return Task::done(Message::Installed(
                        None,
                        Err(Error::Unexpected(
                            "Bitcoin Blake2b requires a local Connect-backed Vault".into(),
                        )),
                    ));
                }
                let _cmd = self
                    .steps
                    .get_mut(self.current)
                    .expect("There is always a step")
                    .update(&mut self.hws, message);
                if let Some(descriptor) = self.context.descriptor.as_ref() {
                    let wallet_id = WalletId::generate(descriptor);
                    let context = self.context.clone();
                    let signer = self.signer.clone();
                    let task = Task::perform(
                        with_wallet_id(
                            wallet_id.clone(),
                            install_local_wallet(context, wallet_id, signer),
                        ),
                        |(id, res)| Message::Installed(Some(id), res.map(Some)),
                    );
                    if self.context.bitcoin_config.chain.is_blake2b() {
                        let (task, handle) = task.abortable();
                        self.fork_install_task = Some(handle.abort_on_drop());
                        task
                    } else {
                        task
                    }
                } else {
                    let ctx = self.context.clone();
                    Task::perform(
                        async move {
                            // We must persist the recovered signer so Breez Liquid/Spark
                            // can load it from the datadir on next startup.
                            // Even though there is no vault descriptor, the seed is needed.
                            if ctx.restored_imported_wallet.is_some()
                                && ctx.recovered_signer.is_none()
                            {
                                // Descriptor-only restore into an already unlocked
                                // Cube: retain its seed and current credentials.
                                if ctx.cube_id.is_none() {
                                    return Err(Error::Unexpected(
                                        "Imported wallet restore is missing its originating Cube."
                                            .into(),
                                    ));
                                }
                                return ensure_gui_config(
                                    &ctx.coincube_directory
                                        .network_directory(ctx.bitcoin_config.chain),
                                    false,
                                );
                            }
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
                                ctx.bitcoin_config.chain,
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
                #[cfg(feature = "mcp-proposals")]
                if self.context.mcp_proposal.is_some() {
                    self.finish_proposal_failure(if matches!(e, Error::CreationAdmission(_)) {
                        "No new installation was admitted. Return to the launcher and prepare a fresh proposal. Inspect any prior admitted attempt before retrying."
                    } else {
                        "Vault installation could not finish. Partial local data is retained. Return to the launcher and inspect the outcome before retrying."
                    });
                }
                #[cfg(feature = "mcp-proposals")]
                let may_cleanup = self.context.mcp_proposal.is_none()
                    && !matches!(e, Error::CreationAdmission(_));
                #[cfg(not(feature = "mcp-proposals"))]
                let may_cleanup = true;
                // A rejected admission owns no wallet to delete. An admitted
                // proposal may have partial durable state: preserve it and its
                // tombstone for inspection rather than guessing at rollback.
                if let Some(wallet_id) = wallet_id.as_ref().filter(|_| may_cleanup) {
                    let network_directory = self
                        .context
                        .coincube_directory
                        .network_directory(self.context.bitcoin_config.chain);
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
                        #[cfg(feature = "mcp-proposals")]
                        {
                            // Keep the shared lease during cleanup. Ordinary
                            // native setup may retry the same Context afterward.
                            if let Ok(mut lease) = self.context.creation_lease.lock() {
                                lease.take();
                            }
                        }
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
        #[cfg(feature = "mcp-proposals")]
        if self.proposal_locked {
            return iced::widget::column![
                coincube_ui::component::text::h3(localized_copy(if self.proposal_failure.is_some() { "Vault setup needs inspection" } else { "Cube locked" })),
                coincube_ui::component::text::text(localized_copy(self.proposal_failure.as_deref().unwrap_or("MCP access ended. An already authorized installation may finish. Its result will return to the launcher and require a fresh unlock."))),
                iced::widget::button(localized_copy("Return to launcher")).on_press_maybe(self.proposal_finished.then_some(Message::BackToApp(self.network)))
            ].spacing(16).padding(24).into();
        }
        let content = self.step_view();

        let content = if let Some(error) = &self.navigation_error {
            #[cfg(feature = "mcp-proposals")]
            let return_label = if self.context.mcp_proposal.is_some() {
                localized_copy("Return to launcher")
            } else {
                localized_copy("Return to Bitcoin Cube")
            };
            #[cfg(not(feature = "mcp-proposals"))]
            let return_label = localized_copy("Return to Bitcoin Cube");
            Column::new()
                .push(
                    coincube_ui::widget::Container::new(
                        Column::new()
                            .spacing(10)
                            .push(coincube_ui::component::card::warning(error.clone()))
                            .push(
                                coincube_ui::component::button::secondary(None, return_label)
                                    .on_press(Message::BackToApp(self.network)),
                            ),
                    )
                    .padding(20),
                )
                .push(content)
                .into()
        } else {
            content
        };

        let content = if let Some((path, chain)) = self.restore {
            self.restore_frame(path, chain, content)
        } else if self.create_vault {
            self.create_vault_frame(content)
        } else {
            content
        };

        if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        }
    }

    /// The step's view, without the installer's own title row when
    /// Import or restore draws the frame around it.
    fn step_view(&self) -> Element<Message> {
        let step = self
            .steps
            .get(self.current)
            .expect("There is always a step");
        let _framed = self
            .restore
            .map(|(path, _)| path.title())
            .or(self.create_vault.then_some(CREATE_VAULT_TITLE))
            .map(view::FramedGuard::enter);
        step.view(&self.hws, self.progress())
    }

    /// The Create Vault stepper for the current step.
    fn create_vault_steps(&self) -> (Vec<&'static str>, usize) {
        vault_stepper(
            self.steps
                .iter()
                .map(|step| (step.vault_stage(), step.off_vault_stepper(&self.context))),
            self.current,
        )
    }

    /// Create Vault around the installer step: its heading and stepper over
    /// the step, which leaves out its own progress row.
    fn create_vault_frame<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        use crate::home::restore_frame;
        use coincube_ui::{component::text::h2, theme, widget::Container};
        use iced::Length;

        let (steps, active) = self.create_vault_steps();
        let column = Column::new()
            .spacing(20)
            .width(Length::Fill)
            .max_width(960)
            .push(h2(localized_copy(CREATE_VAULT_TITLE)))
            .push_maybe(
                coincube_ui::worldview_copy::VoiceLine::CreateVault
                    .get()
                    .map(|line| {
                        coincube_ui::component::text::p2_regular(line).style(theme::text::secondary)
                    }),
            )
            .push(restore_frame::stepper(&steps, active))
            .push(content);
        Container::new(iced::widget::scrollable(
            Container::new(column)
                .padding([40, 24])
                .center_x(Length::Fill),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::container::background)
        .into()
    }

    /// Import or restore (P-Restore) around a Recovery Kit or phone restore:
    /// the same modal sheet as the Launcher's, with its header, path list
    /// and step rail over the installer step.
    fn restore_frame<'a>(
        &'a self,
        path: RestorePath,
        chain: crate::chain::ChainId,
        content: Element<'a, Message>,
    ) -> Element<'a, Message> {
        use crate::home::restore_frame;
        use coincube_ui::component::text::*;
        use coincube_ui::widget::ColumnExt;
        use iced::Length;

        let stage = self
            .steps
            .get(self.current)
            .and_then(|step| step.restore_stage());
        let active = path.index(stage);
        // Close leaves before anything is written; the steps after PIN
        // write the Cube and keep their own buttons.
        // Done: the restored Cube waits for Open Cube, the footer's only
        // action; there is no Back or Close.
        let done = self
            .steps
            .get(self.current)
            .is_some_and(|step| step.restore_done());
        let header = restore_frame::sheet_header(
            localized_copy("Import or restore"),
            restore_frame::network_chip(chain, false),
            if done {
                iced::widget::Space::new().into()
            } else {
                restore_frame::close_plain(
                    stage.is_some().then_some(Message::BackToApp(self.network)),
                )
            },
        );
        // Before anything is written, as for Close, the path list leaves
        // for the Launcher's path; after, it only shows where the user is.
        let leave = |destination: RestoreDestination, open: bool| {
            (stage.is_some() && open).then_some(Message::LeaveRestore(destination))
        };
        let nav = restore_frame::nav()
            .push(restore_frame::nav_action(
                localized_copy("Overview"),
                false,
                leave(RestoreDestination::Overview, true),
            ))
            .push(restore_frame::nav_action(
                localized_copy("Import Bitcoin wallet"),
                false,
                leave(
                    RestoreDestination::ImportBitcoin,
                    chain == crate::chain::ChainId::Bitcoin,
                ),
            ))
            .push(restore_frame::nav_action(
                localized_copy("Recovery Kit"),
                path == RestorePath::Kit,
                leave(RestoreDestination::Kit, path != RestorePath::Kit),
            ))
            .push_maybe(
                crate::feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED.then(|| {
                    restore_frame::nav_action(
                        localized_copy("Recover with phone"),
                        path == RestorePath::Phone,
                        leave(RestoreDestination::Phone, path != RestorePath::Phone),
                    )
                }),
            );
        let body = iced::widget::scrollable(
            Column::new()
                .spacing(20)
                .padding([24, 28])
                .push((!done).then(|| h2(path.title())))
                .push(content),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
        let footer = done.then(|| {
            coincube_ui::widget::Row::new()
                .push(iced::widget::Space::new().width(Length::Fill))
                .push(
                    coincube_ui::component::button::primary(None, CopyText::OpenCube.get())
                        .width(Length::Shrink)
                        .on_press(Message::OpenRestoredCube),
                )
                .into()
        });
        let region = restore_frame::region(
            Some(restore_frame::step_rail(
                match (coincube_ui::locale::language(), path) {
                    (coincube_ui::locale::Language::Spanish, RestorePath::Kit) => {
                        &["Cube", "Contraseña", "PIN", "Listo"]
                    }
                    (coincube_ui::locale::Language::Spanish, RestorePath::Phone) => {
                        &["Cube", "Aprobar", "PIN", "Listo"]
                    }
                    _ => path.steps(),
                },
                active,
            )),
            body,
            footer,
        );
        restore_frame::standalone(restore_frame::sheet(
            header,
            restore_frame::with_nav(nav.into(), region),
            None,
        ))
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

async fn daemon_check_authenticated(
    cfg: coincubed::config::Config,
    client: Option<crate::services::coincube::CoincubeClient>,
    root: &CoincubeDirectory,
) -> Result<(), Error> {
    if !cfg.bitcoin_config.chain.is_blake2b() {
        return daemon_check(cfg);
    }
    use crate::daemon::Daemon;
    let chain = cfg.bitcoin_config.chain;
    let daemon = if crate::chain::is_managed_local_fork(&cfg, root) {
        crate::chain::require_local_feature(chain, client, root)
            .await
            .map_err(Error::Unexpected)?;
        tokio::task::spawn_blocking(move || {
            crate::daemon::embedded::EmbeddedDaemon::start_local_fork(cfg)
        })
        .await
        .map_err(|e| Error::Unexpected(e.to_string()))?
        .map_err(|e| Error::Unexpected(format!("Failed to admit local backend: {e}")))?
    } else {
        let client = client.ok_or_else(|| {
            Error::Unexpected("Connect authentication is required for this backend".into())
        })?;
        crate::chain::require_connect_feature(chain, &client, root)
            .await
            .map_err(Error::Unexpected)?;
        crate::daemon::embedded::EmbeddedDaemon::start_authenticated(cfg, client)
            .await
            .map_err(|e| Error::Unexpected(format!("Failed to admit Connect backend: {e}")))?
    };
    daemon
        .stop()
        .await
        .map_err(|e| Error::Unexpected(format!("Failed to stop Connect validation daemon: {}", e)))
}

fn prepare_installer_data_directory(
    chain: crate::chain::ChainId,
    data_directory: &coincubed::datadir::DataDirectory,
) -> Result<std::path::PathBuf, Error> {
    if chain.is_blake2b() {
        // Do not create/canonicalize a wallet before authenticated admission.
        // absolute() is lexical and does not require the target to exist.
        return std::path::absolute(data_directory.path())
            .map_err(|e| Error::Unexpected(format!("Invalid datadir path: {}", e)));
    }
    data_directory
        .init()
        .map_err(|e| Error::CannotCreateDatadir(e.to_string()))?;
    data_directory
        .path()
        .canonicalize()
        .map_err(|e| Error::Unexpected(format!("Failed to canonicalize datadir path: {}", e)))
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

fn require_installable_chain(ctx: &Context) -> Result<(), Error> {
    if let crate::chain::RuntimeSupport::Dormant { reason } =
        crate::chain::authenticated_connect_support(ctx.bitcoin_config.chain)
    {
        return Err(Error::Unexpected(reason.to_string()));
    }
    if ctx.bitcoin_config.chain.is_blake2b()
        && (ctx.use_coincube_connect || !ctx.fork_local_vault_allowed())
        && ctx
            .coincube_client
            .as_ref()
            .and_then(|c| c.token())
            .is_none()
    {
        return Err(Error::Unexpected(
            "Connect authentication is required for Bitcoin Blake2b".into(),
        ));
    }
    ctx.bitcoin_config
        .check_chain_encoding()
        .map_err(|e| Error::Unexpected(e.to_string()))?;
    if ctx.network != ctx.bitcoin_config.network {
        return Err(Error::Unexpected(
            "Installer encoding differs from its chain".to_string(),
        ));
    }
    Ok(())
}

enum SeedWriteMode {
    Create,
    #[cfg(feature = "integration-tests")]
    ResumeVerified,
}

pub async fn install_local_wallet(
    ctx: Context,
    wallet_id: WalletId,
    signer: Arc<Mutex<Signer>>,
) -> Result<WalletSettings, Error> {
    install_local_wallet_with_seed_mode(ctx, wallet_id, signer, SeedWriteMode::Create).await
}

async fn install_local_wallet_with_seed_mode(
    ctx: Context,
    wallet_id: WalletId,
    signer: Arc<Mutex<Signer>>,
    seed_mode: SeedWriteMode,
) -> Result<WalletSettings, Error> {
    require_installable_chain(&ctx)?;
    #[cfg(feature = "mcp-proposals")]
    {
        let mut reservation = ctx
            .creation_lease
            .lock()
            .map_err(|_| Error::CreationAdmission("Creation reservation unavailable".into()))?;
        if reservation.is_some() {
            return Err(Error::CreationAdmission(
                "Vault creation already admitted for this session".into(),
            ));
        }
        *reservation =
            proposal::reserve(&ctx).map_err(|error| Error::CreationAdmission(error.to_string()))?;
    }
    let network_datadir = ctx
        .coincube_directory
        .network_directory(ctx.bitcoin_config.chain);
    if !ctx.bitcoin_config.chain.is_blake2b() {
        network_datadir
            .init()
            .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;
    }

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
        keychain_keys_recorded: ctx.keychain_keys_recorded,
    };

    let cfg: coincubed::config::Config = extract_daemon_config(&ctx, &wallet_settings)?;

    if ctx.claim_source.is_some() {
        let client = ctx.coincube_client.as_ref().ok_or_else(|| {
            Error::Unexpected("Connect authentication is required for a claim".into())
        })?;
        crate::chain::require_connect_feature(
            ctx.bitcoin_config.chain,
            client,
            &ctx.coincube_directory,
        )
        .await
        .map_err(Error::Unexpected)?;
    }
    daemon_check_authenticated(
        cfg.clone(),
        ctx.coincube_client.clone(),
        &ctx.coincube_directory,
    )
    .await?;
    if ctx.bitcoin_config.chain.is_blake2b() {
        network_datadir
            .init()
            .map_err(|e| Error::Unexpected(format!("Failed to create datadir path: {}", e)))?;
    }

    info!("daemon checked");
    // A claim target is a new fork Cube and needs a master seed file of its
    // own in the fork chain's `mnemonics` folder: the source Cube's mnemonic,
    // as decided by `claim::target_master_seed` (see the invariant recorded
    // there). A Vault created inside an existing Cube uses that Cube's master
    // seed file, which already exists, and the installer never mints one.
    let claim_target_seed = ctx
        .claim_source
        .as_ref()
        .and_then(claim::target_master_seed);
    if let Some(seed) = &claim_target_seed {
        persist_cube_master_seed(
            seed,
            &ctx.coincube_directory,
            ctx.bitcoin_config.chain,
            seed_password(&ctx)?.as_str(),
            ctx.seed_cube_id(),
            seed_device_secret(&ctx)?.as_ref(),
        )
        .map_err(claim_seed_error)?;
    }

    // Step needed because of ValueAfterTable error in the toml serialize implementation.
    let daemon_config_toml = toml::to_string_pretty(&cfg.for_persistence())
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

    if cfg.main_descriptor.as_vault().is_some_and(|vault| {
        vault
            .to_string()
            .contains(&signer.lock().unwrap().fingerprint().to_string())
    }) {
        // Persist the Cube-derived Vault signer under the Cube credential.
        // Historical independent Vault seeds still load by their own fingerprint.
        let password = seed_password(&ctx)?;
        let already_verified = match seed_mode {
            SeedWriteMode::Create => false,
            #[cfg(feature = "integration-tests")]
            SeedWriteMode::ResumeVerified => {
                let secret = seed_device_secret(&ctx)?.ok_or_else(|| {
                    Error::Unexpected("Testnet4 resume requires the Cube device secret".into())
                })?;
                integration::verify_existing_testnet4_seed(
                    &ctx,
                    &wallet_id,
                    &signer.lock().unwrap(),
                    &secret,
                )?
            }
        };
        if !already_verified {
            signer
                .lock()
                .unwrap()
                .store_encrypted_for_chain(
                    &ctx.coincube_directory,
                    cfg.bitcoin_config.chain,
                    &wallet_id.descriptor_checksum,
                    wallet_id
                        .timestamp
                        .expect("Every new wallet have now a timestamp"),
                    password.as_str(),
                    ctx.seed_cube_id(),
                    seed_device_secret(&ctx)?.as_ref(),
                )
                .map_err(|e| Error::Unexpected(format!("Failed to store mnemonic: {}", e)))?;
        }

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
            .store_encrypted_for_chain(
                &ctx.coincube_directory,
                cfg.bitcoin_config.chain,
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

/// The claim path's version of a seed-storage failure.
///
/// The storage layer's message ("Existing Cube master seed does not match this
/// installation") is accurate and tells a user nothing. On the claim path there
/// is exactly one way to reach it: another Cube on this device already holds a
/// Bitcoin Blake2b target for this same master seed — one recovery phrase
/// restored into two Cubes, each claiming. Refusing is right (the alternative
/// is one Cube's seed answering for another); leaving the user without the
/// reason is not.
pub(crate) fn claim_seed_error(error: Error) -> Error {
    // Two distinct storage messages reach here, and the first one is the one a
    // test written from the code alone would miss: the existing file is opened
    // with *this* attempt's credentials before its contents are compared, so a
    // file sealed under another Cube's id fails at the decrypt
    // ("Existing seed file conflicted with unlock credentials") rather than at
    // the mnemonic comparison ("does not match this installation"). Both mean
    // the same thing on this path.
    match error {
        Error::Unexpected(message)
            if message.contains("does not match")
                || message.contains("conflicted with unlock credentials") =>
        {
            Error::Unexpected(
                "Another Cube on this device already has a Bitcoin Blake2b claim for this \
                 recovery phrase. Open that Cube to use it, or claim from the Cube that made it."
                    .to_string(),
            )
        }
        other => other,
    }
}

/// A failed wallet install preserves the Cube's seed-only file. Retry only
/// when that exact file decrypts to the same complete mnemonic; a fingerprint
/// match (or a descriptor-specific sibling) is not proof of seed identity.
pub(crate) fn persist_cube_master_seed(
    signer: &Signer,
    coincube_directory: &CoincubeDirectory,
    chain: crate::chain::ChainId,
    password: &str,
    cube_id: &str,
    device_secret: Option<&coincube_core::seed_crypt::DeviceSecret>,
) -> Result<(), Error> {
    match signer.store_encrypted_seed_only_for_chain(
        coincube_directory,
        chain,
        password,
        cube_id,
        device_secret,
    ) {
        Ok(()) => Ok(()),
        Err(coincube_core::signer::SignerError::MnemonicStorage(ref error))
            if error.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            let filename = coincube_core::signer::MnemonicFileName {
                fingerprint: signer.fingerprint(),
                descriptor_info: None,
            };
            let path = coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(
                coincube_directory.path(),
                chain,
            )
            .join(filename.to_string());
            let bytes = std::fs::read(path).map_err(|e| {
                Error::Unexpected(format!("Failed to read existing Cube master seed: {e}"))
            })?;
            let plaintext =
                coincube_core::seed_crypt::decrypt_with(&bytes, password, cube_id, device_secret)
                    .map_err(|e| {
                    Error::Unexpected(format!(
                        "Existing seed file conflicted with unlock credentials: {e}"
                    ))
                })?;
            let expected = zeroize::Zeroizing::new(signer.mnemonic().join(" "));
            if plaintext.as_slice() != expected.as_bytes() {
                return Err(Error::Unexpected(
                    "Existing Cube master seed does not match this installation".into(),
                ));
            }
            Ok(())
        }
        Err(error) => Err(Error::Unexpected(format!(
            "Failed to store Cube master seed: {error}"
        ))),
    }
}

/// Persist a seed-only (Vault-less) restore and ensure its GUI config exists.
/// Retries use the same exact-file credential and mnemonic check as fresh Cubes.
fn persist_seed_only_install(
    recovered: &Signer,
    coincube_directory: &CoincubeDirectory,
    chain: impl Into<crate::chain::ChainId>,
    password: &str,
    cube_id: &str,
    device_secret: Option<&coincube_core::seed_crypt::DeviceSecret>,
) -> Result<(), Error> {
    let chain = chain.into();
    persist_cube_master_seed(
        recovered,
        coincube_directory,
        chain,
        password,
        cube_id,
        device_secret,
    )?;

    // Write `gui.toml` ourselves — the three wallet installers create it, but
    // the seed-only path never did, so a fresh (non-post-wipe) datadir reached
    // the `CubeSaved` finish line with no config and panicked. Seed-only cubes
    // run no managed bitcoind, so `start_internal_bitcoind` is false.
    let network_datadir = coincube_directory.network_directory(chain);
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
    require_installable_chain(ctx)?;
    let data_directory = ctx
        .coincube_directory
        .network_directory(ctx.bitcoin_config.chain)
        .coincubed_data_directory(&settings.wallet_id());
    let data_directory =
        prepare_installer_data_directory(ctx.bitcoin_config.chain, &data_directory)?;
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
    cfg.pending_managed_node_install = ctx.install_node_alongside_connect.then(|| {
        match ctx.node_flavor {
            crate::node::bitcoind::NodeFlavor::Core => "Core",
            crate::node::bitcoind::NodeFlavor::Knots => "Knots",
            crate::node::bitcoind::NodeFlavor::KnotsBlake2b => "KnotsBlake2b",
        }
        .to_string()
    });
    // Adopt the requested node once it is installed and synced, even if it
    // reused a chainstate and never entered IBD.
    cfg.auto_switch_to_pending = Some(ctx.install_node_alongside_connect);
    Ok(cfg)
}

#[derive(Debug, Clone)]
pub enum Error {
    // DaemonError does not implement Clone.
    // TODO: maybe Arc is overkill
    Backend(Arc<DaemonError>),
    Settings(SettingsError),
    Bitcoind(String),
    Electrum(String),
    Esplora(String),
    CannotCreateDatadir(String),
    CannotCreateFile(String),
    CannotWriteToFile(String),
    CannotGetAvailablePort(String),
    Unexpected(String),
    #[cfg(feature = "mcp-proposals")]
    CreationAdmission(String),
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

impl From<SettingsError> for Error {
    fn from(value: SettingsError) -> Self {
        Self::Settings(value)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Backend(e) => write!(f, "Wallet backend error: {}", e),
            Self::Settings(e) => write!(f, "Settings file error: {}", e),
            Self::Bitcoind(e) => write!(f, "Failed to ping bitcoind: {}", e),
            Self::Electrum(e) => write!(f, "Failed to ping Electrum: {}", e),
            Self::Esplora(e) => write!(f, "Failed to ping Esplora: {}", e),
            Self::CannotCreateDatadir(e) => write!(f, "Failed to create datadir: {}", e),
            Self::CannotGetAvailablePort(e) => write!(f, "Failed to get available port: {}", e),
            Self::CannotWriteToFile(e) => write!(f, "Failed to write to file: {}", e),
            Self::CannotCreateFile(e) => write!(f, "Failed to create file: {}", e),
            Self::Unexpected(e) => write!(f, "Unexpected: {}", e),
            #[cfg(feature = "mcp-proposals")]
            Self::CreationAdmission(e) => write!(f, "Creation not admitted: {}", e),
            Self::HardwareWallet(e) => write!(f, "Hardware Wallet: {}", e),
            Self::Backup(e) => write!(f, "Backup: {:?}", e),
        }
    }
}

/// Select the existing Cube's authenticated master, never an SDK client's
/// unscoped signer. Vault creation never mints a master: on every chain it
/// requires an existing Cube, whose master was created with the Cube. Import
/// and restore flows replace their placeholder with explicitly recovered keys.
fn installer_master(
    datadir: &CoincubeDirectory,
    chain: crate::chain::ChainId,
    flow: &UserFlow,
    cube: Option<&crate::app::settings::CubeSettings>,
) -> Result<coincube_core::signer::MasterSigner, Error> {
    use coincube_core::signer::MasterSigner;
    let unavailable =
        || Error::Unexpected("Unlock this Cube again before creating its Vault".into());
    let mut master = if matches!(flow, UserFlow::CreateWallet) {
        if let Some(cube) = cube {
            if cube.id.is_empty() {
                return Err(unavailable());
            }
            let fp = cube.master_signer_fingerprint.ok_or_else(unavailable)?;
            // Both Bitcoin and fork unlock can retain an authenticated root.
            // Resolve only this Cube's expected fingerprint; the constructor
            // already checked its exact chain, and the root is verified below.
            let cached = crate::app::session::unlocked_signer(&cube.id, fp);
            let signer = if let Some(signer) = cached {
                signer
            } else {
                // Seed-only restore legitimately opens a PIN-only session.
                // Resolve its already-persisted master without asynchronously
                // attaching another secret to the global cache. Passkey Cubes
                // must never treat a per-Vault file or stale PIN as their root.
                if cube.is_passkey_cube() {
                    return Err(unavailable());
                }
                let pin = crate::app::session::pin_for(&cube.id).ok_or_else(unavailable)?;
                crate::services::unlock::open_seed_by_fingerprint(
                    datadir.path(),
                    chain,
                    fp,
                    &pin,
                    &cube.id,
                )
                .map_err(|_| unavailable())?
            };
            if signer.fingerprint(&bitcoin::secp256k1::Secp256k1::new()) != fp {
                return Err(unavailable());
            }
            signer
        } else {
            // Without a Cube there is no master to derive the Cube Key from.
            // Minting one here would create a root outside any Cube.
            return Err(Error::Unexpected(
                "Create or unlock a Cube before creating its Vault".into(),
            ));
        }
    } else {
        MasterSigner::generate(chain.bitcoin_network()).map_err(|_| unavailable())?
    };
    master.set_network(chain.bitcoin_network());
    Ok(master)
}

#[cfg(all(test, feature = "mcp-proposals"))]
mod proposal_handoff_flow_tests {
    use super::proposal::tests::{authenticated_fixture_for_chain as fixture_for_chain, TestDir};

    fn fixture(root: &std::path::Path) -> (Context, Arc<super::proposal::Session>) {
        fixture_for_chain(root, crate::chain::ChainId::Regtest)
    }

    use super::*;

    #[test]
    fn mismatched_proposal_chain_closes_session_and_explains_refusal() {
        let _guard = crate::app::session::test_guard();
        let root = TestDir::new();
        let chain = crate::chain::ChainId::Bitcoin;
        let (mut ctx, session) = fixture_for_chain(root.path(), chain);
        let cube = crate::app::settings::Settings::from_file(
            &ctx.coincube_directory.network_directory(chain),
        )
        .unwrap()
        .cubes
        .remove(0);
        let (mut installer, _) = Installer::try_new_for_chain(
            ctx.coincube_directory.clone(),
            chain,
            UserFlow::CreateWallet,
            true,
            Some(cube),
            None,
            None,
            false,
            None,
        )
        .unwrap();
        ctx.mcp_proposal = None;
        // Simulate a changed native target with the same Bitcoin encoding.
        // The rejected proposal must not silently enter the ordinary flow.
        ctx.bitcoin_config.chain = crate::chain::ChainId::BitcoinBlake2b;
        installer.context = ctx;
        let session = Arc::try_unwrap(session).ok().unwrap();
        assert!(session.active());
        let refused =
            installer.use_mcp_proposal(crate::app::local_mcp::Settings::default(), session);
        assert!(!crate::app::session::is_open());
        assert!(installer.context.mcp_proposal.is_none());
        assert!(installer.proposal_settings.is_none());
        assert_eq!(refused.err(), Some(
            "This proposal belongs to a different network than this Cube. Reopen the Cube and prepare a new proposal."
        ));
    }

    #[test]
    fn prepared_handoff_continues_to_native_selection_and_lock_stops_navigation() {
        let _guard = crate::app::session::test_guard();
        let root = TestDir::new();
        let (mut ctx, session) = fixture(root.path());
        let cube = crate::app::settings::Settings::from_file(
            &ctx.coincube_directory
                .network_directory(crate::chain::ChainId::Regtest),
        )
        .unwrap()
        .cubes
        .remove(0);
        let (mut installer, _) = Installer::try_new_for_chain(
            ctx.coincube_directory.clone(),
            crate::chain::ChainId::Regtest,
            UserFlow::CreateWallet,
            true,
            Some(cube),
            None,
            None,
            false,
            None,
        )
        .unwrap();
        ctx.mcp_proposal = None;
        installer.context = ctx;
        let session = Arc::try_unwrap(session).ok().unwrap();
        let _ = installer
            .use_mcp_proposal(crate::app::local_mcp::Settings::default(), session)
            .unwrap();
        assert_eq!(installer.current, 0);
        let _ = installer.update(Message::Next);
        assert_eq!(installer.current, 1, "Continue enters native key selection");
        installer.expire_mcp_proposal();
        let _ = installer.update(Message::Previous);
        assert_eq!(installer.current, 1, "ended access blocks navigation");
        assert!(installer.proposal_locked);
        drop(installer.view());
    }

    #[test]
    fn ending_handoff_at_first_step_routes_to_launcher_and_closes_proposal() {
        use iced_runtime::futures::futures::StreamExt;

        let _guard = crate::app::session::test_guard();
        let root = TestDir::new();
        let (mut ctx, session) = fixture(root.path());
        let cube = crate::app::settings::Settings::from_file(
            &ctx.coincube_directory
                .network_directory(crate::chain::ChainId::Regtest),
        )
        .unwrap()
        .cubes
        .remove(0);
        let (mut installer, _) = Installer::try_new_for_chain(
            ctx.coincube_directory.clone(),
            crate::chain::ChainId::Regtest,
            UserFlow::CreateWallet,
            true,
            Some(cube),
            None,
            None,
            false,
            None,
        )
        .unwrap();
        ctx.mcp_proposal = None;
        installer.context = ctx;
        let session = Arc::try_unwrap(session).ok().unwrap();
        let _ = installer
            .use_mcp_proposal(crate::app::local_mcp::Settings::default(), session)
            .unwrap();
        let proposal = installer.context.mcp_proposal.as_ref().unwrap().clone();
        assert_eq!(installer.current, 0);
        assert!(proposal.active());

        let task = installer.update(Message::Previous);
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        let action = iced::futures::executor::block_on(stream.next()).unwrap();
        assert!(matches!(
            action,
            iced_runtime::Action::Output(Message::BackToApp(bitcoin::Network::Regtest))
        ));
        installer.stop();
        assert!(!proposal.active());
        assert!(!crate::app::session::is_open());
    }

    #[test]
    fn ended_handoff_focus_keeps_exit_control_available() {
        let _guard = crate::app::session::test_guard();
        let root = TestDir::new();
        let (mut ctx, session) = fixture(root.path());
        let cube = crate::app::settings::Settings::from_file(
            &ctx.coincube_directory
                .network_directory(crate::chain::ChainId::Regtest),
        )
        .unwrap()
        .cubes
        .remove(0);
        let (mut installer, _) = Installer::try_new_for_chain(
            ctx.coincube_directory.clone(),
            crate::chain::ChainId::Regtest,
            UserFlow::CreateWallet,
            true,
            Some(cube),
            None,
            None,
            false,
            None,
        )
        .unwrap();
        ctx.mcp_proposal = None;
        installer.context = ctx;
        let session = Arc::try_unwrap(session).ok().unwrap();
        let _ = installer
            .use_mcp_proposal(crate::app::local_mcp::Settings::default(), session)
            .unwrap();
        let proposal = installer.context.mcp_proposal.as_ref().unwrap().clone();
        proposal.revoke();
        assert!(!proposal.active());
        assert!(
            !installer.proposal_locked,
            "handoff remains visible until lock tick"
        );

        let focus = iced::advanced::widget::Id::new("proposal-handoff-return");
        let _ = installer.update(Message::FocusAction(focus));
        assert!(installer.navigation_error.is_none());
        assert_eq!(installer.current, 0);
        let _ = installer.update(Message::Next);
        assert!(
            installer.navigation_error.is_some(),
            "navigation remains blocked"
        );
    }
}

#[cfg(test)]
mod restore_stepper_tests {
    use super::*;
    use coincube_ui::locale::{render_in, Language};

    #[test]
    fn restore_stepper_follows_the_stage_in_spanish() {
        for language in [Language::English, Language::Spanish] {
            render_in(language, || {
                assert_eq!(RestorePath::Kit.index(Some(RestoreStage::Password)), 1);
                assert_eq!(RestorePath::Phone.index(Some(RestoreStage::Approve)), 1);
                assert_eq!(RestorePath::Kit.index(Some(RestoreStage::Pin)), 2);
                assert_eq!(RestorePath::Kit.index(None), 3);
            });
        }
    }
}

#[cfg(test)]
mod cube_signer_tests {
    use super::*;
    use crate::app::{breez_liquid::BreezClient, settings::CubeSettings};
    use coincube_core::signer::MasterSigner;

    #[test]
    fn vault_keys_come_from_the_cube_master_on_every_chain_and_in_every_mode() {
        let _guard = crate::app::session::test_guard();
        let secp = bitcoin::secp256k1::Secp256k1::new();
        // One synthetic primary seed can reproduce keys with every supported
        // chain's encoding; selecting test networks never mints another seed.
        let master = MasterSigner::generate(Network::Bitcoin).unwrap();
        let master_fp = master.fingerprint(&secp);
        for chain in [
            crate::chain::ChainId::Bitcoin,
            crate::chain::ChainId::Testnet4,
            crate::chain::ChainId::Signet,
            crate::chain::ChainId::Regtest,
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let network = chain.bitcoin_network();
            let cube = CubeSettings::new("synthetic".into(), chain).with_master_signer(master_fp);
            let root = std::env::temp_dir().join(format!("signer-761-{}", uuid::Uuid::new_v4()));
            crate::app::session::close();
            if chain.is_blake2b() {
                // Also exercise a restored fork session whose signer cache is empty.
                Signer::new(master.try_clone().unwrap())
                    .store_encrypted_seed_only_for_chain(
                        &CoincubeDirectory::new(root.clone()),
                        chain,
                        "2468",
                        &cube.id,
                        None,
                    )
                    .unwrap();
            } else {
                crate::app::session::store_unlocked_signer(
                    &cube.id,
                    master_fp,
                    master.try_clone().unwrap(),
                );
            }
            crate::app::session::open(&cube.id, zeroize::Zeroizing::new("2468".into()));
            let mut expected = master.try_clone().unwrap();
            expected.set_network(network);
            let account_path = step::descriptor::editor::key::derivation_path(
                network,
                bitcoin::bip32::ChildNumber::from_hardened_idx(0).unwrap(),
            );
            for developer_mode in [false, true, false] {
                let mut client = crate::services::coincube::CoincubeClient::new();
                client.set_token("synthetic-installer-token");
                let (installer, _) = Installer::try_new_for_chain(
                    CoincubeDirectory::new(root.clone()),
                    chain,
                    UserFlow::CreateWallet,
                    true,
                    Some(cube.clone()),
                    None, // BTCB2 has no SDK client; the session is the source.
                    None,
                    developer_mode,
                    Some(client),
                )
                .unwrap();
                let signer = installer.signer.lock().unwrap();
                assert_eq!(signer.fingerprint(), master_fp, "{:?}", chain);
                assert_eq!(
                    signer.get_extended_pubkey(&account_path),
                    expected.xpub_at(&account_path, &secp),
                    "the complete account xpub, including its network encoding, must match"
                );
                assert_eq!(installer.developer_mode, developer_mode);
                assert_eq!(
                    root.exists(),
                    chain.is_blake2b(),
                    "only fixture setup writes a seed"
                );
                assert_eq!(
                    installer
                        .context
                        .cube_encryption_key
                        .as_ref()
                        .unwrap()
                        .public_key(),
                    crate::services::connect::crypto::CubeEncryptionKey::derive(&expected, network)
                        .public_key()
                );
            }
            if root.exists() {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn an_existing_cube_without_its_scoped_master_is_refused_without_a_random_fallback() {
        let _guard = crate::app::session::test_guard();
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let master = MasterSigner::generate(Network::Bitcoin).unwrap();
        let fp = master.fingerprint(&secp);
        for chain in [
            crate::chain::ChainId::Bitcoin,
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            for developer_mode in [false, true] {
                for missing in [
                    "fingerprint",
                    "session",
                    "wrong-cube",
                    "wrong-fingerprint",
                    "wrong-seed",
                ] {
                    crate::app::session::close();
                    let mut cube =
                        CubeSettings::new("synthetic".into(), chain).with_master_signer(fp);
                    if missing == "fingerprint" {
                        cube.master_signer_fingerprint = None;
                    }
                    if missing != "session" {
                        let session_cube = if missing == "wrong-cube" {
                            "another-cube"
                        } else {
                            &cube.id
                        };
                        let session_fp = if missing == "wrong-fingerprint" {
                            bitcoin::bip32::Fingerprint::from([0; 4])
                        } else {
                            fp
                        };
                        let seed = if missing == "wrong-seed" {
                            MasterSigner::generate(Network::Bitcoin).unwrap()
                        } else {
                            master.try_clone().unwrap()
                        };
                        crate::app::session::store_unlocked_signer(session_cube, session_fp, seed);
                    }
                    let root =
                        std::env::temp_dir().join(format!("signer-761-{}", uuid::Uuid::new_v4()));
                    // An SDK handle, even carrying the expected key, cannot supply
                    // principal/session binding for a locked or different Cube.
                    let client = Arc::new(BreezClient::disconnected_with_signer(
                        chain.bitcoin_network(),
                        Arc::new(Mutex::new(master.try_clone().unwrap())),
                    ));
                    let mut connect = crate::services::coincube::CoincubeClient::new();
                    connect.set_token("synthetic-installer-token");
                    let result = Installer::try_new_for_chain(
                        CoincubeDirectory::new(root.clone()),
                        chain,
                        UserFlow::CreateWallet,
                        true,
                        Some(cube),
                        Some(client),
                        None,
                        developer_mode,
                        Some(connect),
                    );
                    assert!(
                        matches!(result, Err(Error::Unexpected(ref message)) if message.contains("Unlock this Cube")),
                        "{:?}: {}",
                        chain,
                        missing
                    );
                    assert!(!root.exists());
                }
            }
        }
    }

    #[test]
    fn a_passkey_cube_uses_its_authenticated_master_without_a_pin_or_sdk() {
        let _guard = crate::app::session::test_guard();
        for chain in [
            crate::chain::ChainId::Bitcoin,
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            crate::app::session::close();
            let master = MasterSigner::generate(chain.bitcoin_network()).unwrap();
            let fp = master.fingerprint(&bitcoin::secp256k1::Secp256k1::new());
            let cube = CubeSettings::new("synthetic".into(), chain)
                .with_master_signer(fp)
                .with_passkey(crate::app::settings::PasskeyMetadata {
                    credential_id: "synthetic".into(),
                    rp_id: "coincube.io".into(),
                    created_at: 0,
                    label: None,
                });
            crate::app::session::store_unlocked_signer(&cube.id, fp, master);
            assert!(crate::app::session::pin_for(&cube.id).is_none());
            for developer_mode in [false, true] {
                let (installer, _) = Installer::try_new_for_chain(
                    CoincubeDirectory::new(std::path::PathBuf::from("/nonexistent")),
                    chain,
                    UserFlow::CreateWallet,
                    true,
                    Some(cube.clone()),
                    None,
                    None,
                    developer_mode,
                    None,
                )
                .unwrap();
                assert_eq!(installer.signer.lock().unwrap().fingerprint(), fp);
                assert!(installer.context.passkey_seed_password.is_some());
                assert!(installer.context.cube_pin.is_none());
                assert_eq!(installer.developer_mode, developer_mode);
            }
        }
    }

    #[test]
    fn creating_a_bitcoin_vault_without_a_cube_requires_creating_or_unlocking_one() {
        for network in [
            Network::Bitcoin,
            Network::Testnet4,
            Network::Signet,
            Network::Regtest,
        ] {
            for developer_mode in [false, true] {
                let result = Installer::new(
                    CoincubeDirectory::new(std::path::PathBuf::from("/nonexistent")),
                    network,
                    UserFlow::CreateWallet,
                    false,
                    None,
                    None,
                    None,
                    developer_mode,
                    None,
                );
                assert!(
                    matches!(result, Err(Error::Unexpected(ref e)) if e.contains("Create or unlock a Cube"))
                );
            }
        }
    }

    #[test]
    fn a_seed_only_restored_cube_can_create_its_vault_in_the_first_pin_session() {
        use crate::app::session;
        use crate::services::recovery::{SeedBlob, SeedBlobCube, SeedBlobMnemonic, BLOB_VERSION};
        let _guard = session::test_guard();
        let chain = crate::chain::ChainId::Testnet4;
        let network = chain.bitcoin_network();
        let master = MasterSigner::generate(network).unwrap();
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let fp = master.fingerprint(&secp);
        let path = step::descriptor::editor::key::derivation_path(
            network,
            bitcoin::bip32::ChildNumber::from_hardened_idx(0).unwrap(),
        );
        let expected_xpub = master.xpub_at(&path, &secp);
        let expected_cek =
            crate::services::connect::crypto::CubeEncryptionKey::derive(&master, network)
                .public_key();
        let cube = CubeSettings::new("restored".into(), chain).with_master_signer(fp);
        let root =
            std::env::temp_dir().join(format!("restored-vault-761-{}", uuid::Uuid::new_v4()));
        let dir = CoincubeDirectory::new(root.clone());
        let seed = SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: cube.id.clone(),
                name: cube.name.clone(),
                network: chain.api_str().into(),
                created_at: "0".into(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: master.words().join(" "),
                language: "en".into(),
            },
        };
        let staged = step::recovery_kit_restore::stage_restore(
            network,
            RestoreScope::Full,
            Some(&seed),
            None,
        )
        .unwrap();
        let mut ctx = Context::new_for_chain(chain, dir.clone(), None, None);
        ctx.recovered_signer = Some(Arc::new(staged.signer.unwrap()));
        ctx.restore_pin = Some(zeroize::Zeroizing::new("2468".into()));
        ctx.cube_id = Some(cube.id.clone());
        persist_seed_only_install(
            ctx.recovered_signer.as_ref().unwrap(),
            &ctx.coincube_directory,
            ctx.bitcoin_config.chain,
            ctx.restore_pin.as_ref().unwrap(),
            ctx.seed_cube_id(),
            seed_device_secret(&ctx).unwrap().as_ref(),
        )
        .unwrap();

        // Production restore completion opens the PIN session without caching
        // a signer. Do not manufacture a cache entry to make creation work.
        session::close();
        session::open(&cube.id, zeroize::Zeroizing::new("2468".into()));
        assert!(session::unlocked_signer(&cube.id, fp).is_none());
        for developer_mode in [false, true] {
            let (installer, _) = Installer::try_new_for_chain(
                dir.clone(),
                chain,
                UserFlow::CreateWallet,
                true,
                Some(cube.clone()),
                None,
                None,
                developer_mode,
                None,
            )
            .expect("an authenticated seed-only restore can create its Vault without reopening");
            assert_eq!(
                installer.signer.lock().unwrap().get_extended_pubkey(&path),
                expected_xpub
            );
            assert_eq!(
                installer.context.cube_encryption_key.unwrap().public_key(),
                expected_cek
            );
            assert_eq!(installer.developer_mode, developer_mode);
            assert!(
                session::unlocked_signer(&cube.id, fp).is_none(),
                "disk lookup adds no global secret owner"
            );
        }
        // The usable encrypted seed stays present for these negative controls.
        for other in [Some("another-cube"), None] {
            session::close();
            if let Some(id) = other {
                session::open(id, zeroize::Zeroizing::new("2468".into()));
            }
            let result = Installer::try_new_for_chain(
                dir.clone(),
                chain,
                UserFlow::CreateWallet,
                true,
                Some(cube.clone()),
                None,
                None,
                false,
                None,
            );
            assert!(
                matches!(result, Err(Error::Unexpected(ref e)) if e.contains("Unlock this Cube"))
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_passkey_cache_miss_never_reads_a_seed_file_even_with_a_stale_pin() {
        use crate::app::session;
        let _guard = session::test_guard();
        for chain in [
            crate::chain::ChainId::Regtest,
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let master = Signer::generate(chain.bitcoin_network()).unwrap();
            let fp = master.fingerprint();
            let cube = CubeSettings::new("synthetic".into(), chain)
                .with_master_signer(fp)
                .with_passkey(crate::app::settings::PasskeyMetadata {
                    credential_id: "synthetic".into(),
                    rp_id: "coincube.io".into(),
                    created_at: 0,
                    label: None,
                });
            let root =
                std::env::temp_dir().join(format!("passkey-vault-761-{}", uuid::Uuid::new_v4()));
            let dir = CoincubeDirectory::new(root.clone());
            // A matching per-Vault file must not turn a passkey Cube into a PIN Cube.
            master
                .store_encrypted_for_chain(&dir, chain, "vault", 1000, "2468", &cube.id, None)
                .unwrap();
            session::close();
            session::open(&cube.id, zeroize::Zeroizing::new("2468".into()));
            assert!(session::unlocked_signer(&cube.id, fp).is_none());
            let result = Installer::try_new_for_chain(
                dir,
                chain,
                UserFlow::CreateWallet,
                true,
                Some(cube),
                None,
                None,
                false,
                None,
            );
            assert!(
                matches!(result, Err(Error::Unexpected(ref e)) if e.contains("Unlock this Cube"))
            );
            session::close();
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cube_master_recovery_kit_restores_the_exact_vault_signing_key_on_every_chain() {
        use crate::services::recovery::{
            self, DescriptorBlob, DescriptorBlobCube, DescriptorBlobVault, SeedBlob, SeedBlobCube,
            SeedBlobMnemonic, BLOB_VERSION,
        };
        use coincube_core::{
            descriptors::{CoincubeDescriptor, CoincubePolicy, PathInfo},
            miniscript::descriptor::DescriptorPublicKey,
        };
        use std::str::FromStr;
        let _guard = crate::app::session::test_guard();
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let master = MasterSigner::generate(Network::Bitcoin).unwrap();
        let fp = master.fingerprint(&secp);
        let backup_template = SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: "synthetic".into(),
                name: "synthetic".into(),
                network: "mainnet".into(),
                created_at: "0".into(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: master.words().join(" "),
                language: "en".into(),
            },
        };
        for chain in [
            crate::chain::ChainId::Bitcoin,
            crate::chain::ChainId::Testnet4,
            crate::chain::ChainId::Signet,
            crate::chain::ChainId::Regtest,
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let network = chain.bitcoin_network();
            let mut backup = backup_template.clone();
            backup.cube.network = chain.api_str().into();
            // The production encrypted envelope contains one Cube master only.
            let password = zeroize::Zeroizing::new("synthetic-recovery-password".to_string());
            let envelope = recovery::encrypt(
                &serde_json::to_vec(&backup).unwrap(),
                &password,
                recovery::KdfParams::DEFAULT_V1,
                &backup.cube.uuid,
            )
            .unwrap();
            let recovered: SeedBlob = serde_json::from_slice(
                &recovery::decrypt(&envelope, &password, &backup.cube.uuid).unwrap(),
            )
            .unwrap();
            let root =
                std::env::temp_dir().join(format!("signer-recovery-761-{}", uuid::Uuid::new_v4()));
            let cube = CubeSettings::new_with_raw_id("synthetic".into(), "synthetic".into(), chain)
                .with_master_signer(fp);
            crate::app::session::close();
            if chain.is_blake2b() {
                Signer::new(master.try_clone().unwrap())
                    .store_encrypted_seed_only_for_chain(
                        &CoincubeDirectory::new(root.clone()),
                        chain,
                        "2468",
                        &cube.id,
                        None,
                    )
                    .unwrap();
                crate::app::session::open(&cube.id, zeroize::Zeroizing::new("2468".into()));
            } else {
                crate::app::session::store_unlocked_signer(
                    &cube.id,
                    fp,
                    master.try_clone().unwrap(),
                );
            }
            let mut connect = crate::services::coincube::CoincubeClient::new();
            connect.set_token("synthetic-installer-token");
            let (installer, _) = Installer::try_new_for_chain(
                CoincubeDirectory::new(root.clone()),
                chain,
                UserFlow::CreateWallet,
                true,
                Some(cube),
                None,
                None,
                false,
                Some(connect),
            )
            .unwrap();
            let signer = installer.signer.lock().unwrap();
            let key = |account| {
                let path = step::descriptor::editor::key::derivation_path(
                    network,
                    bitcoin::bip32::ChildNumber::from_hardened_idx(account).unwrap(),
                );
                DescriptorPublicKey::from_str(&format!(
                    "[{}/{}]{}/<0;1>/*",
                    fp,
                    path.to_string().trim_start_matches("m/"),
                    signer.get_extended_pubkey(&path)
                ))
                .unwrap()
            };
            let descriptor = CoincubeDescriptor::new(
                CoincubePolicy::new_legacy(
                    PathInfo::Single(key(0)),
                    std::iter::once((10, PathInfo::Single(key(1)))).collect(),
                )
                .unwrap(),
            );
            let blob = DescriptorBlob {
                version: BLOB_VERSION,
                cube: DescriptorBlobCube {
                    uuid: "synthetic".into(),
                    network: chain.api_str().into(),
                },
                vault: DescriptorBlobVault {
                    name: "synthetic".into(),
                    descriptor: descriptor.to_string(),
                    change_descriptor: None,
                    signers: vec![],
                    birthday: Some(1),
                },
            };
            let staged = step::recovery_kit_restore::stage_restore(
                network,
                RestoreScope::Full,
                Some(&recovered),
                Some(&blob),
            )
            .unwrap();
            let restored = staged.signer.unwrap();
            let derived = staged
                .descriptor
                .unwrap()
                .receive_descriptor()
                .derive(7.into(), &secp);
            let prevout = bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(20_000),
                script_pubkey: derived.script_pubkey(),
            };
            let funding = bitcoin::Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![bitcoin::TxIn::default()],
                output: vec![prevout.clone()],
            };
            let input = bitcoin::TxIn {
                previous_output: bitcoin::OutPoint {
                    txid: funding.compute_txid(),
                    vout: 0,
                },
                ..bitcoin::TxIn::default()
            };
            let mut psbt = bitcoin::psbt::Psbt::from_unsigned_tx(bitcoin::Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![input],
                output: vec![bitcoin::TxOut {
                    value: bitcoin::Amount::from_sat(19_000),
                    script_pubkey: bitcoin::ScriptBuf::new(),
                }],
            })
            .unwrap();
            derived.update_psbt_in(&mut psbt.inputs[0]);
            psbt.inputs[0].witness_utxo = Some(prevout);
            psbt.inputs[0].non_witness_utxo = Some(funding);
            if chain.is_blake2b() {
                let psbt = coincube_core::psbt_unified::UnifiedPsbt::from_psbt(psbt).unwrap();
                let expected = signer.sign_psbt_unified(&psbt).unwrap();
                let signed = restored.sign_psbt_unified(&psbt).unwrap();
                assert_eq!(signed, expected);
                assert!(
                    coincube_core::unified_signing::verify_p2wsh_all_unified(&signed, &secp)
                        .unwrap()
                        > 0
                );
            } else {
                let expected = signer.sign_psbt(psbt.clone()).unwrap();
                let signed = restored.sign_psbt(psbt).unwrap();
                assert_eq!(signed, expected);
                assert!(!signed.inputs[0].partial_sigs.is_empty());
            }
            if root.exists() {
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn legacy_cube_master_vault_seed_still_loads_and_signs_without_rewriting() {
        use bitcoin::{hashes::Hash, sighash::SighashCache, Amount, ScriptBuf};
        let network = bitcoin::Network::Regtest;
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let root = std::env::temp_dir().join(format!("legacy-signer-761-{}", uuid::Uuid::new_v4()));
        let cube_id = "synthetic-legacy-developer-cube";
        let master = MasterSigner::generate(network).unwrap();
        let fingerprint = master.fingerprint(&secp);
        let legacy_vault = Signer::new(master.try_clone().unwrap());
        // The old installer wrote this same Cube master as a per-Vault v2
        // encrypted seed. No new signer, migration or rotation may replace it.
        legacy_vault
            .store_encrypted(
                &CoincubeDirectory::new(root.clone()),
                network,
                "oldvault",
                1000,
                "2468",
                cube_id,
                None,
            )
            .unwrap();
        let files: Vec<_> = std::fs::read_dir(MasterSigner::mnemonics_folder(&root, network))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 1);
        let before = std::fs::read(&files[0]).unwrap();
        let loaded = crate::services::unlock::open_seed_by_fingerprint(
            &root,
            network,
            fingerprint,
            "2468",
            cube_id,
        )
        .unwrap();
        let loaded = Signer::new(loaded);
        let path: bitcoin::bip32::DerivationPath = "m/48'/1'/0'/2'/0/0".parse().unwrap();
        let public_key = bitcoin::PublicKey::new(master.xpub_at(&path, &secp).public_key);
        let witness_script = bitcoin::script::Builder::new()
            .push_key(&public_key)
            .push_opcode(bitcoin::opcodes::all::OP_CHECKSIG)
            .into_script();
        let prevout = bitcoin::TxOut {
            value: Amount::from_sat(19_000),
            script_pubkey: witness_script.to_p2wsh(),
        };
        let mut psbt = bitcoin::psbt::Psbt::from_unsigned_tx(bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn::default()],
            output: vec![bitcoin::TxOut {
                value: Amount::from_sat(18_000),
                script_pubkey: ScriptBuf::new(),
            }],
        })
        .unwrap();
        psbt.inputs[0].witness_utxo = Some(prevout.clone());
        psbt.inputs[0].witness_script = Some(witness_script.clone());
        psbt.inputs[0]
            .bip32_derivation
            .insert(public_key.inner, (fingerprint, path));
        let unsigned_tx = psbt.unsigned_tx.clone();
        let expected = legacy_vault.sign_psbt(psbt.clone()).unwrap();
        let signed = loaded.sign_psbt(psbt).unwrap();
        assert_eq!(
            signed, expected,
            "the old key still produces the same signatures"
        );
        assert_eq!(signed.unsigned_tx, unsigned_tx);
        assert_eq!(signed.inputs[0].partial_sigs.len(), 1);
        let signature = signed.inputs[0].partial_sigs.get(&public_key).unwrap();
        let sighash = SighashCache::new(&unsigned_tx)
            .p2wsh_signature_hash(0, &witness_script, prevout.value, signature.sighash_type)
            .unwrap();
        secp.verify_ecdsa(
            &bitcoin::secp256k1::Message::from_digest(sighash.to_byte_array()),
            &signature.signature,
            &public_key.inner,
        )
        .unwrap();
        assert_eq!(std::fs::read(&files[0]).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
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
            chain: None,
            date: None,
            proprietary: serde_json::Map::new(),
            version: 0,
        });
        ctx
    }

    /// A `bitcoin::Network` cannot name a fork, so the Bitcoin constructor
    /// paired with a Bitcoin Blake2b Cube used to build a Bitcoin installer
    /// carrying that Cube. It now goes through the gate and refuses.
    #[test]
    fn the_bitcoin_constructor_refuses_a_cube_on_another_chain() {
        let _guard = crate::app::session::test_guard();
        let root = std::env::temp_dir().join(format!("cube-chain-{}", uuid::Uuid::new_v4()));
        let build = |cube: crate::app::settings::CubeSettings| {
            Installer::new(
                CoincubeDirectory::new(root.clone()),
                bitcoin::Network::Bitcoin,
                UserFlow::CreateWallet,
                true,
                Some(cube),
                None,
                None,
                false,
                None,
            )
        };
        let error = build(crate::app::settings::CubeSettings::new(
            "fork".into(),
            crate::chain::ChainId::BitcoinBlake2b,
        ))
        .err()
        .expect("a Bitcoin installer must not be built for a Bitcoin Blake2b Cube");
        assert!(
            error
                .to_string()
                .contains("Installer chain differs from the Cube chain"),
            "{}",
            error
        );
        // The same constructor still serves a Bitcoin Cube, so the refusal is
        // the chain check and not the constructor refusing every Cube.
        let master = coincube_core::signer::MasterSigner::generate(Network::Bitcoin).unwrap();
        let fp = master.fingerprint(&bitcoin::secp256k1::Secp256k1::new());
        let cube = crate::app::settings::CubeSettings::new(
            "bitcoin".into(),
            crate::chain::ChainId::Bitcoin,
        )
        .with_master_signer(fp);
        crate::app::session::store_unlocked_signer(&cube.id, fp, master);
        let (installer, _) = build(cube).expect("a Bitcoin Cube builds a Bitcoin installer");
        assert_eq!(
            installer.context.bitcoin_config.chain,
            crate::chain::ChainId::Bitcoin
        );
        assert!(!root.exists(), "construction must not touch a datadir");
    }

    /// A Vault created inside an open Bitcoin Blake2b Cube is not a fresh
    /// fork Cube (the Cube and its master seed exist), but it may still run
    /// on a local node without Connect. A copy into another Cube is neither.
    #[test]
    fn an_in_cube_fork_vault_may_run_without_connect() {
        let _guard = crate::app::session::test_guard();
        for chain in [
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let root = std::env::temp_dir().join(format!("fork-in-cube-{}", uuid::Uuid::new_v4()));
            let key =
                coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
            let master = Signer::new(key.try_clone().unwrap());
            let cube = crate::app::settings::CubeSettings::new("Fork".into(), chain)
                .with_master_signer(master.fingerprint());
            master
                .store_encrypted_seed_only_for_chain(
                    &CoincubeDirectory::new(root.clone()),
                    chain,
                    "2468",
                    &cube.id,
                    None,
                )
                .unwrap();
            crate::app::session::open(&cube.id, zeroize::Zeroizing::new("2468".into()));
            let seed_dir =
                coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(&root, chain);
            let seed_path = std::fs::read_dir(&seed_dir)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let seed_before = std::fs::read(&seed_path).unwrap();
            let (installer, _) = Installer::try_new_for_chain(
                CoincubeDirectory::new(root.clone()),
                chain,
                UserFlow::CreateWallet,
                true,
                Some(cube.clone()),
                None,
                None,
                false,
                None,
            )
            .unwrap();
            assert!(installer.context.in_cube_fork_vault);
            assert!(installer.context.fork_local_vault_allowed());
            assert_eq!(installer.master_signer_fingerprint(), master.fingerprint());
            let expected_key = crate::services::connect::crypto::CubeEncryptionKey::derive(
                &key,
                chain.bitcoin_network(),
            );
            assert_eq!(
                installer
                    .context
                    .cube_encryption_key
                    .as_ref()
                    .unwrap()
                    .public_key(),
                expected_key.public_key()
            );

            // Launched from Home, or for a Cube that already has a Vault, it
            // is not a Vault for the open Cube.
            let (from_home, _) = Installer::build_for_chain(
                CoincubeDirectory::new(root.clone()),
                chain,
                UserFlow::CreateWallet,
                false,
                Some(cube.clone()),
                None,
                None,
                false,
                None,
            )
            .unwrap();
            assert!(!from_home.context.in_cube_fork_vault);
            let mut vaulted = cube;
            vaulted.vault_wallet_id = Some(crate::app::settings::WalletId::new(
                "abcd1234".into(),
                Some(1),
            ));
            let (vaulted, _) = Installer::build_for_chain(
                CoincubeDirectory::new(root.clone()),
                chain,
                UserFlow::CreateWallet,
                true,
                Some(vaulted),
                None,
                None,
                false,
                None,
            )
            .unwrap();
            assert!(!vaulted.context.in_cube_fork_vault);
            assert!(!vaulted.context.fork_local_vault_allowed());
            assert_eq!(std::fs::read_dir(&seed_dir).unwrap().count(), 1);
            assert_eq!(
                std::fs::read(&seed_path).unwrap(),
                seed_before,
                "construction must not rewrite the authenticated seed"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn signed_out_fork_restore_can_collect_auth_but_cannot_install_without_it() {
        for chain in [
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            for flow in [
                UserFlow::RestoreFromRecoveryKit { cube_uuid: None },
                UserFlow::RecoverOwnCubeWithPhone {
                    cube_id: 7,
                    full_cube: true,
                },
            ] {
                let root = std::env::temp_dir()
                    .join(format!("fork-restore-entry-{}", uuid::Uuid::new_v4()));
                let (installer, _) = Installer::try_new_for_chain(
                    CoincubeDirectory::new(root.clone()),
                    chain,
                    flow,
                    false,
                    None,
                    None,
                    None,
                    false,
                    None,
                )
                .expect("native restore must be able to collect authentication");
                assert_eq!(installer.context.bitcoin_config.chain, chain);
                assert!(
                    require_installable_chain(&installer.context).is_err(),
                    "starting restore must not authorize backend installation"
                );
                assert!(!root.exists());
            }
        }
    }

    /// Vault creation never mints a master. Without a Cube there is no root
    /// to derive the Cube Key from, so every chain refuses before any key,
    /// SDK handle or file exists — including Bitcoin Blake2b, whose Cubes are
    /// created by Home's Cube journey like Bitcoin's.
    #[test]
    fn vault_creation_without_a_cube_is_refused_on_every_chain_and_mode() {
        for chain in crate::chain::ChainId::ALL {
            let root =
                std::env::temp_dir().join(format!("cubeless-vault-{}", uuid::Uuid::new_v4()));
            for launched_from_app in [false, true] {
                for developer_mode in [false, true] {
                    let built = Installer::build_for_chain(
                        CoincubeDirectory::new(root.clone()),
                        chain,
                        UserFlow::CreateWallet,
                        launched_from_app,
                        None,
                        None,
                        None,
                        developer_mode,
                        None,
                    );
                    assert!(
                        built.is_err(),
                        "{:?} built a Cube-less Vault installer",
                        chain
                    );
                    let admitted = Installer::try_new_for_chain(
                        CoincubeDirectory::new(root.clone()),
                        chain,
                        UserFlow::CreateWallet,
                        launched_from_app,
                        None,
                        None,
                        None,
                        developer_mode,
                        None,
                    );
                    assert!(admitted.is_err(), "{:?} admitted a Cube-less Vault", chain);
                }
            }
            assert!(!root.exists(), "refusal must not touch a datadir");
        }
    }

    #[test]
    fn install_boundary_accepts_a_valid_local_context_without_io() {
        for chain in crate::chain::ChainId::ALL {
            let root =
                std::env::temp_dir().join(format!("local-chain-boundary-{}", uuid::Uuid::new_v4()));
            let mut client = crate::services::coincube::CoincubeClient::new();
            client.set_token("synthetic-boundary-token");
            let ctx = Context::new_for_chain(
                chain,
                CoincubeDirectory::new(root.clone()),
                None,
                Some(client),
            );
            assert!(
                require_installable_chain(&ctx).is_ok(),
                "valid local context for {:?}",
                chain
            );
            assert!(!root.exists(), "boundary checks must not perform file IO");
        }
    }

    #[test]
    fn install_boundary_rejects_dormant_or_mismatched_chain_before_io() {
        for chain in crate::chain::ChainId::ALL {
            let temp =
                std::env::temp_dir().join(format!("btcb2-installer-{}", uuid::Uuid::new_v4()));
            let mut context =
                Context::new_for_chain(chain, CoincubeDirectory::new(temp.clone()), None, None);
            assert_eq!(
                require_installable_chain(&context).is_ok(),
                !chain.is_blake2b()
            );
            context.network = if context.network == Network::Signet {
                Network::Bitcoin
            } else {
                Network::Signet
            };
            assert!(require_installable_chain(&context).is_err());
            assert!(!temp.exists());
        }
    }

    #[test]
    fn fork_path_preparation_does_not_create_wallet_directory() {
        for chain in [
            crate::chain::ChainId::Bitcoin,
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let path =
                std::env::temp_dir().join(format!("installer-admission-{}", uuid::Uuid::new_v4()));
            let dir = coincubed::datadir::DataDirectory::new(path.clone());
            assert!(prepare_installer_data_directory(chain, &dir)
                .unwrap()
                .is_absolute());
            assert_eq!(path.exists(), !chain.is_blake2b());
            if path.exists() {
                std::fs::remove_dir(path).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn fork_missing_startup_client_refuses_before_wallet_writes() {
        let path = std::env::temp_dir().join(format!("installer-no-auth-{}", uuid::Uuid::new_v4()));
        let cfg = coincubed::config::Config::new(
            coincubed::config::BitcoinConfig::new(
                crate::chain::ChainId::BitcoinBlake2bTestnet4,
                std::time::Duration::from_secs(30),
            ),
            None,
            log::LevelFilter::Info,
            staged_with_descriptor(None).descriptor.unwrap(),
            coincubed::datadir::DataDirectory::new(path.clone()),
        );
        let err = daemon_check_authenticated(
            cfg,
            None,
            &CoincubeDirectory::new(std::path::PathBuf::new()),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, Error::Unexpected(reason) if reason.contains("authentication is required"))
        );
        assert!(!path.exists());
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
    fn seed_persistence_retry_uses_explicit_chain() {
        for chain in [
            crate::chain::ChainId::BitcoinBlake2b,
            crate::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let dir = temp_coincube_dir("fork-seed");
            let signer = Signer::generate(chain.bitcoin_network()).unwrap();
            persist_seed_only_install(&signer, &dir, chain, "246810", "cube-a", None).unwrap();
            persist_seed_only_install(&signer, &dir, chain, "246810", "cube-a", None).unwrap();
            assert!(
                persist_seed_only_install(&signer, &dir, chain, "999999", "cube-a", None).is_err()
            );
            assert!(dir
                .network_directory(chain)
                .path()
                .join(gui_config::DEFAULT_FILE_NAME)
                .exists());
            assert!(!dir
                .network_directory(chain.bitcoin_network())
                .path()
                .exists());
        }
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
        // exact on-disk seed verifies against the complete recovered mnemonic.
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
        // `AlreadyExists`, the exact-file verification fails to decrypt, and we
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

#[cfg(test)]
mod chain_provider_tests {
    //! The Esplora provider chain keyed on chain identity (audit F3 on
    //! coincube-api#276; BTCB2 plan PR 2). A Bitcoin Blake2b Cube must never
    //! be pointed at a Bitcoin indexer: the two chains share an address
    //! format, so balances shown would be Bitcoin's and a broadcast there
    //! would replay the BTCB2 spend onto Bitcoin.

    use super::*;
    use crate::chain::ChainId;

    const PUBLIC_HOSTS: [&str; 2] = ["mempool.space", "blockstream.info"];

    #[test]
    fn connect_url_keys_on_the_chain_identity() {
        let base = crate::services::coincube_api_base_url();
        assert_eq!(
            connect_url(ChainId::Bitcoin),
            format!("{}/api/v1/esplora/bitcoin/mainnet", base)
        );
        assert_eq!(
            connect_url(bitcoin::Network::Bitcoin),
            connect_url(ChainId::Bitcoin)
        );
        assert_eq!(
            connect_url(ChainId::BitcoinBlake2b),
            format!("{}/api/v1/esplora/bitcoin-blake2b/mainnet", base)
        );
        assert_eq!(
            connect_url(ChainId::BitcoinBlake2bTestnet4),
            format!("{}/api/v1/esplora/bitcoin-blake2b/testnet4", base)
        );
        // Never the Bitcoin namespace for the fork, however it encodes.
        assert_ne!(
            connect_url(ChainId::BitcoinBlake2b),
            connect_url(ChainId::BitcoinBlake2b.bitcoin_network())
        );
    }

    #[test]
    fn a_blake2b_provider_chain_is_connect_only_with_no_public_fallback() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let cfg = connect_esplora_config(chain, "jwt-token");
            assert_eq!(cfg.addr, connect_url(chain), "{:?}", chain);
            assert_eq!(cfg.token.as_deref(), Some("jwt-token"));
            assert_eq!(cfg.fallback_addr, None, "{:?}", chain);
            assert_eq!(cfg.fallback_token, None);
            assert_eq!(cfg.secondary_fallback_addr, None, "{:?}", chain);
            assert_eq!(cfg.secondary_fallback_token, None);
            // What a `daemon.toml` rendered from this config would contain:
            // no public host anywhere, and no Bitcoin namespace either.
            let rendered = format!(
                "{} {:?} {:?}",
                cfg.addr, cfg.fallback_addr, cfg.secondary_fallback_addr
            );
            for host in PUBLIC_HOSTS {
                assert!(!rendered.contains(host), "{:?}: {}", chain, rendered);
            }
            assert!(!rendered.contains("/esplora/bitcoin/"), "{}", rendered);
        }
    }

    #[test]
    fn the_bitcoin_family_provider_chains_are_unchanged() {
        // Mainnet: Connect primary, public fallbacks.
        let main = connect_esplora_config(ChainId::Bitcoin, "jwt");
        assert_eq!(main.addr, connect_url(ChainId::Bitcoin));
        assert_eq!(
            main.fallback_addr.as_deref(),
            Some("https://mempool.space/api")
        );
        assert_eq!(
            main.secondary_fallback_addr.as_deref(),
            Some("https://blockstream.info/api")
        );
        assert!(connect_esplora_config(bitcoin::Network::Bitcoin, "jwt") == main);
        // A test network: public primary, Connect backstop.
        let t4 = connect_esplora_config(ChainId::Testnet4, "jwt");
        assert_eq!(t4.addr, "https://mempool.space/testnet4/api");
        assert_eq!(
            t4.fallback_addr.as_deref(),
            Some(connect_url(ChainId::Testnet4).as_str())
        );
    }
}

#[cfg(test)]
mod cube_master_seed_retry_tests {
    use super::*;
    use coincube_core::{
        chain::ChainId,
        seed_crypt,
        signer::{MasterSigner, MnemonicFileName},
    };

    fn seed_path(root: &CoincubeDirectory, signer: &Signer) -> std::path::PathBuf {
        MasterSigner::mnemonics_folder_for_chain(root.path(), ChainId::BitcoinBlake2b).join(
            MnemonicFileName {
                fingerprint: signer.fingerprint(),
                descriptor_info: None,
            }
            .to_string(),
        )
    }

    #[test]
    fn fresh_master_seed_retry_preserves_exact_file_with_and_without_device_secret() {
        for secret in [None, Some(zeroize::Zeroizing::new([42; 32]))] {
            let root = CoincubeDirectory::new(
                std::env::temp_dir().join(format!("fork-seed-retry-{}", uuid::Uuid::new_v4())),
            );
            let signer = Signer::generate(Network::Bitcoin).unwrap();
            let cube = uuid::Uuid::new_v4().to_string();
            // This is the same helper the claim-target install calls before the
            // fallible daemon/config writes. A retry must retain its bytes.
            persist_cube_master_seed(
                &signer,
                &root,
                ChainId::BitcoinBlake2b,
                "2468",
                &cube,
                secret.as_ref(),
            )
            .unwrap();
            let before = std::fs::read(seed_path(&root, &signer)).unwrap();
            persist_cube_master_seed(
                &signer,
                &root,
                ChainId::BitcoinBlake2b,
                "2468",
                &cube,
                secret.as_ref(),
            )
            .unwrap();
            assert_eq!(std::fs::read(seed_path(&root, &signer)).unwrap(), before);
            assert!(!root.network_directory(ChainId::Bitcoin).path().exists());
            std::fs::remove_dir_all(root.path()).unwrap();
        }
    }

    #[test]
    fn fresh_master_seed_retry_refuses_conflicts_without_changing_bytes() {
        for case in ["pin", "cube", "seed", "invalid", "device"] {
            let root = CoincubeDirectory::new(
                std::env::temp_dir().join(format!("fork-seed-conflict-{}", uuid::Uuid::new_v4())),
            );
            let signer = Signer::generate(Network::Bitcoin).unwrap();
            let other = Signer::generate(Network::Bitcoin).unwrap();
            let cube = uuid::Uuid::new_v4().to_string();
            let secret = zeroize::Zeroizing::new([42; 32]);
            let supplied_secret = zeroize::Zeroizing::new([43; 32]);
            let mnemonic = zeroize::Zeroizing::new(
                if case == "seed" {
                    other.mnemonic()
                } else {
                    signer.mnemonic()
                }
                .join(" "),
            );
            let bytes = if case == "invalid" {
                b"invalid synthetic seed".to_vec()
            } else {
                seed_crypt::encrypt(
                    mnemonic.as_bytes(),
                    "2468",
                    &cube,
                    (case == "device").then_some(&secret),
                )
                .unwrap()
            };
            let path = seed_path(&root, &signer);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            // A matching descriptor-linked sibling must not authorize reusing
            // a conflicting master file with the same fingerprint in its name.
            signer
                .store_encrypted_for_chain(
                    &root,
                    ChainId::BitcoinBlake2b,
                    "synthetic-vault",
                    1,
                    "2468",
                    &cube,
                    None,
                )
                .unwrap();
            assert!(
                persist_cube_master_seed(
                    &signer,
                    &root,
                    ChainId::BitcoinBlake2b,
                    if case == "pin" { "9999" } else { "2468" },
                    if case == "cube" {
                        "other-synthetic-cube"
                    } else {
                        &cube
                    },
                    (case == "device").then_some(&supplied_secret)
                )
                .is_err(),
                "conflict {}",
                case
            );
            assert_eq!(
                std::fs::read(&path).unwrap(),
                bytes,
                "conflict {case} must preserve bytes"
            );
            std::fs::remove_dir_all(root.path()).unwrap();
        }
    }
}

#[cfg(test)]
mod connect_activation_tests;

#[cfg(test)]
mod claim_target_tests;

#[cfg(test)]
mod create_vault_frame_tests {
    use super::*;
    use crate::chain::ChainId;
    use crate::home::restore_frame::rendered_labels;

    const FULL: [&str; 8] = [
        "Vault", "Policy", "Keys", "Backup", "Connect", "Node", "Name", "Done",
    ];

    /// Create Vault as the open Cube's "Create Vault" button launches it.
    fn in_cube(chain: ChainId) -> Installer {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("create-vault-{}", uuid::Uuid::new_v4())),
        );
        let master =
            coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
        let fp = master.fingerprint(&bitcoin::secp256k1::Secp256k1::new());
        let cube = crate::app::settings::CubeSettings::new("Synthetic".into(), chain)
            .with_master_signer(fp);
        crate::app::session::store_unlocked_signer(&cube.id, fp, master);
        let installer = Installer::try_new_for_chain(
            root,
            chain,
            UserFlow::CreateWallet,
            true,
            Some(cube),
            None,
            None,
            false,
            None,
        )
        .expect("a Create Vault fixture installer")
        .0;
        crate::app::session::close();
        installer
    }

    fn at(installer: &mut Installer, stage: VaultStage) -> &mut Installer {
        installer.current = installer
            .steps
            .iter()
            .position(|step| step.vault_stage() == Some(stage))
            .expect("the stage has a step");
        installer
    }

    #[test]
    fn stages_skip_and_merge() {
        use VaultStage::*;
        let stages = [
            (None, false),
            (Some(Vault), false),
            (Some(Backup), false),
            (Some(Backup), false),
            (Some(Register), true),
            (Some(Connect), false),
            (Some(Node), true),
            (Some(Name), true),
            (Some(Done), false),
        ];
        let (labels, active) = vault_stepper(stages, 3);
        // Register drops out; Node and Name stay although the context skips
        // them before there is a descriptor.
        assert_eq!(
            labels,
            ["Vault", "Backup", "Connect", "Node", "Name", "Done"]
        );
        assert_eq!(active, 1);
        // A step without a stage keeps the stage before it; the first one
        // sits on the first stage.
        assert_eq!(vault_stepper(stages, 0).1, 0);
        // The current step is shown even when the context would skip it.
        let (labels, active) = vault_stepper(stages, 4);
        assert_eq!(
            labels,
            ["Vault", "Backup", "Register", "Connect", "Node", "Name", "Done"]
        );
        assert_eq!(active, 2);
        // A skipped Connect drops out too.
        let mut stages = stages;
        stages[5].1 = true;
        let (labels, active) = vault_stepper(stages, 7);
        assert_eq!(labels, ["Vault", "Backup", "Node", "Name", "Done"]);
        assert_eq!(active, 3);
    }

    #[test]
    fn bitcoin_and_testnet4_follow_the_step_list() {
        let _guard = crate::app::session::test_guard();
        for chain in [ChainId::Bitcoin, ChainId::Testnet4] {
            let mut vault = in_cube(chain);
            assert!(vault.create_vault, "{:?}", chain);
            assert_eq!(
                vault.create_vault_steps(),
                (FULL.to_vec(), 0),
                "{:?}",
                chain
            );
            for (index, stage) in [
                VaultStage::Vault,
                VaultStage::Policy,
                VaultStage::Keys,
                VaultStage::Backup,
                VaultStage::Connect,
                VaultStage::Node,
                VaultStage::Name,
                VaultStage::Done,
            ]
            .iter()
            .copied()
            .enumerate()
            {
                let steps = at(&mut vault, stage).create_vault_steps();
                assert_eq!(steps, (FULL.to_vec(), index), "{:?} {:?}", chain, stage);
            }

            // The descriptor backup is the second Backup step: still Backup.
            vault.current = vault
                .steps
                .iter()
                .rposition(|step| step.vault_stage() == Some(VaultStage::Backup))
                .unwrap();
            assert_eq!(vault.create_vault_steps().1, 3, "{:?}", chain);

            // A hardware key adds its Register step after Backup.
            vault.context.hw_is_used = true;
            let (labels, active) = at(&mut vault, VaultStage::Register).create_vault_steps();
            assert_eq!(
                labels,
                [
                    "Vault", "Policy", "Keys", "Backup", "Register", "Connect", "Node", "Name",
                    "Done"
                ],
                "{:?}",
                chain
            );
            assert_eq!(active, 4, "{:?}", chain);
        }
    }

    /// Connect stays on the stepper once the user has signed in there,
    /// although the step then skips.
    #[test]
    fn connect_stays_after_sign_in() {
        let _guard = crate::app::session::test_guard();
        for chain in [ChainId::Bitcoin, ChainId::Testnet4] {
            let mut vault = in_cube(chain);
            vault.context.connect_jwt = Some(zeroize::Zeroizing::new("jwt".to_string()));
            vault.context.use_coincube_connect = true;
            for &(index, stage) in &[
                (5, VaultStage::Node),
                (6, VaultStage::Name),
                (7, VaultStage::Done),
            ] {
                let steps = at(&mut vault, stage).create_vault_steps();
                assert_eq!(steps, (FULL.to_vec(), index), "{:?} {:?}", chain, stage);
            }
        }
        // A Cube that is already signed in passes Connect without showing
        // it, so the stepper leaves it out from the start.
        let mut vault = in_cube(ChainId::Bitcoin);
        vault.context.connect_signed_in_at_launch = true;
        let without_connect = ["Vault", "Policy", "Keys", "Backup", "Node", "Name", "Done"];
        assert_eq!(vault.create_vault_steps(), (without_connect.to_vec(), 0));
        vault.context.connect_jwt = Some(zeroize::Zeroizing::new("jwt".to_string()));
        assert_eq!(
            at(&mut vault, VaultStage::Node).create_vault_steps(),
            (without_connect.to_vec(), 4)
        );

        // Regtest has no Connect step.
        let mut vault = in_cube(ChainId::Regtest);
        let (labels, active) = at(&mut vault, VaultStage::Node).create_vault_steps();
        assert_eq!(
            labels,
            ["Vault", "Policy", "Keys", "Backup", "Node", "Name", "Done"]
        );
        assert_eq!(active, 4);
    }

    /// A Vault created inside an open Bitcoin Blake2b Cube gets the same
    /// Create Vault frame as Bitcoin's. Signed out, it may run on a local
    /// node, so Connect is off its stepper.
    #[test]
    fn an_in_cube_blake2b_vault_is_framed() {
        let _guard = crate::app::session::test_guard();
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let root = std::env::temp_dir().join(format!("create-vault-{}", uuid::Uuid::new_v4()));
            let master = Signer::generate(chain.bitcoin_network()).unwrap();
            let cube = crate::app::settings::CubeSettings::new("Fork".into(), chain)
                .with_master_signer(master.fingerprint());
            master
                .store_encrypted_seed_only_for_chain(
                    &CoincubeDirectory::new(root.clone()),
                    chain,
                    "2468",
                    &cube.id,
                    None,
                )
                .unwrap();
            crate::app::session::open(&cube.id, zeroize::Zeroizing::new("2468".into()));
            let (installer, _) = Installer::build_for_chain(
                CoincubeDirectory::new(root.clone()),
                chain,
                UserFlow::CreateWallet,
                true,
                Some(cube),
                None,
                None,
                false,
                None,
            )
            .unwrap();
            assert!(installer.context.in_cube_fork_vault);
            assert!(installer.create_vault, "{:?}", chain);
            let (labels, _) = installer.create_vault_steps();
            assert_eq!(
                labels,
                ["Vault", "Policy", "Keys", "Backup", "Node", "Name", "Done"],
                "{:?}",
                chain
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    /// A Bitcoin Blake2b install that is not inside an open Cube, and the
    /// in-Cube Vault restore, keep the installer's layout.
    #[test]
    fn only_create_vault_is_framed() {
        let root = || {
            CoincubeDirectory::new(
                std::env::temp_dir().join(format!("create-vault-{}", uuid::Uuid::new_v4())),
            )
        };
        let flows = [
            (ChainId::BitcoinBlake2b, UserFlow::CreateWallet),
            (ChainId::Bitcoin, UserFlow::RestoreVaultFromRecoveryKit),
            (ChainId::Bitcoin, UserFlow::AddWallet),
        ];
        for (chain, flow) in flows.iter().cloned() {
            let name = format!("{:?} {:?}", chain, flow);
            match Installer::try_new_for_chain(
                root(),
                chain,
                flow,
                true,
                None,
                None,
                None,
                false,
                None,
            ) {
                Ok((installer, _)) => assert!(!installer.create_vault, "{}", name),
                // Refused before it is built: nothing to frame.
                Err(_) => assert!(chain.is_blake2b(), "{}", name),
            }
        }
    }

    // Serialize the process-global authenticated fixture through async rendering.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn create_vault_draws_its_heading_and_stepper() {
        let _guard = crate::app::session::test_guard();
        let mut vault = in_cube(ChainId::Bitcoin);
        let shown = rendered_labels(vault.view()).await;
        assert!(shown.iter().any(|t| t == CREATE_VAULT_TITLE), "{:?}", shown);
        for label in FULL {
            assert!(
                shown.iter().any(|t| t == label),
                "missing {:?} in {:?}",
                label,
                shown
            );
        }
        // The installer's own "n | total" progress row stays out.
        assert!(!shown.iter().any(|t| t.contains(" | ")), "{:?}", shown);
        // So does the restore frame.
        assert!(
            !shown.iter().any(|t| t == "Import or restore"),
            "{:?}",
            shown
        );

        // The Name step's own title still shows under the heading.
        let shown = rendered_labels(at(&mut vault, VaultStage::Name).view()).await;
        assert!(
            shown.iter().any(|t| t == "Give your wallet an alias"),
            "{:?}",
            shown
        );
    }

    /// Opt-in frames of every Create Vault step: `LAUNCHER_CAPTURE_DIR=<dir>
    /// cargo test -p coincube-gui --lib create_vault_capture -- --ignored`.
    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn create_vault_capture() {
        let _guard = crate::app::session::test_guard();
        use crate::home::launcher_capture::{renderer, save};
        let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let mut renderer = renderer().expect("wgpu headless renderer");
        let size = iced::Size::new(1024.0, 768.0);
        let mut vault = in_cube(ChainId::Bitcoin);
        // The steps after Keys need the policy it would build.
        vault.context.descriptor = Some(
            std::str::FromStr::from_str("wsh(or_d(pk([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<0;1>/*),and_v(v:pkh([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<2;3>/*),older(52596))))#jz5sm0xn")
                .unwrap(),
        );
        for index in 0..vault.steps.len() {
            if vault.steps[index].skip(&vault.context) {
                continue;
            }
            vault.current = index;
            vault.steps[index].load_context(&vault.context);
            for (appearance, theme) in [
                ("dark", coincube_ui::theme::Theme::dark()),
                ("light", coincube_ui::theme::Theme::light()),
            ] {
                save(
                    &mut renderer,
                    vault.view(),
                    size,
                    &theme,
                    directory.join(format!("create-vault-step-{:02}-{}.png", index, appearance)),
                );
            }
        }
    }
}

#[cfg(test)]
mod restore_frame_tests {
    use super::*;
    use crate::home::restore_frame::rendered_labels;

    fn installer(flow: UserFlow) -> Installer {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("restore-frame-{}", uuid::Uuid::new_v4())),
        );
        // Rendering CreateWallet still needs a genuinely authenticated Cube.
        let _guard = matches!(flow, UserFlow::CreateWallet).then(crate::app::session::test_guard);
        let cube = if matches!(flow, UserFlow::CreateWallet) {
            let master = coincube_core::signer::MasterSigner::generate(Network::Bitcoin).unwrap();
            let fp = master.fingerprint(&bitcoin::secp256k1::Secp256k1::new());
            let cube = crate::app::settings::CubeSettings::new(
                "Synthetic".into(),
                crate::chain::ChainId::Bitcoin,
            )
            .with_master_signer(fp);
            crate::app::session::store_unlocked_signer(&cube.id, fp, master);
            Some(cube)
        } else {
            None
        };
        let installer = Installer::new(
            root,
            bitcoin::Network::Bitcoin,
            flow,
            false,
            cube,
            None,
            None,
            false,
            None,
        )
        .expect("a Bitcoin fixture installer")
        .0;
        if _guard.is_some() {
            crate::app::session::close();
        }
        installer
    }

    fn has(shown: &[String], label: &str) -> bool {
        shown.iter().any(|text| text == label)
    }

    #[tokio::test]
    async fn a_recovery_kit_restore_runs_in_import_or_restore() {
        let kit = installer(UserFlow::RestoreFromRecoveryKit { cube_uuid: None });
        let shown = rendered_labels(kit.view()).await;
        for label in [
            "Import or restore",
            "Bitcoin SHA-256 · Mainnet",
            "Recovery Kit",
            "Recover with phone",
            "Restore from Cube Recovery Kit",
            "Cube",
            "Password",
            "PIN",
            "Done",
            "Close",
        ] {
            assert!(has(&shown, label), "missing {:?} in {:?}", label, shown);
        }
        // The installer's own "n | total" progress row stays out.
        assert!(
            !shown.iter().any(|text| text.contains(" | ")),
            "{:?}",
            shown
        );

        // Every other flow keeps the installer's layout, including after a
        // framed view on the same thread.
        let create = installer(UserFlow::CreateWallet);
        let shown = rendered_labels(create.view()).await;
        assert!(!has(&shown, "Import or restore"), "{:?}", shown);
        assert!(shown.iter().any(|text| text.contains(" | ")), "{:?}", shown);
    }

    /// The Kit's first step has no Back: like every Import or restore path,
    /// its path list and Close leave it. A path list press goes back to the
    /// Launcher, which takes the path to open once.
    #[tokio::test]
    async fn a_recovery_kit_restore_leaves_from_its_path_list() {
        use iced_runtime::futures::futures::StreamExt;

        let mut kit = installer(UserFlow::RestoreFromRecoveryKit { cube_uuid: None });
        let shown = rendered_labels(kit.view()).await;
        assert!(!has(&shown, "Back"), "{:?}", shown);

        let task = kit.update(Message::LeaveRestore(RestoreDestination::Phone));
        let mut stream = iced_runtime::task::into_stream(task).unwrap();
        assert!(matches!(
            stream.next().await,
            Some(iced_runtime::Action::Output(Message::BackToApp(
                bitcoin::Network::Bitcoin
            )))
        ));
        assert_eq!(kit.take_restore_exit(), Some(RestoreDestination::Phone));
        assert_eq!(kit.take_restore_exit(), None);
    }

    /// Opt-in frames: `LAUNCHER_CAPTURE_DIR=<dir> cargo test -p coincube-gui
    /// --lib restore_paths_capture -- --ignored`.
    #[test]
    #[ignore]
    fn restore_paths_capture() {
        use crate::home::launcher_capture::{renderer, save};
        let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let mut renderer = renderer().expect("wgpu headless renderer");
        let size = iced::Size::new(1440.0, 900.0);
        let kit = installer(UserFlow::RestoreFromRecoveryKit { cube_uuid: None });
        let phone = installer(UserFlow::RecoverOwnCubeWithPhone {
            cube_id: 7,
            full_cube: true,
        });
        for (name, installer) in [("kit", &kit), ("phone", &phone)] {
            save(
                &mut renderer,
                installer.view(),
                size,
                &coincube_ui::theme::Theme::dark(),
                directory.join(format!("restore-{name}-dark.png")),
            );
        }
    }

    /// Opt-in frames of every Create Cube step the installer draws, at the
    /// minimum window in both themes: `LAUNCHER_CAPTURE_DIR=<dir> cargo test
    /// -p coincube-gui --lib create_steps_capture -- --ignored`.
    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn create_steps_capture() {
        use crate::home::launcher_capture::{renderer, save};
        let Ok(directory) = std::env::var("LAUNCHER_CAPTURE_DIR") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let mut renderer = renderer().expect("wgpu headless renderer");
        let size = iced::Size::new(1024.0, 768.0);
        let mut create = installer(UserFlow::CreateWallet);
        // The descriptor steps need the policy the Keys step would build.
        create.context.descriptor = Some(
            std::str::FromStr::from_str("wsh(or_d(pk([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<0;1>/*),and_v(v:pkh([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<2;3>/*),older(52596))))#jz5sm0xn")
                .unwrap(),
        );
        for index in 0..create.steps.len() {
            create.current = index;
            create.steps[index].load_context(&create.context);
            for (appearance, theme) in [
                ("dark", coincube_ui::theme::Theme::dark()),
                ("light", coincube_ui::theme::Theme::light()),
            ] {
                save(
                    &mut renderer,
                    create.view(),
                    size,
                    &theme,
                    directory.join(format!("create-step-{index:02}-{appearance}.png")),
                );
            }
        }
        // Signed in to Connect, the Node step leads with Connect instead.
        create.context.use_coincube_connect = true;
        // `SelectBitcoindTypeStep`'s place in the CreateWallet list.
        let node = 7;
        create.current = node;
        create.steps[node].load_context(&create.context);
        for (appearance, theme) in [
            ("dark", coincube_ui::theme::Theme::dark()),
            ("light", coincube_ui::theme::Theme::light()),
        ] {
            save(
                &mut renderer,
                create.view(),
                size,
                &theme,
                directory.join(format!("create-step-{node:02}-connect-{appearance}.png")),
            );
        }
    }

    /// A Recovery Kit or phone restore ends on Done: "[Cube] is restored"
    /// with Open Cube as the only action, and the Cube opens from it, not
    /// as soon as it is written.
    #[tokio::test]
    async fn a_restore_ends_on_done_and_opens_from_open_cube() {
        for flow in [
            UserFlow::RestoreFromRecoveryKit { cube_uuid: None },
            UserFlow::RecoverOwnCubeWithPhone {
                cube_id: 7,
                full_cube: true,
            },
        ] {
            let mut restore = installer(flow);
            restore.current = restore.steps.len() - 1;
            let mut hws = HardwareWallets::new(restore.datadir.clone(), Network::Bitcoin);
            let step = restore.steps.last_mut().unwrap();
            drop(step.update(&mut hws, Message::InstallFinished));
            assert!(step.restore_done(), "Done waits for Open Cube");
            let shown = rendered_labels(restore.view()).await;
            for label in ["Your Cube is restored", "Open Cube", "Done"] {
                assert!(has(&shown, label), "missing {:?} in {:?}", label, shown);
            }
            for label in ["Close", "Back", "Restore from Cube Recovery Kit"] {
                assert!(!has(&shown, label), "unexpected {:?} in {:?}", label, shown);
            }
        }
        // Other flows still exit as soon as the install finishes.
        let mut plain = Final::new();
        let mut hws = HardwareWallets::new(
            CoincubeDirectory::new(std::env::temp_dir()),
            Network::Bitcoin,
        );
        drop(plain.update(&mut hws, Message::InstallFinished));
        assert!(!plain.restore_done());
    }

    #[tokio::test]
    async fn a_phone_recovery_runs_in_import_or_restore() {
        let phone = installer(UserFlow::RecoverOwnCubeWithPhone {
            cube_id: 7,
            full_cube: true,
        });
        let shown = rendered_labels(phone.view()).await;
        for label in [
            "Import or restore",
            "Recover with phone",
            "Cube",
            "Approve",
            "PIN",
            "Done",
        ] {
            assert!(has(&shown, label), "missing {:?} in {:?}", label, shown);
        }
        assert!(!has(&shown, "Password"), "{:?}", shown);

        // A descriptor-only phone recovery is not a Cube restore.
        let vault = installer(UserFlow::RecoverOwnCubeWithPhone {
            cube_id: 7,
            full_cube: false,
        });
        let shown = rendered_labels(vault.view()).await;
        assert!(!has(&shown, "Import or restore"), "{:?}", shown);
    }
}

/// The Testnet4 resume gate is compiled only into the integration runner.
/// These cases are the ones that decide whether an existing seed is kept,
/// replaced, or refused. They do not touch the system keystore.
#[cfg(all(test, feature = "integration-tests"))]
mod testnet4_seed_resume_tests {
    use super::*;
    use crate::chain::ChainId;
    use coincube_core::{
        descriptors::CoincubeDescriptor,
        miniscript::bitcoin::bip32::DerivationPath,
        seed_crypt::{self, ENCRYPTED_V2_MARKER, ENCRYPTED_V3_MARKER},
        signer::{MasterSigner, MnemonicFileName},
    };
    use std::path::PathBuf;
    use std::str::FromStr;

    const PIN: &str = "2468";
    const TIMESTAMP: i64 = 1_700_000_000;

    struct Sealed {
        root: CoincubeDirectory,
        ctx: Context,
        signer: Signer,
        wallet_id: WalletId,
        path: PathBuf,
        bytes: Vec<u8>,
        secret: seed_crypt::DeviceSecret,
    }

    impl Drop for Sealed {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.root.path());
        }
    }

    fn key(signer: &Signer, branches: &str) -> String {
        let path = DerivationPath::from_str("48'/1'/0'/2'").unwrap();
        format!(
            "[{}/48'/1'/0'/2']{}/<{branches}>/*",
            signer.fingerprint(),
            signer.get_extended_pubkey(&path)
        )
    }

    fn descriptor(primary: &Signer, recovery: &Signer) -> CoincubeDescriptor {
        CoincubeDescriptor::from_str(&format!(
            "wsh(or_d(pk({}),and_v(v:pkh({}),older(3))))",
            key(primary, "0;1"),
            key(recovery, "2;3")
        ))
        .expect("testnet4 descriptor")
    }

    fn checksum(descriptor: &CoincubeDescriptor) -> String {
        descriptor
            .to_string()
            .split_once('#')
            .map(|(_, sum)| sum.to_owned())
            .expect("descriptor display includes its checksum")
    }

    fn seed_path(root: &CoincubeDirectory, signer: &Signer, wallet_id: &WalletId) -> PathBuf {
        MasterSigner::mnemonics_folder_for_chain(root.path(), ChainId::Testnet4).join(
            MnemonicFileName {
                fingerprint: signer.fingerprint(),
                descriptor_info: Some((
                    wallet_id.descriptor_checksum.clone(),
                    wallet_id.timestamp.unwrap(),
                )),
            }
            .to_string(),
        )
    }

    fn seal(primary_is_signer: bool, cube_id: &str) -> Sealed {
        let root = CoincubeDirectory::new(
            std::env::temp_dir().join(format!("t4-seed-resume-{}", uuid::Uuid::new_v4())),
        );
        let signer = Signer::generate(Network::Testnet4).unwrap();
        let other = Signer::generate(Network::Testnet4).unwrap();
        let primary = if primary_is_signer { &signer } else { &other };
        let desc = descriptor(primary, &signer);
        let wallet_id = WalletId::new(checksum(&desc), Some(TIMESTAMP));
        let secret = zeroize::Zeroizing::new([7u8; 32]);
        signer
            .store_encrypted_for_chain(
                &root,
                ChainId::Testnet4,
                &wallet_id.descriptor_checksum,
                TIMESTAMP,
                PIN,
                cube_id,
                Some(&secret),
            )
            .unwrap();
        let path = seed_path(&root, &signer, &wallet_id);
        let bytes = std::fs::read(&path).unwrap();
        let mut ctx = Context::new_for_chain(ChainId::Testnet4, root.clone(), None, None);
        ctx.descriptor = Some(desc);
        ctx.cube_pin = Some(zeroize::Zeroizing::new(PIN.to_string()));
        if !cube_id.is_empty() {
            ctx.cube_id = Some(cube_id.to_string());
        }
        Sealed {
            root,
            ctx,
            signer,
            wallet_id,
            path,
            bytes,
            secret,
        }
    }

    fn fresh() -> Sealed {
        seal(true, "cube-testnet4")
    }

    fn verify(sealed: &Sealed) -> Result<bool, Error> {
        integration::verify_existing_testnet4_seed(
            &sealed.ctx,
            &sealed.wallet_id,
            &sealed.signer,
            &sealed.secret,
        )
    }

    fn assert_refused(
        label: &str,
        ctx: &Context,
        wallet_id: &WalletId,
        signer: &Signer,
        secret: &seed_crypt::DeviceSecret,
        path: &std::path::Path,
        bytes: &[u8],
    ) {
        let error = integration::verify_existing_testnet4_seed(ctx, wallet_id, signer, secret)
            .expect_err(label);
        assert!(
            error
                .to_string()
                .contains("Existing Testnet4 seed could not be verified"),
            "{label}: {error}",
            label = label,
            error = error,
        );
        assert_eq!(
            std::fs::read(path).unwrap(),
            bytes,
            "{label} must not rewrite the seed",
            label = label,
        );
    }

    fn rewrite(path: &std::path::Path, bytes: &[u8]) {
        // `restrict_permissions` seals seed files (mode 0o400 on Unix, the
        // read-only attribute on Windows), so a later test write must be
        // granted by the test on every platform. The verifier itself must not
        // need that.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        #[cfg(not(unix))]
        {
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            std::fs::set_permissions(path, permissions).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn refuse_sealed(label: &str, sealed: &Sealed) {
        assert_refused(
            label,
            &sealed.ctx,
            &sealed.wallet_id,
            &sealed.signer,
            &sealed.secret,
            &sealed.path,
            &sealed.bytes,
        );
    }

    #[test]
    fn a_matching_seed_verifies_twice_and_a_missing_file_is_absent() {
        let sealed = fresh();
        assert!(verify(&sealed).unwrap());
        assert_eq!(std::fs::read(&sealed.path).unwrap(), sealed.bytes);
        assert!(verify(&sealed).unwrap());
        assert_eq!(std::fs::read(&sealed.path).unwrap(), sealed.bytes);
        assert!(!sealed
            .root
            .network_directory(ChainId::Bitcoin)
            .path()
            .exists());

        std::fs::remove_file(&sealed.path).unwrap();
        assert!(!verify(&sealed).unwrap());
        assert!(!sealed.path.exists());
    }

    #[test]
    fn conflicts_are_refusals_rather_than_a_missing_seed() {
        let sealed = fresh();
        let refuse = |label: &str,
                      ctx: &Context,
                      wallet_id: &WalletId,
                      secret: &seed_crypt::DeviceSecret,
                      bytes: &[u8]| {
            assert_refused(
                label,
                ctx,
                wallet_id,
                &sealed.signer,
                secret,
                &sealed.path,
                bytes,
            );
        };

        let mut network = sealed.ctx.clone();
        network.network = Network::Signet;
        refuse(
            "installer network",
            &network,
            &sealed.wallet_id,
            &sealed.secret,
            &sealed.bytes,
        );

        let mut chain = sealed.ctx.clone();
        chain.bitcoin_config.chain = ChainId::Signet;
        refuse(
            "chain",
            &chain,
            &sealed.wallet_id,
            &sealed.secret,
            &sealed.bytes,
        );
        assert!(!sealed
            .root
            .network_directory(ChainId::Signet)
            .path()
            .join("mnemonics")
            .exists());

        let mut checksum = sealed.wallet_id.clone();
        checksum.descriptor_checksum = "deadbeef".into();
        let looked_up = seed_path(&sealed.root, &sealed.signer, &checksum);
        refuse(
            "checksum",
            &sealed.ctx,
            &checksum,
            &sealed.secret,
            &sealed.bytes,
        );
        assert!(!looked_up.exists());

        let mut cube = sealed.ctx.clone();
        cube.cube_id = Some("other-cube".into());
        refuse(
            "cube id",
            &cube,
            &sealed.wallet_id,
            &sealed.secret,
            &sealed.bytes,
        );

        let wrong_secret = zeroize::Zeroizing::new([9u8; 32]);
        refuse(
            "device secret",
            &sealed.ctx,
            &sealed.wallet_id,
            &wrong_secret,
            &sealed.bytes,
        );

        let mut pin = sealed.ctx.clone();
        pin.cube_pin = Some(zeroize::Zeroizing::new("9999".into()));
        refuse(
            "pin",
            &pin,
            &sealed.wallet_id,
            &sealed.secret,
            &sealed.bytes,
        );

        let mut missing_pin = sealed.ctx.clone();
        missing_pin.cube_pin = None;
        refuse(
            "missing pin",
            &missing_pin,
            &sealed.wallet_id,
            &sealed.secret,
            &sealed.bytes,
        );

        let other = Signer::generate(Network::Testnet4).unwrap();
        let wrong_phrase = seed_crypt::encrypt(
            other.mnemonic().join(" ").as_bytes(),
            PIN,
            sealed.ctx.seed_cube_id(),
            Some(&sealed.secret),
        )
        .unwrap();
        rewrite(&sealed.path, &wrong_phrase);
        refuse(
            "other phrase",
            &sealed.ctx,
            &sealed.wallet_id,
            &sealed.secret,
            &wrong_phrase,
        );

        let pin_only = seed_crypt::encrypt(
            sealed.signer.mnemonic().join(" ").as_bytes(),
            PIN,
            sealed.ctx.seed_cube_id(),
            None,
        )
        .unwrap();
        assert!(pin_only.starts_with(ENCRYPTED_V2_MARKER));
        rewrite(&sealed.path, &pin_only);
        refuse(
            "pin-only v2 file",
            &sealed.ctx,
            &sealed.wallet_id,
            &sealed.secret,
            &pin_only,
        );

        let mut huge = ENCRYPTED_V3_MARKER.to_vec();
        huge.resize(4097, 0);
        rewrite(&sealed.path, &huge);
        refuse(
            "oversized file",
            &sealed.ctx,
            &sealed.wallet_id,
            &sealed.secret,
            &huge,
        );

        std::fs::remove_file(&sealed.path).unwrap();
        std::fs::create_dir(&sealed.path).unwrap();
        let error = verify(&sealed).expect_err("directory");
        assert!(
            error
                .to_string()
                .contains("Existing Testnet4 seed could not be verified"),
            "{error}",
            error = error,
        );
        assert!(sealed.path.is_dir());

        let recovery_only = seal(false, "cube-testnet4");
        refuse_sealed(
            "recovery-path key is not the primary vault key",
            &recovery_only,
        );

        let unbound = seal(true, "");
        assert!(unbound.ctx.seed_cube_id().is_empty());
        refuse_sealed("unbound cube", &unbound);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_to_a_valid_seed_is_refused_without_following_it() {
        let sealed = fresh();
        let target = sealed.path.with_file_name("target-seed.txt");
        std::fs::rename(&sealed.path, &target).unwrap();
        std::os::unix::fs::symlink(&target, &sealed.path).unwrap();
        let error = verify(&sealed).expect_err("symlink");
        assert!(error
            .to_string()
            .contains("Existing Testnet4 seed could not be verified"));
        assert!(sealed
            .path
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), sealed.bytes);
        assert!(target.is_file());
    }

    #[test]
    fn resume_refuses_the_wrong_context_before_creating_a_datadir() {
        let signer = || Arc::new(Mutex::new(Signer::generate(Network::Testnet4).unwrap()));
        let wallet_id = WalletId::new("unused".into(), Some(TIMESTAMP));
        let run = |ctx: Context, label: &str| {
            let root = ctx.coincube_directory.path().to_path_buf();
            let error = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(integration::install_testnet4_wallet_resumable(
                    ctx,
                    wallet_id.clone(),
                    signer(),
                ))
                .expect_err(label);
            assert!(
                error
                    .to_string()
                    .contains("Invalid Testnet4 resume context"),
                "{label}: {error}",
                label = label,
                error = error,
            );
            assert!(
                !root.exists(),
                "{label} must not create a datadir",
                label = label,
            );
        };

        let root = |label: &str| {
            CoincubeDirectory::new(std::env::temp_dir().join(format!(
                "t4-resume-context-{label}-{}",
                uuid::Uuid::new_v4()
            )))
        };
        run(
            Context::new(Network::Signet, root("signet"), None, None),
            "signet",
        );

        let mut recovered =
            Context::new_for_chain(ChainId::Testnet4, root("recovered"), None, None);
        recovered.cube_id = Some("cube-testnet4".into());
        recovered.recovered_signer = Some(Arc::new(Signer::generate(Network::Testnet4).unwrap()));
        run(recovered, "recovered signer");

        run(
            Context::new_for_chain(ChainId::Testnet4, root("anonymous"), None, None),
            "empty cube id",
        );
    }
}
