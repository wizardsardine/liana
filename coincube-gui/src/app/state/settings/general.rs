use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use coincube_core::signer::MasterSigner;
use coincube_ui::widget::Element;
use iced::Task;
use rand::seq::SliceRandom;
use zeroize::Zeroizing;

use crate::app::cache::Cache;
use crate::app::error::Error;
use crate::app::menu::Menu;
use crate::app::message::{FiatMessage, Message};
use crate::app::settings::fiat::PriceSetting;
use crate::app::settings::unit::UnitSetting;
use crate::app::settings::{self, update_settings_file};
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;
use crate::dir::CoincubeDirectory;
use crate::pin_input::PinInput;
use crate::services::fiat::currency::Currency;

/// State for the master seed backup flow.
///
/// Unlike the old Liquid-Settings version, this flow no longer depends on
/// the Breez client / Liquid signer — it works on every network by loading
/// the encrypted mnemonic directly from the datadir using the Cube's PIN.
#[derive(Debug, Clone, PartialEq)]
pub enum BackupSeedState {
    /// Not in backup flow.
    None,
    /// Re-prompt for the Cube PIN before revealing the mnemonic. This is
    /// both a security gate and the mechanism by which the encrypted
    /// mnemonic file gets decrypted.
    PinEntry {
        /// Error from the previous verification attempt, if any.
        error: Option<String>,
    },
    /// Intro screen with security warning and "I understand" checkbox.
    Intro(bool),
    /// Show the 12 recovery words in a grid.
    RecoveryPhrase,
    /// Verify the user wrote them down by asking for 3 random words.
    Verification {
        word_indices: [usize; 3],
        word_inputs: [String; 3],
        error: Option<String>,
    },
    /// Backup complete — cube.backed_up is now true.
    Completed,
    /// Passkey re-authentication is required to derive the mnemonic, but the
    /// passkey auth ceremony is not yet wired up.  Show an informational
    /// screen explaining how to back up once passkey auth is available.
    PasskeyPending,
}

/// Generate 3 random unique word indices from 1 to mnemonic_len.
fn generate_random_word_indices(mnemonic_len: usize) -> Option<[usize; 3]> {
    if mnemonic_len < 3 {
        return None;
    }
    let mut indices: Vec<usize> = (1..=mnemonic_len).collect();
    let mut rng = rand::thread_rng();
    indices.shuffle(&mut rng);
    Some([indices[0], indices[1], indices[2]])
}

async fn update_price_setting(
    data_dir: CoincubeDirectory,
    network: Network,
    cube_id: String,
    new_price_setting: PriceSetting,
) -> Result<(), Error> {
    let network_dir = data_dir.network_directory(network);
    let mut cube_found = false;
    let result = update_settings_file(&network_dir, |mut settings| {
        if let Some(cube) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
            cube.fiat_price = Some(new_price_setting);
            cube_found = true;
        } else {
            tracing::error!(
                "Cube not found with id: {} - cannot save price setting",
                cube_id
            );
            tracing::error!(
                "Available cubes: {:?}",
                settings.cubes.iter().map(|c| &c.id).collect::<Vec<_>>()
            );
        }
        Some(settings)
    })
    .await;

    match result {
        Ok(()) if cube_found => Ok(()),
        Ok(()) => Err(Error::Unexpected(
            "Cube not found in settings file".to_string(),
        )),
        Err(e) => {
            tracing::error!("Failed to save price setting: {:?}", e);
            Err(Error::Unexpected(format!(
                "Failed to update settings: {}",
                e
            )))
        }
    }
}

async fn update_unit_setting(
    data_dir: CoincubeDirectory,
    network: Network,
    cube_id: String,
    new_unit_setting: UnitSetting,
) -> Result<(), Error> {
    let network_dir = data_dir.network_directory(network);
    let mut cube_found = false;
    let result = update_settings_file(&network_dir, |mut settings| {
        if let Some(cube) = settings.cubes.iter_mut().find(|c| c.id == cube_id) {
            cube.unit_setting = new_unit_setting;
            cube_found = true;
        } else {
            tracing::error!(
                "Cube not found with id: {} - cannot save unit setting",
                cube_id
            );
            tracing::error!(
                "Available cubes: {:?}",
                settings.cubes.iter().map(|c| &c.id).collect::<Vec<_>>()
            );
        }
        // Always return Some to prevent file deletion
        Some(settings)
    })
    .await;

    match result {
        Ok(()) if cube_found => Ok(()),
        Ok(()) => Err(Error::Unexpected(
            "Cube not found in settings file".to_string(),
        )),
        Err(e) => {
            tracing::error!("Failed to save unit setting: {:?}", e);
            Err(Error::Unexpected(format!(
                "Failed to update settings: {}",
                e
            )))
        }
    }
}

pub struct GeneralSettingsState {
    cube_id: String,
    new_price_setting: PriceSetting,
    new_unit_setting: UnitSetting,
    currencies: Vec<Currency>,
    developer_mode: bool,
    show_direction_badges: bool,
    error: Option<Error>,
    /// Master seed backup flow state.
    pub backup_state: BackupSeedState,
    /// PIN re-entry input for the backup flow's PinEntry state.
    /// Held as a separate field because `PinInput` doesn't implement
    /// `Debug`/`Clone`/`PartialEq` (required by `BackupSeedState`).
    pub backup_pin: PinInput,
    /// Transient 12-word mnemonic held only while the backup flow is
    /// active. Loaded from the datadir via PIN decryption, wiped on
    /// flow completion / cancellation. `Zeroizing` ensures the heap
    /// memory is scrubbed on drop.
    pub backup_mnemonic: Option<Zeroizing<Vec<String>>>,
}

impl From<GeneralSettingsState> for Box<dyn State> {
    fn from(s: GeneralSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

impl GeneralSettingsState {
    pub fn new(
        cube_id: String,
        price_setting: PriceSetting,
        unit_setting: UnitSetting,
        datadir_path: &CoincubeDirectory,
    ) -> Self {
        use crate::app::settings::global::GlobalSettings;
        let global_path = GlobalSettings::path(datadir_path);
        let developer_mode = GlobalSettings::load_developer_mode(&global_path);
        let show_direction_badges = GlobalSettings::load_show_direction_badges(&global_path);
        Self {
            cube_id,
            new_price_setting: price_setting,
            new_unit_setting: unit_setting,
            currencies: Vec::new(),
            developer_mode,
            show_direction_badges,
            error: None,
            backup_state: BackupSeedState::None,
            backup_pin: PinInput::new(),
            backup_mnemonic: None,
        }
    }

    /// Look up this Cube in the settings file on disk.
    ///
    /// Returns the stored `CubeSettings` (which contains the master signer
    /// fingerprint and PIN hash) or `None` if the cube can't be found.
    fn lookup_cube(&self, cache: &Cache) -> Option<settings::CubeSettings> {
        let network_dir = cache.datadir_path.network_directory(cache.network);
        let settings = settings::Settings::from_file(&network_dir).ok()?;
        settings.cubes.into_iter().find(|c| c.id == self.cube_id)
    }

    /// Handle a single `BackupWalletMessage` — returns the task to dispatch.
    fn handle_backup_message(
        &mut self,
        cache: &Cache,
        msg: view::BackupWalletMessage,
    ) -> Task<Message> {
        use view::BackupWalletMessage;

        match msg {
            BackupWalletMessage::Start => {
                // Passkey-backed cubes derive their mnemonic from the WebAuthn
                // PRF output — there is no encrypted mnemonic on disk and no
                // PIN.  Once passkey re-authentication is implemented we will
                // re-derive the mnemonic here; until then, show a helpful
                // holding screen.
                if let Some(cube) = self.lookup_cube(cache) {
                    if cube.is_passkey_cube() {
                        self.backup_state = BackupSeedState::PasskeyPending;
                        return Task::none();
                    }
                }
                // Always re-prompt for PIN before showing anything sensitive.
                self.backup_pin = PinInput::new();
                self.backup_mnemonic = None;
                self.backup_state = BackupSeedState::PinEntry { error: None };
                Task::none()
            }
            BackupWalletMessage::PinInput(pin_msg) => {
                // Clear previous error on new input.
                if let BackupSeedState::PinEntry { error } = &mut self.backup_state {
                    *error = None;
                }
                self.backup_pin.update(pin_msg).map(|m| {
                    Message::View(view::Message::Settings(
                        view::SettingsMessage::BackupMasterSeed(BackupWalletMessage::PinInput(m)),
                    ))
                })
            }
            BackupWalletMessage::VerifyPin => {
                if !matches!(self.backup_state, BackupSeedState::PinEntry { .. }) {
                    return Task::none();
                }
                if !self.backup_pin.is_complete() {
                    self.backup_state = BackupSeedState::PinEntry {
                        error: Some("Please enter all 4 PIN digits".to_string()),
                    };
                    return Task::none();
                }
                let pin = self.backup_pin.value();
                let Some(cube) = self.lookup_cube(cache) else {
                    self.backup_state = BackupSeedState::PinEntry {
                        error: Some("Cube not found in settings".to_string()),
                    };
                    return Task::none();
                };
                let Some(fingerprint) = cube.master_signer_fingerprint else {
                    self.backup_state = BackupSeedState::PinEntry {
                        error: Some("This Cube has no master signer.".to_string()),
                    };
                    return Task::none();
                };

                let datadir = cache.datadir_path.path().to_path_buf();
                let network = cache.network;

                // Run Argon2id PIN verification + mnemonic decryption off
                // the UI thread to avoid blocking the event loop.
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            if !cube.verify_pin(&pin) {
                                return Err("Incorrect PIN. Please try again.".to_string());
                            }
                            load_mnemonic_words(&datadir, network, fingerprint, &pin)
                        })
                        .await
                        .map_err(|e| format!("PIN verification task failed: {}", e))?
                    },
                    |res| {
                        Message::View(view::Message::Settings(
                            view::SettingsMessage::BackupMasterSeed(
                                view::BackupWalletMessage::PinVerified(res),
                            ),
                        ))
                    },
                )
            }
            BackupWalletMessage::PinVerified(result) => {
                match result {
                    Ok(words) => {
                        self.backup_pin.clear();
                        self.backup_mnemonic = Some(Zeroizing::new(words));
                        self.backup_state = BackupSeedState::Intro(false);
                    }
                    Err(e) => {
                        self.backup_pin.clear();
                        self.backup_state = BackupSeedState::PinEntry { error: Some(e) };
                    }
                }
                Task::none()
            }
            BackupWalletMessage::ToggleBackupIntroCheck => {
                if let BackupSeedState::Intro(checked) = self.backup_state {
                    self.backup_state = BackupSeedState::Intro(!checked);
                }
                Task::none()
            }
            BackupWalletMessage::NextStep => {
                self.backup_state = match &self.backup_state {
                    BackupSeedState::Intro(true) => BackupSeedState::RecoveryPhrase,
                    BackupSeedState::RecoveryPhrase => {
                        let mnemonic_len =
                            self.backup_mnemonic.as_ref().map(|m| m.len()).unwrap_or(0);
                        match generate_random_word_indices(mnemonic_len) {
                            Some(word_indices) => BackupSeedState::Verification {
                                word_indices,
                                word_inputs: [String::new(), String::new(), String::new()],
                                error: None,
                            },
                            None => {
                                tracing::error!("Mnemonic unavailable or has fewer than 3 words");
                                self.backup_state.clone()
                            }
                        }
                    }
                    _ => self.backup_state.clone(),
                };
                Task::none()
            }
            BackupWalletMessage::PreviousStep => {
                self.backup_state = match &self.backup_state {
                    BackupSeedState::PinEntry { .. } => {
                        self.backup_pin.clear();
                        BackupSeedState::None
                    }
                    BackupSeedState::Intro(_) => {
                        // Going back from Intro wipes the loaded mnemonic.
                        self.backup_mnemonic = None;
                        BackupSeedState::None
                    }
                    BackupSeedState::RecoveryPhrase => BackupSeedState::Intro(false),
                    BackupSeedState::Verification { .. } => BackupSeedState::RecoveryPhrase,
                    BackupSeedState::Completed => {
                        self.backup_mnemonic = None;
                        BackupSeedState::None
                    }
                    BackupSeedState::PasskeyPending => BackupSeedState::None,
                    BackupSeedState::None => BackupSeedState::None,
                };
                Task::none()
            }
            BackupWalletMessage::WordInput { index, input } => {
                if let BackupSeedState::Verification {
                    word_indices,
                    word_inputs,
                    error,
                } = &self.backup_state
                {
                    let mut new_inputs = word_inputs.clone();
                    if let Some(pos) = word_indices.iter().position(|&i| i == index as usize) {
                        new_inputs[pos] = input;
                    }
                    self.backup_state = BackupSeedState::Verification {
                        word_indices: *word_indices,
                        word_inputs: new_inputs,
                        error: error.clone(),
                    };
                }
                Task::none()
            }
            BackupWalletMessage::VerifyPhrase => {
                let BackupSeedState::Verification {
                    word_indices,
                    word_inputs,
                    ..
                } = &self.backup_state
                else {
                    return Task::none();
                };
                let Some(mnemonic) = &self.backup_mnemonic else {
                    return Task::none();
                };

                let all_correct = word_indices.iter().enumerate().all(|(i, &word_idx)| {
                    if word_idx == 0 || word_idx > mnemonic.len() {
                        return false;
                    }
                    word_inputs[i].trim() == mnemonic[word_idx - 1]
                });

                if all_correct {
                    // Verification passed — persist `backed_up = true` to
                    // settings.json. Completion is handled by the async
                    // BackupMasterSeedUpdated message.
                    let cube_id = self.cube_id.clone();
                    let network = cache.network;
                    let datadir = cache.datadir_path.clone();
                    Task::perform(
                        async move {
                            let network_dir = datadir.network_directory(network);
                            update_settings_file(&network_dir, |mut s| {
                                if let Some(cube) = s.cubes.iter_mut().find(|c| c.id == cube_id) {
                                    cube.backed_up = true;
                                }
                                Some(s)
                            })
                            .await
                            .map_err(|e| format!("Failed to update settings: {}", e))
                        },
                        |res: Result<(), String>| match res {
                            Ok(()) => Message::View(view::Message::Settings(
                                view::SettingsMessage::BackupMasterSeedUpdated,
                            )),
                            Err(e) => Message::View(view::Message::ShowError(e)),
                        },
                    )
                } else {
                    self.backup_state = BackupSeedState::Verification {
                        word_indices: *word_indices,
                        word_inputs: word_inputs.clone(),
                        error: Some(
                            "The words you entered don't match. Please try again.".to_string(),
                        ),
                    };
                    Task::none()
                }
            }
            BackupWalletMessage::Complete => {
                // User dismissed the Completed screen — return to settings.
                self.backup_mnemonic = None;
                self.backup_state = BackupSeedState::None;
                Task::none()
            }
        }
    }
}

/// Load the encrypted mnemonic from the datadir and return the 12 words
/// as a `Vec<String>`. The password is verified by the decryption step —
/// if the PIN is wrong, decryption returns an error.
fn load_mnemonic_words(
    datadir: &std::path::Path,
    network: Network,
    fingerprint: Fingerprint,
    pin: &str,
) -> Result<Vec<String>, String> {
    let signer =
        MasterSigner::from_datadir_by_fingerprint(datadir, network, fingerprint, Some(pin))
            .map_err(|e| e.to_string())?;
    Ok(signer.words().iter().map(|w| (*w).to_string()).collect())
}

impl State for GeneralSettingsState {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::general::general_section(
            menu,
            cache,
            &self.new_price_setting,
            &self.new_unit_setting,
            &self.currencies,
            self.developer_mode,
            self.show_direction_badges,
            &self.backup_state,
            &self.backup_pin,
            self.backup_mnemonic.as_deref().map(|v| v.as_slice()),
        )
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> iced::Task<Message> {
        if self.new_price_setting.is_enabled {
            let source = self.new_price_setting.source;
            return Task::perform(async move { source }, |source| {
                FiatMessage::ListCurrencies(source).into()
            });
        }
        Task::none()
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::Fiat(FiatMessage::SaveChanges) => {
                self.error = None;
                tracing::info!(
                    "Saving cube fiat price setting: {:?}",
                    self.new_price_setting
                );
                let price_setting = self.new_price_setting.clone();
                let network = cache.network;
                let datadir_path = cache.datadir_path.clone();
                let cube_id = self.cube_id.clone();
                Task::perform(
                    async move {
                        update_price_setting(datadir_path, network, cube_id, price_setting).await
                    },
                    |res| match res {
                        Ok(()) => Message::SettingsSaved,
                        Err(e) => Message::SettingsSaveFailed(e),
                    },
                )
            }
            Message::SettingsSaved => {
                tracing::info!("GeneralSettingsState: SettingsSaved received");
                self.error = None;
                // Reload unit setting from disk to sync toggle state with what was saved
                let network_dir = cache.datadir_path.network_directory(cache.network);
                tracing::info!(
                    "GeneralSettingsState: Loading settings from {:?}",
                    network_dir.path()
                );
                if let Ok(settings) = crate::app::settings::Settings::from_file(&network_dir) {
                    tracing::info!(
                        "GeneralSettingsState: Loaded settings, searching for cube_id: {}",
                        self.cube_id
                    );
                    tracing::info!(
                        "GeneralSettingsState: Available cubes: {:?}",
                        settings.cubes.iter().map(|c| &c.id).collect::<Vec<_>>()
                    );
                    if let Some(cube) = settings.cubes.iter().find(|c| c.id == self.cube_id) {
                        tracing::info!(
                            "GeneralSettingsState: Found cube, reloading unit_setting: {:?}",
                            cube.unit_setting.display_unit
                        );
                        self.new_unit_setting = cube.unit_setting.clone();
                        tracing::info!(
                            "GeneralSettingsState: new_unit_setting now set to: {:?}",
                            self.new_unit_setting.display_unit
                        );
                    } else {
                        tracing::warn!(
                            "GeneralSettingsState: Cube not found with id: {}",
                            self.cube_id
                        );
                    }
                } else {
                    tracing::error!("GeneralSettingsState: Failed to load settings from disk");
                }
                Task::none()
            }
            Message::SettingsSaveFailed(e) => {
                let err_msg = e.to_string();
                self.error = Some(e);
                // Show error in global toast
                let toast_task = Task::done(Message::View(view::Message::ShowError(err_msg)));
                // Reload settings from disk to revert toggle state to persisted value
                let network_dir = cache.datadir_path.network_directory(cache.network);
                if let Ok(settings) = crate::app::settings::Settings::from_file(&network_dir) {
                    if let Some(cube) = settings.cubes.iter().find(|c| c.id == self.cube_id) {
                        tracing::info!(
                            "Reverting unit_setting to persisted value after save failure: {:?}",
                            cube.unit_setting.display_unit
                        );
                        self.new_unit_setting = cube.unit_setting.clone();
                        self.new_price_setting = cube.fiat_price.clone().unwrap_or_default();
                    } else {
                        tracing::warn!(
                            "Could not revert settings: Cube not found with id: {}",
                            self.cube_id
                        );
                    }
                } else {
                    tracing::error!("Could not revert settings: Failed to load settings from disk");
                }
                toast_task
            }
            Message::Fiat(FiatMessage::ValidateCurrencySetting) => {
                self.error = None;
                if !self.currencies.contains(&self.new_price_setting.currency) {
                    if self.currencies.contains(&Currency::default()) {
                        self.new_price_setting.currency = Currency::default();
                    } else if let Some(curr) = self.currencies.first() {
                        self.new_price_setting.currency = *curr;
                    } else {
                        let err =
                            Error::Unexpected("No available currencies in the list.".to_string());
                        let err_msg = err.to_string();
                        self.error = Some(err);
                        return Task::done(Message::View(view::Message::ShowError(err_msg)));
                    }
                }
                Task::perform(async move {}, |_| FiatMessage::SaveChanges.into())
            }
            Message::Fiat(FiatMessage::ListCurrenciesResult(source, res)) => {
                if self.new_price_setting.source != source {
                    return Task::none();
                }
                match res {
                    Ok(list) => {
                        self.error = None;
                        self.currencies = list.currencies;
                        Task::perform(async move {}, |_| {
                            FiatMessage::ValidateCurrencySetting.into()
                        })
                    }
                    Err(e) => {
                        let err: Error = e.into();
                        let err_msg = err.to_string();
                        self.error = Some(err);
                        Task::done(Message::View(view::Message::ShowError(err_msg)))
                    }
                }
            }
            Message::View(view::Message::Settings(view::SettingsMessage::Fiat(msg))) => {
                match msg {
                    view::FiatMessage::Enable(is_enabled) => {
                        self.new_price_setting.is_enabled = is_enabled;
                        if self.new_price_setting.is_enabled {
                            let source = self.new_price_setting.source;
                            return Task::perform(async move { source }, |source| {
                                FiatMessage::ListCurrencies(source).into()
                            });
                        } else {
                            return Task::perform(async move {}, |_| {
                                FiatMessage::SaveChanges.into()
                            });
                        }
                    }
                    view::FiatMessage::SourceEdited(source) => {
                        self.new_price_setting.source = source;
                        if self.new_price_setting.is_enabled {
                            let source = self.new_price_setting.source;
                            return Task::perform(async move { source }, |source| {
                                FiatMessage::ListCurrencies(source).into()
                            });
                        }
                    }
                    view::FiatMessage::CurrencyEdited(currency) => {
                        self.new_price_setting.currency = currency;
                        return Task::perform(async move {}, |_| {
                            FiatMessage::ValidateCurrencySetting.into()
                        });
                    }
                }
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::DisplayUnitChanged(
                unit,
            ))) => {
                tracing::info!("GeneralSettingsState: DisplayUnitChanged({:?})", unit);
                self.new_unit_setting.display_unit = unit;
                tracing::info!(
                    "GeneralSettingsState: Updated new_unit_setting to {:?}",
                    self.new_unit_setting.display_unit
                );
                let cube_id = self.cube_id.clone();
                let unit_setting = self.new_unit_setting.clone();
                let network = cache.network;
                let datadir_path = cache.datadir_path.clone();

                // Save to disk - cache update will happen in App::update after this returns
                #[allow(clippy::let_and_return)]
                return Task::perform(
                    async move {
                        tracing::info!(
                            "Saving unit_setting to disk: {:?}",
                            unit_setting.display_unit
                        );
                        update_unit_setting(datadir_path, network, cube_id, unit_setting).await
                    },
                    |res| match res {
                        Ok(()) => {
                            tracing::info!("Unit setting saved successfully");
                            Message::SettingsSaved
                        }
                        Err(e) => {
                            tracing::error!("Unit setting save failed: {:?}", e);
                            Message::SettingsSaveFailed(e)
                        }
                    },
                );
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::ToggleDirectionBadges(show),
            )) => {
                self.show_direction_badges = show;
                let datadir_path = cache.datadir_path.clone();
                Task::perform(
                    async move {
                        use crate::app::settings::global::GlobalSettings;
                        GlobalSettings::update_show_direction_badges(
                            &GlobalSettings::path(&datadir_path),
                            show,
                        )
                    },
                    |res| match res {
                        Ok(()) => Message::SettingsSaved,
                        Err(e) => Message::SettingsSaveFailed(e.into()),
                    },
                )
            }
            Message::View(view::Message::Settings(view::SettingsMessage::TestToast(level))) => {
                let label = match level {
                    log::Level::Error => "Error",
                    log::Level::Warn => "Warn",
                    log::Level::Info => "Info",
                    log::Level::Debug => "Debug",
                    log::Level::Trace => "Trace",
                };
                Task::done(Message::View(view::Message::ShowToast(
                    level,
                    format!("Test {} toast", label),
                )))
            }
            // --- Master seed backup flow ---
            Message::View(view::Message::Settings(view::SettingsMessage::BackupMasterSeed(
                backup_msg,
            ))) => self.handle_backup_message(cache, backup_msg),
            Message::View(view::Message::Settings(
                view::SettingsMessage::BackupMasterSeedUpdated,
            )) => {
                // Cube's backed_up flag has been persisted — transition to
                // the Completed screen. Clear the transient PIN input too.
                self.backup_state = BackupSeedState::Completed;
                self.backup_pin.clear();
                self.backup_mnemonic = None;
                Task::none()
            }
            _ => Task::none(),
        }
    }
}
