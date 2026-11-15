use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::{
    advanced::widget::Id,
    alignment::Horizontal,
    widget::{pick_list, scrollable, Space, Stack, Toggler},
    Alignment, Length, Subscription, Task,
};

use coincube_core::{bip39, miniscript::bitcoin::Network};
use coincube_ui::{
    component::{button, card, notification, spinner, text::*},
    icon, image, theme,
    widget::{
        action::Action as FocusableAction, modal::Modal, CheckBox, Column, ColumnExt, Container,
        Element, Row,
    },
};
use coincubed::config::ConfigError;
use tokio::runtime::Handle;

use crate::app::state::settings::recovery_kit::encrypt_and_upload as recovery_kit_upload;
use crate::chain::{ChainId, ChainIdExt};
use crate::feature_flags;
use crate::pin_input;
use crate::recover_vault::{self, RecoverVaultMessage, RecoverVaultPanel};
use crate::services::coincube::{
    vault_presence_report, CoincubeClient, CoincubeError, CubeLimitsResponse, CubeResponse,
    OwnerSelfRecoverySummary, RegisterCubeRequest, UpdateCubeRequest,
};
#[cfg(not(target_os = "macos"))]
use crate::services::passkey::CeremonyMode;
use crate::services::passkey::{self as passkey_svc, CeremonyOutcome, PasskeyCeremony};
use crate::services::unlock::{self, creation_gate};
use crate::{
    app::{
        self,
        settings::{
            self,
            global::{AccountTier, GlobalSettings},
            BitcoinImport, CubeSettings, WalletSettings,
        },
        state::{
            connect::{
                account::{RecoveryServiceTarget, RecoveryServicesState},
                ConnectAccountPanel,
            },
            settings::general,
        },
        view::{BackupWalletMessage, ConnectAccountMessage, RecoveryKitMessage},
    },
    delete::{delete_wallet, DeleteError},
    dir::{CoincubeDirectory, NetworkDirectory},
    installer::UserFlow,
};
use coincube_core::signer::{MasterSigner, MASTER_SEED_LABEL};

mod launcher_activity;
#[cfg(test)]
pub(crate) mod launcher_capture;
mod launcher_catalog;
mod launcher_chrome;
pub(crate) mod launcher_coin;
mod launcher_gpu;
pub(crate) mod launcher_opening;
pub(crate) mod launcher_presenter;
pub(crate) mod launcher_scene;
mod remote_catalog;
pub(crate) mod restore_frame;
pub(crate) use launcher_catalog::LauncherSceneContext;
#[cfg(feature = "integration-tests")]
pub mod integration;

/// The firmware advisory whose one-time incident notice this install still
/// owes the user, if any.
///
/// One notice per advisory id, recorded in the app-wide global settings. If
/// the settings file can't be read the notice shows: a security notice seen
/// twice costs a click, one never seen costs the whole point of shipping it.
fn pending_advisory_notice(
    datadir_path: &CoincubeDirectory,
) -> Option<&'static crate::hw_advisory::Advisory> {
    let path = GlobalSettings::path(datadir_path);
    crate::hw_advisory::ADVISORIES
        .iter()
        .find(|advisory| !GlobalSettings::advisory_notice_seen(&path, advisory.id))
}

const NETWORKS: [ChainId; 5] = ChainId::LAUNCHER;

/// A pre-unlock catalog action names the persisted Cube, never its position
/// in a list that a background refresh may have reordered.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CubeCatalogKey {
    pub chain: ChainId,
    pub cube_id: String,
}

fn catalog_display_name(name: &str) -> String {
    crate::cube_display::name(name)
}

/// Connect registration and remote-Cube matching must inspect every chain's
/// settings directory, including BTCB2. `NETWORKS` is only the baseline
/// launcher selector and intentionally excludes the two fork identities.
fn locally_stored_cubes(datadir: &CoincubeDirectory) -> Vec<CubeSettings> {
    ChainId::ALL
        .iter()
        .flat_map(|chain| {
            settings::Settings::from_file(&datadir.network_directory(*chain))
                .map(|settings| settings.cubes)
                .unwrap_or_default()
        })
        .collect()
}

#[derive(Debug, Clone)]
pub enum RecoveryServiceDestination {
    Local(ChainId, CubeSettings, app::Config),
    Remote(ChainId),
}

/// The Account row is only a selector. Resolve its server ID and network
/// against a fresh authenticated response, then match the resulting UUID in
/// that chain's local settings before admitting the ordinary unlock path.
fn resolve_recovery_service_destination(
    datadir: &CoincubeDirectory,
    target: &RecoveryServiceTarget,
    cubes: &[CubeResponse],
) -> Result<RecoveryServiceDestination, String> {
    let chain = ChainId::from_api_str(&target.network)
        .ok_or_else(|| "Unsupported Connect Cube network".to_string())?;
    let mut matches = cubes
        .iter()
        .filter(|cube| cube.id == target.cube_id && cube.network == target.network);
    let server_cube = matches
        .next()
        .ok_or_else(|| "Connect Cube no longer belongs to this account".to_string())?;
    if matches.next().is_some() || server_cube.uuid.trim().is_empty() {
        return Err("Connect returned an ambiguous Cube identity".to_string());
    }

    let network_dir = datadir.network_directory(chain);
    let local_cubes = match settings::Settings::from_file(&network_dir) {
        Ok(settings) => settings.cubes,
        Err(settings::SettingsError::NotFound) => {
            return Ok(RecoveryServiceDestination::Remote(chain))
        }
        Err(error) => return Err(format!("Could not read local Cube settings: {error}")),
    };
    let mut local_matches = local_cubes
        .iter()
        .filter(|cube| cube.id == server_cube.uuid);
    let Some(local_cube) = local_matches.next() else {
        return Ok(RecoveryServiceDestination::Remote(chain));
    };
    if local_matches.next().is_some() || local_cube.network != chain {
        return Err("Local Cube identity does not match its chain".to_string());
    }
    let config_path = network_dir.path().join(app::config::DEFAULT_FILE_NAME);
    let config = match app::Config::from_file(&config_path) {
        Ok(config) => config,
        Err(app::config::ConfigError::NotFound) => {
            let config = app::Config::new(false);
            config.to_file(&config_path).map_err(|error| {
                format!("Could not initialize local GUI configuration: {error}")
            })?;
            config
        }
        Err(error) => return Err(format!("Could not read local GUI configuration: {error}")),
    };
    Ok(RecoveryServiceDestination::Local(
        chain,
        local_cube.clone(),
        config,
    ))
}

#[derive(Debug, Clone)]
pub enum State {
    Unchecked,
    Cubes {
        cubes: Vec<CubeSettings>,
        create_cube: bool,
        /// The chain whose directory these records were read from
        /// (`<datadir>/<source.dir_name()>/settings.json`). Carried with the
        /// list, not re-derived from a record, so that opening a Cube can
        /// check the record's own `network` against where it actually came
        /// from — a copied or edited file claiming another chain is refused
        /// instead of being trusted about its own directory.
        source: crate::chain::ChainId,
    },
    /// No Cube on this device. Carries the same `create_cube` flag as
    /// [`State::Cubes`] because this screen is not always empty: remote Cubes
    /// registered in Connect are listed here for restore, and that list needs
    /// the same "Create Cube" button → dedicated form navigation rather than a
    /// form stacked underneath it. An empty first run stays on the Launcher
    /// until creation is explicitly opened.
    NoCube {
        create_cube: bool,
    },
    RecoveryInput,
    /// The creation-time backup step, between "Create Cube" and the Cube
    /// actually existing. See [`CreationBackupStep`].
    CreationBackup(CreationBackupStep),
}

/// Where the user is inside the creation-time backup step.
///
/// # Why creation has to ask
///
/// Sealing the seed file to an OS-keystore device secret (`ENCRYPTED_V3`)
/// means a copied datadir no longer opens — see
/// [`crate::services::unlock::creation_gate`]. That removed the accidental
/// backup users used to have, so the Recovery Kit stopped being advisory. A
/// user who creates a Cube, skips the backup and loses the machine has lost
/// the funds, and no support action gets them back.
///
/// # Nothing is on disk while this runs
///
/// The seed is generated into memory only. The seed file, the settings entry
/// and the OS-keystore device secret are all written by
/// [`Home::finalize_cube_creation`] *after* this step resolves, so abandoning
/// it leaves no half-created Cube behind — see
/// `abandoning_the_backup_step_leaves_no_half_created_cube`.
///
/// The screens themselves are the Settings backup wizard's, reused verbatim
/// via [`app::view::settings::backup`]'s message-generic views, so the two
/// places a user is shown their seed phrase cannot drift apart. The Recovery
/// Kit screens come from the Settings Kit flow the same way.
///
/// # One flow, both unlock methods
///
/// PIN and passkey Cubes run the *same* step machine and get the same three
/// exits: write the phrase down, create a Recovery Kit, or take the recorded
/// acknowledgement. That is only possible because both derive a **12-word**
/// mnemonic — `generate` and `from_prf_output` produce the same shape, so one
/// phrase screen and one 12-word restore grid serve both. Nothing here
/// branches on the unlock method except the copy that has to (a passkey Cube's
/// folder-copy warning is not the PIN one) and the Kit's availability, which
/// depends on Connect rather than on the Cube.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationBackupStep {
    /// Intro + security warning; the bool is the "I understand" checkbox.
    Intro(bool),
    /// The seed phrase itself, in a grid.
    Phrase,
    /// Type back three of the words to prove they were written down.
    Verification {
        word_indices: [usize; 3],
        word_inputs: [String; 3],
        error: Option<String>,
    },
    /// "I'll do this later". The bool is active acceptance of
    /// [`creation_gate::BYPASS_ACKNOWLEDGEMENT`], which is what gets persisted
    /// to the Cube as a [`creation_gate::CreationBackupBypass`].
    Bypass { acknowledged: bool },

    /// The backup hub: Seed phrase and Recovery Kit, either or both. Each
    /// method's screens return here when they finish, and Continue writes the
    /// Cube with whatever evidence was collected. The entry point for every
    /// Cube, whichever unlock method it uses.
    Choice,
    /// Choosing the Recovery Kit's recovery password. The fields themselves
    /// live on [`Home`] (`creation_kit_password` and friends) rather than in
    /// the variant, because this enum is `Clone + PartialEq + Eq` and a
    /// password has no business being either.
    KitPassword,
    /// Registering the Cube with Connect and uploading the Kit.
    KitUploading,
}

/// A passkey credential that has been registered but whose Cube has **not** been
/// written to disk yet.
///
/// Passkey creation used to persist the Cube the instant the ceremony returned,
/// which meant it never passed through the creation-backup step and reached
/// `settings.json` with `creation_backup_required = false` — the shape
/// [`creation_gate::evaluate_for_cube`] reads as "predates the gate" and waves
/// through. A user could fund a Cube whose only recovery was the credential
/// itself, without ever being shown the acknowledgement, let alone accepting it.
///
/// So the registration result is parked here instead, and the Cube is written
/// only by [`Home::finalize_passkey_cube_creation`] once the step resolves.
/// Abandoning leaves nothing behind — the credential exists in the platform
/// authenticator, which is inert without a Cube that references it.
///
/// # What is *not* kept here
///
/// Neither the PRF output nor the derived signer: the fingerprint is taken at
/// ceremony time and both are dropped before this struct is built. A passkey
/// Cube stores no seed either — it re-derives from the credential on every
/// open.
///
/// The derived *phrase* is a different matter, and is not kept in this struct
/// but does outlive the ceremony. `passkey_registration_succeeded` puts it in
/// `creation_backup_words` under `Zeroizing`, where it stays until the backup
/// step ends and `scrub_creation_seed` clears each word. That is deliberate:
/// the Recovery Kit branch has to encrypt the seed, and the PRF output is
/// reachable at the ceremony and nowhere afterwards without a second Touch ID
/// prompt. It is the same bargain, the same field and the same scrub the PIN
/// path already uses.
#[derive(Debug, Clone)]
struct PendingPasskeyCube {
    /// Base64 WebAuthn credential id, as persisted in `PasskeyMetadata`.
    credential_id: String,
    /// Fingerprint of the master signer this credential derives.
    master_fingerprint: coincube_core::miniscript::bitcoin::bip32::Fingerprint,
}

fn bip39_suggestions(prefix: &str, limit: usize) -> Vec<String> {
    if prefix.is_empty() || limit == 0 {
        return Vec::new();
    }

    bip39::Language::English
        .words_by_prefix(&prefix.to_lowercase())
        .iter()
        .take(limit)
        .map(|word| (*word).to_string())
        .collect()
}

/// A cube that exists on the Connect server but has no local data on this machine.
#[derive(Debug, Clone)]
pub struct RemoteCube {
    /// Numeric Connect cube id — needed to launch the phone (owner-keychain)
    /// restore flow, which keys off the id rather than the uuid.
    pub id: u64,
    pub uuid: String,
    pub name: String,
    pub network: String, // API string: "mainnet", "testnet", etc.
    /// Whether the server holds a Cube Recovery Kit for this cube.
    pub has_recovery_kit: bool,
    /// Whether that kit carries the encrypted seed half — the piece a
    /// full restore needs. Password recovery is offered only when this is
    /// `true` (kit present *and* the seed inside it).
    pub has_encrypted_seed: bool,
    /// Whether an owner-keychain ("phone") envelope set has been sealed and
    /// uploaded for this cube — the signal that passwordless phone recovery
    /// is available. Derived from `/recovery-kit/status`'s `ownerSelf` block.
    pub phone_recoverable: bool,
    /// Whether the phone envelope set includes the seed (Full Cube) vs only
    /// the descriptor (Vault-only). Drives the restore scope + label.
    pub phone_full_cube: bool,
    /// The Connect-blinding public key registered for this Cube, derived
    /// from its seed. A recovery phrase typed for this Cube must derive the
    /// same key before the import takes the Cube's identity.
    pub encryption_pubkey: Option<String>,
    /// Whether Connect reports a Vault on this Cube; `None` when no device
    /// has reported it, or the API predates the field.
    pub has_vault: Option<bool>,
}

/// Whether the motion preference saved under `root`, against the system's,
/// reduces motion: the opening that follows an unlock then holds still.
pub(crate) fn motion_reduced(root: &CoincubeDirectory, system: Option<bool>) -> bool {
    use crate::app::settings::global::GlobalSettings;
    GlobalSettings::load_preferences(&GlobalSettings::path(root))
        .unwrap_or_default()
        .resolve(None, system)
        .reduce_motion
}

/// A ghost the recovery-phrase import is restoring: Connect's record of the
/// Cube the user picked on the floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GhostImport {
    pub uuid: String,
    pub name: String,
    pub encryption_pubkey: Option<String>,
    /// The chain Connect registered the Cube on. Test networks share a coin
    /// type, so the key alone cannot tell them apart.
    pub network: ChainId,
    /// The Connect account whose list held the ghost.
    pub account: Option<u32>,
}

/// Whether a parsed recovery phrase may take a ghost's identity: only when
/// Connect registered a seed-derived key for it and the phrase derives the
/// same one. Without a registered key nothing proves the phrase is this
/// Cube's, so the import makes a new Cube instead (`Ok(None)`).
pub(crate) fn ghost_import_identity(
    ghost: &GhostImport,
    network: ChainId,
    mnemonic: &bip39::Mnemonic,
) -> Result<Option<uuid::Uuid>, String> {
    if network != ghost.network {
        return Err(format!(
            "{} is registered on {}. Switch back to {} to restore it, or cancel.",
            catalog_display_name(&ghost.name),
            ghost.network.label(),
            ghost.network.label()
        ));
    }
    let Some(registered) = ghost.encryption_pubkey.as_deref() else {
        return Ok(None);
    };
    let signer = MasterSigner::from_mnemonic(network.bitcoin_network(), mnemonic.clone())
        .map_err(|e| format!("Failed to restore from mnemonic: {}", e))?;
    let derived = crate::services::connect::crypto::CubeEncryptionKey::derive(
        &signer,
        network.bitcoin_network(),
    )
    .public_key_hex();
    if !derived.eq_ignore_ascii_case(registered) {
        return Err(coincube_ui::localized_format!(
            "This recovery phrase doesn't belong to {}. Check the words, or create a new Cube from them.",
            "Esta frase de recuperación no pertenece a {}. Revisa las palabras o crea un Cube nuevo con ellas.",
            catalog_display_name(&ghost.name)
        ));
    }
    uuid::Uuid::parse_str(&ghost.uuid)
        .map(Some)
        .map_err(|_| "This Cube's Connect record has no usable ID.".to_string())
}

/// Which section is shown in the home's main content area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeSection {
    /// Cube list (default)
    Cubes,
    /// Installation-wide preferences, available without a Connect account.
    Settings,
    /// Connect account-level sub-page
    Connect(app::menu::ConnectSubMenu),
    /// Heir "Recover a Vault" discovery surface (COIN-377 / PR 1). Global —
    /// reachable even when the heir owns no Vault of their own.
    RecoverVault,
}

/// Context stashed for firing a remote cube update after local rename succeeds.
struct PendingRemoteRename {
    cube_id: String,
    cube_network: crate::chain::ChainId,
    new_name: String,
}

struct NativeReturn {
    chain: ChainId,
    notice: String,
}

pub struct Home {
    global_blake2b_enabled: bool,
    global_feature_request: u64,
    state: State,
    displayed_networks: Vec<ChainId>,
    /// The Cubes of every other listed network, read-only, so the Launcher
    /// can show and filter all networks at once (P-Launcher's All / Live /
    /// Test / Network filters). Display only: acting on one of these Cubes
    /// first makes its network current, and the checked `state` takes over.
    network_catalog: Vec<(ChainId, Vec<CubeSettings>)>,
    /// The networks `network_catalog` was last asked to load.
    network_catalog_requested: Vec<ChainId>,
    /// Each load claims a generation; an older read is dropped on arrival.
    network_catalog_generation: u64,
    /// Networks whose last read failed. Their previous list, if any, stays.
    network_catalog_unread: Vec<ChainId>,
    network: ChainId,
    pub datadir_path: CoincubeDirectory,
    error: Option<String>,
    /// A locked native refusal survives asynchronous launcher reconciliation
    /// until the user explicitly returns to the ordinary Cube list.
    native_return: Option<NativeReturn>,
    delete_cube_modal: Option<DeleteCubeModal>,
    create_cube_name: coincube_ui::component::form::Value<String>,
    create_cube_pin: pin_input::PinInput,
    create_cube_pin_confirm: pin_input::PinInput,
    /// Presentation only: the Create sheet's network menu is open. No seed,
    /// unlock or persistence decision depends on this flag.
    create_network_menu: bool,
    /// Keep the Create form open when its network picker triggers a datadir
    /// probe. The probe replaces `State`, including its navigation flag.
    create_network_selection_pending: bool,
    /// A stable operation identity selected before Cube creation. It is
    /// persisted with the Cube only after the normal backup gate succeeds.
    pending_bitcoin_import: Option<Box<BitcoinImport>>,
    /// Import or restore shows its Overview (P-Restore's hub) rather than a
    /// path. Presentation only: it holds no wallet material.
    restore_hub: bool,
    /// Import or restore shows its Recover with phone path.
    restore_phone: bool,
    /// Recover with phone's Cube step lists every network's Cubes, not
    /// only the current network's.
    restore_all_networks: bool,
    /// The Import or restore path to open once the Cube list is checked:
    /// the installer's restore left for it from its path list.
    pending_restore_destination: Option<crate::installer::RestoreDestination>,
    /// Opens a newly created Cube through the ordinary unlock path after the
    /// settings reload makes its row available, so the Cube's own screens
    /// (Create Vault, or an import's descriptor steps) come next.
    open_created_cube: Option<String>,
    /// A Cube whose unfinished import asked to delete it. Its delete
    /// confirmation opens once the checked list has its row.
    delete_after_check: Option<String>,
    /// A newly created Cube whose Connect registration is still in flight.
    /// It opens once the result lands: leaving Home first would drop that
    /// result, and the Cube would never be marked synced.
    open_after_registration: Option<String>,
    creating_cube: bool,
    /// UUID pre-generated on the first creation attempt and reused on retries
    /// so that each logical cube has a stable client-side identifier.
    pending_cube_id: Option<uuid::Uuid>,
    /// The ghost a recovery-phrase import was started from, if any.
    ghost_import: Option<GhostImport>,
    /// A ghost import's write in flight: the reclaimed ID and the account
    /// it was listed under.
    ghost_claim: Option<(String, Option<u32>)>,
    recovery_words: [String; 12],
    recovery_active_index: Option<usize>,
    /// The seed phrase of the Cube currently being created, held in memory
    /// only for the duration of [`State::CreationBackup`]. `Zeroizing` because
    /// this is the whole wallet; cleared on every exit from the step —
    /// completion, bypass, abandonment and failure alike.
    creation_backup_words: Option<zeroize::Zeroizing<Vec<String>>>,
    /// The Cube list parked while [`State::CreationBackup`] owns the screen,
    /// so cancelling puts the user back where they were. Empty at every other
    /// time. Not secret — these entries are already in `settings.json`.
    creation_backup_cubes: Vec<CubeSettings>,
    developer_mode: bool,
    settings_error: Option<String>,
    /// Connect account tier — controls how many Cubes can be created per network.
    account_tier: AccountTier,
    /// Account-level Connect panel (login, plan, security, etc.)
    pub connect_account: ConnectAccountPanel,
    /// Reject older Account-row lookups after a newer selection.
    recovery_open_request: u64,
    /// Heir "Recover a Vault" discovery surface state (COIN-377 / PR 1).
    pub recover_vault: RecoverVaultPanel,
    /// Whether the Connect sidebar section is expanded
    pub connect_expanded: bool,
    /// Which section is currently displayed in the main content area
    pub active_section: HomeSection,
    /// Current theme mode (dark/light) — used for theme-aware rendering
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    /// App preferences is open over this Launcher; its top-bar entry shows
    /// the current state (`.tbtn.on`). Set by the GUI that owns the sheet.
    pub(crate) app_preferences_open: bool,
    /// The effective reduce-motion preference: the Connect light holds steady.
    pub(crate) reduce_motion: bool,
    /// The pulsing Connect light's dim phase.
    pub(crate) connect_light_dim: bool,
    /// Last native accessibility observation used by Follow system motion.
    system_reduces_motion: Option<bool>,
    /// A Liquid wallet exists on this device (the sunset gate's local half),
    /// probed when the Cube list loads so the Launcher can list Liquid
    /// without touching the disk on every frame.
    liquid_on_disk: bool,
    /// Whether the user has chosen to create a passkey-derived Cube (no PIN).
    passkey_mode: bool,
    /// A registered passkey credential awaiting the creation-backup step. See
    /// [`PendingPasskeyCube`]. `Some` only while `State::CreationBackup` is on
    /// screen for a passkey Cube, and it is what tells that step — and the
    /// finalizer — which of the two creation paths it is resolving.
    pending_passkey_cube: Option<PendingPasskeyCube>,
    /// Recovery password for a Kit being created during Cube creation, and its
    /// confirmation. Held here rather than in [`CreationBackupStep`] so the
    /// step stays `Clone + Eq`, and in `Zeroizing` so the password is wiped
    /// when the step ends. Cleared by [`Home::scrub_creation_kit`].
    creation_kit_password: zeroize::Zeroizing<String>,
    creation_kit_confirm: zeroize::Zeroizing<String>,
    /// Active acceptance of "I've written this password down" — the same gate
    /// the Settings Kit flow applies.
    creation_kit_acknowledged: bool,
    /// Failure from the Kit upload, shown on the password screen.
    creation_kit_error: Option<String>,
    /// The backup hub's progress: the seed phrase was written down and its
    /// challenge words typed back. Either this or a Kit lets Continue finish
    /// creation; both are recorded on the Cube when both are done.
    creation_seed_confirmed: bool,
    /// The Kit made during this creation, held until Continue writes the Cube
    /// with it. Paired with the account that registered it, captured before
    /// upload, so a later account switch cannot misattribute the registration.
    /// `Some` only after Connect confirmed it is holding the seed.
    creation_kit_evidence: Option<(u32, creation_gate::CreationRecoveryKit)>,
    /// Active passkey ceremony (webview open, awaiting IPC result).
    passkey_ceremony: Option<PasskeyCeremony>,
    /// Active native macOS passkey ceremony (uses AuthenticationServices).
    #[cfg(target_os = "macos")]
    native_passkey_ceremony: Option<crate::services::passkey::macos::NativePasskeyCeremony>,
    /// Whether a Connect session exists in the OS keyring (cached to avoid
    /// synchronous keyring I/O on every render).
    has_stored_session: bool,
    /// Server-authoritative cube limit per network, if fetched from the API.
    /// Takes precedence over `account_tier.cube_limit()` when set.
    server_cube_limit: Option<usize>,
    /// Why a local Cube is still unsynced, keyed by cube id — the server's own
    /// reason for refusing its registration.
    ///
    /// The client-side limit check in `CreateCube` is advisory, not
    /// authoritative: it counts local Cubes plus the `remote_cubes` this device
    /// has fetched, which undercounts whenever that fetch is stale, partial, or
    /// (running against a fresh datadir) has barely started. The server counts
    /// every Cube on the account for that network and is the one that decides.
    /// So a Cube can be created locally and then refused registration, and
    /// before this map the only trace was a log line: the Cubes list went on
    /// promising "it will sync automatically in a moment" forever.
    ///
    /// Written by every path that registers a Cube (creation, catch-up sync) on
    /// both outcomes, so an entry means "the last attempt failed, for this
    /// reason" and its absence means "no attempt has failed".
    cube_sync_errors: std::collections::HashMap<String, String>,
    /// Rename cube modal: stable chain and Cube ID, plus the name input.
    /// A background catalog refresh may reorder the list while it is open.
    rename_cube_modal: Option<(CubeCatalogKey, String)>,
    /// Transient List selection and name search. Presentation only: never
    /// persisted, never an action target on its own.
    catalog: Box<launcher_catalog::CatalogState>,
    /// Pending remote rename: stashed after local rename succeeds so the
    /// `CubeRenamed` handler can fire the API update.
    pending_remote_rename: Option<PendingRemoteRename>,
    /// Cubes that exist on the Connect server but not locally on this machine.
    remote_cubes: Vec<RemoteCube>,
    /// Generation counter identifying the newest *valid* remote-cube fetch.
    ///
    /// `fetch_remote_cubes` has three independent triggers (sign-in, local
    /// delete, remote-delete settled) with nothing serialising them, and each
    /// call is a `list_cubes()` plus one status probe per remote cube — so two
    /// fetches can be in flight and finish in either order. Without a
    /// generation, `RemoteCubesLoaded` is last-writer-wins by *arrival*, and a
    /// slow older fetch overwrites a newer one with a stale list.
    ///
    /// Logout bumps this too (see `invalidate_remote_cubes`), so a fetch issued
    /// for the previous account cannot repopulate the list after it is cleared.
    remote_cubes_request: u64,
    /// Modal for deleting a remote-only cube from the Connect server.
    delete_remote_cube_modal: Option<DeleteRemoteCubeModal>,
    /// Recovery-method picker, shown when a remote cube can be recovered by
    /// *both* a password Recovery Kit and a phone envelope.
    recovery_method_modal: Option<RecoveryMethodModal>,
    /// One-time, app-wide incident notice for a firmware advisory, shown on
    /// the first launch after the update that introduced it and dismissed for
    /// good once acknowledged. `None` once seen — or if there is no advisory
    /// to announce.
    ///
    /// It is shown to *everyone*, not only to people holding the affected
    /// device: a descriptor key records no source device, so there is nothing
    /// to target on. The copy self-selects instead.
    advisory_notice: Option<&'static crate::hw_advisory::Advisory>,
    #[allow(dead_code)]
    welcome_quote: coincube_ui::component::quote_display::Quote,
    #[allow(dead_code)]
    welcome_image_handle: iced::widget::image::Handle,
}

impl Home {
    pub fn new(datadir_path: CoincubeDirectory, network: Option<Network>) -> (Self, Task<Message>) {
        Self::new_for_chain(datadir_path, network.map(ChainId::from))
    }

    pub fn new_for_chain(
        datadir_path: CoincubeDirectory,
        network: Option<ChainId>,
    ) -> (Self, Task<Message>) {
        Self::new_for_chain_inner(datadir_path, network, None)
    }

    /// Show a native refusal on its trusted chain without carrying an unlock
    /// or granting access to a chain that is currently unavailable.
    #[cfg(feature = "mcp-proposals")]
    pub(crate) fn after_native_refusal(
        datadir_path: CoincubeDirectory,
        chain: ChainId,
        notice: impl Into<String>,
    ) -> (Self, Task<Message>) {
        Self::new_for_chain_inner(
            datadir_path,
            Some(chain),
            Some(NativeReturn {
                chain,
                notice: notice.into(),
            }),
        )
    }

    fn new_for_chain_inner(
        datadir_path: CoincubeDirectory,
        network: Option<ChainId>,
        native_return: Option<NativeReturn>,
    ) -> (Self, Task<Message>) {
        let system_reduces_motion = crate::app::settings::system_accessibility::reduces_motion();
        let developer_mode =
            GlobalSettings::load_developer_mode(&GlobalSettings::path(&datadir_path));
        let mut connect_account = ConnectAccountPanel::new();
        connect_account.bitcoin_blake2b_opt_in =
            GlobalSettings::load_bitcoin_blake2b_beta(&GlobalSettings::path(&datadir_path));
        let selected_network = network.unwrap_or(
            NETWORKS
                .iter()
                .find(|net| has_existing_wallet(&datadir_path, **net))
                .cloned()
                .unwrap_or(ChainId::Bitcoin),
        );
        let network = if let Some(context) = &native_return {
            context.chain
        } else if developer_mode || selected_network.is_blake2b() {
            selected_network
        } else {
            ChainId::Bitcoin
        };
        let network_dir = datadir_path.network_directory(network);
        (
            Self {
                global_blake2b_enabled: false,
                global_feature_request: 0,
                state: State::Unchecked,
                network_catalog: Vec::new(),
                network_catalog_requested: Vec::new(),
                network_catalog_generation: 0,
                network_catalog_unread: Vec::new(),
                displayed_networks: if developer_mode {
                    NETWORKS.to_vec()
                } else {
                    vec![ChainId::Bitcoin]
                },
                network,
                datadir_path: datadir_path.clone(),
                error: None,
                native_return,
                delete_cube_modal: None,
                create_cube_name: coincube_ui::component::form::Value::default(),
                create_cube_pin: pin_input::PinInput::new(),
                create_cube_pin_confirm: pin_input::PinInput::new(),
                create_network_menu: false,
                create_network_selection_pending: false,
                pending_bitcoin_import: None,
                restore_hub: false,
                restore_phone: false,
                restore_all_networks: false,
                pending_restore_destination: None,
                open_created_cube: None,
                delete_after_check: None,
                open_after_registration: None,
                creating_cube: false,
                pending_cube_id: None,
                ghost_import: None,
                ghost_claim: None,
                recovery_words: Default::default(),
                recovery_active_index: None,
                creation_backup_words: None,
                creation_backup_cubes: Vec::new(),
                developer_mode,
                account_tier: GlobalSettings::load_account_tier(&GlobalSettings::path(
                    &datadir_path,
                )),
                connect_account,
                recovery_open_request: 0,
                settings_error: None,
                recover_vault: RecoverVaultPanel::new(),
                connect_expanded: false,
                active_section: HomeSection::Cubes,
                theme_mode: GlobalSettings::load_theme_mode(&GlobalSettings::path(&datadir_path)),
                app_preferences_open: false,
                reduce_motion: false,
                connect_light_dim: false,
                system_reduces_motion,
                liquid_on_disk: false,
                // Recomputed whenever the Create Cube form opens — see
                // `Home::default_passkey_mode`. A freshly constructed `Home`
                // has no Connect session, so this is PIN.
                passkey_mode: false,
                pending_passkey_cube: None,
                creation_kit_password: zeroize::Zeroizing::new(String::new()),
                creation_kit_confirm: zeroize::Zeroizing::new(String::new()),
                creation_kit_acknowledged: false,
                creation_kit_error: None,
                creation_seed_confirmed: false,
                creation_kit_evidence: None,
                passkey_ceremony: None,
                #[cfg(target_os = "macos")]
                native_passkey_ceremony: None,
                has_stored_session: false, // Will be checked when cube UUID is set
                server_cube_limit: None,
                cube_sync_errors: std::collections::HashMap::new(),
                rename_cube_modal: None,
                catalog: Box::new(launcher_catalog::CatalogState {
                    mode: Self::load_launcher_mode(&datadir_path, system_reduces_motion),
                    activity: launcher_activity::ActivityState::load(&datadir_path),
                    ..Default::default()
                }),
                pending_remote_rename: None,
                remote_cubes: Vec::new(),
                remote_cubes_request: 0,
                delete_remote_cube_modal: None,
                recovery_method_modal: None,
                advisory_notice: pending_advisory_notice(&datadir_path),
                welcome_quote: coincube_ui::component::quote_display::random_quote("first-launch"),
                welcome_image_handle:
                    coincube_ui::component::quote_display::image_handle_for_context("first-launch"),
            },
            // Kick a Connect session check alongside the datadir probe
            // so the sidebar reflects authenticated state immediately
            // when a session is already in the keyring — without
            // waiting for the user to navigate to the Connect section.
            Task::batch([
                Task::perform(
                    crate::chain::global_blake2b_enabled(CoincubeClient::new()),
                    |enabled| Message::GlobalFeatureLoaded(0, enabled),
                ),
                probe_network_datadir(network, network_dir),
                Task::done(Message::View(ViewMessage::ConnectAccount(
                    ConnectAccountMessage::Init,
                ))),
            ]),
        )
    }

    #[cfg(test)]
    pub(crate) fn is_checked_for_test(&self) -> bool {
        !matches!(self.state, State::Unchecked)
    }

    #[cfg(all(test, feature = "mcp-proposals"))]
    pub(crate) fn chain_for_test(&self) -> ChainId {
        self.network
    }

    /// A sibling tab has just accepted a Connect login or token refresh.
    /// Reuse that accepted bearer instead of starting another keyring refresh:
    /// concurrent refreshes can rotate the token while a Cube is opening.
    pub(crate) fn adopt_connect_session(&mut self, token: &str) -> Task<Message> {
        let mut client = self.connect_account.client.clone();
        client.set_token(token);
        self.adopt_connect_client(client)
    }

    pub(crate) fn adopt_connect_client(&mut self, client: CoincubeClient) -> Task<Message> {
        self.connect_account.invalidate_auth();
        self.connect_account.install_shared_client(client);
        map_connect_task(
            self.connect_account
                .update_message(ConnectAccountMessage::Init),
        )
    }

    pub(crate) fn on_global_connect_logout(&mut self) -> Task<Message> {
        self.connect_account.invalidate_auth();
        map_connect_task(
            self.connect_account
                .update_message(ConnectAccountMessage::LogOut),
        )
    }

    /// Refresh persisted completion/reorg changes when returning to the list,
    /// without replacing an in-progress creation, recovery, or account screen.
    pub fn on_focus(&self) -> Task<Message> {
        if matches!(self.active_section, HomeSection::Cubes)
            && matches!(
                self.state,
                State::Cubes {
                    create_cube: false,
                    ..
                }
            )
        {
            self.reload().map(|message| match message {
                Message::Checked { for_chain, res } => Message::FocusedCubes { for_chain, res },
                other => other,
            })
        } else {
            Task::none()
        }
    }

    pub fn reload(&self) -> Task<Message> {
        probe_network_datadir(
            self.network,
            self.datadir_path.network_directory(self.network),
        )
    }

    fn catalog_index(&self, key: &CubeCatalogKey) -> Option<usize> {
        let State::Cubes { cubes, source, .. } = &self.state else {
            return None;
        };
        if *source != key.chain || self.network != key.chain {
            return None;
        }
        let mut matches = cubes
            .iter()
            .enumerate()
            .filter(|(_, cube)| cube.id == key.cube_id && cube.network == key.chain);
        let (index, _) = matches.next()?;
        matches.next().is_none().then_some(index)
    }

    /// Returns the effective per-network cube limit, preferring the
    /// server-authoritative value when available.
    fn cube_limit(&self) -> usize {
        self.server_cube_limit
            .unwrap_or_else(|| self.account_tier.cube_limit())
    }

    /// Cubes on `chain` that count toward its limit: (on this device,
    /// registered in Connect but not on this device).
    ///
    /// The local count is `None` when this device's list for `chain` cannot be
    /// counted right now: the checked list is still another network's (a
    /// network switch whose read is pending or failed), or `chain` is not
    /// listed here (Developer mode off) or its catalog read failed. Callers
    /// show that as unknown rather than as 0 — a 0 would misstate a network
    /// that may hold Cubes on disk.
    pub(super) fn cube_usage(&self, chain: ChainId) -> (Option<usize>, usize) {
        let local = match &self.state {
            State::Cubes { cubes, source, .. } if *source == chain => Some(cubes.len()),
            State::NoCube { .. } if chain == self.network => Some(0),
            _ if self.network_catalog_unread.contains(&chain) => None,
            _ => self
                .network_catalog
                .iter()
                .find(|(c, _)| *c == chain)
                .map(|(_, cubes)| cubes.len()),
        };
        let network_str = settings::network_to_api_str(chain);
        let remote = self
            .remote_cubes
            .iter()
            .filter(|rc| rc.network == network_str)
            .count();
        (local, remote)
    }

    /// Total cube count (local + remote) for the current network.
    /// The server limit applies across all devices, so remote-only cubes
    /// must be included when checking the limit. While this device's count
    /// is unknown (see [`Self::cube_usage`]) only the Connect-only Cubes are
    /// counted: the client check is advisory, and Connect refuses a
    /// registration over the limit, leaving that Cube on this device only.
    fn total_cube_count(&self) -> usize {
        let (local, remote) = self.cube_usage(self.network);
        local.unwrap_or(0) + remote
    }

    /// "Bitcoin mainnet: 7/7 Cubes on the Estate plan, 2 of them not on this
    /// device." The limit is per network and counts Cubes registered from
    /// other devices, so the copy names both rather than a bare ratio that
    /// disagrees with the Cubes the user can see here. `chain` is the network
    /// the copy is about: the current one for Create, the refused Cube's own
    /// for a sync refusal, which may be listed from another network.
    fn cube_limit_summary(&self, chain: ChainId) -> String {
        let (local, remote) = self.cube_usage(chain);
        let name = launcher_chrome::network_name(chain);
        let limit = self.cube_limit();
        let tier = self.account_tier.display_name();
        let Some(local) = local else {
            return localized_format!(
                "{name}: {remote} Cubes registered in Connect, of {limit} on the \
                 {tier} plan; this device's Cubes there could not be counted.",
                "{name}: {remote} Cubes registrados en Connect, de {limit} en el plan \
                 {tier}; no se pudieron contar los Cubes de este dispositivo en esa red."
            );
        };
        let total = local + remote;
        let mut summary = if total > limit {
            // Already over (the limit dropped, or the plan changed): say how
            // many the plan includes rather than an impossible ratio.
            localized_format!(
                "{name}: you have {total} Cubes; the {tier} plan includes {limit}",
                "{name}: tienes {total} Cubes; el plan {tier} incluye {limit}"
            )
        } else {
            localized_format!(
                "{name}: {}/{limit} Cubes on the {tier} plan",
                "{name}: {}/{limit} Cubes en el plan {tier}",
                total
            )
        };
        if remote > 0 {
            summary.push_str(&localized_format!(
                ", {} of them registered in Connect but not on this device",
                ", {} de ellos registrados en Connect pero no en este dispositivo",
                remote
            ));
        }
        summary.push('.');
        summary
    }

    /// The Create Cube refusal at the limit, stored from `update()` in the
    /// window's language.
    fn cube_limit_error(&self) -> String {
        // Over the limit after a plan change or the 1/3/10 limits: every
        // existing Cube keeps working (billing v2 decision 8, I8); only new
        // Cubes are blocked.
        localized_format!(
            "Cube limit reached. {} Existing Cubes keep working. Upgrade your \
             Connect account or delete a Cube on this network to create more.",
            "Alcanzaste el límite de Cubes. {} Los Cubes existentes siguen \
             funcionando. Mejora tu plan de Connect o elimina un Cube de esta \
             red para crear más.",
            self.cube_limit_summary(self.network),
        )
    }

    /// The server's reason for refusing this Cube's registration, as user copy —
    /// `None` when no attempt has failed, which is the "still mid-sync" case.
    ///
    /// The limit refusal gets expanded into the same wording the Create Cube
    /// form uses, because "Cube limit reached for this network" alone doesn't
    /// tell the user how many they have, on what plan, or what to do about it.
    /// Anything else is shown verbatim: an unrecognised reason is still far more
    /// use than a promise to sync that we know will not be kept.
    fn cube_sync_hint(&self, cube: &CubeSettings) -> Option<String> {
        let reason = self.cube_sync_errors.get(&cube.id)?;
        // Matched on the message rather than the error code because the client
        // keeps only `error.message` from the API envelope (see
        // `NotSuccessResponseInfo::message`). Paired with
        // `responses.ErrCubeLimitReached` / "Cube limit reached for this
        // network" in the API's cube handler — change both together.
        if reason.contains("Cube limit reached") {
            return Some(localized_format!(
                "Out of Cube slots — this Cube is saved on this device only.\n\n\
                 {} Upgrade your Connect account, or delete a Cube on this \
                 network you no longer need, and this one will sync \
                 automatically.", "Alcanzaste el límite de Cubes. Este Cube se guardó solo en este dispositivo.\n\n{} Mejora tu plan de Connect o elimina un Cube de esta red que ya no necesites. Este Cube se sincronizará automáticamente.",
                self.cube_limit_summary(cube.network),
            ));
        }
        Some(localized_format!(
            "Connect refused to register this Cube, so it is saved on this \
             device only.\n\n{reason}", "Connect rechazó el registro de este Cube. Solo se guardó en este dispositivo.\n\n{reason}"
        ))
    }

    /// Whether this screen has any Cubes to list under the Launcher's network
    /// filter — local ones, or remote ones registered in Connect for the
    /// selected network. False on a genuinely fresh device, and when the
    /// filter shows nothing. Empty catalogs remain on the Launcher.
    fn has_cube_list(&self) -> bool {
        self.has_cube_list_under(self.effective_filter())
    }

    /// [`Self::has_cube_list`] for a given filter. The Launcher lists every
    /// listed network's Cubes, so another network's count too, unless the
    /// filter hides it. The selected network's own list always counts.
    fn has_cube_list_under(&self, filter: launcher_catalog::NetworkFilter) -> bool {
        if let State::Cubes { cubes, .. } = &self.state {
            if !cubes.is_empty() {
                return true;
            }
        }
        if self
            .displayed_networks
            .iter()
            .any(|chain| filter.matches(*chain) && !self.network_catalog(*chain).is_empty())
        {
            return true;
        }
        let network_str = settings::network_to_api_str(self.network);
        self.remote_cubes.iter().any(|rc| rc.network == network_str)
    }

    /// Creation is an explicit Launcher action, including on a fresh device.
    /// Neither an empty catalog nor Connect authentication opens the form.
    fn showing_create_form(&self) -> bool {
        match &self.state {
            State::Cubes { create_cube, .. } | State::NoCube { create_cube } => *create_cube,
            _ => false,
        }
    }

    fn showing_create_journey(&self) -> bool {
        !matches!(self.state, State::Unchecked)
            && matches!(self.active_section, HomeSection::Cubes)
            && (matches!(self.state, State::CreationBackup(_)) || self.showing_create_form())
    }

    /// The Create journey shows Import or restore (P-Restore): its Overview,
    /// or the Bitcoin import path through Cube creation and its backup.
    fn showing_import_or_restore(&self) -> bool {
        self.restore_hub || self.pending_bitcoin_import.is_some()
    }

    /// The Launcher remains a destination even when its catalog is empty.
    /// Backup and in-flight creation still retain their existing exit gates.
    fn can_leave_create_journey(&self) -> bool {
        !matches!(self.state, State::CreationBackup(_)) && !self.creating_cube
    }

    pub fn stop(&mut self) {}

    /// Opens Import or restore on `destination` once the Cube list is
    /// checked: the installer's restore left for it from its path list.
    pub fn open_restore_destination(&mut self, destination: crate::installer::RestoreDestination) {
        self.pending_restore_destination = Some(destination);
    }

    /// Set a top-level error message shown on the home screen.
    /// Used by outer state machines (e.g. `gui::tab`) to surface issues
    /// they detect while handling home-originated messages.
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error = Some(msg.into());
    }

    /// A Recovery Services selection belongs to the Overview visit that
    /// started it. Leaving that route invalidates even a result delivered
    /// after the user returns to Overview.
    fn leave_recovery_services(&mut self) {
        if matches!(
            self.active_section,
            HomeSection::Connect(app::menu::ConnectSubMenu::Overview)
        ) {
            self.recovery_open_request = self.recovery_open_request.wrapping_add(1);
        }
    }

    /// The top-level error currently shown on the home screen, if any.
    pub fn error(&self) -> Option<&str> {
        self.native_return
            .as_ref()
            .map(|context| context.notice.as_str())
            .or(self.error.as_deref())
    }

    pub fn subscription(&self, foreground: bool) -> Subscription<Message> {
        // The Connect light pulses while the session is checked or Connect
        // can't be reached, unless motion is reduced.
        let pulse = if foreground
            && !self.reduce_motion
            && matches!(
                launcher_chrome::launcher_connect_state(self).1,
                Some(crate::app::ConnectLight::Connecting | crate::app::ConnectLight::Reconnecting)
            ) {
            iced::time::every(std::time::Duration::from_millis(800))
                .map(|_| Message::ConnectLightTick)
        } else {
            Subscription::none()
        };
        // A stored session that couldn't be checked is checked again (L1).
        let recheck = match self.connect_account.unverified.as_ref() {
            Some(unverified) if foreground => unverified.recheck().map(|_| {
                Message::View(ViewMessage::ConnectAccount(
                    app::view::ConnectAccountMessage::RecheckSession,
                ))
            }),
            _ => Subscription::none(),
        };
        let base = Subscription::batch([
            self.launcher_subscription(foreground),
            self.activity_subscription(foreground),
            pulse,
            recheck,
        ]);
        // Escape closes the network menu first, then the activity source
        // menu, then the plan menu, then shuts the Key card.
        let menu = self.catalog.network_menu;
        let activity = self.catalog.activity.menu;
        let plan = self.catalog.plan_menu;
        if foreground
            && (menu || activity || plan || self.catalog.key_open())
            && launcher_catalog::catalog_visible(self)
        {
            let escape = iced::keyboard::listen()
                .with((menu, activity, plan))
                .filter_map(|((menu, activity, plan), event)| match event {
                    iced::keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                        ..
                    } => Some(Message::View(ViewMessage::Catalog(if menu {
                        launcher_catalog::CatalogMessage::NetworkMenu(false)
                    } else if activity {
                        launcher_catalog::CatalogMessage::Activity(
                            launcher_activity::ActivityMessage::Menu(false),
                        )
                    } else if plan {
                        launcher_catalog::CatalogMessage::PlanMenu(false)
                    } else {
                        launcher_catalog::CatalogMessage::KeyClose
                    }))),
                    _ => None,
                });
            return Subscription::batch([base, escape]);
        }
        base
    }

    fn launcher_subscription(&self, foreground: bool) -> Subscription<Message> {
        if let Some(ceremony) = &self.passkey_ceremony {
            if ceremony.active_webview.is_some() {
                return ceremony
                    .webview_manager
                    .subscription(std::time::Duration::from_millis(25))
                    .map(Message::PasskeyWebviewUpdate);
            }
        }

        // Native macOS passkey ceremony — poll the channel periodically.
        #[cfg(target_os = "macos")]
        {
            if self.native_passkey_ceremony.is_some() {
                return iced::time::every(std::time::Duration::from_millis(50))
                    .map(|_| Message::NativePasskeyTick);
            }
        }

        // The floor's ordinary controls retain focus/activation. Listen only
        // while there is a second page. Focused widgets keep captured keys;
        // sideways wheel deltas also work through the vertical scrollable.
        if foreground
            && self.delete_cube_modal.is_none()
            && self.delete_remote_cube_modal.is_none()
            && self.recovery_method_modal.is_none()
            && self.rename_cube_modal.is_none()
            && self.advisory_notice.is_none()
            && launcher_catalog::catalog_visible(self)
        {
            let projection = launcher_catalog::project(self);
            if projection.floor_eligible(self.catalog.mode)
                && projection.floor_len() > self.catalog.page_size()
            {
                let paging = iced::event::listen_with(launcher_catalog::floor_event);
                if self.catalog.animating() {
                    return Subscription::batch([
                        paging,
                        iced::window::frames().map(|now| {
                            Message::View(ViewMessage::Catalog(
                                launcher_catalog::CatalogMessage::MotionTick(now),
                            ))
                        }),
                    ]);
                }
                return paging;
            }
        }
        if foreground && self.catalog.animating() && launcher_catalog::catalog_visible(self) {
            return iced::window::frames().map(|now| {
                Message::View(ViewMessage::Catalog(
                    launcher_catalog::CatalogMessage::MotionTick(now),
                ))
            });
        }
        Subscription::none()
    }

    /// Local-node visibility uses the anonymous flag; Connect retains its own grant.
    pub(crate) fn chain_availability(&self, chain: ChainId) -> app::features::Availability {
        if !chain.is_blake2b() {
            return app::features::Availability::Available;
        }
        if self.connect_account.bitcoin_blake2b_opt_in
            && (self.global_blake2b_enabled
                || self.connect_chain_availability(chain).is_available())
        {
            app::features::Availability::Available
        } else {
            app::features::Availability::Unavailable { reason: "Bitcoin Blake2b is unavailable. Enable its beta preference in Global Settings and verify feature availability.".into() }
        }
    }

    /// Account-scoped admission for the explicit Connect-only fork route.
    pub(crate) fn connect_chain_availability(&self, chain: ChainId) -> app::features::Availability {
        if !chain.is_blake2b() {
            return app::features::Availability::Available;
        }
        app::features::bitcoin_blake2b(app::features::BitcoinBlake2bServerFlag {
            server_enabled: self.connect_account.bitcoin_blake2b_opt_in
                && self
                    .connect_account
                    .authenticated_client()
                    .is_some_and(|c| c.token().is_some())
                && self
                    .connect_account
                    .features
                    .as_ref()
                    .and_then(|f| f.bitcoin_blake2b_enabled)
                    == Some(true),
        })
    }

    fn recheck_global_feature(&mut self) -> Task<Message> {
        self.global_feature_request = self.global_feature_request.wrapping_add(1);
        let request = self.global_feature_request;
        let client = self.connect_account.client.clone();
        Task::perform(
            crate::chain::global_blake2b_enabled(client),
            move |enabled| Message::GlobalFeatureLoaded(request, enabled),
        )
    }

    fn load_launcher_mode(
        root: &CoincubeDirectory,
        system_reduces_motion: Option<bool>,
    ) -> launcher_catalog::LauncherMode {
        launcher_catalog::load_launcher_mode(root, system_reduces_motion)
    }

    fn apply_launcher_mode(&mut self) {
        self.catalog.activity.apply_preferences(&self.datadir_path);
        let launcher_mode =
            Self::load_launcher_mode(&self.datadir_path, self.system_reduces_motion);
        if self.catalog.mode != launcher_mode {
            self.catalog.mode = launcher_mode;
            self.catalog.query.clear();
            self.catalog.page = 0;
            self.catalog.selected = None;
            self.catalog.motion = None;
            self.catalog.open_from_floor = false;
        }
    }

    /// Refresh only the Launcher presentation after a device style or motion
    /// edit. Feature reconciliation can cancel an in-progress Split flow and
    /// belongs to actual developer/network preference changes.
    pub(crate) fn refresh_launcher_mode(&mut self) {
        self.apply_launcher_mode();
    }

    /// Update the purely visual Launcher mode when macOS Accessibility changes.
    pub(crate) fn set_system_reduces_motion(&mut self, observed: Option<bool>) {
        if self.system_reduces_motion != observed {
            self.system_reduces_motion = observed;
            self.apply_launcher_mode();
        }
    }

    /// Refresh saved preferences after a successful edit in any Home tab.
    pub(crate) fn reload_global_settings(&mut self) -> Task<Message> {
        let task = self.reload_global_settings_inner();
        // Developer mode and the Blake2b beta change which networks show.
        match self.refresh_network_catalog(false) {
            Some(load) => Task::batch([task, load]),
            None => task,
        }
    }

    fn reload_global_settings_inner(&mut self) -> Task<Message> {
        let path = GlobalSettings::path(&self.datadir_path);
        self.apply_launcher_mode();
        self.developer_mode = GlobalSettings::load_developer_mode(&path);
        self.connect_account.bitcoin_blake2b_opt_in =
            GlobalSettings::load_bitcoin_blake2b_beta(&path);
        self.refresh_displayed_networks();
        if self.native_return.is_none() && !self.displayed_networks.contains(&self.network) {
            self.abandon_creation_backup();
            self.create_cube_pin.clear();
            self.create_cube_pin_confirm.clear();
            for word in &mut self.recovery_words {
                word.clear();
            }
            self.pending_cube_id = None;
            self.ghost_import = None;
            self.network = ChainId::Bitcoin;
            self.error = None;
            self.server_cube_limit = None;
            // The selected chain has changed under an in-progress Create
            // journey. Its pending row selection belongs to the old chain.
            self.create_network_menu = false;
            self.create_network_selection_pending = false;
            self.state = State::Unchecked;
            return probe_network_datadir(
                self.network,
                self.datadir_path.network_directory(self.network),
            );
        }
        Task::none()
    }

    fn refresh_displayed_networks(&mut self) {
        self.displayed_networks = if self.developer_mode {
            NETWORKS.to_vec()
        } else {
            vec![ChainId::Bitcoin]
        };
        if self
            .chain_availability(ChainId::BitcoinBlake2b)
            .is_available()
        {
            self.displayed_networks.push(ChainId::BitcoinBlake2b);
            if self.developer_mode {
                self.displayed_networks
                    .push(ChainId::BitcoinBlake2bTestnet4);
            }
        }
    }

    /// A refusal admits only its dismissal and background launcher results.
    /// Tab uses the same boundary before dispatching queued lifecycle outputs.
    pub(crate) fn accepts_launcher_message(&self, message: &Message) -> bool {
        self.native_return.is_none()
            || matches!(
                message,
                Message::GlobalSettingsChanged
                    | Message::ConnectLightTick
                    | Message::ConnectSignedOutBy(_)
                    | Message::GlobalFeatureLoaded(..)
                    | Message::FocusedCubes { .. }
                    | Message::Checked { .. }
                    | Message::NetworkCatalogLoaded { .. }
                    | Message::RemoteCubesLoaded { .. }
                    | Message::CubeLimitsLoaded(_)
                    | Message::CatchUpSyncFinished(_)
                    | Message::ConnectSignedInBubble
                    | Message::View(
                        ViewMessage::DismissNativeReturn
                            | ViewMessage::Check
                            | ViewMessage::ConnectAccount(_)
                            | ViewMessage::ConnectSheetAccount(_)
                    )
            )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        if !self.accepts_launcher_message(&message) {
            return Task::none();
        }
        if matches!(&message, Message::View(ViewMessage::DismissNativeReturn)) {
            if self.native_return.take().is_some() {
                self.error = None;
                return self.reload_global_settings();
            }
            return Task::none();
        }
        // List presentation messages never enter the lifecycle dispatch. The
        // transient selection is reconciled with whatever screen and list
        // each message leaves, including after an early return.
        // Connect results can grant or withdraw Bitcoin Blake2b, which changes
        // which networks are listed (the GUI's preference reload covers the
        // other causes).
        let connect = matches!(
            &message,
            Message::View(ViewMessage::ConnectAccount(_) | ViewMessage::ConnectSheetAccount(_))
        );
        let checked = matches!(&message, Message::Checked { .. });
        let task = match message {
            Message::View(ViewMessage::Catalog(message)) => self.update_catalog(message),
            Message::ConnectLightTick => {
                self.connect_light_dim = !self.connect_light_dim;
                Task::none()
            }
            // Connect, not the user, ended the session; the GUI mirrors it to
            // every tab (L3).
            Message::ConnectSignedOutBy(reason) => {
                self.connect_account.signed_out_by = Some(reason);
                Task::none()
            }
            Message::View(ViewMessage::DismissConnectSignOut) => {
                self.connect_account.signed_out_by = None;
                Task::none()
            }
            message => self.update_lifecycle(message),
        };
        self.reconcile_catalog();
        // The path a restore left for opens through its own message, with
        // that path's guards, once the list it opens over is checked.
        if checked && matches!(self.state, State::Cubes { .. } | State::NoCube { .. }) {
            if let Some(destination) = self.pending_restore_destination.take() {
                use crate::installer::RestoreDestination;
                let open = match destination {
                    RestoreDestination::Overview => ViewMessage::ShowRestoreHub,
                    RestoreDestination::ImportBitcoin => ViewMessage::ShowImportBitcoin,
                    RestoreDestination::Kit => ViewMessage::RestoreFromAnyRecoveryKit,
                    RestoreDestination::Phone => ViewMessage::ShowRestorePhone,
                };
                return Task::batch([task, Task::done(Message::View(open))]);
            }
        }
        // Once the first probe has finished, read the networks such a change
        // lists, whatever screen Home is on: a creation backup or recovery
        // returns to the list without another check. A Bitcoin-only start
        // never claims a generation, so the generation cannot stand for
        // "in use".
        if connect && !matches!(self.state, State::Unchecked) {
            if let Some(load) = self.refresh_network_catalog(false) {
                return Task::batch([task, load]);
            }
        }
        task
    }

    /// Read the other listed networks' Cubes whenever the set changes: the
    /// current network's list was checked (it may have moved), or settings
    /// changed which networks are listed.
    /// `force` rereads even an unchanged set, after the current list was
    /// checked, so another tab's create or delete shows here too.
    fn refresh_network_catalog(&mut self, force: bool) -> Option<Task<Message>> {
        let wanted: Vec<ChainId> = self
            .displayed_networks
            .iter()
            .copied()
            .filter(|chain| *chain != self.network)
            .collect();
        if wanted == self.network_catalog_requested && (!force || wanted.is_empty()) {
            return None;
        }
        self.network_catalog_generation += 1;
        let generation = self.network_catalog_generation;
        self.network_catalog
            .retain(|(chain, _)| wanted.contains(chain));
        self.network_catalog_unread
            .retain(|chain| wanted.contains(chain));
        self.network_catalog_requested = wanted.clone();
        Some(Task::batch(wanted.into_iter().map(|chain| {
            let dir = self.datadir_path.network_directory(chain);
            Task::perform(read_network_catalog(chain, dir), move |cubes| {
                Message::NetworkCatalogLoaded {
                    generation,
                    chain,
                    cubes,
                }
            })
        })))
    }

    /// Another network's Cubes, as last read; empty until loaded.
    pub(crate) fn network_catalog(&self, chain: ChainId) -> &[CubeSettings] {
        self.network_catalog
            .iter()
            .find(|(c, _)| *c == chain)
            .map(|(_, cubes)| cubes.as_slice())
            .unwrap_or(&[])
    }

    /// The Launcher's message handling, unchanged by the List body.
    fn update_lifecycle(&mut self, message: Message) -> Task<Message> {
        let message = match message {
            Message::View(ViewMessage::RunCatalog(key)) => {
                self.catalog.open_from_floor = false;
                let Some(index) = self.catalog_index(&key) else {
                    // A Cube read from another network, or one whose network
                    // is still being checked: switch, then open it once the
                    // checked list confirms it.
                    return self
                        .select_on_other_network(&key, launcher_catalog::PendingIntent::Open)
                        .unwrap_or_else(Task::none);
                };
                Message::View(ViewMessage::Run(index))
            }
            Message::View(ViewMessage::RunFloorCatalog(key)) => {
                self.catalog.open_from_floor = false;
                let Some(index) = self.catalog_index(&key) else {
                    return self
                        .select_on_other_network(&key, launcher_catalog::PendingIntent::Open)
                        .unwrap_or_else(Task::none);
                };
                self.catalog.open_from_floor = self.catalog.selected.as_ref() == Some(&key)
                    && self.catalog.mode == launcher_catalog::LauncherMode::Cubes;
                if self.catalog.open_from_floor {
                    self.catalog.floor_open_generation =
                        self.catalog.floor_open_generation.wrapping_add(1);
                }
                Message::View(ViewMessage::Run(index))
            }
            Message::View(ViewMessage::RenameCatalog(key)) => {
                let Some(index) = self.catalog_index(&key) else {
                    return Task::none();
                };
                Message::View(ViewMessage::RenameCube(index))
            }
            Message::View(ViewMessage::DeleteCatalog(key)) => {
                let Some(index) = self.catalog_index(&key) else {
                    return Task::none();
                };
                Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ShowModal(index)))
            }
            message => message,
        };
        let message = match message {
            Message::FocusedCubes { for_chain, res } => {
                if !matches!(self.active_section, HomeSection::Cubes)
                    || !matches!(
                        self.state,
                        State::Cubes {
                            create_cube: false,
                            ..
                        }
                    )
                {
                    return Task::none();
                }
                Message::Checked { for_chain, res }
            }
            other => other,
        };
        self.update_inner(message)
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        if let Message::GlobalFeatureLoaded(request, enabled) = message {
            if request == self.global_feature_request {
                self.global_blake2b_enabled = enabled;
                return self.reload_global_settings();
            }
            return Task::none();
        }
        self.refresh_displayed_networks();
        if self.network.is_blake2b() {
            // Explicitly supported launcher actions only: the Create Cube
            // journey (name, PIN or passkey, backup), opening, renaming and
            // deleting a Cube, and bringing a Connect Cube back from its
            // recovery phrase, its Recovery Kit or a paired phone. Its Vault
            // is created from inside the Cube. No Bitcoin wallet import or
            // duress completion route.
            let allowed = match &message {
                // An unfinished import's Cube is never on this chain; its
                // request switches the Launcher to the Cube's network.
                Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ShowModalFor {
                    chain,
                    ..
                })) => !chain.is_blake2b(),
                Message::View(view) => matches!(
                    view,
                    ViewMessage::CreateCube
                        | ViewMessage::ShowCreateCube(_)
                        | ViewMessage::CubeNameEdited(_)
                        | ViewMessage::PinInput(_)
                        | ViewMessage::PinConfirmInput(_)
                        | ViewMessage::TogglePasskeyMode(_)
                        | ViewMessage::CreationBackup(_)
                        | ViewMessage::CreationBackupBypassRequested
                        | ViewMessage::CreationBackupChoiceRequested
                        | ViewMessage::CreationBackupAcknowledgeBypass(_)
                        | ViewMessage::CreationBackupBypassConfirmed
                        | ViewMessage::CancelCreationBackup
                        | ViewMessage::CreationKitRequested
                        | ViewMessage::CreationKit(_)
                        | ViewMessage::CreationKitUploaded(_)
                        | ViewMessage::CreateNetworkMenu(_)
                        | ViewMessage::ShowRestoreHub
                        | ViewMessage::ShowRestorePhone
                        | ViewMessage::ShowRestoreAllNetworks(_)
                        | ViewMessage::SelectNetwork(_)
                        | ViewMessage::Check
                        | ViewMessage::Run(_)
                        | ViewMessage::ConnectAccount(_)
                        | ViewMessage::ConnectSheetAccount(_)
                        | ViewMessage::ToggleConnect
                        | ViewMessage::ToggleDeveloperMode(_)
                        | ViewMessage::ToggleBitcoinBlake2bBeta(_)
                        | ViewMessage::ToggleTheme
                        | ViewMessage::OpenAppPreferences
                        | ViewMessage::ChangeDevicePreference(_)
                        | ViewMessage::GoToSection(
                            HomeSection::Cubes | HomeSection::Settings | HomeSection::Connect(_),
                        )
                        | ViewMessage::OpenUrl(_)
                        | ViewMessage::DismissAdvisoryNotice
                        | ViewMessage::RenameCube(_)
                        | ViewMessage::RenameCubeNameEdited(_)
                        | ViewMessage::RenameCubeConfirm
                        | ViewMessage::RenameCubeCancel
                        | ViewMessage::ImportGhostWithPhrase(_)
                        | ViewMessage::RecoveryWordInput { .. }
                        | ViewMessage::SelectRecoverySuggestion { .. }
                        | ViewMessage::SubmitRecovery
                        | ViewMessage::CancelRecovery
                        | ViewMessage::RestoreFromRecoveryKit(_)
                        | ViewMessage::RestoreFromAnyRecoveryKit
                        | ViewMessage::RestoreWithPhone { .. }
                        | ViewMessage::RestoreWithPhoneOn { .. }
                        | ViewMessage::ShowRecoveryMethodPicker(_)
                        | ViewMessage::CloseRecoveryMethodPicker
                        | ViewMessage::DeleteCube(
                            DeleteCubeMessage::ShowModal(_)
                                | DeleteCubeMessage::CloseModal
                                | DeleteCubeMessage::Confirm(_)
                                | DeleteCubeMessage::DeleteConnectBackup(_)
                                | DeleteCubeMessage::Deleted
                                | DeleteCubeMessage::DaemonStopped(_)
                                | DeleteCubeMessage::PinInput(_)
                                | DeleteCubeMessage::ShowRemoteModal(_)
                                | DeleteCubeMessage::CloseRemoteModal
                                | DeleteCubeMessage::RemoteNameInput(_)
                                | DeleteCubeMessage::ConfirmRemoteDelete(_)
                                | DeleteCubeMessage::RestoreInstead(_)
                        )
                ),
                Message::StartRecovery
                | Message::Checked { .. }
                | Message::CubeLimitsLoaded(_)
                | Message::RemoteCubesLoaded { .. }
                | Message::NetworkCatalogLoaded { .. }
                | Message::ConnectSignedInBubble
                | Message::CatchUpSyncFinished(_)
                | Message::CubeRemoteRegistered { .. }
                | Message::CubeRemoteUpdated { .. }
                | Message::CubeCreated(_)
                | Message::CubeBackupDeleted(_)
                | Message::RemoteCubeDeleted(_)
                | Message::CubeRenamed(_)
                | Message::PasskeyWindowId(_)
                | Message::PasskeyWebviewUpdate(_)
                | Message::PasskeyCeremonyResult(_)
                | Message::CancelPasskeyCeremony
                | Message::NativePasskeyTick => true,
                _ => false,
            };
            if !allowed {
                self.set_error("Bitcoin Blake2b Cubes can't be imported or restored here yet");
                return Task::none();
            }
            // Nothing on the Create Cube journey runs while this chain is
            // unavailable: no seed, keystore or passkey work starts.
            if matches!(
                &message,
                Message::View(
                    ViewMessage::CreateCube
                        | ViewMessage::ShowCreateCube(true)
                        | ViewMessage::Run(_)
                        | ViewMessage::TogglePasskeyMode(_)
                        | ViewMessage::CreationBackup(_)
                        | ViewMessage::CreationBackupBypassConfirmed
                        | ViewMessage::CreationKitRequested
                        | ViewMessage::CreationKit(_)
                )
            ) {
                if let Some(reason) = self.chain_availability(self.network).reason() {
                    self.set_error(reason.to_string());
                    return Task::none();
                }
            }
        }
        match message {
            Message::View(ViewMessage::Check) => self.on_focus(),
            Message::View(ViewMessage::ImportWallet) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Task::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::AddWallet, None)
                })
            }
            Message::View(ViewMessage::CreateWallet) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Task::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::CreateWallet, None)
                })
            }
            Message::View(ViewMessage::RestoreFromRecoveryKit(cube_uuid)) => {
                // Selecting a method closes the picker (a no-op when the
                // launch came straight from a single-method row).
                self.recovery_method_modal = None;
                // W13 — same launch shape as CreateWallet; the
                // installer picks the Recovery-Kit step sequence off
                // the UserFlow. The clicked cube's `uuid` is threaded
                // through so the step preselects it and skips the cube
                // picker.
                //
                // Forward the home's already-authenticated Connect
                // client so `RecoveryKitRestoreStep` can skip its own
                // email + OTP form (the user already signed in to see
                // the list of remote cubes on this very screen —
                // re-prompting them for the same credentials right
                // after they clicked "restore this one" is pure churn).
                // Falls back to `None` when the session isn't active
                // yet (developer-mode path, or stale cached home
                // state), in which case the step's own auth form
                // kicks in.
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                let client = self.connect_account.authenticated_client();
                Task::perform(
                    async move { (datadir_path, network, client) },
                    move |(d, n, c)| {
                        Message::Install(
                            d,
                            n,
                            UserFlow::RestoreFromRecoveryKit {
                                cube_uuid: Some(cube_uuid),
                            },
                            c,
                        )
                    },
                )
            }
            Message::View(ViewMessage::ShowRecoveryMethodPicker(cube_uuid)) => {
                // Open the picker for a cube that offers both methods. Snapshot
                // the row so the modal stays valid even if the list reloads.
                if let Some(cube) = self.remote_cubes.iter().find(|r| r.uuid == cube_uuid) {
                    self.recovery_method_modal = Some(RecoveryMethodModal { cube: cube.clone() });
                }
                Task::none()
            }
            Message::View(ViewMessage::CloseRecoveryMethodPicker) => {
                self.recovery_method_modal = None;
                Task::none()
            }
            Message::View(ViewMessage::ShowCreateCube(show)) => {
                // A queued navigation event must not clear the name while a
                // passkey ceremony or PIN creation task is using it.
                if !show && self.creating_cube {
                    return Task::none();
                }
                self.catalog.dismiss_popovers();
                self.restore_hub = false;
                self.restore_phone = false;
                if show {
                    self.pending_bitcoin_import = None;
                    self.create_network_menu = false;
                    self.create_network_selection_pending = false;
                    // Opening the form picks the default afresh: whether the
                    // passkey path can demonstrate a backup depends on the
                    // Connect session, which can change between visits.
                    self.passkey_mode = self.default_passkey_mode();
                }
                // Both list screens navigate to the same form, so both carry
                // the flag, including an empty Launcher on a fresh device.
                let toggled = match &mut self.state {
                    State::Cubes { create_cube, .. } | State::NoCube { create_cube } => {
                        *create_cube = show;
                        true
                    }
                    _ => false,
                };
                if toggled && !show {
                    self.create_network_menu = false;
                    self.create_network_selection_pending = false;
                    self.pending_bitcoin_import = None;
                    self.create_cube_name = coincube_ui::component::form::Value::default();
                    self.create_cube_pin = pin_input::PinInput::new();
                    self.create_cube_pin_confirm = pin_input::PinInput::new();
                    // Back to the default unlock method — dismissing the
                    // form discards the custody choice along with the rest
                    // of the inputs, so reopening it starts from the same
                    // state a first-time user sees.
                    self.passkey_mode = self.default_passkey_mode();
                    // Clear recovery words when exiting create cube flow
                    for word in &mut self.recovery_words {
                        word.clear();
                        word.shrink_to_fit();
                    }
                    self.recovery_active_index = None;
                    // Show every Cube when returning from an empty filter.
                    if !self.has_cube_list() {
                        self.catalog.filter = launcher_catalog::NetworkFilter::All;
                        self.catalog.page = 0;
                    }
                }
                Task::none()
            }
            Message::View(ViewMessage::ShowRestoreAllNetworks(all)) => {
                self.restore_all_networks = all;
                Task::none()
            }
            Message::View(ViewMessage::RestoreWithPhoneOn {
                chain,
                cube_id,
                full_cube,
            }) => {
                let switch = if chain == self.network {
                    Task::none()
                } else {
                    self.update_inner(Message::View(ViewMessage::SelectNetwork(chain)))
                };
                if self.network != chain {
                    // The switch was refused; its reason is on screen.
                    return switch;
                }
                Task::batch([
                    switch,
                    self.update_inner(Message::View(ViewMessage::RestoreWithPhone {
                        cube_id,
                        full_cube,
                    })),
                ])
            }
            Message::View(ViewMessage::CreateNetworkMenu(open)) => {
                self.create_network_menu = open && self.create_network_menu_editable();
                if !open {
                    return create_journey_focus_task(Id::new(restore_frame::NETWORK_CONTROL_ID));
                }
                Task::none()
            }
            Message::View(ViewMessage::ShowRestoreHub) => {
                // Leaving a path for the Overview drops that path's intent;
                // the Cube name and PIN stay as entered.
                if self.creating_cube
                    || !matches!(self.state, State::Cubes { .. } | State::NoCube { .. })
                {
                    return Task::none();
                }
                self.catalog.dismiss_popovers();
                self.pending_bitcoin_import = None;
                self.restore_hub = true;
                self.restore_phone = false;
                self.restore_all_networks = false;
                if let State::Cubes { create_cube, .. } | State::NoCube { create_cube } =
                    &mut self.state
                {
                    *create_cube = true;
                }
                self.error = None;
                Task::none()
            }
            Message::View(ViewMessage::ShowRestorePhone) => {
                if self.creating_cube
                    || !matches!(self.state, State::Cubes { .. } | State::NoCube { .. })
                    || !feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED
                {
                    return Task::none();
                }
                self.catalog.dismiss_popovers();
                self.pending_bitcoin_import = None;
                self.restore_hub = true;
                self.restore_phone = true;
                if let State::Cubes { create_cube, .. } | State::NoCube { create_cube } =
                    &mut self.state
                {
                    *create_cube = true;
                }
                self.error = None;
                Task::none()
            }
            Message::View(ViewMessage::RestoreFromAnyRecoveryKit) => {
                // The Recovery Kit path: the installer signs in to Connect
                // (or reuses this session) and lists the account's Cubes.
                self.recovery_method_modal = None;
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                let client = self.connect_account.authenticated_client();
                Task::perform(
                    async move { (datadir_path, network, client) },
                    move |(d, n, c)| {
                        Message::Install(
                            d,
                            n,
                            UserFlow::RestoreFromRecoveryKit { cube_uuid: None },
                            c,
                        )
                    },
                )
            }
            Message::View(ViewMessage::ShowImportBitcoin) => {
                self.catalog.dismiss_popovers();
                if self.creating_cube
                    || !matches!(
                        self.state,
                        State::Cubes { .. } | State::NoCube { .. } | State::Unchecked
                    )
                {
                    return Task::none();
                }
                if self.network != ChainId::Bitcoin {
                    self.set_error("Import a Bitcoin wallet from the Bitcoin Cube list.");
                    return Task::none();
                }
                // Every entry is disabled at the limit; this refuses a queued
                // or stale press before the user fills in a name and PIN.
                if launcher_catalog::project(self).at_limit() {
                    self.set_error(
                        "Bitcoin mainnet has no free Cube slot on your plan. Upgrade, or delete \
                         a Bitcoin mainnet Cube, to import a wallet.",
                    );
                    return Task::none();
                }
                if self.pending_bitcoin_import.is_none() {
                    self.pending_bitcoin_import = Some(Box::new(BitcoinImport::pending()));
                }
                self.restore_hub = false;
                self.restore_phone = false;
                // A passkey can't hold an imported secret: import sets a PIN.
                self.passkey_mode = false;
                match &mut self.state {
                    State::Cubes { create_cube, .. } | State::NoCube { create_cube } => {
                        *create_cube = true;
                    }
                    State::Unchecked => {}
                    _ => return Task::none(),
                }
                self.error = None;
                Task::none()
            }
            Message::View(ViewMessage::CubeNameEdited(name)) => {
                self.create_cube_name.value = name;
                self.create_cube_name.valid = !self.create_cube_name.value.trim().is_empty();
                self.error = None; // Clear error when user makes changes
                Task::none()
            }
            Message::View(ViewMessage::PinInput(msg)) => {
                self.error = None;
                self.create_cube_pin
                    .update(msg)
                    .map(|m| Message::View(ViewMessage::PinInput(m)))
            }
            Message::View(ViewMessage::PinConfirmInput(msg)) => {
                self.error = None;
                self.create_cube_pin_confirm
                    .update(msg)
                    .map(|m| Message::View(ViewMessage::PinConfirmInput(m)))
            }
            Message::View(ViewMessage::TogglePasskeyMode(enabled)) => {
                // Import sets a PIN only: a passkey can't hold an imported
                // secret.
                self.passkey_mode = enabled && self.pending_bitcoin_import.is_none();
                self.error = None;
                Task::none()
            }
            Message::View(ViewMessage::CreateCube) => {
                if self.creating_cube {
                    return Task::none();
                }
                if self.pending_bitcoin_import.is_some() && self.network != ChainId::Bitcoin {
                    self.set_error("Import a Bitcoin wallet from the Bitcoin Cube list.");
                    return Task::none();
                }

                if self.create_cube_name.value.trim().is_empty() {
                    return Task::none();
                }

                // Enforce per-network Cube limit based on Connect account tier.
                // Includes remote cubes (on server but not local) since the
                // limit applies across all devices.
                let cube_count = self.total_cube_count();
                let limit = self.cube_limit();
                if cube_count >= limit {
                    self.error = Some(self.cube_limit_error());
                    return Task::none();
                }

                // Defensive guard: even if `self.passkey_mode` somehow became
                // true while the feature is disabled (stale state, manual
                // toggle before a hot-reload, etc.), always fall through to
                // the PIN flow when the compile-time flag is off.
                let passkey_mode = self.passkey_mode && feature_flags::PASSKEY_CREATION_AVAILABLE;

                if !passkey_mode {
                    // PIN-based flow: validate PIN
                    if !self.create_cube_pin.is_complete() {
                        self.error = Some("Please enter all 4 PIN digits".to_string());
                        return Task::none();
                    }
                    if !self.create_cube_pin_confirm.is_complete() {
                        self.error = Some("Please confirm all 4 PIN digits".to_string());
                        return Task::none();
                    }
                    if self.create_cube_pin.value() != self.create_cube_pin_confirm.value() {
                        self.error = Some("PIN codes do not match".to_string());
                        return Task::none();
                    }
                }

                self.creating_cube = true;
                let cube_name = self.create_cube_name.value.trim().to_string();

                // Pre-generate the UUID before the async task so that retries
                // reuse the same identifier (idempotent creation). The
                // PIN-based path re-reads it in `finalize_cube_creation`, which
                // is what actually persists the Cube.
                let cube_id = *self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);

                let without_recovery = if passkey_mode {
                    // Passkey-based Cube creation.
                    // On macOS: use the native AuthenticationServices framework
                    //   (WKWebView doesn't have the entitlement to call WebAuthn).
                    // On other platforms: fall back to the embedded webview ceremony.
                    #[cfg(target_os = "macos")]
                    {
                        let user_id_bytes = cube_id.as_bytes().to_vec();
                        match crate::services::passkey::macos::NativePasskeyCeremony::register(
                            passkey_svc::RP_ID,
                            &user_id_bytes,
                            &cube_name,
                        ) {
                            Ok(ceremony) => {
                                self.native_passkey_ceremony = Some(ceremony);
                                Task::none()
                            }
                            Err(e) => {
                                self.creating_cube = false;
                                self.error = Some(coincube_ui::localized_format!(
                                    "Failed to start passkey ceremony: {}",
                                    "No se pudo iniciar la verificación con llave de acceso: {}",
                                    e
                                ));
                                Task::none()
                            }
                        }
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        let user_id = cube_id.to_string();
                        let ceremony = PasskeyCeremony::new(CeremonyMode::Register {
                            user_id,
                            user_name: cube_name,
                        });
                        self.passkey_ceremony = Some(ceremony);
                        // Extract the window ID so we can attach the webview
                        iced_wry::extract_window_id(None).map(Message::PasskeyWindowId)
                    }
                } else {
                    // PIN-based Cube creation, phase 1: generate the seed into
                    // memory and hand it to the backup step.
                    //
                    // Nothing is written to disk here — not the seed file, not
                    // the settings entry, not the OS-keystore device secret.
                    // All of that is `finalize_cube_creation`, which runs only
                    // once the user has demonstrated a backup or explicitly
                    // bypassed. Abandoning the step therefore leaves no
                    // half-created Cube and no orphaned keystore entry.
                    //
                    // Probe the keystore **here**, before a single word is
                    // shown. `finalize_cube_creation` mints the real secret,
                    // and if the keystore is unusable it refuses (I7) — but
                    // refusing there means the user has already written down
                    // twelve words and verified three of them, for a Cube that
                    // will never exist. Probe early, mint late: `capability()`
                    // writes and deletes a throwaway item, so it stakes no
                    // claim on this Cube and leaves 3a's
                    // no-writes-until-committed property intact.
                    if let unlock::device_secret::Capability::Unavailable(why) =
                        unlock::device_secret::capability()
                    {
                        self.creating_cube = false;
                        self.error = Some(why);
                        return Task::none();
                    }
                    match MasterSigner::generate(self.network.bitcoin_network()) {
                        Ok(signer) => {
                            self.creating_cube = false;
                            self.error = None;
                            self.enter_creation_backup(
                                signer.words().iter().map(|w| w.to_string()).collect(),
                            );
                            Task::none()
                        }
                        Err(e) => {
                            self.creating_cube = false;
                            self.error = Some(coincube_ui::localized_format!(
                                "Failed to generate master seed signer: {}",
                                "No se pudo generar el firmante de la frase maestra: {}",
                                e
                            ));
                            Task::none()
                        }
                    }
                };

                without_recovery
            }
            Message::StartRecovery => {
                self.state = State::RecoveryInput;
                self.recovery_active_index = None;
                Task::none()
            }
            Message::View(ViewMessage::ImportGhostWithPhrase(uuid)) => {
                let Some(ghost) = self.remote_cubes.iter().find(|cube| cube.uuid == uuid) else {
                    return Task::none();
                };
                // A ghost's face is shown only on its own network.
                let Some(network) = launcher_catalog::ghost_chain(ghost) else {
                    return Task::none();
                };
                self.ghost_import = Some(GhostImport {
                    uuid: ghost.uuid.clone(),
                    name: ghost.name.clone(),
                    encryption_pubkey: ghost.encryption_pubkey.clone(),
                    network,
                    account: self.connect_account_id(),
                });
                self.create_cube_name = coincube_ui::component::form::Value {
                    value: catalog_display_name(&ghost.name),
                    warning: None,
                    valid: true,
                };
                self.catalog.selected = None;
                self.catalog.motion = None;
                self.error = None;
                self.update(Message::StartRecovery)
            }
            Message::View(ViewMessage::CreationBackup(msg)) => self.update_creation_backup(msg),
            Message::View(ViewMessage::CreationBackupBypassRequested) => {
                // Not while a Kit upload is in flight: its result decides
                // whether a Cube gets written, and taking the acknowledgement
                // underneath it would race two finalizers.
                if matches!(
                    self.state,
                    State::CreationBackup(CreationBackupStep::KitUploading)
                ) || self.creation_backup_hub().any()
                {
                    // A finished backup is written by Continue; the bypass
                    // would record "no backup" over it.
                    return Task::none();
                }
                if matches!(self.state, State::CreationBackup(_)) {
                    self.state = State::CreationBackup(CreationBackupStep::Bypass {
                        acknowledged: false,
                    });
                    self.error = None;
                }
                Task::none()
            }
            Message::View(ViewMessage::CreationBackupAcknowledgeBypass(acknowledged)) => {
                if let State::CreationBackup(CreationBackupStep::Bypass { .. }) = &self.state {
                    self.state = State::CreationBackup(CreationBackupStep::Bypass { acknowledged });
                }
                Task::none()
            }
            Message::View(ViewMessage::CreationBackupBypassConfirmed) => {
                // Only an *acknowledged* bypass proceeds. Reaching this message
                // with the box unticked means the view let a click through, and
                // the safe answer is to do nothing rather than create an
                // unbacked-up Cube the user never agreed to.
                if !matches!(
                    self.state,
                    State::CreationBackup(CreationBackupStep::Bypass { acknowledged: true })
                ) || self.creation_backup_hub().any()
                {
                    return Task::none();
                }
                let bypass = creation_gate::CreationBackupBypass {
                    at: chrono::Utc::now().timestamp(),
                    // Verbatim, so a later support conversation is about the
                    // same words the user actually agreed to.
                    acknowledged: creation_gate::BYPASS_ACKNOWLEDGEMENT.to_string(),
                };
                // Same acknowledgement, same recorded evidence, either shape of
                // Cube. Only the write differs: a passkey Cube has no seed file
                // or device secret to mint.
                if self.creating_passkey_cube() {
                    self.finalize_passkey_cube_creation(false, Some(bypass), None)
                } else {
                    self.finalize_cube_creation(false, Some(bypass), None)
                }
            }
            Message::View(ViewMessage::CreationBackupChoiceRequested) => {
                // Never mid-upload: that result decides whether a Cube gets
                // written, and stepping away underneath it would race two
                // finalizers.
                if matches!(
                    self.state,
                    State::CreationBackup(CreationBackupStep::KitUploading)
                ) {
                    return Task::none();
                }
                if matches!(self.state, State::CreationBackup(_)) {
                    self.scrub_creation_kit();
                    self.state = State::CreationBackup(CreationBackupStep::Choice);
                    self.error = None;
                }
                Task::none()
            }
            Message::View(ViewMessage::CreationKitRequested) => {
                // From the choice screen, or back out of "I'll do this later" —
                // the acknowledgement is a decision the user may reverse while
                // the Cube is still unwritten. Never while a Kit is out of
                // reach: a signed-out session has nowhere to upload one.
                // Applies to both unlock methods.
                if !matches!(
                    self.state,
                    State::CreationBackup(
                        CreationBackupStep::Choice | CreationBackupStep::Bypass { .. }
                    )
                ) || !self.can_create_recovery_kit()
                    || self.creation_kit_evidence.is_some()
                {
                    return Task::none();
                }
                self.scrub_creation_kit();
                self.state = State::CreationBackup(CreationBackupStep::KitPassword);
                Task::none()
            }
            Message::View(ViewMessage::CreationKit(msg)) => self.update_creation_kit(msg),
            Message::View(ViewMessage::CreationKitUploaded(result)) => {
                // Only the upload this step is waiting on counts.
                if !matches!(
                    self.state,
                    State::CreationBackup(CreationBackupStep::KitUploading)
                ) {
                    return Task::none();
                }
                match result {
                    Ok(evidence) => {
                        // The Kit is on Connect. Back to the hub, which shows
                        // it as backed up; Continue there writes the Cube with
                        // this evidence (and the phrase's, if confirmed too).
                        self.scrub_creation_kit();
                        self.creating_cube = false;
                        self.creation_kit_evidence = Some(evidence);
                        self.state = State::CreationBackup(CreationBackupStep::Choice);
                        Task::none()
                    }
                    Err(e) => {
                        // Nothing was written. Back to the password screen with
                        // the reason: the user can retry, or take the
                        // acknowledgement instead. Both exits still exist.
                        tracing::warn!("recovery kit upload during creation failed: {}", e);
                        self.creating_cube = false;
                        self.creation_kit_error = Some(e);
                        self.state = State::CreationBackup(CreationBackupStep::KitPassword);
                        Task::none()
                    }
                }
            }
            Message::View(ViewMessage::CreationBackupContinue) => self.finish_creation_backup(),
            Message::View(ViewMessage::CancelCreationBackup) => {
                self.abandon_creation_backup();
                Task::none()
            }
            Message::CubeCreated(res) => {
                self.creating_cube = false;
                match res {
                    Ok(cube) => {
                        // UUID was consumed successfully — reset it so the next
                        // cube creation starts with a fresh identifier.
                        self.pending_cube_id = None;
                        self.ghost_import = None;
                        let ghost_claim = self.ghost_claim.take();
                        // The operation identity is now durable on this exact
                        // Cube. A fresh creation must not reuse it.
                        self.pending_bitcoin_import = None;
                        self.open_created_cube = None;
                        self.open_after_registration = None;
                        // Clear any previous error state
                        self.error = None;
                        // Reset form fields
                        self.create_cube_name = coincube_ui::component::form::Value::default();
                        self.create_cube_pin = pin_input::PinInput::new();
                        self.create_cube_pin_confirm = pin_input::PinInput::new();
                        // The creation-time backup step's copy of the seed has
                        // done its job — scrub it. The step itself is replaced
                        // just below, and `reload()` refreshes that list.
                        self.scrub_creation_seed();
                        self.scrub_creation_kit();
                        self.reset_creation_backup_progress();
                        // The passkey registration has been consumed by the
                        // write; holding it would let a second finalize insert
                        // a duplicate.
                        self.pending_passkey_cube = None;
                        // Off the backup hub now, not when the reload lands:
                        // in between, its Continue and "I'll do this later"
                        // would act on a Cube that is already written.
                        let parked = std::mem::take(&mut self.creation_backup_cubes);
                        if matches!(self.state, State::CreationBackup(_)) {
                            self.state = State::Cubes {
                                cubes: parked,
                                create_cube: false,
                                source: self.network,
                            };
                        }
                        // Explicitly clear recovery words to prevent mnemonic from lingering in memory
                        for word in &mut self.recovery_words {
                            word.clear();
                            word.shrink_to_fit();
                        }
                        self.recovery_active_index = None;
                        let reload_task = self.reload();

                        // If logged in, register the new cube with the Connect API.
                        // A Cube whose Recovery Kit was made during creation is
                        // already registered — the Kit could not have been
                        // uploaded otherwise — and registering again would
                        // create a duplicate server-side record.
                        if cube.remote_synced {
                            tracing::debug!(
                                "Cube {} was registered by the creation Recovery Kit — \
                                 skipping the post-creation registration",
                                cube.id
                            );
                            self.open_created_cube = Some(cube.id.clone());
                            reload_task
                        } else if ghost_claim.as_ref().is_some_and(|(uuid, account)| {
                            *uuid == cube.id && *account != self.connect_account_id()
                        }) {
                            // A ghost's Cube reclaims a registration in the
                            // account that listed it. Signed out or switched
                            // since, it stays local: the catch-up sync
                            // registers it once that account is back.
                            tracing::warn!(
                                "Cube {} reclaimed a Connect registration from another session; \
                                 not registering it with the current account",
                                cube.id
                            );
                            self.open_created_cube = Some(cube.id.clone());
                            reload_task
                        } else if let Some(client) = self.connect_account.authenticated_client() {
                            let cube_id = cube.id.clone();
                            let cube_network = cube.network;
                            let req = RegisterCubeRequest {
                                uuid: cube.id.clone(),
                                name: cube.name.clone(),
                                network: cube.api_network_string(),
                                // Monotonic upgrade-only report: a freshly-
                                // created Cube has no Vault yet, so omit the
                                // flag (server default `false`). It flips later
                                // via the vault-creation re-report
                                // (PLAN-duress-vault-gate PR 3).
                                has_vault: cube.vault_wallet_id.is_some().then_some(true),
                                spark_stable_balance: cube.spark_stable_balance,
                            };
                            self.open_after_registration = Some(cube.id.clone());
                            let register_task = Task::perform(
                                async move {
                                    client.register_cube(req).await.map_err(|e| e.to_string())
                                },
                                move |result| Message::CubeRemoteRegistered {
                                    cube_id,
                                    network: cube_network,
                                    result,
                                },
                            );
                            Task::batch([reload_task, register_task])
                        } else {
                            self.open_created_cube = Some(cube.id.clone());
                            reload_task
                        }
                    }
                    Err(e) => {
                        self.ghost_claim = None;
                        // Retain pending_cube_id so a retry reuses the same UUID,
                        // unless a Kit was saved for it (`abandon_creation_backup`).
                        // Clear recovery words on error too
                        for word in &mut self.recovery_words {
                            word.clear();
                            word.shrink_to_fit();
                        }
                        self.recovery_active_index = None;
                        // A failed write means no Cube exists, so the seed the
                        // user was just shown is worthless — scrub it and send
                        // them back to the create form, where the error below
                        // is visible and a retry generates a fresh seed.
                        if matches!(self.state, State::CreationBackup(_)) {
                            self.abandon_creation_backup();
                        } else {
                            self.scrub_creation_seed();
                        }
                        self.error = Some(coincube_ui::localized_format!(
                            "Failed to create Cube: {}",
                            "No se pudo crear el Cube: {}",
                            e
                        ));
                        Task::none()
                    }
                }
            }
            // --- Passkey ceremony flow ---
            Message::PasskeyWindowId(window_id) => {
                if let Some(ceremony) = &mut self.passkey_ceremony {
                    if !ceremony.create_webview(window_id) {
                        self.creating_cube = false;
                        self.passkey_ceremony = None;
                        self.error = Some(
                            "Failed to open passkey webview. Check your system's WebView support."
                                .to_string(),
                        );
                    }
                }
                Task::none()
            }
            Message::PasskeyWebviewUpdate(msg) => {
                if let Some(ceremony) = &mut self.passkey_ceremony {
                    ceremony.webview_manager.update(msg);

                    // Poll for IPC result
                    if let Some(result) = ceremony.try_recv_result() {
                        return Task::done(Message::PasskeyCeremonyResult(result));
                    }
                }
                Task::none()
            }
            Message::CubeRemoteRegistered {
                cube_id,
                network,
                result,
            } => {
                let open = self.open_after_registration.as_ref() == Some(&cube_id);
                if open {
                    self.open_after_registration = None;
                    self.open_created_cube = Some(cube_id.clone());
                }
                match result {
                    Ok(resp) => {
                        log::info!(
                            "[LAUNCHER] Cube {} registered remotely (server ID: {})",
                            resp.uuid,
                            resp.id
                        );
                        self.cube_sync_errors.remove(&cube_id);
                        let network_dir = self.datadir_path.network_directory(network);
                        let owner = self.connect_owner();
                        Task::perform(
                            async move {
                                settings::mark_cube_synced(&network_dir, &cube_id, owner)
                                    .await
                                    .ok();
                            },
                            |_| Message::View(ViewMessage::Check),
                        )
                    }
                    Err(e) => {
                        log::warn!(
                            "[LAUNCHER] Failed to register cube {} remotely: {}",
                            cube_id,
                            e
                        );
                        // Keep the reason: this is the only signal the Cubes list
                        // has that the Cube is not merely mid-sync.
                        self.cube_sync_errors.insert(cube_id, e);
                        // The Cube still opens; the catch-up sync registers it.
                        if open {
                            self.reload()
                        } else {
                            Task::none()
                        }
                    }
                }
            }
            Message::CatchUpSyncFinished(Err(())) => {
                // The round never ran, so it has no verdict to give. Keep the
                // reasons already on record: wiping them here would drop a
                // Cube back to "it will sync automatically in a moment" — the
                // false promise this whole map exists to replace — on nothing
                // more than a flaky `list_cubes` during a sign-in.
                Task::none()
            }
            Message::CatchUpSyncFinished(Ok(outcomes)) => {
                // Apply per Cube rather than replacing the map. A Cube the
                // round synced loses its reason (a slot freed up, a rename went
                // through); one it could not sync gains the current one. A Cube
                // the round never examined — created after it snapshotted the
                // unsynced set, and refused in the meantime — keeps what
                // `CubeRemoteRegistered` recorded, which a blanket clear would
                // have erased.
                for (cube_id, outcome) in outcomes {
                    match outcome {
                        Some(reason) => {
                            self.cube_sync_errors.insert(cube_id, reason);
                        }
                        None => {
                            self.cube_sync_errors.remove(&cube_id);
                        }
                    }
                }
                // Successes wrote `remote_synced` to disk; reload so the list
                // reflects them.
                self.reload()
            }
            Message::CubeLimitsLoaded(result) => {
                match result {
                    Ok(limits) => {
                        self.server_cube_limit = Some(limits.max_allowed);
                    }
                    Err(e) => {
                        log::warn!("[LAUNCHER] Failed to fetch cube limits: {}", e);
                    }
                }
                Task::none()
            }
            Message::PasskeyCeremonyResult(result) => {
                // Close the ceremony webview
                if let Some(mut ceremony) = self.passkey_ceremony.take() {
                    ceremony.close();
                }

                match result {
                    Ok(CeremonyOutcome::Registered(registration)) => {
                        // Shared with the native macOS path: one place decides
                        // what a successful registration does, so the two
                        // ceremony backends cannot drift on whether the Cube is
                        // gated (they did — this one persisted immediately).
                        self.passkey_registration_succeeded(
                            registration.credential_id.clone(),
                            &registration.prf_output,
                        )
                    }
                    Ok(CeremonyOutcome::Authenticated(_auth)) => {
                        // Authentication during creation shouldn't happen,
                        // but handle gracefully.
                        self.creating_cube = false;
                        self.error = Some(
                            "Unexpected authentication response during registration.".to_string(),
                        );
                        Task::none()
                    }
                    Err(e) => {
                        self.creating_cube = false;
                        // `Display` is the developer string ("Passkey ceremony
                        // failed: …"); the user gets the registration copy, and
                        // the detail goes to the log where it is of use.
                        tracing::warn!("passkey registration failed: {e}");
                        self.error = Some(e.registration_message());
                        Task::none()
                    }
                }
            }
            Message::CancelPasskeyCeremony => {
                if let Some(mut ceremony) = self.passkey_ceremony.take() {
                    ceremony.close();
                }
                #[cfg(target_os = "macos")]
                {
                    if let Some(ceremony) = self.native_passkey_ceremony.take() {
                        ceremony.cancel();
                    }
                }
                self.creating_cube = false;
                Task::none()
            }
            #[cfg(target_os = "macos")]
            Message::NativePasskeyTick => {
                use crate::services::passkey::macos::NativeOutcome;
                let outcome = self
                    .native_passkey_ceremony
                    .as_ref()
                    .and_then(|c| c.try_recv());
                let Some(outcome) = outcome else {
                    return Task::none();
                };
                // Drop the ceremony now that we have a result.
                self.native_passkey_ceremony = None;

                match outcome {
                    NativeOutcome::Registered {
                        credential_id,
                        prf_output,
                    } => {
                        // Same shared transition as the webview ceremony.
                        self.passkey_registration_succeeded_raw(&credential_id, &prf_output)
                    }
                    NativeOutcome::Authenticated { .. } => {
                        self.creating_cube = false;
                        self.error = Some(
                            "Unexpected authentication response during registration.".to_string(),
                        );
                        Task::none()
                    }
                    NativeOutcome::Failed(e) => {
                        self.creating_cube = false;
                        // Registration, not unlock — but the same rule applies:
                        // a cancelled Touch ID prompt must read as "nothing
                        // happened", not as a failure the user has to interpret.
                        // Nothing has been written to disk at this point, which
                        // is why `registration_message` and not `user_message`:
                        // the unlock copy would name a Cube that doesn't exist
                        // and send the user to a Recovery Kit they can't have.
                        tracing::warn!("passkey registration failed: {e}");
                        self.error = Some(e.registration_message());
                        Task::none()
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            Message::NativePasskeyTick => Task::none(),
            Message::NetworkCatalogLoaded {
                generation,
                chain,
                cubes,
            } => {
                // A read superseded by a newer load, or for a network that
                // has since become current or stopped being listed, is
                // dropped: it describes nothing on screen.
                if generation != self.network_catalog_generation
                    || !self.network_catalog_requested.contains(&chain)
                {
                    return Task::none();
                }
                self.network_catalog_unread.retain(|c| *c != chain);
                match cubes {
                    Ok(cubes) => match self.network_catalog.iter_mut().find(|(c, _)| *c == chain) {
                        Some((_, list)) => *list = cubes,
                        None => self.network_catalog.push((chain, cubes)),
                    },
                    // Keep whatever was last read; the menu says the
                    // network is unavailable rather than showing 0 Cubes.
                    Err(_) => self.network_catalog_unread.push(chain),
                }
                Task::none()
            }
            Message::RemoteCubesLoaded { request_id, result } => {
                self.apply_remote_cubes(request_id, result)
            }
            Message::CubeRemoteUpdated {
                cube_id,
                network,
                result,
            } => match result {
                Ok(_) => {
                    log::info!("[LAUNCHER] Cube {} updated remotely", cube_id);
                    let network_dir = self.datadir_path.network_directory(network);
                    let owner = self.connect_owner();
                    Task::perform(
                        async move {
                            settings::mark_cube_synced(&network_dir, &cube_id, owner)
                                .await
                                .ok();
                        },
                        |_| Message::View(ViewMessage::Check),
                    )
                }
                Err(e) => {
                    log::warn!(
                        "[LAUNCHER] Failed to update cube {} remotely: {}",
                        cube_id,
                        e
                    );
                    Task::none()
                }
            },
            Message::CubeBackupDeleted(result) => {
                match &result {
                    Ok(()) => log::info!("[LAUNCHER] Cube Connect backup deleted"),
                    Err(e) => log::warn!("[LAUNCHER] Failed to delete cube backup: {}", e),
                }
                // Either outcome changes what the restore list should show, and
                // the delete has now settled: on success the Cube is gone from
                // Connect and must not reappear as restorable; on failure its
                // Connect copy survived the local delete, so it *is* restorable
                // and has to show up. Both are one re-fetch away.
                self.fetch_remote_cubes()
            }
            Message::RemoteCubeDeleted(result) => {
                match result {
                    Ok(()) => {
                        log::info!("[LAUNCHER] Remote cube deleted");
                        if let Some(modal) = self.delete_remote_cube_modal.take() {
                            self.remote_cubes.retain(|rc| rc.uuid != modal.cube.uuid);
                            let reload_task = self.reload();
                            // Dropping the row locally is not enough: a fetch
                            // issued before the delete settled is still in
                            // flight, and its pre-delete list still contains
                            // this Cube. `Message::Checked` only *prunes*
                            // `remote_cubes`, so nothing in `reload` would stop
                            // that result from putting the deleted row back on
                            // screen. Re-fetching claims a new generation,
                            // which discards the in-flight result, and replaces
                            // it with the server's post-delete state.
                            return Task::batch([reload_task, self.fetch_remote_cubes()]);
                        }
                    }
                    Err(e) => {
                        log::warn!("[LAUNCHER] Failed to delete remote cube: {}", e);
                        if let Some(modal) = &mut self.delete_remote_cube_modal {
                            modal.deleting = false;
                            modal.error = Some(e);
                        }
                    }
                }
                Task::none()
            }
            Message::CubeRenamed(result) => match result {
                Ok(()) => {
                    self.rename_cube_modal = None;
                    let reload_task = self.reload();

                    // Fire remote update now that local write succeeded
                    if let Some(pending) = self.pending_remote_rename.take() {
                        if let Some(client) = self.connect_account.authenticated_client() {
                            let update_req = UpdateCubeRequest {
                                name: Some(pending.new_name),
                                status: None,
                                // Name-only rename: leave server Vault presence
                                // and the Spark Stable Balance record untouched.
                                has_vault: None,
                                spark_stable_balance: None,
                            };
                            let cube_uuid = pending.cube_id.clone();
                            let cube_id = pending.cube_id;
                            let cube_network = pending.cube_network;
                            let remote_task = Task::perform(
                                async move {
                                    let cubes =
                                        client.list_cubes().await.map_err(|e| e.to_string())?;
                                    let server_cube = cubes.iter().find(|c| c.uuid == cube_uuid);
                                    if let Some(sc) = server_cube {
                                        let server_id = sc.id.to_string();
                                        client
                                            .update_cube(&server_id, update_req)
                                            .await
                                            .map_err(|e| e.to_string())
                                    } else {
                                        Err("Cube not yet registered remotely".to_string())
                                    }
                                },
                                move |result| Message::CubeRemoteUpdated {
                                    cube_id,
                                    network: cube_network,
                                    result,
                                },
                            );
                            return Task::batch([reload_task, remote_task]);
                        }
                    }
                    reload_task
                }
                Err(e) => {
                    // Clear pending remote rename on local failure
                    self.pending_remote_rename = None;
                    self.error = Some(format!("Failed to rename Cube: {}", e));
                    Task::none()
                }
            },
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ShowModalFor {
                cube_id,
                chain,
            })) => {
                // The Launcher may be listing another network. Its probe
                // then reads the Cube's network.
                let load = if chain == self.network {
                    self.reload()
                } else {
                    self.update_inner(Message::View(ViewMessage::SelectNetwork(chain)))
                };
                if self.network != chain {
                    // The switch was refused; its reason is on screen.
                    return load;
                }
                // Never the row already listed: a retained Launcher may hold
                // one from before the import moved on, and the delete checks
                // its snapshot against the Cube's current settings.
                self.delete_after_check = Some(cube_id);
                load
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ShowModal(i))) => {
                if let State::Cubes { cubes, .. } = &self.state {
                    if let Some(cube) = cubes.get(i) {
                        let wallet_datadir = self.datadir_path.network_directory(cube.network);
                        let config_path =
                            wallet_datadir.path().join(app::config::DEFAULT_FILE_NAME);

                        // Get wallet settings if vault exists
                        let (wallet_settings, internal_bitcoind) =
                            if let Some(vault_id) = &cube.vault_wallet_id {
                                match settings::Settings::from_file(&wallet_datadir) {
                                    Ok(s) => {
                                        if let Some(wallet) =
                                            s.wallets.iter().find(|w| w.wallet_id() == *vault_id)
                                        {
                                            let internal_bitcoind =
                                                if wallet.remote_backend_auth.is_some() {
                                                    Some(false)
                                                } else if wallet.start_internal_bitcoind.is_some() {
                                                    wallet.start_internal_bitcoind
                                                } else if let Ok(cfg) =
                                                    app::Config::from_file(&config_path)
                                                {
                                                    Some(cfg.start_internal_bitcoind)
                                                } else {
                                                    None
                                                };
                                            (Some(wallet.clone()), internal_bitcoind)
                                        } else {
                                            (None, None)
                                        }
                                    }
                                    Err(_) => (None, None),
                                }
                            } else {
                                (None, None)
                            };

                        self.delete_cube_modal = Some(DeleteCubeModal::new(
                            cube.clone(),
                            wallet_datadir,
                            self.datadir_path.path().to_path_buf(),
                            wallet_settings,
                            internal_bitcoind,
                            self.connect_account.is_authenticated(),
                        ));
                    }
                }
                Task::none()
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(uuid))) => {
                if let Some(rc) = self.remote_cubes.iter().find(|r| r.uuid == uuid) {
                    self.delete_remote_cube_modal = Some(DeleteRemoteCubeModal {
                        cube: rc.clone(),
                        deleting: false,
                        error: None,
                        typed_name: String::new(),
                    });
                }
                Task::none()
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::CloseRemoteModal)) => {
                let opener = self
                    .delete_remote_cube_modal
                    .as_ref()
                    .map(|modal| modal.cube.uuid.clone());
                self.delete_remote_cube_modal = None;
                opener
                    .as_deref()
                    .map(|uuid| focus_remote_delete_opener(self, uuid))
                    .unwrap_or_else(Task::none)
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::RemoteNameInput(name))) => {
                if let Some(modal) = &mut self.delete_remote_cube_modal {
                    if !modal.deleting {
                        modal.typed_name = name;
                    }
                }
                Task::none()
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::RestoreInstead(uuid))) => {
                // A Cube with nothing this build can restore keeps its modal
                // open: no button offers this, and a stale press must not
                // close it.
                let Some(modal) = self.delete_remote_cube_modal.take_if(|modal| {
                    !modal.deleting && modal.cube.uuid == uuid && modal.restorable()
                }) else {
                    return Task::none();
                };
                match launcher_catalog::remote_restore(&modal.cube).action {
                    Some((restore, _)) => self.update_inner(Message::View(restore)),
                    None => Task::none(),
                }
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ConfirmRemoteDelete(
                uuid,
            ))) => {
                // The view only offers Delete once the name matches; a stale
                // press for another Cube, or one before that, does nothing.
                match &mut self.delete_remote_cube_modal {
                    Some(modal)
                        if !modal.deleting && modal.cube.uuid == uuid && modal.name_confirmed() =>
                    {
                        modal.deleting = true;
                    }
                    _ => return Task::none(),
                }
                if let Some(client) = self.connect_account.authenticated_client() {
                    Task::perform(
                        async move {
                            let cubes = client.list_cubes().await.map_err(|e| e.to_string())?;
                            let server_cube = cubes.iter().find(|c| c.uuid == uuid);
                            if let Some(cube) = server_cube {
                                let server_id = cube.id.to_string();
                                client
                                    .delete_cube(&server_id)
                                    .await
                                    .map_err(|e| e.to_string())
                            } else {
                                Err("Cube not found on server".to_string())
                            }
                        },
                        Message::RemoteCubeDeleted,
                    )
                } else {
                    if let Some(modal) = &mut self.delete_remote_cube_modal {
                        modal.deleting = false;
                        modal.error = Some("Not authenticated with Connect".to_string());
                    }
                    Task::none()
                }
            }
            Message::View(ViewMessage::SelectNetwork(network)) => {
                self.catalog.network_menu = false;
                self.create_network_menu = false;
                // Not while a Cube is mid-creation: its seed, passkey and any
                // Recovery Kit were made for this network, and the write reads
                // `self.network`. The header menu is closed in the step; this
                // refuses the other routes too.
                if matches!(self.state, State::CreationBackup(_)) || self.creating_cube {
                    self.catalog.pending_select = None;
                    self.set_error(
                        "Finish or cancel creating this Cube before switching networks."
                            .to_string(),
                    );
                    return Task::none();
                }
                if !(self.developer_mode
                    || network == ChainId::Bitcoin
                    || network == ChainId::BitcoinBlake2b
                        && self.chain_availability(network).is_available())
                {
                    tracing::debug!(
                        "Ignoring SelectNetwork action because developer mode is disabled"
                    );
                    self.catalog.pending_select = None;
                    return Task::none();
                }
                if let Some(reason) = self.chain_availability(network).reason() {
                    self.set_error(reason.to_string());
                    self.catalog.pending_select = None;
                    return Task::none();
                }
                self.create_network_selection_pending =
                    matches!(self.active_section, HomeSection::Cubes)
                        && self.showing_create_form()
                        && self.pending_bitcoin_import.is_none();
                self.network = network;
                self.open_created_cube = None;
                self.delete_after_check = None;
                self.open_after_registration = None;
                if network != ChainId::Bitcoin {
                    // Import Bitcoin wallet is Bitcoin only: leaving Bitcoin
                    // drops a pending import. Import or restore itself stays
                    // open and re-filters for the new network.
                    self.pending_bitcoin_import = None;
                }
                self.error = None;
                // Clear stale limit from previous network
                self.server_cube_limit = None;
                let network_dir = self.datadir_path.network_directory(self.network);
                let mut tasks: Vec<Task<Message>> =
                    vec![probe_network_datadir(self.network, network_dir)];
                // Re-fetch limits for the new network if authenticated
                if let Some(client) = self.connect_account.authenticated_client() {
                    let network_str = settings::network_to_api_string(self.network);
                    tasks.push(Task::perform(
                        async move {
                            client
                                .get_cube_limits(&network_str)
                                .await
                                .map_err(|e| e.to_string())
                        },
                        Message::CubeLimitsLoaded,
                    ));
                }
                Task::batch(tasks)
            }
            Message::View(ViewMessage::ToggleDeveloperMode(enabled)) => {
                let path = GlobalSettings::path(&self.datadir_path);
                match GlobalSettings::update_developer_mode(&path, enabled) {
                    Ok(()) => {
                        self.settings_error = None;
                        Task::batch([
                            self.reload_global_settings(),
                            Task::done(Message::GlobalSettingsChanged),
                        ])
                    }
                    Err(e) => {
                        self.settings_error = Some(coincube_ui::localized_format!(
                            "Couldn't save developer mode: {e}",
                            "No se pudo guardar el modo de desarrollador: {e}"
                        ));
                        Task::none()
                    }
                }
            }
            Message::View(ViewMessage::ToggleBitcoinBlake2bBeta(enabled)) => {
                let path = GlobalSettings::path(&self.datadir_path);
                match GlobalSettings::update_bitcoin_blake2b_beta(&path, enabled) {
                    Ok(()) => {
                        self.settings_error = None;
                        Task::batch([
                            self.reload_global_settings(),
                            if enabled {
                                self.recheck_global_feature()
                            } else {
                                Task::none()
                            },
                            Task::done(Message::GlobalSettingsChanged),
                        ])
                    }
                    Err(e) => {
                        self.settings_error = Some(coincube_ui::localized_format!(
                            "Couldn't save Bitcoin Blake2b beta: {e}",
                            "No se pudo guardar la beta de Bitcoin Blake2b: {e}"
                        ));
                        Task::none()
                    }
                }
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::Deleted)) => {
                // Only delete from the Connect API if user opted in
                let should_delete_remote = self
                    .delete_cube_modal
                    .as_ref()
                    .is_some_and(|m| m.delete_connect_backup);

                let delete_task = if should_delete_remote {
                    if let Some(client) = self.connect_account.authenticated_client() {
                        self.delete_cube_modal
                            .as_ref()
                            .map(|m| m.cube.id.clone())
                            .map(|uuid| {
                                Task::perform(
                                    async move {
                                        let cubes =
                                            client.list_cubes().await.map_err(|e| e.to_string())?;
                                        let server_cube = cubes.iter().find(|c| c.uuid == uuid);
                                        if let Some(cube) = server_cube {
                                            let server_id = cube.id.to_string();
                                            client
                                                .delete_cube(&server_id)
                                                .await
                                                .map_err(|e| e.to_string())
                                        } else {
                                            Ok(())
                                        }
                                    },
                                    Message::CubeBackupDeleted,
                                )
                            })
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Close modal and reload cubes.
                self.delete_cube_modal = None;
                let reload_task = self.reload();
                // The deleted Cube may still exist on Connect — that's the whole
                // point of leaving the box unchecked — in which case it just
                // became remote-only and belongs in the restore list. `reload`
                // alone can't discover that: `Message::Checked` only *prunes*
                // `remote_cubes`, so without a re-fetch the Cube vanishes from
                // the page entirely and looks unrecoverable until the next
                // sign-in. Re-fetch after the remote delete (when there was one)
                // so the list reflects the server's post-delete state rather
                // than racing it.
                if let Some(delete_task) = delete_task {
                    Task::batch([reload_task, delete_task])
                } else {
                    Task::batch([reload_task, self.fetch_remote_cubes()])
                }
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::CloseModal)) => {
                let opener = self.delete_cube_modal.as_ref().map(|modal| CubeCatalogKey {
                    chain: modal.cube.network,
                    cube_id: modal.cube.id.clone(),
                });
                self.delete_cube_modal = None;
                if self.network == ChainId::Testnet
                    && !has_existing_wallet(&self.datadir_path, Network::Testnet)
                {
                    self.network = ChainId::Testnet4;
                }
                opener
                    .as_ref()
                    .map(|key| focus_local_delete_opener(self, key))
                    .unwrap_or_else(Task::none)
            }
            Message::Checked { for_chain, res } => match res {
                // The probe was started for a network the user has since
                // switched away from: its result describes another
                // directory. Dropping it keeps `State::Cubes::source` — and
                // the list it labels — in step with `self.network`.
                _ if for_chain != self.network => {
                    tracing::debug!(
                        "Ignoring stale datadir probe for {} (now on {})",
                        for_chain,
                        self.network
                    );
                    Task::none()
                }
                Err(e) => {
                    self.catalog.pending_select = None;
                    self.create_network_selection_pending = false;
                    self.delete_after_check = None;
                    self.error = Some(e.to_string());
                    Task::none()
                }
                Ok(mut state) if matches!(self.state, State::CreationBackup(_)) => {
                    // The backup step owns the screen and an unwritten Cube's
                    // seed. A reload that lands now (the catch-up sync after
                    // signing in from the hub, say) must not replace it; it
                    // only refreshes the list a Cancel returns to.
                    if let State::Cubes { cubes, .. } = &mut state {
                        self.creation_backup_cubes = std::mem::take(cubes);
                    }
                    Task::none()
                }
                Ok(mut state) => {
                    if self.create_network_selection_pending || self.showing_create_form() {
                        if let State::Cubes { create_cube, .. } | State::NoCube { create_cube } =
                            &mut state
                        {
                            *create_cube = true;
                        }
                        self.create_network_selection_pending = false;
                    }
                    if self.showing_import_or_restore() {
                        match &mut state {
                            State::Cubes { create_cube, .. } | State::NoCube { create_cube } => {
                                *create_cube = true
                            }
                            _ => {}
                        }
                    }
                    // Prune remote cubes that now exist locally
                    if let State::Cubes { cubes, .. } = &state {
                        let local_ids: std::collections::HashSet<&str> =
                            cubes.iter().map(|c| c.id.as_str()).collect();
                        self.remote_cubes
                            .retain(|rc| !local_ids.contains(rc.uuid.as_str()));
                    }
                    // The list this replaces keeps showing as another
                    // network's, so the floor does not lose those Cubes while
                    // they are read again from disk.
                    if let State::Cubes { cubes, source, .. } = &self.state {
                        if *source != self.network {
                            let (source, cubes) = (*source, cubes.clone());
                            match self.network_catalog.iter_mut().find(|(c, _)| *c == source) {
                                Some((_, list)) => *list = cubes,
                                None => self.network_catalog.push((source, cubes)),
                            }
                        }
                    }
                    self.state = state;
                    self.liquid_on_disk = matches!(self.state, State::Cubes { .. })
                        && !self.network.is_blake2b()
                        && crate::app::breez_liquid::local_state_exists(
                            self.datadir_path.path(),
                            self.network.bitcoin_network(),
                        );
                    // Taken before the import's own Open returns early, so
                    // a pending pick never outlives this checked list.
                    let pending = self.catalog.pending_select.take();
                    // One checked list answers the request: a Cube missing
                    // from it is not asked about by a later reload.
                    if let Some(id) = self.delete_after_check.take() {
                        if let State::Cubes { cubes, .. } = &self.state {
                            if let Some(index) = cubes.iter().position(|cube| cube.id == id) {
                                return self.update_inner(Message::View(ViewMessage::DeleteCube(
                                    DeleteCubeMessage::ShowModal(index),
                                )));
                            }
                        }
                    }
                    if let (Some(id), State::Cubes { cubes, .. }) =
                        (&self.open_created_cube, &self.state)
                    {
                        if let Some(index) = cubes.iter().position(|cube| &cube.id == id) {
                            self.open_created_cube = None;
                            return self.update_inner(Message::View(ViewMessage::Run(index)));
                        }
                    }
                    // A Cube picked on another network made that network
                    // current; now that its list is checked, select it.
                    // Open replays as `RunCatalog`, which resolves the Cube
                    // through `catalog_index` on this checked list again.
                    let select = match pending {
                        Some((key, intent)) if key.chain == self.network => {
                            Task::done(Message::View(match intent {
                                launcher_catalog::PendingIntent::Select => ViewMessage::Catalog(
                                    launcher_catalog::CatalogMessage::SelectFloor(key),
                                ),
                                launcher_catalog::PendingIntent::Open => {
                                    ViewMessage::RunCatalog(key)
                                }
                            }))
                        }
                        _ => Task::none(),
                    };
                    match self.refresh_network_catalog(true) {
                        Some(load) => Task::batch([select, load]),
                        None => select,
                    }
                }
            },
            Message::View(ViewMessage::Run(index)) => {
                if let State::Cubes { cubes, source, .. } = &self.state {
                    if let Some(cube) = cubes.get(index) {
                        let source = *source;
                        // The record was read from `source`'s directory. Its
                        // own `network` must say the same, and this build must
                        // be able to run that chain — both decided here, before
                        // any file the *record* would point at is read. A
                        // record that disagrees is a copied or hand-edited
                        // settings.json; trusting it would open the wrong
                        // chain's config, seed folder and provider.
                        if cube.network != source {
                            self.error = Some(coincube_ui::localized_format!(
                                "This Cube's settings say it lives on {}, but it was found in \
                                 the {} folder. Tenshu won't open it until the two agree.",
                                "Los ajustes de este Cube indican que está en {}, pero se encontró \
                                 en la carpeta {}. Tenshu no lo abrirá hasta que coincidan.",
                                cube.network.label(),
                                source.label(),
                            ));
                            return Task::none();
                        }
                        if let Some(reason) = self.chain_availability(cube.network).reason() {
                            self.error = Some(reason.to_string());
                            return Task::none();
                        }
                        // The GUI config of the directory the record came from —
                        // never the one a record's claim would select.
                        let datadir_path = self.datadir_path.clone();
                        let mut path = self
                            .datadir_path
                            .network_directory(source)
                            .path()
                            .to_path_buf();
                        path.push(app::config::DEFAULT_FILE_NAME);
                        // `check_network_datadir` created this file if it was
                        // missing, but the disk is not frozen between that probe
                        // and this click: surface a read failure, don't panic.
                        let cfg = match app::Config::from_file(&path) {
                            Ok(cfg) => cfg,
                            Err(e) => {
                                self.error = Some(coincube_ui::localized_format!(
                                    "Couldn't read the {} folder's configuration ({}): {}",
                                    "No se pudo leer la configuración de la carpeta {} ({}): {}",
                                    source.label(),
                                    path.display(),
                                    e
                                ));
                                return Task::none();
                            }
                        };
                        let cube = cube.clone();
                        let floor_generation = self
                            .catalog
                            .open_from_floor
                            .then_some(self.catalog.floor_open_generation);
                        Task::perform(
                            async move { (datadir_path, cfg, source, cube, floor_generation) },
                            |(root, cfg, chain, cube, generation)| match generation {
                                Some(generation) => {
                                    Message::RunFromFloor(root, cfg, chain, cube, generation)
                                }
                                None => Message::Run(root, cfg, chain, cube),
                            },
                        )
                    } else {
                        Task::none()
                    }
                } else {
                    Task::none()
                }
            }
            Message::View(ViewMessage::ToggleRecoveryCheckBox) => Task::none(),
            Message::View(ViewMessage::RecoveryWordInput { index, word }) => {
                if index < 12 {
                    let normalized = word
                        .chars()
                        .filter(|c| c.is_ascii_alphabetic())
                        .collect::<String>()
                        .to_lowercase();

                    let mut valid_prefix = String::new();
                    for ch in normalized.chars() {
                        let mut next = valid_prefix.clone();
                        next.push(ch);
                        if bip39_suggestions(&next, 1).is_empty() {
                            break;
                        }
                        valid_prefix = next;
                    }

                    self.recovery_words[index] = valid_prefix.clone();
                    self.recovery_active_index = if valid_prefix.is_empty() {
                        None
                    } else {
                        Some(index)
                    };
                    self.error = None;
                }
                Task::none()
            }
            Message::View(ViewMessage::SelectRecoverySuggestion { index, word }) => {
                if index < 12 {
                    self.recovery_words[index] = word;
                    self.recovery_active_index = None;
                    self.error = None;
                }
                Task::none()
            }
            Message::View(ViewMessage::SubmitRecovery) => {
                let words = self.recovery_words.join(" ");
                match bip39::Mnemonic::parse_in(bip39::Language::English, words) {
                    Ok(mnemonic) => {
                        log::info!("Mnemonic parsed successfully");

                        if self.creating_cube {
                            return Task::none();
                        }

                        if self.create_cube_name.value.trim().is_empty() {
                            return Task::none();
                        }

                        // Validate PIN (always required)
                        if !self.create_cube_pin.is_complete() {
                            self.error = Some("Please enter all 4 PIN digits".to_string());
                            return Task::none();
                        }
                        if !self.create_cube_pin_confirm.is_complete() {
                            self.error = Some("Please confirm all 4 PIN digits".to_string());
                            return Task::none();
                        }
                        if self.create_cube_pin.value() != self.create_cube_pin_confirm.value() {
                            self.error = Some("PIN codes do not match".to_string());
                            return Task::none();
                        }

                        self.creating_cube = true;
                        let network = self.network;
                        let cube_name = self.create_cube_name.value.trim().to_string();
                        let pin = self.create_cube_pin.value();
                        let datadir_path = self.datadir_path.clone();
                        let pending_bitcoin_import = self.pending_bitcoin_import.clone();
                        // A ghost's import takes the ghost's identity only when
                        // the phrase proves it is that Cube's seed, so Connect's
                        // registration is reclaimed rather than duplicated. The
                        // ghost's ID never enters `pending_cube_id`: that slot
                        // outlives this form (a failed write keeps it for a
                        // retry), and a later Create or import must not reuse it.
                        let ghost_id = match &self.ghost_import {
                            Some(ghost) => match self
                                .ghost_still_registered(ghost)
                                .and_then(|()| ghost_import_identity(ghost, network, &mnemonic))
                            {
                                Ok(id) => id,
                                Err(e) => {
                                    self.creating_cube = false;
                                    self.error = Some(e);
                                    return Task::none();
                                }
                            },
                            None => None,
                        };
                        // Held until the write lands: its registration must
                        // go only to the account the ghost was listed under.
                        self.ghost_claim = ghost_id.map(|id| {
                            (
                                id.to_string(),
                                self.ghost_import.as_ref().and_then(|ghost| ghost.account),
                            )
                        });
                        // Otherwise reuse the UUID that was pre-generated when
                        // the user first clicked "Create Cube" (recovery path).
                        let cube_id = match ghost_id {
                            Some(id) => id,
                            None => *self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4),
                        };

                        Task::perform(
                            async move {
                                // Same device-secret mint + refusal as the
                                // non-recovery path above.
                                let device_secret = match unlock::device_secret::capability() {
                                    unlock::device_secret::Capability::Available => Some(
                                        unlock::device_secret::get_or_create(
                                            datadir_path.path(),
                                            &cube_id.to_string(),
                                        )
                                        .map_err(|e| e.to_string())?,
                                    ),
                                    unlock::device_secret::Capability::Unavailable(why) => {
                                        return Err(why)
                                    }
                                };

                                // Restore MasterSigner from recovery mnemonic
                                let master_signer = MasterSigner::from_mnemonic(
                                    network.bitcoin_network(),
                                    mnemonic,
                                )
                                .map_err(|e| format!("Failed to restore from mnemonic: {}", e))?;

                                // Create secp context for fingerprint calculation
                                let secp =
                                    coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
                                let master_fingerprint = master_signer.fingerprint(&secp);

                                // Store master seed mnemonic (encrypted with PIN)
                                let network_dir = datadir_path.network_directory(network);
                                network_dir.init().map_err(|e| {
                                    format!("Failed to create network directory: {}", e)
                                })?;

                                // One timestamp for the seed file and the Cube
                                // — see the note on the non-recovery path (X7).
                                let timestamp = chrono::Utc::now().timestamp();
                                let master_checksum = format!("{}{}", MASTER_SEED_LABEL, timestamp);

                                // Store master seed mnemonic encrypted with PIN (always required)
                                master_signer
                                    .store_encrypted_for_chain(
                                        datadir_path.path(),
                                        network,
                                        &secp,
                                        Some((master_checksum, timestamp)),
                                        &pin,
                                        &cube_id.to_string(),
                                        device_secret.as_ref(),
                                    )
                                    .map_err(|e| {
                                        format!("Failed to store master seed mnemonic: {}", e)
                                    })?;

                                tracing::info!("Master signer created and stored (encrypted with PIN) with fingerprint: {}", master_fingerprint);

                                // Build Cube settings using the pre-generated,
                                // stable UUID. No PIN hash — see the note on
                                // the non-recovery path above.
                                let mut cube =
                                    CubeSettings::new_with_id(cube_id, cube_name, network)
                                        .with_master_signer(master_fingerprint);
                                cube.bitcoin_import = pending_bitcoin_import;
                                // Same instant the seed file carries (X7).
                                cube.created_at = timestamp;
                                // The Cube's second slot, written at creation —
                                // see the note on the non-recovery path (6b).
                                let slot_name = unlock::marker::new_file_name(timestamp);
                                match unlock::marker::write_decoy(
                                    datadir_path.path(),
                                    network,
                                    &cube_id.to_string(),
                                    &slot_name,
                                    device_secret.as_ref(),
                                ) {
                                    Ok(()) => cube.duress_slot_file = Some(slot_name),
                                    Err(e) => tracing::warn!(
                                        "could not write the Cube's second slot: {}",
                                        e
                                    ),
                                }
                                // Reaching here means the user typed all twelve
                                // words of this seed back into the app and they
                                // passed the BIP39 checksum. That *is* the
                                // backup demonstration — a stricter one than
                                // the creation flow's three-word challenge, and
                                // it is the same claim `backed_up` makes
                                // everywhere else ("the user wrote the seed
                                // phrase down and confirmed it").
                                //
                                // Recording it is what makes arming the gate
                                // below safe: a restored Cube satisfies
                                // `creation_gate::evaluate` on its own local
                                // evidence and opens immediately.
                                cube.backed_up = true;
                                cube.creation_backup_required = true;

                                // Save Cube settings to settings file.
                                // Idempotency: skip insert if UUID already exists.
                                settings::update_settings_file(&network_dir, |mut settings| {
                                    if settings.cubes.iter().any(|c| c.id == cube.id) {
                                        return Some(settings);
                                    }
                                    settings.cubes.push(cube.clone());
                                    Some(settings)
                                })
                                .await
                                .map(|_| cube)
                                .map_err(|e| e.to_string())
                            },
                            Message::CubeCreated,
                        )
                    }
                    Err(error) => {
                        // Clear recovery words on error
                        for word in &mut self.recovery_words {
                            word.clear();
                            word.shrink_to_fit();
                        }
                        self.recovery_active_index = None;
                        self.error = Some(error.to_string());
                        Task::none()
                    }
                }
            }
            Message::View(ViewMessage::CancelRecovery) => {
                // Once the write has started it runs to completion; the
                // form's Cancel is disabled until it does.
                if self.creating_cube {
                    return Task::none();
                }
                for word in &mut self.recovery_words {
                    word.clear();
                    word.shrink_to_fit();
                }
                self.recovery_active_index = None;
                self.create_cube_name = coincube_ui::component::form::Value::default();
                self.create_cube_pin = pin_input::PinInput::new();
                self.create_cube_pin_confirm = pin_input::PinInput::new();
                self.pending_bitcoin_import = None;
                self.ghost_import = None;
                self.error = None;
                self.reload()
            }

            Message::View(ViewMessage::GoToSection(section)) => {
                self.catalog.dismiss_popovers();
                // Duress launch kill-switch backstop: a deep-linked or otherwise
                // programmatic navigation to the Duress section while it's gated
                // off (the nav hides the entry, so a click can't produce this)
                // redirects to Overview — fail-closed, hidden not greyed, the
                // same shape as the Marketplace route guard. See
                // `ConnectAccountPanel::show_duress`.
                let section = if matches!(
                    section,
                    HomeSection::Connect(app::menu::ConnectSubMenu::Duress)
                ) && !self.connect_account.show_duress()
                {
                    HomeSection::Connect(app::menu::ConnectSubMenu::Overview)
                } else {
                    section
                };
                if !matches!(
                    section,
                    HomeSection::Connect(app::menu::ConnectSubMenu::Overview)
                ) {
                    self.leave_recovery_services();
                }
                // Update the account panel's active_sub when navigating to a Connect submenu
                if let HomeSection::Connect(ref sub) = section {
                    self.connect_account.active_sub = sub.clone();
                }
                self.active_section = section;
                if matches!(self.active_section, HomeSection::Settings) {
                    return self.recheck_global_feature();
                }
                if matches!(self.active_section, HomeSection::Cubes) {
                    return self.on_focus();
                }
                // If navigating to Connect and not yet initialized, trigger Init
                if matches!(self.active_section, HomeSection::Connect(_))
                    && matches!(
                        self.connect_account.step,
                        crate::app::state::connect::ConnectFlowStep::CheckingSession
                    )
                {
                    return map_connect_task(
                        self.connect_account
                            .update_message(ConnectAccountMessage::Init),
                    );
                }
                // Load Security data on demand (mirrors App::set_current_panel)
                if matches!(
                    self.active_section,
                    HomeSection::Connect(app::menu::ConnectSubMenu::Security)
                ) && self.connect_account.is_authenticated()
                {
                    return map_connect_task(
                        crate::app::state::connect::account::load_security_data(
                            &self.connect_account.client,
                            self.connect_account.session_generation(),
                        ),
                    );
                }
                // Load Overview summary counts on demand
                if matches!(
                    self.active_section,
                    HomeSection::Connect(app::menu::ConnectSubMenu::Overview)
                ) && self.connect_account.is_authenticated()
                {
                    return map_connect_task(self.connect_account.reload_overview());
                }
                // Load Contacts data on demand
                if matches!(
                    self.active_section,
                    HomeSection::Connect(app::menu::ConnectSubMenu::Contacts)
                ) && self.connect_account.is_authenticated()
                {
                    return map_connect_task(self.connect_account.reload_contacts());
                }
                // Load duress Emergency-contacts on demand (Estate Notifications).
                // `reload_duress_contacts` no-ops for non-Estate accounts, so this
                // never fires a request that would just 403.
                if matches!(
                    self.active_section,
                    HomeSection::Connect(app::menu::ConnectSubMenu::Duress)
                ) && self.connect_account.is_authenticated()
                {
                    // Load the Emergency-contacts list, the per-Cube
                    // recovery-kit checklist, and the server-side duress state
                    // (so the screen reflects the enabled state); each no-ops
                    // for accounts that lack the relevant entitlement.
                    return map_connect_task(iced::Task::batch([
                        self.connect_account.reload_duress_contacts(),
                        self.connect_account.reload_duress_cubes(),
                        self.connect_account.reload_duress_state(),
                    ]));
                }
                // Load the recoverable-vault list on demand when opening the
                // heir discovery surface (once per session — `is_loaded()`
                // guards re-fetch on every reopen).
                if matches!(self.active_section, HomeSection::RecoverVault)
                    && !self.recover_vault.is_loaded()
                {
                    let client = self.connect_account.authenticated_client();
                    let gen = self.connect_account.session_generation();
                    return self
                        .recover_vault
                        .update(RecoverVaultMessage::Load, client, gen)
                        .map(|m| Message::View(ViewMessage::RecoverVault(m)));
                }
                Task::none()
            }

            // Heir clicked "Recover" on an actionable row: launch the installer's
            // inheritance-recovery flow (COIN-377 PR 3). Intercepted here (not
            // forwarded to the panel) because launching the installer is a
            // home-level action — the heir's authenticated Connect client is
            // threaded into the installer Context so the decrypt step can fetch
            // the envelopes + build the gRPC relay client.
            Message::View(ViewMessage::RecoverVault(RecoverVaultMessage::Launch {
                cube_id,
                full_cube,
            })) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                let client = self.connect_account.authenticated_client();
                Task::perform(
                    async move { (datadir_path, network, client) },
                    move |(d, n, c)| {
                        Message::Install(
                            d,
                            n,
                            UserFlow::RecoverInheritedVault { cube_id, full_cube },
                            c,
                        )
                    },
                )
            }
            Message::View(ViewMessage::RecoverVault(msg)) => {
                // Forward to the discovery panel with the heir's authenticated
                // Connect client; map the panel's task back into home messages.
                // The session generation lets the panel drop a list fetch that
                // resolves after a logout / account switch.
                let client = self.connect_account.authenticated_client();
                let gen = self.connect_account.session_generation();
                self.recover_vault
                    .update(msg, client, gen)
                    .map(|m| Message::View(ViewMessage::RecoverVault(m)))
            }

            // Owner chose passwordless phone recovery for a Cube: launch the
            // installer's owner-keychain flow (PLAN-owner-keychain-recovery).
            // Launching the installer is a home-level action — the owner's
            // authenticated Connect client is threaded into the installer
            // Context for the decrypt step.
            Message::View(ViewMessage::RestoreWithPhone { cube_id, full_cube }) => {
                // Selecting a method closes the picker (a no-op when the launch
                // came straight from a single-method row).
                self.recovery_method_modal = None;
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                let client = self.connect_account.authenticated_client();
                Task::perform(
                    async move { (datadir_path, network, client) },
                    move |(d, n, c)| {
                        Message::Install(
                            d,
                            n,
                            UserFlow::RecoverOwnCubeWithPhone { cube_id, full_cube },
                            c,
                        )
                    },
                )
            }

            Message::View(ViewMessage::RenameCube(index)) => {
                if let State::Cubes { cubes, .. } = &self.state {
                    if let Some(cube) = cubes.get(index) {
                        self.rename_cube_modal = Some((
                            CubeCatalogKey {
                                chain: cube.network,
                                cube_id: cube.id.clone(),
                            },
                            cube.name.clone(),
                        ));
                    }
                }
                Task::none()
            }
            Message::View(ViewMessage::RenameCubeNameEdited(name)) => {
                if let Some((_, ref mut input)) = self.rename_cube_modal {
                    *input = name;
                }
                Task::none()
            }
            Message::View(ViewMessage::RenameCubeConfirm) => {
                let Some((ref key, ref new_name)) = self.rename_cube_modal else {
                    return Task::none();
                };
                let new_name = new_name.trim().to_string();
                if new_name.is_empty() {
                    return Task::none();
                }
                let Some(index) = self.catalog_index(key) else {
                    self.rename_cube_modal = None;
                    self.error = Some("This Cube is no longer in the current catalog. Review the list and try again.".into());
                    return Task::none();
                };
                let Some(cube) = (if let State::Cubes { cubes, .. } = &self.state {
                    cubes.get(index).cloned()
                } else {
                    None
                }) else {
                    return Task::none();
                };

                let network_dir = self.datadir_path.network_directory(cube.network);
                let cube_id = cube.id.clone();
                let name_for_settings = new_name.clone();

                // Stash context for remote update — will be consumed in
                // CubeRenamed handler only if the local write succeeds.
                if self.connect_account.is_authenticated() {
                    self.pending_remote_rename = Some(PendingRemoteRename {
                        cube_id: cube.id.clone(),
                        cube_network: cube.network,
                        new_name: new_name.clone(),
                    });
                }

                // Update local settings file first; remote update follows
                // in the CubeRenamed success handler.
                Task::perform(
                    async move {
                        settings::update_settings_file(&network_dir, |mut s| {
                            if let Some(c) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                                c.name = name_for_settings;
                                // Mark unsynced so catch-up can pick up the
                                // name change if the remote update fails or
                                // we're offline.
                                c.remote_synced = false;
                            }
                            Some(s)
                        })
                        .await
                        .map_err(|e| e.to_string())
                    },
                    Message::CubeRenamed,
                )
            }
            Message::View(ViewMessage::RenameCubeCancel) => {
                self.rename_cube_modal = None;
                Task::none()
            }

            Message::View(ViewMessage::ToggleConnect) => {
                self.connect_expanded = !self.connect_expanded;
                Task::none()
            }

            Message::PersistDuressEnrollment(payload) => {
                // Drop a completion that outlived its Connect session: a logout
                // or session reset bumps `session_generation`, and persisting
                // now would arm the duress PIN + DuressLocalState for an account
                // the user is no longer signed into.
                if payload.gen != self.connect_account.session_generation() {
                    log::warn!("duress: ignoring enrollment completion from a stale session");
                    return Task::none();
                }
                let app::message::DuressEnrollmentPayload {
                    duress_pin,
                    duress_code,
                    account_id,
                    ..
                } = payload;
                Task::perform(
                    app::persist_duress_enrollment(
                        self.datadir_path.clone(),
                        duress_pin,
                        duress_code,
                        account_id,
                    ),
                    |res| res,
                )
                .then(|res| match res {
                    Ok(()) => Task::batch([
                        // Reflect the enabled state only now that the duress PIN
                        // is actually armed on every Cube.
                        Task::done(Message::View(ViewMessage::ConnectAccount(
                            ConnectAccountMessage::Duress(
                                app::view::DuressMessage::EnrollmentPersisted,
                            ),
                        ))),
                        Task::done(Message::View(ViewMessage::Check)),
                    ]),
                    Err(e) => {
                        log::error!("duress: failed to persist enrollment: {e}");
                        // Surface via the Connect panel's error display.
                        Task::done(Message::View(ViewMessage::ConnectAccount(
                            ConnectAccountMessage::Error(crate::user_error::UserError::logged(
                                "Couldn't finish enabling duress mode",
                                "Try again. If it keeps failing, contact support and quote the reference below.",
                                crate::user_error::CC_API_BADRESP,
                                true,
                                e,
                            )),
                        )))
                    }
                })
            }
            Message::View(ViewMessage::ConnectAccount(
                ConnectAccountMessage::OpenRecoveryService(target),
            )) => {
                if !matches!(
                    self.active_section,
                    HomeSection::Connect(app::menu::ConnectSubMenu::Overview)
                ) || !self.connect_account.is_authenticated()
                    || !matches!(
                        &self.connect_account.recovery_services,
                        RecoveryServicesState::Ready(rows)
                            if rows.iter().filter(|row| row.target == target).count() == 1
                    )
                {
                    return Task::none();
                }
                let (Some(client), Some(user)) = (
                    self.connect_account.authenticated_client(),
                    self.connect_account.user.as_ref(),
                ) else {
                    return Task::none();
                };
                let user_id = user.id;
                let session_generation = self.connect_account.session_generation();
                self.recovery_open_request = self.recovery_open_request.wrapping_add(1);
                let request = self.recovery_open_request;
                self.connect_account.error = None;
                let datadir = self.datadir_path.clone();
                let resolve_target = target.clone();
                Task::perform(
                    async move {
                        let cubes = client
                            .list_cubes()
                            .await
                            .map_err(|error| error.to_string())?;
                        resolve_recovery_service_destination(&datadir, &resolve_target, &cubes)
                    },
                    move |result| Message::RecoveryServiceResolved {
                        target: target.clone(),
                        session_generation,
                        user_id,
                        request,
                        result,
                    },
                )
            }
            Message::RecoveryServiceResolved {
                target,
                session_generation,
                user_id,
                request,
                result,
            } => {
                if request != self.recovery_open_request
                    || session_generation != self.connect_account.session_generation()
                    || !self.connect_account.is_authenticated()
                    || self.connect_account.user.as_ref().map(|user| user.id) != Some(user_id)
                    || !matches!(
                        self.active_section,
                        HomeSection::Connect(app::menu::ConnectSubMenu::Overview)
                    )
                    || !matches!(
                        &self.connect_account.recovery_services,
                        RecoveryServicesState::Ready(rows)
                            if rows.iter().filter(|row| row.target == target).count() == 1
                    )
                {
                    return Task::none();
                }
                let destination = match result {
                    Ok(destination) => destination,
                    Err(detail) => {
                        self.connect_account.error =
                            Some(crate::app::state::connect::account::PanelError::settled(
                                crate::user_error::UserError::logged(
                                    "Couldn't open this Cube",
                                    "Refresh Recovery services, then try again.",
                                    crate::user_error::CC_API_BADRESP,
                                    false,
                                    detail,
                                ),
                            ));
                        return Task::none();
                    }
                };
                let chain = match &destination {
                    RecoveryServiceDestination::Local(chain, ..)
                    | RecoveryServiceDestination::Remote(chain) => *chain,
                };
                if !(self.developer_mode
                    || chain == ChainId::Bitcoin
                    || chain == ChainId::BitcoinBlake2b
                        && self.chain_availability(chain).is_available())
                    || !self.chain_availability(chain).is_available()
                {
                    self.connect_account.error =
                        Some(crate::app::state::connect::account::PanelError::settled(
                            crate::user_error::UserError::new(
                                "This Cube network is unavailable",
                                "Enable its network in Global Settings before opening this Cube.",
                                crate::user_error::CC_API_BADRESP,
                                false,
                            ),
                        ));
                    return Task::none();
                }
                self.connect_account.error = None;
                match destination {
                    RecoveryServiceDestination::Local(chain, cube, config) => Task::done(
                        Message::RunToRecovery(self.datadir_path.clone(), config, chain, cube),
                    ),
                    RecoveryServiceDestination::Remote(chain) => {
                        if chain.is_blake2b() {
                            self.connect_account.error = Some(
                                crate::app::state::connect::account::PanelError::settled(
                                    crate::user_error::UserError::new(
                                        "This Cube cannot be opened here",
                                        "This remote Bitcoin Blake2b Cube is not available in the local Cube catalog. Recover it on its original device.",
                                        crate::user_error::CC_API_BADRESP,
                                        false,
                                    ),
                                ),
                            );
                            return Task::none();
                        }
                        // A Connect record is not an unlock credential. Show
                        // the existing remote-only restore card in the Cube
                        // catalog; the user chooses its normal recovery flow.
                        // Refresh the catalog as its earlier account-scoped
                        // fetch may predate this newly selected server row.
                        self.active_section = HomeSection::Cubes;
                        Task::batch([
                            self.update(Message::View(ViewMessage::SelectNetwork(chain))),
                            self.fetch_remote_cubes(),
                        ])
                    }
                }
            }
            Message::View(
                view @ (ViewMessage::ConnectAccount(_) | ViewMessage::ConnectSheetAccount(_)),
            ) => {
                let sheet_message = matches!(view, ViewMessage::ConnectSheetAccount(_));
                let msg = match view {
                    ViewMessage::ConnectAccount(msg) | ViewMessage::ConnectSheetAccount(msg) => msg,
                    _ => unreachable!(),
                };
                // The banner CTAs navigate the panel to Plan & Billing by
                // setting its `active_sub`, but the sidebar highlight follows
                // the host's `active_section`. Mirror the move here so the two
                // stay in sync (otherwise the rail keeps highlighting Overview
                // while the pane shows the picker/checkout, and re-selecting
                // Overview would silently reset `active_sub`).
                if !sheet_message
                    && matches!(
                        msg,
                        ConnectAccountMessage::OpenPlanBilling
                            | ConnectAccountMessage::RenewCurrentPlan
                    )
                {
                    self.leave_recovery_services();
                    self.active_section =
                        HomeSection::Connect(app::menu::ConnectSubMenu::PlanBilling);
                }
                let explicit_logout = matches!(&msg, ConnectAccountMessage::LogOut);
                if explicit_logout {
                    self.connect_account.forget_shared_session_for_logout();
                }
                let was_authenticated = self.connect_account.is_authenticated();
                let task = map_connect_task(self.connect_account.update_message(msg));
                let now_authenticated = self.connect_account.is_authenticated();
                self.refresh_displayed_networks();
                // Update cached keyring state on login/logout transitions
                if was_authenticated != now_authenticated {
                    self.has_stored_session = now_authenticated;
                }
                // A rejected stored-session refresh dispatches `LogOut` into
                // a fresh Home that is already unauthenticated. Treat that
                // explicit invalidation like a signed-in -> signed-out edge so
                // a pending post-install Split intent cannot survive and resume
                // after a later login.
                if !now_authenticated && (was_authenticated || explicit_logout) {
                    self.server_cube_limit = None;
                    // Refusal reasons are account-scoped: they say what
                    // *that* account's server said about a Cube. The local
                    // Cube outlives the sign-out, so leaving them keyed by
                    // its id would show the previous account's refusal —
                    // rendered with the new account's tier and limits — to
                    // someone who may have slots to spare. Nothing else
                    // clears them promptly: the next catch-up sync only
                    // replaces them if it completes, and an aborted round
                    // deliberately keeps what it already has.
                    self.cube_sync_errors.clear();
                    self.remote_cubes.clear();
                    // Clearing alone isn't enough: a fetch issued for the
                    // account we just left is still in flight and would
                    // repopulate the list. Bump the generation so its
                    // result is discarded on arrival.
                    self.invalidate_remote_cubes();
                    // Reset the heir discovery surface back to Idle so the
                    // next sign-in re-fetches: otherwise its `Loaded` state
                    // (and `is_loaded()` re-fetch guard) would persist and a
                    // later account would see the prior account's vault rows.
                    self.recover_vault = RecoverVaultPanel::new();
                    // Drop any open recovery-method picker so it can't
                    // reference a prior account's remote cube.
                    self.recovery_method_modal = None;
                }
                // Auto-expand Connect submenu and navigate to Cubes after login
                if !was_authenticated && now_authenticated {
                    self.connect_expanded = true;
                    self.active_section = HomeSection::Cubes;
                }
                // Duress launch kill-switch backstop for a mid-session flip: if
                // the features refetch (or a duress-state change) that just flowed
                // through `update_message` turned the gate off while the user is
                // sitting on the Duress section, redirect to Overview. The surface
                // is hidden, not greyed, so a stale route must not keep rendering
                // it — a server flag flip propagates on the next `/connect/
                // features` poll without a restart. See `show_duress`.
                if matches!(
                    self.active_section,
                    HomeSection::Connect(app::menu::ConnectSubMenu::Duress)
                ) && !self.connect_account.show_duress()
                {
                    // Hiding the surface out from under an in-flight enrollment
                    // wizard or step-up dialog: scrub its secrets the same way
                    // Cancel would, so the flag flip can't strand duress
                    // PINs/codes on the heap once the route is unreachable.
                    self.connect_account.scrub_duress_dialogs();
                    self.active_section = HomeSection::Connect(app::menu::ConnectSubMenu::Overview);
                    self.connect_account.active_sub = app::menu::ConnectSubMenu::Overview;
                }
                // Sync account tier from the Connect plan data
                let old_tier = self.account_tier;
                self.account_tier =
                    self.connect_account
                        .plan
                        .as_ref()
                        .map_or(AccountTier::default(), |plan| match plan.tier() {
                            crate::services::coincube::PlanTier::Free => AccountTier::Free,
                            crate::services::coincube::PlanTier::Pro => AccountTier::Pro,
                            crate::services::coincube::PlanTier::Estate => AccountTier::Estate,
                        });
                // When the plan tier changes (e.g. upgrade), invalidate the
                // cached server limit so `cube_limit()` uses the new tier
                // until fresh limits are fetched.
                if old_tier != self.account_tier {
                    self.server_cube_limit = None;
                }
                if let Err(e) = GlobalSettings::update_account_tier(
                    &GlobalSettings::path(&self.datadir_path),
                    self.account_tier,
                ) {
                    log::error!("[LAUNCHER] Failed to persist account tier: {e}");
                }

                // On first login: catch-up sync unsynced cubes + fetch limits
                if !was_authenticated && now_authenticated {
                    // The startup probe may precede Connect's session restore
                    // and its local settings updates. Refresh the visible list
                    // even when every Cube is already synced (so catch-up has
                    // nothing to do). FocusedCubes preserves an in-progress form.
                    let mut tasks = vec![task, self.on_focus()];

                    // Tell the pane so it can rebroadcast the session
                    // check to every other open Cube tab — those panels
                    // hold their own ConnectAccountPanel and would
                    // otherwise stay on the "Sign in" prompt until the
                    // user clicked it or restarted the tab.
                    tasks.push(Task::done(Message::ConnectSignedInBubble));

                    // Fetch cube limits for the current network from the server
                    if let Some(limits_client) = self.connect_account.authenticated_client() {
                        let network_str = settings::network_to_api_string(self.network);
                        tasks.push(Task::perform(
                            async move {
                                limits_client
                                    .get_cube_limits(&network_str)
                                    .await
                                    .map_err(|e| e.to_string())
                            },
                            Message::CubeLimitsLoaded,
                        ));
                    }

                    // Sync all unsynced cubes across ALL networks with the API.
                    // Loads settings from each network directory, collects
                    // unsynced cubes, fetches the server cube list once, then
                    // for each unsynced local cube either registers (new) or
                    // updates (already exists but name may have changed).
                    if let Some(client) = self.connect_account.authenticated_client() {
                        let datadir = self.datadir_path.clone();
                        let owner = self.connect_owner();
                        let unsynced: Vec<CubeSettings> = locally_stored_cubes(&datadir)
                            .into_iter()
                            .filter(|cube| !cube.remote_synced)
                            .collect();
                        if !unsynced.is_empty() {
                            tasks.push(Task::perform(
                                async move {
                                    // One entry per Cube this round examines,
                                    // so the handler can say *why* a Cube is
                                    // still unsynced instead of promising a
                                    // sync that will never land — and can leave
                                    // Cubes outside this round alone.
                                    let mut outcomes: Vec<(String, Option<String>)> = Vec::new();
                                    // Fetch all server cubes once — bail if this
                                    // fails so we don't re-register everything.
                                    let server_cubes = match client.list_cubes().await {
                                        Ok(cubes) => cubes,
                                        Err(e) => {
                                            log::warn!(
                                                "[LAUNCHER] Catch-up sync aborted: \
                                                 failed to list server cubes: {}",
                                                e
                                            );
                                            // Aborting says nothing about any
                                            // individual Cube — reported as a
                                            // distinct outcome so the handler
                                            // keeps what it already knows
                                            // instead of reading an empty list
                                            // as "all clear".
                                            return Err(());
                                        }
                                    };

                                    for cube in &unsynced {
                                        let server_match =
                                            server_cubes.iter().find(|sc| sc.uuid == cube.id);
                                        let outcome = match server_match {
                                            Some(sc) => {
                                                // Already registered — re-sync if the name differs
                                                // or this device can *upgrade* the Vault flag
                                                // (PLAN-duress-vault-gate PR 3: a Vault created
                                                // while offline is reported on the next launch).
                                                //
                                                // Upgrade-only: report `hasVault=true` solely when
                                                // this device holds the Vault and the server doesn't
                                                // already show it. Never send `false` — this device
                                                // can't tell a vaultless Cube from one whose Vault
                                                // lives on another device, and a spurious `false`
                                                // would clobber a `true` that device reported and
                                                // silently unblock its duress gate.
                                                let vault_report = vault_presence_report(
                                                    cube.vault_wallet_id.is_some(),
                                                    sc.has_vault,
                                                );
                                                // Seed the server's Spark Stable Balance record
                                                // from this device's decision only while the
                                                // server has none: once it does, the live
                                                // App reconciles against it and pushes changes
                                                // as they're made, so catch-up never overrides.
                                                let stable_report = cube
                                                    .spark_stable_balance
                                                    .filter(|_| sc.spark_stable_balance.is_none());
                                                let name_drift = sc.name != cube.name;
                                                if name_drift
                                                    || vault_report.is_some()
                                                    || stable_report.is_some()
                                                {
                                                    let req = UpdateCubeRequest {
                                                        name: name_drift.then(|| cube.name.clone()),
                                                        status: None,
                                                        has_vault: vault_report,
                                                        spark_stable_balance: stable_report,
                                                    };
                                                    client
                                                        .update_cube(&sc.id.to_string(), req)
                                                        .await
                                                        .map(|_| ())
                                                        .map_err(|e| e.to_string())
                                                } else {
                                                    Ok(())
                                                }
                                            }
                                            None => {
                                                // Not registered — create. Monotonic upgrade-only:
                                                // report the Vault only when present.
                                                let req = RegisterCubeRequest {
                                                    uuid: cube.id.clone(),
                                                    name: cube.name.clone(),
                                                    network: cube.api_network_string(),
                                                    has_vault: cube
                                                        .vault_wallet_id
                                                        .is_some()
                                                        .then_some(true),
                                                    spark_stable_balance: cube.spark_stable_balance,
                                                };
                                                client
                                                    .register_cube(req)
                                                    .await
                                                    .map(|_| ())
                                                    .map_err(|e| e.to_string())
                                            }
                                        };
                                        match outcome {
                                            Ok(()) => {
                                                let nd = datadir.network_directory(cube.network);
                                                let _ = settings::mark_cube_synced(
                                                    &nd,
                                                    &cube.id,
                                                    owner.clone(),
                                                )
                                                .await;
                                                outcomes.push((cube.id.clone(), None));
                                            }
                                            Err(e) => {
                                                log::warn!(
                                                    "[LAUNCHER] Catch-up sync failed for cube \
                                                     {}: {}",
                                                    cube.id,
                                                    e
                                                );
                                                outcomes.push((cube.id.clone(), Some(e)));
                                            }
                                        }
                                    }
                                    Ok(outcomes)
                                },
                                Message::CatchUpSyncFinished,
                            ));
                        }
                    }

                    // Fetch full server cube list and compare with local cubes
                    // to identify remote-only cubes.
                    tasks.push(self.fetch_remote_cubes());

                    return Task::batch(tasks);
                }

                task
            }

            Message::View(ViewMessage::DismissAdvisoryNotice) => {
                if let Some(advisory) = self.advisory_notice.take() {
                    match GlobalSettings::mark_advisory_notice_seen(
                        &GlobalSettings::path(&self.datadir_path),
                        advisory.id,
                    ) {
                        // Re-ask rather than just clearing: a fresh install
                        // owes a notice for every advisory we ship, and
                        // deferring the second one to the next launch would
                        // let someone who opens the app rarely sit on it for
                        // weeks.
                        Ok(()) => {
                            self.advisory_notice = pending_advisory_notice(&self.datadir_path)
                        }
                        // This modal has no other way out — no blur dismiss,
                        // one button. A settings file we cannot write must
                        // therefore not re-ask, or "Got it" would reopen the
                        // same notice forever. Stay quiet for this session and
                        // let the next launch owe it again.
                        Err(e) => {
                            log::error!("[LAUNCHER] Failed to persist advisory notice: {}", e)
                        }
                    }
                }
                Task::none()
            }

            Message::View(ViewMessage::OpenUrl(url)) => {
                if let Err(e) = crate::browser::open_url(&url) {
                    log::error!("[LAUNCHER] Error opening '{}': {}", url, e);
                }
                Task::none()
            }

            _ => {
                if let Some(modal) = &mut self.delete_cube_modal {
                    return modal.update(message);
                }
                Task::none()
            }
        }
    }

    /// Drive the creation-time backup step.
    ///
    /// The payload is the Settings backup wizard's message type because the
    /// screens are the Settings wizard's screens. The variants that only make
    /// sense there are inert: `PinEntry`/`VerifyPin`/`PinVerified` exist to
    /// decrypt a seed file the user already owns, and at creation the seed has
    /// not been written yet and the PIN was typed seconds ago.
    fn update_creation_backup(&mut self, msg: BackupWalletMessage) -> Task<Message> {
        let State::CreationBackup(step) = &self.state else {
            return Task::none();
        };

        match (step, msg) {
            (CreationBackupStep::Intro(_), BackupWalletMessage::ToggleBackupIntroCheck) => {
                let State::CreationBackup(CreationBackupStep::Intro(checked)) = &self.state else {
                    return Task::none();
                };
                let flipped = !*checked;
                self.state = State::CreationBackup(CreationBackupStep::Intro(flipped));
                Task::none()
            }
            (CreationBackupStep::Intro(true), BackupWalletMessage::NextStep) => {
                self.state = State::CreationBackup(CreationBackupStep::Phrase);
                Task::none()
            }
            (CreationBackupStep::Phrase, BackupWalletMessage::NextStep) => {
                let Some(words) = self.creation_backup_words.as_ref() else {
                    return self.lose_creation_seed();
                };
                match general::generate_random_word_indices(words.len()) {
                    Some(word_indices) => {
                        self.state = State::CreationBackup(CreationBackupStep::Verification {
                            word_indices,
                            word_inputs: Default::default(),
                            error: None,
                        });
                        Task::none()
                    }
                    None => self.lose_creation_seed(),
                }
            }
            (CreationBackupStep::Phrase, BackupWalletMessage::PreviousStep) => {
                self.state = State::CreationBackup(CreationBackupStep::Intro(true));
                Task::none()
            }
            (CreationBackupStep::Verification { .. }, BackupWalletMessage::PreviousStep) => {
                self.state = State::CreationBackup(CreationBackupStep::Phrase);
                Task::none()
            }
            // "Back up now" from the bypass screen — return to the wizard with
            // the intro already acknowledged, since they read it to get here.
            (CreationBackupStep::Bypass { .. }, BackupWalletMessage::PreviousStep) => {
                self.state = State::CreationBackup(CreationBackupStep::Intro(true));
                Task::none()
            }
            // Back out of the first screen: nothing has been written, so this
            // is a plain abandonment.
            // Stepping back off the first phrase screen returns to the choice,
            // where the other two exits are.
            (CreationBackupStep::Choice, BackupWalletMessage::NextStep) => {
                if !self.creation_seed_confirmed {
                    self.state = State::CreationBackup(CreationBackupStep::Intro(false));
                }
                Task::none()
            }
            (CreationBackupStep::Intro(_), BackupWalletMessage::PreviousStep) => {
                // Back to the choice, not out of creation: the other two exits
                // are there and the gate still has to be resolved. Cancelling
                // outright is `CancelCreationBackup`, offered on that screen.
                self.state = State::CreationBackup(CreationBackupStep::Choice);
                Task::none()
            }
            (
                CreationBackupStep::Verification {
                    word_indices,
                    word_inputs,
                    ..
                },
                BackupWalletMessage::WordInput { index, input },
            ) => {
                let Some(pos) = word_indices.iter().position(|&i| i == index as usize) else {
                    return Task::none();
                };
                let mut word_inputs = word_inputs.clone();
                word_inputs[pos] = input;
                self.state = State::CreationBackup(CreationBackupStep::Verification {
                    word_indices: *word_indices,
                    word_inputs,
                    error: None,
                });
                Task::none()
            }
            (
                CreationBackupStep::Verification {
                    word_indices,
                    word_inputs,
                    ..
                },
                BackupWalletMessage::VerifyPhrase,
            ) => {
                let Some(words) = self.creation_backup_words.as_ref() else {
                    return self.lose_creation_seed();
                };
                // `word_indices` are 1-based, matching what the user is shown.
                let all_correct = word_indices.iter().enumerate().all(|(i, &word_idx)| {
                    words
                        .get(word_idx - 1)
                        .is_some_and(|expected| word_inputs[i].trim() == expected)
                });

                if all_correct {
                    // Back to the hub, which now shows the phrase as
                    // confirmed. The Cube is written by Continue there, so a
                    // Recovery Kit can still be added first.
                    self.creation_seed_confirmed = true;
                    self.state = State::CreationBackup(CreationBackupStep::Choice);
                    Task::none()
                } else {
                    self.state = State::CreationBackup(CreationBackupStep::Verification {
                        word_indices: *word_indices,
                        word_inputs: word_inputs.clone(),
                        error: Some(
                            "The words you entered don't match. Please try again.".to_string(),
                        ),
                    });
                    Task::none()
                }
            }
            _ => Task::none(),
        }
    }

    /// The seed vanished from memory between screens — only reachable via a
    /// bug. Throw the half-finished creation away rather than persisting a
    /// Cube whose seed phrase nobody has.
    fn lose_creation_seed(&mut self) -> Task<Message> {
        self.abandon_creation_backup();
        self.error = Some(
            "Lost this Cube's seed phrase before it was saved. Nothing was written to disk — \
             please start again."
                .to_string(),
        );
        Task::none()
    }

    /// Scrub the in-memory seed of the Cube being created.
    ///
    /// `Zeroizing` already wipes the `Vec`'s own buffer on drop, but not the
    /// heap allocations of the `String`s inside it — the words themselves.
    /// Clearing each one first is what actually removes the phrase from
    /// memory, matching the treatment of `recovery_words`.
    /// The signed-in Connect account, as the owner to record on a Cube this
    /// session registers. `None` while the account's profile hasn't loaded.
    fn connect_owner(&self) -> Option<settings::ConnectOwner> {
        self.connect_account
            .user
            .as_ref()
            .map(settings::ConnectOwner::from_user)
    }

    fn scrub_creation_seed(&mut self) {
        if let Some(words) = &mut self.creation_backup_words {
            for word in words.iter_mut() {
                word.clear();
                word.shrink_to_fit();
            }
        }
        self.creation_backup_words = None;
    }

    /// Enter the creation-time backup step, parking the Cube list.
    ///
    /// The step owns the whole `State`, so the list the user was looking at has
    /// to be held somewhere for [`Self::abandon_creation_backup`] to put back.
    fn enter_creation_backup(&mut self, words: Vec<String>) {
        self.creation_backup_cubes = match &mut self.state {
            State::Cubes { cubes, .. } => std::mem::take(cubes),
            _ => Vec::new(),
        };
        self.creation_backup_words = Some(zeroize::Zeroizing::new(words));
        self.reset_creation_backup_progress();
        self.state = State::CreationBackup(CreationBackupStep::Choice);
    }

    /// Continue from the backup hub: write the Cube with every backup the user
    /// finished there, the confirmed phrase, the Kit, or both. Does nothing
    /// until at least one is done; "I'll do this later" is the other way out.
    ///
    /// The two finalizers differ only in what they write — a PIN Cube gets a
    /// device secret and a sealed seed file, a passkey Cube gets neither — so
    /// the hub does not need to know which it is talking to.
    fn finish_creation_backup(&mut self) -> Task<Message> {
        if !matches!(
            self.state,
            State::CreationBackup(CreationBackupStep::Choice)
        ) || self.creating_cube
        {
            return Task::none();
        }
        let backed_up = self.creation_seed_confirmed;
        if let Some((account_id, _)) = &self.creation_kit_evidence {
            if Some(*account_id) != self.connect_account_id() {
                self.error = Some(
                    "Sign back in to the Connect account you used to create this Recovery Kit \
                     before continuing. Nothing was written."
                        .to_string(),
                );
                return Task::none();
            }
        }
        let kit = self
            .creation_kit_evidence
            .as_ref()
            .map(|(_, evidence)| evidence.clone());
        if !backed_up && kit.is_none() {
            return Task::none();
        }
        if self.creating_passkey_cube() {
            self.finalize_passkey_cube_creation(backed_up, None, kit)
        } else {
            self.finalize_cube_creation(backed_up, None, kit)
        }
    }

    /// Both passkey ceremonies land here — the native macOS one and the
    /// webview one — so there is exactly one answer to "what happens when a
    /// credential is registered", and it is testable without a real ceremony.
    ///
    /// It derives the Cube's fingerprint, drops the PRF output and the signer,
    /// keeps the phrase for the duration of the step (see
    /// [`PendingPasskeyCube`]), and hands over to the creation-backup step.
    ///
    /// **Nothing is written to disk**: that is
    /// [`Self::finalize_passkey_cube_creation`], reached once the step resolves
    /// through any of its three exits — the written phrase, a Recovery Kit, or
    /// the recorded bypass acknowledgement. The acknowledgement was the only
    /// one a passkey Cube had until both paths moved to a 12-word mnemonic and
    /// the same wizard; [`Self::default_passkey_mode`] records that change.
    fn passkey_registration_succeeded(
        &mut self,
        credential_id: String,
        prf_output: &zeroize::Zeroizing<[u8; 32]>,
    ) -> Task<Message> {
        // Derive the signer, take its fingerprint, and keep the phrase only for
        // the duration of the step.
        //
        // The seed is held because the Recovery Kit branch has to encrypt it,
        // and the credential's PRF output is available *here* and nowhere later
        // without a second Touch ID ceremony. It is the same bargain the PIN
        // path already makes with `creation_backup_words` — the same field, the
        // same `Zeroizing`, and the same `scrub_creation_seed` on every exit —
        // and unlike the PIN path this phrase is never displayed.
        let (master_fingerprint, words) = {
            let master_signer =
                match MasterSigner::from_prf_output(self.network.bitcoin_network(), prf_output) {
                    Ok(signer) => signer,
                    Err(e) => {
                        self.creating_cube = false;
                        self.error = Some(coincube_ui::localized_format!(
                            "Failed to derive master signer: {}",
                            "No se pudo derivar el firmante maestro: {}",
                            e
                        ));
                        return Task::none();
                    }
                };
            let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
            let words: Vec<String> = master_signer
                .words()
                .iter()
                .map(|w| w.to_string())
                .collect();
            (master_signer.fingerprint(&secp), words)
        };

        self.pending_passkey_cube = Some(PendingPasskeyCube {
            credential_id,
            master_fingerprint,
        });
        self.creation_backup_words = Some(zeroize::Zeroizing::new(words));
        self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);
        self.creating_cube = false;
        self.error = None;

        // Park the Cube list exactly as the PIN path does, so cancelling puts
        // the user back where they were.
        self.creation_backup_cubes = match &mut self.state {
            State::Cubes { cubes, .. } => std::mem::take(cubes),
            _ => Vec::new(),
        };
        // The same entry point a PIN Cube gets, starting from nothing done.
        // Which methods the hub offers depends on the session, not on the
        // unlock method.
        self.reset_creation_backup_progress();
        self.state = State::CreationBackup(CreationBackupStep::Choice);
        Task::none()
    }

    /// Which unlock method the Create Cube form starts on.
    ///
    /// **Passkey wherever the platform supports it.**
    ///
    /// This used to be conditional on a Connect session, because the Recovery
    /// Kit was the only backup a passkey Cube could get and the Kit needs an
    /// account. It no longer is: both creation paths derive a 12-word mnemonic
    /// and run the same wizard, so a signed-out passkey Cube can write its
    /// phrase down and leave creation with verified, offline-checkable
    /// evidence — exactly like a PIN Cube. With the exits equal, there is
    /// nothing left for the default to protect against.
    fn default_passkey_mode(&self) -> bool {
        feature_flags::PASSKEY_CREATION_AVAILABLE
    }

    /// Whether the creation flow can offer a Recovery Kit.
    ///
    /// The Kit lives on Connect: it needs an authenticated session to upload
    /// to, and a Cube registered there to hang off. Signed out, neither exists,
    /// so the exit is hidden rather than shown as a button that cannot work.
    /// The written-phrase exit is unaffected — that one needs no account.
    fn can_create_recovery_kit(&self) -> bool {
        self.connect_account.authenticated_client().is_some()
    }

    /// The recovery-password screen of the creation Kit branch.
    ///
    /// Shares the Settings Kit flow's message type and its screen, so the two
    /// places a user chooses this password apply the same rules. Variants that
    /// belong to Settings alone (PIN re-entry, protection-mode choice, removal)
    /// are inert here.
    fn update_creation_kit(&mut self, msg: RecoveryKitMessage) -> Task<Message> {
        if !matches!(self.state, State::CreationBackup(_)) {
            return Task::none();
        }
        match msg {
            RecoveryKitMessage::PasswordChanged(v) => {
                self.creation_kit_password = v;
                self.creation_kit_error = None;
                Task::none()
            }
            RecoveryKitMessage::ConfirmChanged(v) => {
                self.creation_kit_confirm = v;
                self.creation_kit_error = None;
                Task::none()
            }
            RecoveryKitMessage::AcknowledgeToggled(v) => {
                self.creation_kit_acknowledged = v;
                Task::none()
            }
            RecoveryKitMessage::Cancel => {
                // Back to the choice, not out of creation: the user still has
                // to resolve the gate one way or the other.
                self.scrub_creation_kit();
                self.state = State::CreationBackup(CreationBackupStep::Choice);
                Task::none()
            }
            RecoveryKitMessage::SubmitPassword => self.submit_creation_kit(),
            _ => Task::none(),
        }
    }

    /// Register the Cube with Connect and upload its Recovery Kit.
    ///
    /// The order is forced: `put_recovery_kit` is keyed by Connect's **numeric**
    /// cube id, which only exists once the Cube is registered. Registering
    /// before the local Cube is written is safe — the server record is name,
    /// uuid and network, and a Cube that exists only there holds no funds and
    /// cannot be opened. If the upload then fails, the local write never
    /// happens and there is no half-created Cube; the stray server record is
    /// reconciled by the same catch-up sync that already handles a Cube
    /// registered from another device.
    fn submit_creation_kit(&mut self) -> Task<Message> {
        use crate::services::recovery::MIN_PASSWORD_LEN;

        let Some(client) = self.connect_account.authenticated_client() else {
            self.creation_kit_error = Some(
                "You're signed out of Connect, so there's nowhere to save a Recovery Kit. \
                 Sign in and try again, or go back and write down the seed phrase."
                    .to_string(),
            );
            return Task::none();
        };
        let Some(account_id) = self.connect_account_id() else {
            self.creation_kit_error = Some(
                "Your Connect account is still loading. Please try again in a moment.".to_string(),
            );
            return Task::none();
        };
        let Some(words) = self.creation_backup_words.clone() else {
            // The seed is what the Kit is *for*; without it there is nothing to
            // encrypt and the Cube must not be written.
            self.error = Some(
                "This Cube's seed was lost before the Recovery Kit could be made. \
                 Nothing was written — please start again."
                    .to_string(),
            );
            self.abandon_creation_backup();
            return Task::none();
        };

        // The same three gates the Settings screen shows, re-checked here: the
        // button is only one of the two ways this message can arrive.
        let password = self.creation_kit_password.clone();
        if password.len() < MIN_PASSWORD_LEN {
            self.creation_kit_error = Some(format!(
                "Password must be at least {} characters.",
                MIN_PASSWORD_LEN
            ));
            return Task::none();
        }
        if password.as_str() != self.creation_kit_confirm.as_str() {
            self.creation_kit_error = Some("Passwords don't match.".to_string());
            return Task::none();
        }
        if !self.creation_kit_acknowledged {
            self.creation_kit_error =
                Some("Confirm you've written the recovery password down.".to_string());
            return Task::none();
        }

        let cube_id = *self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);
        let cube_name = self.create_cube_name.value.trim().to_string();
        let network = self.network;
        let api_network = settings::network_to_api_string(network);
        let created_at = chrono::Utc::now();

        self.creation_kit_error = None;
        self.creating_cube = true;
        self.state = State::CreationBackup(CreationBackupStep::KitUploading);

        Task::perform(
            async move {
                // 1. Register, so the Kit has a cube to hang off. A Cube the
                //    user already registered from another device comes back
                //    from the list rather than being created twice.
                let cube_uuid = cube_id.to_string();
                let registered = match client
                    .register_cube(RegisterCubeRequest {
                        uuid: cube_uuid.clone(),
                        name: cube_name.clone(),
                        network: api_network.clone(),
                        // A Cube being created has no Vault yet; the flag is
                        // monotonic and flips later. No Spark decision yet either.
                        has_vault: None,
                        spark_stable_balance: None,
                    })
                    .await
                {
                    Ok(resp) => Ok(resp.id),
                    Err(e) => {
                        // Registration is not idempotent, so a retry after a
                        // partial failure must find the existing record rather
                        // than give up.
                        match client.list_cubes().await {
                            Ok(cubes) => cubes
                                .into_iter()
                                .find(|c| c.uuid == cube_uuid)
                                .map(|c| c.id)
                                .ok_or_else(|| {
                                    format!("Couldn't register this Cube with Connect: {}", e)
                                }),
                            Err(_) => {
                                Err(format!("Couldn't register this Cube with Connect: {}", e))
                            }
                        }
                    }
                }?;

                // 2. Encrypt and upload — the *same* function Settings uses, so
                //    a Kit made here and a Kit made later are the same artifact.
                //    Seed only: a Cube at creation has no Vault, and
                //    `cube_backup_completeness` counts a seed-only Kit as
                //    complete for a vaultless Cube.
                let outcome = recovery_kit_upload(
                    client,
                    registered,
                    Some(words),
                    // No Vault at creation, so no descriptor half to seal.
                    None,
                    crate::services::recovery::SeedBlobCube {
                        uuid: cube_uuid,
                        name: cube_name,
                        network: api_network,
                        created_at: created_at.to_rfc3339(),
                        lightning_address: None,
                    },
                    password,
                )
                .await?;

                if !outcome.now_has_seed {
                    // Connect accepted the call but is not holding the seed —
                    // that is not a backup, and must not be recorded as one.
                    return Err("Connect didn't store this Cube's encrypted seed. \
                                Nothing was written — please try again."
                        .to_string());
                }

                Ok((
                    account_id,
                    creation_gate::CreationRecoveryKit {
                        at: created_at.timestamp(),
                        cube_id: registered,
                        has_seed: outcome.now_has_seed,
                    },
                ))
            },
            |result| Message::View(ViewMessage::CreationKitUploaded(result)),
        )
    }

    /// Wipe the recovery password and its confirmation.
    ///
    /// Called on every exit from the Kit branch — success, failure, cancel —
    /// alongside [`Self::scrub_creation_seed`]. `Zeroizing` wipes the buffer
    /// on drop; replacing the value is what makes that drop happen now rather
    /// than whenever the panel is next reused.
    fn scrub_creation_kit(&mut self) {
        self.creation_kit_password = zeroize::Zeroizing::new(String::new());
        self.creation_kit_confirm = zeroize::Zeroizing::new(String::new());
        self.creation_kit_acknowledged = false;
        self.creation_kit_error = None;
    }

    /// The native macOS ceremony hands back a raw credential id where the
    /// webview one hands back base64. That encoding is the *only* difference
    /// between the two completion paths; everything else is
    /// [`Self::passkey_registration_succeeded`].
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn passkey_registration_succeeded_raw(
        &mut self,
        credential_id: &[u8],
        prf_output: &zeroize::Zeroizing<[u8; 32]>,
    ) -> Task<Message> {
        let credential_id =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, credential_id);
        self.passkey_registration_succeeded(credential_id, prf_output)
    }

    /// Forget the backup hub's progress. Called on every way into the step
    /// (PIN and passkey) and every way out (abandon, write, failure), so one
    /// creation's confirmed phrase or Kit can never be carried into the next.
    fn reset_creation_backup_progress(&mut self) {
        self.creation_seed_confirmed = false;
        self.creation_kit_evidence = None;
    }

    /// What the backup hub has collected for the Cube being created.
    fn creation_backup_hub(&self) -> CreationBackupHub {
        CreationBackupHub {
            seed_confirmed: self.creation_seed_confirmed,
            kit_saved: self.creation_kit_evidence.is_some(),
        }
    }

    /// Whether the creation-backup step on screen belongs to a passkey Cube.
    fn creating_passkey_cube(&self) -> bool {
        self.pending_passkey_cube.is_some()
    }

    /// Phase 2 of **passkey** creation: the only place a passkey Cube is
    /// written to `settings.json`.
    ///
    /// Mirrors [`Self::finalize_cube_creation`] and arms the same gate. There is
    /// no seed file, no device secret and no PIN to write — a passkey Cube is
    /// its metadata plus a fingerprint — so the write is just the settings
    /// insert, but the *evidence* rules are identical: `backed_up` or a recorded
    /// bypass, and `creation_backup_required = true` either way.
    ///
    /// Refuses outright if none is present. That combination — armed, no
    /// evidence — is the one shape `creation_gate::evaluate_for_cube` blocks,
    /// and writing it would produce a Cube that cannot be opened.
    fn finalize_passkey_cube_creation(
        &mut self,
        backed_up: bool,
        bypass: Option<creation_gate::CreationBackupBypass>,
        recovery_kit: Option<creation_gate::CreationRecoveryKit>,
    ) -> Task<Message> {
        let Some(pending) = self.pending_passkey_cube.clone() else {
            self.error = Some(
                "This passkey Cube's registration was lost before it could be saved. \
                 Nothing was written — please start again."
                    .to_string(),
            );
            self.abandon_creation_backup();
            return Task::none();
        };
        if !backed_up && bypass.is_none() && recovery_kit.is_none() {
            // Unreachable through the UI; a hard refusal rather than a Cube
            // nobody can open if it ever becomes reachable.
            tracing::error!("refusing to persist a passkey Cube with no backup evidence");
            self.error = Some(
                "This Cube can't be created without a backup or an acknowledgement. \
                 Nothing was written."
                    .to_string(),
            );
            return Task::none();
        }

        self.creating_cube = true;
        self.error = None;
        let network = self.network;
        let cube_name = self.create_cube_name.value.trim().to_string();
        let datadir_path = self.datadir_path.clone();
        let cube_id = *self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);
        let owner = self.connect_owner();
        let pending_bitcoin_import = self.pending_bitcoin_import.clone();

        Task::perform(
            async move {
                let network_dir = datadir_path.network_directory(network);
                network_dir
                    .init()
                    .map_err(|e| format!("Failed to create network directory: {}", e))?;

                // Passkey Cube: no encrypted seed file, no PIN, no device
                // secret. The credential is the key.
                let passkey_metadata = settings::PasskeyMetadata {
                    credential_id: pending.credential_id,
                    rp_id: passkey_svc::RP_ID.to_string(),
                    created_at: chrono::Utc::now().timestamp(),
                    label: None,
                };

                let mut cube = CubeSettings::new_with_id(cube_id, cube_name, network)
                    .with_master_signer(pending.master_fingerprint)
                    .with_passkey(passkey_metadata);
                cube.bitcoin_import = pending_bitcoin_import;
                cube.backed_up = backed_up;
                cube.creation_backup_bypass = bypass;
                // The Kit was uploaded before this write, so the Cube is
                // registered with Connect by definition — recording that here
                // stops `CubeCreated` registering it a second time.
                if recovery_kit.is_some() {
                    cube.remote_synced = true;
                    cube.connect_owner = owner;
                }
                cube.creation_recovery_kit = recovery_kit;
                // Armed, like every Cube created under the gate. Without this a
                // passkey Cube reads as one that predates the gate and is waved
                // through unbacked-up forever — the audited hole.
                cube.creation_backup_required = true;

                tracing::info!(
                    "Passkey Cube created with fingerprint: {} (no seed on disk, \
                     backed_up={}, bypass={}, recovery_kit={})",
                    pending.master_fingerprint,
                    backed_up,
                    cube.creation_backup_bypass.is_some(),
                    cube.creation_recovery_kit.is_some(),
                );

                settings::update_settings_file(&network_dir, |mut settings| {
                    if settings.cubes.iter().any(|c| c.id == cube.id) {
                        return Some(settings);
                    }
                    settings.cubes.push(cube.clone());
                    Some(settings)
                })
                .await
                .map(|_| cube)
                .map_err(|e| e.to_string())
            },
            Message::CubeCreated,
        )
    }

    /// Leave the backup step without creating anything, scrubbing the seed.
    fn abandon_creation_backup(&mut self) {
        // Take the parked list *before* anything else can replace the state.
        //
        // Rebuilding `State::Cubes` with an empty vector instead made a
        // cancelled creation look like it had wiped every Cube on the device,
        // and left `total_cube_count` — the check that enforces the per-network
        // Cube limit — reading zero until the next reload.
        let cubes = std::mem::take(&mut self.creation_backup_cubes);
        // A Kit saved during this creation is on Connect under
        // `pending_cube_id`, sealing *this* seed. A retry generates a new
        // seed, so it must not reuse that UUID: the server record would then
        // hold a Kit for a seed the new Cube doesn't have. The abandoned
        // record stays on Connect as a Cube that holds no funds.
        if self.creation_kit_evidence.is_some() {
            self.pending_cube_id = None;
        }
        self.scrub_creation_seed();
        self.scrub_creation_kit();
        self.reset_creation_backup_progress();
        // A registered-but-unsaved passkey credential goes with it. The
        // credential itself stays in the platform authenticator — harmless,
        // since nothing references it — but this process must not still be
        // holding something it could turn into a Cube later.
        self.pending_passkey_cube = None;
        self.creating_cube = false;
        self.error = None;
        // Back to the create form the user came from. `pending_cube_id` is
        // kept unless a Kit was saved (above): a retry reuses the same UUID,
        // matching the existing failure path. A Cube opened later than its
        // creation would surprise the user, so a pending auto-open goes too.
        self.open_created_cube = None;
        self.open_after_registration = None;
        self.state = State::Cubes {
            cubes,
            create_cube: true,
            source: self.network,
        };
    }

    /// Phase 2 of PIN-based creation: everything that touches disk.
    ///
    /// Mints the device secret, writes the sealed seed file and inserts the
    /// Cube into `settings.json` — in that order, and only now, so that a user
    /// who walked away during the backup step left nothing behind.
    ///
    /// Exactly one of the three exits is meaningful, and all three are
    /// recorded on the Cube: `backed_up` when the user typed the challenge
    /// words back correctly, `recovery_kit` when they made a Kit, `bypass`
    /// when they accepted [`creation_gate::BYPASS_ACKNOWLEDGEMENT`] instead.
    fn finalize_cube_creation(
        &mut self,
        backed_up: bool,
        bypass: Option<creation_gate::CreationBackupBypass>,
        recovery_kit: Option<creation_gate::CreationRecoveryKit>,
    ) -> Task<Message> {
        let Some(words) = self.creation_backup_words.clone() else {
            return self.lose_creation_seed();
        };
        let mnemonic = match bip39::Mnemonic::parse_in(bip39::Language::English, words.join(" ")) {
            Ok(mnemonic) => mnemonic,
            Err(e) => {
                self.abandon_creation_backup();
                self.error = Some(format!(
                    "The generated seed phrase failed its own checksum ({}). Nothing was \
                     written to disk — please start again.",
                    e
                ));
                return Task::none();
            }
        };

        self.creating_cube = true;
        self.error = None;
        let network = self.network;
        let cube_name = self.create_cube_name.value.trim().to_string();
        let pin = self.create_cube_pin.value();
        let datadir_path = self.datadir_path.clone();
        let cube_id = *self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);
        let owner = self.connect_owner();
        let pending_bitcoin_import = self.pending_bitcoin_import.clone();

        Task::perform(
            async move {
                // Mint this Cube's device secret in the OS keystore
                // BEFORE anything is written to disk.
                //
                // The seed file is sealed under PIN + device secret
                // (`ENCRYPTED_V3`), which is what makes a copied
                // datadir useless. If the keystore is unusable — the
                // common case on headless Linux and minimal WMs —
                // this **refuses** rather than silently falling back
                // to PIN-only. A user who believes they have
                // two-factor protection and has one factor is worse
                // off than a user who was told the truth (I7).
                let device_secret = match unlock::device_secret::capability() {
                    unlock::device_secret::Capability::Available => Some(
                        unlock::device_secret::get_or_create(
                            datadir_path.path(),
                            &cube_id.to_string(),
                        )
                        .map_err(|e| e.to_string())?,
                    ),
                    unlock::device_secret::Capability::Unavailable(why) => return Err(why),
                };

                // Rebuild the signer from the phrase the user was just shown.
                // Same call the recovery path makes, so "what the user wrote
                // down" and "what gets sealed" are the same words by
                // construction.
                let master_signer =
                    MasterSigner::from_mnemonic(network.bitcoin_network(), mnemonic)
                        .map_err(|e| format!("Failed to build master seed signer: {}", e))?;

                // Create secp context for fingerprint calculation
                let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
                let master_fingerprint = master_signer.fingerprint(&secp);

                // Store master seed mnemonic (encrypted with PIN)
                let network_dir = datadir_path.network_directory(network);
                network_dir
                    .init()
                    .map_err(|e| format!("Failed to create network directory: {}", e))?;

                // **One** timestamp for the seed file and the Cube (X7).
                //
                // This used to be read here and again inside
                // `CubeSettings::new_with_id`, with the ~831 ms Argon2
                // derivation of `store_encrypted` in between. Two consequences,
                // both real:
                //
                // 1. The duress marker stamps itself with the Cube's
                //    `created_at`, so on any machine slow enough for the two
                //    reads to differ, the file whose timestamps match
                //    `settings.json` exactly is the marker. A distinguisher for
                //    free.
                // 2. `settings::derive_master_signer_fingerprint` only
                //    considers seed files within
                //    `MASTER_SEED_CREATION_WINDOW_SECS` (2) of `created_at`. A
                //    creation slower than that made the Cube's *own* seed file
                //    invisible to the backfill.
                //
                // Taking it once removes both. `cube.created_at` is overwritten
                // with this value below.
                let timestamp = chrono::Utc::now().timestamp();
                let master_checksum = format!("{}{}", MASTER_SEED_LABEL, timestamp);

                // Store master seed mnemonic encrypted with PIN
                master_signer
                    .store_encrypted_for_chain(
                        datadir_path.path(),
                        network,
                        &secp,
                        Some((master_checksum, timestamp)),
                        &pin,
                        &cube_id.to_string(),
                        device_secret.as_ref(),
                    )
                    .map_err(|e| format!("Failed to store master seed mnemonic: {}", e))?;

                tracing::info!(
                    "Master signer created and stored (encrypted with PIN) \
                     with fingerprint: {}",
                    master_fingerprint
                );

                // Build Cube settings. No PIN hash is stored: the
                // Cube's PIN is the one the seed file above was
                // encrypted under, and it is verified by decrypting
                // that file. A second, cheaper verifier next to it
                // is the bug this whole change removes (I1).
                let mut cube = CubeSettings::new_with_id(cube_id, cube_name, network)
                    .with_master_signer(master_fingerprint);
                cube.bitcoin_import = pending_bitcoin_import;
                // Same instant the seed file carries — see the note above.
                cube.created_at = timestamp;

                // The Cube's second `mnemonics/` slot, written **now** rather
                // than at duress enrolment (plan unit 6b).
                //
                // It holds a decoy until duress is armed, and arming simply
                // overwrites it. Creating it at enrolment instead would make
                // mtime the oracle the decoy exists to remove — a slot whose
                // timestamp is months newer than its Cube's seed file announces
                // the day duress was turned on. Written with the same device
                // secret as the seed file above so its wire version matches.
                //
                // Best-effort: a Cube that opens is worth more than a Cube with
                // a perfect on-disk shape, and migration backfills a missing
                // slot on the next unlock. The failure is logged, never
                // surfaced — telling the user "the duress decoy failed" on a
                // machine with no duress enrolled would leak more than the
                // missing file does.
                let slot_name = unlock::marker::new_file_name(timestamp);
                match unlock::marker::write_decoy(
                    datadir_path.path(),
                    network,
                    &cube_id.to_string(),
                    &slot_name,
                    device_secret.as_ref(),
                ) {
                    Ok(()) => cube.duress_slot_file = Some(slot_name),
                    Err(e) => tracing::warn!("could not write the Cube's second slot: {}", e),
                }
                // The outcome of the backup step above. `backed_up` is the
                // same flag Settings → Backup Master Seed sets, and it is what
                // satisfies `creation_gate::evaluate`.
                cube.backed_up = backed_up;
                cube.creation_backup_bypass = bypass;
                // A Kit could only have been uploaded against a registered
                // Cube, so recording it also means "already registered" — see
                // `finalize_passkey_cube_creation`, which does the same.
                if recovery_kit.is_some() {
                    cube.remote_synced = true;
                    cube.connect_owner = owner;
                }
                cube.creation_recovery_kit = recovery_kit;
                // Arm the gate. Reaching this line means one of the two
                // fields above is set: the user either typed the challenge
                // words back correctly or accepted
                // `BYPASS_ACKNOWLEDGEMENT`. `creation_gate::evaluate` is
                // satisfied by either, so a Cube written here always opens.
                //
                // The flag is per-Cube rather than global precisely so that
                // Cubes predating the backup step are never retroactively
                // held to it — see `cubes_that_predate_the_gate_are_never_blocked`.
                cube.creation_backup_required = true;

                // Save Cube settings
                settings::update_settings_file(&network_dir, |mut settings| {
                    if settings.cubes.iter().any(|c| c.id == cube.id) {
                        return Some(settings);
                    }
                    settings.cubes.push(cube.clone());
                    Some(settings)
                })
                .await
                .map(|_| cube)
                .map_err(|e| e.to_string())
            },
            Message::CubeCreated,
        )
    }

    fn creation_form(&self, submit_in_form: bool) -> Element<ViewMessage> {
        create_cube_form(
            &self.create_cube_name,
            &self.create_cube_pin,
            &self.create_cube_pin_confirm,
            &self.error,
            CreateCubeFormOptions {
                spark_available: crate::app::features::spark(self.network).is_available(),
                creating_cube: self.creating_cube,
                passkey_mode: self.passkey_mode,
                importing_bitcoin: self.pending_bitcoin_import.is_some(),
                // Import or restore is reachable wherever Home lets it open.
                can_import_bitcoin: !self.network.is_blake2b(),
                submit_in_form,
            },
        )
    }

    /// Return the ID of the Connect control actually mounted by Home.
    pub(crate) fn connect_sheet_origin_id(&self) -> Id {
        // Explicit creation takes precedence over the catalog. The Create
        // sheet's signed-out notice carries the sign-in ID; the empty
        // Launcher uses the catalog IDs below.
        if matches!(
            self.state,
            State::CreationBackup(CreationBackupStep::Choice)
        ) && self.showing_create_journey()
        {
            return Id::new(if self.can_create_recovery_kit() {
                CREATE_BACKUP_KIT_ID
            } else {
                CREATE_BACKUP_SIGN_IN_ID
            });
        }
        if self.showing_create_journey() {
            return Id::new(if self.connect_account.is_authenticated() {
                LEGACY_CONNECT_ENTRY_ID
            } else {
                LEGACY_CONNECT_SIGN_IN_ID
            });
        }
        // The Launcher's control keeps one id in every state (L6).
        if launcher_catalog::catalog_visible(self) {
            return launcher_catalog::connect_id();
        }
        if !self.connect_account.is_authenticated() {
            return Id::new(LEGACY_CONNECT_SIGN_IN_ID);
        }
        Id::new(LEGACY_CONNECT_ENTRY_ID)
    }

    /// Replace a saved Connect control only when it belongs to the Launcher.
    /// Other sheet triggers retain their exact return target.
    pub(crate) fn connect_sheet_return_id(&self, saved: &Id) -> Option<Id> {
        if *saved == launcher_catalog::connect_id()
            || *saved == launcher_catalog::sign_in_id()
            || *saved == Id::new(LEGACY_CONNECT_ENTRY_ID)
            || *saved == Id::new(LEGACY_CONNECT_SIGN_IN_ID)
            || *saved == Id::new(CREATE_BACKUP_SIGN_IN_ID)
        {
            // Both Launcher modes make the control inert while the session is
            // first checked, so there is no control to focus then. A stored
            // session waiting to be re-checked keeps it pressable.
            (!self.connect_account.is_checking_session()
                || self.connect_account.unverified.is_some())
            .then(|| self.connect_sheet_origin_id())
        } else {
            Some(saved.clone())
        }
    }

    #[cfg(test)]
    pub fn view(&self) -> Element<Message> {
        self.view_with_worldview(Default::default())
    }

    pub(crate) fn view_with_worldview(
        &self,
        worldview: crate::app::settings::preferences::WorldviewMode,
    ) -> Element<Message> {
        if let Some(context) = &self.native_return {
            return Container::new(
                Column::new()
                    .spacing(16)
                    .push(h3(context.chain.label()))
                    .push(text(coincube_ui::locale::notice(context.notice.clone())))
                    .push(
                        button::secondary(None, localized_copy("Back to Cube list"))
                            .on_press(Message::View(ViewMessage::DismissNativeReturn)),
                    ),
            )
            .padding(30)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into();
        }
        // The Launcher and Create journey own their chrome and body scrolling.
        // Other Home sections keep the legacy frame.
        let layout: Element<Message> = if self.showing_create_journey() {
            self.create_journey_view_with_worldview(worldview)
        } else if launcher_catalog::catalog_visible(self) {
            launcher_catalog::launcher_view_with_worldview(self, worldview).map(Message::View)
        } else {
            self.legacy_frame()
        };

        // No test-network banner here: the Launcher lists Cubes from every
        // network, and each Cube's own screens show the banner for its network.
        // If passkey ceremony webview is active, overlay it on top
        let layout = if let Some(ceremony) = &self.passkey_ceremony {
            if let Some(active) = &ceremony.active_webview {
                let cancel_btn = button::secondary(None, localized_copy("Cancel"))
                    .on_press(Message::CancelPasskeyCeremony)
                    .width(Length::Fixed(150.0));

                let webview_modal = Container::new(
                    Column::new()
                        .spacing(15)
                        .align_x(Alignment::Center)
                        .push(h4_bold(localized_copy("Passkey Registration")))
                        .push(
                            p1_regular(localized_copy(
                                "Complete the passkey setup in the window below.",
                            ))
                            .style(theme::text::secondary),
                        )
                        .push(active.view(Length::Fixed(500.0), Length::Fixed(400.0)))
                        .push(cancel_btn)
                        .width(550),
                )
                .padding(20)
                .style(theme::card::modal);

                Modal::new(Container::new(layout).height(Length::Fill), webview_modal)
                    .on_blur(Some(Message::CancelPasskeyCeremony))
                    .into()
            } else {
                layout
            }
        } else {
            layout
        };

        // Native macOS passkey ceremony status modal
        #[cfg(target_os = "macos")]
        let layout = if self.native_passkey_ceremony.is_some() {
            let cancel_btn = button::secondary(None, localized_copy("Cancel"))
                .on_press(Message::CancelPasskeyCeremony)
                .width(Length::Fixed(150.0));

            let status_modal = Container::new(
                Column::new()
                    .spacing(20)
                    .align_x(Alignment::Center)
                    .push(h4_bold(localized_copy("Passkey Registration")))
                    .push(
                        p1_regular(localized_copy(
                            "Authenticate with Touch ID to create your passkey.\n\
                             Look for the system prompt.",
                        ))
                        .style(theme::text::secondary),
                    )
                    .push(cancel_btn)
                    .width(450),
            )
            .padding(30)
            .style(theme::card::modal);

            Modal::new(Container::new(layout).height(Length::Fill), status_modal)
                .on_blur(Some(Message::CancelPasskeyCeremony))
                .into()
        } else {
            layout
        };

        if let Some(modal) = &self.delete_cube_modal {
            Modal::new(Container::new(layout).height(Length::Fill), modal.view())
                .on_blur(Some(Message::View(ViewMessage::DeleteCube(
                    DeleteCubeMessage::CloseModal,
                ))))
                .into()
        } else if let Some(modal) = &self.delete_remote_cube_modal {
            Modal::new(Container::new(layout).height(Length::Fill), modal.view())
                .on_blur(if modal.deleting {
                    None
                } else {
                    Some(Message::View(ViewMessage::DeleteCube(
                        DeleteCubeMessage::CloseRemoteModal,
                    )))
                })
                .into()
        } else if let Some(modal) = &self.recovery_method_modal {
            Modal::new(Container::new(layout).height(Length::Fill), modal.view())
                .on_blur(Some(Message::View(ViewMessage::CloseRecoveryMethodPicker)))
                .into()
        } else if let Some((_, ref name_input)) = self.rename_cube_modal {
            use coincube_ui::widget::TextInput;
            let modal_content = Container::new(
                Column::new()
                    .spacing(15)
                    .padding(20)
                    .width(Length::Fixed(400.0))
                    .push(h4_bold("Rename Cube"))
                    .push(
                        TextInput::new("Cube Name", name_input)
                            .on_input(|s| Message::View(ViewMessage::RenameCubeNameEdited(s)))
                            .padding(10)
                            .size(16),
                    )
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(
                                button::secondary(
                                    Some(icon::cross_icon()),
                                    localized_copy("Cancel"),
                                )
                                .on_press(Message::View(ViewMessage::RenameCubeCancel))
                                .width(Length::Fill),
                            )
                            .push(if name_input.trim().is_empty() {
                                button::primary(Some(icon::check_icon()), localized_copy("Save"))
                                    .width(Length::Fill)
                            } else {
                                button::primary(Some(icon::check_icon()), localized_copy("Save"))
                                    .on_press(Message::View(ViewMessage::RenameCubeConfirm))
                                    .width(Length::Fill)
                            }),
                    ),
            )
            .style(theme::card::simple);
            Modal::new(Container::new(layout).height(Length::Fill), modal_content)
                .on_blur(Some(Message::View(ViewMessage::RenameCubeCancel)))
                .into()
        } else if let Some(advisory) = self.advisory_notice {
            // Last in the chain: every other modal is something the user just
            // asked for, and this one waits its turn. No `on_blur` — it is
            // dismissed by the button, once, and then never again.
            Modal::new(
                Container::new(layout).height(Length::Fill),
                advisory_notice_modal(advisory),
            )
            .into()
        } else {
            layout
        }
    }

    #[cfg(test)]
    fn create_journey_view(&self) -> Element<Message> {
        self.create_journey_view_with_worldview(Default::default())
    }

    /// Create-only presentation around the existing network, Cube and backup
    /// state machines. The controls below dispatch their original messages;
    /// the journey does not hold a second copy of any wallet material.
    fn create_journey_view_with_worldview(
        &self,
        worldview: crate::app::settings::preferences::WorldviewMode,
    ) -> Element<Message> {
        // Presented like Connect and App preferences: a sheet over the
        // inert, dimmed Launcher, its top bar left in view. The backdrop
        // drops the dock, which the sheet would otherwise half cover.
        crate::gui::compose_overlay(
            launcher_catalog::launcher_backdrop(self, worldview).map(Message::View),
            self.create_journey_frame().map(Message::View),
            true,
        )
    }

    /// The journey before it is mapped into Home's messages, so tests can
    /// press its controls as the user does. It is the sheet only; `view`
    /// lays it over the dimmed Launcher.
    fn create_journey_frame(&self) -> Element<ViewMessage> {
        let backing_up = matches!(self.state, State::CreationBackup(_));
        // Import or restore (P-Restore) shares this frame.
        let restoring = self.showing_import_or_restore();
        let hub = restoring && self.restore_hub && !backing_up;
        let light = matches!(
            self.theme_mode,
            coincube_ui::theme::palette::ThemeMode::Light
        );
        // The import path after the Cube: the imported-wallet screens inside
        // it (Source, Review, Scan) finish the import.
        // Recover with phone's first step, its Cube, is drawn here; the
        // installer draws the rest of the path.
        let phone = hub && self.restore_phone;
        let steps: &[&str] = if phone {
            crate::installer::RestorePath::Phone.steps()
        } else if restoring {
            restore_frame::import_steps()
        } else {
            create_cube_steps(self.passkey_mode)
        };
        // Import sets a PIN only, so its Cube step has no Passkey after it.
        let passkey_step = self.passkey_mode && !restoring;
        let active_step = match (backing_up, passkey_step) {
            (true, true) => 2,
            (true, false) => 1,
            (false, true) if self.creating_cube => 1,
            (false, _) => 0,
        };

        // The network is chosen in the header, and only before the Cube is
        // being written; after that the chip is static.
        let menu_open = self.create_network_menu_editable() && self.create_network_menu;
        let network = restore_frame::network_control(
            self.network,
            light,
            self.create_network_menu_editable()
                .then_some(ViewMessage::CreateNetworkMenu(!self.create_network_menu)),
            menu_open,
            create_journey_focus,
        );
        let title = if restoring {
            localized_copy("Import or restore")
        } else {
            localized_copy("Create Cube")
        };
        let header = restore_frame::sheet_header(
            title,
            network,
            restore_frame::close_button(
                if restoring {
                    RESTORE_CLOSE_ID
                } else {
                    CREATE_CLOSE_ID
                },
                self.can_leave_create_journey()
                    .then_some(ViewMessage::ShowCreateCube(false)),
                create_journey_focus,
            ),
        );

        let body: Element<ViewMessage> = if hub {
            if self.restore_phone {
                self.restore_phone_view()
            } else {
                self.restore_hub_view()
            }
        } else if let State::CreationBackup(step) = &self.state {
            // Wide enough for the 4×150 word grid and its gutters.
            Container::new(
                Column::new()
                    .spacing(24)
                    .push(
                        self.error
                            .as_ref()
                            .map(|e| card::simple(text(coincube_ui::locale::notice(e.clone())))),
                    )
                    .push(creation_backup_view(
                        step,
                        self.creation_backup_words.as_ref().map(|w| w.as_slice()),
                        self.creating_cube,
                        self.creating_passkey_cube(),
                        &self.creation_kit_password,
                        &self.creation_kit_confirm,
                        self.creation_kit_acknowledged,
                        self.creation_kit_error.as_deref(),
                        self.can_create_recovery_kit(),
                        self.creation_backup_hub(),
                    )),
            )
            .max_width(760)
            .into()
        } else {
            let form = self.creation_form(false);
            if restoring {
                form
            } else if let Some(line) = coincube_ui::worldview_copy::VoiceLine::CreateCube.get() {
                Column::new()
                    .spacing(12)
                    .push(p2_regular(line).style(theme::text::secondary))
                    .push(form)
                    .into()
            } else {
                form
            }
        };
        let body: Element<ViewMessage> =
            // Every step's column is centred in the sheet, so a narrow form
            // does not leave a dead gutter on its right.
            scrollable(Container::new(body).padding(28).center_x(Length::Fill))
                .id(Id::new(CREATE_BODY_SCROLL_ID))
                .height(Length::Fill)
                .into();

        // The Overview has no steps. The backup hub has a footer; the
        // method screens it opens carry their own actions.
        let rail = (!hub || phone).then(|| restore_frame::step_rail(steps, active_step));
        let on_backup_hub = matches!(
            self.state,
            State::CreationBackup(CreationBackupStep::Choice)
        );
        let footer: Option<Element<ViewMessage>> = if on_backup_hub {
            Some(creation_choice_footer(
                self.creating_cube,
                self.creation_backup_hub(),
            ))
        } else {
            (!hub && !backing_up).then(|| {
                Row::new()
                    .push(Space::new().width(Length::Fill))
                    .push(
                        button::primary_action(
                            "tenshu.create:cube-submit",
                            None,
                            if self.creating_cube {
                                "Creating…"
                            } else {
                                "Create Cube"
                            },
                            create_journey_focus,
                        )
                        .width(Length::Shrink)
                        .on_press_maybe(
                            create_cube_can_submit(
                                &self.create_cube_name,
                                &self.create_cube_pin,
                                &self.create_cube_pin_confirm,
                                self.passkey_mode,
                                self.creating_cube,
                            )
                            .then_some(ViewMessage::CreateCube),
                        ),
                    )
                    .into()
            })
        };
        let region = restore_frame::region(rail, body, footer);
        // Import or restore: its path list beside the path.
        let content = if restoring {
            restore_frame::with_nav(self.restore_nav(backing_up), region)
        } else {
            region
        };
        let menu = menu_open.then(|| {
            (
                restore_frame::network_menu(title, self.create_network_menu_rows(light)),
                ViewMessage::CreateNetworkMenu(false),
            )
        });
        restore_frame::sheet(header, content, menu)
    }

    /// The header's network menu is open to changes on Create's first step
    /// and on Import or restore's Overview and Cube steps, where a change
    /// re-filters; not once a Cube is being made or an import is under way.
    fn create_network_menu_editable(&self) -> bool {
        self.pending_bitcoin_import.is_none()
            && !self.creating_cube
            && !matches!(self.state, State::CreationBackup(_))
    }

    /// The network menu's rows, as the Network step listed them: Live
    /// networks, then Test networks while Developer mode is on.
    fn create_network_menu_rows(&self, light: bool) -> Element<ViewMessage> {
        let network_row = |chain: ChainId| -> Element<ViewMessage> {
            let available = self.chain_availability(chain).is_available();
            let selected = chain == self.network;
            let subtitle = if chain.is_blake2b() && !available {
                localized_copy("Bitcoin Blake2b availability could not be verified. Vault creation is disabled.")
            } else {
                match chain {
                    ChainId::Bitcoin => "Spark · Vault",
                    ChainId::Regtest => "Spark · Vault",
                    ChainId::BitcoinBlake2b => {
                        localized_copy("Vault only. Spark isn't available on Bitcoin Blake2b.")
                    }
                    ChainId::BitcoinBlake2bTestnet4 => localized_copy(
                        "Vault only. Spark isn't available on Bitcoin Blake2b Testnet4.",
                    ),
                    ChainId::Testnet => {
                        localized_copy("Vault only. Spark isn't available on Testnet.")
                    }
                    ChainId::Testnet4 => {
                        localized_copy("Vault only. Spark isn't available on Testnet4.")
                    }
                    ChainId::Signet => {
                        localized_copy("Vault only. Spark isn't available on Signet.")
                    }
                }
            };
            let id = create_network_row_id(chain);
            let mut name = Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(create_journey_coin(chain, light, 18.0))
                .push(p2_regular(chain.label()));
            if chain.is_blake2b() && !launcher_coin::is_test_network(chain) {
                name = name.push(create_journey_tag("Blake2b", true));
            }
            if launcher_coin::is_test_network(chain) {
                name = name.push(create_journey_tag(chain.label(), false));
            }
            let row = FocusableAction::new(
                id.clone(),
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(
                        Container::new(selected.then(icon::check_icon)).width(Length::Fixed(16.0)),
                    )
                    .push(
                        Column::new()
                            .spacing(3)
                            .push(name)
                            .push(p2_regular(subtitle).size(12).style(theme::text::secondary)),
                    ),
                create_journey_focus,
            )
            .width(Length::Fill)
            .padding([8, 10])
            .style(move |theme, status| {
                // A selected option stays at normal contrast even though
                // activating it only closes the menu.
                let mut style = theme::button::transparent(
                    theme,
                    if selected {
                        iced::widget::button::Status::Active
                    } else {
                        status
                    },
                );
                if selected {
                    style.background = Some(iced::Background::Color(theme.colors.shell.selected));
                    style.text_color = theme.colors.shell.ink;
                }
                style.border.radius = 6.0.into();
                style
            });
            row.on_press_maybe(create_network_row_action(chain, available, selected))
                .into()
        };
        let mut live = self
            .displayed_networks
            .iter()
            .copied()
            .filter(|chain| matches!(chain, ChainId::Bitcoin | ChainId::BitcoinBlake2b))
            .collect::<Vec<_>>();
        let mut test = self
            .displayed_networks
            .iter()
            .copied()
            .filter(|chain| !matches!(chain, ChainId::Bitcoin | ChainId::BitcoinBlake2b))
            .collect::<Vec<_>>();
        // The per-device toggle controls visibility. An enabled device may
        // still be waiting for feature verification: explain that state
        // with disabled rows, while SelectNetwork remains fail-closed.
        if self.connect_account.bitcoin_blake2b_opt_in {
            if !live.contains(&ChainId::BitcoinBlake2b) {
                live.push(ChainId::BitcoinBlake2b);
            }
            if self.developer_mode && !test.contains(&ChainId::BitcoinBlake2bTestnet4) {
                test.push(ChainId::BitcoinBlake2bTestnet4);
            }
        }
        let mut rows = Column::new()
            .spacing(2)
            .push(restore_frame::menu_heading("Live networks"));
        for chain in live {
            rows = rows.push(network_row(chain));
        }
        if self.developer_mode {
            rows = rows.push(restore_frame::menu_heading("Test networks"));
            for chain in test {
                rows = rows.push(network_row(chain));
            }
        } else {
            rows = rows.push(
                Container::new(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .push(
                            p2_regular(localized_copy(
                                "Test networks appear when Developer mode is on.",
                            ))
                            .size(12)
                            .style(theme::text::secondary)
                            .width(Length::Fill),
                        )
                        .push(
                            button::secondary_action(
                                "tenshu.create:app-preferences",
                                None,
                                coincube_ui::locale::copy("App preferences"),
                                create_journey_focus,
                            )
                            .width(Length::Shrink)
                            .on_press(ViewMessage::OpenAppPreferences),
                        ),
                )
                .padding([8, 10]),
            );
        }
        rows.into()
    }

    /// Import Bitcoin wallet is open while Bitcoin mainnet is current and
    /// has a free Cube slot on the plan.
    fn can_start_bitcoin_import(&self) -> bool {
        self.network == ChainId::Bitcoin && !launcher_catalog::project(self).at_limit()
    }

    /// Why Import Bitcoin wallet is closed on this network, if it is. The
    /// design opens it on Bitcoin and Bitcoin Blake2b; this build's import
    /// reads Bitcoin descriptors only, so Blake2b says so.
    fn bitcoin_import_network_reason(&self) -> Option<&'static str> {
        match self.network {
            ChainId::Bitcoin => None,
            ChainId::BitcoinBlake2b => Some("Not yet on Bitcoin Blake2b in this build."),
            _ => Some("Bitcoin and Bitcoin Blake2b only."),
        }
    }

    /// Recovery Kit and phone restores open Cubes saved on Connect, so they
    /// need a session.
    fn connect_restore_reason(&self) -> Option<&'static str> {
        if !self.connect_account.is_authenticated() {
            Some("Sign in to Connect")
        } else {
            None
        }
    }

    /// Recover a Vault, where this build turns it on.
    fn recover_vault_offered(&self) -> bool {
        self.connect_account.is_authenticated()
            && !self.network.is_blake2b()
            && feature_flags::RECOVER_VAULT_ENABLED
    }

    /// Connect Cubes on the current network a restore path can open:
    /// every one for a Recovery Kit, phone-recoverable ones for the phone.
    fn restorable_here(&self, phone: bool) -> usize {
        let network = settings::network_to_api_string(self.network);
        self.remote_cubes
            .iter()
            .filter(|cube| cube.network == network)
            .filter(|cube| !phone || (cube.phone_recoverable && cube.phone_full_cube))
            .count()
    }

    /// Import or restore's path list (P-Restore's `nav`). A Cube being
    /// created or backed up keeps the user on its path. A path closed on
    /// this network or without Connect says why under its name.
    fn restore_nav(&self, backing_up: bool) -> Element<ViewMessage> {
        let locked = backing_up || self.creating_cube;
        let item = |id: &'static str,
                    label: &'static str,
                    sub: Option<String>,
                    current: bool,
                    message: ViewMessage| {
            let open = sub.is_none();
            let content = Column::new()
                .spacing(2)
                .push(p2_regular(label).style(move |theme| {
                    if open || current {
                        theme::text::primary(theme)
                    } else {
                        theme::text::secondary(theme)
                    }
                }))
                .push_maybe(sub.map(|sub| p2_regular(sub).size(12).style(theme::text::secondary)));
            FocusableAction::new(Id::new(id), content, create_journey_focus)
                .width(Length::Fill)
                .padding([10, 12])
                .style(restore_frame::nav_style(current))
                .on_press_maybe((!locked && !current && open).then_some(message))
        };
        let importing = self.pending_bitcoin_import.is_some();
        let connect_sub = self.connect_restore_reason().map(str::to_owned);
        let mut nav = restore_frame::nav()
            .push(item(
                RESTORE_OVERVIEW_ID,
                "Overview",
                None,
                self.restore_hub && !importing && !self.restore_phone,
                ViewMessage::ShowRestoreHub,
            ))
            .push(
                item(
                    RESTORE_IMPORT_ID,
                    "Import Bitcoin wallet",
                    (!importing && self.network != ChainId::Bitcoin)
                        .then(|| format!("Not on {}", self.network.label())),
                    importing,
                    ViewMessage::ShowImportBitcoin,
                )
                // A full plan closes it too, without a line under it: the
                // Overview says why.
                .on_press_maybe(
                    (!locked && !importing && self.can_start_bitcoin_import())
                        .then_some(ViewMessage::ShowImportBitcoin),
                ),
            );
        nav = nav.push(item(
            RESTORE_KIT_ID,
            "Recovery Kit",
            connect_sub.clone(),
            false,
            ViewMessage::RestoreFromAnyRecoveryKit,
        ));
        if feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED {
            nav = nav.push(item(
                RESTORE_PHONE_ID,
                "Recover with phone",
                connect_sub,
                self.restore_phone && !importing,
                ViewMessage::ShowRestorePhone,
            ));
        }
        if self.recover_vault_offered() {
            nav = nav.push(item(
                RESTORE_RECOVER_VAULT_ID,
                "Recover a Vault",
                None,
                false,
                ViewMessage::GoToSection(HomeSection::RecoverVault),
            ));
        }
        nav.into()
    }

    /// Import or restore's Overview (P-Restore's hub): every way to bring a
    /// wallet or Cube in, on the network the header shows.
    fn restore_hub_view(&self) -> Element<ViewMessage> {
        let rule = || -> Element<ViewMessage> { restore_frame::full_rule() };
        let sign_in = || -> Element<ViewMessage> {
            button::secondary_action(
                RESTORE_SIGN_IN_ID,
                Some(icon::person_icon()),
                coincube_ui::locale::copy("Sign in to Connect"),
                create_journey_focus,
            )
            .width(Length::Shrink)
            .on_press_maybe((!self.connect_account.is_checking_session()).then_some(
                ViewMessage::GoToSection(HomeSection::Connect(app::menu::ConnectSubMenu::Overview)),
            ))
            .into()
        };
        // A closed path is greyed, with its reason (and remedy) under it.
        let path = |title: &'static str,
                    line: &'static str,
                    why: Option<Element<'static, ViewMessage>>,
                    tag: Element<'static, ViewMessage>,
                    start: Option<ViewMessage>,
                    start_id: &'static str|
         -> Element<ViewMessage> {
            let open = start.is_some();
            let muted = move |theme: &theme::Theme| {
                if open {
                    theme::text::primary(theme)
                } else {
                    theme::text::secondary(theme)
                }
            };
            Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .padding([14, 0])
                .push(
                    Column::new()
                        .spacing(3)
                        .width(Length::Fill)
                        .push(p1_regular(title).style(muted))
                        .push(p2_regular(line).style(theme::text::secondary))
                        .push_maybe(why),
                )
                .push(tag)
                .push(
                    button::secondary_action(
                        start_id,
                        None,
                        coincube_ui::locale::copy("Start"),
                        create_journey_focus,
                    )
                    .width(Length::Shrink)
                    .on_press_maybe(start),
                )
                .into()
        };
        let pill = |label: String, ok: bool| -> Element<'static, ViewMessage> {
            Container::new(
                p2_regular(label)
                    .size(12)
                    .style(move |theme: &theme::Theme| iced::widget::text::Style {
                        color: Some(if ok {
                            theme.colors.shell.good
                        } else {
                            theme.colors.shell.muted
                        }),
                    }),
            )
            .padding([2, 8])
            .style(move |theme: &theme::Theme| iced::widget::container::Style {
                border: iced::Border {
                    color: if ok {
                        theme.colors.shell.good
                    } else {
                        theme.colors.shell.line
                    },
                    width: 1.0,
                    radius: 10.0.into(),
                },
                ..Default::default()
            })
            .into()
        };
        // Counts only Connect Cubes missing from this device, so say so:
        // a bare "N Cubes" reads as the whole account.
        let connected = |phone: bool| -> Element<'static, ViewMessage> {
            let count = self.restorable_here(phone);
            pill(
                format!(
                    "Connected · {count} {} not on this device",
                    if count == 1 { "Cube" } else { "Cubes" },
                ),
                true,
            )
        };
        let projection = launcher_catalog::project(self);
        let import_why: Option<Element<ViewMessage>> =
            self.bitcoin_import_network_reason().map(|why| {
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(p2_regular(why).size(12).style(theme::text::warning))
                    .push(
                        FocusableAction::new(
                            Id::new(RESTORE_SWITCH_TO_BITCOIN_ID),
                            p2_regular(localized_copy("Switch to Bitcoin")).size(12),
                            create_journey_focus,
                        )
                        .padding(0)
                        .style(theme::button::link)
                        .on_press(ViewMessage::SelectNetwork(ChainId::Bitcoin)),
                    )
                    .into()
            });
        let import_tag = if self.bitcoin_import_network_reason().is_some() {
            pill(format!("Not on {}", self.network.label()), false)
        } else {
            pill("No sign-in".to_string(), false)
        };
        let mut paths = Column::new().push(rule()).push(path(
            "Import Bitcoin wallet",
            "A recovery phrase, descriptor or hardware wallet, as a new Cube's Vault.",
            import_why,
            import_tag,
            self.can_start_bitcoin_import()
                .then_some(ViewMessage::ShowImportBitcoin),
            RESTORE_START_IMPORT_ID,
        ));
        let signed_in = self.connect_account.is_authenticated();
        let connect_tag = |phone: bool| -> Element<'static, ViewMessage> {
            if signed_in {
                connected(phone)
            } else {
                sign_in()
            }
        };
        let connect_open = self.connect_restore_reason().is_none();
        paths = paths.push(rule()).push(path(
            "Restore from Recovery Kit",
            "Your Recovery Kit from Connect and its recovery password.",
            None,
            connect_tag(false),
            connect_open.then_some(ViewMessage::RestoreFromAnyRecoveryKit),
            RESTORE_START_KIT_ID,
        ));
        if feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED {
            paths = paths.push(rule()).push(path(
                "Recover with phone",
                "Approve on the phone that holds the Cube's backup.",
                None,
                connect_tag(true),
                connect_open.then_some(ViewMessage::ShowRestorePhone),
                RESTORE_START_PHONE_ID,
            ));
        }
        if self.recover_vault_offered() {
            paths = paths.push(rule()).push(path(
                "Recover a Vault",
                "Vaults you're a keyholder for.",
                None,
                pill("Connect".to_string(), true),
                Some(ViewMessage::GoToSection(HomeSection::RecoverVault)),
                RESTORE_START_RECOVER_VAULT_ID,
            ));
        }
        let mut content = Column::new()
            .spacing(16)
            .max_width(760)
            .push(
                Column::new()
                    .spacing(4)
                    .push(h4_bold(localized_copy("Choose what to bring in")))
                    .push(
                        p2_regular(localized_format!(
                            "Into a new Cube on {}.",
                            "En un nuevo Cube en {}.",
                            self.network.label()
                        ))
                        .style(theme::text::secondary),
                    ),
            )
            .push(paths.push(rule()));
        if projection.at_limit() {
            content = content.push(
                p2_regular(if projection.total_count > projection.cube_limit {
                    localized_format!(
                        "{}: {} Cubes; the {} plan includes {}. Existing Cubes keep working. Upgrade, or delete a Cube on this network, to bring one in.",
                        "{}: {} Cubes; el plan {} incluye {}. Los Cubes existentes siguen funcionando. Mejora tu plan, o elimina un Cube en esta red, para traer uno.",
                        launcher_chrome::network_name(self.network),
                        projection.total_count,
                        projection.tier,
                        projection.cube_limit
                    )
                } else {
                    localized_format!(
                        "{}: {} of {} Cubes on the {} plan. Upgrade, or delete a Cube on this network, to bring one in.", "{}: {} de {} Cubes en el plan {}. Actualizar, o eliminar un Cube en esta red, para traer uno en.",
                        launcher_chrome::network_name(self.network),
                        projection.total_count,
                        projection.cube_limit,
                        projection.tier
                    )
                })
                .style(theme::text::warning),
            );
        }
        if !signed_in {
            content = content.push(
                Container::new(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Start)
                        .push(icon::tooltip_icon().style(theme::text::secondary))
                        .push(
                            p2_regular(localized_copy(
                                "Recovery Kit and phone recovery restore Cubes saved on your \
                                 Connect account, so they open after you sign in. Use Sign in \
                                 to Connect next to either one. Wallet import needs no account.",
                            ))
                            .style(theme::text::secondary),
                        ),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::card::simple),
            );
        }
        content.into()
    }

    /// Recover with phone's Cube step: the Connect Cubes on this network
    /// whose backup a paired phone can decrypt, then the count on other
    /// networks and a switch to list them all. Picking a Cube on another
    /// network switches to that network.
    fn restore_phone_view(&self) -> Element<ViewMessage> {
        let rule = || -> Element<ViewMessage> { restore_frame::full_rule() };
        let content = Column::new()
            .spacing(16)
            .max_width(760)
            .push(h4_bold(localized_copy("Which Cube?")))
            .push(
                p2_regular(localized_format!(
                    "Cubes on your Connect account · {}",
                    "Cubes en tu cuenta de Connect · {}",
                    self.network.label()
                ))
                .style(theme::text::secondary),
            );
        if !self.connect_account.is_authenticated() {
            return content
                .push(
                    p2_regular(localized_copy(
                        "Sign in to Connect to see the Cubes your phone can recover.",
                    ))
                    .style(theme::text::secondary),
                )
                .push(
                    button::secondary_action(
                        RESTORE_SIGN_IN_ID,
                        Some(icon::person_icon()),
                        coincube_ui::locale::copy("Sign in to Connect"),
                        create_journey_focus,
                    )
                    .width(Length::Shrink)
                    .on_press_maybe(
                        (!self.connect_account.is_checking_session()).then_some(
                            ViewMessage::GoToSection(HomeSection::Connect(
                                app::menu::ConnectSubMenu::Overview,
                            )),
                        ),
                    ),
                )
                .into();
        }
        let network = settings::network_to_api_string(self.network);
        // A descriptor-only phone recovery restores a Vault into an existing
        // Cube, not a Cube.
        let phones: Vec<(ChainId, &RemoteCube)> = self
            .remote_cubes
            .iter()
            .filter(|cube| cube.phone_recoverable && cube.phone_full_cube)
            .filter_map(|cube| restore_chain(&cube.network).map(|chain| (chain, cube)))
            .collect();
        let here = phones
            .iter()
            .filter(|(_, cube)| cube.network == network)
            .count();
        let others = phones.len() - here;
        let shown: Vec<&(ChainId, &RemoteCube)> = phones
            .iter()
            .filter(|(_, cube)| self.restore_all_networks || cube.network == network)
            .collect();
        let mut column = content;
        if shown.is_empty() {
            column = column.push(
                p2_regular(localized_format!(
                    "No Cube on your Connect account can be recovered with a phone on {}.",
                    "Ningún Cube de tu cuenta de Connect se puede recuperar con un teléfono en {}.",
                    self.network.label()
                ))
                .style(theme::text::secondary),
            );
        } else {
            let light = matches!(
                self.theme_mode,
                coincube_ui::theme::palette::ThemeMode::Light
            );
            let mut cubes = Column::new();
            for (chain, cube) in shown {
                let mut name = Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(p1_regular(cube.name.clone()));
                if *chain != self.network {
                    name = name
                        .push(create_journey_coin(*chain, light, 14.0))
                        .push(create_journey_tag(chain.label(), false));
                }
                cubes = cubes.push(rule()).push(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .padding([12, 0])
                        .push(Container::new(name).width(Length::Fill))
                        .push(
                            button::secondary_action(
                                Id::from(format!("tenshu.restore:phone:{}", cube.uuid)),
                                None,
                                coincube_ui::locale::copy("Recover"),
                                create_journey_focus,
                            )
                            .width(Length::Shrink)
                            .on_press(
                                ViewMessage::RestoreWithPhoneOn {
                                    chain: *chain,
                                    cube_id: cube.id,
                                    full_cube: cube.phone_full_cube,
                                },
                            ),
                        ),
                );
            }
            column = column.push(cubes.push(rule()));
        }
        if others > 0 {
            column = column.push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        p2_regular(localized_format!(
                            "{others} more on other networks",
                            "{others} más en otras redes"
                        ))
                        .style(theme::text::secondary),
                    )
                    .push(p2_regular("·").style(theme::text::secondary))
                    .push(
                        FocusableAction::new(
                            Id::new(RESTORE_ALL_NETWORKS_ID),
                            p2_regular(if self.restore_all_networks {
                                localized_format!(
                                    "Show {} only",
                                    "Mostrar solo {}",
                                    self.network.label()
                                )
                            } else {
                                "Show all networks".to_string()
                            }),
                            create_journey_focus,
                        )
                        .padding(0)
                        .style(theme::button::link)
                        .on_press(ViewMessage::ShowRestoreAllNetworks(
                            !self.restore_all_networks,
                        )),
                    ),
            );
        }
        column.into()
    }

    /// Home's frame for every screen except the visible Launcher List: the
    /// 190px sidebar beside the scrolling section body.
    fn legacy_frame(&self) -> Element<Message> {
        if matches!(self.state, State::Unchecked)
            && matches!(self.active_section, HomeSection::Cubes)
        {
            // A fresh launch and a network probe have not established whether
            // any Cubes exist yet. Keep creation actions out of this frame.
            return Container::new(
                Column::new()
                    .spacing(8)
                    .push(h4_bold(localized_copy("Checking for Cubes")))
                    .push(
                        p2_regular(localized_copy("Loading local Cube settings…"))
                            .style(theme::text::secondary),
                    ),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(theme::container::background)
            .into();
        }
        let unavailable = self.chain_availability(self.network);
        let unavailable_content = unavailable.reason().map(|reason| {
            Column::new()
                .spacing(16)
                .push(h3(self.network.label()))
                .push(text(reason.to_string()))
                .push(
                    button::secondary(None, localized_copy("Back to Bitcoin"))
                        .on_press(Message::View(ViewMessage::SelectNetwork(ChainId::Bitcoin))),
                )
                .into()
        });
        let content = Into::<Element<ViewMessage>>::into(scrollable(
            Column::new()
                // Network selection — right-aligned at top
                .push(
                    Row::new()
                        .push(if self.showing_create_form() && self.can_leave_create_journey() {
                            Some(
                                button::secondary(Some(icon::previous_icon()), localized_copy("Back to Cube list"))
                                    .on_press_maybe(if self.creating_cube {
                                        None
                                    } else {
                                        Some(ViewMessage::ShowCreateCube(false))
                                    }),
                            )
                        } else {
                            None
                        })
                        .push(Space::new().width(Length::Fill))
                        .spacing(10)
                        .push(
                            pick_list(
                                self.displayed_networks.as_slice(),
                                Some(self.network),
                                ViewMessage::SelectNetwork,
                            )
                            .style(theme::pick_list::primary)
                            .padding(10),
                        )
                        .align_y(Alignment::Center)
                        .padding(iced::Padding::from([10, 0])),
                )
                .push(
                    Container::new(
                        Column::new()
                            .align_x(Alignment::Center)
                            .spacing(20)
                            // "Your Cubes" heading — over any list screen,
                            // including the one that lists only remote Cubes.
                            .push(
                                if matches!(self.state, State::Cubes { .. } | State::NoCube { .. })
                                    && !self.showing_create_form()
                                {
                                    Some(text(localized_copy("Your Cubes")).size(24).bold())
                                } else {
                                    None
                                },
                            )
                            .push({
                                // Only show error at top if not in create cube
                                // form — the form renders `self.error` itself.
                                if !self.showing_create_form() {
                                    self.error.as_ref().map(|e| {
                                        card::simple(text(coincube_ui::locale::notice(e.clone())))
                                    })
                                } else {
                                    None
                                }
                            })
                            .push(match &self.state {
                                State::RecoveryInput => recovery_input_view(
                                    &self.recovery_words,
                                    self.recovery_active_index,
                                    self.creating_cube,
                                ),
                                State::CreationBackup(step) => {
                                    let view = creation_backup_view(
                                        step,
                                        self.creation_backup_words.as_ref().map(|w| w.as_slice()),
                                        self.creating_cube,
                                        self.creating_passkey_cube(),
                                        &self.creation_kit_password,
                                        &self.creation_kit_confirm,
                                        self.creation_kit_acknowledged,
                                        self.creation_kit_error.as_deref(),
                                        self.can_create_recovery_kit(),
                                        self.creation_backup_hub(),
                                    );
                                    // No sheet footer here, so the hub's
                                    // actions sit under it.
                                    if matches!(step, CreationBackupStep::Choice) {
                                        Column::new()
                                            .spacing(24)
                                            .push(view)
                                            .push(creation_choice_footer(
                                                self.creating_cube,
                                                self.creation_backup_hub(),
                                            ))
                                            .into()
                                    } else {
                                        view
                                    }
                                }
                                State::Cubes { create_cube, .. } => {
                                    if *create_cube {
                                        self.creation_form(true)
                                    } else {
                                        launcher_catalog::catalog_view(self)
                                    }
                                }
                                State::NoCube { .. } | State::Unchecked => {
                                    let current_net_str =
                                        settings::network_to_api_string(self.network);
                                    let remote_for_net: Vec<_> = self
                                        .remote_cubes
                                        .iter()
                                        .filter(|rc| rc.network == current_net_str)
                                        .collect();
                                    let total_count = self.total_cube_count();
                                    let at_limit = total_count >= self.cube_limit();

                                    // Center the children: the create form is a
                                    // `center_x(Fill)` container, so without this
                                    // the form spans/centers full width while the
                                    // shrink-width remote-cube rows default to the
                                    // left edge — leaving the list and form
                                    // misaligned.
                                    let mut col =
                                        Column::new().spacing(20).align_x(Alignment::Center);

                                    // The form owns the whole screen when it is
                                    // shown — never stacked under the remote-Cube
                                    // list, which is what made this screen read as
                                    // two pages crammed into one.
                                    if self.showing_create_form() {
                                        if at_limit && !remote_for_net.is_empty() {
                                            col = col.push(
                                                Container::new(
                                                    p1_regular(localized_format!(
                                                        "Cube limit reached. {} \
                                                         Upgrade your Connect account or delete a remote Cube to create one here.", "Alcanzaste el límite de Cubes. {} Mejora tu plan de Connect o elimina un Cube remoto para crear uno aquí.",
                                                        self.cube_limit_summary(self.network),
                                                    ))
                                                    .style(theme::text::secondary),
                                                )
                                                .max_width(500),
                                            );
                                        } else {
                                            col = col.push(self.creation_form(true));
                                        }
                                        col.into()
                                    } else {
                                        launcher_catalog::catalog_view(self)
                                    }
                                }
                            })
                            .align_x(Alignment::Center),
                    )
                    .center_x(Length::Fill),
                )
                .push(Space::new().height(Length::Fixed(40.0))),
        ))
        .map(Message::View);

        // If active section is Connect, show the account panel instead of cube list
        let main_content: Element<Message> = if matches!(self.active_section, HomeSection::Settings)
        {
            global_settings_view(self)
        } else if let HomeSection::Connect(_) = &self.active_section {
            // Render Connect account panel view
            let connect_view: Element<ConnectAccountMessage> =
                crate::app::view::connect::connect_account_panel(&self.connect_account);
            connect_view.map(|msg| Message::View(ViewMessage::ConnectAccount(msg)))
        } else if matches!(self.active_section, HomeSection::RecoverVault) {
            // Heir "Recover a Vault" discovery surface (COIN-377 / PR 1).
            recover_vault::view(&self.recover_vault)
                .map(|msg| Message::View(ViewMessage::RecoverVault(msg)))
        } else {
            unavailable_content.unwrap_or(content)
        };

        // Build the sidebar
        let sidebar = home_sidebar(self);

        // Wrap sidebar + content in a Row
        Row::new()
            .push(
                Container::new(sidebar)
                    .height(Length::Fill)
                    .width(Length::Fixed(190.0))
                    .style(coincube_ui::theme::container::foreground),
            )
            .push(
                Container::new(scrollable(
                    Row::new()
                        .push(Space::new().width(Length::FillPortion(1)))
                        .push(
                            Column::new()
                                .push(Space::new().height(Length::Fixed(30.0)))
                                .push(main_content)
                                .width(Length::FillPortion(8))
                                .max_width(1500),
                        )
                        .push(Space::new().width(Length::FillPortion(1))),
                ))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(coincube_ui::theme::container::background),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

/// The one-time incident notice. Deliberately says what happened, what it does
/// and doesn't mean for a multisig Cube, and what to do — and then gets out of
/// the way. It is not a warning about anything the app is currently doing.
fn advisory_notice_modal(
    advisory: &'static crate::hw_advisory::Advisory,
) -> Element<'static, Message> {
    Container::new(
        Container::new(scrollable(
            Column::new()
                .spacing(15)
                .padding(25)
                .width(Length::Fill)
                .push(h4_bold(advisory.headline))
                .push(p1_regular(advisory.notice))
                .push(
                    Row::new()
                        .spacing(10)
                        .push(
                            button::secondary(Some(icon::link_icon()), advisory.guide_label)
                                .on_press(Message::View(ViewMessage::OpenUrl(
                                    advisory.url.to_string(),
                                )))
                                .width(Length::Fill),
                        )
                        .push(
                            button::primary(Some(icon::check_icon()), localized_copy("Got it"))
                                .on_press(Message::View(ViewMessage::DismissAdvisoryNotice))
                                .width(Length::Fill),
                        ),
                ),
        ))
        .width(Length::Fill)
        .max_width(560)
        .max_height(700)
        .style(theme::card::modal),
    )
    .padding(16)
    .center_x(Length::Fill)
    .into()
}

fn global_settings_view(home: &Home) -> Element<Message> {
    let setting = |title, description, enabled, action: fn(bool) -> ViewMessage| {
        Row::new()
            .spacing(24)
            .align_y(Alignment::Center)
            .push(
                Column::new()
                    .spacing(6)
                    .width(Length::Fill)
                    .push(p1_bold(title))
                    .push(p1_regular(description).style(theme::text::secondary)),
            )
            .push(
                Toggler::new(enabled)
                    .on_toggle(move |value| Message::View(action(value)))
                    .width(50)
                    .style(theme::toggler::orange),
            )
            .padding(20)
    };
    let settings = Column::new()
        .spacing(0)
        .push(setting(
            "Developer mode",
            "Show test networks in the Cubes network dropdown.",
            home.developer_mode,
            ViewMessage::ToggleDeveloperMode,
        ))
        .push(setting(
            "Bitcoin Blake2b - Beta",
            "Show Bitcoin Blake2b beta features in the app.",
            home.connect_account.bitcoin_blake2b_opt_in,
            ViewMessage::ToggleBitcoinBlake2bBeta,
        ));
    let mut content = Column::new()
        .spacing(24)
        .max_width(700)
        .push(h3(localized_copy("Global Settings")))
        .push(
            p1_regular(localized_copy(
                "Preferences apply to all Cubes on this device. Changes are saved automatically.",
            ))
            .style(theme::text::secondary),
        )
        .push(card::simple(settings));
    if let Some(error) = &home.settings_error {
        content = content
            .push(p1_regular(coincube_ui::locale::notice(error.clone())).style(theme::text::error));
    }
    content.into()
}

const LEGACY_CONNECT_SIGN_IN_ID: &str = "tenshu.launcher.legacy:connect:sign-in";
const LEGACY_CONNECT_ENTRY_ID: &str = "tenshu.launcher.legacy:connect:entry";
pub(crate) const CREATE_BODY_SCROLL_ID: &str = "tenshu.create:body";
const CREATE_CLOSE_ID: &str = "tenshu.create:close";
const CREATE_IMPORT_OR_RESTORE_ID: &str = "tenshu.create:import-or-restore";
const RESTORE_CLOSE_ID: &str = "tenshu.restore:close";
const RESTORE_OVERVIEW_ID: &str = "tenshu.restore:nav:overview";
const RESTORE_IMPORT_ID: &str = "tenshu.restore:nav:import";
const RESTORE_RECOVER_VAULT_ID: &str = "tenshu.restore:nav:recover-vault";
const RESTORE_START_IMPORT_ID: &str = "tenshu.restore:start:import";
const RESTORE_START_RECOVER_VAULT_ID: &str = "tenshu.restore:start:recover-vault";
const RESTORE_SIGN_IN_ID: &str = "tenshu.restore:sign-in";
const RESTORE_KIT_ID: &str = "tenshu.restore:nav:kit";
const RESTORE_PHONE_ID: &str = "tenshu.restore:nav:phone";
const RESTORE_START_KIT_ID: &str = "tenshu.restore:start:kit";
const RESTORE_START_PHONE_ID: &str = "tenshu.restore:start:phone";
const RESTORE_SWITCH_TO_BITCOIN_ID: &str = "tenshu.restore:switch-to-bitcoin";
const RESTORE_ALL_NETWORKS_ID: &str = "tenshu.restore:all-networks";

/// The network a Connect Cube's API string names.
fn restore_chain(api: &str) -> Option<ChainId> {
    [
        ChainId::Bitcoin,
        ChainId::BitcoinBlake2b,
        ChainId::Testnet,
        ChainId::Testnet4,
        ChainId::Signet,
        ChainId::Regtest,
        ChainId::BitcoinBlake2bTestnet4,
    ]
    .iter()
    .copied()
    .find(|chain| settings::network_to_api_string(*chain) == api)
}

fn create_journey_focus(id: Id) -> ViewMessage {
    ViewMessage::Catalog(launcher_catalog::CatalogMessage::Focus(id))
}

fn create_journey_focus_task(id: Id) -> Task<Message> {
    Task::done(Message::View(create_journey_focus(id)))
}

fn create_network_row_id(chain: ChainId) -> Id {
    Id::from(format!(
        "tenshu.create:network:{}",
        settings::network_to_api_string(chain)
    ))
}

fn create_network_row_action(
    chain: ChainId,
    available: bool,
    selected: bool,
) -> Option<ViewMessage> {
    if !available {
        None
    } else if selected {
        // The current row stays in Tab order; choosing it again only closes
        // the menu, without restarting a network probe.
        Some(ViewMessage::CreateNetworkMenu(false))
    } else {
        Some(ViewMessage::SelectNetwork(chain))
    }
}

/// The same supplied coin art used by the Launcher, including the dashed and
/// faded ring on test networks. Kept in Home so Create can render it without
/// changing the Launcher or a shared widget.
fn create_journey_coin(chain: ChainId, light: bool, size: f32) -> Element<'static, ViewMessage> {
    iced::widget::Svg::new(iced::widget::svg::Handle::from_memory(
        launcher_coin::widget_source(chain, size, 1.0, light).into_bytes(),
    ))
    .width(Length::Fixed(size))
    .height(Length::Fixed(size))
    .into()
}

fn create_journey_tag<'a>(label: &'a str, blake2b: bool) -> Element<'a, ViewMessage> {
    Container::new(p2_regular(label).size(12))
        .padding([2, 6])
        .style(move |theme: &theme::Theme| iced::widget::container::Style {
            border: iced::Border {
                color: if blake2b {
                    theme.colors.shell.warn
                } else {
                    theme.colors.shell.muted
                },
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn legacy_connect_focus(id: Id) -> Message {
    Message::View(ViewMessage::Catalog(
        launcher_catalog::CatalogMessage::Focus(id),
    ))
}

fn home_sidebar<'a>(home: &'a Home) -> Element<'a, Message> {
    use coincube_ui::{component::button as btn, icon as ic};

    let msg = |vm: ViewMessage| -> Message { Message::View(vm) };

    let is_cubes_active = matches!(home.active_section, HomeSection::Cubes);
    let cubes_button = if is_cubes_active {
        Row::new()
            .push(btn::menu_active(Some(ic::cube_icon()), "Cubes").width(Length::Fill))
            .width(Length::Fill)
    } else {
        Row::new()
            .push(
                btn::menu(Some(ic::cube_icon()), "Cubes")
                    .on_press(msg(ViewMessage::GoToSection(HomeSection::Cubes)))
                    .width(Length::Fill),
            )
            .width(Length::Fill)
    };

    let is_authenticated = home.connect_account.is_authenticated();

    let mut col = Column::new()
        .spacing(0)
        .width(Length::Fill)
        .push(
            Container::new(image::tenshu_wordmark(28.0))
                .padding(10)
                .center_x(Length::Fill),
        )
        .push(cubes_button);

    if is_authenticated {
        let connect_chevron = if home.connect_expanded {
            ic::up_icon()
        } else {
            ic::down_icon()
        };
        let connect_button: Element<Message> = FocusableAction::new(
            LEGACY_CONNECT_ENTRY_ID,
            Container::new(
                Row::new()
                    .spacing(10)
                    .align_y(iced::alignment::Vertical::Center)
                    .push(ic::connect_icon().style(coincube_ui::theme::text::secondary))
                    .push(
                        coincube_ui::component::text::p1_regular("Connect")
                            .style(coincube_ui::theme::text::secondary),
                    )
                    .push(connect_chevron.style(coincube_ui::theme::text::secondary)),
            )
            .align_x(Alignment::Start)
            .width(Length::Fill)
            .padding(15),
            legacy_connect_focus,
        )
        .width(Length::Fill)
        .style(coincube_ui::theme::button::menu)
        .on_press(msg(ViewMessage::ToggleConnect))
        .into();
        col = col.push(connect_button);
    }

    if home.connect_expanded && is_authenticated {
        use app::menu::ConnectSubMenu;
        let mut items: Vec<(&str, ConnectSubMenu)> = vec![
            ("Overview", ConnectSubMenu::Overview),
            ("Contacts", ConnectSubMenu::Contacts),
            ("Plan & Billing", ConnectSubMenu::PlanBilling),
            ("Security", ConnectSubMenu::Security),
        ];
        // Duress (Phase 9) is a launch kill-switch: shown only when the account
        // is entitled AND the launch gate is on (server `duressEnabled`, or this
        // account is already enrolled — the grandfather case). Hidden, not
        // greyed, when off — like Marketplace. See
        // `ConnectAccountPanel::show_duress`.
        if home.connect_account.show_duress() {
            items.push(("Duress", ConnectSubMenu::Duress));
        }
        for (label, sub) in &items {
            let is_active = matches!(
                &home.active_section,
                HomeSection::Connect(s) if *s == *sub
            );
            let item = if is_active {
                Row::new()
                    .push(btn::menu_active(None, label).width(Length::Fill))
                    .width(Length::Fill)
            } else {
                Row::new()
                    .push(
                        btn::menu(None, label)
                            .on_press(msg(ViewMessage::GoToSection(HomeSection::Connect(
                                sub.clone(),
                            ))))
                            .width(Length::Fill),
                    )
                    .width(Length::Fill)
            };
            col = col.push(item);
        }
    }

    // Heir "Recover a Vault" — global discovery surface (COIN-377 / PR 1).
    // Gated behind the capability flag (dark until the API's `recoverable`
    // endpoint + COIN-376 sweep ship) and only shown to a signed-in account.
    if is_authenticated && !home.network.is_blake2b() && feature_flags::RECOVER_VAULT_ENABLED {
        let is_active = matches!(home.active_section, HomeSection::RecoverVault);
        let recover_button = if is_active {
            Row::new()
                .push(
                    btn::menu_active(Some(ic::cube_icon()), "Recover a Vault").width(Length::Fill),
                )
                .width(Length::Fill)
        } else {
            Row::new()
                .push(
                    btn::menu(Some(ic::cube_icon()), "Recover a Vault")
                        .on_press(msg(ViewMessage::GoToSection(HomeSection::RecoverVault)))
                        .width(Length::Fill),
                )
                .width(Length::Fill)
        };
        col = col.push(recover_button);
    }

    // Bottom-pinned utilities: Settings and theme.
    let mut bottom_col = Column::new().spacing(0).width(Length::Fill);

    let settings_button = btn::menu(Some(ic::settings_icon()), "App preferences")
        .on_press(msg(ViewMessage::OpenAppPreferences))
        .width(Length::Fill);
    bottom_col = bottom_col.push(settings_button);

    let theme_toggle_btn =
        coincube_ui::image::theme_toggle_button(home.theme_mode, msg(ViewMessage::ToggleTheme));

    bottom_col = bottom_col.push(
        Container::new(theme_toggle_btn)
            .padding(iced::Padding {
                top: 4.0,
                right: 8.0,
                bottom: 16.0,
                left: 8.0,
            })
            .align_x(Alignment::Start)
            .width(Length::Fill),
    );

    if is_authenticated || home.connect_account.is_checking_session() {
        bottom_col = bottom_col.push(connect_status_row(&home.connect_account));
    } else {
        bottom_col = bottom_col.push(
            Container::new(
                FocusableAction::new(
                    LEGACY_CONNECT_SIGN_IN_ID,
                    Container::new(p1_bold(localized_copy("Sign In")))
                        .center_x(Length::Fill)
                        .padding(5),
                    legacy_connect_focus,
                )
                .on_press(msg(ViewMessage::GoToSection(HomeSection::Connect(
                    app::menu::ConnectSubMenu::Overview,
                ))))
                .style(theme::button::primary)
                .width(Length::Fill),
            )
            .padding(10)
            .width(Length::Fill),
        );
    }

    // Outer layout: scrollable menu fills, bottom section pinned
    Column::new()
        .push(scrollable(col).height(Length::Fill))
        .push(bottom_col)
        .height(Length::Fill)
        .into()
}

/// Connect session indicator for an authenticated or pending session. It
/// replaces the Sign In button; a signed-out session needs only that action.
fn connect_status_row<'a>(account: &ConnectAccountPanel) -> Element<'a, Message> {
    coincube_ui::component::session_status::indicator(account.is_checking_session())
}

/// The Create Cube journey's steps. It ends once the Cube is written: the
/// Vault is created later, from inside the Cube, by its own wizard.
fn create_cube_steps(passkey_mode: bool) -> &'static [&'static str] {
    if passkey_mode {
        &["Cube", "Passkey", "Backup", "Done"]
    } else {
        &["Cube", "Backup", "Done"]
    }
}

#[derive(Clone, Copy)]
struct CreateCubeFormOptions {
    spark_available: bool,
    creating_cube: bool,
    passkey_mode: bool,
    importing_bitcoin: bool,
    can_import_bitcoin: bool,
    submit_in_form: bool,
}

/// Live comparison of the Create form's two PIN fields, shown as the user
/// types so a mismatch is caught before they press Create Cube.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreatePinMatch {
    /// The first PIN is incomplete, or the confirmation agrees so far.
    Pending,
    /// A confirmation digit differs from the one entered above it.
    Mismatch,
    /// Both PINs are complete and equal.
    Match,
}

fn create_cube_pin_match(
    pin: &pin_input::PinInput,
    pin_confirm: &pin_input::PinInput,
) -> CreatePinMatch {
    if !pin.is_complete() {
        return CreatePinMatch::Pending;
    }
    let differs = pin
        .digits
        .iter()
        .zip(pin_confirm.digits.iter())
        .any(|(entered, confirmed)| !confirmed.is_empty() && entered != confirmed);
    if differs {
        CreatePinMatch::Mismatch
    } else if pin_confirm.is_complete() {
        CreatePinMatch::Match
    } else {
        CreatePinMatch::Pending
    }
}

fn create_cube_can_submit(
    cube_name: &coincube_ui::component::form::Value<String>,
    pin: &pin_input::PinInput,
    pin_confirm: &pin_input::PinInput,
    passkey_mode: bool,
    creating_cube: bool,
) -> bool {
    !creating_cube
        && cube_name.valid
        && !cube_name.value.trim().is_empty()
        && (passkey_mode || create_cube_pin_match(pin, pin_confirm) == CreatePinMatch::Match)
}

fn create_cube_form<'a>(
    cube_name: &coincube_ui::component::form::Value<String>,
    pin: &'a pin_input::PinInput,
    pin_confirm: &'a pin_input::PinInput,
    error: &'a Option<String>,
    options: CreateCubeFormOptions,
) -> Element<'a, ViewMessage> {
    use coincube_ui::component::form;
    let CreateCubeFormOptions {
        spark_available,
        creating_cube,
        passkey_mode,
        importing_bitcoin,
        can_import_bitcoin,
        submit_in_form,
    } = options;

    let mut column = Column::new()
        .spacing(20)
        .align_x(if importing_bitcoin { Alignment::Center } else { Alignment::Start })
        .width(Length::Fill)
        .max_width(if importing_bitcoin { 500.0 } else { 640.0 })
        .push(h4_bold(if importing_bitcoin {
            localized_copy("Create a Cube for your Bitcoin wallet")
        } else {
            localized_copy("Create a new Cube")
        }))
        .push(
            p1_regular(if importing_bitcoin {
                localized_copy("Create and back up your Cube first, then import the public descriptors for your existing Bitcoin wallet. Your original wallet signers remain necessary to spend.")
            } else if spark_available {
                localized_copy("A Cube is an account. It holds a Spark wallet and a Vault.")
            } else {
                localized_copy("A Cube is an account. On this network it holds a Vault.")
            })
            .style(theme::text::secondary),
        );

    // Unlock-method toggle — hidden entirely when the passkey feature is off,
    // and on every platform but macOS regardless. The Windows Hello and
    // security-key labels this used to carry offered a choice whose unlock path
    // does not exist yet; taking it would have produced a Cube the build could
    // not open. See `PASSKEY_CREATION_AVAILABLE`.
    //
    // Styled as a two-label A/B row to match the General Settings toggles
    // (BTC/Sats, Fiat/Bitcoin): left label = toggler off. Passkey sits on the
    // left because it is the default.
    if feature_flags::PASSKEY_CREATION_AVAILABLE && !importing_bitcoin {
        column = column.push(
            card::simple(
                Row::new()
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(text(localized_copy("Unlock method:")).bold())
                    .push(Space::new().width(Length::Fill))
                    .push(text("Passkey"))
                    .push(
                        Toggler::new(!passkey_mode)
                            .on_toggle(|is_pin| ViewMessage::TogglePasskeyMode(!is_pin))
                            .width(50)
                            .style(theme::toggler::orange),
                    )
                    .push(text("PIN")),
            )
            .width(Length::Fill),
        );
    }
    column = column.push(
        Container::new(
            form::Form::new(
                coincube_ui::locale::copy("Cube Name"),
                cube_name,
                ViewMessage::CubeNameEdited,
            )
            .warning(localized_copy("Please enter a name"))
            .size(20)
            .padding(10),
        )
        .width(Length::Fill),
    );

    // PIN or passkey info section
    if passkey_mode {
        // Passkey mode: no PIN needed — biometric auth replaces it
        let description = if cfg!(target_os = "macos") {
            "Your Cube will be secured with a passkey. No PIN is needed \u{2014} \
             you'll use Touch ID to unlock it."
        } else if cfg!(target_os = "windows") {
            "Your Cube will be secured with a passkey. No PIN is needed \u{2014} \
             you'll use Windows Hello to unlock it."
        } else {
            "Your Cube will be secured with a passkey. No PIN is needed \u{2014} \
             you'll use a FIDO2 security key to unlock it."
        };
        column = column.push(p1_regular(description).style(theme::text::secondary));
    } else {
        // Enter and Confirm sit side by side, using the Launcher unlock
        // face's compact row (276px each), and wrap onto two lines where
        // the form is narrower than both — the 500px Bitcoin-import form.
        let pin_field = |label: &'static str, input: Element<'a, ViewMessage>| {
            Column::new()
                .spacing(8)
                .width(Length::Fixed(276.0))
                .push(p1_regular(label).style(theme::text::secondary))
                .push(input)
        };
        column = column.push(
            Row::new()
                .spacing(32)
                .push(pin_field(
                    "Enter PIN:",
                    pin.face_view().map(ViewMessage::PinInput),
                ))
                .push(pin_field(
                    "Confirm PIN:",
                    pin_confirm.face_view().map(ViewMessage::PinConfirmInput),
                ))
                .wrap()
                .vertical_spacing(20),
        );
        // The status line keeps its height while empty so the form doesn't
        // shift when the result appears.
        let status: Element<'a, ViewMessage> = match create_cube_pin_match(pin, pin_confirm) {
            CreatePinMatch::Mismatch => Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(icon::cross_icon().style(theme::text::error))
                .push(
                    p2_regular(localized_copy("Doesn't match the PIN you entered."))
                        .style(theme::text::error),
                )
                .into(),
            CreatePinMatch::Match => Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(icon::check_icon().style(theme::text::success))
                .push(p2_regular("PINs match").style(theme::text::success))
                .into(),
            CreatePinMatch::Pending => Space::new().into(),
        };
        column = column.push(Container::new(status).height(Length::Fixed(20.0)));
    }

    column = column.push(Space::new().height(Length::Fixed(10.0)));

    // Show error above the button
    if let Some(err) = error {
        column = column.push(
            p1_regular(coincube_ui::locale::notice(err.to_owned())).style(theme::text::error),
        );
    }

    // The Create journey keeps its submit action in a fixed footer so PIN
    // fields remain scrollable at the minimum window height. The legacy
    // Bitcoin import form still renders its own submit action here.
    if submit_in_form {
        let submit_button = if creating_cube {
            iced::widget::button(
                Container::new(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .push(text(localized_copy("Creating")))
                        .push(
                            Container::new(spinner::typing_text_carousel(
                                "...",
                                true,
                                std::time::Duration::from_millis(500),
                                text,
                            ))
                            .width(Length::Fixed(20.0)),
                        ),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .width(Length::Fixed(200.0))
            .height(Length::Fixed(44.0))
            .style(theme::button::primary)
        } else {
            button::primary(None, localized_copy("Create Cube"))
                .width(Length::Fixed(200.0))
                .on_press_maybe(
                    create_cube_can_submit(
                        cube_name,
                        pin,
                        pin_confirm,
                        passkey_mode,
                        creating_cube,
                    )
                    .then_some(ViewMessage::CreateCube),
                )
        };
        column = column.push(submit_button);
    }
    if !importing_bitcoin && can_import_bitcoin {
        column = column.push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(
                    p2_regular(localized_copy("Already have a wallet or a Recovery Kit?"))
                        .style(theme::text::secondary),
                )
                .push(
                    FocusableAction::new(
                        Id::new(CREATE_IMPORT_OR_RESTORE_ID),
                        p2_regular(localized_copy("Import or restore instead")),
                        create_journey_focus,
                    )
                    .padding(0)
                    .style(theme::button::link)
                    .on_press_maybe((!creating_cube).then_some(ViewMessage::ShowRestoreHub)),
                ),
        );
    }

    Container::new(column)
        .padding(if importing_bitcoin { 20 } else { 0 })
        .center_x(Length::Fill)
        .into()
}

/// The Vault line under a Cube's name on the Cubes list.
///
/// Three states, matching Keychain's cubes list character for character
/// (`plans/PLAN-vault-identity-unification.md` D2):
///
/// - no Vault → `No Vault configured`
/// - a Vault whose fingerprint is known → `Vault 8099ee80`, the same id shown
///   in Vault settings, sent in the pairing QR, and displayed by Keychain
/// - a Vault predating [`CubeSettings::vault_fingerprint`] → `Vault
///   configured`, no id, until the backfill runs on its first open
///
/// The last case deliberately shows nothing rather than falling back to
/// `WalletId::descriptor_checksum` or `wallet_name()`. Neither identifies the
/// vault — the checksum is a storage key and the name is a placeholder — and
/// rendering one as if it did is the bug this replaced.
fn vault_subtitle(cube: &CubeSettings) -> String {
    if let Some(import) = &cube.bitcoin_import {
        return match &import.stage {
            settings::BitcoinImportStage::PendingSource => "Vault import pending".into(),
            settings::BitcoinImportStage::Installing { .. } => {
                "Vault import interrupted — resume import".into()
            }
            settings::BitcoinImportStage::Syncing { .. } => {
                "Vault · Imported wallet · Sync incomplete".into()
            }
            settings::BitcoinImportStage::Ready { .. } => "Vault · Imported wallet".into(),
        };
    }
    match (&cube.vault_wallet_id, &cube.vault_fingerprint) {
        (None, _) => "No Vault configured".to_string(),
        (Some(_), None) => "Vault configured".to_string(),
        (Some(_), Some(fp)) => format!("Vault {}", fp),
    }
}

/// Derive phone (owner-keychain) recovery availability from the `ownerSelf`
/// block of a `/recovery-kit/status` response. Returns
/// `(phone_recoverable, phone_full_cube)`:
///   * `phone_recoverable` — a recipient is registered *and* an envelope set
///     has actually been sealed to it. A bare recipient with nothing uploaded
///     is **not** recoverable (the phone can register the key before the
///     desktop seals + uploads), so the "Recover" action wouldn't dead-end.
///   * `phone_full_cube` — the sealed set includes the seed artifact (Full
///     Cube) rather than the descriptor alone (Vault-only). Drives the restore
///     scope + label.
fn derive_phone_recovery(owner_self: Option<&OwnerSelfRecoverySummary>) -> (bool, bool) {
    match owner_self {
        Some(o) => (
            o.has_recipient && o.has_envelope(),
            o.envelope_kinds.iter().any(|k| k == "seed"),
        ),
        None => (false, false),
    }
}

/// The creation-time backup step.
///
/// The three seed phrase screens are the Settings ones, reused verbatim
/// through `app::view::settings::backup`'s message-generic views with
/// [`ViewMessage::CreationBackup`] as the mapper; the Kit password screen is
/// the Settings Kit flow's. What is creation-specific belongs here:
///
/// - The hub ([`creation_choice_view`]), which carries
///   [`creation_gate::not_a_backup_copy`], because the mistake this step
///   exists to prevent is a user believing the datadir folder is their backup.
/// - The "I'll do this later" escape, offered from the hub until a backup
///   exists, and its acknowledgement screen. Settings has no equivalent:
///   there, declining just closes a panel.
#[allow(clippy::too_many_arguments)]
fn creation_backup_view<'a>(
    step: &'a CreationBackupStep,
    words: Option<&'a [String]>,
    saving: bool,
    is_passkey: bool,
    kit_password: &'a zeroize::Zeroizing<String>,
    kit_confirm: &'a zeroize::Zeroizing<String>,
    kit_acknowledged: bool,
    kit_error: Option<&'a str>,
    kit_available: bool,
    hub: CreationBackupHub,
) -> Element<'a, ViewMessage> {
    use crate::app::view::settings::backup;

    // The method screens return to the hub (their Back or Cancel), where
    // "I'll do this later" and Cancel live; they don't repeat them.
    match step {
        CreationBackupStep::Intro(checked) => {
            backup::intro_view(*checked, None, ViewMessage::CreationBackup)
        }
        CreationBackupStep::Phrase => match words {
            Some(words) => backup::recovery_phrase_view(words, ViewMessage::CreationBackup),
            // Only reachable if the seed was scrubbed underneath the view;
            // `update_creation_backup` turns the same condition into an error
            // and abandons. Render nothing rather than an empty word grid.
            None => Column::new().into(),
        },
        CreationBackupStep::Verification {
            word_indices,
            word_inputs,
            error,
        } => backup::verification_view(
            word_indices,
            word_inputs,
            error.as_deref(),
            saving,
            ViewMessage::CreationBackup,
        ),
        CreationBackupStep::Bypass { acknowledged } => {
            creation_backup_bypass_view(*acknowledged, is_passkey)
        }
        CreationBackupStep::Choice => creation_choice_view(saving, is_passkey, kit_available, hub),
        CreationBackupStep::KitPassword => {
            // The Settings Kit flow's own screen, message-generic so both
            // places apply the same length, strength and acknowledgement gates.
            crate::app::view::settings::recovery_kit::password_entry_view(
                kit_password,
                kit_confirm,
                kit_acknowledged,
                kit_error,
                ViewMessage::CreationKit,
            )
        }
        CreationBackupStep::KitUploading => Container::new(
            Column::new()
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .spacing(20)
                .push(
                    text(localized_copy("Saving your Recovery Kit…"))
                        .size(24)
                        .bold(),
                )
                .push(
                    Container::new(
                        p1_regular(localized_copy(
                            "Encrypting this Cube's master seed with your recovery password \
                             and saving it to COINCUBE Connect. You'll come back to Backup \
                             when it's done.",
                        ))
                        .style(theme::text::secondary)
                        .align_x(iced::alignment::Horizontal::Center),
                    )
                    .max_width(500),
                ),
        )
        .width(Length::Fill)
        .into(),
    }
}

/// What the backup hub has collected so far for the Cube being created.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CreationBackupHub {
    seed_confirmed: bool,
    kit_saved: bool,
}

impl CreationBackupHub {
    fn any(self) -> bool {
        self.seed_confirmed || self.kit_saved
    }
}

const CREATE_BACKUP_SEED_ID: &str = "tenshu.create:backup-seed";
const CREATE_BACKUP_SIGN_IN_ID: &str = "tenshu.create:backup-sign-in";
const CREATE_BACKUP_KIT_ID: &str = "tenshu.create:backup-kit";
const CREATE_BACKUP_CANCEL_ID: &str = "tenshu.create:backup-cancel";
const CREATE_BACKUP_LATER_ID: &str = "tenshu.create:backup-later";
const CREATE_BACKUP_CONTINUE_ID: &str = "tenshu.create:backup-continue";

/// The backup hub: Seed phrase and Recovery Kit as two rows, named and worded
/// as in Settings › Backup methods. Either or both; each one's screens return
/// here, its row then shows the result, and the footer's Continue writes the
/// Cube (see [`Home::finish_creation_backup`]).
///
/// The Recovery Kit needs a Connect account, so a signed-out row offers
/// Sign in in place of its action. This is the one place creation asks for it:
/// the moment it decides where a backup can go.
fn creation_choice_view(
    saving: bool,
    is_passkey: bool,
    kit_available: bool,
    hub: CreationBackupHub,
) -> Element<'static, ViewMessage> {
    use crate::app::view::settings::page;

    let seed_row = page::row(localized_copy("Seed phrase"))
        .description(localized_copy("12 words, written down offline."));
    let seed_row = if hub.seed_confirmed {
        seed_row.accessory(page::status(page::Status::Ok, localized_copy("Confirmed")))
    } else {
        seed_row.control(
            button::secondary_action(
                CREATE_BACKUP_SEED_ID,
                None,
                coincube_ui::locale::copy("Write it down"),
                create_journey_focus,
            )
            .width(Length::Shrink)
            .on_press_maybe(
                (!saving).then_some(ViewMessage::CreationBackup(BackupWalletMessage::NextStep)),
            ),
        )
    };

    let kit_row = page::row(localized_copy("Recovery Kit"));
    let kit_row = if hub.kit_saved {
        kit_row
            .description(localized_copy(
                "Encrypted copy kept with your Connect account.",
            ))
            .accessory(page::status(page::Status::Ok, localized_copy("Backed up")))
    } else if kit_available {
        kit_row
            .description(localized_copy(
                "Master seed, encrypted with a recovery password only you know.",
            ))
            .control(
                button::secondary_action(
                    CREATE_BACKUP_KIT_ID,
                    None,
                    coincube_ui::locale::copy("Create Recovery Kit"),
                    create_journey_focus,
                )
                .width(Length::Shrink)
                .on_press_maybe((!saving).then_some(ViewMessage::CreationKitRequested)),
            )
    } else {
        kit_row
            .description(localized_copy(
                "Kept with your Connect account. Signing in also syncs this Cube to your \
                 other devices and the Keychain app.",
            ))
            .accessory(page::status(
                page::Status::Unknown,
                localized_copy("Sign in required"),
            ))
            .control(
                // Opens the Connect sign-in sheet over this one. Its Close
                // returns focus to this row: to Sign in, or to Create Recovery
                // Kit once signed in (`connect_sheet_origin_id`).
                button::secondary_action(
                    CREATE_BACKUP_SIGN_IN_ID,
                    None,
                    coincube_ui::locale::copy("Sign in"),
                    create_journey_focus,
                )
                .width(Length::Shrink)
                .on_press_maybe((!saving).then_some(ViewMessage::GoToSection(
                    HomeSection::Connect(app::menu::ConnectSubMenu::Overview),
                ))),
            )
    };

    Column::new()
        .width(Length::Fill)
        .max_width(640)
        .spacing(20)
        .push(h4_bold(localized_copy("Back up this Cube")))
        .push(
            p1_regular(if is_passkey {
                localized_copy(
                    "Set up one or both. Either one restores this Cube on another computer, \
                 even without your passkey.",
                )
            } else {
                localized_copy(
                    "Set up one or both. Either one restores this Cube on another computer, \
                 even without your PIN.",
                )
            })
            .style(theme::text::secondary),
        )
        .push(page::rows([seed_row.into(), kit_row.into()]))
        .push(
            p2_regular(creation_gate::not_a_backup_copy(is_passkey)).style(theme::text::secondary),
        )
        .into()
}

/// The backup hub's footer: Cancel on the left; "I'll do this later" until a
/// backup exists, then Continue, on the right.
fn creation_choice_footer(saving: bool, hub: CreationBackupHub) -> Element<'static, ViewMessage> {
    let mut row = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(
            button::secondary_action(
                CREATE_BACKUP_CANCEL_ID,
                None,
                coincube_ui::locale::copy("Cancel"),
                create_journey_focus,
            )
            .width(Length::Shrink)
            .on_press_maybe((!saving).then_some(ViewMessage::CancelCreationBackup)),
        )
        .push(Space::new().width(Length::Fill));
    if !hub.any() {
        row = row.push(
            button::secondary_action(
                CREATE_BACKUP_LATER_ID,
                None,
                coincube_ui::locale::copy("I'll do this later"),
                create_journey_focus,
            )
            .width(Length::Shrink)
            .on_press_maybe((!saving).then_some(ViewMessage::CreationBackupBypassRequested)),
        );
    }
    row.push(
        button::primary_action(
            CREATE_BACKUP_CONTINUE_ID,
            None,
            if saving { "Creating…" } else { "Continue" },
            create_journey_focus,
        )
        .width(Length::Shrink)
        .on_press_maybe((hub.any() && !saving).then_some(ViewMessage::CreationBackupContinue)),
    )
    .into()
}

/// The bypass screen: the acknowledgement, a checkbox that must be actively
/// ticked, and no way past it that does not go through the checkbox.
///
/// `is_passkey` swaps the folder-copy copy — the PIN version's device-secret
/// reasoning is false of a passkey Cube (`creation_gate::not_a_backup_copy`).
/// "Back up now" returns to the choice screen for both shapes, because both
/// have the same exits waiting there.
fn creation_backup_bypass_view(
    acknowledged: bool,
    is_passkey: bool,
) -> Element<'static, ViewMessage> {
    Container::new(
        Column::new()
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .spacing(20)
            .push(
                text(localized_copy("Create this Cube without a backup?"))
                    .size(24)
                    .bold(),
            )
            .push(
                Container::new(
                    p1_regular(creation_gate::not_a_backup_copy(is_passkey))
                        .style(theme::text::secondary)
                        .align_x(iced::alignment::Horizontal::Center),
                )
                .max_width(500),
            )
            .push(
                Container::new(
                    CheckBox::new(acknowledged)
                        .label(creation_gate::BYPASS_ACKNOWLEDGEMENT)
                        .on_toggle(ViewMessage::CreationBackupAcknowledgeBypass)
                        .style(theme::checkbox::primary)
                        .size(20),
                )
                .max_width(500),
            )
            .push(
                Row::new()
                    .spacing(20)
                    .push(
                        button::secondary(
                            Some(icon::previous_icon()),
                            localized_copy("Back up now"),
                        )
                        .on_press(ViewMessage::CreationBackupChoiceRequested)
                        .width(Length::Fixed(240.0)),
                    )
                    .push(
                        button::primary(None, localized_copy("Create without a backup"))
                            .on_press_maybe(
                                acknowledged.then_some(ViewMessage::CreationBackupBypassConfirmed),
                            )
                            .width(Length::Fixed(240.0)),
                    ),
            )
            .push(
                button::transparent(None, localized_copy("Cancel"))
                    .on_press(ViewMessage::CancelCreationBackup),
            ),
    )
    .width(Length::Fill)
    .into()
}

fn recovery_input_view(
    recovery_words: &[String; 12],
    active_index: Option<usize>,
    creating: bool,
) -> Element<ViewMessage> {
    use coincube_ui::widget::{Row, TextInput};

    const INPUT_WIDTH: f32 = 150.0;
    const INPUT_ROW_HEIGHT: f32 = 46.0;
    const GRID_COL_SPACING: f32 = 40.0;
    const GRID_ROW_SPACING: f32 = 30.0;
    const OVERLAY_TOP_GAP: f32 = 6.0;
    const GRID_WIDTH: f32 = (INPUT_WIDTH * 4.0) + (GRID_COL_SPACING * 3.0);
    const OVERLAY_BOTTOM_RESERVE: f32 = 220.0;

    // Create the mnemonic input grid (3 rows x 4 columns)
    let mut grid = Column::new().spacing(30).align_x(Alignment::Center);

    for row in 0..3 {
        let mut row_widget = Row::new().spacing(40).align_y(Alignment::Center);

        for col in 0..4 {
            let index = row * 4 + col;
            let word_value = &recovery_words[index];
            let placeholder = format!("{}.", index + 1);

            let text_input = TextInput::new(&placeholder, word_value)
                .on_input(move |input| ViewMessage::RecoveryWordInput { index, word: input })
                .padding(12)
                .width(Length::Fixed(INPUT_WIDTH))
                .style(theme::text_input::primary);

            row_widget = row_widget.push(text_input);
        }

        grid = grid.push(row_widget);
    }

    let suggestions_overlay: Option<Element<ViewMessage>> = active_index.and_then(|index| {
        let word_value = recovery_words.get(index)?;
        if word_value.len() < 2 {
            return None;
        }

        let suggestions: Vec<String> = bip39_suggestions(word_value, 12)
            .into_iter()
            .filter(|s| s != word_value)
            .take(6)
            .collect();
        if suggestions.is_empty() {
            return None;
        }

        let suggestion_list = suggestions.into_iter().fold(
            Column::new().spacing(2).width(Length::Fill),
            |col, suggestion| {
                col.push(
                    iced::widget::button(text(suggestion.clone()))
                        .style(theme::button::secondary)
                        .width(Length::Fill)
                        .on_press(ViewMessage::SelectRecoverySuggestion {
                            index,
                            word: suggestion,
                        }),
                )
            },
        );

        let row = index / 4;
        let col = index % 4;
        let top_offset =
            row as f32 * (INPUT_ROW_HEIGHT + GRID_ROW_SPACING) + INPUT_ROW_HEIGHT + OVERLAY_TOP_GAP;
        let left_offset = col as f32 * (INPUT_WIDTH + GRID_COL_SPACING);

        Some(
            Column::new()
                .push(Space::new().height(Length::Fixed(top_offset)))
                .push(
                    Row::new()
                        .push(Space::new().width(Length::Fill))
                        .push(
                            Container::new(
                                Row::new()
                                    .push(Space::new().width(Length::Fixed(left_offset)))
                                    .push(
                                        Container::new(suggestion_list)
                                            .width(Length::Fixed(INPUT_WIDTH))
                                            .padding(6)
                                            .style(theme::card::simple),
                                    )
                                    .push(Space::new().width(Length::Fill)),
                            )
                            .width(Length::Fixed(GRID_WIDTH)),
                        )
                        .push(Space::new().width(Length::Fill)),
                )
                .into(),
        )
    });

    let overlay_layer: Element<ViewMessage> = suggestions_overlay.unwrap_or_else(|| {
        Container::new(Space::new())
            .width(Length::Fill)
            .height(Length::Shrink)
            .into()
    });

    // Check if all words are filled
    let all_filled = recovery_words.iter().all(|w| {
        let word = w.trim();
        !word.is_empty() && bip39::Language::English.find_word(word).is_some()
    });

    let grid_row: Element<ViewMessage> = Row::new()
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .push(Space::new().width(Length::Fill))
        .push(grid)
        .push(Space::new().width(Length::Fill))
        .into();

    let actions_row: Element<ViewMessage> = Row::new()
        .width(Length::Fill)
        .spacing(15)
        .align_y(Alignment::Center)
        .push(Space::new().width(Length::Fill))
        .push(
            button::secondary(None, localized_copy("Cancel"))
                .width(Length::Fixed(145.0))
                .on_press_maybe((!creating).then_some(ViewMessage::CancelRecovery)),
        )
        .push(
            button::primary(
                None,
                if creating {
                    "Recovering…"
                } else {
                    "Recover Wallet"
                },
            )
            .width(Length::Fixed(145.0))
            .on_press_maybe(if all_filled && !creating {
                Some(ViewMessage::SubmitRecovery)
            } else {
                None
            }),
        )
        .push(Space::new().width(Length::Fill))
        .into();

    let section_base: Element<ViewMessage> = Column::new()
        .push(grid_row)
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(actions_row)
        .push(Space::new().height(Length::Fixed(OVERLAY_BOTTOM_RESERVE)))
        .into();

    let section_with_overlay: Element<ViewMessage> =
        Stack::new().push(section_base).push(overlay_layer).into();

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .push(h4_bold("Enter Recovery Phrase"))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(
                        p1_regular(localized_copy(
                            "Enter your 12-word recovery phrase to restore your Cube.",
                        ))
                        .align_x(iced::alignment::Horizontal::Center),
                    )
                    .width(Length::Fixed(700.0))
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(section_with_overlay)
        .into()
}

fn has_existing_wallet(data_dir: &CoincubeDirectory, network: impl Into<ChainId>) -> bool {
    data_dir
        .path()
        .join(network.into().dir_name())
        .join(settings::SETTINGS_FILE_NAME)
        .exists()
}

/// Map a `Task<app::message::Message>` (from ConnectAccountPanel) into a
/// `Task<home::Message>` by extracting the ConnectAccountMessage.
fn map_connect_task(task: Task<app::message::Message>) -> Task<Message> {
    task.map(|app_msg| match app_msg {
        app::message::Message::View(app::view::Message::ConnectAccount(acct_msg)) => {
            Message::View(ViewMessage::ConnectAccount(acct_msg))
        }
        app::message::Message::View(app::view::Message::OpenUrl(url)) => {
            Message::View(ViewMessage::OpenUrl(url))
        }
        // Duress enrollment persistence (Phases 2 & 8): relay to Home, which
        // has the datadir + Cube settings to actually write it.
        app::message::Message::CompleteDuressEnrollment(payload) => {
            Message::PersistDuressEnrollment(payload)
        }
        app::message::Message::ConnectSignedOutBy(reason) => Message::ConnectSignedOutBy(reason),
        // The duress-disable confirmation toast (Issue 2). Home has no toast
        // surface; the Duress panel already reflects the disabled state in its
        // own fields, so swallow it quietly rather than warn.
        app::message::Message::View(app::view::Message::ShowSuccess(_)) => {
            Message::View(ViewMessage::Check)
        }
        _ => {
            log::warn!("[LAUNCHER] Unexpected message from ConnectAccountPanel");
            Message::View(ViewMessage::Check)
        }
    })
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Bubble successful preference writes to the GUI for every open tab.
    GlobalSettingsChanged,
    /// Flips the pulsing Connect light between its bright and dim phase.
    ConnectLightTick,
    /// Connect, not the user, ended the session (from this Launcher's account
    /// panel or relayed by the GUI from another tab).
    ConnectSignedOutBy(crate::app::SignedOutBy),
    GlobalFeatureLoaded(u64, bool),
    RecoveryServiceResolved {
        target: RecoveryServiceTarget,
        session_generation: u64,
        user_id: u32,
        request: u64,
        result: Result<RecoveryServiceDestination, String>,
    },
    View(ViewMessage),
    /// A focus refresh must not replace a form opened while the read ran.
    FocusedCubes {
        for_chain: crate::chain::ChainId,
        res: Result<State, String>,
    },
    /// Bubbles up to the pane on the auth-success edge so it can
    /// broadcast a session re-check to every open Cube tab. Carries
    /// no payload — the Cube tabs read the keyring themselves.
    ConnectSignedInBubble,
    /// Launch the installer. The trailing `Option<CoincubeClient>`
    /// forwards the home's already-authenticated Connect session
    /// into the installer so steps like `RecoveryKitRestoreStep` don't
    /// demand the user retype email + OTP a second time. `None` means
    /// "home had no Connect session" — the relevant installer step
    /// then falls back to its own auth form.
    Install(CoincubeDirectory, ChainId, UserFlow, Option<CoincubeClient>),
    /// Result of probing a chain directory. `for_chain` is the chain the probe
    /// was started for; a result that arrives after the user has switched
    /// networks is stale and must not replace the current list.
    Checked {
        for_chain: crate::chain::ChainId,
        res: Result<State, String>,
    },
    /// Open a Cube: the datadir, the GUI config read from the Cube's chain
    /// directory, the chain that directory belongs to, and the Cube. The tab
    /// checks that the Cube's own `network` agrees with the directory it was
    /// found in and that this build can run that chain before anything is
    /// unlocked or started.
    Run(
        CoincubeDirectory,
        app::config::Config,
        crate::chain::ChainId,
        CubeSettings,
    ),
    /// The same open, with an immutable floor origin attached before the
    /// asynchronous handoff. GUI validates it before retiring public owners.
    RunFromFloor(
        CoincubeDirectory,
        app::config::Config,
        crate::chain::ChainId,
        CubeSettings,
        u64,
    ),
    /// Open the selected local Cube through its normal unlock path, then show
    /// its existing Cube → Settings → Recovery management screen.
    RunToRecovery(
        CoincubeDirectory,
        app::config::Config,
        crate::chain::ChainId,
        CubeSettings,
    ),
    StartRecovery,
    CubeCreated(Result<CubeSettings, String>),
    /// Relay of `app::Message::CompleteDuressEnrollment` from the Connect panel
    /// hosted here. Home owns the datadir + Cube settings, so it persists the
    /// duress PIN + device code (the panel itself can't). Without this the
    /// enrollment would be silently dropped by `map_connect_task`.
    PersistDuressEnrollment(app::message::DuressEnrollmentPayload),
    /// Window ID extracted for passkey webview.
    PasskeyWindowId(iced_wry::ExtractedWindowId),
    /// Passkey webview manager update.
    PasskeyWebviewUpdate(iced_wry::IcedWryMessage),
    /// Passkey ceremony completed (registration or authentication).
    PasskeyCeremonyResult(Result<CeremonyOutcome, passkey_svc::PasskeyError>),
    /// Cancel an in-progress passkey ceremony.
    CancelPasskeyCeremony,
    /// Poll tick for native (macOS) passkey ceremony.
    NativePasskeyTick,
    /// Result of registering a cube with the remote Connect API.
    CubeRemoteRegistered {
        cube_id: String,
        network: crate::chain::ChainId,
        result: Result<CubeResponse, String>,
    },
    /// Catch-up sync finished.
    ///
    /// `Ok` carries one `(cube_id, outcome)` entry per Cube the round actually
    /// examined — `None` where it synced, `Some(reason)` where the server
    /// refused it. It deliberately speaks for *only* those Cubes: the round
    /// works from a snapshot of the unsynced set taken when it started, so a
    /// Cube created and refused while it was running is not in it and must keep
    /// the reason `CubeRemoteRegistered` recorded.
    ///
    /// `Err` means the round never got that far (the initial `list_cubes`
    /// failed), so it says nothing about any Cube at all and must not be read
    /// as "nothing is wrong".
    CatchUpSyncFinished(Result<Vec<(String, Option<String>)>, ()>),
    /// Result of fetching cube limits from the Connect API.
    CubeLimitsLoaded(Result<CubeLimitsResponse, String>),
    /// Result of updating a cube on the remote Connect API.
    CubeRemoteUpdated {
        cube_id: String,
        network: crate::chain::ChainId,
        result: Result<CubeResponse, String>,
    },
    /// Result of deleting a local cube's Connect backup.
    CubeBackupDeleted(Result<(), String>),
    /// Result of deleting a remote-only cube from the Connect API.
    RemoteCubeDeleted(Result<(), String>),
    /// Result of renaming a cube locally (settings file updated).
    CubeRenamed(Result<(), String>),
    /// Remote-only cubes (on server but not local) computed off the UI thread.
    /// Completion of a `fetch_remote_cubes`. `request_id` is the generation the
    /// fetch claimed; the handler drops anything that isn't the current one, so
    /// out-of-order completions and post-logout stragglers can't apply.
    RemoteCubesLoaded {
        request_id: u64,
        result: Result<Vec<RemoteCube>, String>,
    },
    /// Another network's Cubes, read without touching its directory.
    NetworkCatalogLoaded {
        generation: u64,
        chain: ChainId,
        cubes: Result<Vec<CubeSettings>, String>,
    },
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
    /// Acknowledge a locked native refusal and resume normal launcher policy.
    DismissNativeReturn,
    /// "Not now" on the Connect sheet's signed-out notice: clears the red
    /// light everywhere (the GUI relays it to every tab).
    DismissConnectSignOut,
    /// A message from the creation-time backup step. The payload is the
    /// Settings backup wizard's own message type: `State::CreationBackup`
    /// renders that wizard's views, and this variant is the `fn` mapper they
    /// are parameterised over. Variants that belong to the Settings flow only
    /// (the PIN re-entry gate, `Start`) are inert here — at creation the PIN
    /// was just typed and the seed is already in hand, so there is nothing to
    /// re-authenticate and nothing to decrypt.
    CreationBackup(BackupWalletMessage),
    /// "I'll do this later" — open the bypass screen.
    CreationBackupBypassRequested,
    /// "Back up now" — return to the choice screen from the bypass. The
    /// acknowledgement is reversible while the Cube is still unwritten.
    CreationBackupChoiceRequested,
    /// The bypass acknowledgement checkbox. Bypassing requires an active
    /// acceptance, not a dismissal.
    CreationBackupAcknowledgeBypass(bool),
    /// Accept the acknowledgement and finish creation with no backup. Records
    /// a `CreationBackupBypass` on the Cube so support can answer "did this
    /// user skip the backup?" from the datadir.
    CreationBackupBypassConfirmed,
    /// Leave the backup step without creating anything.
    CancelCreationBackup,
    /// Continue from the backup hub once the seed phrase, a Recovery Kit or
    /// both are done: write the Cube with that evidence.
    CreationBackupContinue,

    // ── Passkey creation: the Recovery Kit branch ────────────────────────
    /// "Create Recovery Kit" from [`CreationBackupStep::Choice`] — opens the
    /// recovery-password screen.
    CreationKitRequested,
    /// The Settings Kit flow's own message type. The creation flow renders that
    /// flow's password screen, and this is the `fn` mapper it is parameterised
    /// over, exactly as [`Self::CreationBackup`] is for the backup wizard.
    /// Variants belonging to Settings alone are inert here.
    CreationKit(RecoveryKitMessage),
    /// The Kit upload finished. `Ok` carries the registering account ID and
    /// evidence to persist on the Cube; `Err` returns the user to the password
    /// screen with the reason and **nothing written**.
    CreationKitUploaded(Result<(u32, creation_gate::CreationRecoveryKit), String>),
    ImportWallet,
    CreateWallet,
    /// W13 — launch the installer in "restore from Cube Recovery Kit"
    /// mode for a specific remote cube (payload is its `uuid`). The
    /// installer preselects the cube and skips the picker. This is the
    /// password-based recovery path.
    RestoreFromRecoveryKit(String),
    /// Import the ghost with this `uuid` from its recovery phrase: the
    /// recovery form, prefilled with its name, reclaims its identity once the
    /// phrase matches it (see [`ghost_import_identity`]).
    ImportGhostWithPhrase(String),
    /// Launch the installer in passwordless owner-keychain ("phone")
    /// restore mode for a specific remote cube. `cube_id` is the numeric
    /// Connect id; `full_cube` picks Full-Cube vs Vault-only scope. Home
    /// forwards this to `UserFlow::RecoverOwnCubeWithPhone`.
    RestoreWithPhone {
        cube_id: u64,
        full_cube: bool,
    },
    /// Open the recovery-method picker for a remote cube (payload is its
    /// `uuid`). Shown only when a cube has *both* a password Recovery Kit
    /// and a phone envelope; the user chooses which to use.
    ShowRecoveryMethodPicker(String),
    /// Dismiss the recovery-method picker without choosing.
    CloseRecoveryMethodPicker,
    ShowCreateCube(bool),
    /// Open or close the Create sheet's network menu.
    CreateNetworkMenu(bool),
    /// Recover with phone's Cube step: list every network's Cubes.
    ShowRestoreAllNetworks(bool),
    /// Recover with phone for a Cube on another network: switch to it first.
    RestoreWithPhoneOn {
        chain: ChainId,
        cube_id: u64,
        full_cube: bool,
    },
    ShowImportBitcoin,
    /// Import or restore's Overview: every way to bring a wallet or Cube in.
    ShowRestoreHub,
    /// Import or restore's Recover with phone path: the Connect Cubes a
    /// paired phone can recover.
    ShowRestorePhone,
    /// Import or restore's Recovery Kit path, with no Cube chosen yet.
    RestoreFromAnyRecoveryKit,
    CubeNameEdited(String),
    CreateCube,
    PinInput(pin_input::Message),
    PinConfirmInput(pin_input::Message),
    SelectNetwork(ChainId),
    StartInstall(ChainId),
    Check,
    Run(usize),
    RunCatalog(CubeCatalogKey),
    /// Open from the mounted floor details face, which can turn toward unlock.
    RunFloorCatalog(CubeCatalogKey),
    RenameCatalog(CubeCatalogKey),
    DeleteCatalog(CubeCatalogKey),
    /// List selection, search and focus. Presentation only; see
    /// [`launcher_catalog::CatalogMessage`].
    Catalog(launcher_catalog::CatalogMessage),
    DeleteCube(DeleteCubeMessage),
    ToggleRecoveryCheckBox,
    ToggleDeveloperMode(bool),
    ToggleBitcoinBlake2bBeta(bool),
    RecoveryWordInput {
        index: usize,
        word: String,
    },
    SelectRecoverySuggestion {
        index: usize,
        word: String,
    },
    SubmitRecovery,
    CancelRecovery,
    /// Open the rename modal for a cube at the given index.
    RenameCube(usize),
    /// Update the name input in the rename modal.
    RenameCubeNameEdited(String),
    /// Confirm the rename and persist it.
    RenameCubeConfirm,
    /// Cancel the rename modal.
    RenameCubeCancel,
    /// Navigate to a home section (Cubes or Connect submenu)
    GoToSection(HomeSection),
    /// Toggle the Connect sidebar section expand/collapse
    ToggleConnect,
    /// Account-level Connect messages (login, plan, security, etc.)
    ConnectAccount(ConnectAccountMessage),
    /// Account action from the GUI-owned sheet; retain the Launcher section.
    ConnectSheetAccount(ConnectAccountMessage),
    /// Heir "Recover a Vault" discovery-surface messages (COIN-377 / PR 1).
    RecoverVault(RecoverVaultMessage),
    /// Toggle light/dark theme
    ToggleTheme,
    /// Open the device-wide preferences sheet over the current screen.
    OpenAppPreferences,
    /// The dock's network activity source, bubbled to the GUI to save. The
    /// Launcher has no hide-balances eye; only open Cubes show one.
    ChangeDevicePreference(crate::app::settings::preferences::PreferenceChange),
    /// Toggle passkey mode for Cube creation (no PIN when enabled).
    TogglePasskeyMode(bool),
    /// Open a URL in the default browser
    OpenUrl(String),
    /// Acknowledge the one-time firmware-advisory incident notice. Persisted,
    /// so the notice never fires again on this install.
    DismissAdvisoryNotice,
}

#[derive(Debug, Clone)]
pub enum DeleteCubeMessage {
    ShowModal(usize),
    /// [`Self::ShowModal`] for a Cube on `chain`, once a list checked after
    /// this request has its row.
    ShowModalFor {
        cube_id: String,
        chain: ChainId,
    },
    ShowRemoteModal(String), // uuid of remote-only cube
    CloseModal,
    Confirm(String), // Cube ID
    DeleteConnectBackup(bool),
    Deleted,
    /// A daemon this Cube left stopping when it was locked has finished, so
    /// a confirmed delete can go ahead. Carries the Cube ID.
    DaemonStopped(String),
    PinInput(pin_input::Message),
    // Remote-only cube deletion
    ConfirmRemoteDelete(String), // uuid
    CloseRemoteModal,
    RemoteNameInput(String),
    /// Close the remote delete and start that Cube's restore instead.
    RestoreInstead(String), // uuid
}

struct DeleteCubeModal {
    cube: CubeSettings,
    network_directory: NetworkDirectory,
    wallet_settings: Option<WalletSettings>,
    warning: Option<DeleteError>,
    deleted: bool,
    /// Whether to also delete the cube from the Connect API (frees a cube slot).
    delete_connect_backup: bool,
    /// Whether the user is authenticated and the cube is synced (backup exists).
    can_delete_backup: bool,
    // `None` means we were not able to determine whether wallet uses internal bitcoind.
    internal_bitcoind: Option<bool>,
    /// Data root. The PIN is verified by decrypting this Cube's seed file, so
    /// the modal needs to reach `<root>/<network>/mnemonics/` — the
    /// `network_directory` above is already one level in.
    datadir_root: std::path::PathBuf,
    pin_input: pin_input::PinInput,
    pin_error: Option<String>,
    /// The PIN was accepted, and the delete waits for a daemon this Cube left
    /// stopping when it was locked.
    waiting_for_stop: bool,
    /// That daemon outlasted the wait, so nothing was deleted. Delete can be
    /// pressed again.
    stop_refused: bool,
}

/// Modal for deleting a remote-only cube (exists on server, not locally).
struct DeleteRemoteCubeModal {
    cube: RemoteCube,
    deleting: bool,
    error: Option<String>,
    /// The name typed to confirm deleting a Cube Connect could restore.
    typed_name: String,
}

fn delete_modal_focus(id: Id) -> ViewMessage {
    ViewMessage::Catalog(launcher_catalog::CatalogMessage::Focus(id))
}

fn focus_local_delete_opener(home: &Home, key: &CubeCatalogKey) -> Task<Message> {
    if !launcher_catalog::catalog_visible(home) {
        return Task::none();
    }
    let projection = launcher_catalog::project(home);
    let row = projection
        .locals
        .iter()
        .position(|row| row.key == *key && row.actionable);
    let mounted = row.is_some_and(|index| projection.selected == Some(index));
    if mounted {
        iced::widget::operation::focus(launcher_catalog::delete_id(key))
    } else {
        Task::none()
    }
}

fn focus_remote_delete_opener(home: &Home, uuid: &str) -> Task<Message> {
    if !launcher_catalog::catalog_visible(home) {
        return Task::none();
    }
    let projection = launcher_catalog::project(home);
    let mounted = !projection.floor_eligible(home.catalog.mode)
        && projection.remotes.iter().any(|cube| cube.uuid == uuid);
    if mounted {
        iced::widget::operation::focus(launcher_catalog::remote_delete_id(uuid))
    } else {
        Task::none()
    }
}

fn delete_modal_card(theme: &theme::Theme) -> iced::widget::container::Style {
    let mut style = theme::card::modal(theme);
    style.background = Some(iced::Background::Color(theme.colors.shell.panel));
    style.border.color = theme.colors.shell.line;
    style.border.width = 1.0;
    style.border.radius = 12.0.into();
    style
}

fn delete_modal_danger_text(theme: &theme::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme.colors.shell.bad),
    }
}

fn delete_modal_danger_button(
    theme: &theme::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    use iced::widget::button::Status;

    let bad = theme.colors.shell.bad;
    let mut style = theme::button::secondary(theme, status);
    style.border.color = bad;
    style.background = Some(iced::Background::Color(theme.colors.shell.panel));
    match status {
        Status::Active => style.text_color = bad,
        Status::Hovered | Status::Pressed => {
            style.background = Some(iced::Background::Color(bad));
            style.text_color = theme.colors.shell.background;
        }
        Status::Disabled => {
            style.text_color = iced::Color { a: 0.5, ..bad };
            style.border.color = iced::Color { a: 0.5, ..bad };
        }
    }
    style
}

const DELETE_LOCAL_CANCEL_ID: &str = "tenshu.delete-cube:cancel";
const DELETE_LOCAL_CONFIRM_ID: &str = "tenshu.delete-cube:confirm";
const DELETE_REMOTE_CANCEL_ID: &str = "tenshu.delete-remote-cube:cancel";
const DELETE_REMOTE_CONFIRM_ID: &str = "tenshu.delete-remote-cube:confirm";

const DELETE_REMOTE_NAME_ID: &str = "tenshu.delete-remote-cube:name";
const DELETE_REMOTE_RESTORE_ID: &str = "tenshu.delete-remote-cube:restore";

fn delete_modal_callout(theme: &theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.background)),
        border: iced::Border {
            color: theme.colors.shell.line,
            width: 1.0,
            radius: 10.0.into(),
        },
        ..Default::default()
    }
}

fn delete_modal_warn_icon(theme: &theme::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme.colors.shell.warn),
    }
}

impl DeleteRemoteCubeModal {
    /// Connect holds a way to restore this Cube here (the test the ghost's
    /// edge uses), so deleting it may take the Cube's only backup with it.
    fn restorable(&self) -> bool {
        launcher_catalog::remote_restore(&self.cube)
            .action
            .is_some()
    }

    /// Delete removes recorded backup material even when this build can't
    /// restore the whole Cube from it.
    fn holds_backup(&self) -> bool {
        self.cube.has_recovery_kit || self.cube.phone_recoverable
    }

    /// A Cube with a backup in Connect is confirmed by typing its name as the
    /// Launcher shows it; one with none needs only the button.
    fn name_confirmed(&self) -> bool {
        !self.holds_backup() || self.typed_name.trim() == catalog_display_name(&self.cube.name)
    }

    fn view(&self) -> Element<Message> {
        let display_name = catalog_display_name(&self.cube.name);
        let restorable = self.restorable();
        let holds_backup = self.holds_backup();
        let network = ChainId::from_api_str(&self.cube.network)
            .map(|chain| chain.label())
            .unwrap_or("Unknown network");
        let backup = localized_copy(match launcher_catalog::remote_recovery(&self.cube) {
            (true, _) => "Recovery Kit in Connect",
            (false, true) => "Phone recovery in Connect",
            (false, false) if self.cube.has_recovery_kit => "Recovery Kit has no Master Seed",
            (false, false) if self.cube.phone_recoverable => "Phone recovery in Connect",
            (false, false) => "No Recovery Kit in Connect",
        });

        let header = Row::new()
            .spacing(16)
            .align_y(Alignment::Center)
            .push(launcher_presenter::ghost_avatar(44.0))
            .push(
                Column::new()
                    .spacing(6)
                    .width(Length::Fill)
                    .push(h4_bold(format!(
                        "Delete \u{201c}{}\u{201d} from Connect?",
                        display_name
                    )))
                    .push(
                        p2_regular(format!("{} \u{b7} {}", network, backup))
                            .style(theme::text::secondary),
                    ),
            );

        let consequence = |line: &'static str| {
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(icon::dash_circle_icon().style(theme::text::secondary))
                .push(p1_regular(line))
        };
        let consequences = Column::new()
            .spacing(8)
            .push(consequence("Its Cube slot is freed"))
            .push_maybe(if restorable {
                Some(consequence(
                    "You can\u{2019}t restore it from Connect anymore",
                ))
            } else {
                holds_backup.then(|| consequence("Its backup in Connect is deleted"))
            })
            .push(consequence(
                "Its members, Lightning address and Vault monitoring end",
            ));

        let mut col = Column::new()
            .spacing(0)
            .push(header)
            .push(Space::new().height(24.0))
            .push(p1_regular(
                "It\u{2019}s only in your Connect account, not on this device. \
                 This can\u{2019}t be undone.",
            ))
            .push(Space::new().height(16.0))
            .push(iced::widget::rule::horizontal(1))
            .push(Space::new().height(14.0))
            .push(consequences);

        if holds_backup {
            // Only where this build can restore the Cube is there anything to
            // restore instead.
            let restore_btn = restorable.then(|| {
                button::secondary_compact_action(
                    DELETE_REMOTE_RESTORE_ID,
                    None,
                    "Restore instead",
                    delete_modal_focus,
                )
                .on_press_maybe((!self.deleting).then(|| {
                    ViewMessage::DeleteCube(DeleteCubeMessage::RestoreInstead(
                        self.cube.uuid.clone(),
                    ))
                }))
            });
            col = col.push(Space::new().height(18.0)).push(
                Container::new(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .push(icon::warning_icon().style(delete_modal_warn_icon))
                        .push(
                            Column::new()
                                .width(Length::Fill)
                                .spacing(2)
                                .push(p1_bold(localized_copy("Is Connect its only backup?")))
                                .push(p2_regular(localized_copy(if restorable {
                                    "Without its recovery phrase or another device, any funds \
                                     in this Cube will be lost. Restore it first if you\u{2019}re \
                                     not sure."
                                } else {
                                    "Without its recovery phrase or another device, any funds \
                                     in this Cube will be lost."
                                }))),
                        )
                        .push(restore_btn),
                )
                .padding([12, 14])
                .width(Length::Fill)
                .style(delete_modal_callout),
            );
        }

        col = col.push(Space::new().height(12.0)).push(
            p2_regular(localized_copy(
                "On another device too? Delete it there, or it registers again \
                 and takes the slot back.",
            ))
            .style(theme::text::secondary),
        );

        if holds_backup {
            use coincube_ui::widget::TextInput;
            let mut input = TextInput::new(&display_name, &self.typed_name)
                .id(DELETE_REMOTE_NAME_ID)
                .padding(12)
                .size(16);
            if !self.deleting {
                input = input
                    .on_input(|s| ViewMessage::DeleteCube(DeleteCubeMessage::RemoteNameInput(s)));
            }
            col = col
                .push(Space::new().height(20.0))
                .push(p1_medium(localized_format!(
                    "Type {} to confirm",
                    "Tipo {} para confirmar",
                    display_name
                )))
                .push(Space::new().height(8.0))
                .push(input);
        }

        if let Some(err) = &self.error {
            let message = match err.as_str() {
                "Cube not found on server" => {
                    "It\u{2019}s already gone from Connect. Another device may have deleted it."
                        .to_string()
                }
                "Not authenticated with Connect" => {
                    "You\u{2019}re signed out of Connect. Sign in again, then delete. \
                     Nothing was deleted."
                        .to_string()
                }
                other => other.to_string(),
            };
            col = col
                .push(Space::new().height(16.0))
                .push(notification::warning(message.clone(), message).width(Length::Fill));
        }

        let delete_btn = button::secondary_action(
            DELETE_REMOTE_CONFIRM_ID,
            None,
            if self.deleting {
                "Deleting\u{2026}"
            } else {
                "Delete from Connect"
            },
            delete_modal_focus,
        )
        .width(Length::Fixed(200.0))
        .style(delete_modal_danger_button)
        .on_press_maybe((!self.deleting && self.name_confirmed()).then(|| {
            ViewMessage::DeleteCube(DeleteCubeMessage::ConfirmRemoteDelete(
                self.cube.uuid.clone(),
            ))
        }));

        let cancel_btn = button::secondary_action(
            DELETE_REMOTE_CANCEL_ID,
            None,
            coincube_ui::locale::copy("Cancel"),
            delete_modal_focus,
        )
        .width(Length::Fixed(120.0))
        .on_press_maybe(
            (!self.deleting)
                .then_some(ViewMessage::DeleteCube(DeleteCubeMessage::CloseRemoteModal)),
        );

        col = col.push(Space::new().height(24.0)).push(
            Row::new()
                .spacing(10)
                .push(Space::new().width(Length::Fill))
                .push(cancel_btn)
                .push(delete_btn),
        );

        Into::<Element<ViewMessage>>::into(
            Container::new(col)
                .padding([32.0, 32.0])
                .width(Length::Fixed(560.0))
                .style(delete_modal_card),
        )
        .map(Message::View)
    }
}

/// Recovery-method picker for a remote cube that can be recovered by *both* a
/// password Recovery Kit and a phone (owner-keychain) envelope. Both restore
/// the same seed/descriptor content — the choice is purely which key unlocks
/// it. Shown only when both methods are available; a cube with a single method
/// launches that flow directly without this step.
struct RecoveryMethodModal {
    cube: RemoteCube,
}

impl RecoveryMethodModal {
    fn view(&self) -> Element<Message> {
        let display_name = catalog_display_name(&self.cube.name);
        let col = Column::new()
            .spacing(15)
            .padding(20)
            .width(Length::Fixed(460.0))
            .push(
                h4_bold(localized_format!(
                    "Recover \"{}\"",
                    "Recuperar \"{}\"",
                    display_name
                ))
                .width(Length::Fill),
            )
            .push(
                p1_regular(localized_copy(
                    "This Cube has two backups. Choose how to recover it — \
                     both restore the same wallet.",
                ))
                .style(theme::text::secondary),
            )
            .push(
                button::secondary(
                    Some(icon::key_icon()),
                    localized_copy("Recover with password"),
                )
                .width(Length::Fill)
                .on_press(ViewMessage::RestoreFromRecoveryKit(self.cube.uuid.clone())),
            )
            .push(
                button::secondary(
                    Some(icon::phone_icon()),
                    localized_copy("Recover with phone"),
                )
                .width(Length::Fill)
                .on_press(ViewMessage::RestoreWithPhone {
                    cube_id: self.cube.id,
                    full_cube: self.cube.phone_full_cube,
                }),
            )
            .push(
                button::secondary(Some(icon::cross_icon()), localized_copy("Cancel"))
                    .width(Length::Fill)
                    .on_press(ViewMessage::CloseRecoveryMethodPicker),
            );

        Into::<Element<ViewMessage>>::into(card::simple(col)).map(Message::View)
    }
}

impl DeleteCubeModal {
    fn new(
        cube: CubeSettings,
        network_directory: NetworkDirectory,
        datadir_root: std::path::PathBuf,
        wallet_settings: Option<WalletSettings>,
        internal_bitcoind: Option<bool>,
        is_authenticated: bool,
    ) -> Self {
        let can_delete_backup = is_authenticated && cube.remote_synced;
        Self {
            cube,
            wallet_settings,
            network_directory,
            datadir_root,
            warning: None,
            deleted: false,
            delete_connect_backup: false,
            can_delete_backup,
            internal_bitcoind,
            pin_input: pin_input::PinInput::new(),
            pin_error: None,
            waiting_for_stop: false,
            stop_refused: false,
        }
    }

    /// Directories whose daemon a delete must wait for: the Vault's, or an
    /// imported wallet's staged and final ones.
    fn daemon_directories(&self) -> Vec<std::path::PathBuf> {
        let mut directories = Vec::new();
        if let Some(wallet) = &self.wallet_settings {
            directories.push(
                self.network_directory
                    .coincubed_data_directory(&wallet.wallet_id())
                    .path()
                    .to_path_buf(),
            );
        }
        if let Some(import) = &self.cube.bitcoin_import {
            directories.push(
                self.network_directory
                    .path()
                    .join("data")
                    .join(format!(".import-{}", import.operation_id)),
            );
            if let Some(wallet) = import.attachment() {
                directories.push(
                    self.network_directory
                        .coincubed_data_directory(&wallet.wallet_id)
                        .path()
                        .to_path_buf(),
                );
            }
        }
        directories
    }

    /// The confirmed delete, once the PIN is accepted and no daemon of this
    /// Cube is still stopping.
    fn delete(&mut self) -> Task<Message> {
        // Never remove a directory a daemon may still write to, and never wait
        // for one here on the UI thread.
        if crate::daemon::stopping::any_stopping(&self.daemon_directories()) {
            self.stop_refused = true;
            return Task::none();
        }
        self.warning = None;

        // Pairing is identifying authorization material. Remove it
        // before deleting the Cube, while its exact identity is known.
        if let Err(error) =
            crate::app::local_mcp::remove_cube_pairings(&self.datadir_root, &self.cube)
        {
            self.warning = Some(DeleteError::Settings(settings::SettingsError::WritingFile(
                error.to_string(),
            )));
            return Task::none();
        }

        // Imported wallets have no Vault id. Their database and
        // journal belong to an exact Cube/import operation, so use
        // the ownership-checked removal path rather than the generic
        // wallet deletion below.
        if self.cube.bitcoin_import.is_some() {
            if let Err(error) =
                Handle::current().block_on(crate::services::bitcoin_import::remove_cube_import(
                    &self.network_directory,
                    &self.cube,
                ))
            {
                if error == crate::daemon::stopping::STILL_STOPPING {
                    self.stop_refused = true;
                } else {
                    self.warning = Some(DeleteError::Settings(
                        settings::SettingsError::WritingFile(error),
                    ));
                }
                return Task::none();
            }
            self.deleted = true;
            return Task::perform(async {}, |_| {
                Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::Deleted))
            });
        }

        // Delete vault if it exists
        if let Some(wallet_settings) = &self.wallet_settings {
            if let Err(e) =
                Handle::current().block_on(delete_wallet(&self.network_directory, wallet_settings))
            {
                if matches!(e, DeleteError::DaemonStopping) {
                    self.stop_refused = true;
                } else {
                    self.warning = Some(e);
                }
                return Task::none();
            }
        }

        // Delete the cube from settings
        let network_dir = self.network_directory.clone();
        let cube_id = self.cube.id.clone();
        if let Err(e) = Handle::current().block_on(async {
            settings::update_settings_file(&network_dir, |mut settings| {
                settings.cubes.retain(|cube| cube.id != cube_id);
                // Delete file if both cubes and wallets are empty
                if settings.cubes.is_empty() && settings.wallets.is_empty() {
                    None
                } else {
                    Some(settings)
                }
            })
            .await
        }) {
            self.warning = Some(DeleteError::Settings(e));
        } else {
            self.deleted = true;
            return Task::perform(async {}, |_| {
                Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::Deleted))
            });
        }
        Task::none()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::Confirm(cube_id))) => {
                if cube_id != self.cube.id {
                    return Task::none();
                }

                // Verify the PIN before proceeding with deletion. There is no
                // stored PIN hash any more: the check is a trial decryption of
                // this Cube's seed file, which costs ~831 ms. That is a
                // blocking call on the UI thread, consistent with the
                // `block_on(delete_wallet(...))` in `delete` below — this modal
                // has always been synchronous. A confirm dialog that takes a
                // beat to answer is acceptable; a cheap verifier next to an
                // expensive one is not (I1).
                if self.cube.has_pin(&self.datadir_root) {
                    let pin = self.pin_input.value();
                    let loc =
                        crate::services::unlock::CubeLocation::new(&self.datadir_root, &self.cube);
                    // The generic unlock refuses a Bitcoin Blake2b Cube before
                    // it reads the PIN (the chain is dormant to it), which
                    // left these Cubes undeletable behind an "update Tenshu"
                    // notice. Check their PIN with the fork's own trial, as
                    // the PIN screen does. Deleting runs nothing on the fork,
                    // so the PIN is the whole admission; the fork trial has no
                    // duress outcome, so a duress PIN is a wrong PIN here too.
                    let attempt = if self.cube.network.is_blake2b() {
                        crate::services::unlock::throttle::classify_connect_fork(
                            &self.datadir_root,
                            &loc,
                            &pin,
                        )
                    } else {
                        crate::services::unlock::throttle::classify_regular_pin(
                            &self.datadir_root,
                            &loc,
                            &pin,
                        )
                    };
                    match attempt {
                        Ok(crate::services::unlock::throttle::PinAttempt::Classified(
                            crate::services::unlock::PinOutcome::Unlock(_),
                        )) => {}
                        Ok(crate::services::unlock::throttle::PinAttempt::Locked(remaining)) => {
                            self.pin_error = Some(
                                crate::services::unlock::throttle::lockout_message(remaining),
                            );
                            self.pin_input.clear();
                            return Task::none();
                        }
                        // A duress PIN must not delete the Cube here — that
                        // would be a quieter, unlogged wipe than the duress
                        // path itself, and would confirm to an observer that
                        // the PIN meant something. Treat it as a wrong PIN.
                        Ok(_) => {
                            self.pin_error = Some("Incorrect PIN. Please try again.".to_string());
                            self.pin_input.clear();
                            return Task::none();
                        }
                        Err(e) => {
                            // Keystore failures are not wrong PINs (I7).
                            self.pin_error = Some(e.to_string());
                            self.pin_input.clear();
                            return Task::none();
                        }
                    }
                }

                // Deleting a Cube right after locking it: its daemon may still
                // be stopping in the directories removed below. Wait for that in
                // a task, not in `delete_wallet` on the UI thread.
                self.stop_refused = false;
                let directories = self.daemon_directories();
                if crate::daemon::stopping::any_stopping(&directories) {
                    self.waiting_for_stop = true;
                    return Task::perform(
                        async move {
                            // A wait that runs out is caught by `delete`,
                            // which refuses while the stop is registered.
                            for dir in directories {
                                if !crate::daemon::stopping::wait_for_async(dir).await {
                                    break;
                                }
                            }
                        },
                        move |_| {
                            Message::View(ViewMessage::DeleteCube(
                                DeleteCubeMessage::DaemonStopped(cube_id),
                            ))
                        },
                    );
                }
                return self.delete();
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::DaemonStopped(cube_id))) => {
                if cube_id == self.cube.id && self.waiting_for_stop {
                    self.waiting_for_stop = false;
                    return self.delete();
                }
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::DeleteConnectBackup(
                delete,
            ))) => {
                self.delete_connect_backup = delete;
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::PinInput(msg))) => {
                self.pin_error = None;
                return self.pin_input.update(msg).map(|m| {
                    Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::PinInput(m)))
                });
            }
            _ => {}
        }
        Task::none()
    }

    fn view(&self) -> Element<Message> {
        let pin_ready = !self.cube.has_pin(&self.datadir_root) || self.pin_input.is_complete();
        let can_delete = pin_ready && self.warning.is_none() && !self.waiting_for_stop;
        let display_name = catalog_display_name(&self.cube.name);
        let mut confirm_button = button::secondary_action(
            DELETE_LOCAL_CONFIRM_ID,
            None,
            coincube_ui::locale::copy("Delete Cube"),
            delete_modal_focus,
        )
        .width(Length::Fixed(200.0))
        .style(delete_modal_danger_button);
        if can_delete {
            confirm_button = confirm_button.on_press(ViewMessage::DeleteCube(
                DeleteCubeMessage::Confirm(self.cube.id.clone()),
            ));
        }

        // Determine what's being deleted
        let has_vault = self.wallet_settings.is_some();
        let legacy_backend = self
            .wallet_settings
            .as_ref()
            .and_then(|w| w.remote_backend_auth.as_ref());

        let help_text_1 = if has_vault {
            format!(
                "Are you sure you want to delete the Cube \"{}\" and {}?",
                display_name,
                if legacy_backend.is_some() {
                    "its associated Vault configuration"
                } else {
                    "all its associated data (including Vault)"
                }
            )
        } else {
            format!(
                "Are you sure you want to delete the Cube \"{}\"?",
                display_name
            )
        };

        let help_text_2 = match self.internal_bitcoind {
            Some(true) => Some("(The Tenshu-managed Bitcoin node for this network will not be affected by this action.)"),
            Some(false) => None,
            None if has_vault => Some("(If you are using a Tenshu-managed Bitcoin node, it will not be affected by this action.)"),
            _ => None,
        };
        let help_text_3 = "WARNING: This cannot be undone.";

        let mut col = Column::new()
            .spacing(10)
            .push(Container::new(
                h4_bold(localized_format!(
                    "Delete Cube \"{}\"",
                    "Borrar Cube \"{}\"",
                    display_name
                ))
                .style(delete_modal_danger_text)
                .width(Length::Fill),
            ))
            .push(Row::new().push(text(help_text_1)))
            .push(help_text_2.map(|t| Row::new().push(p1_regular(t).style(theme::text::secondary))))
            // A legacy Liana Connect Vault still lives on that backend; say so
            // before the user removes the only copy COINCUBE can show them.
            .push(legacy_backend.map(|auth| {
                Row::new()
                    .push(p1_regular(auth.retired_backend_notice()).style(theme::text::warning))
            }))
            .push(Row::new())
            .push(Row::new().push(text(help_text_3)));

        // Option to also delete the Connect API backup (only when relevant)
        if self.can_delete_backup {
            col = col.push(Space::new().height(Length::Fixed(5.0))).push(
                CheckBox::new(self.delete_connect_backup)
                    .label(localized_copy(
                        "Also delete Connect backup (frees a Cube slot)",
                    ))
                    .on_toggle(|checked| {
                        ViewMessage::DeleteCube(DeleteCubeMessage::DeleteConnectBackup(checked))
                    }),
            );
            if self.delete_connect_backup {
                col = col.push(
                    p1_regular(localized_copy(
                        "The Connect backup will be permanently deleted. \
                         This frees a Cube slot on your account.",
                    ))
                    .style(theme::text::warning),
                );
            }
        }

        // PIN entry section
        if self.cube.has_pin(&self.datadir_root) {
            col = col
                .push(Space::new().height(Length::Fixed(5.0)))
                .push(
                    p1_regular(localized_copy("Enter your PIN to confirm:"))
                        .style(theme::text::secondary),
                )
                .push(
                    Container::new(
                        self.pin_input
                            .view()
                            .map(|m| ViewMessage::DeleteCube(DeleteCubeMessage::PinInput(m))),
                    )
                    .center_x(Length::Fill),
                );
            if let Some(err) = &self.pin_error {
                col = col.push(
                    Container::new(
                        p1_regular(coincube_ui::locale::notice(err.to_owned()))
                            .style(theme::text::error),
                    )
                    .center_x(Length::Fill),
                );
            }
        }

        if self.stop_refused {
            col = col.push(
                Container::new(
                    p1_regular(crate::daemon::stopping::STILL_STOPPING).style(theme::text::warning),
                )
                .center_x(Length::Fill),
            );
        }
        if self.waiting_for_stop {
            col = col.push(
                Container::new(
                    p1_regular(localized_copy(
                        "Finishing locking this Cube before deleting it…",
                    ))
                    .style(theme::text::secondary),
                )
                .center_x(Length::Fill),
            );
        }

        col = col
            .push(
                self.warning.as_ref().map(|w| {
                    notification::warning(w.to_string(), w.to_string()).width(Length::Fill)
                }),
            )
            .push(
                Container::new(if !self.deleted {
                    Row::new()
                        .spacing(10)
                        .push(
                            button::secondary_action(
                                DELETE_LOCAL_CANCEL_ID,
                                None,
                                coincube_ui::locale::copy("Cancel"),
                                delete_modal_focus,
                            )
                            .width(Length::Fixed(120.0))
                            .on_press(ViewMessage::DeleteCube(DeleteCubeMessage::CloseModal)),
                        )
                        .push(confirm_button)
                } else {
                    Row::new()
                        .spacing(10)
                        .push(icon::square_check_icon().style(theme::text::success))
                        .push(
                            text(localized_copy("Cube successfully deleted"))
                                .style(theme::text::success),
                        )
                })
                .align_x(Horizontal::Center)
                .width(Length::Fill),
            );

        Into::<Element<ViewMessage>>::into(
            Container::new(col)
                .padding([28.0, 32.0])
                .width(Length::Fixed(700.0))
                .style(delete_modal_card),
        )
        .map(Message::View)
    }
}

/// Probe the selected network's directory and report through
/// [`Message::Checked`], tagged with the chain it was started for.
fn probe_network_datadir<C: Into<crate::chain::ChainId>>(
    network: C,
    dir: NetworkDirectory,
) -> Task<Message> {
    let for_chain = network.into();
    Task::perform(check_network_datadir(for_chain, dir), move |res| {
        Message::Checked { for_chain, res }
    })
}

/// Another network's Cubes for the Launcher's all-network view. Strictly
/// read-only: unlike [`check_network_datadir`] it creates no directory or
/// config, so browsing never writes to a network. Records that claim another
/// chain are left out, as the open path would refuse them.
async fn read_network_catalog(
    chain: ChainId,
    dir: NetworkDirectory,
) -> Result<Vec<CubeSettings>, String> {
    if !dir.path().join(settings::SETTINGS_FILE_NAME).exists() {
        return Ok(Vec::new());
    }
    match settings::Settings::from_file(&dir) {
        Ok(settings) => Ok(settings
            .cubes
            .into_iter()
            .filter(|cube| cube.network == chain)
            .collect()),
        Err(e) => {
            tracing::warn!("Couldn't read the {} Cube list: {}", chain, e);
            Err(e.to_string())
        }
    }
}

/// Probe `path` — the directory of `source` — and load the Cubes stored there.
/// The returned [`State::Cubes`] carries `source` so nothing downstream has to
/// trust a record's own `network` field about where it lives.
async fn check_network_datadir(
    source: crate::chain::ChainId,
    path: NetworkDirectory,
) -> Result<State, String> {
    // Fork discovery is read-only. Admission, not browsing, creates its files.
    if source.is_blake2b() {
        if !path.path().join(settings::SETTINGS_FILE_NAME).exists() {
            return Ok(State::NoCube { create_cube: false });
        }
        let settings = settings::Settings::from_file(&path).map_err(|e| e.to_string())?;
        return Ok(State::Cubes {
            cubes: settings.cubes,
            create_cube: false,
            source,
        });
    }
    // Ensure the network directory exists
    if let Err(e) = tokio::fs::create_dir_all(path.path()).await {
        return Err(format!(
            "Failed to create directory {}: {}",
            path.path().to_string_lossy(),
            e
        ));
    }

    let mut config_path = path.clone().path().to_path_buf();
    config_path.push(app::config::DEFAULT_FILE_NAME);

    if let Err(e) = app::Config::from_file(&config_path) {
        if e == app::config::ConfigError::NotFound {
            // Create default config file
            let default_config = app::Config::new(false);
            if let Err(e) = default_config.to_file(&config_path) {
                return Err(format!("Failed to create default GUI config file: {}", e));
            }
            return Ok(State::NoCube { create_cube: false });
        } else {
            return Err(format!(
                "Failed to read GUI configuration file in the directory: {}",
                path.path().to_string_lossy()
            ));
        }
    }

    let mut daemon_config_path = path.clone().path().to_path_buf();
    daemon_config_path.push("daemon.toml");

    if daemon_config_path.exists() {
        coincubed::config::Config::from_file(Some(daemon_config_path.clone())).map_err(
            |e| match e {
                ConfigError::FileNotFound | ConfigError::DatadirNotFound => {
                    format!(
                        "Failed to read daemon configuration file in the directory: {}",
                        daemon_config_path.to_string_lossy()
                    )
                }
                ConfigError::ReadingFile(e) => {
                    if e.starts_with("Parsing configuration file: Error parsing descriptor") {
                        "There is an issue with the configuration for this network. You most likely use a descriptor containing one or more public key(s) without origin.".to_string()
                    } else {
                        format!(
                            "Failed to read daemon configuration file in the directory: {}",
                            daemon_config_path.to_string_lossy()
                        )
                    }
                }
                ConfigError::UnexpectedDescriptor(_) => {
                    "There is an issue with the configuration for this network. You most likely use a descriptor containing one or more public key(s) without origin.".to_string()
                }
                ConfigError::Unexpected(e) => {
                    format!("Unexpected {}", e)
                }
            },
        )?;
    }

    // Try to load cubes from settings
    match settings::Settings::from_file(&path) {
        Ok(s) => {
            // All cubes are required to have PINs
            if s.cubes.is_empty() {
                Ok(State::NoCube { create_cube: false })
            } else {
                Ok(State::Cubes {
                    cubes: s.cubes,
                    create_cube: false,
                    source,
                })
            }
        }
        Err(settings::SettingsError::NotFound) => Ok(State::NoCube { create_cube: false }),
        Err(e) => Err(format!("Failed to read settings: {}", e)),
    }
}

#[cfg(all(test, feature = "mcp-proposals"))]
mod native_return_tests;

#[cfg(test)]
mod network_catalog_tests {
    use super::*;

    #[tokio::test]
    async fn reading_another_network_never_writes_to_it() {
        let root = std::env::temp_dir().join(format!("catalog-{}", uuid::Uuid::new_v4()));
        let datadir = CoincubeDirectory::new(root.clone());
        let dir = datadir.network_directory(ChainId::Testnet4);
        assert!(read_network_catalog(ChainId::Testnet4, dir.clone())
            .await
            .unwrap()
            .is_empty());
        assert!(
            !dir.path().exists(),
            "browsing must not create the network's directory"
        );

        std::fs::create_dir_all(dir.path()).unwrap();
        let settings = settings::Settings {
            cubes: vec![
                CubeSettings::new_with_raw_id("t1".into(), "Lab".into(), ChainId::Testnet4),
                // A record claiming another chain is left out, as opening would.
                CubeSettings::new_with_raw_id("x".into(), "Stray".into(), ChainId::Bitcoin),
            ],
            ..Default::default()
        };
        std::fs::write(
            dir.path().join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings).unwrap(),
        )
        .unwrap();
        let cubes = read_network_catalog(ChainId::Testnet4, dir.clone())
            .await
            .unwrap();
        assert_eq!(
            cubes.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            ["t1"]
        );
        std::fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::{app::state::connect::ConnectFlowStep, services::coincube::User};

    fn owner_self(has_recipient: bool, tier: &str, kinds: &[&str]) -> OwnerSelfRecoverySummary {
        OwnerSelfRecoverySummary {
            has_recipient,
            tier: tier.to_string(),
            envelope_kinds: kinds.iter().map(|k| k.to_string()).collect(),
            updated_at: None,
        }
    }

    fn test_datadir() -> CoincubeDirectory {
        CoincubeDirectory::new(PathBuf::new())
    }

    /// D2 (`plans/PLAN-vault-identity-unification.md`): the Cubes list shows
    /// the vault's *fingerprint* — the same id as Vault settings, the pairing
    /// QR, and Keychain — and shows **no** id rather than falling back to the
    /// descriptor checksum when it isn't known yet. The checksum fallback
    /// (`Vault: Coincube-njhdtwde`) is the bug this replaced: it rendered a
    /// local storage key as though it identified the vault.
    #[test]
    fn the_cubes_list_shows_the_vault_fingerprint_and_never_the_checksum() {
        use crate::app::settings::{VaultIdentity, WalletId};

        let no_vault = CubeSettings::new("Empty".to_string(), Network::Bitcoin);
        assert_eq!(vault_subtitle(&no_vault), "No Vault configured");

        // A Cube attached before `vault_fingerprint` existed: known Vault,
        // unknown id, until the backfill runs on its first open.
        let checksum = "njhdtwde".to_string();
        let pre_backfill =
            CubeSettings::new("Legacy".to_string(), Network::Bitcoin).with_vault(VaultIdentity {
                wallet_id: WalletId::new(checksum.clone(), Some(1)),
                fingerprint: None,
            });
        let subtitle = vault_subtitle(&pre_backfill);
        assert_eq!(subtitle, "Vault configured");
        assert!(
            !subtitle.contains(&checksum),
            "the descriptor checksum is a storage key and must never be shown as a vault id"
        );

        let identified =
            CubeSettings::new("Family".to_string(), Network::Bitcoin).with_vault(VaultIdentity {
                wallet_id: WalletId::new(checksum.clone(), Some(1)),
                fingerprint: Some("8099ee80".to_string()),
            });
        let subtitle = vault_subtitle(&identified);
        assert_eq!(subtitle, "Vault 8099ee80");
        assert!(!subtitle.contains(&checksum));
    }

    fn home() -> Home {
        Home::new(test_datadir(), Some(Network::Bitcoin)).0
    }

    /// A restore that left from the installer's path list opens that path
    /// over the checked list, through the path's own message, and only once.
    #[test]
    fn a_restore_left_from_its_path_list_opens_that_path() {
        use crate::installer::RestoreDestination;
        for (destination, opens) in [
            (RestoreDestination::Overview, "ShowRestoreHub"),
            (RestoreDestination::ImportBitcoin, "ShowImportBitcoin"),
            (RestoreDestination::Kit, "RestoreFromAnyRecoveryKit"),
            (RestoreDestination::Phone, "ShowRestorePhone"),
        ] {
            let mut home = home();
            home.open_restore_destination(destination);
            let checked = || Message::Checked {
                for_chain: ChainId::Bitcoin,
                res: Ok(State::NoCube { create_cube: false }),
            };
            let next = drain(home.update(checked()));
            let opened: Vec<_> = next
                .iter()
                .filter_map(|message| match message {
                    Message::View(view) => Some(format!("{view:?}")),
                    _ => None,
                })
                .collect();
            assert_eq!(opened, [opens], "{:?}", destination);
            assert!(drain(home.update(checked()))
                .iter()
                .all(|message| !matches!(message, Message::View(_))));
        }

        let mut home = home();
        home.open_restore_destination(RestoreDestination::Overview);
        for message in drain(home.update(Message::Checked {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::NoCube { create_cube: false }),
        })) {
            drop(home.update(message));
        }
        assert!(home.restore_hub && home.showing_create_form());
    }

    #[test]
    fn blake2b_create_goes_through_the_cube_journey() {
        let mut home = home();
        enable_btcb2(&mut home);
        home.network = ChainId::BitcoinBlake2b;
        home.state = State::NoCube { create_cube: true };

        // The sheet opens on the Cube step, as on Bitcoin. Nothing starts
        // the Vault installer: the Vault is created inside the Cube.
        let next = drain(home.update(Message::View(ViewMessage::CreateNetworkMenu(true))));
        assert!(!next
            .iter()
            .any(|message| matches!(message, Message::Install(..))));
        assert!(home.create_network_menu);
        assert!(home.error.is_none());

        drop(home.update(Message::View(ViewMessage::CubeNameEdited("Fork".into()))));
        assert_eq!(home.create_cube_name.value, "Fork");
        drop(home.update(Message::View(ViewMessage::TogglePasskeyMode(false))));
        assert!(
            home.error.is_none(),
            "PIN or passkey is a Cube choice here too"
        );

        // An incomplete Cube form creates nothing and installs nothing.
        assert!(!drain(home.update(Message::View(ViewMessage::CreateCube)))
            .iter()
            .any(|message| matches!(message, Message::Install(..))));

        // The fresh-fork installer route is gone from Home.
        assert!(
            !drain(home.update(Message::View(ViewMessage::CreateWallet)))
                .iter()
                .any(|message| matches!(message, Message::Install(..)))
        );
    }

    #[test]
    fn a_late_network_probe_preserves_creation_opened_after_network_selection() {
        for with_cubes in [false, true] {
            let mut home = home();
            home.developer_mode = true;
            home.state = State::NoCube { create_cube: false };
            drop(home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Signet))));
            assert!(!home.create_network_selection_pending);
            drop(home.update(Message::View(ViewMessage::ShowCreateCube(true))));
            drop(home.update(Message::View(ViewMessage::CubeNameEdited("Travel".into()))));
            let checked = || Message::Checked {
                for_chain: ChainId::Signet,
                res: Ok(if with_cubes {
                    State::Cubes {
                        cubes: vec![CubeSettings::new("Existing".into(), ChainId::Signet)],
                        create_cube: false,
                        source: ChainId::Signet,
                    }
                } else {
                    State::NoCube { create_cube: false }
                }),
            };
            drop(home.update(checked()));
            assert!(
                home.showing_create_form(),
                "late probe hid explicit creation"
            );
            assert_eq!(home.create_cube_name.value, "Travel");
            assert!(!home.create_network_selection_pending);

            drop(home.update(Message::View(ViewMessage::ShowCreateCube(false))));
            drop(home.update(checked()));
            assert!(
                !home.showing_create_form(),
                "late probe reopened cancelled creation"
            );
            assert!(launcher_catalog::catalog_visible(&home));
        }
    }

    #[test]
    fn create_network_probe_keeps_the_form_after_stale_result() {
        let mut home = home();
        home.developer_mode = true;
        home.state = State::Cubes {
            cubes: vec![CubeSettings::new("Existing".into(), ChainId::Bitcoin)],
            create_cube: true,
            source: ChainId::Bitcoin,
        };
        drop(home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Signet))));
        assert_eq!(home.network, ChainId::Signet);
        assert!(home.create_network_selection_pending);

        drop(home.update(Message::Checked {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::NoCube { create_cube: false }),
        }));
        assert!(home.create_network_selection_pending);

        drop(home.update(Message::Checked {
            for_chain: ChainId::Signet,
            res: Ok(State::Cubes {
                cubes: vec![CubeSettings::new("Signet cube".into(), ChainId::Signet)],
                create_cube: false,
                source: ChainId::Signet,
            }),
        }));
        assert!(matches!(
            home.state,
            State::Cubes {
                create_cube: true,
                source: ChainId::Signet,
                ..
            }
        ));
        assert!(!home.create_network_selection_pending);
    }

    #[test]
    fn create_journey_actions_route_focus_through_catalog() {
        let id = Id::new("tenshu.create:network-continue");
        assert!(matches!(
            create_journey_focus(id.clone()),
            ViewMessage::Catalog(launcher_catalog::CatalogMessage::Focus(target)) if target == id
        ));
        // The Action widget owns Enter/Space and disabled-action behavior;
        // its keyboard tests exercise those semantics in coincube-ui.
    }

    #[test]
    fn create_network_menu_opens_on_the_cube_step_only() {
        let mut home = home();
        home.state = State::NoCube { create_cube: true };
        drop(home.update(Message::View(ViewMessage::CreateNetworkMenu(true))));
        assert!(home.create_network_menu);
        // Closing returns focus to the header's network control.
        let close_messages =
            drain(home.update(Message::View(ViewMessage::CreateNetworkMenu(false))));
        assert!(!home.create_network_menu);
        assert!(matches!(
            close_messages.as_slice(),
            [Message::View(ViewMessage::Catalog(launcher_catalog::CatalogMessage::Focus(id)))]
                if id == &Id::new(restore_frame::NETWORK_CONTROL_ID)
        ));

        // Once the Cube is being written the chip is static.
        home.creating_cube = true;
        drop(home.update(Message::View(ViewMessage::CreateNetworkMenu(true))));
        assert!(!home.create_network_menu);
        home.creating_cube = false;

        // Import or restore's Overview re-filters on a change, but an
        // import under way keeps its network.
        home.restore_hub = true;
        drop(home.update(Message::View(ViewMessage::CreateNetworkMenu(true))));
        assert!(home.create_network_menu);
        drop(home.update(Message::View(ViewMessage::CreateNetworkMenu(false))));
        home.restore_hub = false;
        home.pending_bitcoin_import = Some(Box::new(BitcoinImport::pending()));
        drop(home.update(Message::View(ViewMessage::CreateNetworkMenu(true))));
        assert!(!home.create_network_menu);
    }

    #[test]
    fn selected_create_network_remains_keyboard_actionable_without_reprobing() {
        assert!(matches!(
            create_network_row_action(ChainId::Bitcoin, true, true),
            Some(ViewMessage::CreateNetworkMenu(false))
        ));
        assert!(matches!(
            create_network_row_action(ChainId::Signet, true, false),
            Some(ViewMessage::SelectNetwork(ChainId::Signet))
        ));
        assert!(create_network_row_action(ChainId::Bitcoin, false, true).is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn selected_create_network_is_in_rendered_tab_order() {
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::operation::{self, focusable, Outcome},
            widget::{Operation, Tree},
            Layout,
        };

        #[derive(Default)]
        struct Focusables {
            actions: Vec<Id>,
            scrolls: Vec<Id>,
        }
        impl Operation for Focusables {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn focusable(
                &mut self,
                id: Option<&Id>,
                _: iced::Rectangle,
                _: &mut dyn iced::advanced::widget::operation::Focusable,
            ) {
                if let Some(id) = id {
                    self.actions.push(id.clone());
                }
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                _: iced::Rectangle,
                _: iced::Rectangle,
                _: iced::Vector,
                _: &mut dyn iced::advanced::widget::operation::Scrollable,
            ) {
                if let Some(id) = id {
                    self.scrolls.push(id.clone());
                }
            }
        }

        let mut home = home();
        home.state = State::NoCube { create_cube: true };
        home.developer_mode = true;
        home.refresh_displayed_networks();
        home.create_network_menu = true;
        let mut element = home.create_journey_view();
        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1024.0, 768.0)),
        );
        let mut focusables = Focusables::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut focusables);
        assert!(focusables
            .actions
            .contains(&Id::new("tenshu.create:network:mainnet")));
        assert!(focusables.scrolls.contains(&Id::new(CREATE_BODY_SCROLL_ID)));

        let focused_id = create_network_row_id(ChainId::Bitcoin);
        let mut focus: Box<dyn Operation<()>> = Box::new(focusable::focus(focused_id.clone()));
        loop {
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut operation::black_box(focus.as_mut()),
            );
            match focus.finish() {
                Outcome::Chain(next) => focus = next,
                Outcome::None | Outcome::Some(()) => break,
            }
        }
        let mut before: Box<dyn Operation<Id>> = Box::new(focusable::find_focused());
        let initially_focused = loop {
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut operation::black_box(before.as_mut()),
            );
            match before.finish() {
                Outcome::None => break None,
                Outcome::Some(id) => break Some(id),
                Outcome::Chain(next) => before = next,
            }
        };
        assert_eq!(initially_focused, Some(focused_id.clone()));
        drop(element);

        // A switch across live/test groups changes the header's tag, but the
        // open menu stays at the same tree index and retains row focus.
        home.network = ChainId::Testnet4;
        let mut element = home.create_journey_view();
        tree.diff(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1024.0, 768.0)),
        );
        let mut after_focusables = Focusables::default();
        element.as_widget_mut().operate(
            &mut tree,
            Layout::new(&node),
            &renderer,
            &mut after_focusables,
        );
        assert!(after_focusables.actions.contains(&focused_id));
        let mut find: Box<dyn Operation<Id>> = Box::new(focusable::find_focused());
        let restored = loop {
            element.as_widget_mut().operate(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &mut operation::black_box(find.as_mut()),
            );
            match find.finish() {
                Outcome::None => break None,
                Outcome::Some(id) => break Some(id),
                Outcome::Chain(next) => find = next,
            }
        };
        assert_eq!(restored, Some(focused_id));
    }

    #[test]
    fn empty_home_waits_for_explicit_creation_after_probe() {
        let mut home = home();
        assert!(matches!(home.state, State::Unchecked));
        assert!(!home.showing_create_journey());

        home.state = State::NoCube { create_cube: false };
        assert!(!home.showing_create_journey());
        assert!(launcher_catalog::catalog_visible(&home));
        drop(home.update(Message::View(ViewMessage::ShowCreateCube(true))));
        assert!(home.showing_create_journey());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unchecked_home_renders_loading_without_creation_actions() {
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{Id, Operation, Tree},
            Layout,
        };

        #[derive(Default)]
        struct Labels(Vec<String>);
        impl Operation for Labels {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, text: &str) {
                self.0.push(text.to_owned());
            }
        }

        let mut home = home();
        home.advisory_notice = None;
        assert!(matches!(home.state, State::Unchecked));
        let mut element = home.view();
        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1024.0, 768.0)),
        );
        let mut labels = Labels::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
        assert!(labels.0.iter().any(|label| label == "Checking for Cubes"));
        assert!(!labels
            .0
            .iter()
            .any(|label| label == "Create Cube" || label == "Create a new Cube"));

        drop(element);
        home.active_section = HomeSection::Settings;
        let mut settings = home.view();
        let mut tree = Tree::new(settings.as_widget());
        let node = settings.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1024.0, 768.0)),
        );
        let mut labels = Labels::default();
        settings
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
        assert!(labels.0.iter().any(|label| label == "Global Settings"));
        assert!(!labels.0.iter().any(|label| label == "Checking for Cubes"));
    }

    #[test]
    fn in_flight_create_cannot_leave_and_clear_its_name() {
        let mut home = home();
        home.state = State::Cubes {
            cubes: vec![CubeSettings::new("Existing".into(), ChainId::Bitcoin)],
            create_cube: true,
            source: ChainId::Bitcoin,
        };
        home.create_cube_name.value = "Travel".into();
        home.creating_cube = true;
        assert!(!home.can_leave_create_journey());

        drop(home.update(Message::View(ViewMessage::ShowCreateCube(false))));
        assert!(matches!(
            home.state,
            State::Cubes {
                create_cube: true,
                ..
            }
        ));
        assert_eq!(home.create_cube_name.value, "Travel");

        home.creating_cube = false;
        assert!(home.can_leave_create_journey());
    }

    #[test]
    fn create_journey_footer_preserves_name_pin_and_busy_gates() {
        let mut home = home();
        assert!(!create_cube_can_submit(
            &home.create_cube_name,
            &home.create_cube_pin,
            &home.create_cube_pin_confirm,
            false,
            false,
        ));
        home.create_cube_name.value = "Travel".into();
        home.create_cube_name.valid = true;
        assert!(!create_cube_can_submit(
            &home.create_cube_name,
            &home.create_cube_pin,
            &home.create_cube_pin_confirm,
            false,
            false,
        ));
        assert!(create_cube_can_submit(
            &home.create_cube_name,
            &home.create_cube_pin,
            &home.create_cube_pin_confirm,
            true,
            false,
        ));
        assert!(!create_cube_can_submit(
            &home.create_cube_name,
            &home.create_cube_pin,
            &home.create_cube_pin_confirm,
            true,
            true,
        ));
    }

    #[test]
    fn create_pin_match_is_checked_while_typing() {
        fn pin(digits: [&str; 4]) -> pin_input::PinInput {
            let mut input = pin_input::PinInput::new();
            input.digits = digits.map(str::to_string);
            input
        }
        let entered = pin(["2", "4", "2", "1"]);

        // Nothing to compare until the first PIN is complete.
        assert_eq!(
            create_cube_pin_match(&pin(["2", "4", "", ""]), &pin(["9", "", "", ""])),
            CreatePinMatch::Pending
        );
        // A confirmation that agrees so far is not yet a result.
        assert_eq!(
            create_cube_pin_match(&entered, &pin(["2", "4", "", ""])),
            CreatePinMatch::Pending
        );
        // The first differing digit is a mismatch, before the field is full.
        assert_eq!(
            create_cube_pin_match(&entered, &pin(["2", "4", "1", ""])),
            CreatePinMatch::Mismatch
        );
        assert_eq!(
            create_cube_pin_match(&entered, &pin(["2", "4", "1", "7"])),
            CreatePinMatch::Mismatch
        );
        assert_eq!(
            create_cube_pin_match(&entered, &pin(["2", "4", "2", "1"])),
            CreatePinMatch::Match
        );

        // Create Cube stays disabled until the PINs match.
        let mut home = home();
        home.create_cube_name.value = "Travel".into();
        home.create_cube_name.valid = true;
        let mismatched = pin(["2", "4", "1", "7"]);
        assert!(!create_cube_can_submit(
            &home.create_cube_name,
            &entered,
            &mismatched,
            false,
            false,
        ));
        assert!(create_cube_can_submit(
            &home.create_cube_name,
            &entered,
            &pin(["2", "4", "2", "1"]),
            false,
            false,
        ));
    }

    /// Synthetic Create frames for comparing the minimum window with P-Create.
    #[test]
    #[ignore = "writes PNG frames; needs a headless renderer"]
    fn create_journey_capture_1024() {
        use iced::advanced::{
            layout, mouse, renderer::Headless, widget::Tree, Layout, Renderer as _, Shell,
        };

        let Ok(directory) = std::env::var("CREATE_CAPTURE_DIR") else {
            return;
        };
        std::fs::create_dir_all(&directory).unwrap();
        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("wgpu"),
        ))
        .expect("headless renderer");
        let size = iced::Size::new(1024.0, 768.0);

        for light in [false, true] {
            let appearance = if light { "light" } else { "dark" };
            let palette = if light {
                theme::Theme::light()
            } else {
                theme::Theme::dark()
            };
            let mut home = home();
            home.state = State::NoCube { create_cube: true };
            home.developer_mode = true;
            home.displayed_networks = vec![
                ChainId::Bitcoin,
                ChainId::Testnet4,
                ChainId::Signet,
                ChainId::Regtest,
            ];
            home.theme_mode = if light {
                coincube_ui::theme::palette::ThemeMode::Light
            } else {
                coincube_ui::theme::palette::ThemeMode::Dark
            };
            let words: Vec<String> = MasterSigner::generate(Network::Bitcoin)
                .unwrap()
                .words()
                .iter()
                .map(|w| w.to_string())
                .collect();
            home.creation_backup_words = Some(zeroize::Zeroizing::new(words));
            for (name, chain, confirmed, bottom, backup) in [
                ("network", ChainId::Bitcoin, false, false, None),
                ("cube", ChainId::Bitcoin, true, false, None),
                ("cube-bottom", ChainId::Bitcoin, true, true, None),
                ("test-network", ChainId::Testnet4, false, false, None),
                (
                    "backup-choice",
                    ChainId::Testnet4,
                    true,
                    false,
                    Some(CreationBackupStep::Choice),
                ),
                (
                    "backup-intro",
                    ChainId::Testnet4,
                    true,
                    false,
                    Some(CreationBackupStep::Intro(false)),
                ),
                (
                    "backup-phrase",
                    ChainId::Testnet4,
                    true,
                    false,
                    Some(CreationBackupStep::Phrase),
                ),
                (
                    "backup-verify",
                    ChainId::Testnet4,
                    true,
                    false,
                    Some(CreationBackupStep::Verification {
                        word_indices: [2, 6, 11],
                        word_inputs: Default::default(),
                        error: Some("Word 6 doesn't match. Check your list.".into()),
                    }),
                ),
                (
                    "backup-bypass",
                    ChainId::Testnet4,
                    true,
                    false,
                    Some(CreationBackupStep::Bypass {
                        acknowledged: false,
                    }),
                ),
                (
                    "backup-kit-password",
                    ChainId::Testnet4,
                    true,
                    false,
                    Some(CreationBackupStep::KitPassword),
                ),
                (
                    "backup-kit-uploading",
                    ChainId::Testnet4,
                    true,
                    false,
                    Some(CreationBackupStep::KitUploading),
                ),
            ] {
                home.network = chain;
                // The old Network step is the header's open menu.
                home.create_network_menu = !confirmed && backup.is_none();
                home.state = match backup {
                    Some(step) => State::CreationBackup(step),
                    None => State::NoCube { create_cube: true },
                };
                let mut element = home.create_journey_view();
                let mut tree = Tree::new(element.as_widget());
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(iced::Size::ZERO, size),
                );
                if bottom {
                    let mut snap = iced::advanced::widget::operation::scrollable::snap_to(
                        Id::new(CREATE_BODY_SCROLL_ID),
                        iced::widget::scrollable::RelativeOffset {
                            x: None,
                            y: Some(1.0),
                        },
                    );
                    element.as_widget_mut().operate(
                        &mut tree,
                        Layout::new(&node),
                        &renderer,
                        &mut snap,
                    );
                }
                let viewport = iced::Rectangle::with_size(size);
                renderer.reset(viewport);
                let mut messages = Vec::new();
                element.as_widget_mut().update(
                    &mut tree,
                    &iced::Event::Window(iced::window::Event::RedrawRequested(
                        std::time::Instant::now(),
                    )),
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &renderer,
                    &mut iced::advanced::clipboard::Null,
                    &mut Shell::new(&mut messages),
                    &viewport,
                );
                element.as_widget().draw(
                    &tree,
                    &mut renderer,
                    &palette,
                    &iced::advanced::renderer::Style {
                        text_color: palette.colors.shell.ink,
                    },
                    Layout::new(&node),
                    mouse::Cursor::Unavailable,
                    &viewport,
                );
                let pixels = renderer.screenshot(
                    iced::Size::new(1024, 768),
                    1.0,
                    palette.colors.shell.background,
                );
                ::image::save_buffer(
                    format!("{directory}/{appearance}-{name}-1024x768.png"),
                    &pixels,
                    1024,
                    768,
                    ::image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }

    #[test]
    fn import_entry_preserves_one_operation_until_creation_is_cancelled() {
        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert!(matches!(home.state, State::NoCube { create_cube: true }));
        let first = home
            .pending_bitcoin_import
            .as_ref()
            .expect("import intent")
            .operation_id;

        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert_eq!(
            home.pending_bitcoin_import.as_ref().unwrap().operation_id,
            first
        );

        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(false)));
        assert!(home.pending_bitcoin_import.is_none());
        assert!(matches!(home.state, State::NoCube { create_cube: false }));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bitcoin_import_legacy_form_keeps_its_create_submit() {
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{Id, Operation, Tree},
            Layout,
        };

        #[derive(Default)]
        struct Labels(Vec<String>);
        impl Operation for Labels {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, text: &str) {
                self.0.push(text.to_owned());
            }
        }

        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert!(home.pending_bitcoin_import.is_some());
        let mut element = home.creation_form(true);
        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1024.0, 1800.0)),
        );
        let mut labels = Labels::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
        assert!(
            labels.0.iter().any(|label| label == "Create Cube"),
            "rendered labels: {:?}",
            labels.0
        );
    }

    /// Import Bitcoin wallet opens Import or restore in the Create frame,
    /// never the legacy sidebar form; its Overview lists the other paths.
    #[tokio::test(flavor = "multi_thread")]
    async fn import_opens_import_or_restore_in_the_create_frame() {
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{Id, Operation, Tree},
            Layout,
        };

        #[derive(Default)]
        struct Labels(Vec<String>);
        impl Operation for Labels {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, text: &str) {
                self.0.push(text.to_owned());
            }
        }
        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        use coincube_ui::{theme::WorldviewMode, worldview_copy};
        let labels = |home: &Home| {
            let mut element =
                worldview_copy::render_in(WorldviewMode::Suitcoiner, || home.create_journey_view());
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1440.0, 900.0)),
            );
            let mut labels = Labels::default();
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
            labels.0
        };

        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert!(home.showing_create_journey());
        let shown = labels(&home);
        for label in [
            "Import or restore",
            "Overview",
            "Import Bitcoin wallet",
            "Create a Cube for your Bitcoin wallet",
            "Source",
            "Create Cube",
        ] {
            assert!(
                shown.iter().any(|l| l == label),
                "missing {:?}: {:?}",
                label,
                shown
            );
        }
        // Neither the legacy sidebar nor Create's network step.
        for label in [
            "Cubes",
            "Light Mode",
            "Live networks",
            "Establish a new position.",
        ] {
            assert!(
                !shown.iter().any(|l| l == label),
                "{:?} shown: {:?}",
                label,
                shown
            );
        }

        // The Overview drops the import intent and lists the paths.
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        assert!(home.pending_bitcoin_import.is_none());
        assert!(home.restore_hub);
        assert!(home.showing_create_journey());
        let shown = labels(&home);
        for label in [
            "Choose what to bring in",
            "Restore from Recovery Kit",
            "Sign in to Connect",
            "No sign-in",
            "Recovery Kit and phone recovery restore Cubes saved on your Connect account, so \
             they open after you sign in. Use Sign in to Connect next to either one. Wallet \
             import needs no account.",
        ] {
            assert!(
                shown.iter().any(|l| l == label),
                "missing {:?}: {:?}",
                label,
                shown
            );
        }
        assert!(!shown.iter().any(|l| l == "Create Cube"));

        // Back to the import path, then Close leaves both.
        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert!(home.pending_bitcoin_import.is_some());
        assert!(!home.restore_hub);
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(false)));
        assert!(!home.restore_hub);
        assert!(!home.showing_import_or_restore());

        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(true)));
        let shown = labels(&home);
        assert!(shown
            .iter()
            .any(|label| label == "Establish a new position."));
    }

    /// Changing the network in the header re-filters Import or restore
    /// instead of closing it. Import Bitcoin wallet closes off Bitcoin, with
    /// its reason and a way back; Kit and phone wait for Connect sign-in.
    #[tokio::test(flavor = "multi_thread")]
    async fn import_or_restore_refilters_on_a_network_change() {
        let mut home = home();
        home.developer_mode = true;
        home.refresh_displayed_networks();
        home.state = State::NoCube { create_cube: false };
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        assert!(home.create_network_menu_editable());
        let _ = home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Testnet4)));
        assert_eq!(home.network, ChainId::Testnet4);
        assert!(home.restore_hub, "the wizard stays open");
        assert!(home.showing_import_or_restore());
        assert!(!home.can_start_bitcoin_import());

        let shown = restore_frame::rendered_labels(home.create_journey_frame()).await;
        for label in [
            "Into a new Cube on Testnet4.",
            "Bitcoin and Bitcoin Blake2b only.",
            "Switch to Bitcoin",
            "Not on Testnet4",
            "Sign in to Connect",
        ] {
            assert!(
                shown.iter().any(|l| l == label),
                "missing {:?}: {:?}",
                label,
                shown
            );
        }
        assert_eq!(home.connect_restore_reason(), Some("Sign in to Connect"));

        // Signed in, Kit and phone open on every non-Blake2b network and
        // say how many Cubes they can restore here.
        let mut home = signed_in_home();
        home.developer_mode = true;
        home.refresh_displayed_networks();
        home.state = State::NoCube { create_cube: false };
        home.network = ChainId::Signet;
        let _ = home.update(Message::View(ViewMessage::ShowRestoreHub));
        assert_eq!(home.connect_restore_reason(), None);
        let shown = restore_frame::rendered_labels(home.create_journey_frame()).await;
        assert!(
            shown
                .iter()
                .any(|l| l == "Connected · 0 Cubes not on this device"),
            "{:?}",
            shown
        );
    }

    /// Recover with phone's Cube step lists this network's Cubes, then
    /// counts the rest, which "Show all networks" lists.
    #[tokio::test(flavor = "multi_thread")]
    async fn phone_cube_step_counts_and_shows_other_networks() {
        let remote = |id: u64, name: &str, chain: ChainId| RemoteCube {
            id,
            uuid: format!("uuid-{id}"),
            name: name.to_string(),
            network: settings::network_to_api_string(chain),
            has_recovery_kit: true,
            has_encrypted_seed: true,
            phone_recoverable: true,
            phone_full_cube: true,
            encryption_pubkey: None,
            has_vault: None,
        };
        let mut home = signed_in_home();
        home.state = State::NoCube { create_cube: false };
        home.remote_cubes = vec![
            remote(1, "Everyday", ChainId::Bitcoin),
            remote(2, "Sandbox", ChainId::Signet),
        ];
        let _ = home.update(Message::View(ViewMessage::ShowRestorePhone));
        let shown = restore_frame::rendered_labels(home.create_journey_frame()).await;
        assert!(shown.iter().any(|l| l == "Everyday"), "{:?}", shown);
        assert!(!shown.iter().any(|l| l == "Sandbox"), "{:?}", shown);
        assert!(shown.iter().any(|l| l == "1 more on other networks"));
        assert!(
            shown.iter().any(|l| l == "Approve"),
            "phone rail: {:?}",
            shown
        );

        let _ = home.update(Message::View(ViewMessage::ShowRestoreAllNetworks(true)));
        let shown = restore_frame::rendered_labels(home.create_journey_frame()).await;
        assert!(shown.iter().any(|l| l == "Sandbox"), "{:?}", shown);
    }

    /// Import sets a PIN only: a passkey can't hold an imported secret.
    #[test]
    fn import_sets_a_pin_only() {
        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        home.passkey_mode = true;
        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert!(!home.passkey_mode);
        let _ = home.update(Message::View(ViewMessage::TogglePasskeyMode(true)));
        assert!(!home.passkey_mode);
    }

    #[test]
    fn a_full_bitcoin_plan_closes_the_import_path() {
        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        assert!(home.can_start_bitcoin_import());
        home.server_cube_limit = Some(0);
        assert!(!home.can_start_bitcoin_import());
        home.server_cube_limit = Some(10);
        home.network = ChainId::Testnet4;
        assert!(!home.can_start_bitcoin_import(), "Import is Bitcoin only");
    }

    #[test]
    fn first_run_import_entry_and_cube_subtitle_are_distinct_from_vault() {
        let mut home = home();
        home.state = State::Unchecked;
        let _ = home.update(Message::View(ViewMessage::ShowImportBitcoin));
        assert!(home.pending_bitcoin_import.is_some());
        let mut cube = CubeSettings::new("Imported".to_string(), Network::Bitcoin);
        cube.bitcoin_import = home.pending_bitcoin_import.clone();
        assert_eq!(vault_subtitle(&cube), "Vault import pending");
    }

    /// The Launcher's control describes the account (it has no live updates):
    /// the light and words follow the session, never "Sign in" for a session
    /// that is only waiting for the network.
    #[test]
    fn the_launcher_connect_control_describes_the_account() {
        use crate::app::state::connect::account::UnverifiedSession;
        use crate::app::view::shared::connect_control::ConnectDetail;
        use crate::app::{ConnectLight, SignedOutBy};
        use crate::services::connect::grpc::trouble::Trouble;
        let state = |home: &Home| {
            let (detail, light, label) = launcher_chrome::launcher_connect_state(home);
            (format!("{detail:?}"), light, label)
        };
        let signed_in = signed_in_home();
        let (detail, light, label) = state(&signed_in);
        assert_eq!(
            detail,
            format!("{:?}", ConnectDetail::Account("founder@example.com"))
        );
        assert_eq!(light, Some(ConnectLight::On));
        assert_eq!(label, "Connect account, founder@example.com, signed in");

        let idle = || ConnectFlowStep::Login {
            email: String::new(),
            loading: false,
        };
        let waiting = |trouble| {
            let mut home = home();
            home.connect_account.step = idle();
            home.connect_account.unverified = Some(UnverifiedSession {
                email: "alex@example.test".into(),
                trouble,
                attempts: 1,
            });
            home
        };
        let (detail, light, label) = state(&waiting(Trouble::Offline));
        assert_eq!(detail, format!("{:?}", ConnectDetail::Offline));
        assert_eq!(light, Some(ConnectLight::Offline));
        assert!(label.ends_with("offline: no internet connection"));
        let (detail, light, _) = state(&waiting(Trouble::Unreachable));
        assert_eq!(
            detail,
            format!("{:?}", ConnectDetail::Account("alex@example.test"))
        );
        assert_eq!(light, Some(ConnectLight::Reconnecting));

        let mut ended = home();
        ended.connect_account.step = idle();
        ended.connect_account.signed_out_by = Some(SignedOutBy::SessionEnded);
        let (detail, light, label) = state(&ended);
        assert_eq!(detail, format!("{:?}", ConnectDetail::SignIn));
        assert_eq!(light, Some(ConnectLight::NeedsYou));
        assert!(label.contains("needs attention"));

        let mut plain = home();
        plain.connect_account.step = idle();
        let (detail, light, label) = state(&plain);
        assert_eq!(detail, format!("{:?}", ConnectDetail::SignIn));
        assert_eq!(light, None);
        assert_eq!(label, "Connect: Sign in");
    }

    /// Not now and the relayed notice reach the Launcher's account (L3).
    #[test]
    fn the_signed_out_notice_comes_and_goes() {
        use crate::app::SignedOutBy;
        let mut home = home();
        drop(home.update(Message::ConnectSignedOutBy(SignedOutBy::ComputerRemoved)));
        assert_eq!(
            home.connect_account.signed_out_by,
            Some(SignedOutBy::ComputerRemoved)
        );
        drop(home.update(Message::View(ViewMessage::DismissConnectSignOut)));
        assert_eq!(home.connect_account.signed_out_by, None);
    }

    fn signed_in_home() -> Home {
        let mut home = home();
        home.connect_account.step = ConnectFlowStep::Dashboard;
        home.connect_account.user = Some(User {
            id: 7,
            email: "founder@example.com".to_string(),
            email_verified: Some(true),
        });
        home
    }

    pub(super) fn enable_btcb2(home: &mut Home) {
        let mut client = CoincubeClient::new();
        client.set_token("synthetic-btcb2-token");
        home.connect_account.install_admitted_client(client);
        home.connect_account.bitcoin_blake2b_opt_in = true;
        home.connect_account.step = ConnectFlowStep::Dashboard;
        home.connect_account.user = Some(User {
            id: 7,
            email: "founder@example.com".to_string(),
            email_verified: Some(true),
        });
        home.connect_account.features = Some(
            serde_json::from_value(serde_json::json!({
                "plans": [],
                "bitcoinBlake2bEnabled": true
            }))
            .unwrap(),
        );
    }

    /// A fresh, empty datadir — see the same helper in
    /// `phone_signer::pairing_store`.
    fn fresh_datadir() -> CoincubeDirectory {
        let mut path = std::env::temp_dir();
        path.push(format!("coincube-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("mkdir tempdir");
        CoincubeDirectory::new(path)
    }

    #[test]
    fn connect_catch_up_and_remote_matching_include_btcb2_cubes() {
        let datadir = fresh_datadir();
        for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
            let mut cube = CubeSettings::new(chain.api_str().to_string(), chain);
            cube.remote_synced = chain == ChainId::Bitcoin;
            let directory = datadir.network_directory(chain);
            std::fs::create_dir_all(directory.path()).unwrap();
            std::fs::write(
                directory.path().join(settings::SETTINGS_FILE_NAME),
                serde_json::to_vec(&settings::Settings {
                    cubes: vec![cube],
                    ..Default::default()
                })
                .unwrap(),
            )
            .unwrap();
        }

        let cubes = locally_stored_cubes(&datadir);
        assert_eq!(cubes.len(), 2, "remote matching must see both chains");
        assert_eq!(
            cubes
                .iter()
                .filter(|cube| !cube.remote_synced)
                .map(|cube| cube.network)
                .collect::<Vec<_>>(),
            vec![ChainId::BitcoinBlake2b],
            "catch-up must register the unsynced BTCB2 Cube",
        );
        std::fs::remove_dir_all(datadir.path()).unwrap();
    }

    #[test]
    fn each_advisory_notice_fires_once_per_install() {
        let datadir = fresh_datadir();

        // A fresh install owes one notice per advisory we ship, in table
        // order, and each is acknowledged exactly once.
        for expected in crate::hw_advisory::ADVISORIES {
            let advisory = pending_advisory_notice(&datadir).expect("notice pending");
            assert_eq!(advisory.id, expected.id);
            assert!(!advisory.notice.is_empty());

            // Acknowledging is what `ViewMessage::DismissAdvisoryNotice` does.
            GlobalSettings::mark_advisory_notice_seen(&GlobalSettings::path(&datadir), advisory.id)
                .expect("persist");
        }

        // Every launch after that, including a fresh `Home`.
        assert!(pending_advisory_notice(&datadir).is_none());
        let home = Home::new(datadir, Some(Network::Bitcoin)).0;
        assert!(home.advisory_notice.is_none());
    }

    #[test]
    fn acknowledging_one_notice_surfaces_the_next_without_a_relaunch() {
        let datadir = fresh_datadir();
        let mut home = Home::new(datadir, Some(Network::Bitcoin)).0;

        // Walk the whole queue through the message the button sends, and
        // check it drains in one session rather than one notice per launch.
        for expected in crate::hw_advisory::ADVISORIES {
            assert_eq!(
                home.advisory_notice.expect("notice pending").id,
                expected.id
            );
            let _ = home.update(Message::View(ViewMessage::DismissAdvisoryNotice));
        }
        assert!(home.advisory_notice.is_none());
    }

    #[test]
    fn advisory_notice_is_owed_on_a_brand_new_install() {
        // Nothing about the notice is conditional on owning the device: a
        // descriptor key doesn't record where it came from, so everyone sees
        // it and the copy self-selects.
        let home = Home::new(fresh_datadir(), Some(Network::Bitcoin)).0;
        assert!(home.advisory_notice.is_some());
    }

    fn cube(id: &str, name: &str, network: Network) -> CubeSettings {
        CubeSettings::new_with_raw_id(id.to_string(), name.to_string(), network)
    }

    fn remote_cube(uuid: &str, name: &str, network: Network) -> RemoteCube {
        RemoteCube {
            id: 42,
            uuid: uuid.to_string(),
            name: name.to_string(),
            network: settings::network_to_api_string(network),
            has_recovery_kit: true,
            has_encrypted_seed: true,
            phone_recoverable: false,
            phone_full_cube: false,
            encryption_pubkey: None,
            has_vault: None,
        }
    }

    fn abandon_phrase() -> bip39::Mnemonic {
        bip39::Mnemonic::parse_in(
            bip39::Language::English,
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .unwrap()
    }

    fn registered_key(mnemonic: &bip39::Mnemonic, chain: ChainId) -> String {
        let signer =
            MasterSigner::from_mnemonic(chain.bitcoin_network(), mnemonic.clone()).unwrap();
        crate::services::connect::crypto::CubeEncryptionKey::derive(
            &signer,
            chain.bitcoin_network(),
        )
        .public_key_hex()
    }

    #[test]
    fn a_ghost_import_takes_the_ghost_identity_only_for_its_own_phrase() {
        let uuid = "6f1c7a3e-2b4d-4c8e-9a10-3f2e1d0c9b8a";
        let phrase = abandon_phrase();
        let mut ghost = GhostImport {
            uuid: uuid.into(),
            name: "Old laptop".into(),
            encryption_pubkey: Some(registered_key(&phrase, ChainId::Bitcoin)),
            network: ChainId::Bitcoin,
            account: None,
        };
        assert_eq!(
            ghost_import_identity(&ghost, ChainId::Bitcoin, &phrase),
            Ok(Some(uuid::Uuid::parse_str(uuid).unwrap()))
        );
        // Another Cube's phrase is refused, naming the Cube it was typed for.
        let other = bip39::Mnemonic::parse_in(
            bip39::Language::English,
            "legal winner thank year wave sausage worth useful legal winner thank yellow",
        )
        .unwrap();
        let refused = ghost_import_identity(&ghost, ChainId::Bitcoin, &other).unwrap_err();
        assert!(
            refused.contains("doesn't belong to Old laptop"),
            "{}",
            refused
        );
        // The key is per network: the same seed on Testnet4 is not this Cube.
        assert!(ghost_import_identity(&ghost, ChainId::Testnet4, &phrase).is_err());
        // Test networks share a coin type, so the same key matches on each;
        // only the registered chain may take the identity.
        let mut testnet = GhostImport {
            encryption_pubkey: Some(registered_key(&phrase, ChainId::Testnet4)),
            network: ChainId::Testnet4,
            ..ghost.clone()
        };
        assert_eq!(
            registered_key(&phrase, ChainId::Testnet4),
            registered_key(&phrase, ChainId::Signet),
            "the premise: one key on both test networks"
        );
        assert!(ghost_import_identity(&testnet, ChainId::Testnet4, &phrase).is_ok());
        let switched = ghost_import_identity(&testnet, ChainId::Signet, &phrase).unwrap_err();
        assert!(switched.contains("registered on Testnet4"), "{}", switched);
        testnet.encryption_pubkey = None;
        assert!(
            ghost_import_identity(&testnet, ChainId::Signet, &phrase).is_err(),
            "even with no key, the form belongs to its own chain"
        );
        // Nothing registered proves the phrase, so it becomes a new Cube.
        ghost.encryption_pubkey = None;
        assert_eq!(
            ghost_import_identity(&ghost, ChainId::Bitcoin, &phrase),
            Ok(None)
        );
    }

    #[test]
    fn a_ghost_import_never_leaves_its_identity_for_the_next_cube() {
        let phrase = abandon_phrase();
        let uuid = "6f1c7a3e-2b4d-4c8e-9a10-3f2e1d0c9b8a";
        let mut home = home();
        let mut ghost = remote_cube(uuid, "Old laptop", Network::Bitcoin);
        ghost.encryption_pubkey = Some(registered_key(&phrase, ChainId::Bitcoin));
        home.remote_cubes = vec![ghost];
        let _ = home.update(Message::View(ViewMessage::ImportGhostWithPhrase(
            uuid.into(),
        )));
        for (index, word) in phrase.words().enumerate() {
            home.recovery_words[index] = word.to_string();
        }
        for (index, digit) in ["1", "2", "3", "4"].iter().enumerate() {
            let _ = home
                .create_cube_pin
                .update(pin_input::Message::DigitChanged(index, digit.to_string()));
            let _ = home
                .create_cube_pin_confirm
                .update(pin_input::Message::DigitChanged(index, digit.to_string()));
        }
        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));
        assert!(home.creating_cube, "the matching phrase is accepted");
        // The ghost's ID was used for this write only: the retry slot that
        // a failed write keeps, and that Create reuses, never holds it.
        assert_ne!(
            home.pending_cube_id.map(|id| id.to_string()).as_deref(),
            Some(uuid)
        );
        let _ = home.update(Message::CubeCreated(Err("disk full".into())));
        let _ = home.update(Message::View(ViewMessage::CancelRecovery));
        assert_ne!(
            home.pending_cube_id.map(|id| id.to_string()).as_deref(),
            Some(uuid)
        );
    }

    #[test]
    fn a_blake2b_ghost_imports_from_its_recovery_phrase() {
        let phrase = abandon_phrase();
        let uuid = "0b2b2b2b-2b4d-4c8e-9a10-3f2e1d0c9b8a";
        let mut home = home();
        enable_btcb2(&mut home);
        home.network = ChainId::BitcoinBlake2b;
        home.state = State::NoCube { create_cube: false };
        let mut ghost = remote_cube(uuid, "Old laptop", Network::Bitcoin);
        ghost.network = settings::network_to_api_string(ChainId::BitcoinBlake2b);
        ghost.encryption_pubkey = Some(registered_key(&phrase, ChainId::BitcoinBlake2b));
        home.remote_cubes = vec![ghost];
        let _ = home.update(Message::View(ViewMessage::ImportGhostWithPhrase(
            uuid.into(),
        )));
        assert!(home.error.is_none(), "{:?}", home.error);
        assert!(matches!(home.state, State::RecoveryInput));
        assert_eq!(
            home.ghost_import.as_ref().map(|ghost| ghost.network),
            Some(ChainId::BitcoinBlake2b)
        );
        for (index, word) in phrase.words().enumerate() {
            home.recovery_words[index] = word.to_string();
        }
        for (index, digit) in ["1", "2", "3", "4"].iter().enumerate() {
            let _ = home
                .create_cube_pin
                .update(pin_input::Message::DigitChanged(index, digit.to_string()));
            let _ = home
                .create_cube_pin_confirm
                .update(pin_input::Message::DigitChanged(index, digit.to_string()));
        }
        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));
        assert!(home.error.is_none(), "{:?}", home.error);
        assert!(home.creating_cube, "the matching phrase is accepted");
        // It reclaims the ghost's Connect identity, as on Bitcoin.
        assert_eq!(
            home.ghost_claim.as_ref().map(|(id, _)| id.as_str()),
            Some(uuid)
        );
    }

    /// A Home on the ghost import form with `phrase` and a PIN typed in.
    fn ghost_import_form(phrase: &bip39::Mnemonic, uuid: &str) -> Home {
        let mut home = home();
        let mut ghost = remote_cube(uuid, "Old laptop", Network::Bitcoin);
        ghost.encryption_pubkey = Some(registered_key(phrase, ChainId::Bitcoin));
        home.remote_cubes = vec![ghost];
        let _ = home.update(Message::View(ViewMessage::ImportGhostWithPhrase(
            uuid.into(),
        )));
        for (index, word) in phrase.words().enumerate() {
            home.recovery_words[index] = word.to_string();
        }
        for (index, digit) in ["1", "2", "3", "4"].iter().enumerate() {
            let _ = home
                .create_cube_pin
                .update(pin_input::Message::DigitChanged(index, digit.to_string()));
            let _ = home
                .create_cube_pin_confirm
                .update(pin_input::Message::DigitChanged(index, digit.to_string()));
        }
        home
    }

    #[test]
    fn a_ghost_import_is_refused_once_its_account_no_longer_lists_it() {
        let phrase = abandon_phrase();
        let uuid = "6f1c7a3e-2b4d-4c8e-9a10-3f2e1d0c9b8a";
        // Signed out with the form open: the logout path empties the list.
        let mut home = ghost_import_form(&phrase, uuid);
        home.remote_cubes.clear();
        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));
        assert!(!home.creating_cube, "no write starts");
        assert!(home.ghost_claim.is_none());
        let error = home.error.clone().unwrap_or_default();
        assert!(error.contains("no longer lists Old laptop"), "{}", error);
        // Another account signed in since the ghost was picked.
        let mut home = ghost_import_form(&phrase, uuid);
        home.ghost_import.as_mut().unwrap().account = Some(41);
        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));
        assert!(!home.creating_cube);
        // The same account still listing it goes ahead and holds the claim.
        let mut home = ghost_import_form(&phrase, uuid);
        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));
        assert!(home.creating_cube);
        assert_eq!(home.ghost_claim, Some((uuid.to_string(), None)));
    }

    #[test]
    fn cancel_waits_for_a_ghost_import_that_is_already_writing() {
        let phrase = abandon_phrase();
        let uuid = "6f1c7a3e-2b4d-4c8e-9a10-3f2e1d0c9b8a";
        let mut home = ghost_import_form(&phrase, uuid);
        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));
        assert!(home.creating_cube);
        let _ = home.update(Message::View(ViewMessage::CancelRecovery));
        assert!(home.creating_cube, "the write is not abandoned mid-way");
        assert!(home.ghost_import.is_some() && home.ghost_claim.is_some());
        assert!(matches!(home.state, State::RecoveryInput));
        // A failed write releases the form, and Cancel works again.
        let _ = home.update(Message::CubeCreated(Err("disk full".into())));
        assert!(home.ghost_claim.is_none());
        let _ = home.update(Message::View(ViewMessage::CancelRecovery));
        assert!(home.ghost_import.is_none());
    }

    #[test]
    fn import_with_phrase_opens_the_recovery_form_named_for_the_ghost() {
        let mut home = home();
        let mut ghost = remote_cube("ghost-a", "Old laptop", Network::Bitcoin);
        ghost.encryption_pubkey = Some("02ab".into());
        home.remote_cubes = vec![ghost];
        let _ = home.update(Message::View(ViewMessage::ImportGhostWithPhrase(
            "ghost-a".into(),
        )));
        assert!(matches!(home.state, State::RecoveryInput));
        assert_eq!(home.create_cube_name.value, "Old laptop");
        assert_eq!(
            home.ghost_import,
            Some(GhostImport {
                uuid: "ghost-a".into(),
                name: "Old laptop".into(),
                encryption_pubkey: Some("02ab".into()),
                network: ChainId::Bitcoin,
                account: None,
            })
        );
        // Cancel forgets the ghost, so a later import is a plain new Cube.
        let _ = home.update(Message::View(ViewMessage::CancelRecovery));
        assert_eq!(home.ghost_import, None);
        // An unknown registration changes nothing.
        let _ = home.update(Message::View(ViewMessage::ImportGhostWithPhrase(
            "missing".into(),
        )));
        assert_eq!(home.ghost_import, None);
    }

    #[test]
    fn bip39_suggestions_are_lowercase_limited_and_prefix_checked() {
        assert!(bip39_suggestions("", 5).is_empty());
        assert!(bip39_suggestions("ab", 0).is_empty());

        let suggestions = bip39_suggestions("AB", 3);
        assert_eq!(suggestions.len(), 3);
        assert!(suggestions.iter().all(|word| word.starts_with("ab")));
    }

    #[test]
    fn cube_limit_prefers_server_limit_and_counts_remote_cubes_on_current_network() {
        let mut home = home();
        home.account_tier = AccountTier::Free;
        assert_eq!(home.cube_limit(), 1);

        home.server_cube_limit = Some(5);
        assert_eq!(home.cube_limit(), 5);

        home.state = State::Cubes {
            cubes: vec![
                cube("local-a", "Local A", Network::Bitcoin),
                cube("local-b", "Local B", Network::Bitcoin),
            ],
            create_cube: false,
            source: crate::chain::ChainId::Bitcoin,
        };
        home.remote_cubes = vec![
            remote_cube("remote-mainnet", "Remote Mainnet", Network::Bitcoin),
            remote_cube("remote-signet", "Remote Signet", Network::Signet),
        ];

        assert_eq!(home.total_cube_count(), 3);
    }

    #[test]
    fn create_cube_form_toggles_and_resets_transient_inputs() {
        let mut home = home();
        home.state = State::Cubes {
            cubes: Vec::new(),
            create_cube: false,
            source: crate::chain::ChainId::Bitcoin,
        };

        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(true)));
        assert!(matches!(
            home.state,
            State::Cubes {
                create_cube: true,
                ..
            }
        ));

        let _ = home.update(Message::View(ViewMessage::CubeNameEdited("  ".to_string())));
        assert!(!home.create_cube_name.valid);
        assert!(home.error.is_none());

        home.create_cube_name.value = "Temporary".to_string();
        home.create_cube_pin.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        home.create_cube_pin_confirm.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        home.recovery_words[0] = "abandon".to_string();
        home.recovery_active_index = Some(0);
        home.passkey_mode = !home.default_passkey_mode();

        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(false)));

        assert!(matches!(
            home.state,
            State::Cubes {
                create_cube: false,
                ..
            }
        ));
        assert!(home.create_cube_name.value.is_empty());
        assert_eq!(home.create_cube_pin.value().as_str(), "");
        assert_eq!(home.create_cube_pin_confirm.value().as_str(), "");
        assert!(home.recovery_words.iter().all(String::is_empty));
        assert!(home.recovery_active_index.is_none());
        let expected_default = home.default_passkey_mode();
        assert_eq!(
            home.passkey_mode, expected_default,
            "dismissing restores the default unlock method"
        );
    }

    /// With no local Cube but remote ones to restore, `NoCube` is a list
    /// screen: it shows the list plus a "Create Cube" button, and the form is
    /// only reached by pressing it. Stacking the form under the list is what
    /// made this screen read as two pages at once.
    #[test]
    fn no_cube_with_remote_cubes_lists_them_instead_of_showing_the_form() {
        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        home.remote_cubes = vec![remote_cube("remote-a", "Remote A", Network::Bitcoin)];

        assert!(home.has_cube_list());
        assert!(
            !home.showing_create_form(),
            "the list, not the form, is the landing screen when there is something to list"
        );

        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(true)));
        assert!(matches!(home.state, State::NoCube { create_cube: true }));
        assert!(home.showing_create_form());

        let _ = home.update(Message::View(ViewMessage::ShowCreateCube(false)));
        assert!(matches!(home.state, State::NoCube { create_cube: false }));
        assert!(!home.showing_create_form());
        home.view();
    }

    #[test]
    fn no_local_cubes_land_on_launcher_regardless_of_connect_session() {
        for signed_in in [false, true] {
            let mut home = home();
            if signed_in {
                let mut client = CoincubeClient::new();
                client.set_token("synthetic-launcher-session");
                home.connect_account.install_admitted_client(client);
                home.connect_account.step = ConnectFlowStep::Dashboard;
            }
            assert_eq!(home.connect_account.is_authenticated(), signed_in);
            drop(home.update(Message::Checked {
                for_chain: ChainId::Bitcoin,
                res: Ok(State::NoCube { create_cube: false }),
            }));
            assert!(!home.has_cube_list());
            assert!(!home.showing_create_form());
            assert!(launcher_catalog::catalog_visible(&home));
            home.view();

            drop(home.update(Message::View(ViewMessage::ShowCreateCube(true))));
            assert!(home.showing_create_journey());
            assert!(home.can_leave_create_journey());
            drop(home.update(Message::View(ViewMessage::ShowCreateCube(false))));
            assert!(launcher_catalog::catalog_visible(&home));
        }
    }

    /// A late Connect response must preserve explicitly opened creation.
    #[test]
    fn a_remote_cube_fetch_does_not_steal_a_create_form_being_filled_in() {
        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        drop(home.update(Message::View(ViewMessage::ShowCreateCube(true))));
        assert!(home.showing_create_form());
        assert!(matches!(home.state, State::NoCube { create_cube: true }));

        let _ = home.update(Message::View(ViewMessage::CubeNameEdited(
            "Half typed".to_string(),
        )));

        let current = home.remote_cubes_request;
        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: current,
            result: Ok(vec![remote_cube("remote-a", "Remote A", Network::Bitcoin)]),
        });

        assert!(home.has_cube_list(), "the fetch did land a list");
        assert!(
            matches!(home.state, State::NoCube { create_cube: true }),
            "the explicit creation choice survives the fetch"
        );
        assert!(
            home.showing_create_form(),
            "the user keeps the form they were filling in"
        );
        assert_eq!(home.create_cube_name.value, "Half typed");
    }

    /// A late Connect response populates the Launcher without opening creation.
    #[test]
    fn a_remote_cube_fetch_keeps_the_launcher_visible() {
        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        assert!(!home.showing_create_form());

        let current = home.remote_cubes_request;
        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: current,
            result: Ok(vec![remote_cube("remote-a", "Remote A", Network::Bitcoin)]),
        });

        assert!(matches!(home.state, State::NoCube { create_cube: false }));
        assert!(
            !home.showing_create_form(),
            "the freshly-loaded list stays on the Launcher"
        );
        home.view();
    }

    /// An empty network stays on the Launcher even with remote Cubes elsewhere.
    #[test]
    fn remote_cubes_on_another_network_do_not_count_as_a_list() {
        let mut home = home();
        home.state = State::NoCube { create_cube: false };
        home.remote_cubes = vec![remote_cube(
            "remote-signet",
            "Remote Signet",
            Network::Signet,
        )];

        assert!(!home.has_cube_list());
        assert!(!home.showing_create_form());
    }

    #[test]
    fn create_cube_validation_reports_pin_and_limit_errors_before_side_effects() {
        let mut home = home();
        home.state = State::Cubes {
            cubes: Vec::new(),
            create_cube: true,
            source: crate::chain::ChainId::Bitcoin,
        };
        home.create_cube_name.value = "My Cube".to_string();
        // PIN-path validation: the form defaults to a passkey where one is
        // available, which skips every check below.
        home.passkey_mode = false;

        let _ = home.update(Message::View(ViewMessage::CreateCube));
        assert_eq!(home.error.as_deref(), Some("Please enter all 4 PIN digits"));

        home.create_cube_pin.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        let _ = home.update(Message::View(ViewMessage::CreateCube));
        assert_eq!(
            home.error.as_deref(),
            Some("Please confirm all 4 PIN digits")
        );

        home.create_cube_pin_confirm.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "5".to_string(),
        ];
        let _ = home.update(Message::View(ViewMessage::CreateCube));
        assert_eq!(home.error.as_deref(), Some("PIN codes do not match"));

        home.state = State::Cubes {
            cubes: vec![
                cube("local-a", "Local A", Network::Bitcoin),
                cube("local-b", "Local B", Network::Bitcoin),
            ],
            create_cube: true,
            source: crate::chain::ChainId::Bitcoin,
        };
        home.create_cube_pin_confirm.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        home.account_tier = AccountTier::Free;
        home.server_cube_limit = None;

        let _ = home.update(Message::View(ViewMessage::CreateCube));
        // Two Cubes on Free's one (billing v2 limits): over the limit, the
        // copy says what the plan includes and that existing Cubes keep working.
        let error = home.error.clone().unwrap_or_default();
        assert!(
            error.contains(
                "Cube limit reached. Bitcoin mainnet: you have 2 Cubes; the Free plan includes 1."
            ),
            "{}",
            error
        );
        assert!(error.contains("Existing Cubes keep working."), "{}", error);
        assert!(!home.creating_cube);

        // Exactly at the limit keeps the ratio.
        home.error = None;
        home.state = State::Cubes {
            cubes: vec![cube("local-a", "Local A", Network::Bitcoin)],
            create_cube: true,
            source: crate::chain::ChainId::Bitcoin,
        };
        let _ = home.update(Message::View(ViewMessage::CreateCube));
        let error = home.error.clone().unwrap_or_default();
        assert!(
            error.contains("Bitcoin mainnet: 1/1 Cubes on the Free plan."),
            "{}",
            error
        );
    }

    #[test]
    fn recovery_word_input_normalizes_to_valid_bip39_prefixes() {
        let mut home = home();
        home.error = Some("old".to_string());

        let _ = home.update(Message::View(ViewMessage::RecoveryWordInput {
            index: 0,
            word: "ABANDON123!".to_string(),
        }));

        assert_eq!(home.recovery_words[0], "abandon");
        assert_eq!(home.recovery_active_index, Some(0));
        assert!(home.error.is_none());

        let _ = home.update(Message::View(ViewMessage::SelectRecoverySuggestion {
            index: 0,
            word: "ability".to_string(),
        }));
        assert_eq!(home.recovery_words[0], "ability");
        assert!(home.recovery_active_index.is_none());

        let _ = home.update(Message::View(ViewMessage::RecoveryWordInput {
            index: 20,
            word: "zoo".to_string(),
        }));
        assert_eq!(home.recovery_words[0], "ability");
    }

    #[test]
    fn invalid_recovery_submit_clears_words_and_sets_parse_error() {
        let mut home = home();
        home.recovery_words[0] = "abandon".to_string();
        home.recovery_active_index = Some(0);

        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));

        assert!(home.recovery_words.iter().all(String::is_empty));
        assert!(home.recovery_active_index.is_none());
        assert!(home.error.is_some());
    }

    #[test]
    fn remote_cube_and_recovery_method_modals_follow_selected_cube() {
        let mut home = home();
        home.remote_cubes = vec![remote_cube("remote-a", "Remote A", Network::Bitcoin)];

        let _ = home.update(Message::View(ViewMessage::ShowRecoveryMethodPicker(
            "remote-a".to_string(),
        )));
        assert!(matches!(
            &home.recovery_method_modal,
            Some(modal) if modal.cube.uuid == "remote-a"
        ));

        let _ = home.update(Message::View(ViewMessage::CloseRecoveryMethodPicker));
        assert!(home.recovery_method_modal.is_none());

        let _ = home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::ShowRemoteModal("remote-a".to_string()),
        )));
        assert!(matches!(
            &home.delete_remote_cube_modal,
            Some(modal) if modal.cube.uuid == "remote-a" && !modal.deleting
        ));

        // Connect could restore this Cube, so Delete waits for its name.
        let _ = home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::ConfirmRemoteDelete("remote-a".to_string()),
        )));
        assert!(matches!(
            &home.delete_remote_cube_modal,
            Some(modal) if !modal.deleting && modal.error.is_none()
        ));
        let typed =
            catalog_display_name(&home.delete_remote_cube_modal.as_ref().unwrap().cube.name);
        let _ = home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::RemoteNameInput(typed),
        )));
        let _ = home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::ConfirmRemoteDelete("remote-a".to_string()),
        )));
        assert!(matches!(
            &home.delete_remote_cube_modal,
            Some(modal)
                if !modal.deleting
                    && modal.error.as_deref() == Some("Not authenticated with Connect")
        ));

        let _ = home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::CloseRemoteModal,
        )));
        assert!(home.delete_remote_cube_modal.is_none());
    }

    #[test]
    fn remote_delete_requires_the_name_when_connect_holds_any_backup() {
        let restorable = remote_cube("remote-a", "Remote A", Network::Bitcoin);
        let mut bare = remote_cube("remote-b", "Remote B", Network::Bitcoin);
        bare.has_recovery_kit = false;
        bare.has_encrypted_seed = false;
        let modal = |cube: RemoteCube, typed: &str| DeleteRemoteCubeModal {
            cube,
            deleting: false,
            error: None,
            typed_name: typed.to_string(),
        };

        assert!(!modal(restorable.clone(), "").name_confirmed());
        assert!(!modal(restorable.clone(), "Remote").name_confirmed());
        assert!(modal(restorable.clone(), " Remote A ").name_confirmed());
        assert!(modal(bare, "").name_confirmed());

        let mut descriptor_only = remote_cube("remote-c", "Remote C", Network::Bitcoin);
        descriptor_only.network = settings::network_to_api_string(ChainId::BitcoinBlake2b);
        descriptor_only.has_encrypted_seed = false;
        let guarded = modal(descriptor_only.clone(), "");
        assert!(guarded.holds_backup());
        assert!(!guarded.restorable());
        assert!(!guarded.name_confirmed());
        assert!(modal(descriptor_only, "Remote C").name_confirmed());

        // Restore instead closes the delete and hands over to the restore.
        let mut home = home();
        home.remote_cubes = vec![restorable.clone()];
        home.delete_remote_cube_modal = Some(modal(restorable, ""));
        let _ = home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::RestoreInstead("remote-a".to_string()),
        )));
        assert!(home.delete_remote_cube_modal.is_none());
    }

    #[test]
    fn loaded_remote_cubes_limits_and_checked_state_update_home_cache() {
        let mut home = home();

        let _ = home.update(Message::CubeLimitsLoaded(Ok(CubeLimitsResponse {
            network: "mainnet".to_string(),
            current_count: 1,
            max_allowed: 4,
        })));
        assert_eq!(home.server_cube_limit, Some(4));

        let remote = remote_cube("local-a", "Remote A", Network::Bitcoin);
        let current = home.remote_cubes_request;
        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: current,
            result: Ok(vec![remote.clone()]),
        });
        assert_eq!(home.remote_cubes.len(), 1);

        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: current,
            result: Err("offline".to_string()),
        });
        assert_eq!(home.remote_cubes.len(), 1);

        let _ = home.update(Message::Checked {
            for_chain: crate::chain::ChainId::Bitcoin,
            res: Ok(State::Cubes {
                cubes: vec![cube("local-a", "Local A", Network::Bitcoin)],
                create_cube: false,
                source: crate::chain::ChainId::Bitcoin,
            }),
        });

        assert!(home.remote_cubes.is_empty());
        assert!(matches!(home.state, State::Cubes { .. }));
    }

    /// Two fetches in flight, the newer one finishing first. The older result
    /// must not be applied: `fetch_remote_cubes` has three unsynchronised
    /// triggers and each call's latency scales with the number of remote cubes
    /// to probe, so arrival order is not issue order. Applying by arrival rolls
    /// the list back to a stale server view.
    #[test]
    fn a_late_remote_cube_fetch_does_not_clobber_a_newer_one() {
        let mut home = signed_in_home();

        // Two overlapping fetches, each claiming its own generation.
        let _ = home.fetch_remote_cubes();
        let older = home.remote_cubes_request;
        let _ = home.fetch_remote_cubes();
        let newer = home.remote_cubes_request;
        assert_ne!(older, newer, "each fetch must claim its own generation");

        // The newer fetch lands first and is applied.
        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: newer,
            result: Ok(vec![remote_cube("newer", "Newer", Network::Bitcoin)]),
        });
        assert_eq!(home.remote_cubes.len(), 1);
        assert_eq!(home.remote_cubes[0].uuid, "newer");

        // The older one straggles in afterwards and must be dropped.
        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: older,
            result: Ok(vec![
                remote_cube("stale-a", "Stale A", Network::Bitcoin),
                remote_cube("stale-b", "Stale B", Network::Bitcoin),
            ]),
        });
        assert_eq!(
            home.remote_cubes.len(),
            1,
            "a superseded fetch must not overwrite the newer server view"
        );
        assert_eq!(home.remote_cubes[0].uuid, "newer");
    }

    /// Deleting a remote Cube while a fetch is in flight. The fetch was issued
    /// before the delete settled, so its list still contains the deleted Cube —
    /// applying it would put the row the user just deleted back on screen. The
    /// delete branch must invalidate it and re-fetch.
    #[test]
    fn a_fetch_in_flight_at_remote_delete_cannot_restore_the_deleted_cube() {
        let mut home = signed_in_home();
        home.remote_cubes = vec![
            remote_cube("remote-a", "Remote A", Network::Bitcoin),
            remote_cube("remote-b", "Remote B", Network::Bitcoin),
        ];

        // A fetch starts, then the delete for "remote-a" confirms and settles
        // before that fetch's response arrives.
        let _ = home.fetch_remote_cubes();
        let inflight = home.remote_cubes_request;

        let _ = home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::ShowRemoteModal("remote-a".to_string()),
        )));
        let _ = home.update(Message::RemoteCubeDeleted(Ok(())));

        assert!(
            home.delete_remote_cube_modal.is_none(),
            "a settled delete must close its modal"
        );
        assert_eq!(
            home.remote_cubes
                .iter()
                .map(|rc| rc.uuid.as_str())
                .collect::<Vec<_>>(),
            vec!["remote-b"],
        );
        assert_ne!(
            home.remote_cubes_request, inflight,
            "the delete must supersede the pre-delete fetch"
        );

        // The pre-delete response straggles in, still listing the deleted Cube.
        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: inflight,
            result: Ok(vec![
                remote_cube("remote-a", "Remote A", Network::Bitcoin),
                remote_cube("remote-b", "Remote B", Network::Bitcoin),
            ]),
        });
        assert_eq!(
            home.remote_cubes
                .iter()
                .map(|rc| rc.uuid.as_str())
                .collect::<Vec<_>>(),
            vec!["remote-b"],
            "a fetch issued before the delete must not restore the deleted Cube"
        );

        // The refreshed fetch the delete kicked off is authoritative.
        let refreshed = home.remote_cubes_request;
        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: refreshed,
            result: Ok(vec![remote_cube("remote-b", "Remote B", Network::Bitcoin)]),
        });
        assert_eq!(
            home.remote_cubes
                .iter()
                .map(|rc| rc.uuid.as_str())
                .collect::<Vec<_>>(),
            vec!["remote-b"],
        );
    }

    /// Logging out while a fetch is in flight. The logout branch clears
    /// `remote_cubes`, but the running fetch belongs to the account just left —
    /// if its result still applied, the previous account's Cubes would reappear
    /// on screen after sign-out.
    #[test]
    fn a_fetch_in_flight_at_logout_cannot_repopulate_the_list() {
        use crate::app::view::ConnectAccountMessage;

        let mut home = signed_in_home();
        let _ = home.fetch_remote_cubes();
        let inflight = home.remote_cubes_request;

        // Real logout path, not a direct poke at the generation: this is the
        // branch that clears the list, and it has to invalidate too.
        let _ = home.update(Message::View(ViewMessage::ConnectAccount(
            ConnectAccountMessage::LogOut,
        )));
        assert!(!home.connect_account.is_authenticated());
        assert!(home.remote_cubes.is_empty());

        let _ = home.update(Message::RemoteCubesLoaded {
            request_id: inflight,
            result: Ok(vec![remote_cube(
                "prior-account",
                "Prior Account Cube",
                Network::Bitcoin,
            )]),
        });
        assert!(
            home.remote_cubes.is_empty(),
            "a fetch issued for the signed-out account must not repopulate the list"
        );
    }

    /// A refusal reason describes what *one account's* server said. The local
    /// Cube outlives the sign-out, so a reason left behind would be shown under
    /// the next account — rendered with that account's tier and limits — to
    /// someone who may have slots to spare.
    #[test]
    fn a_refusal_reason_does_not_follow_the_local_cube_into_the_next_account() {
        use crate::app::view::ConnectAccountMessage;

        let mut home = signed_in_home();
        let local = cube("local-a", "Local A", Network::Bitcoin);
        home.state = State::Cubes {
            cubes: vec![local.clone()],
            create_cube: false,
            source: crate::chain::ChainId::Bitcoin,
        };
        home.cube_sync_errors.insert(
            local.id.clone(),
            "Cube limit reached for this network".to_string(),
        );

        // Real logout path — the branch that already clears the rest of the
        // account-scoped state.
        let _ = home.update(Message::View(ViewMessage::ConnectAccount(
            ConnectAccountMessage::LogOut,
        )));
        assert!(!home.connect_account.is_authenticated());

        assert!(
            home.cube_sync_hint(&local).is_none(),
            "the previous account's refusal must not describe this Cube under the next one"
        );
    }

    #[test]
    fn rename_modal_edits_and_cancels_without_touching_disk() {
        let mut home = home();
        home.state = State::Cubes {
            cubes: vec![cube("local-a", "Local A", Network::Bitcoin)],
            create_cube: false,
            source: crate::chain::ChainId::Bitcoin,
        };

        let _ = home.update(Message::View(ViewMessage::RenameCube(0)));
        let key = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "local-a".into(),
        };
        assert_eq!(
            home.rename_cube_modal,
            Some((key.clone(), "Local A".to_string()))
        );

        let _ = home.update(Message::View(ViewMessage::RenameCubeNameEdited(
            "Renamed".to_string(),
        )));
        assert_eq!(home.rename_cube_modal, Some((key, "Renamed".to_string())));

        let _ = home.update(Message::View(ViewMessage::RenameCubeCancel));
        assert!(home.rename_cube_modal.is_none());
    }

    #[test]
    fn a_blake2b_cube_renames_on_its_own_launcher() {
        let datadir = fresh_datadir();
        let directory = datadir.network_directory(ChainId::BitcoinBlake2b);
        std::fs::create_dir_all(directory.path()).unwrap();
        let fork =
            CubeSettings::new_with_raw_id("fork".into(), "Fork".into(), ChainId::BitcoinBlake2b);
        let settings_path = directory.path().join(settings::SETTINGS_FILE_NAME);
        std::fs::write(
            &settings_path,
            serde_json::to_vec(&settings::Settings {
                cubes: vec![fork.clone()],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let mut home = Home::new_for_chain(datadir.clone(), Some(ChainId::Bitcoin)).0;
        enable_btcb2(&mut home);
        home.network = ChainId::BitcoinBlake2b;
        home.state = State::Cubes {
            cubes: vec![fork.clone()],
            create_cube: false,
            source: ChainId::BitcoinBlake2b,
        };
        let key = CubeCatalogKey {
            chain: ChainId::BitcoinBlake2b,
            cube_id: fork.id.clone(),
        };

        drop(home.update(Message::View(ViewMessage::RenameCatalog(key.clone()))));
        assert_eq!(home.rename_cube_modal, Some((key, "Fork".to_string())));
        drop(home.update(Message::View(ViewMessage::RenameCubeNameEdited(
            "Fork renamed".into(),
        ))));
        let output = drain(home.update(Message::View(ViewMessage::RenameCubeConfirm)));
        assert!(matches!(output.as_slice(), [Message::CubeRenamed(Ok(()))]));
        assert!(home.error().is_none());
        let saved: settings::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(saved.cubes[0].name, "Fork renamed");

        // The local write's result closes the dialog on this chain too.
        drop(home.update(Message::CubeRenamed(Ok(()))));
        assert!(home.rename_cube_modal.is_none());
        assert!(home.error().is_none());
    }

    #[test]
    fn a_blake2b_connect_only_cube_restores_or_deletes_from_connect() {
        let mut home = home();
        enable_btcb2(&mut home);
        home.network = ChainId::BitcoinBlake2b;
        home.state = State::NoCube { create_cube: false };
        let mut kept = remote_cube("kept", "Kept", Network::Bitcoin);
        kept.network = settings::network_to_api_string(ChainId::BitcoinBlake2b);
        home.remote_cubes = vec![kept];
        assert_eq!(home.total_cube_count(), 1);

        let open = Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(
            "kept".to_string(),
        )));
        drop(home.update(open));
        assert!(home.error().is_none());
        let modal = home
            .delete_remote_cube_modal
            .as_ref()
            .expect("remote modal");
        // Connect holds its Recovery Kit, and it restores as on Bitcoin.
        assert!(modal.restorable());
        assert!(modal.holds_backup());

        // Restore instead closes the modal and starts the restore.
        let restore = drain(home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::RestoreInstead("kept".to_string()),
        ))));
        assert!(home.error().is_none());
        assert!(home.delete_remote_cube_modal.is_none());
        assert!(
            home.recovery_method_modal.is_some()
                || matches!(
                    restore.as_slice(),
                    [Message::Install(_, ChainId::BitcoinBlake2b, _, _)]
                ),
            "{:?}",
            restore.len()
        );
        home.recovery_method_modal = None;
        drop(home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::ShowRemoteModal("kept".to_string()),
        ))));
        assert!(home.delete_remote_cube_modal.is_some());

        // Its backup goes with it, so Delete still waits for its name.
        drop(home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::ConfirmRemoteDelete("kept".to_string()),
        ))));
        assert!(!home.delete_remote_cube_modal.as_ref().unwrap().deleting);
        drop(home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::RemoteNameInput("Kept".to_string()),
        ))));
        drop(home.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::ConfirmRemoteDelete("kept".to_string()),
        ))));
        assert!(home.error().is_none());
        assert!(home.delete_remote_cube_modal.as_ref().unwrap().deleting);

        // Connect's answer frees the slot.
        drop(home.update(Message::RemoteCubeDeleted(Ok(()))));
        assert!(home.error().is_none());
        assert!(home.delete_remote_cube_modal.is_none());
        assert!(home.remote_cubes.is_empty());
        assert_eq!(home.total_cube_count(), 0);
    }

    #[test]
    fn rename_confirmation_resolves_the_same_cube_after_reorder_and_refuses_removal() {
        let datadir = fresh_datadir();
        let directory = datadir.network_directory(ChainId::Bitcoin);
        std::fs::create_dir_all(directory.path()).unwrap();
        let first = CubeSettings::new_with_raw_id("first".into(), "First".into(), ChainId::Bitcoin);
        let second =
            CubeSettings::new_with_raw_id("second".into(), "Second".into(), ChainId::Bitcoin);
        let settings_path = directory.path().join(settings::SETTINGS_FILE_NAME);
        std::fs::write(
            &settings_path,
            serde_json::to_vec(&settings::Settings {
                cubes: vec![first.clone(), second.clone()],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let mut home = Home::new_for_chain(datadir.clone(), Some(ChainId::Bitcoin)).0;
        home.state = State::Cubes {
            cubes: vec![first.clone(), second.clone()],
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        drop(
            home.update(Message::View(ViewMessage::RenameCatalog(CubeCatalogKey {
                chain: ChainId::Bitcoin,
                cube_id: first.id.clone(),
            }))),
        );
        drop(home.update(Message::View(ViewMessage::RenameCubeNameEdited(
            "Renamed first".into(),
        ))));
        drop(home.update(Message::FocusedCubes {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::Cubes {
                cubes: vec![second.clone(), first.clone()],
                create_cube: false,
                source: ChainId::Bitcoin,
            }),
        }));
        assert!(matches!(&home.state, State::Cubes { cubes, .. }
            if cubes.first().is_some_and(|cube| cube.id == second.id)));
        let output = drain(home.update(Message::View(ViewMessage::RenameCubeConfirm)));
        assert!(matches!(output.as_slice(), [Message::CubeRenamed(Ok(()))]));
        let saved: settings::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(
            saved
                .cubes
                .iter()
                .find(|cube| cube.id == first.id)
                .unwrap()
                .name,
            "Renamed first"
        );
        assert_eq!(
            saved
                .cubes
                .iter()
                .find(|cube| cube.id == second.id)
                .unwrap()
                .name,
            "Second"
        );

        enable_btcb2(&mut home);
        assert!(home.connect_account.is_authenticated());

        // The row may disappear while the dialog remains open. No local or
        // remote rename is then scheduled for whichever row took its place.
        drop(
            home.update(Message::View(ViewMessage::RenameCatalog(CubeCatalogKey {
                chain: ChainId::Bitcoin,
                cube_id: second.id.clone(),
            }))),
        );
        drop(home.update(Message::FocusedCubes {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::Cubes {
                cubes: vec![first.clone()],
                create_cube: false,
                source: ChainId::Bitcoin,
            }),
        }));
        assert!(drain(home.update(Message::View(ViewMessage::RenameCubeConfirm))).is_empty());
        assert!(home.rename_cube_modal.is_none());
        assert!(home.error().is_some());
        assert!(home.pending_remote_rename.is_none());
        let saved: settings::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(
            saved
                .cubes
                .iter()
                .find(|cube| cube.id == second.id)
                .unwrap()
                .name,
            "Second"
        );

        drop(
            home.update(Message::View(ViewMessage::RenameCatalog(CubeCatalogKey {
                chain: ChainId::Bitcoin,
                cube_id: first.id.clone(),
            }))),
        );
        drop(home.update(Message::FocusedCubes {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::Cubes {
                cubes: vec![first.clone(), first.clone()],
                create_cube: false,
                source: ChainId::Bitcoin,
            }),
        }));
        assert!(drain(home.update(Message::View(ViewMessage::RenameCubeConfirm))).is_empty());
        assert!(
            home.rename_cube_modal.is_none(),
            "ambiguous identity invalidates the draft"
        );
        assert!(home.pending_remote_rename.is_none());

        drop(home.update(Message::FocusedCubes {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::Cubes {
                cubes: vec![first.clone()],
                create_cube: false,
                source: ChainId::Bitcoin,
            }),
        }));
        drop(
            home.update(Message::View(ViewMessage::RenameCatalog(CubeCatalogKey {
                chain: ChainId::Bitcoin,
                cube_id: first.id.clone(),
            }))),
        );
        home.network = ChainId::Testnet4;
        assert!(drain(home.update(Message::View(ViewMessage::RenameCubeConfirm))).is_empty());
        assert!(
            home.rename_cube_modal.is_none(),
            "another chain cannot inherit a rename draft"
        );
        assert!(home.pending_remote_rename.is_none());
        let saved: settings::Settings =
            serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert_eq!(
            saved
                .cubes
                .iter()
                .find(|cube| cube.id == first.id)
                .unwrap()
                .name,
            "Renamed first"
        );
        std::fs::remove_dir_all(datadir.path()).unwrap();
    }

    #[test]
    fn no_owner_self_block_is_not_phone_recoverable() {
        assert_eq!(derive_phone_recovery(None), (false, false));
    }

    #[test]
    fn registered_recipient_without_envelope_is_not_recoverable() {
        // The phone can register the `owner-self` key before the desktop seals
        // + uploads anything. A bare recipient must not offer recovery, or the
        // launch would dead-end on an empty-envelope fetch.
        let s = owner_self(true, "full_cube", &[]);
        assert_eq!(derive_phone_recovery(Some(&s)), (false, false));
    }

    #[test]
    fn recipient_absent_but_kinds_present_is_not_recoverable() {
        // has_recipient gates recoverability even if kinds somehow arrive.
        let s = owner_self(false, "full_cube", &["seed", "descriptor"]);
        let (recoverable, _) = derive_phone_recovery(Some(&s));
        assert!(!recoverable);
    }

    #[test]
    fn sealed_seed_envelope_is_full_cube_recoverable() {
        let s = owner_self(true, "full_cube", &["descriptor", "seed"]);
        assert_eq!(derive_phone_recovery(Some(&s)), (true, true));
    }

    #[test]
    fn sealed_descriptor_only_envelope_is_vault_only_recoverable() {
        // Descriptor sealed but no seed → recoverable, but Vault-only scope.
        let s = owner_self(true, "vault_only", &["descriptor"]);
        assert_eq!(derive_phone_recovery(Some(&s)), (true, false));
    }

    #[test]
    fn view_builds_home_sections_and_create_recovery_states() {
        let mut home = home();
        home.view();

        home.state = State::NoCube { create_cube: false };
        home.error = Some("create error".to_string());
        home.view();

        home.state = State::RecoveryInput;
        home.recovery_words[0] = "ab".to_string();
        home.recovery_active_index = Some(0);
        home.view();

        home.state = State::Cubes {
            cubes: vec![cube("local-a", "Local A", Network::Bitcoin)],
            create_cube: true,
            source: crate::chain::ChainId::Bitcoin,
        };
        home.create_cube_name.value = "New Cube".to_string();
        home.create_cube_name.valid = true;
        home.create_cube_pin.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        home.create_cube_pin_confirm.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        home.view();

        let mut signed_in = signed_in_home();
        signed_in.connect_expanded = true;
        signed_in.state = State::Cubes {
            cubes: vec![cube("local-a", "Local A", Network::Bitcoin)],
            create_cube: false,
            source: crate::chain::ChainId::Bitcoin,
        };
        signed_in.remote_cubes = vec![remote_cube("remote-a", "Remote A", Network::Bitcoin)];
        signed_in.view();

        signed_in.active_section = HomeSection::Connect(app::menu::ConnectSubMenu::Security);
        signed_in.view();

        signed_in.active_section = HomeSection::RecoverVault;
        signed_in.view();
    }

    // ── Duress launch kill-switch route backstop (PLAN-feature-flags PR 2) ──

    /// A Pro plan carrying the paid `duress` entitlement.
    fn duress_entitled_plan() -> crate::services::coincube::ConnectPlan {
        use crate::services::coincube::{ConnectPlan, PlanEntitlements, PlanStatus, PlanTier};
        ConnectPlan {
            plan: PlanTier::Pro,
            status: PlanStatus::Active,
            renewal_at: None,
            entitlements: PlanEntitlements {
                duress: true,
                ..PlanEntitlements::default()
            },
            billing_cycle: None,
            plan_provenance: None,
            pending_plan: None,
            pending_plan_at: None,
            pending_billing_cycle: None,
            payment_method: None,
            auto_renew: None,
            next_charge_at: None,
            next_charge_amount: None,
            grace_ends_at: None,
        }
    }

    /// A features payload with the duress launch flag explicitly off.
    fn features_duress_off() -> crate::services::coincube::FeaturesResponse {
        crate::services::coincube::FeaturesResponse {
            plans: Vec::new(),
            pricing_schema_version: None,
            purchasing_enabled: None,
            marketplace_enabled: None,
            liquid_enabled: None,
            buy_sell_enabled: None,
            p2p_enabled: None,
            duress_enabled: Some(false),
            bitcoin_blake2b_enabled: None,
            card_payments_enabled: None,
        }
    }

    #[test]
    fn duress_route_redirects_to_overview_when_gated_off() {
        // Entitled Pro, launch flag off, not enrolled → the surface is hidden.
        // A programmatic or restored navigation to it fails closed onto
        // Overview rather than rendering the gated-off panel.
        let mut home = signed_in_home();
        home.connect_account.plan = Some(duress_entitled_plan());
        home.connect_account.features = Some(features_duress_off());
        assert!(!home.connect_account.show_duress());

        let _ = home.update(Message::View(ViewMessage::GoToSection(
            HomeSection::Connect(app::menu::ConnectSubMenu::Duress),
        )));

        assert_eq!(
            home.active_section,
            HomeSection::Connect(app::menu::ConnectSubMenu::Overview)
        );
        assert_eq!(
            home.connect_account.active_sub,
            app::menu::ConnectSubMenu::Overview
        );
    }

    #[test]
    fn duress_route_reachable_when_enrolled_with_flag_off() {
        // Grandfathered: enrolled on this device, launch flag off → the surface
        // stays fully reachable. Navigation to it sticks rather than redirecting.
        let mut home = signed_in_home();
        home.connect_account.plan = Some(duress_entitled_plan());
        home.connect_account.features = Some(features_duress_off());
        home.connect_account.duress_locally_armed = true;
        assert!(home.connect_account.show_duress());

        let _ = home.update(Message::View(ViewMessage::GoToSection(
            HomeSection::Connect(app::menu::ConnectSubMenu::Duress),
        )));

        assert_eq!(
            home.active_section,
            HomeSection::Connect(app::menu::ConnectSubMenu::Duress)
        );
    }

    fn registered_cube_response(uuid: &str) -> CubeResponse {
        CubeResponse {
            id: 193,
            uuid: uuid.to_string(),
            name: "Local A".to_string(),
            network: "bitcoin".to_string(),
            lightning_address: None,
            status: "active".to_string(),
            has_recovery_kit: false,
            has_vault: None,
            spark_stable_balance: None,
            encryption_pubkey: None,
            members: Vec::new(),
            pending_invites: Vec::new(),
            vault: None,
        }
    }

    #[test]
    fn launcher_catalog_uses_sanitized_names_and_chain_bound_ids() {
        assert_eq!(
            catalog_display_name("  Family\n\u{202e} Cube  "),
            "Family Cube"
        );
        assert_eq!(catalog_display_name("\u{0000}\n"), "Unnamed Cube");

        let mut home = home();
        home.network = ChainId::Bitcoin;
        home.state = State::Cubes {
            cubes: vec![
                CubeSettings::new_with_raw_id("b".into(), "B".into(), ChainId::Bitcoin),
                CubeSettings::new_with_raw_id("a".into(), "A".into(), ChainId::Bitcoin),
            ],
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        let key = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: "a".into(),
        };
        assert_eq!(home.catalog_index(&key), Some(1));
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes.reverse();
        }
        assert_eq!(home.catalog_index(&key), Some(0));
        assert_eq!(
            home.catalog_index(&CubeCatalogKey {
                chain: ChainId::Testnet4,
                cube_id: "a".into(),
            }),
            None
        );
        if let State::Cubes { cubes, .. } = &mut home.state {
            cubes.push(cubes[0].clone());
        }
        assert_eq!(home.catalog_index(&key), None);
    }

    #[test]
    fn recovery_service_open_resolves_exact_server_and_local_identity() {
        let datadir = fresh_datadir();
        let directory = datadir.network_directory(ChainId::Bitcoin);
        std::fs::create_dir_all(directory.path()).unwrap();
        let local = CubeSettings::new_with_raw_id(
            "local-uuid".to_string(),
            "Local Cube".to_string(),
            ChainId::Bitcoin,
        );
        std::fs::write(
            directory.path().join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings::Settings {
                cubes: vec![local],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();
        let target = RecoveryServiceTarget {
            cube_id: 193,
            network: "mainnet".to_string(),
        };
        let mut server = registered_cube_response("local-uuid");
        server.network = "mainnet".to_string();
        assert!(matches!(
            resolve_recovery_service_destination(&datadir, &target, &[server.clone()]).unwrap(),
            RecoveryServiceDestination::Local(ChainId::Bitcoin, cube, _) if cube.id == "local-uuid"
        ));
        assert!(matches!(
            resolve_recovery_service_destination(
                &datadir,
                &target,
                &[CubeResponse {
                    uuid: "other-device-uuid".to_string(),
                    ..server.clone()
                }],
            )
            .unwrap(),
            RecoveryServiceDestination::Remote(ChainId::Bitcoin)
        ));
        let mut wrong_network = server.clone();
        wrong_network.network = "testnet4".to_string();
        assert!(resolve_recovery_service_destination(&datadir, &target, &[wrong_network]).is_err());
        assert!(
            resolve_recovery_service_destination(&datadir, &target, &[server.clone(), server])
                .is_err()
        );
        std::fs::remove_dir_all(datadir.path()).unwrap();
    }

    #[test]
    fn recovery_service_completion_only_opens_the_current_account_selection() {
        let mut home = signed_in_home();
        home.active_section = HomeSection::Connect(app::menu::ConnectSubMenu::Overview);
        let target = RecoveryServiceTarget {
            cube_id: 193,
            network: "mainnet".to_string(),
        };
        let mut server = registered_cube_response("local-uuid");
        server.network = "mainnet".to_string();
        home.connect_account.recovery_services = RecoveryServicesState::Ready(vec![server.into()]);
        home.recovery_open_request = 4;
        let generation = home.connect_account.session_generation();
        let result = || {
            Ok(RecoveryServiceDestination::Local(
                ChainId::Bitcoin,
                CubeSettings::new_with_raw_id(
                    "local-uuid".to_string(),
                    "Local Cube".to_string(),
                    ChainId::Bitcoin,
                ),
                app::Config::new(false),
            ))
        };

        assert!(drain(home.update(Message::RecoveryServiceResolved {
            target: target.clone(),
            session_generation: generation,
            user_id: 7,
            request: 3,
            result: result(),
        }))
        .is_empty());
        assert!(drain(home.update(Message::RecoveryServiceResolved {
            target: target.clone(),
            session_generation: generation,
            user_id: 8,
            request: 4,
            result: result(),
        }))
        .is_empty());
        let outputs = drain(home.update(Message::RecoveryServiceResolved {
            target,
            session_generation: generation,
            user_id: 7,
            request: 4,
            result: result(),
        }));
        assert!(
            matches!(outputs.as_slice(), [Message::RunToRecovery(_, _, ChainId::Bitcoin, cube)] if cube.id == "local-uuid")
        );
    }

    #[test]
    fn remote_fork_recovery_selection_stays_on_connect_with_a_truthful_error() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let mut home = signed_in_home();
            enable_btcb2(&mut home);
            home.global_blake2b_enabled = true;
            home.developer_mode = true; // testnet4 is available only in developer mode
            home.active_section = HomeSection::Connect(app::menu::ConnectSubMenu::Overview);
            let original_network = home.network;
            let target = RecoveryServiceTarget {
                cube_id: 193,
                network: settings::network_to_api_string(chain),
            };
            let mut server = registered_cube_response("remote-only-fork");
            server.network = target.network.clone();
            home.connect_account.recovery_services =
                RecoveryServicesState::Ready(vec![server.into()]);
            drop(home.update(Message::View(ViewMessage::ConnectAccount(
                ConnectAccountMessage::OpenRecoveryService(target.clone()),
            ))));
            let request = home.recovery_open_request;
            assert!(drain(home.update(Message::RecoveryServiceResolved {
                target,
                session_generation: home.connect_account.session_generation(),
                user_id: 7,
                request,
                result: Ok(RecoveryServiceDestination::Remote(chain)),
            }))
            .is_empty());
            assert!(matches!(
                home.active_section,
                HomeSection::Connect(app::menu::ConnectSubMenu::Overview)
            ));
            assert_eq!(home.network, original_network);
            let error = &home.connect_account.error.as_ref().unwrap().user;
            assert!(error.guidance.contains("original device"));
            assert!(!error.retryable);
        }
    }

    #[test]
    fn remote_bitcoin_recovery_selection_opens_the_catalog() {
        let mut home = signed_in_home();
        home.active_section = HomeSection::Connect(app::menu::ConnectSubMenu::Overview);
        let target = RecoveryServiceTarget {
            cube_id: 193,
            network: "mainnet".to_string(),
        };
        let mut server = registered_cube_response("remote-bitcoin");
        server.network = target.network.clone();
        home.connect_account.recovery_services = RecoveryServicesState::Ready(vec![server.into()]);
        drop(home.update(Message::View(ViewMessage::ConnectAccount(
            ConnectAccountMessage::OpenRecoveryService(target.clone()),
        ))));
        let request = home.recovery_open_request;
        // The fetch Task is allowed to remain pending; the selected route must
        // already be the remote-only catalog that can display this chain.
        drop(home.update(Message::RecoveryServiceResolved {
            target,
            session_generation: home.connect_account.session_generation(),
            user_id: 7,
            request,
            result: Ok(RecoveryServiceDestination::Remote(ChainId::Bitcoin)),
        }));
        assert_eq!(home.active_section, HomeSection::Cubes);
        assert_eq!(home.network, ChainId::Bitcoin);
        assert!(home.connect_account.error.is_none());
    }

    #[test]
    fn recovery_selection_is_invalidated_on_security_plan_and_leave_return() {
        for leave in [
            HomeSection::Connect(app::menu::ConnectSubMenu::Security),
            HomeSection::Connect(app::menu::ConnectSubMenu::PlanBilling),
        ] {
            let mut home = signed_in_home();
            home.active_section = HomeSection::Connect(app::menu::ConnectSubMenu::Overview);
            let target = RecoveryServiceTarget {
                cube_id: 193,
                network: "mainnet".to_string(),
            };
            let mut server = registered_cube_response("local-uuid");
            server.network = "mainnet".to_string();
            home.connect_account.recovery_services =
                RecoveryServicesState::Ready(vec![server.into()]);
            drop(home.update(Message::View(ViewMessage::ConnectAccount(
                ConnectAccountMessage::OpenRecoveryService(target.clone()),
            ))));
            let old_request = home.recovery_open_request;
            let session_generation = home.connect_account.session_generation();
            if matches!(
                leave,
                HomeSection::Connect(app::menu::ConnectSubMenu::PlanBilling)
            ) {
                drop(home.update(Message::View(ViewMessage::ConnectAccount(
                    ConnectAccountMessage::OpenPlanBilling,
                ))));
            } else {
                drop(home.update(Message::View(ViewMessage::GoToSection(leave))));
            }
            assert_ne!(home.recovery_open_request, old_request);
            let completion = |request| Message::RecoveryServiceResolved {
                target: target.clone(),
                session_generation,
                user_id: 7,
                request,
                result: Ok(RecoveryServiceDestination::Remote(ChainId::Bitcoin)),
            };
            assert!(drain(home.update(completion(old_request))).is_empty());
            assert_ne!(home.active_section, HomeSection::Cubes);
            drop(home.update(Message::View(ViewMessage::GoToSection(
                HomeSection::Connect(app::menu::ConnectSubMenu::Overview),
            ))));
            assert!(drain(home.update(completion(old_request))).is_empty());
            assert_ne!(home.active_section, HomeSection::Cubes);
            // Returning to Overview refreshes the row list. Model that
            // completion before making a new selection; the old selection
            // must still remain invalid across the page visit.
            let mut server = registered_cube_response("local-uuid");
            server.network = "mainnet".to_string();
            home.connect_account.recovery_services =
                RecoveryServicesState::Ready(vec![server.into()]);
            drop(home.update(Message::View(ViewMessage::ConnectAccount(
                ConnectAccountMessage::OpenRecoveryService(target.clone()),
            ))));
            assert!(home.recovery_open_request > old_request);
            // A fresh selection on the returned Overview is still actionable.
            let fresh = home.recovery_open_request;
            drop(home.update(completion(fresh)));
            assert_eq!(home.active_section, HomeSection::Cubes);
        }
    }

    /// The client-side limit check is advisory; the server is authoritative and
    /// can refuse a Cube that was already created locally. When it does, the
    /// Cubes list must stop promising a sync that will never happen.
    #[test]
    fn a_refused_cube_reports_the_servers_reason_not_a_promise_to_sync() {
        let mut home = signed_in_home();
        let local = cube("local-a", "Local A", Network::Bitcoin);
        home.state = State::Cubes {
            cubes: vec![local.clone()],
            create_cube: false,
            source: crate::chain::ChainId::Bitcoin,
        };
        home.remote_cubes = vec![remote_cube("remote-a", "Remote A", Network::Bitcoin)];
        home.server_cube_limit = Some(2);

        // Nothing has failed yet, so the Cube genuinely is mid-sync.
        assert!(home.cube_sync_hint(&local).is_none());

        // The limit refusal is expanded into copy that says where the user
        // stands and what to do about it — the bare server string does not.
        home.cube_sync_errors.insert(
            local.id.clone(),
            "Cube limit reached for this network".to_string(),
        );
        let hint = home
            .cube_sync_hint(&local)
            .expect("a refusal must be shown");
        assert!(hint.contains("Out of Cube slots"), "{}", hint);
        assert!(hint.contains("2/2"), "{}", hint);
        assert!(hint.contains("Upgrade"), "{}", hint);

        // A Cube listed from another network's read-only catalog is described
        // by its own network's count, not the current network's.
        let mut other = local.clone();
        other.id = "testnet4-cube".to_string();
        other.network = ChainId::Testnet4;
        home.network_catalog = vec![(ChainId::Testnet4, vec![other.clone()])];
        home.cube_sync_errors.insert(
            other.id.clone(),
            "Cube limit reached for this network".to_string(),
        );
        let hint = home
            .cube_sync_hint(&other)
            .expect("a refusal must be shown");
        assert!(hint.contains("Testnet4: 1/2"), "{}", hint);
        assert!(!hint.contains("Bitcoin mainnet"), "{}", hint);

        // Any other refusal is surfaced verbatim rather than mislabelled.
        home.cube_sync_errors
            .insert(local.id.clone(), "Network unreachable".to_string());
        let hint = home
            .cube_sync_hint(&local)
            .expect("a refusal must be shown");
        assert!(hint.contains("Network unreachable"), "{}", hint);
        assert!(!hint.contains("Out of Cube slots"), "{}", hint);
    }

    /// The catch-up sync speaks for the Cubes it examined, and only those: a
    /// Cube it synced loses its reason, one it could not sync gains the current
    /// one, and a Cube outside the round is left alone. That last part matters
    /// because the round works from a snapshot taken when it started — a Cube
    /// created and refused while it was running is not in it, and a blanket
    /// replace would erase what `CubeRemoteRegistered` had just recorded.
    #[test]
    fn catch_up_sync_results_apply_only_to_the_cubes_it_examined() {
        let mut home = signed_in_home();
        home.cube_sync_errors
            .insert("went-through".to_string(), "Cube limit reached".to_string());
        home.cube_sync_errors.insert(
            "refused-mid-round".to_string(),
            "Cube limit reached for this network".to_string(),
        );

        let _ = home.update(Message::CatchUpSyncFinished(Ok(vec![
            ("went-through".to_string(), None),
            (
                "still-stuck".to_string(),
                Some("Cube limit reached for this network".to_string()),
            ),
        ])));

        assert!(
            !home.cube_sync_errors.contains_key("went-through"),
            "a Cube the round synced must lose its stale reason"
        );
        assert_eq!(
            home.cube_sync_errors.get("still-stuck").map(String::as_str),
            Some("Cube limit reached for this network"),
        );
        assert_eq!(
            home.cube_sync_errors
                .get("refused-mid-round")
                .map(String::as_str),
            Some("Cube limit reached for this network"),
            "a Cube created after the round snapshotted the unsynced set must \
             keep the reason its own registration recorded",
        );
    }

    /// A sync that never ran has no verdict to give. Wiping the recorded
    /// reasons on an aborted round would drop a refused Cube back to "it will
    /// sync automatically in a moment" — the false promise the map replaces —
    /// on nothing more than a flaky `list_cubes` during sign-in.
    #[test]
    fn an_aborted_catch_up_sync_keeps_what_is_already_known() {
        let mut home = signed_in_home();
        home.cube_sync_errors.insert(
            "refused".to_string(),
            "Cube limit reached for this network".to_string(),
        );

        let _ = home.update(Message::CatchUpSyncFinished(Err(())));

        assert_eq!(
            home.cube_sync_errors.get("refused").map(String::as_str),
            Some("Cube limit reached for this network"),
            "an aborted round must not be read as \"nothing is wrong\"",
        );

        // A round that *did* examine it still gets the last word.
        let _ = home.update(Message::CatchUpSyncFinished(Ok(vec![(
            "refused".to_string(),
            None,
        )])));
        assert!(
            home.cube_sync_errors.is_empty(),
            "a completed round that synced the Cube must clear its reason"
        );
    }

    /// A successful registration clears the reason; a failed one records it.
    #[test]
    fn remote_registration_outcomes_track_the_sync_reason() {
        let dir = tmp_datadir("sync-reason");
        let mut home = signed_in_home_with_datadir(&dir);

        let _ = home.update(Message::CubeRemoteRegistered {
            cube_id: "c1".to_string(),
            network: crate::chain::ChainId::Bitcoin,
            result: Err("Cube limit reached for this network".to_string()),
        });
        assert_eq!(
            home.cube_sync_errors.get("c1").map(String::as_str),
            Some("Cube limit reached for this network"),
        );

        let _ = home.update(Message::CubeRemoteRegistered {
            cube_id: "c1".to_string(),
            network: crate::chain::ChainId::Bitcoin,
            result: Ok(registered_cube_response("c1")),
        });
        assert!(
            !home.cube_sync_errors.contains_key("c1"),
            "a Cube that registered must not keep showing a warning"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pure_home_view_helpers_build_for_local_remote_and_form_variants() {
        use launcher_catalog::{recovery_kit_caption, remote_restore, Tone};
        let mut local = cube("local-a", "Local A", Network::Bitcoin);
        assert_eq!(
            recovery_kit_caption(&local, false, None),
            (Tone::Warn, "None — local only".to_string())
        );
        // Sovereign + signed in, with the server's refusal recorded: the
        // warning variant, carrying the server's reason as visible text.
        assert_eq!(
            recovery_kit_caption(&local, true, Some("Out of Cube slots".to_string())),
            (Tone::Warn, "Out of Cube slots".to_string())
        );
        assert_eq!(
            recovery_kit_caption(&local, true, None),
            (Tone::Muted, "None yet — syncing to Connect".to_string())
        );

        local.remote_synced = true;
        assert_eq!(
            recovery_kit_caption(&local, true, None),
            (Tone::Muted, "None yet".to_string())
        );

        local.recovery_kit_last_backed_up_descriptor_fingerprint = Some("hash".to_string());
        assert_eq!(
            recovery_kit_caption(&local, true, None),
            (Tone::Good, "On Connect".to_string())
        );

        let mut remote = remote_cube("remote-a", "Remote A", Network::Bitcoin);
        let restore = remote_restore(&remote);
        assert_eq!(restore.status, "Recovery Kit available");
        assert!(matches!(
            restore.action,
            Some((ViewMessage::RestoreFromRecoveryKit(ref uuid), _)) if uuid == "remote-a"
        ));
        remote.has_encrypted_seed = false;
        let restore = remote_restore(&remote);
        assert!(restore.action.is_none());
        assert!(restore
            .status
            .starts_with("Recovery Kit has no Master Seed"));

        let valid_name = coincube_ui::component::form::Value {
            value: "New Cube".to_string(),
            valid: true,
            warning: None,
        };
        let invalid_name = coincube_ui::component::form::Value {
            value: String::new(),
            valid: false,
            warning: None,
        };
        let mut pin = pin_input::PinInput::new();
        pin.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        let _ = create_cube_form(
            &valid_name,
            &pin,
            &pin,
            &None,
            CreateCubeFormOptions {
                spark_available: true,
                creating_cube: false,
                passkey_mode: false,
                importing_bitcoin: false,
                can_import_bitcoin: true,
                submit_in_form: true,
            },
        );
        let _ = create_cube_form(
            &invalid_name,
            &pin_input::PinInput::new(),
            &pin_input::PinInput::new(),
            &Some("form error".to_string()),
            CreateCubeFormOptions {
                spark_available: false,
                creating_cube: true,
                passkey_mode: true,
                importing_bitcoin: true,
                can_import_bitcoin: true,
                submit_in_form: true,
            },
        );

        let mut words: [String; 12] = Default::default();
        words[0] = "ab".to_string();
        let _ = recovery_input_view(&words, Some(0), false);
        words.fill("abandon".to_string());
        let _ = recovery_input_view(&words, None, true);
    }

    /// Tenshu v175 hides Liquid (master §3 decision 2), so creating or
    /// recovering a Cube names no Liquid wallet. Restore this copy when
    /// Liquid reopens.
    #[test]
    fn create_and_recovery_copy_names_no_liquid_wallet() {
        use crate::app::view::shared::landing::capture;
        fn inspect(content: Element<'_, Message>) -> capture::Inspection {
            capture::inspect_full_window(
                "create-recovery-no-liquid",
                content,
                (1440, 900),
                "dark",
                theme::Theme::dark(),
            )
        }
        let mut home = home();
        home.state = State::NoCube { create_cube: true };
        home.developer_mode = true;
        home.displayed_networks = vec![
            ChainId::Bitcoin,
            ChainId::Testnet,
            ChainId::Testnet4,
            ChainId::Signet,
            ChainId::Regtest,
        ];

        home.create_network_menu = true;
        let networks = inspect(home.create_journey_view());
        assert!(networks.contains("Spark · Vault"));
        for chain in ["Testnet", "Testnet4", "Signet"] {
            assert!(networks.contains(&format!("Vault only. Spark isn't available on {chain}.")));
        }
        assert!(!networks.contains("Liquid"));

        home.create_network_menu = false;
        let cube = inspect(home.create_journey_view());
        assert!(cube.contains("A Cube is an account. It holds a Spark wallet and a Vault."));
        assert!(!cube.contains("Liquid"));

        // Without Spark on the network, the Cube copy does not promise it.
        home.network = ChainId::Testnet4;
        let cube = inspect(home.create_journey_view());
        assert!(cube.contains("A Cube is an account. On this network it holds a Vault."));
        assert!(!cube.contains("Spark wallet"));

        let words: [String; 12] = Default::default();
        let recovery = inspect(recovery_input_view(&words, None, false).map(Message::View));
        assert!(recovery.contains("Enter your 12-word recovery phrase to restore your Cube."));
        assert!(!recovery.contains("Liquid"));
    }

    /// Deleting a Cube right after locking it: its daemon may still be
    /// stopping. The confirm must return at once and leave the files alone,
    /// and the delete runs when the daemon has stopped. Before, the update
    /// blocked the UI thread in `delete_wallet` for up to two minutes.
    #[test]
    fn a_delete_waits_for_a_stopping_daemon_off_the_ui_thread() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let root = std::env::temp_dir().join(format!("delete-stopping-{}", uuid::Uuid::new_v4()));
        let datadir = CoincubeDirectory::new(root.clone());
        let network_dir = datadir.network_directory(Network::Bitcoin);
        let wallet = WalletSettings {
            name: "Coincube-kt6ht0kt".to_string(),
            alias: None,
            descriptor_checksum: "kt6ht0kt".to_string(),
            pinned_at: Some(1_720_000_000),
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: Some(false),
            pending_rescan: None,
            keychain_keys_recorded: false,
        };
        let wallet_dir = network_dir
            .coincubed_data_directory(&wallet.wallet_id())
            .path()
            .to_path_buf();
        std::fs::create_dir_all(&wallet_dir).unwrap();
        std::fs::write(wallet_dir.join("coincubed.sqlite3"), b"synthetic").unwrap();
        let cube = cube("local-a", "Local A", Network::Bitcoin);
        let mut modal = DeleteCubeModal::new(
            cube.clone(),
            network_dir,
            root.clone(),
            Some(wallet),
            Some(false),
            false,
        );

        let (release, released) = std::sync::mpsc::channel::<()>();
        // Registered as production does: the daemon config's canonical path,
        // while the modal looks the directory up from the Cube's settings.
        let registered = std::fs::canonicalize(&wallet_dir).unwrap();
        crate::daemon::stopping::run_in_background(Some(registered), move || {
            released.recv().unwrap();
        });
        let started = std::time::Instant::now();
        let _ = modal.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::Confirm(cube.id.clone()),
        )));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the confirm must not wait for the daemon on the UI thread"
        );
        assert!(modal.waiting_for_stop);
        assert!(!modal.deleted);
        assert!(wallet_dir.join("coincubed.sqlite3").is_file());
        let _ = modal.view();

        release.send(()).unwrap();
        assert!(crate::daemon::stopping::wait_for(&wallet_dir));
        assert!(!crate::daemon::stopping::any_stopping(
            std::slice::from_ref(&wallet_dir)
        ));
        let _ = modal.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::DaemonStopped(cube.id.clone()),
        )));
        assert!(!modal.waiting_for_stop);
        assert!(
            !wallet_dir.exists(),
            "the delete ran once the daemon stopped"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A daemon that outlasts the wait: the delete is refused, the files
    /// stay, the modal says why, and Delete can be pressed again.
    #[test]
    fn a_delete_refuses_when_the_daemon_outlasts_the_wait() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let root = std::env::temp_dir().join(format!("delete-outlast-{}", uuid::Uuid::new_v4()));
        let datadir = CoincubeDirectory::new(root.clone());
        let network_dir = datadir.network_directory(Network::Bitcoin);
        let wallet = WalletSettings {
            name: "Coincube-kt6ht0kt".to_string(),
            alias: None,
            descriptor_checksum: "kt6ht0kt".to_string(),
            pinned_at: Some(1_720_000_000),
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: None,
            start_internal_bitcoind: Some(false),
            pending_rescan: None,
            keychain_keys_recorded: false,
        };
        let wallet_dir = network_dir
            .coincubed_data_directory(&wallet.wallet_id())
            .path()
            .to_path_buf();
        std::fs::create_dir_all(&wallet_dir).unwrap();
        std::fs::write(wallet_dir.join("coincubed.sqlite3"), b"synthetic").unwrap();
        let cube = cube("local-a", "Local A", Network::Bitcoin);
        let mut modal = DeleteCubeModal::new(
            cube.clone(),
            network_dir,
            root.clone(),
            Some(wallet),
            Some(false),
            false,
        );
        let (release, released) = std::sync::mpsc::channel::<()>();
        // Registered as production does: the daemon config's canonical path,
        // while the modal looks the directory up from the Cube's settings.
        let registered = std::fs::canonicalize(&wallet_dir).unwrap();
        crate::daemon::stopping::run_in_background(Some(registered), move || {
            released.recv().unwrap();
        });
        let _ = modal.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::Confirm(cube.id.clone()),
        )));
        assert!(modal.waiting_for_stop);

        // The task's wait ran out with the stop still registered.
        let started = std::time::Instant::now();
        let _ = modal.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::DaemonStopped(cube.id.clone()),
        )));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(!modal.waiting_for_stop);
        assert!(modal.stop_refused);
        assert!(!modal.deleted);
        assert!(modal.warning.is_none(), "Delete stays available to retry");
        assert!(wallet_dir.join("coincubed.sqlite3").is_file());
        let _ = modal.view();

        // Once the stop has finished, pressing Delete again deletes.
        release.send(()).unwrap();
        assert!(crate::daemon::stopping::wait_for(&wallet_dir));
        let _ = modal.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::Confirm(cube.id.clone()),
        )));
        assert!(!modal.stop_refused);
        assert!(!wallet_dir.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A Vault on the retired Liana Connect backend is delete-only. Delete
    /// never contacts that backend, so nothing about it can block removing the
    /// Cube, and the modal says where the wallet still lives. No runtime is
    /// entered: an old check blocked on one.
    #[test]
    fn a_legacy_remote_backend_does_not_block_deleting_the_cube() {
        let started = std::time::Instant::now();
        let root =
            std::env::temp_dir().join(format!("delete-legacy-backend-{}", uuid::Uuid::new_v4()));
        let datadir = CoincubeDirectory::new(root.clone());
        let wallet = WalletSettings {
            name: "Coincube-kt6ht0kt".to_string(),
            alias: None,
            descriptor_checksum: "kt6ht0kt".to_string(),
            pinned_at: Some(1_720_000_000),
            keys: Vec::new(),
            hardware_wallets: Vec::new(),
            remote_backend_auth: Some(crate::app::settings::AuthConfig::new(
                "alex@example.test".into(),
                "legacy-wallet".into(),
            )),
            start_internal_bitcoind: Some(false),
            pending_rescan: None,
            keychain_keys_recorded: false,
        };
        let cube = cube("local-a", "Local A", Network::Bitcoin);
        let modal = DeleteCubeModal::new(
            cube,
            datadir.network_directory(Network::Bitcoin),
            root.clone(),
            Some(wallet),
            Some(false),
            false,
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "opening Delete must not wait on the legacy backend"
        );
        assert!(
            modal.warning.is_none(),
            "an unreachable legacy backend must not disable Delete"
        );
        assert!(
            modal
                .wallet_settings
                .as_ref()
                .and_then(|wallet| wallet.remote_backend_auth.as_ref())
                .is_some(),
            "the legacy backend is still recorded; Delete no longer asks it"
        );
        let _ = modal.view();
        // The confirm button is enabled exactly when the PIN is ready, there
        // is no warning, and no daemon stop is in progress.
        assert!(!modal.cube.has_pin(&root));
        assert!(!modal.waiting_for_stop);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Bitcoin Blake2b Cubes are dormant to the generic unlock, so the
    /// delete confirmation refused them before reading the PIN with "update
    /// Tenshu…" and they could not be deleted at all. Their PIN is checked
    /// with the fork's own trial: a wrong PIN still refuses, the right one
    /// deletes.
    #[test]
    fn a_blake2b_cube_deletes_with_its_pin() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let _enter = runtime.enter();
        let enter_pin = |modal: &mut DeleteCubeModal, pin: &str| {
            for (index, digit) in pin.chars().enumerate() {
                let _ = modal.update(Message::View(ViewMessage::DeleteCube(
                    DeleteCubeMessage::PinInput(pin_input::Message::DigitChanged(
                        index,
                        digit.to_string(),
                    )),
                )));
            }
        };
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let root =
                std::env::temp_dir().join(format!("delete-blake2b-{}", uuid::Uuid::new_v4()));
            let network_dir = CoincubeDirectory::new(root.clone()).network_directory(chain);
            let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
            let signer =
                coincube_core::signer::MasterSigner::generate(chain.bitcoin_network()).unwrap();
            let mut cube =
                CubeSettings::new_with_raw_id("blake2b-delete".into(), "Fork".into(), chain);
            cube.master_signer_fingerprint = Some(signer.fingerprint(&secp));
            signer
                .store_encrypted_for_chain(&root, chain, &secp, None, "1234", &cube.id, None)
                .unwrap();
            let listed = cube.clone();
            Handle::current()
                .block_on(settings::update_settings_file(&network_dir, |mut s| {
                    s.cubes.push(listed);
                    Some(s)
                }))
                .unwrap();
            let on_disk = || {
                settings::Settings::from_file(&network_dir)
                    .map(|s| s.cubes.iter().any(|c| c.id == "blake2b-delete"))
                    .unwrap_or(false)
            };
            let mut modal = DeleteCubeModal::new(
                cube.clone(),
                network_dir.clone(),
                root.clone(),
                None,
                None,
                false,
            );
            assert!(modal.cube.has_pin(&root), "{:?}", chain);

            enter_pin(&mut modal, "9999");
            let _ = modal.update(Message::View(ViewMessage::DeleteCube(
                DeleteCubeMessage::Confirm(cube.id.clone()),
            )));
            assert_eq!(
                modal.pin_error.as_deref(),
                Some("Incorrect PIN. Please try again."),
                "{:?}",
                chain
            );
            assert!(
                !modal.deleted && on_disk(),
                "{:?}: a wrong PIN deleted",
                chain
            );

            enter_pin(&mut modal, "1234");
            let _ = modal.update(Message::View(ViewMessage::DeleteCube(
                DeleteCubeMessage::Confirm(cube.id.clone()),
            )));
            assert!(
                modal
                    .pin_error
                    .as_deref()
                    .is_none_or(|error| !error.contains(crate::chain::BTCB2_DORMANT_REASON)),
                "{:?}: {:?}",
                chain,
                modal.pin_error
            );
            assert!(modal.deleted, "{:?}: {:?}", chain, modal.pin_error);
            assert!(!on_disk(), "{:?}: the Cube is still listed", chain);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn modal_views_and_non_destructive_modal_updates_build() {
        let cube = cube("local-a", "Local A", Network::Bitcoin);
        let network_dir = test_datadir().network_directory(Network::Bitcoin);
        let mut delete_modal = DeleteCubeModal::new(
            cube.clone(),
            network_dir,
            test_datadir().path().to_path_buf(),
            None,
            Some(true),
            true,
        );

        let _ = delete_modal.view();
        let _ = delete_modal.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::DeleteConnectBackup(true),
        )));
        assert!(delete_modal.delete_connect_backup);

        let remote = remote_cube("remote-a", "Remote A", Network::Bitcoin);
        let _ = DeleteRemoteCubeModal {
            cube: remote.clone(),
            deleting: false,
            error: Some("server refused".to_string()),
            typed_name: String::new(),
        }
        .view();
        let _ = DeleteRemoteCubeModal {
            cube: remote.clone(),
            deleting: true,
            error: None,
            typed_name: String::new(),
        }
        .view();
        let _ = RecoveryMethodModal { cube: remote }.view();

        let mut home = signed_in_home();
        home.state = State::Cubes {
            cubes: vec![cube],
            create_cube: false,
            source: crate::chain::ChainId::Bitcoin,
        };
        home.rename_cube_modal = Some((
            CubeCatalogKey {
                chain: ChainId::Bitcoin,
                cube_id: "local-a".into(),
            },
            "Renamed Cube".to_string(),
        ));
        home.view();
        home.rename_cube_modal = None;
        home.delete_remote_cube_modal = Some(DeleteRemoteCubeModal {
            cube: remote_cube("remote-b", "Remote B", Network::Bitcoin),
            deleting: false,
            error: None,
            typed_name: String::new(),
        });
        home.view();
        home.delete_remote_cube_modal = None;
        home.recovery_method_modal = Some(RecoveryMethodModal {
            cube: remote_cube("remote-c", "Remote C", Network::Bitcoin),
        });
        home.view();
    }

    #[test]
    fn delete_cube_modals_focus_and_activate_their_actions() {
        use crate::app::view::shared::landing::capture;
        use iced::keyboard::key::Named;

        let datadir = test_datadir();
        let local = || {
            DeleteCubeModal::new(
                cube("local-a", "Local A", Network::Bitcoin),
                datadir.network_directory(Network::Bitcoin),
                datadir.path().to_path_buf(),
                None,
                Some(true),
                true,
            )
        };
        let remote = |deleting| DeleteRemoteCubeModal {
            cube: remote_cube("remote-a", "Remote A", Network::Bitcoin),
            deleting,
            error: None,
            typed_name: "Remote A".to_string(),
        };
        let size = (1024, 768);
        let local_cancel = Id::new(DELETE_LOCAL_CANCEL_ID);
        let local_confirm = Id::new(DELETE_LOCAL_CONFIRM_ID);
        let remote_cancel = Id::new(DELETE_REMOTE_CANCEL_ID);
        let remote_confirm = Id::new(DELETE_REMOTE_CONFIRM_ID);

        let local_view = capture::inspect_full_window(
            "delete-cube-keyboard",
            local().view(),
            size,
            "dark",
            theme::Theme::dark(),
        );
        assert!(local_view.has_focus(local_cancel.clone()));
        assert!(local_view.has_focus(local_confirm.clone()));
        assert!(
            local_view
                .focus
                .iter()
                .position(|(id, _)| id == &local_cancel)
                < local_view
                    .focus
                    .iter()
                    .position(|(id, _)| id == &local_confirm)
        );

        let remote_view = capture::inspect_full_window(
            "delete-remote-cube-keyboard",
            remote(false).view(),
            size,
            "light",
            theme::Theme::light(),
        );
        assert!(remote_view.has_focus(remote_cancel.clone()));
        assert!(remote_view.has_focus(remote_confirm.clone()));
        assert!(
            remote_view
                .focus
                .iter()
                .position(|(id, _)| id == &remote_cancel)
                < remote_view
                    .focus
                    .iter()
                    .position(|(id, _)| id == &remote_confirm)
        );

        let local_light = capture::inspect_full_window(
            "delete-cube-keyboard",
            local().view(),
            size,
            "light",
            theme::Theme::light(),
        );
        assert!(local_light.has_focus(local_cancel.clone()));
        assert!(local_light.has_focus(local_confirm.clone()));
        let remote_dark = capture::inspect_full_window(
            "delete-remote-cube-keyboard",
            remote(false).view(),
            size,
            "dark",
            theme::Theme::dark(),
        );
        assert!(remote_dark.has_focus(remote_cancel.clone()));
        assert!(remote_dark.has_focus(remote_confirm.clone()));

        for key in [Named::Enter, Named::Space] {
            assert!(matches!(
                capture::activate_with_key(local().view(), local_cancel.clone(), size, false, key)
                    .as_slice(),
                [Message::View(ViewMessage::DeleteCube(
                    DeleteCubeMessage::CloseModal
                ))]
            ));
            assert!(matches!(
                capture::activate_with_key(local().view(), local_confirm.clone(), size, false, key)
                    .as_slice(),
                [Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::Confirm(id)))]
                    if id == "local-a"
            ));
            assert!(matches!(
                capture::activate_with_key(
                    remote(false).view(),
                    remote_cancel.clone(),
                    size,
                    false,
                    key
                )
                .as_slice(),
                [Message::View(ViewMessage::DeleteCube(
                    DeleteCubeMessage::CloseRemoteModal
                ))]
            ));
            assert!(matches!(
                capture::activate_with_key(remote(false).view(), remote_confirm.clone(), size, false, key)
                    .as_slice(),
                [Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ConfirmRemoteDelete(id)))]
                    if id == "remote-a"
            ));
        }

        let mut blocked_local = local();
        blocked_local.warning = Some(DeleteError::Io(std::io::Error::other("blocked")));
        let blocked_view = capture::inspect_full_window(
            "delete-cube-blocked-keyboard",
            blocked_local.view(),
            size,
            "dark",
            theme::Theme::dark(),
        );
        assert!(blocked_view.has_focus(local_cancel));
        assert!(!blocked_view.has_focus(local_confirm));

        let deleting_view = capture::inspect_full_window(
            "delete-remote-cube-deleting-keyboard",
            remote(true).view(),
            size,
            "light",
            theme::Theme::light(),
        );
        assert!(!deleting_view.has_focus(remote_cancel));
        assert!(!deleting_view.has_focus(remote_confirm));
    }

    #[test]
    fn delete_modal_danger_ink_matches_canvas_and_meets_body_contrast() {
        use iced::widget::button::Status;

        for (name, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let bad = theme.colors.shell.bad;
            let panel = theme.colors.shell.panel;
            let title = delete_modal_danger_text(&theme);
            let active = delete_modal_danger_button(&theme, Status::Active);
            let hovered = delete_modal_danger_button(&theme, Status::Hovered);
            let disabled = delete_modal_danger_button(&theme, Status::Disabled);

            assert_eq!(title.color, Some(bad), "{name} title");
            assert_eq!(active.text_color, bad, "{name} action ink");
            assert_eq!(active.border.color, bad, "{name} action border");
            assert!(
                theme::tenshu::contrast(active.text_color, panel) >= 4.5,
                "{} action label on panel",
                name
            );
            assert!(
                theme::tenshu::contrast(hovered.text_color, bad) >= 4.5,
                "{} hovered action label on danger fill",
                name
            );
            assert_eq!(disabled.text_color.a, 0.5, "{name} disabled label");
        }
    }

    #[test]
    fn an_abandoned_import_opens_the_delete_confirmation_for_its_cube() {
        let other = cube("other", "Other", Network::Bitcoin);
        let unfinished = cube("unfinished", "Unfinished", Network::Bitcoin);
        let ask = || {
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ShowModalFor {
                cube_id: "unfinished".into(),
                chain: ChainId::Bitcoin,
            }))
        };
        let checked = |chain: ChainId, cubes: Vec<CubeSettings>| Message::Checked {
            for_chain: chain,
            res: Ok(State::Cubes {
                cubes,
                create_cube: false,
                source: chain,
            }),
        };
        let asked = |home: &Home| {
            home.delete_cube_modal
                .as_ref()
                .map(|m| (m.cube.id.clone(), m.cube.name.clone()))
        };
        let fresh = Some(("unfinished".to_string(), "Unfinished".to_string()));

        // A listed row may predate the import's later stages: the
        // confirmation waits for a list checked after the request.
        let mut home = home();
        home.network = ChainId::Bitcoin;
        let mut stale = unfinished.clone();
        stale.name = "Stale".into();
        home.state = State::Cubes {
            cubes: vec![other.clone(), stale],
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        let _ = home.update(ask());
        assert!(home.delete_cube_modal.is_none());
        let _ = home.update(checked(
            ChainId::Bitcoin,
            vec![other.clone(), unfinished.clone()],
        ));
        assert_eq!(asked(&home), fresh);
        assert!(home.delete_after_check.is_none());

        // A Launcher left on another network, Bitcoin Blake2b included,
        // switches to the Cube's network; that network's list opens it.
        let mut home = self::home();
        home.network = ChainId::BitcoinBlake2b;
        let _ = home.update(ask());
        assert_eq!(home.network, ChainId::Bitcoin);
        assert!(home.error.is_none());
        let _ = home.update(checked(ChainId::BitcoinBlake2b, vec![]));
        assert!(home.delete_cube_modal.is_none());
        let _ = home.update(checked(
            ChainId::Bitcoin,
            vec![other.clone(), unfinished.clone()],
        ));
        assert_eq!(asked(&home), fresh);

        // One checked list answers the request. A Cube missing from it is
        // not asked about by a later reload that lists it.
        let mut home = self::home();
        home.network = ChainId::Bitcoin;
        let _ = home.update(ask());
        let _ = home.update(checked(ChainId::Bitcoin, vec![other.clone()]));
        assert!(home.delete_after_check.is_none());
        let _ = home.update(checked(ChainId::Bitcoin, vec![other, unfinished]));
        assert!(home.delete_cube_modal.is_none());
    }

    #[test]
    fn closing_delete_modals_returns_focus_only_to_a_visible_opener() {
        use iced_runtime::futures::futures::StreamExt;

        fn has_focus_operation(task: Task<Message>) -> bool {
            let Some(stream) = iced_runtime::task::into_stream(task) else {
                return false;
            };
            iced_runtime::futures::futures::executor::block_on(stream.collect::<Vec<_>>())
                .iter()
                .any(|action| matches!(action, iced_runtime::Action::Widget(_)))
        }

        let local_cube = cube("local-a", "Local A", Network::Bitcoin);
        let mut home = home();
        home.state = State::Cubes {
            cubes: vec![local_cube.clone()],
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        home.catalog.mode = launcher_catalog::LauncherMode::List;
        let local_key = CubeCatalogKey {
            chain: ChainId::Bitcoin,
            cube_id: local_cube.id.clone(),
        };
        home.catalog.selected = Some(local_key.clone());
        home.delete_cube_modal = Some(DeleteCubeModal::new(
            local_cube.clone(),
            home.datadir_path.network_directory(Network::Bitcoin),
            home.datadir_path.path().to_path_buf(),
            None,
            Some(true),
            false,
        ));
        assert!(has_focus_operation(home.update(Message::View(
            ViewMessage::DeleteCube(DeleteCubeMessage::CloseModal)
        ))));
        assert!(home.delete_cube_modal.is_none());

        home.catalog.selected = None;
        assert!(!has_focus_operation(focus_local_delete_opener(
            &home, &local_key
        )));
        home.catalog.mode = launcher_catalog::LauncherMode::Cubes;
        home.catalog.selected = Some(local_key.clone());
        assert!(has_focus_operation(focus_local_delete_opener(
            &home, &local_key
        )));
        home.catalog.selected = None;
        assert!(!has_focus_operation(focus_local_delete_opener(
            &home, &local_key
        )));
        home.catalog.mode = launcher_catalog::LauncherMode::List;
        home.catalog.query = "not this Cube".into();
        assert!(!has_focus_operation(focus_local_delete_opener(
            &home, &local_key
        )));
        home.catalog.query.clear();
        home.state = State::Cubes {
            cubes: vec![],
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        assert!(!has_focus_operation(focus_local_delete_opener(
            &home, &local_key
        )));
        home.state = State::Cubes {
            cubes: vec![local_cube],
            create_cube: false,
            source: ChainId::Bitcoin,
        };

        let remote_cube = remote_cube("remote-a", "Remote A", Network::Bitcoin);
        home.remote_cubes = vec![remote_cube.clone()];
        home.delete_remote_cube_modal = Some(DeleteRemoteCubeModal {
            cube: remote_cube.clone(),
            deleting: false,
            error: None,
            typed_name: String::new(),
        });
        assert!(has_focus_operation(home.update(Message::View(
            ViewMessage::DeleteCube(DeleteCubeMessage::CloseRemoteModal)
        ))));
        assert!(home.delete_remote_cube_modal.is_none());

        home.catalog.mode = launcher_catalog::LauncherMode::Cubes;
        assert!(!has_focus_operation(focus_remote_delete_opener(
            &home,
            &remote_cube.uuid
        )));
        home.catalog.query.clear();
        home.remote_cubes.clear();
        assert!(!has_focus_operation(focus_remote_delete_opener(
            &home,
            &remote_cube.uuid
        )));
        home.catalog.mode = launcher_catalog::LauncherMode::List;
        home.catalog.query = "not this Cube".into();
        assert!(!has_focus_operation(focus_remote_delete_opener(
            &home,
            &remote_cube.uuid
        )));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn delete_and_recovery_modals_render_safe_names_without_changing_action_identity() {
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{Id, Operation, Tree},
            Layout,
        };

        #[derive(Default)]
        struct Labels(Vec<String>);

        impl Operation for Labels {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }

            fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, text: &str) {
                self.0.push(text.to_owned());
            }
        }

        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer for modal text");
        let labels = |mut element: Element<Message>| {
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(iced::Size::ZERO, iced::Size::new(900.0, 1200.0)),
            );
            let mut labels = Labels::default();
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
            labels.0
        };

        let raw_bidi_name = "  Family\n\u{202e}\u{200b} Cube  ";
        let remote = remote_cube("remote-uuid", raw_bidi_name, Network::Bitcoin);
        let delete_remote_labels = labels(
            DeleteRemoteCubeModal {
                cube: remote.clone(),
                deleting: false,
                error: None,
                typed_name: String::new(),
            }
            .view(),
        );
        assert!(delete_remote_labels
            .contains(&"Delete \u{201c}Family Cube\u{201d} from Connect?".into()));
        assert!(delete_remote_labels.contains(&"Delete from Connect".into()));
        assert!(delete_remote_labels.contains(&"Type Family Cube to confirm".into()));
        let recovery_labels = labels(
            RecoveryMethodModal {
                cube: remote.clone(),
            }
            .view(),
        );
        assert!(recovery_labels.contains(&"Recover \"Family Cube\"".into()));
        assert!(recovery_labels.contains(&"Recover with password".into()));
        assert!(recovery_labels.contains(&"Recover with phone".into()));
        assert!(delete_remote_labels
            .iter()
            .chain(recovery_labels.iter())
            .all(|label| !label.contains('\u{202e}') && !label.contains('\u{200b}')));
        assert_eq!(remote.name, raw_bidi_name);
        assert_eq!(remote.uuid, "remote-uuid");

        let raw_long_name = "界".repeat(300);
        let local = cube("local-id", &raw_long_name, Network::Bitcoin);
        let datadir = test_datadir();
        let delete_local_labels = labels(
            DeleteCubeModal::new(
                local.clone(),
                datadir.network_directory(Network::Bitcoin),
                datadir.path().to_path_buf(),
                None,
                Some(true),
                false,
            )
            .view(),
        );
        let bounded_name = format!("{}…", "界".repeat(79));
        assert!(delete_local_labels.contains(&format!("Delete Cube \"{bounded_name}\"")));
        assert!(delete_local_labels.contains(&format!(
            "Are you sure you want to delete the Cube \"{bounded_name}\"?"
        )));
        assert!(delete_local_labels.contains(&"Delete Cube".into()));
        assert!(!delete_local_labels
            .iter()
            .any(|label| label.contains(&raw_long_name)));
        assert_eq!(local.id, "local-id");
        assert_eq!(local.name, raw_long_name);
    }

    // ---------------------------------------------------------------------
    // Creation-time backup step
    // ---------------------------------------------------------------------

    /// A `Home` pointed at a real, empty datadir.
    fn home_with_datadir(dir: &std::path::Path) -> Home {
        let mut home = Home::new(
            CoincubeDirectory::new(dir.to_path_buf()),
            Some(Network::Bitcoin),
        )
        .0;
        home.state = State::Cubes {
            cubes: Vec::new(),
            create_cube: true,
            source: crate::chain::ChainId::Bitcoin,
        };
        home
    }

    fn tmp_datadir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "coincube-creation-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Run a `Task` to completion and collect the messages it emitted.
    ///
    /// This is what makes the tests below end-to-end rather than helper-only:
    /// the async work `Home::update` hands back is actually executed and its
    /// output fed straight back into `update`, exactly as the iced runtime
    /// does in the running application.
    fn drain(task: Task<Message>) -> Vec<Message> {
        use iced_runtime::futures::futures::StreamExt;

        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Vec::new();
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_runtime::Action::Output(msg) => Some(msg),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>()
                .await
        })
    }

    /// Type a 4-digit PIN into a `PinInput` the way the keypad does.
    fn type_pin(input: &mut pin_input::PinInput, pin: &str) {
        for (i, digit) in pin.chars().enumerate() {
            let _ = input.update(pin_input::Message::DigitChanged(i, digit.to_string()));
        }
    }

    /// Fill in the create form and click "Create Cube".
    ///
    /// On a machine whose keystore is unusable this refuses at phase 1 by
    /// design (3b: probe early, mint late), so it does not always land in the
    /// backup step. Tests that need the wizard regardless of the machine use
    /// [`enter_backup_step`].
    fn start_creation(home: &mut Home, name: &str, pin: &str) -> Task<Message> {
        home.create_cube_name = coincube_ui::component::form::Value {
            value: name.to_string(),
            ..Default::default()
        };
        // The PIN path explicitly: `passkey_mode` defaults on wherever passkeys
        // are available, and a passkey creation would launch a real ceremony.
        home.passkey_mode = false;
        type_pin(&mut home.create_cube_pin, pin);
        type_pin(&mut home.create_cube_pin_confirm, pin);
        home.update(Message::View(ViewMessage::CreateCube))
    }

    /// Put `home` into the backup step without going through the keystore
    /// probe, reproducing exactly the state phase 1 leaves behind.
    ///
    /// This is a fixture for testing the wizard's **state machine**, not a
    /// shortcut around production behaviour. Entry into the step is a real
    /// property of the machine and is covered separately at both ends:
    /// `an_unusable_keystore_refuses_before_any_seed_word_is_shown` for the
    /// refusal, `creating_a_cube_walks_the_backup_step_and_the_cube_opens` for
    /// the full production walk. Without this, every wizard test would silently
    /// stop running on an unsigned macOS build — the machines this is developed
    /// on — and the state machine would go untested exactly where it is
    /// hardest to notice.
    fn enter_backup_step(home: &mut Home, name: &str, pin: &str) {
        home.create_cube_name = coincube_ui::component::form::Value {
            value: name.to_string(),
            ..Default::default()
        };
        home.passkey_mode = false;
        type_pin(&mut home.create_cube_pin, pin);
        type_pin(&mut home.create_cube_pin_confirm, pin);
        let signer =
            MasterSigner::generate(home.network.bitcoin_network()).expect("seed generation");
        home.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);
        home.creating_cube = false;
        home.error = None;
        // The real transition, so the Cube list is parked exactly as it is in
        // production rather than by a copy of the logic that can drift.
        home.enter_creation_backup(signer.words().iter().map(|w| w.to_string()).collect());
    }

    /// The backup step the flow is currently on. Panics if it left the step —
    /// which is the assertion every caller actually wants.
    fn step(home: &Home) -> CreationBackupStep {
        match &home.state {
            State::CreationBackup(step) => step.clone(),
            other => panic!("expected the creation backup step, got {:?}", other),
        }
    }

    fn backup(msg: BackupWalletMessage) -> Message {
        Message::View(ViewMessage::CreationBackup(msg))
    }

    /// The words the step is currently showing, copied out for the test to
    /// answer the verification challenge with.
    fn shown_words(home: &Home) -> Vec<String> {
        home.creation_backup_words
            .as_ref()
            .expect("the backup step must be holding the new Cube's seed phrase")
            .to_vec()
    }

    /// Walk intro → phrase → verification, answer the challenge correctly,
    /// then Continue from the backup hub. Returns the task Continue produced.
    fn walk_backup_to_verified(home: &mut Home) -> Task<Message> {
        // Every creation now opens on the choice screen; "write down my
        // recovery phrase" is what leads into the wizard.
        assert_eq!(
            step(home),
            CreationBackupStep::Choice,
            "creation must open on the backup choice"
        );
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let _ = home.update(backup(BackupWalletMessage::ToggleBackupIntroCheck));
        assert_eq!(
            step(home),
            CreationBackupStep::Intro(true),
            "ticking the intro checkbox must be what unlocks the phrase screen"
        );
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        assert_eq!(step(home), CreationBackupStep::Phrase);

        let words = shown_words(home);
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let State::CreationBackup(CreationBackupStep::Verification { word_indices, .. }) =
            home.state.clone()
        else {
            panic!("expected the verification screen, got {:?}", home.state);
        };

        for &index in &word_indices {
            let _ = home.update(backup(BackupWalletMessage::WordInput {
                index: index as u8,
                input: words[index - 1].clone(),
            }));
        }
        let verified = drain(home.update(backup(BackupWalletMessage::VerifyPhrase)));
        assert!(
            verified.is_empty(),
            "a confirmed phrase returns to the backup hub; it must not write the Cube itself"
        );
        assert_eq!(step(home), CreationBackupStep::Choice);
        assert!(home.creation_seed_confirmed);
        home.update(Message::View(ViewMessage::CreationBackupContinue))
    }

    /// Read back what actually landed in `settings.json`.
    fn cubes_on_disk(dir: &std::path::Path) -> Vec<CubeSettings> {
        let network_dir =
            CoincubeDirectory::new(dir.to_path_buf()).network_directory(Network::Bitcoin);
        match settings::Settings::from_file(&network_dir) {
            Ok(s) => s.cubes,
            Err(_) => Vec::new(),
        }
    }

    /// **The end-to-end creation test.**
    ///
    /// Drives the production message sequence — create form → intro → phrase →
    /// verification → the real async write — and then asserts the Cube that
    /// landed on disk is one the creation gate will open. No helper shortcuts:
    /// every step goes through `Home::update`, and the task it returns is
    /// actually run.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"
    )]
    fn creating_a_cube_walks_the_backup_step_and_the_cube_opens() {
        create_verify_and_reopen_cube(Network::Bitcoin, "Fresh");
    }

    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs the signed macOS test runner for the data-protection keychain"
    )]
    fn creating_a_testnet4_cube_walks_the_backup_step_and_reopens() {
        create_verify_and_reopen_cube(Network::Testnet4, "E2E-Testnet4");
    }

    fn create_verify_and_reopen_cube(network: Network, name: &str) {
        let dir = tmp_datadir("e2e");
        let mut home = home_with_datadir(&dir);
        let _ = drain(home.update(Message::View(ViewMessage::ToggleDeveloperMode(true))));
        let select = Message::View(ViewMessage::SelectNetwork(network.into()));
        for result in drain(home.update(select)) {
            let _ = drain(home.update(result));
        }
        assert_eq!(home.network, network.into());

        // This PIN is used only for the disposable test Cube.
        let pin = zeroize::Zeroizing::new(format!("{:04}", rand::random::<u16>() % 10_000));
        let task = start_creation(&mut home, name, &pin);
        assert!(
            drain(task).is_empty(),
            "clicking Create must not create anything on its own — the backup step comes first"
        );
        assert_eq!(
            step(&home),
            CreationBackupStep::Choice,
            "creation must land on the backup step, not on a finished Cube"
        );
        assert!(
            settings::Settings::from_file(&home.datadir_path.network_directory(network))
                .map_or(true, |settings| settings.cubes.is_empty()),
            "no Cube may exist on disk while the backup step is still running"
        );

        let words = shown_words(&home);
        assert_eq!(words.len(), 12, "a generated Cube gets a 12-word phrase");

        let finalize = walk_backup_to_verified(&mut home);
        let out = drain(finalize);
        let [Message::CubeCreated(result)] = out.as_slice() else {
            panic!("expected exactly one CubeCreated, got {:?}", out);
        };
        let created = result
            .as_ref()
            .unwrap_or_else(|e| panic!("creation failed after a completed backup: {}", e));

        // Feed the result back in, as the runtime would.
        let _ = home.update(Message::CubeCreated(Ok(created.clone())));
        assert!(
            home.creation_backup_words.is_none(),
            "the seed phrase must not outlive the creation flow"
        );

        // Reconstruct Home from disk and run its startup probe before unlocking.
        drop(home);
        let directory = CoincubeDirectory::new(dir.clone());
        let network_dir = directory.network_directory(network);
        let (mut reopened, startup) = Home::new(directory, Some(network));
        for result in drain(startup) {
            let _ = drain(reopened.update(result));
        }
        assert_eq!(reopened.network, network.into());
        assert!(matches!(
            &reopened.state,
            State::Cubes { cubes, .. } if cubes.iter().any(|cube| cube.name == name)
        ));
        let on_disk = settings::Settings::from_file(&network_dir).unwrap().cubes;
        if network == Network::Testnet4 {
            assert!(cubes_on_disk(&dir).is_empty());
        }
        assert_eq!(
            on_disk.len(),
            1,
            "expected exactly one Cube, got {:?}",
            on_disk
        );
        let cube = &on_disk[0];
        assert_eq!(cube.network, network.into());
        assert_eq!(cube.name, name);
        assert!(
            cube.backed_up,
            "a completed verification must be recorded as a backup"
        );
        assert!(
            cube.creation_backup_bypass.is_none(),
            "a Cube that was backed up must not also record a bypass"
        );
        assert_eq!(
            creation_gate::evaluate_for_cube(cube, None),
            creation_gate::CreationGate::Satisfied,
            "a Cube created through the backup step must open"
        );

        // …and it opens for real: the production unlock path, the PIN the user
        // typed into the create form, and the seed file that was actually
        // sealed. This is the assertion the bricking bug would have failed.
        let loc = unlock::CubeLocation::new(&dir, cube);
        let result = unlock::unlock_blocking(&loc, &pin);
        unlock::device_secret::delete(&cube.id).expect("remove the disposable device secret");
        std::fs::remove_dir_all(&dir).expect("remove the disposable Cube");
        let Ok(unlock::PinOutcome::Unlock(signer)) = result else {
            panic!("a freshly created Cube did not unlock");
        };
        assert!(
            signer.words() == words,
            "the verified phrase must match the phrase that was sealed"
        );
    }

    /// The bypass completes creation and leaves the evidence support needs.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"
    )]
    fn the_bypass_path_completes_and_records_the_bypass() {
        let dir = tmp_datadir("bypass");
        let mut home = home_with_datadir(&dir);
        let _ = drain(start_creation(&mut home, "Skipped", "4321"));

        let _ = home.update(Message::View(ViewMessage::CreationBackupBypassRequested));
        assert_eq!(
            step(&home),
            CreationBackupStep::Bypass {
                acknowledged: false
            }
        );

        // Confirming without acknowledging must do nothing at all.
        let out = drain(home.update(Message::View(ViewMessage::CreationBackupBypassConfirmed)));
        assert!(
            out.is_empty(),
            "an unacknowledged bypass must not create a Cube, got {:?}",
            out
        );
        assert!(cubes_on_disk(&dir).is_empty());

        let _ = home.update(Message::View(ViewMessage::CreationBackupAcknowledgeBypass(
            true,
        )));
        let out = drain(home.update(Message::View(ViewMessage::CreationBackupBypassConfirmed)));
        let [Message::CubeCreated(result)] = out.as_slice() else {
            panic!("expected exactly one CubeCreated, got {:?}", out);
        };
        result
            .as_ref()
            .unwrap_or_else(|e| panic!("bypassed creation failed: {}", e));

        let on_disk = cubes_on_disk(&dir);
        assert_eq!(
            on_disk.len(),
            1,
            "expected exactly one Cube, got {:?}",
            on_disk
        );
        let cube = &on_disk[0];
        assert!(
            !cube.backed_up,
            "a bypass is not a backup and must not be recorded as one"
        );
        let bypass = cube
            .creation_backup_bypass
            .as_ref()
            .expect("the bypass must be persisted so support can identify these Cubes");
        assert_eq!(
            bypass.acknowledged,
            creation_gate::BYPASS_ACKNOWLEDGEMENT,
            "the acknowledgement must be stored verbatim"
        );
        assert!(bypass.at > 0, "the bypass must be timestamped");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Walking away mid-backup must leave nothing behind — not a settings
    /// entry, not a seed file, not a keystore item.
    ///
    /// This is why nothing is written until the step resolves. Needs no
    /// keychain precisely because the abandoned path never reaches one.
    #[test]
    fn abandoning_the_backup_step_leaves_no_half_created_cube() {
        let dir = tmp_datadir("abandon");
        let mut home = home_with_datadir(&dir);
        enter_backup_step(&mut home, "Abandoned", "1111");

        // Get as deep as the flow allows before walking away: choice → intro →
        // phrase → verification.
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let _ = home.update(backup(BackupWalletMessage::ToggleBackupIntroCheck));
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        assert!(matches!(
            home.state,
            State::CreationBackup(CreationBackupStep::Verification { .. })
        ));

        let out = drain(home.update(Message::View(ViewMessage::CancelCreationBackup)));
        assert!(
            out.is_empty(),
            "abandoning must not create a Cube: {:?}",
            out
        );

        assert!(
            cubes_on_disk(&dir).is_empty(),
            "an abandoned creation must leave no Cube in settings.json"
        );
        assert!(
            home.creation_backup_words.is_none(),
            "an abandoned creation must scrub the seed phrase"
        );
        assert!(!home.creating_cube);
        assert!(
            matches!(
                home.state,
                State::Cubes {
                    create_cube: true,
                    ..
                }
            ),
            "abandoning returns to the create form, got {:?}",
            home.state
        );

        // Nothing at all was written under the datadir — no seed file, no
        // network directory, no marker.
        let stray: Vec<_> = walk_files(&dir);
        assert!(
            stray.is_empty(),
            "an abandoned creation left files behind: {:?}",
            stray
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cancelling a *second* Cube's creation must not look like the first one
    /// was deleted.
    ///
    /// The backup step owns the whole `State`, and rebuilding `State::Cubes`
    /// with an empty vector on the way out emptied the list on screen and made
    /// `total_cube_count` read zero — the count the per-network Cube limit is
    /// enforced against — until something else triggered a reload.
    #[test]
    fn cancelling_the_backup_step_puts_the_existing_cubes_back() {
        let dir = tmp_datadir("cancel-keeps-cubes");
        let mut home = home_with_datadir(&dir);
        let existing = cube("already-here", "First", home.network.bitcoin_network());
        home.state = State::Cubes {
            cubes: vec![existing.clone()],
            create_cube: true,
            source: crate::chain::ChainId::Bitcoin,
        };

        enter_backup_step(&mut home, "Second", "1111");
        assert!(matches!(home.state, State::CreationBackup(_)));

        let _ = home.update(Message::View(ViewMessage::CancelCreationBackup));

        let State::Cubes {
            cubes, create_cube, ..
        } = &home.state
        else {
            panic!(
                "cancelling must return to the Cube list, got {:?}",
                home.state
            );
        };
        assert!(create_cube, "cancelling returns to the create form");
        assert_eq!(
            cubes.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
            vec![existing.id.as_str()],
            "the Cube the user already had vanished from the list"
        );
        assert_eq!(
            home.total_cube_count(),
            1,
            "the Cube limit would be computed against a list that lost a Cube"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn walk_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk_files(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    /// The same walk as the strict test above, asserting the invariant that
    /// holds on **every** platform: what is on disk agrees with what
    /// `CubeCreated` reported, and a creation that failed leaves nothing.
    ///
    /// This is the locally-runnable half of the end-to-end coverage. It enters
    /// the wizard through [`enter_backup_step`] so the state machine and the
    /// real async write are exercised even where the keystore refuses; the
    /// write itself then fails there, and the `Err` arm asserts the refusal is
    /// clean — which is the property that actually protects the user. The
    /// strict version above starts from the create form and asserts the
    /// success branch unconditionally.
    #[test]
    fn what_lands_on_disk_always_agrees_with_the_creation_result() {
        let dir = tmp_datadir("agrees");
        let mut home = home_with_datadir(&dir);
        enter_backup_step(&mut home, "Consistent", "5555");

        let words = shown_words(&home);
        let out = drain(walk_backup_to_verified(&mut home));
        let [Message::CubeCreated(result)] = out.as_slice() else {
            panic!(
                "verification must produce exactly one CubeCreated, got {:?}",
                out
            );
        };

        let _ = home.update(Message::CubeCreated(result.clone()));
        assert!(
            home.creation_backup_words.is_none(),
            "the seed must be scrubbed whichever way creation went"
        );

        match result {
            Ok(cube) => {
                let on_disk = cubes_on_disk(&dir);
                assert_eq!(on_disk.len(), 1, "expected one Cube, got {:?}", on_disk);
                assert_eq!(on_disk[0].id, cube.id);
                assert!(
                    on_disk[0].backed_up,
                    "a completed verification must be recorded as a backup"
                );
                assert!(
                    on_disk[0].creation_backup_required,
                    "the gate is armed for Cubes created under it"
                );
                match unlock::unlock_blocking(&unlock::CubeLocation::new(&dir, &on_disk[0]), "5555")
                {
                    Ok(unlock::PinOutcome::Unlock(signer)) => {
                        assert_eq!(signer.words(), words)
                    }
                    other => panic!("a freshly created Cube did not unlock: {:?}", other),
                }
            }
            Err(e) => {
                // The refusal path. Nothing may be left behind — that is the
                // whole reason the writes happen after the backup step.
                assert!(
                    cubes_on_disk(&dir).is_empty(),
                    "a failed creation left a Cube in settings.json ({})",
                    e
                );
                assert!(
                    matches!(
                        home.state,
                        State::Cubes {
                            create_cube: true,
                            ..
                        }
                    ),
                    "a failed creation must return to the create form, got {:?}",
                    home.state
                );
                assert!(
                    home.error
                        .as_deref()
                        .is_some_and(|m| m.contains(e.as_str())),
                    "a failed creation must surface the reason; error was {:?}",
                    home.error
                );
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Restore is not creation: recovering a Cube from a phrase the user
    /// already holds must not send them through a backup step for it.
    #[test]
    fn restoring_from_a_recovery_phrase_is_not_gated_by_the_backup_step() {
        let dir = tmp_datadir("restore");
        let mut home = home_with_datadir(&dir);
        home.create_cube_name = coincube_ui::component::form::Value {
            value: "Restored".to_string(),
            ..Default::default()
        };
        type_pin(&mut home.create_cube_pin, "2468");
        type_pin(&mut home.create_cube_pin_confirm, "2468");

        // A real BIP39 phrase, entered word by word as the recovery grid does.
        let mnemonic = "abandon abandon abandon abandon abandon abandon \
                        abandon abandon abandon abandon abandon about";
        for (i, word) in mnemonic.split_whitespace().enumerate() {
            let _ = home.update(Message::View(ViewMessage::RecoveryWordInput {
                index: i,
                word: word.to_string(),
            }));
        }
        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));

        assert!(
            home.creating_cube,
            "the phrase must have been accepted — otherwise this test proves nothing"
        );
        assert!(
            !matches!(home.state, State::CreationBackup(_)),
            "restore must go straight to creation, not through the backup step; state was {:?}",
            home.state
        );
        assert!(
            home.creation_backup_words.is_none(),
            "restore must not stage a seed phrase for backup"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The verification challenge is real: wrong words do not pass, and a
    /// failed attempt neither creates a Cube nor loses the seed.
    #[test]
    fn wrong_verification_words_do_not_create_a_cube() {
        let dir = tmp_datadir("wrong-words");
        let mut home = home_with_datadir(&dir);
        enter_backup_step(&mut home, "Typo", "9876");

        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let _ = home.update(backup(BackupWalletMessage::ToggleBackupIntroCheck));
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let State::CreationBackup(CreationBackupStep::Verification { word_indices, .. }) =
            home.state.clone()
        else {
            panic!("expected the verification screen, got {:?}", home.state);
        };

        for &index in &word_indices {
            let _ = home.update(backup(BackupWalletMessage::WordInput {
                index: index as u8,
                input: "zoo".to_string(),
            }));
        }
        let out = drain(home.update(backup(BackupWalletMessage::VerifyPhrase)));
        assert!(
            out.is_empty(),
            "a failed verification must not create a Cube: {:?}",
            out
        );
        assert!(cubes_on_disk(&dir).is_empty());

        let State::CreationBackup(CreationBackupStep::Verification { error, .. }) = &home.state
        else {
            panic!(
                "expected to stay on the verification screen, got {:?}",
                home.state
            );
        };
        assert!(
            error.is_some(),
            "a failed verification must say so rather than silently doing nothing"
        );
        assert!(
            home.creation_backup_words.is_some(),
            "a failed attempt must keep the seed so the user can retry"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The intro checkbox actually gates the phrase.
    #[test]
    fn the_phrase_is_not_shown_until_the_intro_is_acknowledged() {
        let dir = tmp_datadir("intro-gate");
        let mut home = home_with_datadir(&dir);
        enter_backup_step(&mut home, "Careful", "1357");

        let _ = home.update(backup(BackupWalletMessage::NextStep));
        assert_eq!(
            step(&home),
            CreationBackupStep::Intro(false),
            "Next must do nothing until the user acknowledges the warning"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default stays `false` even though creation now arms the gate.
    ///
    /// `new_*` is reused to *reconstruct* Cubes in the installer restore paths
    /// (`gui/tab.rs`, `app/mod.rs`), which must not be gated — the Cube being
    /// restored predates this machine. Only the two creation sites in `home.rs`
    /// set the flag, and each sets evidence alongside it.
    #[test]
    fn the_constructor_default_stays_unarmed_for_restore_paths() {
        let cube = CubeSettings::new("Restored".to_string(), Network::Bitcoin);
        assert!(
            !cube.creation_backup_required,
            "restore paths reconstruct Cubes through `new_*` and must not be gated"
        );
        assert_eq!(
            creation_gate::evaluate_for_cube(&cube, None),
            creation_gate::CreationGate::Satisfied
        );
    }

    /// **Unit 6b: the slot is written at Cube creation, not at enrolment.**
    ///
    /// Creating it when duress is armed instead would make mtime the oracle
    /// the decoy exists to remove — a slot months newer than its Cube's seed
    /// file announces the day duress was turned on. So it has to be here, in
    /// the creation path, before any enrolment exists.
    ///
    /// Runs wherever the keystore works; where it does not, creation refuses
    /// at phase 1 and there is no Cube to inspect.
    #[test]
    fn the_second_slot_is_written_at_creation_not_at_enrolment() {
        let dir = tmp_datadir("slot-at-creation");
        let mut home = home_with_datadir(&dir);
        enter_backup_step(&mut home, "Slotted", "1234");

        let out = drain(walk_backup_to_verified(&mut home));
        let [Message::CubeCreated(Ok(cube))] = out.as_slice() else {
            // The keystore refused, so nothing was created — covered by
            // `an_unusable_keystore_refuses_before_any_seed_word_is_shown`.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        };

        let slot = cube
            .duress_slot_file
            .as_deref()
            .expect("a Cube must leave creation with its second slot recorded");
        assert!(
            unlock::marker::exists(&dir, Network::Bitcoin, Some(slot)),
            "the recorded slot {} is not on disk",
            slot
        );

        // Two blobs, and no PIN opens the second one — it is a decoy, and no
        // duress enrolment has happened.
        let folder = coincube_core::signer::MasterSigner::mnemonics_folder(&dir, Network::Bitcoin);
        let count = std::fs::read_dir(&folder).unwrap().count();
        assert_eq!(count, 2, "expected a seed file and a slot, found {}", count);
        for pin in ["1234", "0000", "8765"] {
            assert!(
                !unlock::marker::verify(&dir, Network::Bitcoin, &cube.id, Some(slot), pin, None),
                "PIN {} opened the slot of a Cube with no duress enrolled",
                pin
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The invariant that makes X4's `None` sound** (see `gui/tab.rs`).
    ///
    /// Creation must never persist a Cube that is armed but carries neither
    /// piece of local backup evidence, because that shape is what the open
    /// gate would block with "couldn't confirm this Cube's backup". Both
    /// exits from the backup step are checked against what actually lands in
    /// `settings.json`.
    ///
    /// The verified exit runs the real write, so it needs the keystore; the
    /// assertion is written to hold on either outcome, and the strict
    /// keystore-dependent version is
    /// `creating_a_cube_walks_the_backup_step_and_the_cube_opens`.
    #[test]
    fn creation_never_persists_an_armed_cube_without_evidence() {
        let dir = tmp_datadir("armed-evidence");
        let mut home = home_with_datadir(&dir);
        enter_backup_step(&mut home, "Evidence", "1234");

        let _ = drain(walk_backup_to_verified(&mut home));
        for cube in cubes_on_disk(&dir) {
            assert!(
                cube.creation_backup_required,
                "a Cube written by the creation flow must be armed"
            );
            assert!(
                cube.backed_up || cube.creation_backup_bypass.is_some(),
                "an armed Cube reached disk with no backup evidence — it would be \
                 blocked at open; cube was {:?}",
                cube.id
            );
            assert!(
                !matches!(
                    creation_gate::evaluate_for_cube(&cube, None),
                    creation_gate::CreationGate::Blocked(_)
                ),
                "a Cube straight out of creation must not be blocked at open"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Probe early, mint late.**
    ///
    /// On a machine whose keystore is unusable, creation must refuse *before*
    /// a single seed word is shown. 3a moved the `capability()` probe into
    /// `finalize_cube_creation`, which meant the user wrote down twelve words
    /// and verified three of them before being told the Cube could not be
    /// created — left holding a written phrase for a wallet that will never
    /// exist.
    ///
    /// The assertion is conditioned on the platform's actual answer rather
    /// than mocked, because the refusal is a real property of the machine:
    /// where the keystore works there is nothing to refuse, and where it does
    /// not — an unsigned macOS build, headless Linux — the seed must never
    /// appear. On this repo's macOS dev machines it is the second branch that
    /// runs, which is the branch that regressed.
    #[test]
    fn an_unusable_keystore_refuses_before_any_seed_word_is_shown() {
        let dir = tmp_datadir("probe-order");
        let mut home = home_with_datadir(&dir);
        let out = drain(start_creation(&mut home, "No Keystore", "1234"));
        assert!(
            out.is_empty(),
            "phase 1 must not create anything: {:?}",
            out
        );

        match unlock::device_secret::capability() {
            unlock::device_secret::Capability::Unavailable(why) => {
                assert!(
                    home.creation_backup_words.is_none(),
                    "a seed phrase was generated for a Cube that cannot be created"
                );
                assert!(
                    !matches!(home.state, State::CreationBackup(_)),
                    "the backup wizard opened despite an unusable keystore; state was {:?}",
                    home.state
                );
                assert_eq!(
                    home.error.as_deref(),
                    Some(why.as_str()),
                    "the refusal must carry the keystore's own explanation"
                );
                assert!(!home.creating_cube, "the creating-cube spinner must clear");
                assert!(
                    cubes_on_disk(&dir).is_empty(),
                    "a refused creation must write nothing"
                );
            }
            unlock::device_secret::Capability::Available => {
                // The keystore works, so there is nothing to refuse and the
                // wizard is expected to be open with a phrase staged.
                assert!(
                    matches!(home.state, State::CreationBackup(_)),
                    "a usable keystore must let creation proceed; state was {:?}",
                    home.state
                );
                assert!(home.creation_backup_words.is_some());
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every screen of the step renders, including the bypass acknowledgement.
    #[test]
    fn creation_backup_screens_render() {
        let words: Vec<String> = MasterSigner::generate(Network::Bitcoin)
            .unwrap()
            .words()
            .iter()
            .map(|w| w.to_string())
            .collect();

        let password = zeroize::Zeroizing::new("correct horse battery staple".to_string());
        let empty = zeroize::Zeroizing::new(String::new());

        for step in [
            CreationBackupStep::Intro(false),
            CreationBackupStep::Intro(true),
            CreationBackupStep::Phrase,
            CreationBackupStep::Verification {
                word_indices: [1, 5, 9],
                word_inputs: Default::default(),
                error: Some("nope".to_string()),
            },
            CreationBackupStep::Bypass {
                acknowledged: false,
            },
            CreationBackupStep::Bypass { acknowledged: true },
            CreationBackupStep::Choice,
            CreationBackupStep::KitPassword,
            CreationBackupStep::KitUploading,
        ] {
            for is_passkey in [false, true] {
                for (pw, confirm, ack, err) in [
                    (&empty, &empty, false, None),
                    (&password, &password, true, None),
                    (&password, &empty, false, Some("upload failed")),
                ] {
                    for kit_available in [false, true] {
                        for saving in [false, true] {
                            for (seed_confirmed, kit_saved) in
                                [(false, false), (true, false), (false, true), (true, true)]
                            {
                                let hub = CreationBackupHub {
                                    seed_confirmed,
                                    kit_saved,
                                };
                                let _ = creation_backup_view(
                                    &step,
                                    Some(&words),
                                    saving,
                                    is_passkey,
                                    pw,
                                    confirm,
                                    ack,
                                    err,
                                    kit_available,
                                    hub,
                                );
                                let _ = creation_choice_footer(saving, hub);
                            }
                        }
                        // The seed being gone mid-render must not panic.
                        let _ = creation_backup_view(
                            &step,
                            None,
                            false,
                            is_passkey,
                            pw,
                            confirm,
                            ack,
                            err,
                            kit_available,
                            CreationBackupHub::default(),
                        );
                    }
                }
            }
        }
    }

    // ── Passkey creation is gated like every other creation ──────────────────
    //
    // These run everywhere, including unsigned macOS: a passkey Cube writes no
    // seed file and mints no device secret, so nothing here touches the
    // data-protection keychain that makes the PIN-path tests `#[ignore]`d.

    /// A PRF output to derive from. Not a real ceremony result — the value is
    /// irrelevant, only that the same 32 bytes always produce the same Cube.
    fn fake_prf(byte: u8) -> zeroize::Zeroizing<[u8; 32]> {
        zeroize::Zeroizing::new([byte; 32])
    }

    /// Complete a registration through the **webview** ceremony's real message,
    /// exactly as `PasskeyCeremony` delivers it.
    fn webview_registration(home: &mut Home, credential_id: &str, prf: u8) -> Task<Message> {
        home.update(Message::PasskeyCeremonyResult(Ok(
            CeremonyOutcome::Registered(passkey_svc::PasskeyRegistration {
                credential_id: credential_id.to_string(),
                prf_output: fake_prf(prf),
            }),
        )))
    }

    /// Complete a registration through the **native macOS** ceremony's entry
    /// point. The channel-and-ObjC-controller half cannot be built in a unit
    /// test, so this is the deepest common point both paths share — and it is
    /// the whole of the native arm apart from reading the channel.
    fn native_registration(home: &mut Home, credential_id: &[u8], prf: u8) -> Task<Message> {
        home.passkey_registration_succeeded_raw(credential_id, &fake_prf(prf))
    }

    /// Put the panel in "the user pressed Create Cube in passkey mode".
    fn passkey_create_form(home: &mut Home, name: &str) {
        home.passkey_mode = true;
        home.create_cube_name.value = name.to_string();
        home.create_cube_name.valid = true;
    }

    /// Accept the acknowledgement and let the write run.
    fn acknowledge_and_confirm(home: &mut Home) -> Vec<Message> {
        // The acknowledgement is now one exit of the shared choice screen.
        if !matches!(
            home.state,
            State::CreationBackup(CreationBackupStep::Bypass { .. })
        ) {
            let _ = home.update(Message::View(ViewMessage::CreationBackupBypassRequested));
        }
        let _ = home.update(Message::View(ViewMessage::CreationBackupAcknowledgeBypass(
            true,
        )));
        drain(home.update(Message::View(ViewMessage::CreationBackupBypassConfirmed)))
    }

    /// **The audited hole, as a test.** Registration alone must not put a Cube
    /// on disk — and it must never put one there in the shape that reads as
    /// "predates the gate" (`creation_backup_required == false`) while carrying
    /// no backup evidence at all.
    #[test]
    fn a_passkey_registration_alone_never_reaches_disk() {
        for native in [false, true] {
            let dir = tmp_datadir("passkey-no-write");
            let mut home = home_with_datadir(&dir);
            passkey_create_form(&mut home, "Registered");

            let out = drain(if native {
                native_registration(&mut home, b"raw-credential", 7)
            } else {
                webview_registration(&mut home, "b64-credential", 7)
            });

            assert!(
                out.is_empty(),
                "registration must not produce a CubeCreated on its own: {:?}",
                out
            );
            assert!(
                cubes_on_disk(&dir).is_empty(),
                "a registered passkey credential wrote a Cube before any backup step"
            );
            assert!(
                matches!(
                    home.state,
                    State::CreationBackup(CreationBackupStep::Choice)
                ),
                "registration must hand over to the creation-backup step, got {:?}",
                home.state
            );
            assert!(home.creating_passkey_cube());
            assert!(!home.creating_cube, "the spinner must clear for the step");

            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The forbidden combination, stated directly: no Cube may be persisted
    /// with `backed_up == false`, no bypass, and the gate disarmed. That is the
    /// shape `evaluate_for_cube` waves through as legacy, and it is exactly
    /// what passkey creation used to write.
    #[test]
    fn no_passkey_cube_lands_ungated_without_evidence() {
        let dir = tmp_datadir("passkey-shape");
        let mut home = home_with_datadir(&dir);
        passkey_create_form(&mut home, "Shape");
        let _ = drain(webview_registration(&mut home, "cred", 3));
        let _ = acknowledge_and_confirm(&mut home);

        let on_disk = cubes_on_disk(&dir);
        assert_eq!(on_disk.len(), 1, "expected one Cube, got {:?}", on_disk);
        for cube in &on_disk {
            assert!(cube.is_passkey_cube(), "the Cube must carry its passkey");
            assert!(
                cube.creation_backup_required,
                "a passkey Cube reached disk disarmed — it would be treated as \
                 predating the gate and never asked for a backup again"
            );
            assert!(
                cube.backed_up || cube.creation_backup_bypass.is_some(),
                "a passkey Cube reached disk with no backup evidence"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cancelling before evidence leaves nothing behind — no Cube, and no
    /// registration this process could still turn into one.
    #[test]
    fn cancelling_passkey_creation_leaves_no_cube() {
        let dir = tmp_datadir("passkey-cancel");
        let mut home = home_with_datadir(&dir);
        passkey_create_form(&mut home, "Cancelled");
        let _ = drain(webview_registration(&mut home, "cred", 9));
        assert!(home.creating_passkey_cube());

        let out = drain(home.update(Message::View(ViewMessage::CancelCreationBackup)));
        assert!(out.is_empty(), "cancelling must create nothing: {:?}", out);
        assert!(
            cubes_on_disk(&dir).is_empty(),
            "a cancelled passkey creation must leave no Cube in settings.json"
        );
        assert!(
            !home.creating_passkey_cube(),
            "the registration must not survive a cancel"
        );
        assert!(matches!(
            home.state,
            State::Cubes {
                create_cube: true,
                ..
            }
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A passkey Cube that completed creation opens: the gate is armed *and*
    /// satisfied by the recorded bypass, never `Blocked`.
    #[test]
    fn a_created_passkey_cube_passes_the_creation_gate() {
        for native in [false, true] {
            let dir = tmp_datadir("passkey-gate");
            let mut home = home_with_datadir(&dir);
            passkey_create_form(&mut home, "Openable");
            let _ = drain(if native {
                native_registration(&mut home, b"raw", 5)
            } else {
                webview_registration(&mut home, "b64", 5)
            });
            let out = acknowledge_and_confirm(&mut home);

            let [Message::CubeCreated(Ok(cube))] = out.as_slice() else {
                panic!("expected exactly one successful CubeCreated, got {:?}", out);
            };
            assert_eq!(
                creation_gate::evaluate_for_cube(cube, None),
                creation_gate::CreationGate::Bypassed,
            );
            for on_disk in cubes_on_disk(&dir) {
                assert!(
                    !matches!(
                        creation_gate::evaluate_for_cube(&on_disk, None),
                        creation_gate::CreationGate::Blocked(_)
                    ),
                    "a passkey Cube straight out of creation must not be blocked at open"
                );
            }

            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// The bypass a passkey Cube records is the same evidence the PIN flow
    /// records — same constant, stored verbatim, with a timestamp — so support
    /// can answer "did this user bypass?" identically for both shapes.
    #[test]
    fn a_bypassed_passkey_cube_records_the_same_evidence_as_the_pin_flow() {
        let dir = tmp_datadir("passkey-evidence");
        let mut home = home_with_datadir(&dir);
        passkey_create_form(&mut home, "Bypassed");
        let _ = drain(webview_registration(&mut home, "cred", 11));

        // The unticked box must not be enough.
        let out = drain(home.update(Message::View(ViewMessage::CreationBackupBypassConfirmed)));
        assert!(
            out.is_empty(),
            "an unacknowledged bypass must not create a Cube: {:?}",
            out
        );
        assert!(cubes_on_disk(&dir).is_empty());

        let _ = acknowledge_and_confirm(&mut home);
        let on_disk = cubes_on_disk(&dir);
        assert_eq!(on_disk.len(), 1);
        let bypass = on_disk[0]
            .creation_backup_bypass
            .as_ref()
            .expect("the bypass must be recorded on the Cube");
        assert_eq!(
            bypass.acknowledged,
            creation_gate::BYPASS_ACKNOWLEDGEMENT,
            "the acknowledgement must be stored verbatim, as the PIN flow does"
        );
        assert!(bypass.at > 0, "the bypass must carry when it happened");
        assert!(
            !on_disk[0].backed_up,
            "a bypass is not a backup and must not be recorded as one"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Both ceremony backends produce the same Cube from the same PRF output,
    /// and both go through the same gate. If one path ever stops routing
    /// through the shared transition, this diverges.
    #[test]
    fn both_ceremony_paths_produce_the_same_gated_cube() {
        let mut fingerprints = Vec::new();
        for native in [false, true] {
            let dir = tmp_datadir("passkey-parity");
            let mut home = home_with_datadir(&dir);
            passkey_create_form(&mut home, "Parity");
            let _ = drain(if native {
                // Same bytes either way: the webview delivers the base64 of
                // what the native path delivers raw.
                native_registration(&mut home, b"same-credential", 21)
            } else {
                webview_registration(
                    &mut home,
                    &base64::Engine::encode(
                        &base64::engine::general_purpose::STANDARD,
                        b"same-credential",
                    ),
                    21,
                )
            });
            let out = acknowledge_and_confirm(&mut home);
            let [Message::CubeCreated(Ok(cube))] = out.as_slice() else {
                panic!("expected one CubeCreated, got {:?}", out);
            };
            assert_eq!(
                cube.passkey_metadata
                    .as_ref()
                    .map(|m| m.credential_id.clone()),
                Some(base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    b"same-credential"
                )),
                "both paths must persist the same base64 credential id"
            );
            assert!(cube.creation_backup_required);
            assert!(cube.creation_backup_bypass.is_some());
            fingerprints.push(cube.master_signer_fingerprint);

            let _ = std::fs::remove_dir_all(&dir);
        }
        assert_eq!(
            fingerprints[0], fingerprints[1],
            "the same PRF output must derive the same master signer on both paths"
        );
        assert!(
            fingerprints[0].is_some(),
            "the fingerprint must be recorded"
        );
    }

    /// **A passkey Cube can now write its phrase down.** Same wizard, same
    /// screens, same verification, same evidence as a PIN Cube — which is only
    /// possible because both derive a 12-word mnemonic the restore grid takes
    /// back.
    ///
    /// This is the exit that works with no Connect account at all.
    #[test]
    fn a_passkey_cube_can_take_the_written_phrase_exit() {
        let dir = tmp_datadir("passkey-phrase-exit");
        // Signed *out*: the Kit is unreachable, so this is the only backup on
        // offer — and it is a real one.
        let mut home = home_with_datadir(&dir);
        assert!(!home.can_create_recovery_kit());
        passkey_create_form(&mut home, "WrittenDown");
        let _ = drain(webview_registration(&mut home, "cred", 13));
        assert_eq!(step(&home), CreationBackupStep::Choice);

        let words = shown_words(&home);
        assert_eq!(words.len(), 12, "the wizard shows a 12-word phrase");

        let out = drain(walk_backup_to_verified(&mut home));
        let [Message::CubeCreated(Ok(cube))] = out.as_slice() else {
            panic!("verification must create the Cube, got {:?}", out);
        };

        assert!(cube.is_passkey_cube());
        assert!(
            cube.backed_up,
            "a completed verification is the same evidence for either shape of Cube"
        );
        assert!(cube.creation_backup_required, "the gate is still armed");
        assert!(cube.creation_backup_bypass.is_none());
        assert!(cube.creation_recovery_kit.is_none());
        assert_eq!(
            creation_gate::evaluate_for_cube(cube, None),
            creation_gate::CreationGate::Satisfied,
            "a written-down passkey Cube must open, with no network"
        );

        // And the phrase it showed is the one that rebuilds it.
        let restored = MasterSigner::from_str(Network::Bitcoin, &words.join(" ")).unwrap();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        assert_eq!(
            Some(restored.fingerprint(&secp)),
            cube.master_signer_fingerprint,
            "the phrase shown must rebuild this exact Cube"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PIN Cube gets the Recovery Kit exit too. One flow, three exits, both
    /// unlock methods — the Kit's availability depends on the Connect session,
    /// never on how the Cube is unlocked.
    #[test]
    fn a_pin_cube_can_take_the_recovery_kit_exit() {
        let dir = tmp_datadir("pin-kit-exit");
        let mut home = signed_in_home_with_datadir(&dir);
        enter_backup_step(&mut home, "PinWithKit", "4321");
        assert_eq!(step(&home), CreationBackupStep::Choice);
        assert!(!home.creating_passkey_cube(), "this is the PIN path");

        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        assert_eq!(
            step(&home),
            CreationBackupStep::KitPassword,
            "the Kit must be offered to a PIN Cube as well"
        );
        type_kit_password(&mut home, "correct horse battery staple");
        home.state = State::CreationBackup(CreationBackupStep::KitUploading);

        let uploaded = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                7,
                kit_evidence(77),
            ))))),
        );
        assert!(
            uploaded.is_empty(),
            "a saved Kit returns to the backup hub; Continue writes the Cube"
        );
        assert_eq!(step(&home), CreationBackupStep::Choice);
        let out = drain(home.update(Message::View(ViewMessage::CreationBackupContinue)));
        let [Message::CubeCreated(result)] = out.as_slice() else {
            panic!("expected one CubeCreated, got {:?}", out);
        };
        // The PIN write can refuse on an unsigned macOS build (no keystore);
        // what matters is that when it succeeds it carries the Kit evidence,
        // and when it fails it leaves nothing behind.
        match result {
            Ok(cube) => {
                assert_eq!(
                    cube.creation_recovery_kit.as_ref().map(|k| k.cube_id),
                    Some(77)
                );
                assert!(cube.creation_backup_required);
                assert!(!cube.backed_up, "a Kit is not the written-phrase backup");
                assert_eq!(
                    creation_gate::evaluate_for_cube(cube, None),
                    creation_gate::CreationGate::Satisfied
                );
            }
            Err(_) => assert!(cubes_on_disk(&dir).is_empty()),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default creation method no longer has to hedge: both paths run the
    /// same wizard and reach the same exits, so passkey is the default wherever
    /// the platform supports it — signed in or not.
    #[test]
    fn the_default_creation_method_is_passkey_wherever_it_is_supported() {
        let dir = tmp_datadir("passkey-default");

        for mut home in [home_with_datadir(&dir), signed_in_home_with_datadir(&dir)] {
            assert_eq!(
                home.default_passkey_mode(),
                feature_flags::PASSKEY_CREATION_AVAILABLE,
                "the default must not depend on the Connect session — the \
                 written-phrase exit needs no account"
            );

            // Opening the form applies it.
            home.passkey_mode = !home.default_passkey_mode();
            let expected = home.default_passkey_mode();
            let _ = home.update(Message::View(ViewMessage::ShowCreateCube(true)));
            assert_eq!(home.passkey_mode, expected);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Recovery Kit during creation ─────────────────────────────────────

    /// A signed-in Home rooted at `dir`, so `can_create_recovery_kit()` is true.
    /// No network call is made by any test below: every one either stops at a
    /// synchronous gate or feeds the upload result in directly.
    fn signed_in_home_with_datadir(dir: &std::path::Path) -> Home {
        let mut home = home_with_datadir(dir);
        home.connect_account.step = ConnectFlowStep::Dashboard;
        home.connect_account.user = Some(User {
            id: 7,
            email: "founder@example.com".to_string(),
            email_verified: Some(true),
        });
        home
    }

    fn kit_evidence(cube_id: u64) -> creation_gate::CreationRecoveryKit {
        creation_gate::CreationRecoveryKit {
            at: 1_700_000_000,
            cube_id,
            has_seed: true,
        }
    }

    /// Fill in a valid recovery password on the Kit screen.
    fn type_kit_password(home: &mut Home, password: &str) {
        let _ = home.update(Message::View(ViewMessage::CreationKit(
            RecoveryKitMessage::PasswordChanged(zeroize::Zeroizing::new(password.to_string())),
        )));
        let _ = home.update(Message::View(ViewMessage::CreationKit(
            RecoveryKitMessage::ConfirmChanged(zeroize::Zeroizing::new(password.to_string())),
        )));
        let _ = home.update(Message::View(ViewMessage::CreationKit(
            RecoveryKitMessage::AcknowledgeToggled(true),
        )));
    }

    /// The backup hub collects either or both: a confirmed phrase and a Kit
    /// return to it, Continue does nothing until one exists, and the Cube it
    /// writes records both.
    #[test]
    fn backup_hub_records_the_seed_phrase_and_recovery_kit_together() {
        let dir = tmp_datadir("hub-both");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Both");
        let _ = drain(webview_registration(&mut home, "cred", 9));
        assert_eq!(step(&home), CreationBackupStep::Choice);
        assert!(
            drain(home.update(Message::View(ViewMessage::CreationBackupContinue))).is_empty(),
            "Continue must not write a Cube with no backup"
        );
        assert!(cubes_on_disk(&dir).is_empty());

        // Seed phrase, back to the hub.
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let _ = home.update(backup(BackupWalletMessage::ToggleBackupIntroCheck));
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let words = shown_words(&home);
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let State::CreationBackup(CreationBackupStep::Verification { word_indices, .. }) =
            home.state.clone()
        else {
            panic!("expected the verification screen, got {:?}", home.state);
        };
        for &index in &word_indices {
            let _ = home.update(backup(BackupWalletMessage::WordInput {
                index: index as u8,
                input: words[index - 1].clone(),
            }));
        }
        assert!(drain(home.update(backup(BackupWalletMessage::VerifyPhrase))).is_empty());
        assert_eq!(step(&home), CreationBackupStep::Choice);
        assert!(
            cubes_on_disk(&dir).is_empty(),
            "nothing is written before Continue"
        );
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        assert_eq!(
            step(&home),
            CreationBackupStep::Choice,
            "a confirmed phrase is not walked again"
        );

        // Recovery Kit, back to the hub.
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        assert_eq!(step(&home), CreationBackupStep::KitPassword);
        type_kit_password(&mut home, "correct horse battery staple");
        home.state = State::CreationBackup(CreationBackupStep::KitUploading);
        assert!(drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                7,
                kit_evidence(5),
            )))))
        )
        .is_empty());
        assert_eq!(step(&home), CreationBackupStep::Choice);
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        assert_eq!(
            step(&home),
            CreationBackupStep::Choice,
            "a second Kit is not offered for the same creation"
        );

        let out = drain(home.update(Message::View(ViewMessage::CreationBackupContinue)));
        let [Message::CubeCreated(Ok(cube))] = out.as_slice() else {
            panic!("expected exactly one successful CubeCreated, got {:?}", out);
        };
        assert!(cube.backed_up, "the confirmed phrase must be recorded");
        assert_eq!(
            cube.creation_recovery_kit.as_ref().map(|k| k.cube_id),
            Some(5),
            "and so must the Kit"
        );
        assert_eq!(
            creation_gate::evaluate_for_cube(cube, None),
            creation_gate::CreationGate::Satisfied
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Kit belongs to its registering account even if the session changes
    /// before the upload result arrives. Both creation paths must refuse to
    /// write it under another account, including with a confirmed seed phrase.
    #[test]
    fn backup_hub_requires_the_kits_registering_account() {
        for passkey in [false, true] {
            for switch_during_upload in [false, true] {
                let dir = tmp_datadir("hub-kit-account");
                let mut home = signed_in_home_with_datadir(&dir);
                if passkey {
                    passkey_create_form(&mut home, "AccountBound");
                    let _ = drain(webview_registration(&mut home, "cred", 19));
                } else {
                    enter_backup_step(&mut home, "AccountBound", "4321");
                }
                confirm_seed_from_hub(&mut home);
                home.state = State::CreationBackup(CreationBackupStep::KitUploading);
                if switch_during_upload {
                    home.connect_account.user.as_mut().unwrap().id = 8;
                }
                let _ = drain(
                    home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                        7,
                        kit_evidence(42),
                    ))))),
                );
                assert_eq!(
                    home.creation_kit_evidence.as_ref().map(|(id, _)| *id),
                    Some(7)
                );
                home.connect_account.user.as_mut().unwrap().id = 8;
                let pending_id = home.pending_cube_id;

                for signed_out in [false, true] {
                    if signed_out {
                        home.connect_account.user = None;
                        assert_eq!(home.connect_account_id(), None);
                    }
                    assert!(
                        drain(home.update(Message::View(ViewMessage::CreationBackupContinue)))
                            .is_empty()
                    );
                    assert_eq!(step(&home), CreationBackupStep::Choice);
                    assert!(home.error.as_deref().unwrap().contains("Sign back in"));
                    assert!(!home.creating_cube);
                    assert_eq!(home.pending_cube_id, pending_id);
                    assert!(home.creation_kit_evidence.is_some());
                    assert!(home.creation_backup_words.is_some());
                    assert!(cubes_on_disk(&dir).is_empty());
                }

                // Signing back in to the registering account can finish the
                // same Cube with both backups and the correct Connect owner.
                if passkey {
                    home.connect_account.step = ConnectFlowStep::Dashboard;
                    home.connect_account.user = Some(User {
                        id: 7,
                        email: "founder@example.com".to_string(),
                        email_verified: Some(true),
                    });
                    let out =
                        drain(home.update(Message::View(ViewMessage::CreationBackupContinue)));
                    let [Message::CubeCreated(Ok(cube))] = out.as_slice() else {
                        panic!("expected one successful CubeCreated, got {:?}", out);
                    };
                    assert!(cube.remote_synced);
                    assert!(cube.backed_up);
                    assert_eq!(
                        cube.connect_owner.as_ref().map(|owner| owner.user_id),
                        Some(7)
                    );
                    assert_eq!(
                        cube.creation_recovery_kit.as_ref().map(|kit| kit.cube_id),
                        Some(42)
                    );
                    let _ = drain(home.update(Message::CubeCreated(Ok(cube.clone()))));
                } else {
                    let _ = home.update(Message::View(ViewMessage::CancelCreationBackup));
                }
                assert!(
                    home.creation_kit_evidence.is_none(),
                    "the registering account is cleared too"
                );
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn backup_hub_shows_why_another_account_cannot_continue() {
        let dir = tmp_datadir("hub-kit-account-warning");
        let mut home = signed_in_home_with_datadir(&dir);
        home.state = State::CreationBackup(CreationBackupStep::Choice);
        home.creation_kit_evidence = Some((8, kit_evidence(42)));
        drop(home.finish_creation_backup());

        let shown = restore_frame::rendered_labels(home.create_journey_frame()).await;
        assert!(shown.iter().any(|label| Some(label) == home.error.as_ref()));
        assert!(home.error.as_deref().unwrap().contains("Sign back in"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An authenticated client without its user record cannot identify the
    /// registering account, so no registration or upload may start yet.
    #[test]
    fn a_creation_kit_waits_for_the_connect_account_to_load() {
        let dir = tmp_datadir("kit-account-loading");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Loading");
        let _ = drain(webview_registration(&mut home, "cred", 20));
        let _ = home.update(Message::View(ViewMessage::CreationKitRequested));
        type_kit_password(&mut home, "correct horse battery staple");
        home.connect_account
            .install_shared_client(home.connect_account.client.clone());
        home.connect_account.user = None;
        assert!(home.can_create_recovery_kit());
        assert!(drain(home.submit_creation_kit()).is_empty());
        assert_eq!(step(&home), CreationBackupStep::KitPassword);
        assert!(!home.creating_cube);
        assert!(home
            .creation_kit_error
            .as_deref()
            .unwrap()
            .contains("still loading"));
        assert!(home.creation_kit_evidence.is_none());
        assert!(cubes_on_disk(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Walk the seed phrase from the hub and answer it correctly; ends back on
    /// the hub with the phrase confirmed.
    fn confirm_seed_from_hub(home: &mut Home) {
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let _ = home.update(backup(BackupWalletMessage::ToggleBackupIntroCheck));
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let words = shown_words(home);
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        let State::CreationBackup(CreationBackupStep::Verification { word_indices, .. }) =
            home.state.clone()
        else {
            panic!("expected the verification screen, got {:?}", home.state);
        };
        for &index in &word_indices {
            let _ = home.update(backup(BackupWalletMessage::WordInput {
                index: index as u8,
                input: words[index - 1].clone(),
            }));
        }
        let _ = drain(home.update(backup(BackupWalletMessage::VerifyPhrase)));
        assert_eq!(step(home), CreationBackupStep::Choice);
        assert!(home.creation_seed_confirmed);
    }

    /// A written Cube leaves the hub at once, and its confirmed phrase and Kit
    /// are not carried into the next creation in the same session.
    #[test]
    fn backup_hub_progress_does_not_outlive_its_cube() {
        let dir = tmp_datadir("hub-twice");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "First");
        let _ = drain(webview_registration(&mut home, "cred-1", 11));
        confirm_seed_from_hub(&mut home);
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        type_kit_password(&mut home, "correct horse battery staple");
        home.state = State::CreationBackup(CreationBackupStep::KitUploading);
        let _ = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                7,
                kit_evidence(21),
            ))))),
        );
        let out = drain(home.update(Message::View(ViewMessage::CreationBackupContinue)));
        let [Message::CubeCreated(result)] = out.as_slice() else {
            panic!("expected one CubeCreated, got {:?}", out);
        };
        let _ = drain(home.update(Message::CubeCreated(result.clone())));
        assert!(
            !matches!(home.state, State::CreationBackup(_)),
            "a written Cube must not leave its hub (and its Continue) on screen"
        );
        assert_eq!(home.creation_backup_hub(), CreationBackupHub::default());
        assert!(drain(home.update(Message::View(ViewMessage::CreationBackupContinue))).is_empty());

        passkey_create_form(&mut home, "Second");
        let _ = drain(webview_registration(&mut home, "cred-2", 12));
        assert_eq!(step(&home), CreationBackupStep::Choice);
        assert_eq!(
            home.creation_backup_hub(),
            CreationBackupHub::default(),
            "the second Cube must start with nothing backed up"
        );
        assert!(
            drain(home.update(Message::View(ViewMessage::CreationBackupContinue))).is_empty(),
            "Continue must not write the second Cube with the first one's backups"
        );
        assert_eq!(cubes_on_disk(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A reload landing while the hub is open (the catch-up sync after Sign
    /// in, say) refreshes the list Cancel returns to and leaves the step alone.
    #[test]
    fn a_reload_during_the_backup_step_keeps_the_unwritten_cube() {
        let dir = tmp_datadir("hub-reload");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Pending");
        let _ = drain(webview_registration(&mut home, "cred", 6));
        confirm_seed_from_hub(&mut home);

        let network = home.network;
        drop(home.update(Message::Checked {
            for_chain: network,
            res: Ok(State::Cubes {
                cubes: vec![CubeSettings::new("Synced".into(), network)],
                create_cube: false,
                source: network,
            }),
        }));
        assert_eq!(step(&home), CreationBackupStep::Choice);
        assert!(home.creation_seed_confirmed);
        assert!(home.creation_backup_words.is_some());
        assert!(home.pending_passkey_cube.is_some());

        let _ = home.update(Message::View(ViewMessage::CancelCreationBackup));
        let State::Cubes { cubes, .. } = &home.state else {
            panic!("expected the Cube list after Cancel, got {:?}", home.state);
        };
        assert_eq!(cubes.len(), 1, "Cancel returns to the refreshed list");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cancel after a Kit was saved: the retry must not reuse the UUID that
    /// Connect now holds a Kit for, sealing the abandoned seed.
    #[test]
    fn cancelling_after_a_saved_kit_gives_the_retry_a_new_uuid() {
        let dir = tmp_datadir("hub-cancel-kit");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Abandoned");
        let _ = drain(webview_registration(&mut home, "cred", 8));
        let kit_uuid = home.pending_cube_id;
        assert!(kit_uuid.is_some());
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        type_kit_password(&mut home, "correct horse battery staple");
        home.state = State::CreationBackup(CreationBackupStep::KitUploading);
        let _ = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                7,
                kit_evidence(31),
            ))))),
        );
        let _ = home.update(Message::View(ViewMessage::CancelCreationBackup));
        assert_eq!(home.pending_cube_id, None);
        assert_eq!(home.creation_backup_hub(), CreationBackupHub::default());

        // Without a Kit, a retry keeps its UUID as before.
        passkey_create_form(&mut home, "Plain");
        let _ = drain(webview_registration(&mut home, "cred-2", 9));
        let plain_uuid = home.pending_cube_id;
        let _ = home.update(Message::View(ViewMessage::CancelCreationBackup));
        assert_eq!(home.pending_cube_id, plain_uuid);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Once a backup exists the bypass is refused: it would record "no backup"
    /// over a finished one.
    #[test]
    fn the_bypass_is_refused_once_a_backup_exists() {
        let dir = tmp_datadir("hub-bypass");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Kept");
        let _ = drain(webview_registration(&mut home, "cred", 10));
        confirm_seed_from_hub(&mut home);
        let _ = home.update(Message::View(ViewMessage::CreationBackupBypassRequested));
        assert_eq!(step(&home), CreationBackupStep::Choice);
        let _ = home.update(Message::View(ViewMessage::CreationBackupBypassConfirmed));
        assert!(cubes_on_disk(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The hub's guards hold even for messages its screens don't send: an
    /// acknowledged bypass with a Kit already saved, a network switch, and a
    /// Kit result the step isn't waiting for.
    #[test]
    fn the_backup_hub_refuses_messages_that_would_discard_or_misfile_a_backup() {
        let dir = tmp_datadir("hub-guards");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Guarded");
        let _ = drain(webview_registration(&mut home, "cred", 13));

        // A stray Kit result outside the upload is ignored.
        let _ = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                7,
                kit_evidence(40),
            ))))),
        );
        assert_eq!(home.creation_backup_hub(), CreationBackupHub::default());

        // Acknowledge the bypass, then make a Kit from there.
        let _ = home.update(Message::View(ViewMessage::CreationBackupBypassRequested));
        let _ = home.update(Message::View(ViewMessage::CreationBackupAcknowledgeBypass(
            true,
        )));
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        assert_eq!(step(&home), CreationBackupStep::KitPassword);
        type_kit_password(&mut home, "correct horse battery staple");
        // The upload itself is Connect's; stand in for it having started.
        home.state = State::CreationBackup(CreationBackupStep::KitUploading);
        let _ = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                7,
                kit_evidence(41),
            ))))),
        );
        assert!(home.creation_backup_hub().kit_saved);

        // Even forced back onto an acknowledged bypass, confirming it must not
        // write "no backup" over the Kit.
        home.state = State::CreationBackup(CreationBackupStep::Bypass { acknowledged: true });
        assert!(
            drain(home.update(Message::View(ViewMessage::CreationBackupBypassConfirmed)))
                .is_empty()
        );
        assert!(cubes_on_disk(&dir).is_empty());

        // The network can't change under the unwritten Cube. Developer mode
        // makes Signet selectable, so only the creation guard refuses it.
        home.state = State::CreationBackup(CreationBackupStep::Choice);
        home.developer_mode = true;
        let network = home.network;
        assert_ne!(network, ChainId::Signet);
        assert!(
            drain(home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Signet))))
                .is_empty()
        );
        assert_eq!(home.network, network);
        assert!(home.error.is_some(), "the refusal says why");
        assert_eq!(step(&home), CreationBackupStep::Choice);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Sign in from the hub's Recovery Kit row opens Connect over the step;
    /// coming back finds the same unwritten Cube and its seed.
    #[test]
    fn signing_in_from_the_backup_hub_keeps_the_unwritten_cube() {
        let dir = tmp_datadir("hub-sign-in");
        let mut home = home_with_datadir(&dir);
        passkey_create_form(&mut home, "Later");
        let _ = drain(webview_registration(&mut home, "cred", 4));
        assert_eq!(step(&home), CreationBackupStep::Choice);
        assert!(!home.can_create_recovery_kit());

        let _ = home.update(Message::View(ViewMessage::GoToSection(
            HomeSection::Connect(app::menu::ConnectSubMenu::Overview),
        )));
        let _ = home.update(Message::View(ViewMessage::GoToSection(HomeSection::Cubes)));
        assert_eq!(step(&home), CreationBackupStep::Choice);
        assert!(home.creation_backup_words.is_some());
        assert!(home.pending_passkey_cube.is_some());
        assert!(cubes_on_disk(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The choice screen is the entry point either way; what changes with the
    /// session is whether the **Kit** exit is on it. It is never offered as a
    /// button that cannot work, and never reachable when it can't.
    #[test]
    fn the_kit_exit_appears_only_when_connect_is_reachable() {
        // Signed out: choice screen, no Kit.
        let dir = tmp_datadir("kit-offer-out");
        let mut home = home_with_datadir(&dir);
        passkey_create_form(&mut home, "SignedOut");
        let _ = drain(webview_registration(&mut home, "cred", 3));
        assert!(!home.can_create_recovery_kit());
        assert_eq!(step(&home), CreationBackupStep::Choice);

        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        assert_eq!(
            step(&home),
            CreationBackupStep::Choice,
            "the Kit must not be reachable with nowhere to upload it"
        );
        // The written-phrase exit still is.
        let _ = home.update(backup(BackupWalletMessage::NextStep));
        assert_eq!(step(&home), CreationBackupStep::Intro(false));
        let _ = std::fs::remove_dir_all(&dir);

        // Signed in: same screen, Kit reachable.
        let dir = tmp_datadir("kit-offer-in");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "SignedIn");
        let _ = drain(webview_registration(&mut home, "cred", 3));
        assert!(home.can_create_recovery_kit());
        assert_eq!(step(&home), CreationBackupStep::Choice);
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        assert_eq!(step(&home), CreationBackupStep::KitPassword);
        assert!(cubes_on_disk(&dir).is_empty(), "still nothing written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The password gates are re-checked in the handler, not just in the view:
    /// too short, mismatched, or unacknowledged never starts an upload and
    /// never writes a Cube.
    #[test]
    fn a_weak_or_unconfirmed_recovery_password_never_starts_an_upload() {
        let dir = tmp_datadir("kit-password-gates");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Gates");
        let _ = drain(webview_registration(&mut home, "cred", 5));
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        assert!(matches!(
            home.state,
            State::CreationBackup(CreationBackupStep::KitPassword)
        ));

        let submit = |home: &mut Home| {
            home.update(Message::View(ViewMessage::CreationKit(
                RecoveryKitMessage::SubmitPassword,
            )))
        };

        // Too short.
        type_kit_password(&mut home, "short");
        let _ = drain(submit(&mut home));
        assert!(home.creation_kit_error.is_some());
        assert!(matches!(
            home.state,
            State::CreationBackup(CreationBackupStep::KitPassword)
        ));

        // Long enough, but the confirmation doesn't match.
        type_kit_password(&mut home, "correct horse battery staple");
        let _ = home.update(Message::View(ViewMessage::CreationKit(
            RecoveryKitMessage::ConfirmChanged(zeroize::Zeroizing::new("something else".into())),
        )));
        let _ = drain(submit(&mut home));
        assert_eq!(
            home.creation_kit_error.as_deref(),
            Some("Passwords don't match.")
        );

        // Matching, but not acknowledged.
        type_kit_password(&mut home, "correct horse battery staple");
        let _ = home.update(Message::View(ViewMessage::CreationKit(
            RecoveryKitMessage::AcknowledgeToggled(false),
        )));
        let _ = drain(submit(&mut home));
        assert!(home
            .creation_kit_error
            .as_deref()
            .is_some_and(|e| e.contains("written")));

        assert!(
            cubes_on_disk(&dir).is_empty(),
            "no refused password may write a Cube"
        );
        assert!(!home.creating_cube);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Kit made during creation is what satisfies the gate — and the Cube
    /// records it, so the gate is satisfied **offline**, which is the only way
    /// it is ever evaluated at open time.
    #[test]
    fn a_cube_created_with_a_kit_is_satisfied_not_bypassed() {
        let dir = tmp_datadir("kit-satisfies");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Kitted");
        let _ = drain(webview_registration(&mut home, "cred", 7));
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        type_kit_password(&mut home, "correct horse battery staple");

        // The upload's result, fed in directly — the network half is the
        // Connect client's, not this state machine's.
        home.state = State::CreationBackup(CreationBackupStep::KitUploading);
        let uploaded = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                7,
                kit_evidence(42),
            ))))),
        );
        assert!(
            uploaded.is_empty(),
            "a saved Kit returns to the backup hub; Continue writes the Cube"
        );
        assert_eq!(step(&home), CreationBackupStep::Choice);
        let out = drain(home.update(Message::View(ViewMessage::CreationBackupContinue)));
        let [Message::CubeCreated(Ok(cube))] = out.as_slice() else {
            panic!("expected exactly one successful CubeCreated, got {:?}", out);
        };

        assert_eq!(
            creation_gate::evaluate_for_cube(cube, None),
            creation_gate::CreationGate::Satisfied,
            "a Kit is a backup, not a bypass"
        );
        let on_disk = cubes_on_disk(&dir);
        assert_eq!(on_disk.len(), 1);
        let saved = &on_disk[0];
        assert_eq!(
            saved.creation_recovery_kit.as_ref().map(|k| k.cube_id),
            Some(42),
            "the Cube must record the Kit it was created with"
        );
        assert!(saved.creation_backup_required, "the gate is still armed");
        assert!(
            saved.creation_backup_bypass.is_none(),
            "a Kit must not also record a bypass"
        );
        assert!(
            !saved.backed_up,
            "a Kit is not the written-phrase backup and must not claim to be"
        );
        assert!(
            saved.remote_synced,
            "the Kit could only be uploaded against a registered Cube"
        );
        assert_eq!(
            creation_gate::evaluate_for_cube(saved, None),
            creation_gate::CreationGate::Satisfied,
            "the gate must be satisfiable with no server status at all"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A failed upload writes nothing and leaves both exits open — the user can
    /// retry the Kit or take the acknowledgement.
    #[test]
    fn a_failed_kit_upload_writes_no_cube_and_keeps_both_exits() {
        let dir = tmp_datadir("kit-upload-fails");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Failed");
        let _ = drain(webview_registration(&mut home, "cred", 11));
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        type_kit_password(&mut home, "correct horse battery staple");
        home.state = State::CreationBackup(CreationBackupStep::KitUploading);

        let out = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Err(
                "Connect is unreachable".to_string(),
            )))),
        );
        assert!(out.is_empty(), "a failed upload must create nothing");
        assert!(cubes_on_disk(&dir).is_empty());
        assert!(!home.creating_cube);
        assert_eq!(
            home.creation_kit_error.as_deref(),
            Some("Connect is unreachable")
        );
        assert!(matches!(
            home.state,
            State::CreationBackup(CreationBackupStep::KitPassword)
        ));
        // The seed is still staged, so a retry doesn't need a second ceremony.
        assert!(home.creation_backup_words.is_some());

        // Exit 2 is still available: take the acknowledgement instead.
        let _ = drain(home.update(Message::View(ViewMessage::CreationBackupBypassRequested)));
        let _ = home.update(Message::View(ViewMessage::CreationBackupAcknowledgeBypass(
            true,
        )));
        let out = drain(home.update(Message::View(ViewMessage::CreationBackupBypassConfirmed)));
        let [Message::CubeCreated(Ok(cube))] = out.as_slice() else {
            panic!("expected a bypassed Cube, got {:?}", out);
        };
        assert_eq!(
            creation_gate::evaluate_for_cube(cube, None),
            creation_gate::CreationGate::Bypassed
        );
        assert!(cube.creation_recovery_kit.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "I'll do this later" is reversible while the Cube is still unwritten:
    /// the user can go back and make the Kit after all.
    #[test]
    fn choosing_later_can_be_reversed_back_into_the_kit() {
        let dir = tmp_datadir("kit-later-reversible");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Reversible");
        let _ = drain(webview_registration(&mut home, "cred", 13));

        let _ = drain(home.update(Message::View(ViewMessage::CreationBackupBypassRequested)));
        assert!(matches!(
            home.state,
            State::CreationBackup(CreationBackupStep::Bypass { .. })
        ));

        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        assert!(
            matches!(
                home.state,
                State::CreationBackup(CreationBackupStep::KitPassword)
            ),
            "the acknowledgement must be reversible while nothing is written"
        );
        assert!(cubes_on_disk(&dir).is_empty());

        // Cancelling the password screen goes back to the choice, not out of
        // creation — the gate still has to be resolved.
        let _ = drain(home.update(Message::View(ViewMessage::CreationKit(
            RecoveryKitMessage::Cancel,
        ))));
        assert!(matches!(
            home.state,
            State::CreationBackup(CreationBackupStep::Choice)
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The recovery password and the staged seed are wiped on every exit from
    /// the Kit branch — success and cancellation alike.
    #[test]
    fn the_recovery_password_and_seed_are_scrubbed_on_every_exit() {
        // Success.
        let dir = tmp_datadir("kit-scrub-success");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Scrubbed");
        let _ = drain(webview_registration(&mut home, "cred", 17));
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        type_kit_password(&mut home, "correct horse battery staple");
        assert!(!home.creation_kit_password.is_empty());
        home.state = State::CreationBackup(CreationBackupStep::KitUploading);

        let uploaded = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok((
                7,
                kit_evidence(1),
            ))))),
        );
        assert!(
            uploaded.is_empty(),
            "a saved Kit returns to the backup hub; Continue writes the Cube"
        );
        assert_eq!(step(&home), CreationBackupStep::Choice);
        let out = drain(home.update(Message::View(ViewMessage::CreationBackupContinue)));
        let [Message::CubeCreated(result)] = out.as_slice() else {
            panic!("expected one CubeCreated, got {:?}", out);
        };
        let _ = drain(home.update(Message::CubeCreated(result.clone())));
        assert!(
            home.creation_kit_password.is_empty() && home.creation_kit_confirm.is_empty(),
            "the recovery password must not outlive the creation flow"
        );
        assert!(!home.creation_kit_acknowledged);
        assert!(
            home.creation_backup_words.is_none(),
            "the staged seed must be scrubbed once the Cube exists"
        );
        let _ = std::fs::remove_dir_all(&dir);

        // Cancellation.
        let dir = tmp_datadir("kit-scrub-cancel");
        let mut home = signed_in_home_with_datadir(&dir);
        passkey_create_form(&mut home, "Cancelled");
        let _ = drain(webview_registration(&mut home, "cred", 19));
        let _ = drain(home.update(Message::View(ViewMessage::CreationKitRequested)));
        type_kit_password(&mut home, "correct horse battery staple");

        let _ = drain(home.update(Message::View(ViewMessage::CancelCreationBackup)));
        assert!(home.creation_kit_password.is_empty());
        assert!(home.creation_kit_confirm.is_empty());
        assert!(home.creation_backup_words.is_none());
        assert!(!home.creating_passkey_cube());
        assert!(cubes_on_disk(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Cube registered by the Kit branch must not be registered a second time
    /// when creation completes — that would leave two server-side records for
    /// one Cube.
    #[test]
    fn a_kit_registered_cube_is_not_registered_again() {
        let dir = tmp_datadir("kit-no-double-register");
        let mut home = signed_in_home_with_datadir(&dir);
        let mut cube = CubeSettings::new("Registered".to_string(), Network::Bitcoin);
        cube.remote_synced = true;
        cube.creation_backup_required = true;
        cube.creation_recovery_kit = Some(kit_evidence(99));

        let out = drain(home.update(Message::CubeCreated(Ok(cube))));
        assert!(
            !out.iter()
                .any(|m| matches!(m, Message::CubeRemoteRegistered { .. })),
            "a Cube registered during the Kit upload must not be registered again: {:?}",
            out
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Create Cube journey ends with the Cube. Vault steps belong to the
    /// Create Vault wizard inside the Cube.
    #[test]
    fn create_cube_header_lists_only_cube_steps() {
        // The network is chosen in the sheet header, not on a step.
        assert_eq!(create_cube_steps(false), &["Cube", "Backup", "Done"]);
        assert_eq!(
            create_cube_steps(true),
            &["Cube", "Passkey", "Backup", "Done"]
        );
        for vault_step in ["Connect", "Vault", "Policy", "Keys", "Node"] {
            assert!(!create_cube_steps(false).contains(&vault_step));
            assert!(!create_cube_steps(true).contains(&vault_step));
        }
    }

    /// A newly created Cube opens next, so its Create Vault screen follows
    /// the Create Cube journey. Signed out, nothing waits on Connect.
    #[test]
    fn a_created_cube_opens_once_written() {
        let dir = tmp_datadir("open-created-signed-out");
        let mut home = home_with_datadir(&dir);
        let cube = CubeSettings::new("Opened".to_string(), Network::Bitcoin);
        let id = cube.id.clone();

        let out = drain(home.update(Message::CubeCreated(Ok(cube.clone()))));
        assert!(
            !out.iter()
                .any(|m| matches!(m, Message::CubeRemoteRegistered { .. })),
            "a signed-out Cube is not registered: {:?}",
            out
        );
        assert_eq!(home.open_created_cube.as_ref(), Some(&id));
        assert!(home.open_after_registration.is_none());
        assert_checked_list_opens(&mut home, cube);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reloaded list that holds the created Cube hands it to the unlock
    /// path, once.
    fn assert_checked_list_opens(home: &mut Home, cube: CubeSettings) {
        let id = cube.id.clone();
        let checked = |cube: CubeSettings| Message::Checked {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::Cubes {
                cubes: vec![cube],
                create_cube: false,
                source: ChainId::Bitcoin,
            }),
        };
        let out = drain(home.update(checked(cube.clone())));
        assert!(
            out.iter().any(|m| matches!(
                m,
                Message::Run(_, _, ChainId::Bitcoin, opened) if opened.id == id
            )),
            "the created Cube opens from the checked list: {:?}",
            out
        );
        assert!(home.open_created_cube.is_none());
        let again = drain(home.update(checked(cube)));
        assert!(
            !again.iter().any(|m| matches!(m, Message::Run(..))),
            "a later reload does not open it again: {:?}",
            again
        );
    }

    /// Signed in, the Cube opens only after its registration result lands.
    /// Leaving Home first would drop that result, so a registered Cube would
    /// never be marked synced. A failed registration still opens it.
    #[test]
    fn a_created_cube_waits_for_its_registration_before_opening() {
        let dir = tmp_datadir("open-created-signed-in");
        let mut home = signed_in_home_with_datadir(&dir);
        let cube = CubeSettings::new("Registering".to_string(), Network::Bitcoin);
        let id = cube.id.clone();

        let out = drain(home.update(Message::CubeCreated(Ok(cube.clone()))));
        assert!(
            out.iter()
                .any(|m| matches!(m, Message::CubeRemoteRegistered { .. })),
            "a signed-in Cube is registered: {:?}",
            out
        );
        assert!(home.open_created_cube.is_none());
        assert_eq!(home.open_after_registration.as_ref(), Some(&id));

        let _ = home.update(Message::CubeRemoteRegistered {
            cube_id: id.clone(),
            network: ChainId::Bitcoin,
            result: Err("offline".to_string()),
        });
        assert!(home.open_after_registration.is_none());
        assert_eq!(home.open_created_cube.as_ref(), Some(&id));
        assert_checked_list_opens(&mut home, cube);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An upload that comes back without the seed half is not a backup, and
    /// must not be recorded as one. The upload task refuses to write that
    /// evidence — but `has_seed` is read back off disk, so the gate itself has
    /// to hold the line for a record written by anything else.
    #[test]
    fn only_a_kit_that_holds_the_seed_counts_as_evidence() {
        let kitted = |has_seed| {
            let mut cube = CubeSettings::new("SeedOnly".to_string(), Network::Bitcoin);
            cube.creation_backup_required = true;
            cube.creation_recovery_kit = Some(creation_gate::CreationRecoveryKit {
                at: 1_700_000_000,
                cube_id: 5,
                has_seed,
            });
            cube
        };

        let cube = kitted(true);
        assert!(cube
            .creation_recovery_kit
            .as_ref()
            .is_some_and(|k| k.has_seed));
        assert_eq!(
            creation_gate::evaluate_for_cube(&cube, None),
            creation_gate::CreationGate::Satisfied
        );

        // A Kit that never got the seed restores nothing. Its mere presence
        // must not stand in for a backup.
        assert!(
            matches!(
                creation_gate::evaluate_for_cube(&kitted(false), None),
                creation_gate::CreationGate::Blocked(_)
            ),
            "a seedless Kit satisfied the gate — the Cube it cannot restore is \
             free to be funded"
        );
    }

    // ── Passkey seed word count ──────────────────────────────────────────

    /// **The reason the two paths derive the same shape of phrase.** A passkey
    /// Cube's mnemonic is 12 words, so the app's own 12-word restore grid can
    /// take it back — it is a recovery artifact, not something we can only
    /// display.
    ///
    /// This drives the real restore path with a real passkey-derived phrase.
    #[test]
    fn a_passkey_cubes_phrase_goes_back_into_the_12_word_restore_grid() {
        let signer = MasterSigner::from_prf_output(Network::Bitcoin, &[0x5A; 32]).unwrap();
        let words: Vec<String> = signer.words().iter().map(|w| w.to_string()).collect();
        assert_eq!(
            words.len(),
            12,
            "a passkey Cube's phrase must fit the restore grid"
        );

        let dir = tmp_datadir("passkey-phrase-restore");
        let mut home = home_with_datadir(&dir);
        home.create_cube_name = coincube_ui::component::form::Value {
            value: "FromPasskeyPhrase".to_string(),
            ..Default::default()
        };
        type_pin(&mut home.create_cube_pin, "2468");
        type_pin(&mut home.create_cube_pin_confirm, "2468");

        // Typed word by word, exactly as the grid does — and the grid has
        // twelve slots, which is the whole point.
        for (i, word) in words.iter().enumerate() {
            let _ = home.update(Message::View(ViewMessage::RecoveryWordInput {
                index: i,
                word: word.clone(),
            }));
        }
        assert_eq!(
            home.recovery_words.to_vec(),
            words,
            "every word of a passkey phrase must fit the grid"
        );

        let _ = home.update(Message::View(ViewMessage::SubmitRecovery));
        assert!(
            home.creating_cube,
            "the passkey Cube's own phrase was rejected by the restore screen"
        );
        assert!(home.error.is_none(), "restore reported: {:?}", home.error);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Both creation paths stage a phrase of the same shape, so one backup
    /// wizard can serve both.
    #[test]
    fn both_creation_paths_stage_a_twelve_word_phrase() {
        let dir = tmp_datadir("phrase-parity");

        // Passkey: staged by the ceremony transition.
        let mut home = home_with_datadir(&dir);
        passkey_create_form(&mut home, "Passkey");
        let _ = drain(webview_registration(&mut home, "cred", 23));
        let passkey_words = shown_words(&home).len();

        // PIN: staged by `generate`.
        let pin_words = MasterSigner::generate(Network::Bitcoin)
            .unwrap()
            .words()
            .len();

        assert_eq!(passkey_words, 12);
        assert_eq!(pin_words, 12);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod chain_identity_open_tests {
    //! Opening a Cube from the launcher, driven from the real datadir probe
    //! through the real click handler (BTCB2 plan PR 2, review P2 on
    //! coincube#370).
    //!
    //! A settings.json record is not trusted about where it lives: the chain
    //! whose directory the list was read from travels with the list
    //! (`State::Cubes::source`), and a click checks the record's own `network`
    //! against it — and this build's ability to run that chain — *before*
    //! reading any config the record would point at. No `expect`, no panic.

    use super::*;
    use crate::chain::{ChainId, BTCB2_DORMANT_REASON};

    fn tmp_datadir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "coincube-open-gate-{}-{}-{}",
            tag,
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Run a `Task` to completion and collect its messages (same as the main
    /// test module's helper).
    fn drain(task: Task<Message>) -> Vec<Message> {
        use iced_runtime::futures::futures::StreamExt;
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Vec::new();
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_runtime::Action::Output(msg) => Some(msg),
                        _ => None,
                    }
                })
                .collect::<Vec<_>>()
                .await
        })
    }

    fn record(id: &str, chain: ChainId) -> CubeSettings {
        let mut c = CubeSettings::new_with_raw_id(id.to_string(), id.to_string(), chain);
        c.created_at = 0; // predates the creation-backup gate
        c
    }

    /// A datadir whose `bitcoin/` directory holds a real GUI config and a
    /// settings.json with the given records — exactly what a copied or edited
    /// file looks like on disk.
    fn bitcoin_datadir_with(records: Vec<CubeSettings>) -> std::path::PathBuf {
        let dir = tmp_datadir("records");
        let bitcoin = dir.join("bitcoin");
        std::fs::create_dir_all(&bitcoin).unwrap();
        app::Config::new(false)
            .to_file(&bitcoin.join(app::config::DEFAULT_FILE_NAME))
            .unwrap();
        let settings = settings::Settings {
            cubes: records,
            ..Default::default()
        };
        std::fs::write(
            bitcoin.join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec_pretty(&settings).unwrap(),
        )
        .unwrap();
        dir
    }

    /// `Home` on the Bitcoin network, its list loaded through the real probe
    /// (`reload` → `check_network_datadir` → `Message::Checked`).
    fn loaded_home(dir: &std::path::Path) -> Home {
        let mut home = Home::new(
            CoincubeDirectory::new(dir.to_path_buf()),
            Some(Network::Bitcoin),
        )
        .0;
        assert_eq!(home.network, ChainId::Bitcoin);
        let msgs = drain(home.reload());
        assert_eq!(msgs.len(), 1, "one probe result expected");
        for m in msgs {
            let _ = home.update(m);
        }
        home
    }

    #[test]
    fn connect_cube_recheck_refreshes_startup_snapshot_without_network_switch() {
        let local = record("local", ChainId::Bitcoin);
        let dir = bitcoin_datadir_with(vec![local.clone()]);
        let mut home = loaded_home(&dir);
        let mut connected = record("connected", ChainId::Bitcoin);
        connected.remote_synced = true;
        std::fs::write(
            dir.join("bitcoin").join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&settings::Settings {
                cubes: vec![local, connected],
                ..Default::default()
            })
            .unwrap(),
        )
        .unwrap();

        let messages = drain(home.update(Message::View(ViewMessage::Check)));
        assert_eq!(messages.len(), 1, "recheck must run the directory probe");
        for message in messages {
            let _ = home.update(message);
        }
        assert_eq!(home.network, ChainId::Bitcoin);
        let State::Cubes { cubes, .. } = &home.state else {
            panic!("expected the Cube list");
        };
        assert_eq!(cubes.len(), 2);
        assert!(cubes.iter().any(|cube| cube.id == "connected"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn connect_startup_completion_refreshes_list_and_preserves_creation_form() {
        use crate::app::view::ConnectAccountMessage;
        use crate::services::coincube::{DuressCheckOutcome, DuressState};

        for editing in [false, true] {
            let mut connected = record("connected", ChainId::Bitcoin);
            connected.remote_synced = true;
            let dir = bitcoin_datadir_with(vec![connected]);
            let mut home = loaded_home(&dir);
            // Model a list read before startup's settings reconciliation.
            home.state = State::Cubes {
                cubes: vec![],
                create_cube: editing,
                source: ChainId::Bitcoin,
            };
            home.create_cube_name.value = "In progress".into();
            let generation = home.connect_account.session_generation();
            // No client/user is installed: exercise the dashboard transition
            // without making network requests or accessing account storage.
            let messages = drain(home.update(Message::View(ViewMessage::ConnectAccount(
                ConnectAccountMessage::DuressStateChecked(
                    DuressCheckOutcome::Ok(DuressState {
                        active: false,
                        unlock_at: None,
                        enrolled: false,
                        this_device_registered: false,
                    }),
                    generation,
                    0,
                ),
            ))));
            let refreshes: Vec<_> = messages
                .into_iter()
                .filter(|message| matches!(message, Message::FocusedCubes { .. }))
                .collect();
            assert_eq!(refreshes.len(), usize::from(!editing));
            for message in refreshes {
                let _ = home.update(message);
            }
            let State::Cubes {
                cubes, create_cube, ..
            } = &home.state
            else {
                panic!("expected Cube state");
            };
            assert_eq!(*create_cube, editing);
            assert_eq!(cubes.len(), usize::from(!editing));
            assert_eq!(home.create_cube_name.value, "In progress");
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    /// Click record `index`: the messages the click emitted, and the home
    /// error afterwards. A panic here is the P2 failure mode.
    fn click(home: &mut Home, index: usize) -> (Vec<Message>, Option<String>) {
        let msgs = drain(home.update(Message::View(ViewMessage::Run(index))));
        (msgs, home.error().map(|e| e.to_string()))
    }

    fn is_run_for(msg: &Message, chain: ChainId, id: &str) -> bool {
        matches!(msg, Message::Run(_, _, c, cube) if *c == chain && cube.id == id)
    }

    #[test]
    fn anonymous_feature_refresh_recovers_and_discards_stale_denial() {
        let _guard = crate::app::session::test_guard();
        let root = CoincubeDirectory::new(tmp_datadir("anonymous-refresh"));
        GlobalSettings::update_bitcoin_blake2b_beta(&GlobalSettings::path(&root), true).unwrap();
        let (mut home, task) = Home::new_for_chain(root.clone(), None);
        drop(task);
        drop(home.update(Message::GlobalFeatureLoaded(0, false)));
        assert!(!home
            .chain_availability(ChainId::BitcoinBlake2b)
            .is_available());
        drop(home.update(Message::View(ViewMessage::GoToSection(
            HomeSection::Settings,
        ))));
        let request = home.global_feature_request;
        assert_eq!(request, 1);
        drop(home.update(Message::GlobalFeatureLoaded(request, true)));
        drop(home.update(Message::GlobalFeatureLoaded(0, false)));
        assert!(home
            .chain_availability(ChainId::BitcoinBlake2b)
            .is_available());
        assert!(home.connect_account.features.is_none());
        assert!(!home.connect_account.is_authenticated());
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn anonymous_global_flag_offers_local_creation_without_connect_entitlements() {
        let _guard = crate::app::session::test_guard();
        let root = CoincubeDirectory::new(tmp_datadir("anonymous-local"));
        let (mut home, task) = Home::new_for_chain(root.clone(), None);
        drop(task);
        home.connect_account.bitcoin_blake2b_opt_in = true;
        home.global_blake2b_enabled = true;
        home.refresh_displayed_networks();
        assert!(home.displayed_networks.contains(&ChainId::BitcoinBlake2b));
        assert!(home.connect_account.features.is_none());
        assert!(!home.connect_account.is_authenticated());
        assert!(!home
            .connect_chain_availability(ChainId::BitcoinBlake2b)
            .is_available());
        home.network = ChainId::BitcoinBlake2b;
        home.state = State::Cubes {
            cubes: vec![],
            create_cube: false,
            source: ChainId::BitcoinBlake2b,
        };
        // Signed out, the Create Cube journey opens as on Bitcoin; the
        // Vault comes later, from inside the Cube.
        let produced = drain(home.update(Message::View(ViewMessage::ShowCreateCube(true))));
        assert!(!produced
            .iter()
            .any(|message| matches!(message, Message::Install(..))));
        assert!(home.error.is_none());
        assert!(home.showing_create_form());
        home.connect_account.bitcoin_blake2b_opt_in = false;
        home.refresh_displayed_networks();
        assert!(!home.displayed_networks.contains(&ChainId::BitcoinBlake2b));
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn global_beta_requires_both_switches_and_blocks_direct_entry() {
        let dir = tmp_datadir("global-beta-gate");
        let root = CoincubeDirectory::new(dir.clone());
        let mut home = Home::new(root.clone(), Some(Network::Bitcoin)).0;
        let cube = CubeSettings::new("Source".into(), Network::Bitcoin).with_vault(
            settings::VaultIdentity {
                wallet_id: settings::WalletId::new("source".into(), Some(1)),
                fingerprint: None,
            },
        );
        home.state = State::Cubes {
            cubes: vec![cube],
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        super::tests::enable_btcb2(&mut home);
        for server in [false, true] {
            home.connect_account.features = Some(
                serde_json::from_value(serde_json::json!({
                    "plans": [], "bitcoinBlake2bEnabled": server,
                }))
                .unwrap(),
            );
            for opted_in in [false, true] {
                home.connect_account.bitcoin_blake2b_opt_in = opted_in;
                home.refresh_displayed_networks();
                assert_eq!(
                    home.displayed_networks.contains(&ChainId::BitcoinBlake2b),
                    server && opted_in
                );
                if !(server && opted_in) {
                    assert!(drain(home.update(Message::View(ViewMessage::SelectNetwork(
                        ChainId::BitcoinBlake2b
                    ))))
                    .is_empty());
                    assert_eq!(home.network, ChainId::Bitcoin);
                }
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn global_settings_remains_open_and_refreshes_a_sibling_home() {
        let dir = tmp_datadir("global-beta-tabs");
        let root = CoincubeDirectory::new(dir.clone());
        let mut home = Home::new(root.clone(), Some(Network::Bitcoin)).0;
        let mut sibling = Home::new(root.clone(), Some(Network::Bitcoin)).0;
        super::tests::enable_btcb2(&mut home);
        super::tests::enable_btcb2(&mut sibling);
        drop(home.update(Message::View(ViewMessage::GoToSection(
            HomeSection::Settings,
        ))));
        drop(home.update(Message::View(ViewMessage::ToggleBitcoinBlake2bBeta(true))));
        drop(sibling.reload_global_settings());
        assert!(sibling
            .connect_chain_availability(ChainId::BitcoinBlake2b)
            .is_available());
        home.network = ChainId::BitcoinBlake2b;
        sibling.network = ChainId::BitcoinBlake2b;
        drop(home.update(Message::View(ViewMessage::ToggleBitcoinBlake2bBeta(false))));
        drop(sibling.reload_global_settings());
        assert_eq!(home.active_section, HomeSection::Settings);
        assert_eq!(home.network, ChainId::Bitcoin);
        assert_eq!(sibling.network, ChainId::Bitcoin);
        assert!(!sibling
            .displayed_networks
            .contains(&ChainId::BitcoinBlake2b));
        assert!(
            !Home::new(root, Some(Network::Bitcoin))
                .0
                .connect_account
                .bitcoin_blake2b_opt_in
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn launcher_motion_observation_updates_open_home_without_overwriting_choice() {
        use crate::app::settings::preferences::{
            LauncherStyle, MotionPreference, PreferenceChange,
        };

        let dir = tmp_datadir("launcher-system-motion");
        let root = CoincubeDirectory::new(dir.clone());
        let path = GlobalSettings::path(&root);
        GlobalSettings::save_preference(
            &path,
            PreferenceChange::LauncherStyle(LauncherStyle::Cubes),
        )
        .unwrap();
        let mut home = Home::new(root.clone(), Some(Network::Bitcoin)).0;
        home.set_system_reduces_motion(Some(false));
        assert_eq!(home.catalog.mode, launcher_catalog::LauncherMode::Cubes);
        home.set_system_reduces_motion(Some(true));
        assert_eq!(home.catalog.mode, launcher_catalog::LauncherMode::List);
        home.set_system_reduces_motion(Some(false));
        assert_eq!(home.catalog.mode, launcher_catalog::LauncherMode::Cubes);
        assert_eq!(
            GlobalSettings::load_preferences(&path)
                .unwrap()
                .launcher_style,
            LauncherStyle::Cubes
        );

        GlobalSettings::save_preference(&path, PreferenceChange::Motion(MotionPreference::Reduced))
            .unwrap();
        drop(home.reload_global_settings());
        assert_eq!(home.catalog.mode, launcher_catalog::LauncherMode::List);
        home.set_system_reduces_motion(Some(false));
        assert_eq!(home.catalog.mode, launcher_catalog::LauncherMode::List);

        GlobalSettings::save_preference(
            &path,
            PreferenceChange::Motion(MotionPreference::FollowSystem),
        )
        .unwrap();
        drop(home.reload_global_settings());
        assert_eq!(home.catalog.mode, launcher_catalog::LauncherMode::Cubes);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn global_settings_network_reset_scrubs_staged_creation_and_recovery() {
        let dir = tmp_datadir("global-settings-creation");
        let mut home = Home::new(CoincubeDirectory::new(dir.clone()), Some(Network::Signet)).0;
        home.network = ChainId::Signet;
        home.developer_mode = true;
        home.creation_backup_words = Some(zeroize::Zeroizing::new(vec!["synthetic".into()]));
        home.recovery_words[0] = "synthetic".into();
        home.state = State::CreationBackup(CreationBackupStep::Choice);
        home.creating_cube = true;
        home.create_network_menu = true;
        home.create_network_selection_pending = true;
        drop(home.reload_global_settings());
        assert_eq!(home.network, ChainId::Bitcoin);
        assert!(!home.create_network_menu);
        assert!(!home.create_network_selection_pending);
        assert!(matches!(home.state, State::Unchecked));
        drop(home.update(Message::Checked {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::NoCube { create_cube: false }),
        }));
        assert!(!home.showing_create_journey());
        assert!(!home.create_network_menu);
        assert!(home.creation_backup_words.is_none());
        assert!(home.recovery_words.iter().all(String::is_empty));
        assert!(!home.creating_cube);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_global_settings_write_preserves_the_toggle() {
        let dir = tmp_datadir("global-settings-write-error");
        let root = CoincubeDirectory::new(dir.clone());
        std::fs::write(GlobalSettings::path(&root), "invalid json").unwrap();
        let mut home = Home::new(root, Some(Network::Bitcoin)).0;
        drop(home.update(Message::View(ViewMessage::ToggleBitcoinBlake2bBeta(true))));
        assert!(!home.connect_account.bitcoin_blake2b_opt_in);
        assert!(home.settings_error.is_some());
        drop(home.update(Message::View(ViewMessage::ToggleDeveloperMode(true))));
        assert!(!home.developer_mode);
        assert!(home.settings_error.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn authenticated_flag_controls_fork_launcher_and_its_restore() {
        let dir = tmp_datadir("fork-feature-entry");
        let mut home = Home::new(CoincubeDirectory::new(dir.clone()), Some(Network::Bitcoin)).0;
        let mut client = CoincubeClient::new();
        client.set_token("synthetic-feature-token");
        home.connect_account.install_admitted_client(client);
        home.connect_account.bitcoin_blake2b_opt_in = true;
        for flag in [None, Some(false), Some(true)] {
            home.connect_account.features = Some(
                serde_json::from_value(serde_json::json!({
                    "plans": [], "bitcoinBlake2bEnabled": flag
                }))
                .unwrap(),
            );
            home.refresh_displayed_networks();
            assert_eq!(
                home.displayed_networks.contains(&ChainId::BitcoinBlake2b),
                flag == Some(true)
            );
            assert!(!home
                .displayed_networks
                .contains(&ChainId::BitcoinBlake2bTestnet4));
            let messages = drain(home.update(Message::View(ViewMessage::SelectNetwork(
                ChainId::BitcoinBlake2b,
            ))));
            if flag != Some(true) {
                assert!(messages.is_empty());
                assert_eq!(home.network, ChainId::Bitcoin);
            } else {
                for msg in messages {
                    let _ = home.update(msg);
                }
                assert_eq!(home.network, ChainId::BitcoinBlake2b);
                assert!(!dir.join("bitcoin-blake2b").exists());
                // Creation is the Cube journey; the Vault installer's own
                // create and import routes stay closed.
                for event in [ViewMessage::CreateWallet, ViewMessage::ImportWallet] {
                    assert!(drain(home.update(Message::View(event))).is_empty());
                }
                // A Connect Cube restores from its Recovery Kit as on Bitcoin.
                let restore = drain(home.update(Message::View(
                    ViewMessage::RestoreFromRecoveryKit("synthetic".into()),
                )));
                assert!(matches!(
                    restore.as_slice(),
                    [Message::Install(
                        _,
                        ChainId::BitcoinBlake2b,
                        UserFlow::RestoreFromRecoveryKit { cube_uuid: Some(uuid) },
                        _,
                    )] if uuid == "synthetic"
                ));
                assert!(!dir.join("bitcoin-blake2b").exists());
                home.developer_mode = true;
                home.refresh_displayed_networks();
                assert!(home
                    .displayed_networks
                    .contains(&ChainId::BitcoinBlake2bTestnet4));
                let messages = drain(home.update(Message::View(ViewMessage::SelectNetwork(
                    ChainId::BitcoinBlake2bTestnet4,
                ))));
                for msg in messages {
                    let _ = home.update(msg);
                }
                assert_eq!(home.network, ChainId::BitcoinBlake2bTestnet4);
                assert!(!dir.join("bitcoin-blake2b-testnet4").exists());
            }
        }
        home.connect_account.features = None;
        assert!(drain(home.update(Message::View(ViewMessage::CreateWallet))).is_empty());
        assert!(home
            .connect_chain_availability(home.network)
            .reason()
            .is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dormant_home_refuses_creation_and_recovery_before_side_effects() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let dir = tmp_datadir("dormant-home");
            let mut home = Home::new_for_chain(CoincubeDirectory::new(dir.clone()), Some(chain)).0;
            assert_eq!(home.network, chain);
            assert!(!home.displayed_networks.contains(&chain));
            for event in [
                ViewMessage::CreateCube,
                ViewMessage::CreateWallet,
                ViewMessage::ImportWallet,
                ViewMessage::SubmitRecovery,
                ViewMessage::ShowCreateCube(true),
                ViewMessage::TogglePasskeyMode(true),
            ] {
                assert!(drain(home.update(Message::View(event))).is_empty());
                assert!(home.error().is_some());
                assert!(!home.creating_cube);
                assert!(home.pending_cube_id.is_none());
                assert!(home.creation_backup_words.is_none());
                assert!(!dir.join(chain.dir_name()).exists());
                assert!(!dir.join(chain.bitcoin_network().to_string()).exists());
            }
            let _ = home.view();
            home.developer_mode = false;
            let _ = home.update(Message::View(ViewMessage::SelectNetwork(ChainId::Bitcoin)));
            assert_eq!(home.network, ChainId::Bitcoin);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn a_direct_dormant_selection_cannot_change_the_active_chain() {
        let dir = tmp_datadir("dormant-selection");
        let mut home = Home::new(CoincubeDirectory::new(dir.clone()), Some(Network::Bitcoin)).0;
        home.developer_mode = true;
        assert!(drain(home.update(Message::View(ViewMessage::SelectNetwork(
            ChainId::BitcoinBlake2b
        ))))
        .is_empty());
        assert_eq!(home.network, ChainId::Bitcoin);
        assert!(home.error().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn home_installer_handoff_retains_selected_chain() {
        let dir = tmp_datadir("installer-chain");
        let mut home = Home::new(CoincubeDirectory::new(dir.clone()), Some(Network::Bitcoin)).0;
        home.developer_mode = true;
        home.network = ChainId::Testnet4;
        let messages = drain(home.update(Message::View(ViewMessage::CreateWallet)));
        assert!(matches!(
            messages.as_slice(),
            [Message::Install(
                _,
                ChainId::Testnet4,
                UserFlow::CreateWallet,
                _
            )]
        ));
        assert!(!dir.join("testnet4").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_probe_stamps_the_list_with_the_directory_it_was_read_from() {
        let dir = bitcoin_datadir_with(vec![record("main", ChainId::Bitcoin)]);
        let home = loaded_home(&dir);
        match &home.state {
            State::Cubes { cubes, source, .. } => {
                assert_eq!(*source, ChainId::Bitcoin);
                assert_eq!(cubes.len(), 1);
            }
            other => panic!("expected the Cube list, got {:?}", other),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_ordinary_bitcoin_cube_still_opens() {
        let dir = bitcoin_datadir_with(vec![record("main", ChainId::Bitcoin)]);
        let mut home = loaded_home(&dir);
        let (msgs, error) = click(&mut home, 0);
        assert_eq!(error, None);
        assert!(
            msgs.iter().any(|m| is_run_for(m, ChainId::Bitcoin, "main")),
            "expected a Run for the Bitcoin Cube"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reviewer's reproduction: records in `bitcoin/settings.json`
    /// claiming another chain. With the claimed chain's config absent this
    /// used to panic on `expect("Already checked")` before any refusal.
    #[test]
    fn a_record_claiming_another_chain_is_refused_not_panicked_when_its_config_is_absent() {
        for claimed in [
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
            ChainId::Signet,
        ] {
            let dir = bitcoin_datadir_with(vec![
                record("main", ChainId::Bitcoin),
                record("claimant", claimed),
            ]);
            assert!(!dir.join(claimed.dir_name()).exists());
            let mut home = loaded_home(&dir);
            // Both records are listed (the probe does not validate them).
            assert!(matches!(&home.state, State::Cubes { cubes, .. } if cubes.len() == 2));

            let (msgs, error) = click(&mut home, 1);
            assert!(
                msgs.is_empty(),
                "{:?}: a refused click must emit nothing",
                claimed
            );
            let error = error.unwrap_or_else(|| panic!("{:?}: no home error", claimed));
            assert!(error.contains(claimed.label()), "{}", error);
            assert!(error.contains(ChainId::Bitcoin.label()), "{}", error);
            // Refused for *where it was found*, before the dormant question.
            assert!(!error.contains(BTCB2_DORMANT_REASON));
            // Nothing was created for the claimed chain either.
            assert!(!dir.join(claimed.dir_name()).exists());

            // The ordinary Cube beside it is unaffected (the banner from the
            // refused click is dismissed first — Home keeps it until the next
            // action clears it, as for every other error).
            home.error = None;
            let (msgs, error) = click(&mut home, 0);
            assert_eq!(error, None);
            assert!(msgs.iter().any(|m| is_run_for(m, ChainId::Bitcoin, "main")));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// With the claimed chain's config *present* the record is still refused:
    /// the decision is about the directory the record came from, not about
    /// whether the file its claim points at happens to exist.
    #[test]
    fn a_record_claiming_another_chain_is_refused_even_when_its_config_exists() {
        for claimed in [ChainId::BitcoinBlake2b, ChainId::Signet] {
            let dir = bitcoin_datadir_with(vec![record("claimant", claimed)]);
            let other = dir.join(claimed.dir_name());
            std::fs::create_dir_all(&other).unwrap();
            app::Config::new(true)
                .to_file(&other.join(app::config::DEFAULT_FILE_NAME))
                .unwrap();
            let mut home = loaded_home(&dir);
            let (msgs, error) = click(&mut home, 0);
            assert!(msgs.is_empty(), "{:?}", claimed);
            let error = error.expect("home error");
            assert!(
                error.contains(claimed.label()) && error.contains("Bitcoin folder"),
                "{}",
                error
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A list genuinely read from a BTCB2 directory (unreachable through the
    /// launcher today, which is typed on `bitcoin::Network`, but the handler
    /// must not depend on that): the record agrees with its directory and is
    /// refused for the dormant runtime, without reading any config.
    #[test]
    fn a_dormant_chain_record_in_its_own_directory_is_refused_for_the_runtime() {
        let dir = tmp_datadir("dormant");
        let mut home = Home::new(CoincubeDirectory::new(dir.clone()), Some(Network::Bitcoin)).0;
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            home.state = State::Cubes {
                cubes: vec![record("fork", chain)],
                create_cube: false,
                source: chain,
            };
            home.error = None;
            let (msgs, error) = click(&mut home, 0);
            assert!(msgs.is_empty(), "{:?}", chain);
            assert_eq!(
                error.as_deref(),
                Some("Bitcoin Blake2b is unavailable. Enable its beta preference in Global Settings and verify feature availability."),
                "{:?}",
                chain
            );
            assert!(
                !dir.join(chain.dir_name()).exists(),
                "no config was read or created"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A probe result that arrives for a network the user has already
    /// switched away from must not replace the current list (its `source`
    /// would then disagree with `self.network`).
    #[test]
    fn a_stale_probe_result_for_another_network_is_dropped() {
        let dir = bitcoin_datadir_with(vec![record("main", ChainId::Bitcoin)]);
        let mut home = loaded_home(&dir);
        let before = format!("{:?}", home.state);

        let _ = home.update(Message::Checked {
            for_chain: ChainId::Signet,
            res: Ok(State::Cubes {
                cubes: vec![record("stale", ChainId::Signet)],
                create_cube: false,
                source: ChainId::Signet,
            }),
        });
        assert_eq!(
            format!("{:?}", home.state),
            before,
            "stale result replaced the list"
        );
        assert_eq!(home.error(), None);

        // A stale *error* is dropped too.
        let _ = home.update(Message::Checked {
            for_chain: ChainId::Signet,
            res: Err("boom".to_string()),
        });
        assert_eq!(home.error(), None);

        // …while a fresh result for the current network is applied.
        let _ = home.update(Message::Checked {
            for_chain: ChainId::Bitcoin,
            res: Ok(State::NoCube { create_cube: false }),
        });
        assert!(matches!(home.state, State::NoCube { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
