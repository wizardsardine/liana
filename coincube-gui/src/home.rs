use iced::{
    alignment::Horizontal,
    widget::{pick_list, scrollable, tooltip as iced_tooltip, Button, Space, Stack, Toggler},
    Alignment, Length, Subscription, Task,
};

use coincube_core::{bip39, miniscript::bitcoin::Network};
use coincube_ui::{
    component::{button, card, network_banner, notification, spinner, text::*},
    icon, image, theme,
    widget::{modal::Modal, CheckBox, Column, Container, Element, Row},
};
use coincubed::config::ConfigError;
use tokio::runtime::Handle;

use crate::app::state::settings::recovery_kit::encrypt_and_upload as recovery_kit_upload;
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
            AuthConfig, CubeConnectState, CubeSettings, WalletSettings,
        },
        state::{connect::ConnectAccountPanel, settings::general},
        view::{BackupWalletMessage, ConnectAccountMessage, RecoveryKitMessage},
    },
    delete::{delete_wallet, DeleteError},
    dir::{CoincubeDirectory, NetworkDirectory},
    installer::UserFlow,
    services::connect::{
        client::{auth::AuthClient, backend::api::UserRole, get_service_config},
        login::{connect_with_credentials, BackendState},
    },
};
use coincube_core::signer::{MasterSigner, MASTER_SEED_LABEL};

const NETWORKS: [Network; 5] = [
    Network::Bitcoin,
    Network::Testnet,
    Network::Testnet4,
    Network::Signet,
    Network::Regtest,
];

#[derive(Debug, Clone)]
pub enum State {
    Unchecked,
    Cubes {
        cubes: Vec<CubeSettings>,
        create_cube: bool,
    },
    NoCube,
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

    /// Choosing *how* to back up. The entry point for every Cube, whichever
    /// unlock method it uses.
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
/// Neither the PRF output nor the derived signer. The fingerprint is computed
/// at ceremony time and the seed material dropped immediately: a passkey Cube
/// stores no seed anyway (it re-derives from the credential on every open), so
/// holding it across a user-paced step would be secret-lifetime for nothing.
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
}

/// Which section is shown in the home's main content area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeSection {
    /// Cube list (default)
    Cubes,
    /// Connect account-level sub-page
    Connect(app::menu::ConnectSubMenu),
    /// Heir "Recover a Vault" discovery surface (COIN-377 / PR 1). Global —
    /// reachable even when the heir owns no Vault of their own.
    RecoverVault,
}

/// Context stashed for firing a remote cube update after local rename succeeds.
struct PendingRemoteRename {
    cube_id: String,
    cube_network: Network,
    new_name: String,
}

pub struct Home {
    state: State,
    displayed_networks: Vec<Network>,
    network: Network,
    pub datadir_path: CoincubeDirectory,
    error: Option<String>,
    delete_cube_modal: Option<DeleteCubeModal>,
    create_cube_name: coincube_ui::component::form::Value<String>,
    create_cube_pin: pin_input::PinInput,
    create_cube_pin_confirm: pin_input::PinInput,
    creating_cube: bool,
    /// UUID pre-generated on the first creation attempt and reused on retries
    /// so that each logical cube has a stable client-side identifier.
    pending_cube_id: Option<uuid::Uuid>,
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
    /// Connect account tier — controls how many Cubes can be created per network.
    account_tier: AccountTier,
    /// Account-level Connect panel (login, plan, security, etc.)
    pub connect_account: ConnectAccountPanel,
    /// Heir "Recover a Vault" discovery surface state (COIN-377 / PR 1).
    pub recover_vault: RecoverVaultPanel,
    /// Whether the Connect sidebar section is expanded
    pub connect_expanded: bool,
    /// Which section is currently displayed in the main content area
    pub active_section: HomeSection,
    /// Current theme mode (dark/light) — used for theme-aware rendering
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
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
    /// Rename cube modal: (cube index, new name input)
    rename_cube_modal: Option<(usize, String)>,
    /// Pending remote rename: stashed after local rename succeeds so the
    /// `CubeRenamed` handler can fire the API update.
    pending_remote_rename: Option<PendingRemoteRename>,
    /// Cubes that exist on the Connect server but not locally on this machine.
    remote_cubes: Vec<RemoteCube>,
    /// Modal for deleting a remote-only cube from the Connect server.
    delete_remote_cube_modal: Option<DeleteRemoteCubeModal>,
    /// Recovery-method picker, shown when a remote cube can be recovered by
    /// *both* a password Recovery Kit and a phone envelope.
    recovery_method_modal: Option<RecoveryMethodModal>,
    #[allow(dead_code)]
    welcome_quote: coincube_ui::component::quote_display::Quote,
    #[allow(dead_code)]
    welcome_image_handle: iced::widget::image::Handle,
}

impl Home {
    pub fn new(datadir_path: CoincubeDirectory, network: Option<Network>) -> (Self, Task<Message>) {
        let developer_mode =
            GlobalSettings::load_developer_mode(&GlobalSettings::path(&datadir_path));
        let selected_network = network.unwrap_or(
            NETWORKS
                .iter()
                .find(|net| has_existing_wallet(&datadir_path, **net))
                .cloned()
                .unwrap_or(Network::Bitcoin),
        );
        let network = if developer_mode {
            selected_network
        } else {
            Network::Bitcoin
        };
        let network_dir = datadir_path.network_directory(network);
        (
            Self {
                state: State::Unchecked,
                displayed_networks: NETWORKS.to_vec(),
                network,
                datadir_path: datadir_path.clone(),
                error: None,
                delete_cube_modal: None,
                create_cube_name: coincube_ui::component::form::Value::default(),
                create_cube_pin: pin_input::PinInput::new(),
                create_cube_pin_confirm: pin_input::PinInput::new(),
                creating_cube: false,
                pending_cube_id: None,
                recovery_words: Default::default(),
                recovery_active_index: None,
                creation_backup_words: None,
                creation_backup_cubes: Vec::new(),
                developer_mode,
                account_tier: GlobalSettings::load_account_tier(&GlobalSettings::path(
                    &datadir_path,
                )),
                connect_account: ConnectAccountPanel::new(),
                recover_vault: RecoverVaultPanel::new(),
                connect_expanded: false,
                active_section: HomeSection::Cubes,
                theme_mode: GlobalSettings::load_theme_mode(&GlobalSettings::path(&datadir_path)),
                // Recomputed whenever the Create Cube form opens — see
                // `Home::default_passkey_mode`. A freshly constructed `Home`
                // has no Connect session, so this is PIN.
                passkey_mode: false,
                pending_passkey_cube: None,
                creation_kit_password: zeroize::Zeroizing::new(String::new()),
                creation_kit_confirm: zeroize::Zeroizing::new(String::new()),
                creation_kit_acknowledged: false,
                creation_kit_error: None,
                passkey_ceremony: None,
                #[cfg(target_os = "macos")]
                native_passkey_ceremony: None,
                has_stored_session: false, // Will be checked when cube UUID is set
                server_cube_limit: None,
                rename_cube_modal: None,
                pending_remote_rename: None,
                remote_cubes: Vec::new(),
                delete_remote_cube_modal: None,
                recovery_method_modal: None,
                welcome_quote: coincube_ui::component::quote_display::random_quote("first-launch"),
                welcome_image_handle:
                    coincube_ui::component::quote_display::image_handle_for_context("first-launch"),
            },
            // Kick a Connect session check alongside the datadir probe
            // so the sidebar reflects authenticated state immediately
            // when a session is already in the keyring — without
            // waiting for the user to navigate to the Connect section.
            Task::batch([
                Task::perform(check_network_datadir(network_dir), Message::Checked),
                Task::done(Message::View(ViewMessage::ConnectAccount(
                    ConnectAccountMessage::Init,
                ))),
            ]),
        )
    }

    pub fn reload(&self) -> Task<Message> {
        Task::perform(
            check_network_datadir(self.datadir_path.network_directory(self.network)),
            Message::Checked,
        )
    }

    /// Returns the effective per-network cube limit, preferring the
    /// server-authoritative value when available.
    fn cube_limit(&self) -> usize {
        self.server_cube_limit
            .unwrap_or_else(|| self.account_tier.cube_limit())
    }

    /// Total cube count (local + remote) for the current network.
    /// The server limit applies across all devices, so remote-only cubes
    /// must be included when checking the limit.
    fn total_cube_count(&self) -> usize {
        let local_count = if let State::Cubes { cubes, .. } = &self.state {
            cubes.len()
        } else {
            0
        };
        let network_str = settings::network_to_api_string(self.network);
        let remote_count = self
            .remote_cubes
            .iter()
            .filter(|rc| rc.network == network_str)
            .count();
        local_count + remote_count
    }

    pub fn stop(&mut self) {}

    /// Set a top-level error message shown on the home screen.
    /// Used by outer state machines (e.g. `gui::tab`) to surface issues
    /// they detect while handling home-originated messages.
    pub fn set_error(&mut self, msg: impl Into<String>) {
        self.error = Some(msg.into());
    }

    pub fn subscription(&self) -> Subscription<Message> {
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

        Subscription::none()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
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
                if show {
                    // Opening the form picks the default afresh: whether the
                    // passkey path can demonstrate a backup depends on the
                    // Connect session, which can change between visits.
                    self.passkey_mode = self.default_passkey_mode();
                }
                if let State::Cubes { create_cube, .. } = &mut self.state {
                    *create_cube = show;
                    if !show {
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
                    }
                }
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
                self.passkey_mode = enabled;
                self.error = None;
                Task::none()
            }
            Message::View(ViewMessage::CreateCube) => {
                if self.creating_cube {
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
                    self.error = Some(format!(
                        "Cube limit reached ({}/{}) for the {} plan. \
                         Upgrade your Connect account to create more Cubes.",
                        cube_count,
                        limit,
                        self.account_tier.display_name(),
                    ));
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
                                self.error =
                                    Some(format!("Failed to start passkey ceremony: {}", e));
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
                    match MasterSigner::generate(self.network) {
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
                            self.error =
                                Some(format!("Failed to generate master seed signer: {}", e));
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
            Message::View(ViewMessage::CreationBackup(msg)) => self.update_creation_backup(msg),
            Message::View(ViewMessage::CreationBackupBypassRequested) => {
                // Not while a Kit upload is in flight: its result decides
                // whether a Cube gets written, and taking the acknowledgement
                // underneath it would race two finalizers.
                if matches!(
                    self.state,
                    State::CreationBackup(CreationBackupStep::KitUploading)
                ) {
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
                ) {
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
                {
                    return Task::none();
                }
                self.scrub_creation_kit();
                self.state = State::CreationBackup(CreationBackupStep::KitPassword);
                Task::none()
            }
            Message::View(ViewMessage::CreationKit(msg)) => self.update_creation_kit(msg),
            Message::View(ViewMessage::CreationKitUploaded(result)) => {
                match result {
                    Ok(evidence) => {
                        // The Kit is on Connect. Only now is the Cube written —
                        // and it is written with the evidence, so it opens.
                        self.scrub_creation_kit();
                        if self.creating_passkey_cube() {
                            self.finalize_passkey_cube_creation(false, None, Some(evidence))
                        } else {
                            self.finalize_cube_creation(false, None, Some(evidence))
                        }
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
                        // Clear any previous error state
                        self.error = None;
                        // Reset form fields
                        self.create_cube_name = coincube_ui::component::form::Value::default();
                        self.create_cube_pin = pin_input::PinInput::new();
                        self.create_cube_pin_confirm = pin_input::PinInput::new();
                        // The creation-time backup step's copy of the seed has
                        // done its job — scrub it. `reload()` below replaces
                        // `State::CreationBackup` with the Cube list, which
                        // also makes the list parked for a cancel stale.
                        self.scrub_creation_seed();
                        self.scrub_creation_kit();
                        // The passkey registration has been consumed by the
                        // write; holding it would let a second finalize insert
                        // a duplicate.
                        self.pending_passkey_cube = None;
                        self.creation_backup_cubes = Vec::new();
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
                            };
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
                            reload_task
                        }
                    }
                    Err(e) => {
                        // Retain pending_cube_id so a retry reuses the same UUID.
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
                        self.error = Some(format!("Failed to create Cube: {}", e));
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
            } => match result {
                Ok(resp) => {
                    log::info!(
                        "[LAUNCHER] Cube {} registered remotely (server ID: {})",
                        resp.uuid,
                        resp.id
                    );
                    let network_dir = self.datadir_path.network_directory(network);
                    Task::perform(
                        async move {
                            settings::mark_cube_synced(&network_dir, &cube_id)
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
                    Task::none()
                }
            },
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
            Message::RemoteCubesLoaded(result) => {
                match result {
                    Ok(remote_only) => {
                        self.remote_cubes = remote_only;
                    }
                    Err(e) => {
                        log::warn!("[LAUNCHER] Failed to fetch remote cubes: {e}");
                    }
                }
                Task::none()
            }
            Message::CubeRemoteUpdated {
                cube_id,
                network,
                result,
            } => match result {
                Ok(_) => {
                    log::info!("[LAUNCHER] Cube {} updated remotely", cube_id);
                    let network_dir = self.datadir_path.network_directory(network);
                    Task::perform(
                        async move {
                            settings::mark_cube_synced(&network_dir, &cube_id)
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
                Task::none()
            }
            Message::RemoteCubeDeleted(result) => {
                match result {
                    Ok(()) => {
                        log::info!("[LAUNCHER] Remote cube deleted");
                        if let Some(modal) = self.delete_remote_cube_modal.take() {
                            self.remote_cubes.retain(|rc| rc.uuid != modal.cube.uuid);
                            return self.reload();
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
                                // untouched.
                                has_vault: None,
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
                    });
                }
                Task::none()
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::CloseRemoteModal)) => {
                self.delete_remote_cube_modal = None;
                Task::none()
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::ConfirmRemoteDelete(
                uuid,
            ))) => {
                if let Some(modal) = &mut self.delete_remote_cube_modal {
                    modal.deleting = true;
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
                if !self.developer_mode {
                    tracing::debug!(
                        "Ignoring SelectNetwork action because developer mode is disabled"
                    );
                    return Task::none();
                }
                self.network = network;
                // Clear stale limit from previous network
                self.server_cube_limit = None;
                let network_dir = self.datadir_path.network_directory(self.network);
                let mut tasks: Vec<Task<Message>> = vec![Task::perform(
                    check_network_datadir(network_dir),
                    Message::Checked,
                )];
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
                let previous_developer_mode = self.developer_mode;
                self.developer_mode = enabled;
                let path = GlobalSettings::path(&self.datadir_path);
                if let Err(e) = GlobalSettings::update_developer_mode(&path, enabled) {
                    self.developer_mode = previous_developer_mode;
                    self.error = Some(format!("Failed to update developer mode: {}", e));
                } else {
                    self.error = None;
                }

                if !enabled && self.network != Network::Bitcoin {
                    self.network = Network::Bitcoin;
                    let network_dir = self.datadir_path.network_directory(self.network);
                    return Task::perform(check_network_datadir(network_dir), Message::Checked);
                }

                Task::none()
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

                // Close modal and reload cubes
                self.delete_cube_modal = None;
                let reload_task = self.reload();
                if let Some(delete_task) = delete_task {
                    Task::batch([reload_task, delete_task])
                } else {
                    reload_task
                }
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::CloseModal)) => {
                self.delete_cube_modal = None;
                if self.network == Network::Testnet
                    && !has_existing_wallet(&self.datadir_path, Network::Testnet)
                {
                    self.network = Network::Testnet4;
                }
                Task::none()
            }
            Message::Checked(res) => match res {
                Err(e) => {
                    self.error = Some(e.to_string());
                    Task::none()
                }
                Ok(state) => {
                    // Prune remote cubes that now exist locally
                    if let State::Cubes { cubes, .. } = &state {
                        let local_ids: std::collections::HashSet<&str> =
                            cubes.iter().map(|c| c.id.as_str()).collect();
                        self.remote_cubes
                            .retain(|rc| !local_ids.contains(rc.uuid.as_str()));
                    }
                    self.state = state;
                    Task::none()
                }
            },
            Message::View(ViewMessage::Run(index)) => {
                if let State::Cubes { cubes, .. } = &self.state {
                    if let Some(cube) = cubes.get(index) {
                        let datadir_path = self.datadir_path.clone();
                        let mut path = self
                            .datadir_path
                            .network_directory(cube.network)
                            .path()
                            .to_path_buf();
                        path.push(app::config::DEFAULT_FILE_NAME);
                        let cfg = app::Config::from_file(&path).expect("Already checked");
                        let network = cube.network;
                        let cube = cube.clone();
                        Task::perform(
                            async move { (datadir_path.clone(), cfg, network, cube) },
                            |m| Message::Run(m.0, m.1, m.2, m.3),
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
                        // Reuse the UUID that was pre-generated when the user
                        // first clicked "Create Cube" (recovery path).
                        let cube_id = *self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);

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
                                let master_signer = MasterSigner::from_mnemonic(network, mnemonic)
                                    .map_err(|e| {
                                        format!("Failed to restore from mnemonic: {}", e)
                                    })?;

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
                                    .store_encrypted(
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
                for word in &mut self.recovery_words {
                    word.clear();
                    word.shrink_to_fit();
                }
                self.recovery_active_index = None;
                self.create_cube_name = coincube_ui::component::form::Value::default();
                self.create_cube_pin = pin_input::PinInput::new();
                self.create_cube_pin_confirm = pin_input::PinInput::new();
                self.error = None;
                self.reload()
            }

            Message::View(ViewMessage::GoToSection(section)) => {
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
                // Update the account panel's active_sub when navigating to a Connect submenu
                if let HomeSection::Connect(ref sub) = section {
                    self.connect_account.active_sub = sub.clone();
                }
                self.active_section = section;
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
                        self.rename_cube_modal = Some((index, cube.name.clone()));
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
                let Some((index, ref new_name)) = self.rename_cube_modal else {
                    return Task::none();
                };
                let new_name = new_name.trim().to_string();
                if new_name.is_empty() {
                    return Task::none();
                }
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
                            ConnectAccountMessage::Error(format!(
                                "Couldn't finish enabling duress mode: {e}. Please try again."
                            )),
                        )))
                    }
                })
            }
            Message::View(ViewMessage::ConnectAccount(msg)) => {
                // The banner CTAs navigate the panel to Plan & Billing by
                // setting its `active_sub`, but the sidebar highlight follows
                // the host's `active_section`. Mirror the move here so the two
                // stay in sync (otherwise the rail keeps highlighting Overview
                // while the pane shows the picker/checkout, and re-selecting
                // Overview would silently reset `active_sub`).
                if matches!(
                    msg,
                    ConnectAccountMessage::OpenPlanBilling
                        | ConnectAccountMessage::RenewCurrentPlan
                ) {
                    self.active_section =
                        HomeSection::Connect(app::menu::ConnectSubMenu::PlanBilling);
                }
                let was_authenticated = self.connect_account.is_authenticated();
                let task = map_connect_task(self.connect_account.update_message(msg));
                let now_authenticated = self.connect_account.is_authenticated();
                // Update cached keyring state on login/logout transitions
                if was_authenticated != now_authenticated {
                    self.has_stored_session = now_authenticated;
                    if !now_authenticated {
                        self.server_cube_limit = None;
                        self.remote_cubes.clear();
                        // Reset the heir discovery surface back to Idle so the
                        // next sign-in re-fetches: otherwise its `Loaded` state
                        // (and `is_loaded()` re-fetch guard) would persist and a
                        // later account would see the prior account's vault rows.
                        self.recover_vault = RecoverVaultPanel::new();
                        // Drop any open recovery-method picker so it can't
                        // reference a prior account's remote cube.
                        self.recovery_method_modal = None;
                    }
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
                    let mut tasks = vec![task];

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
                        let mut unsynced: Vec<CubeSettings> = Vec::new();
                        for net in &NETWORKS {
                            let nd = datadir.network_directory(*net);
                            if let Ok(s) = settings::Settings::from_file(&nd) {
                                for cube in s.cubes {
                                    if !cube.remote_synced {
                                        unsynced.push(cube);
                                    }
                                }
                            }
                        }
                        if !unsynced.is_empty() {
                            tasks.push(Task::perform(
                                async move {
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
                                            return;
                                        }
                                    };

                                    for cube in &unsynced {
                                        let server_match =
                                            server_cubes.iter().find(|sc| sc.uuid == cube.id);
                                        let ok = match server_match {
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
                                                let name_drift = sc.name != cube.name;
                                                if name_drift || vault_report.is_some() {
                                                    let req = UpdateCubeRequest {
                                                        name: name_drift.then(|| cube.name.clone()),
                                                        status: None,
                                                        has_vault: vault_report,
                                                    };
                                                    client
                                                        .update_cube(&sc.id.to_string(), req)
                                                        .await
                                                        .is_ok()
                                                } else {
                                                    true
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
                                                };
                                                client.register_cube(req).await.is_ok()
                                            }
                                        };
                                        if ok {
                                            let nd = datadir.network_directory(cube.network);
                                            let _ = settings::mark_cube_synced(&nd, &cube.id).await;
                                        }
                                    }
                                },
                                |_| Message::View(ViewMessage::Check),
                            ));
                        }
                    }

                    // Fetch full server cube list and compare with local cubes
                    // to identify remote-only cubes. Both the API call and the
                    // local settings reads run off the UI thread.
                    if let Some(rc_client) = self.connect_account.authenticated_client() {
                        let datadir = self.datadir_path.clone();
                        tasks.push(Task::perform(
                            async move {
                                let server_cubes =
                                    rc_client.list_cubes().await.map_err(|e| e.to_string())?;

                                // Collect local cube UUIDs across all networks
                                let mut local_uuids = std::collections::HashSet::new();
                                for net in &NETWORKS {
                                    let nd = datadir.network_directory(*net);
                                    if let Ok(s) = settings::Settings::from_file(&nd) {
                                        for cube in &s.cubes {
                                            local_uuids.insert(cube.id.clone());
                                        }
                                    }
                                }

                                // Keep only server cubes with no local counterpart
                                let remote_source: Vec<CubeResponse> = server_cubes
                                    .into_iter()
                                    .filter(|sc| !local_uuids.contains(&sc.uuid))
                                    .collect();

                                // Determine restorability per cube. `has_recovery_kit`
                                // comes free from `list_cubes()`, but both the seed half
                                // (what a password restore needs) and the owner-keychain
                                // "phone" envelope summary live on `/recovery-kit/status`,
                                // so probe it for *every* remote cube — a phone-only cube
                                // has `has_recovery_kit == false` yet is still recoverable
                                // via its envelope, so gating the probe on the password
                                // kit would hide it. Probes run concurrently (`join_all`)
                                // so latency is one round-trip, not N. A flaky probe must
                                // not blank the whole list — on any error we log and treat
                                // every method as absent (the row still shows, just
                                // without a restore icon). `join_all` preserves input
                                // order.
                                //
                                // Duress note: phone availability is read from the
                                // `ownerSelf` *status* summary (presence/tier, no
                                // ciphertext); the duress gate fires later, at the actual
                                // recovery attempt (`OwnerKeychainRestoreStep` →
                                // `fetch_owner_recovery_envelope`), which returns neutral
                                // "unavailable, try later" copy on a 423 (invariant I3).
                                let client_ref = &rc_client;
                                let remote_only: Vec<RemoteCube> = iced::futures::future::join_all(
                                    remote_source.into_iter().map(|sc| async move {
                                        let (
                                            has_encrypted_seed,
                                            phone_recoverable,
                                            phone_full_cube,
                                        ) = match client_ref.get_recovery_kit_status(sc.id).await {
                                            Ok(status) => {
                                                let (phone_recoverable, phone_full_cube) =
                                                    derive_phone_recovery(
                                                        status.owner_self.as_ref(),
                                                    );
                                                (
                                                    status.has_encrypted_seed,
                                                    phone_recoverable,
                                                    phone_full_cube,
                                                )
                                            }
                                            // No kit/status yet → nothing recoverable.
                                            Err(CoincubeError::NotFound) => (false, false, false),
                                            Err(e) => {
                                                log::warn!(
                                                    "[LAUNCHER] Recovery Kit status probe \
                                                         failed for \"{}\": {}",
                                                    sc.name,
                                                    e
                                                );
                                                (false, false, false)
                                            }
                                        };
                                        RemoteCube {
                                            id: sc.id,
                                            uuid: sc.uuid,
                                            name: sc.name,
                                            network: sc.network,
                                            has_recovery_kit: sc.has_recovery_kit,
                                            has_encrypted_seed,
                                            phone_recoverable,
                                            phone_full_cube,
                                        }
                                    }),
                                )
                                .await;

                                Ok(remote_only)
                            },
                            Message::RemoteCubesLoaded,
                        ));
                    }

                    return Task::batch(tasks);
                }

                task
            }

            Message::View(ViewMessage::OpenUrl(url)) => {
                if let Err(e) = open::that_detached(&url) {
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
                self.state = State::CreationBackup(CreationBackupStep::Intro(false));
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
                    // Same evidence, either shape of Cube; only the write
                    // differs (a passkey Cube has no seed file or device
                    // secret to mint).
                    self.finalize_backed_up_cube()
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
        self.state = State::CreationBackup(CreationBackupStep::Choice);
    }

    /// Finish creation with the **written-phrase** exit, whichever unlock
    /// method the Cube uses.
    ///
    /// The two finalizers differ only in what they write — a PIN Cube gets a
    /// device secret and a sealed seed file, a passkey Cube gets neither — so
    /// the wizard that produced the evidence does not need to know which it is
    /// talking to.
    fn finalize_backed_up_cube(&mut self) -> Task<Message> {
        if self.creating_passkey_cube() {
            self.finalize_passkey_cube_creation(true, None, None)
        } else {
            self.finalize_cube_creation(true, None, None)
        }
    }

    /// Both passkey ceremonies land here — the native macOS one and the
    /// webview one — so there is exactly one answer to "what happens when a
    /// credential is registered", and it is testable without a real ceremony.
    ///
    /// It derives the Cube's fingerprint, drops the seed material, and hands
    /// over to the creation-backup step. **Nothing is written to disk**: that is
    /// [`Self::finalize_passkey_cube_creation`], reached only once the user has
    /// accepted the bypass acknowledgement (the sole exit a passkey Cube has
    /// today — see [`DEFAULT_PASSKEY_MODE`]).
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
            let master_signer = match MasterSigner::from_prf_output(self.network, prf_output) {
                Ok(signer) => signer,
                Err(e) => {
                    self.creating_cube = false;
                    self.error = Some(format!("Failed to derive master signer: {}", e));
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
        // The same entry point a PIN Cube gets. Which exits the choice screen
        // offers depends on the session, not on the unlock method.
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
                 Sign in and try again, or choose \"I'll do this later\"."
                    .to_string(),
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
                        // monotonic and flips later.
                        has_vault: None,
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

                Ok(creation_gate::CreationRecoveryKit {
                    at: created_at.timestamp(),
                    cube_id: registered,
                    has_seed: outcome.now_has_seed,
                })
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
                cube.backed_up = backed_up;
                cube.creation_backup_bypass = bypass;
                // The Kit was uploaded before this write, so the Cube is
                // registered with Connect by definition — recording that here
                // stops `CubeCreated` registering it a second time.
                if recovery_kit.is_some() {
                    cube.remote_synced = true;
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
        self.scrub_creation_seed();
        self.scrub_creation_kit();
        // A registered-but-unsaved passkey credential goes with it. The
        // credential itself stays in the platform authenticator — harmless,
        // since nothing references it — but this process must not still be
        // holding something it could turn into a Cube later.
        self.pending_passkey_cube = None;
        self.creating_cube = false;
        self.error = None;
        // Back to the create form the user came from. `pending_cube_id` is
        // deliberately kept: a retry reuses the same UUID, matching the
        // existing failure path.
        self.state = State::Cubes {
            cubes,
            create_cube: true,
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
                let master_signer = MasterSigner::from_mnemonic(network, mnemonic)
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
                    .store_encrypted(
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

    pub fn view(&self) -> Element<Message> {
        let content = Into::<Element<ViewMessage>>::into(scrollable(
            Column::new()
                // Developer mode controls — right-aligned at top
                .push(
                    Row::new()
                        .push(if let State::Cubes {
                            create_cube: true, ..
                        } = &self.state
                        {
                            Some(
                                button::secondary(Some(icon::previous_icon()), "Back to Cube list")
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
                            Row::new()
                                .spacing(10)
                                .align_y(Alignment::Center)
                                .push(text("Developer mode").style(theme::text::secondary))
                                .push(
                                    Toggler::new(self.developer_mode)
                                        .on_toggle(ViewMessage::ToggleDeveloperMode)
                                        .width(50)
                                        .style(theme::toggler::orange),
                                ),
                        )
                        .push(if self.developer_mode {
                            Some(
                                pick_list(
                                    self.displayed_networks.as_slice(),
                                    Some(self.network),
                                    ViewMessage::SelectNetwork,
                                )
                                .style(theme::pick_list::primary)
                                .padding(10),
                            )
                        } else {
                            None
                        })
                        .align_y(Alignment::Center)
                        .padding(iced::Padding::from([10, 0])),
                )
                .push(
                    Container::new(
                        Column::new()
                            .align_x(Alignment::Center)
                            .spacing(20)
                            // "Your Cubes" heading
                            .push(if matches!(self.state, State::Cubes { create_cube: false, .. }) {
                                Some(text("Your Cubes").size(24).bold())
                            } else {
                                None
                            })
                            .push({
                                // Only show error at top if not in create cube form
                                let in_create_form = matches!(
                                    self.state,
                                    State::Cubes {
                                        create_cube: true,
                                        ..
                                    } | State::NoCube
                                );
                                if !in_create_form {
                                    self.error.as_ref().map(|e| card::simple(text(e)))
                                } else {
                                    None
                                }
                            })
                            .push(match &self.state {
                                State::RecoveryInput => recovery_input_view(
                                    &self.recovery_words,
                                    self.recovery_active_index,
                                ),
                                State::CreationBackup(step) => creation_backup_view(
                                    step,
                                    self.creation_backup_words.as_ref().map(|w| w.as_slice()),
                                    self.creating_cube,
                                    self.creating_passkey_cube(),
                                    &self.creation_kit_password,
                                    &self.creation_kit_confirm,
                                    self.creation_kit_acknowledged,
                                    self.creation_kit_error.as_deref(),
                                    self.can_create_recovery_kit(),
                                ),
                                State::Cubes { cubes, create_cube } => {
                                    if *create_cube {
                                        create_cube_form(
                                            &self.create_cube_name,
                                            &self.create_cube_pin,
                                            &self.create_cube_pin_confirm,
                                            &self.error,
                                            self.creating_cube,
                                            self.passkey_mode,
                                            self.connect_account.authenticated_client().is_some(),
                                        )
                                    } else {
                                        let current_net_str =
                                            settings::network_to_api_string(self.network);
                                        let signed_in =
                                            self.connect_account.authenticated_client().is_some();
                                        let mut col =
                                            cubes.iter().enumerate().fold(
                                                Column::new().spacing(20),
                                                |col, (i, cube)| {
                                                    col.push(cubes_list_item(cube, i, signed_in))
                                                },
                                            );
                                        // Show remote-only cubes (on server but not local)
                                        for rc in self
                                            .remote_cubes
                                            .iter()
                                            .filter(|rc| rc.network == current_net_str)
                                        {
                                            col = col.push(remote_cube_list_item(rc));
                                        }
                                        let total_count = self.total_cube_count();
                                        let at_limit = cubes.len() >= self.account_tier.cube_limit()
                                            && matches!(self.network, Network::Bitcoin);
                                        if at_limit {
                                            col = col.push(
                                                Column::new()
                                                    .spacing(8)
                                                    .push(
                                                        button::secondary(
                                                            Some(icon::plus_icon()),
                                                            "Create Cube",
                                                        )
                                                        .padding(10)
                                                        .width(Length::Fixed(500.0)),
                                                    )
                                                    .push(
                                                        Container::new(
                                                            p1_regular(format!(
                                                                "Cube limit reached ({}/{}) on the {} plan. \
                                                                 Upgrade your Connect account to create more.",
                                                                total_count,
                                                                self.cube_limit(),
                                                                self.account_tier.display_name(),
                                                            ))
                                                            .style(theme::text::secondary),
                                                        )
                                                        .max_width(500),
                                                    ),
                                            );
                                        } else {
                                            col = col.push(
                                                Column::new().push(
                                                    button::secondary(
                                                        Some(icon::plus_icon()),
                                                        "Create Cube",
                                                    )
                                                    .on_press(ViewMessage::ShowCreateCube(true))
                                                    .padding(10)
                                                    .width(Length::Fixed(500.0)),
                                                ),
                                            );
                                        }
                                        col.into()
                                    }
                                }
                                State::NoCube | State::Unchecked => {
                                    let current_net_str =
                                        settings::network_to_api_string(self.network);
                                    let remote_for_net: Vec<_> = self
                                        .remote_cubes
                                        .iter()
                                        .filter(|rc| rc.network == current_net_str)
                                        .collect();

                                    // Center the children: the create form below
                                    // is a `center_x(Fill)` container, so without
                                    // this the form spans/centers full width while
                                    // the shrink-width remote-cube rows default to
                                    // the left edge — leaving the list and form
                                    // misaligned.
                                    let mut col =
                                        Column::new().spacing(20).align_x(Alignment::Center);
                                    for rc in &remote_for_net {
                                        col = col.push(remote_cube_list_item(rc));
                                    }

                                    let total_count = self.total_cube_count();
                                    let at_limit = total_count >= self.cube_limit();
                                    if at_limit && !remote_for_net.is_empty() {
                                        col = col.push(
                                            Container::new(
                                                p1_regular(format!(
                                                    "Cube limit reached ({}/{}) on the {} plan. \
                                                     Upgrade your Connect account or delete a remote Cube to create one here.",
                                                    total_count,
                                                    self.cube_limit(),
                                                    self.account_tier.display_name(),
                                                ))
                                                .style(theme::text::secondary),
                                            )
                                            .max_width(500),
                                        );
                                    } else {
                                        col = col.push(create_cube_form(
                                            &self.create_cube_name,
                                            &self.create_cube_pin,
                                            &self.create_cube_pin_confirm,
                                            &self.error,
                                            self.creating_cube,
                                            self.passkey_mode,
                                            self.connect_account.authenticated_client().is_some(),
                                        ));
                                    }
                                    col.into()
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
        let main_content: Element<Message> = if let HomeSection::Connect(_) = &self.active_section {
            // Render Connect account panel view
            let connect_view: Element<ConnectAccountMessage> =
                crate::app::view::connect::connect_account_panel(&self.connect_account);
            connect_view.map(|msg| Message::View(ViewMessage::ConnectAccount(msg)))
        } else if matches!(self.active_section, HomeSection::RecoverVault) {
            // Heir "Recover a Vault" discovery surface (COIN-377 / PR 1).
            recover_vault::view(&self.recover_vault)
                .map(|msg| Message::View(ViewMessage::RecoverVault(msg)))
        } else {
            content
        };

        // Build the sidebar
        let sidebar = home_sidebar(self);

        // Wrap sidebar + content in a Row
        let layout: Element<Message> = Row::new()
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
            .into();

        let layout = if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), layout]).into()
        } else {
            layout
        };
        // If passkey ceremony webview is active, overlay it on top
        let layout = if let Some(ceremony) = &self.passkey_ceremony {
            if let Some(active) = &ceremony.active_webview {
                let cancel_btn = button::secondary(None, "Cancel")
                    .on_press(Message::CancelPasskeyCeremony)
                    .width(Length::Fixed(150.0));

                let webview_modal = Container::new(
                    Column::new()
                        .spacing(15)
                        .align_x(Alignment::Center)
                        .push(h4_bold("Passkey Registration"))
                        .push(
                            p1_regular("Complete the passkey setup in the window below.")
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
            let cancel_btn = button::secondary(None, "Cancel")
                .on_press(Message::CancelPasskeyCeremony)
                .width(Length::Fixed(150.0));

            let status_modal = Container::new(
                Column::new()
                    .spacing(20)
                    .align_x(Alignment::Center)
                    .push(h4_bold("Passkey Registration"))
                    .push(
                        p1_regular(
                            "Authenticate with Touch ID to create your passkey.\n\
                             Look for the system prompt.",
                        )
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
                                button::secondary(Some(icon::cross_icon()), "Cancel")
                                    .on_press(Message::View(ViewMessage::RenameCubeCancel))
                                    .width(Length::Fill),
                            )
                            .push(if name_input.trim().is_empty() {
                                button::primary(Some(icon::check_icon()), "Save")
                                    .width(Length::Fill)
                            } else {
                                button::primary(Some(icon::check_icon()), "Save")
                                    .on_press(Message::View(ViewMessage::RenameCubeConfirm))
                                    .width(Length::Fill)
                            }),
                    ),
            )
            .style(theme::card::simple);
            Modal::new(Container::new(layout).height(Length::Fill), modal_content)
                .on_blur(Some(Message::View(ViewMessage::RenameCubeCancel)))
                .into()
        } else {
            layout
        }
    }
}

fn home_sidebar<'a>(home: &'a Home) -> Element<'a, Message> {
    use coincube_ui::{color, component::button as btn, component::text as txt, icon as ic};

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
        let connect_button: Element<Message> = iced::widget::Button::new(
            Row::new()
                .spacing(10)
                .align_y(iced::alignment::Vertical::Center)
                .push(ic::connect_icon().style(coincube_ui::theme::text::secondary))
                .push(
                    coincube_ui::component::text::p1_regular("Connect")
                        .style(coincube_ui::theme::text::secondary),
                )
                .push(Space::new().width(Length::Fill))
                .push(connect_chevron.style(coincube_ui::theme::text::secondary))
                .padding(10),
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
                    .push(Space::new().width(Length::Fixed(20.0)))
                    .push(btn::menu_active(None, label).width(Length::Fill))
                    .width(Length::Fill)
            } else {
                Row::new()
                    .push(Space::new().width(Length::Fixed(20.0)))
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
    if is_authenticated && feature_flags::RECOVER_VAULT_ENABLED {
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

    // Bottom-pinned section: Sign In / email + theme toggle
    let mut bottom_col = Column::new().spacing(0).width(Length::Fill);

    if !is_authenticated {
        bottom_col = bottom_col.push(
            Container::new(
                btn::primary(None, "Sign In")
                    .on_press(msg(ViewMessage::GoToSection(HomeSection::Connect(
                        app::menu::ConnectSubMenu::Overview,
                    ))))
                    .width(Length::Fill),
            )
            .padding(10)
            .width(Length::Fill),
        );
    } else if let Some(user) = &home.connect_account.user {
        bottom_col = bottom_col.push(
            Container::new(
                txt::caption(&user.email)
                    .color(color::GREY_3)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                    .align_x(Alignment::Center),
            )
            .padding(10)
            .width(Length::Fill)
            .center_x(Length::Fill),
        );
    }

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
            .center_x(Length::Fill),
    );

    // Outer layout: scrollable menu fills, bottom section pinned
    Column::new()
        .push(scrollable(col).height(Length::Fill))
        .push(bottom_col)
        .height(Length::Fill)
        .into()
}

fn create_cube_form<'a>(
    cube_name: &coincube_ui::component::form::Value<String>,
    pin: &'a pin_input::PinInput,
    pin_confirm: &'a pin_input::PinInput,
    error: &'a Option<String>,
    creating_cube: bool,
    passkey_mode: bool,
    signed_in: bool,
) -> Element<'a, ViewMessage> {
    use coincube_ui::component::form;
    use std::time::Duration;

    let mut column = Column::new()
        .spacing(20)
        .align_x(Alignment::Center)
        .width(Length::Fixed(500.0))
        .push(h4_bold("Create a new Cube"))
        .push(
            p1_regular(
                "A Cube is your account which can contain a Spark wallet, a Liquid wallet, a Vault wallet and other features.",
            )
            .style(theme::text::secondary),
        );

    // When signed out, the cube is created and stored locally but is NOT
    // registered with the Connect server (see the `authenticated_client()`
    // gate in the CreateCube handler). It therefore won't appear on paired
    // mobile signers (the Keychain app) until the user signs in, which
    // triggers the catch-up sync. Surface that up front so the absence on
    // mobile isn't a surprise.
    if !signed_in {
        column = column.push(
            Container::new(
                Column::new()
                    .spacing(10)
                    .push(
                        Row::new()
                            .spacing(10)
                            .align_y(Alignment::Center)
                            .push(icon::cloud_slash_icon().style(theme::text::warning))
                            .push(
                                p2_regular(
                                    "You're signed out. This Cube will be saved on this device \
                                     only. Sign in to Connect to sync it to your other devices \
                                     and the Keychain mobile app.",
                                )
                                .style(theme::text::secondary),
                            ),
                    )
                    .push(
                        Row::new()
                            .align_y(Alignment::Center)
                            .push(Space::new().width(Length::Fill))
                            .push(
                                button::secondary(None, "Sign In")
                                    .on_press(ViewMessage::GoToSection(HomeSection::Connect(
                                        app::menu::ConnectSubMenu::Overview,
                                    )))
                                    .width(Length::Fixed(140.0)),
                            ),
                    ),
            )
            .padding(12)
            .width(Length::Fill)
            .style(theme::card::simple),
        );
    }

    // Unlock-method toggle — hidden entirely when the passkey feature is off,
    // and on every platform but macOS regardless. The Windows Hello and
    // security-key labels this used to carry offered a choice whose unlock path
    // does not exist yet; taking it would have produced a Cube the build could
    // not open. See `PASSKEY_CREATION_AVAILABLE`.
    //
    // Styled as a two-label A/B row to match the General Settings toggles
    // (BTC/Sats, Fiat/Bitcoin): left label = toggler off. Passkey sits on the
    // left because it is the default.
    if feature_flags::PASSKEY_CREATION_AVAILABLE {
        column = column.push(
            card::simple(
                Row::new()
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(text("Unlock method:").bold())
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
            form::Form::new("Cube Name", cube_name, ViewMessage::CubeNameEdited)
                .warning("Please enter a name")
                .size(20)
                .padding(10),
        )
        .width(Length::Fill),
    );

    // PIN or passkey info section
    column = column.push(Space::new().height(Length::Fixed(10.0)));

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
        // The recovery story, stated at the moment the choice is made.
        //
        // This is the positive claim that replaces the device-bound one. A
        // passkey Cube skips the creation-time written-phrase step entirely, so
        // without this sentence a user would leave creation with no idea what
        // their backup is — and the honest answer is not "your passkey", which
        // reaches a second Mac only if iCloud Keychain happens to be on
        // (**I11**).
        column = column.push(
            p1_regular(
                "Your backup is your Recovery Kit. Create one in Settings \u{2014} it \
                 carries this Cube's master seed, encrypted with a recovery password \
                 only you know, so you can restore the Cube even without your passkey.",
            )
            .style(theme::text::secondary),
        );
    } else {
        // PIN setup section
        column = column.push(Space::new().height(Length::Fixed(10.0)));

        let pin_label = p1_regular("Enter PIN:").style(theme::text::secondary);
        column = column.push(pin_label);
        column = column.push(pin.view().map(ViewMessage::PinInput));

        column = column.push(Space::new().height(Length::Fixed(20.0)));

        let pin_confirm_label = p1_regular("Confirm PIN:").style(theme::text::secondary);
        column = column.push(pin_confirm_label);
        column = column.push(pin_confirm.view().map(ViewMessage::PinConfirmInput));
    }

    column = column.push(Space::new().height(Length::Fixed(10.0)));

    // Show error above the button
    if let Some(err) = error {
        column = column.push(p1_regular(err).style(theme::text::error));
    }

    column = column.push(Space::new().height(Length::Fixed(10.0)));
    // Determine if button should be enabled
    let can_create = if passkey_mode {
        !creating_cube && cube_name.valid && !cube_name.value.trim().is_empty()
    } else {
        !creating_cube
            && cube_name.valid
            && !cube_name.value.trim().is_empty()
            && pin.is_complete()
            && pin_confirm.is_complete()
    };

    let submit_button = if creating_cube {
        iced::widget::button(
            Container::new(
                Row::new()
                    .spacing(5)
                    .align_y(Alignment::Center)
                    .push(text("Creating"))
                    .push(
                        Container::new(spinner::typing_text_carousel(
                            "...",
                            true,
                            Duration::from_millis(500),
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
        button::primary(None, "Create Cube")
            .width(Length::Fixed(200.0))
            .on_press_maybe(if can_create {
                Some(ViewMessage::CreateCube)
            } else {
                None
            })
    };

    column = column.push(submit_button);

    Container::new(column)
        .padding(20)
        .center_x(Length::Fill)
        .into()
}

fn cubes_list_item<'a>(
    cube: &'a CubeSettings,
    i: usize,
    signed_in: bool,
) -> Element<'a, ViewMessage> {
    // Single tri-state cube icon (Phase 1, duress mode): the Cube's
    // relationship to Connect — Sovereign (outline) → Registered (filled,
    // half-tone) → Backed up (filled, full colour) — so users can tell at a
    // glance whether a Cube has a recovery kit before they're told what a
    // duress wipe costs. The signed-in-but-not-yet-synced case keeps its
    // distinct "mid-sync" wording (catch-up sync registers it on reload).
    let (sync_icon, sync_hint): (Element<'a, ViewMessage>, &'static str) =
        match cube.connect_state() {
            CubeConnectState::BackedUp => (
                icon::cube_icon().style(theme::text::success).into(),
                "Backed up to Connect — recovery kit ready",
            ),
            CubeConnectState::Registered => (
                icon::cube_icon().style(theme::text::secondary).into(),
                "Registered to Connect — no recovery kit",
            ),
            CubeConnectState::Sovereign if signed_in => (
                icon::cube_outline_icon()
                    .style(theme::text::secondary)
                    .into(),
                "Not yet synced to Connect. It will sync automatically in a moment.",
            ),
            CubeConnectState::Sovereign => (
                icon::cube_outline_icon().style(theme::text::warning).into(),
                "Sovereign — local only",
            ),
        };
    let sync_indicator = iced_tooltip::Tooltip::new(
        sync_icon,
        Container::new(p2_regular(sync_hint))
            .padding(8)
            .max_width(260)
            .style(theme::card::simple),
        iced_tooltip::Position::Bottom,
    );

    Container::new(
        Row::new()
            .align_y(Alignment::Center)
            .spacing(20)
            .push(
                Container::new(
                    Button::new(
                        Column::new()
                            .push(
                                Row::new()
                                    .spacing(8)
                                    .align_y(Alignment::Center)
                                    .push(p1_bold(&cube.name))
                                    .push(sync_indicator),
                            )
                            .push(if let Some(vault_id) = &cube.vault_wallet_id {
                                Some(
                                    p1_regular(format!(
                                        "Vault: Coincube-{}",
                                        vault_id.descriptor_checksum
                                    ))
                                    .style(theme::text::secondary),
                                )
                            } else {
                                Some(
                                    p1_regular("No Vault configured").style(theme::text::secondary),
                                )
                            }),
                    )
                    .on_press(ViewMessage::Run(i))
                    .padding(15)
                    .style(theme::button::container_border)
                    .width(Length::Fixed(500.0)),
                )
                .style(theme::card::simple),
            )
            .push(
                Button::new(icon::pencil_icon())
                    .style(theme::button::secondary)
                    .padding(10)
                    .on_press(ViewMessage::RenameCube(i)),
            )
            .push(
                Button::new(icon::trash_icon())
                    .style(theme::button::secondary)
                    .padding(10)
                    .on_press(ViewMessage::DeleteCube(DeleteCubeMessage::ShowModal(i))),
            ),
    )
    .into()
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

fn remote_cube_list_item<'a>(cube: &'a RemoteCube) -> Element<'a, ViewMessage> {
    // A remote cube can be recovered from this screen two ways, and both
    // rebuild the *same* wallet — they differ only in which key unlocks the
    // backup:
    //   * Password Recovery Kit — available when the server holds a kit that
    //     carries the encrypted seed half (a full restore needs the seed).
    //   * Phone (owner-keychain) envelope — available when an envelope set was
    //     sealed to the owner's phone key. Gated by the kill-switch flag so it
    //     stays dark until the keychain-app + API endpoints ship.
    // When both exist the cloud button opens a picker; when one exists it
    // launches that flow directly; when neither exists the row shows only its
    // honest status and a delete button.
    let password_recoverable = cube.has_recovery_kit && cube.has_encrypted_seed;
    let phone_recoverable =
        cube.phone_recoverable && feature_flags::OWNER_KEYCHAIN_RECOVERY_ENABLED;

    // Render the cube-name area as a plain card, NOT a Button. Using a
    // Button without `.on_press` makes Iced render it in
    // `Status::Disabled` (alpha 0.2 on the text), which made the whole
    // row read as "disabled" to users and obscured the fact that the
    // cloud-download icon on the right is an active restore trigger.
    // The copy tells the user *what* they can restore with (and, when a
    // descriptor-only kit exists, *why* a full restore isn't possible here):
    let status_line = match (password_recoverable, phone_recoverable) {
        (true, true) => "Recovery Kit + Phone recovery available — click to restore",
        (true, false) => "Recovery Kit available — click the download icon to restore",
        (false, true) => "Phone recovery available — click the download icon to restore",
        (false, false) if cube.has_recovery_kit => {
            "Recovery Kit has no Master Seed — restore from the device where it was backed up"
        }
        (false, false) => "Registered in Connect (no Recovery Kit backed up)",
    };
    // Mute the whole remote row (title included) relative to local
    // cubes: these cubes have no data on this machine, so rendering
    // their name in the same bright white as a locally-available Cube
    // over-states their presence. `theme::text::secondary` on the bold
    // title greys it back a notch while keeping the weight contrast.
    let card = Container::new(
        Column::new()
            .spacing(4)
            .push(p1_bold(&cube.name).style(theme::text::secondary))
            .push(p1_regular(status_line).style(theme::text::secondary)),
    )
    .padding(15)
    .width(Length::Fixed(500.0))
    .style(theme::card::simple);

    // Entry point on the home: clicking cloud-arrow-down starts recovery.
    // The action depends on which methods are available:
    //   * both     → open the picker so the owner chooses password vs phone
    //   * password → `RestoreFromRecoveryKit { cube_uuid }` (the step preselects
    //                 the cube and skips the picker)
    //   * phone    → `RestoreWithPhone { cube_id, full_cube }` → the
    //                 owner-keychain restore flow (no password)
    // The icon shows only when at least one method is available, so the click
    // always has a valid target.
    let restore_action: Option<(ViewMessage, &str)> =
        match (password_recoverable, phone_recoverable) {
            (true, true) => Some((
                ViewMessage::ShowRecoveryMethodPicker(cube.uuid.clone()),
                "Recover this Cube — choose password or phone",
            )),
            (true, false) => Some((
                ViewMessage::RestoreFromRecoveryKit(cube.uuid.clone()),
                "Restore this Cube from its Recovery Kit",
            )),
            (false, true) => Some((
                ViewMessage::RestoreWithPhone {
                    cube_id: cube.id,
                    full_cube: cube.phone_full_cube,
                },
                "Recover this Cube with your phone",
            )),
            (false, false) => None,
        };
    let restore_button = restore_action.map(|(on_press, tip)| {
        iced_tooltip::Tooltip::new(
            Button::new(icon::cloud_arrow_down_icon())
                .style(theme::button::secondary)
                .padding(10)
                .on_press(on_press),
            Container::new(p1_regular(tip))
                .padding(8)
                .style(theme::card::simple),
            iced_tooltip::Position::Bottom,
        )
    });

    let mut row = Row::new().align_y(Alignment::Center).spacing(20).push(card);
    if let Some(restore_button) = restore_button {
        row = row.push(restore_button);
    }
    row = row.push(
        Button::new(icon::trash_icon())
            .style(theme::button::secondary)
            .padding(10)
            .on_press(ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(
                cube.uuid.clone(),
            ))),
    );

    Container::new(row).into()
}

/// The creation-time backup step.
///
/// The three wizard screens are the Settings ones, reused verbatim through
/// `app::view::settings::backup`'s message-generic views with
/// [`ViewMessage::CreationBackup`] as the mapper. What is added around them is
/// creation-specific and belongs here:
///
/// - [`creation_gate::NOT_A_BACKUP_COPY`], because the mistake this step
///   exists to prevent is a user believing the datadir folder is their backup.
/// - The "I'll do this later" escape and its acknowledgement screen. Settings
///   has no equivalent — there, declining just closes a panel.
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
) -> Element<'a, ViewMessage> {
    use crate::app::view::settings::backup;

    // "I'll do this later", offered on every screen of the step. The gate is
    // not armed yet (unit 3b), so this is currently the difference between a
    // Cube that records a bypass and one that does not — but it is written as
    // the real escape hatch it will be.
    let bypass_link = || {
        button::transparent(None, "I'll do this later")
            .on_press_maybe((!saving).then_some(ViewMessage::CreationBackupBypassRequested))
    };

    let with_bypass = |inner: Element<'a, ViewMessage>| -> Element<'a, ViewMessage> {
        Column::new()
            .align_x(Alignment::Center)
            .spacing(20)
            .push(inner)
            .push(bypass_link())
            .into()
    };

    match step {
        CreationBackupStep::Intro(checked) => Column::new()
            .align_x(Alignment::Center)
            .spacing(20)
            .push(backup::intro_view(*checked, ViewMessage::CreationBackup))
            .push(
                Container::new(
                    p1_regular(creation_gate::NOT_A_BACKUP_COPY).style(theme::text::secondary),
                )
                .max_width(500),
            )
            .push(bypass_link())
            .into(),
        CreationBackupStep::Phrase => match words {
            Some(words) => with_bypass(backup::recovery_phrase_view(
                words,
                ViewMessage::CreationBackup,
            )),
            // Only reachable if the seed was scrubbed underneath the view;
            // `update_creation_backup` turns the same condition into an error
            // and abandons. Render nothing rather than an empty word grid.
            None => Column::new().into(),
        },
        CreationBackupStep::Verification {
            word_indices,
            word_inputs,
            error,
        } => with_bypass(backup::verification_view(
            word_indices,
            word_inputs,
            error.as_deref(),
            saving,
            ViewMessage::CreationBackup,
        )),
        CreationBackupStep::Bypass { acknowledged } => {
            creation_backup_bypass_view(*acknowledged, is_passkey)
        }
        CreationBackupStep::Choice => creation_choice_view(saving, is_passkey, kit_available),
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
                .align_x(Alignment::Center)
                .spacing(20)
                .push(text("Saving your Recovery Kit…").size(24).bold())
                .push(
                    Container::new(
                        p1_regular(
                            "Encrypting this Cube's master seed with your recovery password \
                             and saving it to COINCUBE Connect. Your Cube is created once \
                             this finishes.",
                        )
                        .style(theme::text::secondary),
                    )
                    .max_width(500),
                ),
        )
        .center_x(Length::Fill)
        .into(),
    }
}

/// How do you want to back this Cube up? The entry point for every creation,
/// PIN and passkey alike.
///
/// Three exits, in the order of how much protection they give:
///
/// 1. **Write the phrase down** — verified on the spot, works with no account
///    and no network, and restores through the app's own 12-word grid. Offered
///    for both unlock methods, which is only possible because both derive a
///    12-word mnemonic.
/// 2. **Recovery Kit** — the master seed encrypted under a recovery password
///    and held by Connect. Needs an account, so it is hidden when signed out
///    rather than shown as a button that cannot work.
/// 3. **Later** — the recorded acknowledgement. Last, and named for what it
///    actually is.
fn creation_choice_view(
    saving: bool,
    is_passkey: bool,
    kit_available: bool,
) -> Element<'static, ViewMessage> {
    let mut options = Column::new().align_x(Alignment::Center).spacing(12).push(
        button::primary(None, "Write down my recovery phrase")
            .on_press_maybe(
                (!saving).then_some(ViewMessage::CreationBackup(BackupWalletMessage::NextStep)),
            )
            .width(Length::Fixed(340.0)),
    );

    if kit_available {
        options = options.push(
            button::secondary(None, "Save a Recovery Kit to Connect")
                .on_press_maybe((!saving).then_some(ViewMessage::CreationKitRequested))
                .width(Length::Fixed(340.0)),
        );
    }

    let mut content = Column::new()
        .align_x(Alignment::Center)
        .spacing(20)
        .push(text("Back up this Cube").size(24).bold())
        .push(
            Container::new(
                p1_regular(
                    "Your recovery phrase is twelve words that rebuild this Cube on any \
                     computer. Write them down and keep them somewhere safe — or save a \
                     Recovery Kit, which stores the same seed with COINCUBE Connect, \
                     encrypted with a recovery password only you know.",
                )
                .style(theme::text::secondary),
            )
            .max_width(560),
        )
        .push(
            Container::new(
                p1_regular(creation_gate::not_a_backup_copy(is_passkey))
                    .style(theme::text::secondary),
            )
            .max_width(560),
        );

    if !kit_available {
        content = content.push(
            Container::new(
                p2_regular(
                    "Sign in to COINCUBE Connect to save a Recovery Kit instead. \
                     Your written phrase works either way.",
                )
                .style(theme::text::secondary),
            )
            .max_width(560),
        );
    }

    Container::new(
        content
            .push(Space::new().height(Length::Fixed(10.0)))
            .push(options)
            .push(
                button::transparent(None, "I'll do this later").on_press_maybe(
                    (!saving).then_some(ViewMessage::CreationBackupBypassRequested),
                ),
            )
            .push(button::transparent(None, "Cancel").on_press(ViewMessage::CancelCreationBackup)),
    )
    .center_x(Length::Fill)
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
            .align_x(Alignment::Center)
            .spacing(20)
            .push(text("Create this Cube without a backup?").size(24).bold())
            .push(
                Container::new(
                    p1_regular(creation_gate::not_a_backup_copy(is_passkey))
                        .style(theme::text::secondary),
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
                        button::secondary(Some(icon::previous_icon()), "Back up now")
                            .on_press(ViewMessage::CreationBackupChoiceRequested)
                            .width(Length::Fixed(240.0)),
                    )
                    .push(
                        button::primary(None, "Create without a backup")
                            .on_press_maybe(
                                acknowledged.then_some(ViewMessage::CreationBackupBypassConfirmed),
                            )
                            .width(Length::Fixed(240.0)),
                    ),
            )
            .push(button::transparent(None, "Cancel").on_press(ViewMessage::CancelCreationBackup)),
    )
    .center_x(Length::Fill)
    .into()
}

fn recovery_input_view(
    recovery_words: &[String; 12],
    active_index: Option<usize>,
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
            button::secondary(None, "Cancel")
                .width(Length::Fixed(145.0))
                .on_press(ViewMessage::CancelRecovery),
        )
        .push(
            button::primary(None, "Recover Wallet")
                .width(Length::Fixed(145.0))
                .on_press_maybe(if all_filled {
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
                        p1_regular(
                            "Enter your 12-word recovery phrase to restore your Liquid wallet.",
                        )
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

fn has_existing_wallet(data_dir: &CoincubeDirectory, network: Network) -> bool {
    data_dir
        .path()
        .join(network.to_string())
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

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Message {
    View(ViewMessage),
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
    Install(CoincubeDirectory, Network, UserFlow, Option<CoincubeClient>),
    Checked(Result<State, String>),
    Run(
        CoincubeDirectory,
        app::config::Config,
        Network,
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
        network: Network,
        result: Result<CubeResponse, String>,
    },
    /// Result of fetching cube limits from the Connect API.
    CubeLimitsLoaded(Result<CubeLimitsResponse, String>),
    /// Result of updating a cube on the remote Connect API.
    CubeRemoteUpdated {
        cube_id: String,
        network: Network,
        result: Result<CubeResponse, String>,
    },
    /// Result of deleting a local cube's Connect backup.
    CubeBackupDeleted(Result<(), String>),
    /// Result of deleting a remote-only cube from the Connect API.
    RemoteCubeDeleted(Result<(), String>),
    /// Result of renaming a cube locally (settings file updated).
    CubeRenamed(Result<(), String>),
    /// Remote-only cubes (on server but not local) computed off the UI thread.
    RemoteCubesLoaded(Result<Vec<RemoteCube>, String>),
}

#[derive(Debug, Clone)]
pub enum ViewMessage {
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

    // ── Passkey creation: the Recovery Kit branch ────────────────────────
    /// "Create Recovery Kit" from [`CreationBackupStep::Choice`] — opens the
    /// recovery-password screen.
    CreationKitRequested,
    /// The Settings Kit flow's own message type. The creation flow renders that
    /// flow's password screen, and this is the `fn` mapper it is parameterised
    /// over, exactly as [`Self::CreationBackup`] is for the backup wizard.
    /// Variants belonging to Settings alone are inert here.
    CreationKit(RecoveryKitMessage),
    /// The Kit upload finished. `Ok` carries the evidence to persist on the
    /// Cube; `Err` returns the user to the password screen with the reason and
    /// **nothing written**.
    CreationKitUploaded(Result<creation_gate::CreationRecoveryKit, String>),
    ImportWallet,
    CreateWallet,
    /// W13 — launch the installer in "restore from Cube Recovery Kit"
    /// mode for a specific remote cube (payload is its `uuid`). The
    /// installer preselects the cube and skips the picker. This is the
    /// password-based recovery path.
    RestoreFromRecoveryKit(String),
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
    CubeNameEdited(String),
    CreateCube,
    PinInput(pin_input::Message),
    PinConfirmInput(pin_input::Message),
    SelectNetwork(Network),
    StartInstall(Network),
    Check,
    Run(usize),
    DeleteCube(DeleteCubeMessage),
    ToggleRecoveryCheckBox,
    ToggleDeveloperMode(bool),
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
    /// Heir "Recover a Vault" discovery-surface messages (COIN-377 / PR 1).
    RecoverVault(RecoverVaultMessage),
    /// Toggle light/dark theme
    ToggleTheme,
    /// Toggle passkey mode for Cube creation (no PIN when enabled).
    TogglePasskeyMode(bool),
    /// Open a URL in the default browser
    OpenUrl(String),
}

#[derive(Debug, Clone)]
pub enum DeleteCubeMessage {
    ShowModal(usize),
    ShowRemoteModal(String), // uuid of remote-only cube
    CloseModal,
    Confirm(String), // Cube ID
    DeleteLianaConnect(bool),
    DeleteConnectBackup(bool),
    Deleted,
    PinInput(pin_input::Message),
    // Remote-only cube deletion
    ConfirmRemoteDelete(String), // uuid
    CloseRemoteModal,
}

struct DeleteCubeModal {
    cube: CubeSettings,
    network_directory: NetworkDirectory,
    wallet_settings: Option<WalletSettings>,
    warning: Option<DeleteError>,
    deleted: bool,
    delete_liana_connect: bool,
    /// Whether to also delete the cube from the Connect API (frees a cube slot).
    delete_connect_backup: bool,
    /// Whether the user is authenticated and the cube is synced (backup exists).
    can_delete_backup: bool,
    user_role: Option<UserRole>,
    // `None` means we were not able to determine whether wallet uses internal bitcoind.
    internal_bitcoind: Option<bool>,
    /// Data root. The PIN is verified by decrypting this Cube's seed file, so
    /// the modal needs to reach `<root>/<network>/mnemonics/` — the
    /// `network_directory` above is already one level in.
    datadir_root: std::path::PathBuf,
    pin_input: pin_input::PinInput,
    pin_error: Option<String>,
}

/// Modal for deleting a remote-only cube (exists on server, not locally).
struct DeleteRemoteCubeModal {
    cube: RemoteCube,
    deleting: bool,
    error: Option<String>,
}

impl DeleteRemoteCubeModal {
    fn view(&self) -> Element<Message> {
        let mut col = Column::new()
            .spacing(10)
            .push(Container::new(
                h4_bold(format!("Delete Remote Cube \"{}\"", self.cube.name))
                    .style(theme::text::destructive)
                    .width(Length::Fill),
            ))
            .push(text(
                "This Cube exists on the Connect server but not on this device. \
                 Deleting it will permanently remove it and free a Cube slot.",
            ))
            .push(Row::new())
            .push(Row::new())
            .push(text("WARNING: This cannot be undone."))
            .push(
                p1_regular(
                    "If another device still has this Cube locally, \
                     it will re-sync to Connect the next time it opens, \
                     consuming a Cube slot again. To permanently free the slot, \
                     delete the Cube on all devices.",
                )
                .style(theme::text::secondary),
            );

        if let Some(err) = &self.error {
            col = col
                .push(notification::warning(err.to_string(), err.to_string()).width(Length::Fill));
        }

        let mut delete_btn = button::secondary(None, "Delete Remote Cube")
            .width(Length::Fixed(250.0))
            .style(theme::button::destructive);
        if !self.deleting {
            delete_btn = delete_btn.on_press(ViewMessage::DeleteCube(
                DeleteCubeMessage::ConfirmRemoteDelete(self.cube.uuid.clone()),
            ));
        }

        let mut cancel_btn = button::secondary(None, "Cancel").width(Length::Fixed(120.0));
        if !self.deleting {
            cancel_btn =
                cancel_btn.on_press(ViewMessage::DeleteCube(DeleteCubeMessage::CloseRemoteModal));
        }

        col = col.push(
            Container::new(if self.deleting {
                Row::new().spacing(10).push(text("Deleting..."))
            } else {
                Row::new().spacing(10).push(cancel_btn).push(delete_btn)
            })
            .align_x(Horizontal::Center)
            .width(Length::Fill),
        );

        Into::<Element<ViewMessage>>::into(card::simple(col).width(Length::Fixed(700.0)))
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
        let col = Column::new()
            .spacing(15)
            .padding(20)
            .width(Length::Fixed(460.0))
            .push(h4_bold(format!("Recover \"{}\"", self.cube.name)).width(Length::Fill))
            .push(
                p1_regular(
                    "This Cube has two backups. Choose how to recover it — \
                     both restore the same wallet.",
                )
                .style(theme::text::secondary),
            )
            .push(
                button::secondary(Some(icon::key_icon()), "Recover with password")
                    .width(Length::Fill)
                    .on_press(ViewMessage::RestoreFromRecoveryKit(self.cube.uuid.clone())),
            )
            .push(
                button::secondary(Some(icon::phone_icon()), "Recover with phone")
                    .width(Length::Fill)
                    .on_press(ViewMessage::RestoreWithPhone {
                        cube_id: self.cube.id,
                        full_cube: self.cube.phone_full_cube,
                    }),
            )
            .push(
                button::secondary(Some(icon::cross_icon()), "Cancel")
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
        let mut modal = Self {
            cube: cube.clone(),
            wallet_settings: wallet_settings.clone(),
            network_directory,
            datadir_root,
            warning: None,
            deleted: false,
            delete_liana_connect: false,
            delete_connect_backup: false,
            can_delete_backup,
            internal_bitcoind,
            user_role: None,
            pin_input: pin_input::PinInput::new(),
            pin_error: None,
        };
        if let Some(wallet) = &wallet_settings {
            if let Some(auth) = &wallet.remote_backend_auth {
                match Handle::current().block_on(check_membership(
                    cube.network,
                    &modal.network_directory,
                    auth,
                )) {
                    Err(e) => {
                        modal.warning = Some(e);
                    }
                    Ok(user_role) => {
                        modal.user_role = user_role;
                    }
                }
            }
        }
        modal
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
                // `block_on(delete_wallet(...))` immediately below — this modal
                // has always been synchronous. A confirm dialog that takes a
                // beat to answer is acceptable; a cheap verifier next to an
                // expensive one is not (I1).
                if self.cube.has_pin(&self.datadir_root) {
                    let pin = self.pin_input.value();
                    let loc =
                        crate::services::unlock::CubeLocation::new(&self.datadir_root, &self.cube);
                    match crate::services::unlock::unlock_blocking(&loc, &pin) {
                        Ok(crate::services::unlock::PinOutcome::Unlock(_)) => {}
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

                self.warning = None;

                // Delete vault if it exists
                if let Some(wallet_settings) = &self.wallet_settings {
                    if let Err(e) = Handle::current().block_on(delete_wallet(
                        self.cube.network,
                        &self.network_directory,
                        wallet_settings,
                        self.delete_liana_connect,
                    )) {
                        self.warning = Some(e);
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
            }
            Message::View(ViewMessage::DeleteCube(DeleteCubeMessage::DeleteLianaConnect(
                delete,
            ))) => {
                self.delete_liana_connect = delete;
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
        let can_delete = pin_ready && self.warning.is_none();
        let mut confirm_button = button::secondary(None, "Delete Cube")
            .width(Length::Fixed(200.0))
            .style(theme::button::destructive);
        if can_delete {
            confirm_button = confirm_button.on_press(ViewMessage::DeleteCube(
                DeleteCubeMessage::Confirm(self.cube.id.clone()),
            ));
        }

        // Determine what's being deleted
        let has_vault = self.wallet_settings.is_some();
        let has_remote_backend = self
            .wallet_settings
            .as_ref()
            .and_then(|w| w.remote_backend_auth.as_ref())
            .is_some();

        let help_text_1 = if has_vault {
            format!(
                "Are you sure you want to delete the Cube \"{}\" and {}?",
                self.cube.name,
                if has_remote_backend {
                    "its associated Vault configuration"
                } else {
                    "all its associated data (including Vault)"
                }
            )
        } else {
            format!(
                "Are you sure you want to delete the Cube \"{}\"?",
                self.cube.name
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
                h4_bold(format!("Delete Cube \"{}\"", self.cube.name))
                    .style(theme::text::destructive)
                    .width(Length::Fill),
            ))
            .push(Row::new().push(text(help_text_1)))
            .push(help_text_2.map(|t| Row::new().push(p1_regular(t).style(theme::text::secondary))))
            .push(Row::new())
            .push(Row::new().push(text(help_text_3)));

        // Option to also delete the Connect API backup (only when relevant)
        if self.can_delete_backup {
            col = col.push(Space::new().height(Length::Fixed(5.0))).push(
                CheckBox::new(self.delete_connect_backup)
                    .label("Also delete Connect backup (frees a Cube slot)")
                    .on_toggle(|checked| {
                        ViewMessage::DeleteCube(DeleteCubeMessage::DeleteConnectBackup(checked))
                    }),
            );
            if self.delete_connect_backup {
                col = col.push(
                    p1_regular(
                        "The Connect backup will be permanently deleted. \
                         This frees a Cube slot on your account.",
                    )
                    .style(theme::text::warning),
                );
            }
        }

        // PIN entry section
        if self.cube.has_pin(&self.datadir_root) {
            col = col
                .push(Space::new().height(Length::Fixed(5.0)))
                .push(p1_regular("Enter your PIN to confirm:").style(theme::text::secondary))
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
                    Container::new(p1_regular(err).style(theme::text::error))
                        .center_x(Length::Fill),
                );
            }
        }

        col = col
            .push(
                self.warning.as_ref().map(|w| {
                    notification::warning(w.to_string(), w.to_string()).width(Length::Fill)
                }),
            )
            .push(
                Container::new(if !self.deleted {
                    Row::new().push(confirm_button)
                } else {
                    Row::new()
                        .spacing(10)
                        .push(icon::square_check_icon().style(theme::text::success))
                        .push(text("Cube successfully deleted").style(theme::text::success))
                })
                .align_x(Horizontal::Center)
                .width(Length::Fill),
            );

        Into::<Element<ViewMessage>>::into(card::simple(col).width(Length::Fixed(700.0)))
            .map(Message::View)
    }
}

pub async fn check_membership(
    network: Network,
    network_dir: &NetworkDirectory,
    auth: &AuthConfig,
) -> Result<Option<UserRole>, DeleteError> {
    let service_config = get_service_config(network)
        .await
        .map_err(|e| DeleteError::Connect(e.to_string()))?;

    if let BackendState::WalletExists(client, _, _) = connect_with_credentials(
        AuthClient::new(
            service_config.auth_api_url,
            service_config.auth_api_public_key,
            auth.email.to_string(),
        ),
        auth.wallet_id.clone(),
        service_config.backend_api_url,
        None,
        network,
        network_dir,
    )
    .await
    .map_err(|e| DeleteError::Connect(e.to_string()))?
    {
        Ok(Some(
            client
                .user_wallet_membership()
                .await
                .map_err(|e| DeleteError::Connect(e.to_string()))?,
        ))
    } else {
        Ok(None)
    }
}

async fn check_network_datadir(path: NetworkDirectory) -> Result<State, String> {
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
            return Ok(State::NoCube);
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
                Ok(State::NoCube)
            } else {
                Ok(State::Cubes {
                    cubes: s.cubes,
                    create_cube: false,
                })
            }
        }
        Err(settings::SettingsError::NotFound) => Ok(State::NoCube),
        Err(e) => Err(format!("Failed to read settings: {}", e)),
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

    fn home() -> Home {
        Home::new(test_datadir(), Some(Network::Bitcoin)).0
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
        }
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
        assert_eq!(home.cube_limit(), 2);

        home.server_cube_limit = Some(5);
        assert_eq!(home.cube_limit(), 5);

        home.state = State::Cubes {
            cubes: vec![
                cube("local-a", "Local A", Network::Bitcoin),
                cube("local-b", "Local B", Network::Bitcoin),
            ],
            create_cube: false,
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

    #[test]
    fn create_cube_validation_reports_pin_and_limit_errors_before_side_effects() {
        let mut home = home();
        home.state = State::Cubes {
            cubes: Vec::new(),
            create_cube: true,
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
        assert!(home
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Cube limit reached (2/2)")));
        assert!(!home.creating_cube);
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
    fn loaded_remote_cubes_limits_and_checked_state_update_home_cache() {
        let mut home = home();

        let _ = home.update(Message::CubeLimitsLoaded(Ok(CubeLimitsResponse {
            network: "mainnet".to_string(),
            current_count: 1,
            max_allowed: 4,
        })));
        assert_eq!(home.server_cube_limit, Some(4));

        let remote = remote_cube("local-a", "Remote A", Network::Bitcoin);
        let _ = home.update(Message::RemoteCubesLoaded(Ok(vec![remote.clone()])));
        assert_eq!(home.remote_cubes.len(), 1);

        let _ = home.update(Message::RemoteCubesLoaded(Err("offline".to_string())));
        assert_eq!(home.remote_cubes.len(), 1);

        let _ = home.update(Message::Checked(Ok(State::Cubes {
            cubes: vec![cube("local-a", "Local A", Network::Bitcoin)],
            create_cube: false,
        })));

        assert!(home.remote_cubes.is_empty());
        assert!(matches!(home.state, State::Cubes { .. }));
    }

    #[test]
    fn rename_modal_edits_and_cancels_without_touching_disk() {
        let mut home = home();
        home.state = State::Cubes {
            cubes: vec![cube("local-a", "Local A", Network::Bitcoin)],
            create_cube: false,
        };

        let _ = home.update(Message::View(ViewMessage::RenameCube(0)));
        assert_eq!(home.rename_cube_modal, Some((0, "Local A".to_string())));

        let _ = home.update(Message::View(ViewMessage::RenameCubeNameEdited(
            "Renamed".to_string(),
        )));
        assert_eq!(home.rename_cube_modal, Some((0, "Renamed".to_string())));

        let _ = home.update(Message::View(ViewMessage::RenameCubeCancel));
        assert!(home.rename_cube_modal.is_none());
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

        home.state = State::NoCube;
        home.error = Some("create error".to_string());
        home.view();

        home.state = State::RecoveryInput;
        home.recovery_words[0] = "ab".to_string();
        home.recovery_active_index = Some(0);
        home.view();

        home.state = State::Cubes {
            cubes: vec![cube("local-a", "Local A", Network::Bitcoin)],
            create_cube: true,
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

    #[test]
    fn pure_home_view_helpers_build_for_local_remote_and_form_variants() {
        let mut local = cube("local-a", "Local A", Network::Bitcoin);
        let _ = cubes_list_item(&local, 0, false);

        local.remote_synced = true;
        let _ = cubes_list_item(&local, 1, true);

        local.recovery_kit_last_backed_up_descriptor_fingerprint = Some("hash".to_string());
        let _ = cubes_list_item(&local, 2, true);

        let mut remote = remote_cube("remote-a", "Remote A", Network::Bitcoin);
        let _ = remote_cube_list_item(&remote);
        remote.has_encrypted_seed = false;
        let _ = remote_cube_list_item(&remote);

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
        let _ = create_cube_form(&valid_name, &pin, &pin, &None, false, false, true);
        let _ = create_cube_form(
            &invalid_name,
            &pin_input::PinInput::new(),
            &pin_input::PinInput::new(),
            &Some("form error".to_string()),
            true,
            true,
            false,
        );

        let mut words: [String; 12] = Default::default();
        words[0] = "ab".to_string();
        let _ = recovery_input_view(&words, Some(0));
        words.fill("abandon".to_string());
        let _ = recovery_input_view(&words, None);
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

        let _ = delete_modal.update(Message::View(ViewMessage::DeleteCube(
            DeleteCubeMessage::DeleteLianaConnect(true),
        )));
        assert!(delete_modal.delete_liana_connect);

        let remote = remote_cube("remote-a", "Remote A", Network::Bitcoin);
        let _ = DeleteRemoteCubeModal {
            cube: remote.clone(),
            deleting: false,
            error: Some("server refused".to_string()),
        }
        .view();
        let _ = DeleteRemoteCubeModal {
            cube: remote.clone(),
            deleting: true,
            error: None,
        }
        .view();
        let _ = RecoveryMethodModal { cube: remote }.view();

        let mut home = signed_in_home();
        home.state = State::Cubes {
            cubes: vec![cube],
            create_cube: false,
        };
        home.rename_cube_modal = Some((0, "Renamed Cube".to_string()));
        home.view();
        home.rename_cube_modal = None;
        home.delete_remote_cube_modal = Some(DeleteRemoteCubeModal {
            cube: remote_cube("remote-b", "Remote B", Network::Bitcoin),
            deleting: false,
            error: None,
        });
        home.view();
        home.delete_remote_cube_modal = None;
        home.recovery_method_modal = Some(RecoveryMethodModal {
            cube: remote_cube("remote-c", "Remote C", Network::Bitcoin),
        });
        home.view();
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
        let signer = MasterSigner::generate(home.network).expect("seed generation");
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

    /// Walk intro → phrase → verification and answer the challenge correctly.
    /// Returns the task `VerifyPhrase` produced.
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
        home.update(backup(BackupWalletMessage::VerifyPhrase))
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
        let dir = tmp_datadir("e2e");
        let mut home = home_with_datadir(&dir);

        let task = start_creation(&mut home, "Fresh", "1234");
        assert!(
            drain(task).is_empty(),
            "clicking Create must not create anything on its own — the backup step comes first"
        );
        assert_eq!(
            step(&home),
            CreationBackupStep::Intro(false),
            "creation must land on the backup step, not on a finished Cube"
        );
        assert!(
            cubes_on_disk(&dir).is_empty(),
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

        let on_disk = cubes_on_disk(&dir);
        assert_eq!(
            on_disk.len(),
            1,
            "expected exactly one Cube, got {:?}",
            on_disk
        );
        let cube = &on_disk[0];
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
        match unlock::unlock_blocking(&loc, "1234") {
            Ok(unlock::PinOutcome::Unlock(signer)) => assert_eq!(
                signer.words(),
                words,
                "the phrase the user wrote down must be the phrase that got sealed"
            ),
            other => panic!("a freshly created Cube did not unlock: {:?}", other),
        }

        let _ = std::fs::remove_dir_all(&dir);
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
        let existing = cube("already-here", "First", home.network);
        home.state = State::Cubes {
            cubes: vec![existing.clone()],
            create_cube: true,
        };

        enter_backup_step(&mut home, "Second", "1111");
        assert!(matches!(home.state, State::CreationBackup(_)));

        let _ = home.update(Message::View(ViewMessage::CancelCreationBackup));

        let State::Cubes { cubes, create_cube } = &home.state else {
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
                            );
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

        let out = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok(
                kit_evidence(77),
            )))),
        );
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
        let out = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok(
                kit_evidence(42),
            )))),
        );
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

        let out = drain(
            home.update(Message::View(ViewMessage::CreationKitUploaded(Ok(
                kit_evidence(1),
            )))),
        );
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

    /// An upload that comes back without the seed half is not a backup, and
    /// must not be recorded as one. (The refusal lives in the upload task; this
    /// pins the evidence shape it is allowed to produce.)
    #[test]
    fn only_a_kit_that_holds_the_seed_counts_as_evidence() {
        let mut cube = CubeSettings::new("SeedOnly".to_string(), Network::Bitcoin);
        cube.creation_backup_required = true;
        cube.creation_recovery_kit = Some(creation_gate::CreationRecoveryKit {
            at: 1_700_000_000,
            cube_id: 5,
            has_seed: true,
        });
        assert!(cube
            .creation_recovery_kit
            .as_ref()
            .is_some_and(|k| k.has_seed));
        assert_eq!(
            creation_gate::evaluate_for_cube(&cube, None),
            creation_gate::CreationGate::Satisfied
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
