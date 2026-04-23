use iced::{
    alignment::Horizontal,
    widget::{pick_list, scrollable, Button, Space, Stack, Toggler},
    Alignment, Length, Subscription, Task,
};

use coincube_core::{bip39, miniscript::bitcoin::Network};
use coincube_ui::{
    color,
    component::{button, card, network_banner, notification, spinner, text::*},
    icon, image, theme,
    widget::{modal::Modal, CheckBox, Column, Container, Element, Row},
};
use coincubed::config::ConfigError;
use tokio::runtime::Handle;

use crate::feature_flags;
use crate::pin_input;
use crate::services::coincube::{
    CubeLimitsResponse, CubeResponse, RegisterCubeRequest, UpdateCubeRequest,
};
#[cfg(not(target_os = "macos"))]
use crate::services::passkey::CeremonyMode;
use crate::services::passkey::{self as passkey_svc, CeremonyOutcome, PasskeyCeremony};
use crate::{
    app::{
        self,
        settings::{
            self,
            global::{AccountTier, GlobalSettings},
            AuthConfig, CubeSettings, WalletSettings,
        },
        state::connect::ConnectAccountPanel,
        view::ConnectAccountMessage,
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
    pub uuid: String,
    pub name: String,
    pub network: String, // API string: "mainnet", "testnet", etc.
}

/// Which section is shown in the launcher's main content area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LauncherSection {
    /// Cube list (default)
    Cubes,
    /// Connect account-level sub-page
    Connect(app::menu::ConnectSubMenu),
}

/// Context stashed for firing a remote cube update after local rename succeeds.
struct PendingRemoteRename {
    cube_id: String,
    cube_network: Network,
    new_name: String,
}

pub struct Launcher {
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
    developer_mode: bool,
    /// Connect account tier — controls how many Cubes can be created per network.
    account_tier: AccountTier,
    /// Account-level Connect panel (login, plan, security, etc.)
    pub connect_account: ConnectAccountPanel,
    /// Whether the Connect sidebar section is expanded
    pub connect_expanded: bool,
    /// Which section is currently displayed in the main content area
    pub active_section: LauncherSection,
    /// Current theme mode (dark/light) — used for theme-aware rendering
    pub theme_mode: coincube_ui::theme::palette::ThemeMode,
    /// Whether the user has chosen to create a passkey-derived Cube (no PIN).
    passkey_mode: bool,
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
    #[allow(dead_code)]
    welcome_quote: coincube_ui::component::quote_display::Quote,
    #[allow(dead_code)]
    welcome_image_handle: iced::widget::image::Handle,
}

impl Launcher {
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
                developer_mode,
                account_tier: GlobalSettings::load_account_tier(&GlobalSettings::path(
                    &datadir_path,
                )),
                connect_account: ConnectAccountPanel::new(),
                connect_expanded: false,
                active_section: LauncherSection::Cubes,
                theme_mode: GlobalSettings::load_theme_mode(&GlobalSettings::path(&datadir_path)),
                // Default to the feature flag value. When the passkey feature
                // is disabled (the common case pre-launch), this is always
                // `false`, forcing the PIN flow.
                passkey_mode: feature_flags::PASSKEY_ENABLED,
                passkey_ceremony: None,
                #[cfg(target_os = "macos")]
                native_passkey_ceremony: None,
                has_stored_session: ConnectAccountPanel::has_stored_session(),
                server_cube_limit: None,
                rename_cube_modal: None,
                pending_remote_rename: None,
                remote_cubes: Vec::new(),
                delete_remote_cube_modal: None,
                welcome_quote: coincube_ui::component::quote_display::random_quote("first-launch"),
                welcome_image_handle:
                    coincube_ui::component::quote_display::image_handle_for_context("first-launch"),
            },
            Task::perform(check_network_datadir(network_dir), Message::Checked),
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

    /// Set a top-level error message shown on the launcher screen.
    /// Used by outer state machines (e.g. `gui::tab`) to surface issues
    /// they detect while handling launcher-originated messages.
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
                    Message::Install(d, n, UserFlow::AddWallet)
                })
            }
            Message::View(ViewMessage::CreateWallet) => {
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Task::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::CreateWallet)
                })
            }
            Message::View(ViewMessage::RestoreFromRecoveryKit) => {
                // W13 — same launch shape as CreateWallet; the
                // installer picks the Recovery-Kit step sequence off
                // the UserFlow.
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Task::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::RestoreFromRecoveryKit)
                })
            }
            Message::View(ViewMessage::ShareXpubs) => {
                if !self.developer_mode {
                    tracing::debug!(
                        "Ignoring ShareXpubs action because developer mode is disabled"
                    );
                    return Task::none();
                }
                let datadir_path = self.datadir_path.clone();
                let network = self.network;
                Task::perform(async move { (datadir_path, network) }, |(d, n)| {
                    Message::Install(d, n, UserFlow::ShareXpubs)
                })
            }
            Message::View(ViewMessage::ShowCreateCube(show)) => {
                if let State::Cubes { create_cube, .. } = &mut self.state {
                    *create_cube = show;
                    if !show {
                        self.create_cube_name = coincube_ui::component::form::Value::default();
                        self.create_cube_pin = pin_input::PinInput::new();
                        self.create_cube_pin_confirm = pin_input::PinInput::new();
                        // Reset to the feature flag default (false when disabled).
                        self.passkey_mode = feature_flags::PASSKEY_ENABLED;
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
                let passkey_mode = self.passkey_mode && feature_flags::PASSKEY_ENABLED;

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
                let network = self.network;
                let cube_name = self.create_cube_name.value.trim().to_string();
                let pin = if passkey_mode {
                    String::new()
                } else {
                    self.create_cube_pin.value()
                };
                let datadir_path = self.datadir_path.clone();

                // Pre-generate the UUID before the async task so that retries
                // reuse the same identifier (idempotent creation).
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
                    // PIN-based Cube creation
                    Task::perform(
                        async move {
                            // Generate MasterSigner
                            let master_signer = MasterSigner::generate(network).map_err(|e| {
                                format!("Failed to generate master seed signer: {}", e)
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

                            // Use a timestamp for the master seed storage
                            let timestamp = chrono::Utc::now().timestamp();
                            let master_checksum = format!("{}{}", MASTER_SEED_LABEL, timestamp);

                            // Store master seed mnemonic encrypted with PIN
                            master_signer
                                .store_encrypted(
                                    datadir_path.path(),
                                    network,
                                    &secp,
                                    Some((master_checksum, timestamp)),
                                    Some(&pin),
                                )
                                .map_err(|e| {
                                    format!("Failed to store master seed mnemonic: {}", e)
                                })?;

                            tracing::info!(
                                "Master signer created and stored (encrypted with PIN) \
                                 with fingerprint: {}",
                                master_fingerprint
                            );

                            // Build Cube settings
                            let cube = CubeSettings::new_with_id(cube_id, cube_name, network)
                                .with_master_signer(master_fingerprint)
                                .with_pin(&pin)
                                .map_err(|e| format!("Failed to hash PIN: {}", e))?;

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
                };

                without_recovery
            }
            Message::StartRecovery => {
                self.state = State::RecoveryInput;
                self.recovery_active_index = None;
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
                        // Explicitly clear recovery words to prevent mnemonic from lingering in memory
                        for word in &mut self.recovery_words {
                            word.clear();
                            word.shrink_to_fit();
                        }
                        self.recovery_active_index = None;
                        let reload_task = self.reload();

                        // If logged in, register the new cube with the Connect API
                        if let Some(client) = self.connect_account.authenticated_client() {
                            let cube_id = cube.id.clone();
                            let cube_network = cube.network;
                            let req = RegisterCubeRequest {
                                uuid: cube.id.clone(),
                                name: cube.name.clone(),
                                network: cube.api_network_string(),
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
                        // Derive master signer from PRF output
                        let network = self.network;
                        let datadir_path = self.datadir_path.clone();
                        let cube_id = *self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);
                        let cube_name = self.create_cube_name.value.trim().to_string();
                        let credential_id = registration.credential_id.clone();
                        let prf_output = registration.prf_output;

                        Task::perform(
                            async move {
                                let master_signer =
                                    MasterSigner::from_prf_output(network, &prf_output).map_err(
                                        |e| format!("Failed to derive master signer: {}", e),
                                    )?;

                                let secp =
                                    coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
                                let master_fingerprint = master_signer.fingerprint(&secp);

                                let network_dir = datadir_path.network_directory(network);
                                network_dir.init().map_err(|e| {
                                    format!("Failed to create network directory: {}", e)
                                })?;

                                // Passkey Cube: no encrypted seed file, no PIN.
                                let passkey_metadata = settings::PasskeyMetadata {
                                    credential_id,
                                    rp_id: passkey_svc::RP_ID.to_string(),
                                    created_at: chrono::Utc::now().timestamp(),
                                    label: None,
                                };

                                let cube = CubeSettings::new_with_id(cube_id, cube_name, network)
                                    .with_master_signer(master_fingerprint)
                                    .with_passkey(passkey_metadata);

                                tracing::info!(
                                    "Passkey Cube created with fingerprint: {} (no seed on disk)",
                                    master_fingerprint
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
                        self.error = Some(e.to_string());
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
                        let network = self.network;
                        let datadir_path = self.datadir_path.clone();
                        let cube_id = *self.pending_cube_id.get_or_insert_with(uuid::Uuid::new_v4);
                        let cube_name = self.create_cube_name.value.trim().to_string();
                        let credential_id_b64 = base64::Engine::encode(
                            &base64::engine::general_purpose::STANDARD,
                            &credential_id,
                        );
                        Task::perform(
                            async move {
                                let master_signer =
                                    MasterSigner::from_prf_output(network, &prf_output).map_err(
                                        |e| format!("Failed to derive master signer: {}", e),
                                    )?;

                                let secp =
                                    coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::new();
                                let master_fingerprint = master_signer.fingerprint(&secp);

                                let network_dir = datadir_path.network_directory(network);
                                network_dir.init().map_err(|e| {
                                    format!("Failed to create network directory: {}", e)
                                })?;

                                let passkey_metadata = settings::PasskeyMetadata {
                                    credential_id: credential_id_b64,
                                    rp_id: passkey_svc::RP_ID.to_string(),
                                    created_at: chrono::Utc::now().timestamp(),
                                    label: None,
                                };

                                let cube = CubeSettings::new_with_id(cube_id, cube_name, network)
                                    .with_master_signer(master_fingerprint)
                                    .with_passkey(passkey_metadata);

                                tracing::info!(
                                    "Passkey Cube created (native macOS) with fingerprint: {}",
                                    master_fingerprint
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
                    NativeOutcome::Authenticated { .. } => {
                        self.creating_cube = false;
                        self.error = Some(
                            "Unexpected authentication response during registration.".to_string(),
                        );
                        Task::none()
                    }
                    NativeOutcome::Error(e) => {
                        self.creating_cube = false;
                        self.error = Some(e);
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

                                // Use a timestamp for the master seed storage
                                let timestamp = chrono::Utc::now().timestamp();
                                let master_checksum = format!("{}{}", MASTER_SEED_LABEL, timestamp);

                                // Store master seed mnemonic encrypted with PIN (always required)
                                master_signer
                                    .store_encrypted(
                                        datadir_path.path(),
                                        network,
                                        &secp,
                                        Some((master_checksum, timestamp)),
                                        Some(&pin),
                                    )
                                    .map_err(|e| {
                                        format!("Failed to store master seed mnemonic: {}", e)
                                    })?;

                                tracing::info!("Master signer created and stored (encrypted with PIN) with fingerprint: {}", master_fingerprint);

                                // Build Cube settings using the pre-generated, stable UUID.
                                let cube = CubeSettings::new_with_id(cube_id, cube_name, network)
                                    .with_master_signer(master_fingerprint)
                                    .with_pin(&pin)
                                    .map_err(|e| format!("Failed to hash PIN: {}", e))?;

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
                // Update the account panel's active_sub when navigating to a Connect submenu
                if let LauncherSection::Connect(ref sub) = section {
                    self.connect_account.active_sub = sub.clone();
                }
                self.active_section = section;
                // If navigating to Connect and not yet initialized, trigger Init
                if matches!(self.active_section, LauncherSection::Connect(_))
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
                    LauncherSection::Connect(app::menu::ConnectSubMenu::Security)
                ) && self.connect_account.is_authenticated()
                {
                    return map_connect_task(
                        crate::app::state::connect::account::load_security_data(
                            &self.connect_account.client,
                            self.connect_account.session_generation(),
                        ),
                    );
                }
                // Load Contacts data on demand
                if matches!(
                    self.active_section,
                    LauncherSection::Connect(app::menu::ConnectSubMenu::Contacts)
                ) && self.connect_account.is_authenticated()
                {
                    return map_connect_task(self.connect_account.reload_contacts());
                }
                Task::none()
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

            Message::View(ViewMessage::ConnectAccount(msg)) => {
                let was_authenticated = self.connect_account.is_authenticated();
                let task = map_connect_task(self.connect_account.update_message(msg));
                let now_authenticated = self.connect_account.is_authenticated();
                // Update cached keyring state on login/logout transitions
                if was_authenticated != now_authenticated {
                    self.has_stored_session = now_authenticated;
                    if !now_authenticated {
                        self.server_cube_limit = None;
                        self.remote_cubes.clear();
                    }
                }
                // Auto-expand Connect submenu and navigate to Cubes after login
                if !was_authenticated && now_authenticated {
                    self.connect_expanded = true;
                    self.active_section = LauncherSection::Cubes;
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
                            crate::services::coincube::PlanTier::Legacy => AccountTier::Legacy,
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
                                                // Already registered — update if name differs
                                                if sc.name != cube.name {
                                                    let req = UpdateCubeRequest {
                                                        name: Some(cube.name.clone()),
                                                        status: None,
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
                                                // Not registered — create
                                                let req = RegisterCubeRequest {
                                                    uuid: cube.id.clone(),
                                                    name: cube.name.clone(),
                                                    network: cube.api_network_string(),
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
                                let remote_only: Vec<RemoteCube> = server_cubes
                                    .into_iter()
                                    .filter(|sc| !local_uuids.contains(&sc.uuid))
                                    .map(|sc| RemoteCube {
                                        uuid: sc.uuid,
                                        name: sc.name,
                                        network: sc.network,
                                    })
                                    .collect();

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
                                button::xpubs_button(None, "Share Xpubs")
                                    .on_press(ViewMessage::ShareXpubs),
                            )
                        } else {
                            None
                        })
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
                                State::Cubes { cubes, create_cube } => {
                                    if *create_cube {
                                        create_cube_form(
                                            &self.create_cube_name,
                                            &self.create_cube_pin,
                                            &self.create_cube_pin_confirm,
                                            &self.error,
                                            self.creating_cube,
                                            self.passkey_mode,
                                        )
                                    } else {
                                        let current_net_str =
                                            settings::network_to_api_string(self.network);
                                        let mut col =
                                            cubes.iter().enumerate().fold(
                                                Column::new().spacing(20),
                                                |col, (i, cube)| col.push(cubes_list_item(cube, i)),
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

                                    let mut col = Column::new().spacing(20);
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
        let main_content: Element<Message> =
            if let LauncherSection::Connect(_) = &self.active_section {
                // Render Connect account panel view
                let connect_view: Element<ConnectAccountMessage> =
                    crate::app::view::connect::connect_account_panel(&self.connect_account);
                connect_view.map(|msg| Message::View(ViewMessage::ConnectAccount(msg)))
            } else {
                content
            };

        // Build the sidebar
        let sidebar = launcher_sidebar(self);

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

fn launcher_sidebar<'a>(launcher: &'a Launcher) -> Element<'a, Message> {
    use coincube_ui::{color, component::button as btn, component::text as txt, icon as ic};

    let msg = |vm: ViewMessage| -> Message { Message::View(vm) };

    let is_cubes_active = matches!(launcher.active_section, LauncherSection::Cubes);
    let cubes_button = if is_cubes_active {
        Row::new()
            .push(btn::menu_active(Some(ic::cube_icon()), "Cubes").width(Length::Fill))
            .width(Length::Fill)
    } else {
        Row::new()
            .push(
                btn::menu(Some(ic::cube_icon()), "Cubes")
                    .on_press(msg(ViewMessage::GoToSection(LauncherSection::Cubes)))
                    .width(Length::Fill),
            )
            .width(Length::Fill)
    };

    let is_authenticated = launcher.connect_account.is_authenticated();

    let mut col = Column::new()
        .spacing(0)
        .width(Length::Fill)
        .push(
            Container::new(image::coincube_wordmark(28.0))
                .padding(10)
                .center_x(Length::Fill),
        )
        .push(cubes_button);

    if is_authenticated {
        let connect_chevron = if launcher.connect_expanded {
            ic::up_icon()
        } else {
            ic::down_icon()
        };
        let connect_button: Element<Message> = iced::widget::Button::new(
            Row::new()
                .spacing(10)
                .align_y(iced::alignment::Vertical::Center)
                .push(ic::coins_icon().style(coincube_ui::theme::text::secondary))
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

    if launcher.connect_expanded && is_authenticated {
        use app::menu::ConnectSubMenu;
        let items: &[(&str, ConnectSubMenu)] = &[
            ("Overview", ConnectSubMenu::Overview),
            ("Contacts", ConnectSubMenu::Contacts),
            ("Plan & Billing", ConnectSubMenu::PlanBilling),
            ("Security", ConnectSubMenu::Security),
            ("Duress", ConnectSubMenu::Duress),
            // Invites is per-Cube (key holders), not shown in launcher
        ];
        for (label, sub) in items {
            let is_active = matches!(
                &launcher.active_section,
                LauncherSection::Connect(s) if *s == *sub
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
                            .on_press(msg(ViewMessage::GoToSection(LauncherSection::Connect(
                                sub.clone(),
                            ))))
                            .width(Length::Fill),
                    )
                    .width(Length::Fill)
            };
            col = col.push(item);
        }
    }

    // Bottom-pinned section: Sign In / email + theme toggle
    let mut bottom_col = Column::new().spacing(0).width(Length::Fill);

    if !is_authenticated {
        bottom_col = bottom_col.push(
            Container::new(
                btn::primary(None, "Sign In")
                    .on_press(msg(ViewMessage::GoToSection(LauncherSection::Connect(
                        app::menu::ConnectSubMenu::Overview,
                    ))))
                    .width(Length::Fill),
            )
            .padding(10)
            .width(Length::Fill),
        );
    } else if let Some(user) = &launcher.connect_account.user {
        bottom_col = bottom_col.push(
            Container::new(
                txt::caption(&user.email)
                    .color(color::GREY_3)
                    .align_x(Alignment::Center),
            )
            .padding(10)
            .width(Length::Fill)
            .center_x(Length::Fill),
        );
    }

    let theme_toggle_btn =
        coincube_ui::image::theme_toggle_button(launcher.theme_mode, msg(ViewMessage::ToggleTheme));

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
                "A Cube is your account which can contain a Liquid wallet, a Vault wallet and other features.",
            )
            .style(theme::text::secondary),
        );

    // Passkey toggle — hidden entirely when the passkey feature is disabled
    // via the COINCUBE_ENABLE_PASSKEY env var. The surrounding PIN flow
    // remains fully functional in that case.
    if feature_flags::PASSKEY_ENABLED {
        column = column.push(
            Toggler::new(passkey_mode)
                .label(if cfg!(target_os = "macos") {
                    "Use Passkey (Touch ID)"
                } else if cfg!(target_os = "windows") {
                    "Use Passkey (Windows Hello)"
                } else {
                    "Use Passkey (Security Key)"
                })
                .on_toggle(ViewMessage::TogglePasskeyMode),
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

fn cubes_list_item<'a>(cube: &'a CubeSettings, i: usize) -> Element<'a, ViewMessage> {
    let sync_indicator = if cube.remote_synced {
        icon::cloud_check_icon().style(theme::text::success)
    } else {
        icon::cloud_slash_icon().style(theme::text::secondary)
    };

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

fn remote_cube_list_item<'a>(cube: &'a RemoteCube) -> Element<'a, ViewMessage> {
    let disabled_style = |_t: &theme::Theme| theme::text::custom(color::GREY_3);
    Container::new(
        Row::new()
            .align_y(Alignment::Center)
            .spacing(20)
            .push(
                Container::new(
                    Button::new(
                        Column::new()
                            .push(p1_bold(&cube.name).style(disabled_style))
                            .push(p1_regular("On another device").style(disabled_style)),
                    )
                    .padding(15)
                    .style(theme::button::container_border)
                    .width(Length::Fixed(500.0)),
                )
                .style(theme::card::simple),
            )
            .push(
                Button::new(icon::cloud_arrow_down_icon())
                    .style(theme::button::secondary)
                    .padding(10),
            )
            .push(
                Button::new(icon::trash_icon())
                    .style(theme::button::secondary)
                    .padding(10)
                    .on_press(ViewMessage::DeleteCube(DeleteCubeMessage::ShowRemoteModal(
                        cube.uuid.clone(),
                    ))),
            ),
    )
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
/// `Task<launcher::Message>` by extracting the ConnectAccountMessage.
fn map_connect_task(task: Task<app::message::Message>) -> Task<Message> {
    task.map(|app_msg| match app_msg {
        app::message::Message::View(app::view::Message::ConnectAccount(acct_msg)) => {
            Message::View(ViewMessage::ConnectAccount(acct_msg))
        }
        app::message::Message::View(app::view::Message::OpenUrl(url)) => {
            Message::View(ViewMessage::OpenUrl(url))
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
    Install(CoincubeDirectory, Network, UserFlow),
    Checked(Result<State, String>),
    Run(
        CoincubeDirectory,
        app::config::Config,
        Network,
        CubeSettings,
    ),
    StartRecovery,
    CubeCreated(Result<CubeSettings, String>),
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
    ImportWallet,
    CreateWallet,
    /// W13 — launch the installer in "restore from Connect Recovery
    /// Kit" mode. Sibling to `CreateWallet` / `ImportWallet` in the
    /// cube-setup menu.
    RestoreFromRecoveryKit,
    ShowCreateCube(bool),
    CubeNameEdited(String),
    CreateCube,
    PinInput(pin_input::Message),
    PinConfirmInput(pin_input::Message),
    ShareXpubs,
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
    /// Navigate to a launcher section (Cubes or Connect submenu)
    GoToSection(LauncherSection),
    /// Toggle the Connect sidebar section expand/collapse
    ToggleConnect,
    /// Account-level Connect messages (login, plan, security, etc.)
    ConnectAccount(ConnectAccountMessage),
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

impl DeleteCubeModal {
    fn new(
        cube: CubeSettings,
        network_directory: NetworkDirectory,
        wallet_settings: Option<WalletSettings>,
        internal_bitcoind: Option<bool>,
        is_authenticated: bool,
    ) -> Self {
        let can_delete_backup = is_authenticated && cube.remote_synced;
        let mut modal = Self {
            cube: cube.clone(),
            wallet_settings: wallet_settings.clone(),
            network_directory,
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

                // Verify PIN before proceeding with deletion
                if self.cube.has_pin() {
                    let pin = self.pin_input.value();
                    if !self.cube.verify_pin(&pin) {
                        self.pin_error = Some("Incorrect PIN. Please try again.".to_string());
                        self.pin_input.clear();
                        return Task::none();
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
        let pin_ready = !self.cube.has_pin() || self.pin_input.is_complete();
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
            Some(true) => Some("(The COINCUBE-managed Bitcoin node for this network will not be affected by this action.)"),
            Some(false) => None,
            None if has_vault => Some("(If you are using a COINCUBE-managed Bitcoin node, it will not be affected by this action.)"),
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
        if self.cube.has_pin() {
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
