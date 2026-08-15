use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use coincube_core::signer::SignerError;
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
use crate::services::unlock;

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
        /// True while the async settings.json write is in flight after a
        /// successful verification. Suppresses duplicate Verify clicks.
        saving: bool,
    },
    /// Backup complete — cube.backed_up is now true.
    Completed,
    /// Terminal failure with copy for the user. Reached only from the passkey
    /// path — the PIN path lands its errors back on the keypad, which a
    /// system-owned prompt has no equivalent of.
    Error(String),
    /// The system passkey prompt is up, re-deriving this Cube's master seed
    /// from a WebAuthn assertion.
    ///
    /// This replaces `PasskeyPending`, which was a dead end: it told the user
    /// that re-authentication was "coming soon" and to keep hold of the device
    /// holding their passkey — advice that was both useless and, after the
    /// 2026-08-04 decision, wrong, since a platform passkey is Apple-ID-bound
    /// rather than device-bound.
    PasskeyReauth,
}

/// Generate 3 random unique word indices from 1 to mnemonic_len.
///
/// Shared with the creation-time backup step in `home.rs` so the two places
/// that verify a written-down seed phrase challenge the user identically.
pub(crate) fn generate_random_word_indices(mnemonic_len: usize) -> Option<[usize; 3]> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::fiat::{
        api::{ListCurrenciesResult, PriceApiError},
        PriceSource,
    };
    use coincube_ui::component::amount::BitcoinDisplayUnit;

    fn state() -> GeneralSettingsState {
        GeneralSettingsState::new(
            "cube-1".to_string(),
            SettingsSection::General,
            PriceSetting::default(),
            UnitSetting::default(),
            &CoincubeDirectory::new(std::path::PathBuf::new()),
        )
    }

    /// The seed-reveal flows share the unlock throttle, so misclassifying an
    /// operational failure as a wrong PIN spends the owner's guesses on a fault
    /// no PIN can fix — and eventually locks them out of their own Cube.
    #[test]
    fn only_a_failed_decryption_counts_as_a_wrong_pin() {
        assert!(is_wrong_pin(&SignerError::InvalidPassword));

        for e in [
            SignerError::DeviceSecretRequired,
            SignerError::SignerNotFound(Fingerprint::default()),
            SignerError::MnemonicStorage(std::io::Error::other("disk")),
            SignerError::NotEncryptedFile,
            SignerError::InvalidFileFormat,
            SignerError::DecryptionFailed("argon2".to_string()),
            SignerError::PasswordRequired,
        ] {
            assert!(
                !is_wrong_pin(&e),
                "{} would be reported as a wrong PIN and charged to the throttle",
                e
            );
        }
    }

    fn backup_msg(msg: view::BackupWalletMessage) -> Message {
        Message::View(view::Message::Settings(
            view::SettingsMessage::BackupMasterSeed(msg),
        ))
    }

    fn words() -> Vec<String> {
        [
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
            "juliet", "kilo", "lima",
        ]
        .iter()
        .map(|word| (*word).to_string())
        .collect()
    }

    #[test]
    fn random_word_indices_require_three_unique_in_range_values() {
        assert!(generate_random_word_indices(2).is_none());

        let indices = generate_random_word_indices(12).unwrap();
        assert!(indices.iter().all(|index| (1..=12).contains(index)));
        assert_ne!(indices[0], indices[1]);
        assert_ne!(indices[0], indices[2]);
        assert_ne!(indices[1], indices[2]);
    }

    #[test]
    fn backup_start_prompts_for_pin_when_cube_is_not_passkey() {
        let mut state = state();

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::Start),
        );

        assert!(matches!(
            state.backup_state,
            BackupSeedState::PinEntry { error: None }
        ));
        assert!(state.backup_mnemonic.is_none());
    }

    #[test]
    fn verify_pin_requires_pin_entry_and_all_digits() {
        let mut state = state();

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::VerifyPin),
        );
        assert_eq!(state.backup_state, BackupSeedState::None);

        state.backup_state = BackupSeedState::PinEntry { error: None };
        state.backup_pin.digits = [
            "1".to_string(),
            "2".to_string(),
            String::new(),
            "4".to_string(),
        ];

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::VerifyPin),
        );

        assert!(matches!(
            &state.backup_state,
            BackupSeedState::PinEntry { error: Some(error) }
                if error == "Please enter all 4 PIN digits"
        ));
    }

    /// A verification result that arrives after the user has left the wizard
    /// must be dropped, not acted on.
    ///
    /// This is reachable, not theoretical: `PreviousStep` cancels a live passkey
    /// ceremony, which wakes the waiting task with `Cancelled`, and the
    /// authenticator can answer in the same instant the user backs out. Both
    /// arms were wrong before the guard — `Ok` walked someone who had just left
    /// back into the flow that reveals the master seed, and `Err` landed a
    /// passkey Cube on a PIN keypad it has no PIN for.
    ///
    /// The invariant used to live only in a comment on `PreviousStep`.
    #[test]
    fn a_stale_verification_result_is_ignored() {
        for result in [Ok(Zeroizing::new(words())), Err("cancelled".to_string())] {
            let mut state = state();
            // Whatever the wizard was doing, the user has since backed out.
            assert_eq!(state.backup_state, BackupSeedState::None);

            let _ = state.update(
                None,
                &Cache::default(),
                backup_msg(view::BackupWalletMessage::PinVerified(result)),
            );

            assert_eq!(
                state.backup_state,
                BackupSeedState::None,
                "a stale result reopened a wizard the user had left"
            );
            assert!(
                state.backup_mnemonic.is_none(),
                "a stale result loaded the master seed for display"
            );
        }
    }

    /// The seed phrase must be `Zeroizing` *before* it enters the message
    /// queue, not after it is delivered.
    ///
    /// iced clones messages freely, so a bare `Vec<String>` in the message meant
    /// every in-flight copy dropped un-scrubbed and only the one that reached
    /// state was protected. This is a type-level property — the test exists so
    /// the signature can't quietly revert to `Vec<String>`, which would still
    /// compile everywhere the value is used and leave no other trace.
    #[test]
    fn the_seed_is_zeroizing_before_it_enters_the_message_queue() {
        fn assert_zeroizing(_: &view::BackupWalletMessage) {}
        let msg = view::BackupWalletMessage::PinVerified(Ok(Zeroizing::new(words())));
        assert_zeroizing(&msg);

        // The sibling flow in `recovery_kit.rs` carries the same shape. If these
        // two ever disagree again, one of them is leaking copies of a seed.
        let _: Result<Zeroizing<Vec<String>>, String> = Ok(Zeroizing::new(words()));

        // And it is redacted in Debug, so a `{:?}` on the message can't print it.
        assert!(!format!("{:?}", msg).contains(&words()[0]));
    }

    #[test]
    fn pin_verified_success_stores_words_and_clears_pin() {
        let mut state = state();
        state.backup_state = BackupSeedState::PinEntry { error: None };
        state.backup_pin.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::PinVerified(Ok(Zeroizing::new(
                words(),
            )))),
        );

        assert_eq!(state.backup_pin.value().as_str(), "");
        assert!(matches!(state.backup_state, BackupSeedState::Intro(false)));
        assert_eq!(
            state.backup_mnemonic.as_ref().map(|words| words.len()),
            Some(12)
        );
    }

    #[test]
    fn pin_verified_error_returns_to_pin_entry_and_clears_pin() {
        let mut state = state();
        state.backup_state = BackupSeedState::PinEntry { error: None };
        state.backup_pin.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::PinVerified(Err(
                "Incorrect PIN".to_string()
            ))),
        );

        assert_eq!(state.backup_pin.value().as_str(), "");
        assert!(matches!(
            &state.backup_state,
            BackupSeedState::PinEntry { error: Some(error) } if error == "Incorrect PIN"
        ));
    }

    #[test]
    fn intro_and_previous_step_transitions_clear_sensitive_state() {
        let mut state = state();
        state.backup_mnemonic = Some(Zeroizing::new(words()));
        state.backup_state = BackupSeedState::Intro(false);

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::ToggleBackupIntroCheck),
        );
        assert_eq!(state.backup_state, BackupSeedState::Intro(true));

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::NextStep),
        );
        assert_eq!(state.backup_state, BackupSeedState::RecoveryPhrase);

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::PreviousStep),
        );
        assert_eq!(state.backup_state, BackupSeedState::Intro(false));

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::PreviousStep),
        );
        assert_eq!(state.backup_state, BackupSeedState::None);
        assert!(state.backup_mnemonic.is_none());
    }

    #[test]
    fn recovery_phrase_advances_to_verification_only_with_loaded_words() {
        let mut state = state();
        state.backup_state = BackupSeedState::RecoveryPhrase;

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::NextStep),
        );
        assert_eq!(state.backup_state, BackupSeedState::RecoveryPhrase);

        state.backup_mnemonic = Some(Zeroizing::new(words()));
        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::NextStep),
        );

        assert!(matches!(
            &state.backup_state,
            BackupSeedState::Verification {
                word_indices,
                word_inputs,
                error: None,
                saving: false
            } if word_indices.iter().all(|index| (1..=12).contains(index))
                && word_inputs.iter().all(String::is_empty)
        ));
    }

    #[test]
    fn word_input_updates_matching_prompt_and_ignores_edits_while_saving() {
        let mut state = state();
        state.backup_state = BackupSeedState::Verification {
            word_indices: [1, 3, 5],
            word_inputs: [String::new(), String::new(), String::new()],
            error: Some("old error".to_string()),
            saving: false,
        };

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::WordInput {
                index: 3,
                input: "charlie".to_string(),
            }),
        );

        assert!(matches!(
            &state.backup_state,
            BackupSeedState::Verification {
                word_inputs,
                error: Some(error),
                saving: false,
                ..
            } if word_inputs[1] == "charlie" && error == "old error"
        ));

        state.backup_state = BackupSeedState::Verification {
            word_indices: [1, 3, 5],
            word_inputs: [
                "alpha".to_string(),
                "charlie".to_string(),
                "echo".to_string(),
            ],
            error: None,
            saving: true,
        };
        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::WordInput {
                index: 1,
                input: "changed".to_string(),
            }),
        );
        assert!(matches!(
            &state.backup_state,
            BackupSeedState::Verification {
                word_inputs,
                saving: true,
                ..
            } if word_inputs[0] == "alpha"
        ));
    }

    #[test]
    fn verify_phrase_sets_inline_error_for_mismatched_words() {
        let mut state = state();
        state.backup_mnemonic = Some(Zeroizing::new(words()));
        state.backup_state = BackupSeedState::Verification {
            word_indices: [1, 2, 3],
            word_inputs: [
                "alpha".to_string(),
                "wrong".to_string(),
                "charlie".to_string(),
            ],
            error: None,
            saving: false,
        };

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::VerifyPhrase),
        );

        assert!(matches!(
            &state.backup_state,
            BackupSeedState::Verification {
                error: Some(error),
                saving: false,
                ..
            } if error == "The words you entered don't match. Please try again."
        ));
    }

    #[test]
    fn verify_phrase_marks_saving_for_correct_words() {
        let mut state = state();
        state.backup_mnemonic = Some(Zeroizing::new(words()));
        state.backup_state = BackupSeedState::Verification {
            word_indices: [1, 2, 3],
            word_inputs: [
                "alpha".to_string(),
                "bravo".to_string(),
                "charlie".to_string(),
            ],
            error: Some("old error".to_string()),
            saving: false,
        };

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::VerifyPhrase),
        );

        assert!(matches!(
            &state.backup_state,
            BackupSeedState::Verification {
                error: None,
                saving: true,
                ..
            }
        ));
    }

    #[test]
    fn backup_save_failure_restores_verification_error() {
        let mut state = state();
        state.backup_state = BackupSeedState::Verification {
            word_indices: [1, 2, 3],
            word_inputs: [
                "alpha".to_string(),
                "bravo".to_string(),
                "charlie".to_string(),
            ],
            error: None,
            saving: true,
        };

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::BackupSaveResult(Err(
                "disk full".to_string(),
            ))),
        );

        assert!(matches!(
            &state.backup_state,
            BackupSeedState::Verification {
                error: Some(error),
                saving: false,
                ..
            } if error == "Failed to save backup status: disk full"
        ));
    }

    #[test]
    fn backup_completed_message_clears_pin_and_mnemonic() {
        let mut state = state();
        state.backup_pin.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        state.backup_mnemonic = Some(Zeroizing::new(words()));

        let _ = state.update(
            None,
            &Cache::default(),
            Message::View(view::Message::Settings(
                view::SettingsMessage::BackupMasterSeedUpdated,
            )),
        );

        assert_eq!(state.backup_state, BackupSeedState::Completed);
        assert_eq!(state.backup_pin.value().as_str(), "");
        assert!(state.backup_mnemonic.is_none());

        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::Complete),
        );
        assert_eq!(state.backup_state, BackupSeedState::None);
    }

    #[test]
    fn fiat_currency_results_ignore_stale_source_and_store_matching_result() {
        let mut state = state();
        state.new_price_setting.source = PriceSource::Coincube;
        state.error = Some(Error::Unexpected("old".to_string()));

        let _ = state.update(
            None,
            &Cache::default(),
            Message::Fiat(FiatMessage::ListCurrenciesResult(
                PriceSource::CoinGecko,
                Ok(ListCurrenciesResult {
                    currencies: vec![Currency::EUR],
                }),
            )),
        );
        assert!(state.currencies.is_empty());
        assert!(state.error.is_some());

        let _ = state.update(
            None,
            &Cache::default(),
            Message::Fiat(FiatMessage::ListCurrenciesResult(
                PriceSource::Coincube,
                Ok(ListCurrenciesResult {
                    currencies: vec![Currency::USD, Currency::EUR],
                }),
            )),
        );
        assert_eq!(state.currencies, vec![Currency::USD, Currency::EUR]);
        assert!(state.error.is_none());

        let _ = state.update(
            None,
            &Cache::default(),
            Message::Fiat(FiatMessage::ListCurrenciesResult(
                PriceSource::Coincube,
                Err(PriceApiError::RequestFailed("timeout".to_string())),
            )),
        );
        assert!(state.error.is_some());
    }

    #[test]
    fn fiat_validation_falls_back_or_errors_when_currency_unavailable() {
        let mut state = state();
        state.new_price_setting.currency = Currency::EUR;
        state.currencies = vec![Currency::USD, Currency::GBP];

        let _ = state.update(
            None,
            &Cache::default(),
            Message::Fiat(FiatMessage::ValidateCurrencySetting),
        );
        assert_eq!(state.new_price_setting.currency, Currency::USD);

        state.new_price_setting.currency = Currency::EUR;
        state.currencies.clear();
        let _ = state.update(
            None,
            &Cache::default(),
            Message::Fiat(FiatMessage::ValidateCurrencySetting),
        );
        assert!(matches!(state.error, Some(Error::Unexpected(_))));
    }

    #[test]
    fn fiat_and_display_preference_messages_update_draft_state() {
        let mut state = state();
        state.new_price_setting.is_enabled = false;

        let _ = state.update(
            None,
            &Cache::default(),
            Message::View(view::Message::Settings(view::SettingsMessage::Fiat(
                view::FiatMessage::Enable(true),
            ))),
        );
        assert!(state.new_price_setting.is_enabled);

        let _ = state.update(
            None,
            &Cache::default(),
            Message::View(view::Message::Settings(view::SettingsMessage::Fiat(
                view::FiatMessage::SourceEdited(PriceSource::CoinGecko),
            ))),
        );
        assert_eq!(state.new_price_setting.source, PriceSource::CoinGecko);

        let _ = state.update(
            None,
            &Cache::default(),
            Message::View(view::Message::Settings(view::SettingsMessage::Fiat(
                view::FiatMessage::CurrencyEdited(Currency::GBP),
            ))),
        );
        assert_eq!(state.new_price_setting.currency, Currency::GBP);

        let _ = state.update(
            None,
            &Cache::default(),
            Message::View(view::Message::Settings(
                view::SettingsMessage::DisplayUnitChanged(BitcoinDisplayUnit::BTC),
            )),
        );
        assert_eq!(state.new_unit_setting.display_unit, BitcoinDisplayUnit::BTC);

        let _ = state.update(
            None,
            &Cache::default(),
            Message::View(view::Message::Settings(
                view::SettingsMessage::ToggleDirectionBadges(false),
            )),
        );
        assert!(!state.show_direction_badges);
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

/// Which face of the settings-content state to render. The General and Recovery
/// sub-tabs share this one `State` type (they both own the local backup flow and
/// flow through the same wrapper downcast); this discriminator selects the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    /// App/display preferences (network, unit, display mode, badges, fiat).
    General,
    /// Backup & recovery: local paper backup + Recovery Kit + Vault alerts.
    Recovery,
}

pub struct GeneralSettingsState {
    cube_id: String,
    /// Which sub-tab's content this instance renders (General prefs vs Recovery).
    section: SettingsSection,
    new_price_setting: PriceSetting,
    new_unit_setting: UnitSetting,
    currencies: Vec<Currency>,
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
        section: SettingsSection,
        price_setting: PriceSetting,
        unit_setting: UnitSetting,
        datadir_path: &CoincubeDirectory,
    ) -> Self {
        use crate::app::settings::global::GlobalSettings;
        let global_path = GlobalSettings::path(datadir_path);
        let show_direction_badges = GlobalSettings::load_show_direction_badges(&global_path);
        Self {
            cube_id,
            section,
            new_price_setting: price_setting,
            new_unit_setting: unit_setting,
            currencies: Vec::new(),
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
                // Passkey Cubes derive their mnemonic from the WebAuthn PRF
                // output — there is no encrypted mnemonic on disk and no PIN.
                // Re-derive it through an assertion, which is the passkey
                // path's exact analogue of the PIN re-prompt below: the open
                // Cube's signer is already in `app::session`, so neither
                // prompt is fetching something we lack. Both are asking for
                // consent to put the master seed on screen.
                if let Some(cube) = self.lookup_cube(cache) {
                    if cube.is_passkey_cube() {
                        self.backup_mnemonic = None;
                        return match crate::services::passkey::reauth::begin(&cube) {
                            Ok(fut) => {
                                self.backup_state = BackupSeedState::PasskeyReauth;
                                Task::perform(fut, |res| {
                                    Message::View(view::Message::Settings(
                                        view::SettingsMessage::BackupMasterSeed(
                                            view::BackupWalletMessage::PinVerified(
                                                res.map_err(|e| e.user_message()),
                                            ),
                                        ),
                                    ))
                                })
                            }
                            Err(e) => {
                                self.backup_state = BackupSeedState::Error(e.user_message());
                                Task::none()
                            }
                        };
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
                let cube_id = cube.id.clone();

                // This is the *second* door to the same secret, and it is the
                // one that hands over the permanent one. Reaching it needs an
                // unlocked Cube, so it is not the laptop-thief case the unlock
                // throttle was built for — but someone at an unattended,
                // already-open Cube who wants the seed phrase rather than the
                // session's balance would otherwise get unlimited guesses here.
                // Share the unlock counter: it is the same PIN and the same
                // Cube, so guesses should accumulate across both surfaces.
                let throttle_root = datadir.clone();
                let remaining = unlock::throttle::ThrottleState::load(&throttle_root)
                    .remaining_lockout(&cube_id);
                if !remaining.is_zero() {
                    return Task::done(Message::View(view::Message::Settings(
                        view::SettingsMessage::BackupMasterSeed(
                            view::BackupWalletMessage::PinVerified(Err(
                                unlock::throttle::lockout_message(remaining),
                            )),
                        ),
                    )));
                }

                // Run Argon2id PIN verification + mnemonic decryption off
                // the UI thread to avoid blocking the event loop.
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            // `load_mnemonic_words` is the PIN check: the PIN
                            // the unlock authenticated when a session can
                            // answer, the seed file's GCM tag when it cannot.
                            // The `verify_pin` call that used to gate this was
                            // the cheap Argon2 oracle (m=19 MiB vs the seed
                            // file's 256 MiB) and is gone; see
                            // PLAN-cube-unlock-hardening I1.
                            let mut throttle =
                                unlock::throttle::ThrottleState::load(&throttle_root);
                            match load_mnemonic_words(
                                &datadir,
                                network,
                                fingerprint,
                                &pin,
                                &cube_id,
                            ) {
                                Ok(words) => {
                                    throttle.record_success(&throttle_root, &cube_id);
                                    // Wrap here, not after delivery: from this
                                    // point on every copy the message queue
                                    // makes is scrubbed on drop.
                                    Ok(Zeroizing::new(words))
                                }
                                // Only a wrong PIN is a guess. An operational
                                // failure keeps its own message and costs the
                                // user nothing against the throttle.
                                Err(e) if !is_wrong_pin(&e) => Err(e.to_string()),
                                Err(_) => {
                                    let penalty = throttle.record_failure(&throttle_root, &cube_id);
                                    Err(if penalty.is_zero() {
                                        "Incorrect PIN. Please try again.".to_string()
                                    } else {
                                        unlock::throttle::lockout_message(penalty)
                                    })
                                }
                            }
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
                // Shared by both unlock shapes: a verified PIN and a completed
                // assertion both arrive here holding the same words.
                //
                // Only the two states that *asked* for a verification may
                // consume one — same guard `VerifyPin` applies above. A stale
                // result is not hypothetical: `PreviousStep` cancels a live
                // ceremony, which wakes the waiting task with `Cancelled`, and
                // the authenticator can answer in the same instant the user
                // backs out. Acting on either would reopen a wizard the user
                // just left, and on the `Ok` path that means re-entering the
                // flow that puts the master seed on screen, unprompted. The
                // `Err` path was no better: with the state already back at
                // `None`, `from_passkey` read `false` and a cancelled Touch ID
                // prompt landed a passkey Cube on a PIN keypad it has no PIN
                // for.
                let from_passkey = match &self.backup_state {
                    BackupSeedState::PasskeyReauth => true,
                    BackupSeedState::PinEntry { .. } => false,
                    _ => return Task::none(),
                };
                if from_passkey {
                    // The ceremony is over; drop the parked controller.
                    crate::services::passkey::reauth::cancel();
                }
                match result {
                    Ok(words) => {
                        self.backup_pin.clear();
                        self.backup_mnemonic = Some(words);
                        self.backup_state = BackupSeedState::Intro(false);
                    }
                    Err(e) => {
                        self.backup_pin.clear();
                        self.backup_state = if from_passkey {
                            // No keypad to return to — the prompt was the
                            // system's. `e` is already I12-compliant copy from
                            // `PasskeyError::user_message`.
                            BackupSeedState::Error(e)
                        } else {
                            BackupSeedState::PinEntry { error: Some(e) }
                        };
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
                                saving: false,
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
                    // Backing out of a live ceremony: dropping the parked
                    // controller dismisses the system sheet and wakes the
                    // waiting task, whose late result the `PinVerified` arm
                    // then ignores because the state has moved on.
                    BackupSeedState::PasskeyReauth => {
                        crate::services::passkey::reauth::cancel();
                        BackupSeedState::None
                    }
                    BackupSeedState::Error(_) => BackupSeedState::None,
                    BackupSeedState::None => BackupSeedState::None,
                };
                Task::none()
            }
            BackupWalletMessage::WordInput { index, input } => {
                if let BackupSeedState::Verification {
                    word_indices,
                    word_inputs,
                    error,
                    saving,
                } = &self.backup_state
                {
                    // Ignore edits while the async save is in flight.
                    if *saving {
                        return Task::none();
                    }
                    let mut new_inputs = word_inputs.clone();
                    if let Some(pos) = word_indices.iter().position(|&i| i == index as usize) {
                        new_inputs[pos] = input;
                    }
                    self.backup_state = BackupSeedState::Verification {
                        word_indices: *word_indices,
                        word_inputs: new_inputs,
                        error: error.clone(),
                        saving: false,
                    };
                }
                Task::none()
            }
            BackupWalletMessage::VerifyPhrase => {
                let BackupSeedState::Verification {
                    word_indices,
                    word_inputs,
                    saving,
                    ..
                } = &self.backup_state
                else {
                    return Task::none();
                };
                // Ignore duplicate clicks while the async save is in flight.
                if *saving {
                    return Task::none();
                }
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
                    // Verification passed — mark the state as saving so the
                    // Verify button is disabled, then persist
                    // `backed_up = true` to settings.json. The async result is
                    // handled by `BackupSaveResult` below: success transitions
                    // to Completed via `BackupMasterSeedUpdated`, failure
                    // restores the verification screen with an error message.
                    let word_indices = *word_indices;
                    let word_inputs = word_inputs.clone();
                    self.backup_state = BackupSeedState::Verification {
                        word_indices,
                        word_inputs,
                        error: None,
                        saving: true,
                    };
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
                        |res: Result<(), String>| {
                            Message::View(view::Message::Settings(
                                view::SettingsMessage::BackupMasterSeed(
                                    view::BackupWalletMessage::BackupSaveResult(res),
                                ),
                            ))
                        },
                    )
                } else {
                    self.backup_state = BackupSeedState::Verification {
                        word_indices: *word_indices,
                        word_inputs: word_inputs.clone(),
                        error: Some(
                            "The words you entered don't match. Please try again.".to_string(),
                        ),
                        saving: false,
                    };
                    Task::none()
                }
            }
            BackupWalletMessage::BackupSaveResult(res) => {
                // The async settings.json write completed. On success, fan out
                // `BackupMasterSeedUpdated` so the App-level interceptor can
                // refresh `cache.current_cube_backed_up` and the global
                // settings panel transitions to Completed (which clears the
                // mnemonic). On failure, restore the verification screen with
                // an inline error and surface a top-level toast.
                match res {
                    Ok(()) => Task::done(Message::View(view::Message::Settings(
                        view::SettingsMessage::BackupMasterSeedUpdated,
                    ))),
                    Err(e) => {
                        if let BackupSeedState::Verification {
                            word_indices,
                            word_inputs,
                            ..
                        } = &self.backup_state
                        {
                            self.backup_state = BackupSeedState::Verification {
                                word_indices: *word_indices,
                                word_inputs: word_inputs.clone(),
                                error: Some(format!("Failed to save backup status: {}", e)),
                                saving: false,
                            };
                        }
                        Task::done(Message::View(view::Message::ShowError(e)))
                    }
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

/// This Cube's master seed words, gated on `pin`.
///
/// # Where the words come from
///
/// The session cache first, exactly as the Liquid and Spark loaders do. The
/// unlock that opened this Cube already paid the ~831 ms Argon2id pass and is
/// holding the decrypted signer, so re-reading the seed file buys nothing.
/// Reading from disk is the fallback for the entry points that have no session
/// — a Cube the installer just restored lands in the app without one.
///
/// # What checks the PIN
///
/// Not the decryption, on the fast path: there is no decryption on it.
/// `session::unlocked_signer_with_pin_verification` compares against the PIN the
/// unlock authenticated, under the session lock, which proves the same thing —
/// nothing is handed back without the PIN that opened this Cube. What it does
/// *not* do is cost 831 ms a guess, so the escalating lockout in
/// `services::unlock::throttle` is what rate-limits this surface. Every caller
/// checks it before calling here and charges it on every wrong PIN; see
/// [`is_wrong_pin`].
///
/// A wrong PIN short-circuits rather than falling through to disk. Falling
/// through would pay a full Argon2id pass to re-derive an answer the session has
/// already given, and would report `InvalidPassword` either way.
///
/// The disk fallback goes through `services::unlock`, **not**
/// `MasterSigner::from_datadir_by_fingerprint`: `coincube-core` has no keystore
/// access, so it answers `DeviceSecretRequired` for every `ENCRYPTED_V3` file —
/// which, once the Tier 1 migration has run, is every Cube. Using it here is
/// what made every one of these flows fail with a keychain error.
///
/// Visible to sibling modules under `settings::` — `recovery_kit.rs` and
/// `recovery_alerts.rs` reuse it for the same PIN → mnemonic path. Kept here
/// rather than hoisted to `mod.rs` because this is where the backup flow's PIN
/// gate already lives.
pub(super) fn load_mnemonic_words(
    datadir: &std::path::Path,
    network: Network,
    fingerprint: Fingerprint,
    pin: &str,
    cube_id: &str,
) -> Result<Vec<String>, SignerError> {
    let signer =
        crate::app::session::unlocked_signer_with_pin_verification(cube_id, fingerprint, pin)
            .or_else(|e| {
                // A wrong PIN is a wrong PIN. Keep it, so the caller's throttle
                // records the guess rather than reporting an operational fault.
                if matches!(e, SignerError::InvalidPassword) {
                    return Err(e);
                }
                // No session, or none holding this key.
                crate::services::unlock::open_seed_by_fingerprint(
                    datadir,
                    network,
                    fingerprint,
                    pin,
                    cube_id,
                )
            })?;

    Ok(signer.words().iter().map(|w| (*w).to_string()).collect())
}

/// Whether a failed seed decryption means **the user typed the wrong PIN**.
///
/// Only `InvalidPassword` does: `seed_crypt::decrypt_with` returns it for a
/// failed GCM tag and nothing else, and deliberately does *not* use it for a
/// missing device secret (invariant I7). Everything else — an unreadable
/// mnemonics folder, no file for this fingerprint, a v3 file whose keychain
/// entry is unavailable — is an operational fault the PIN cannot fix.
///
/// The distinction has teeth on both sides. Reporting a locked keychain as
/// "Incorrect PIN" sends the user to retype a PIN that was right, and each
/// retry spends a guess against the shared unlock throttle — so a fault
/// entirely outside their control locks them out of their own Cube.
pub(super) fn is_wrong_pin(e: &SignerError) -> bool {
    matches!(e, SignerError::InvalidPassword)
}

impl GeneralSettingsState {
    /// Variant of `view` that also threads the Cube Recovery Kit
    /// status through so the "Back up your Recovery Kit" card can
    /// render. `SettingsState::view` (the outer wrapper) calls this
    /// after downcasting via `as_any`; the plain `State::view` impl
    /// below falls back to rendering without the card for callers
    /// that don't have the recovery-kit data on hand.
    pub fn view_with_recovery_kit<'a>(
        &'a self,
        menu: &'a Menu,
        cache: &'a Cache,
        rk: &'a super::recovery_kit::RecoveryKit,
        ra: &'a super::recovery_alerts::RecoveryAlerts,
    ) -> Element<'a, view::Message> {
        crate::app::view::settings::general::settings_content_section(
            self.section,
            menu,
            cache,
            &self.new_price_setting,
            &self.new_unit_setting,
            &self.currencies,
            self.show_direction_badges,
            &self.backup_state,
            &self.backup_pin,
            self.backup_mnemonic.as_deref().map(|v| v.as_slice()),
            Some(rk),
            Some(ra),
        )
    }
}

impl State for GeneralSettingsState {
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        crate::app::view::settings::general::settings_content_section(
            self.section,
            menu,
            cache,
            &self.new_price_setting,
            &self.new_unit_setting,
            &self.currencies,
            self.show_direction_badges,
            &self.backup_state,
            &self.backup_pin,
            self.backup_mnemonic.as_deref().map(|v| v.as_slice()),
            None,
            None,
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
