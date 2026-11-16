use std::sync::Arc;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
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
    network: impl Into<crate::chain::ChainId>,
    cube_id: String,
    mut new_price_setting: PriceSetting,
) -> Result<(), Error> {
    let network = network.into();
    if network.is_blake2b() {
        new_price_setting.source = crate::services::fiat::PriceSource::Coincube;
    }
    let network_dir = data_dir.network_directory(network);
    let mut cube_found = false;
    let result = update_settings_file(&network_dir, |mut settings| {
        if let Some(cube) = settings
            .cubes
            .iter_mut()
            .find(|c| c.id == cube_id && c.network == network)
        {
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
    #[tokio::test]
    async fn backup_metadata_keeps_twin_cubes_separate() {
        use crate::{chain::ChainId, dir::CoincubeDirectory};
        let root_path = std::env::temp_dir().join(format!("backup-meta-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(root_path.clone());
        for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
            let cube = settings::CubeSettings::new_with_raw_id(
                "same-id".into(),
                chain.api_str().into(),
                chain,
            );
            crate::app::settings::update_settings_file(
                &root.network_directory(chain),
                move |mut s| {
                    s.cubes.push(cube);
                    Some(s)
                },
            )
            .await
            .unwrap();
        }
        let mut cache = Cache {
            datadir_path: root.clone(),
            fiat_chain: ChainId::BitcoinBlake2b,
            ..Cache::default()
        };
        let state = GeneralSettingsState::new(
            "same-id".into(),
            SettingsSection::General,
            PriceSetting::default(),
            UnitSetting::default(),
            &root,
        );
        assert_eq!(
            state.lookup_cube(&cache).map(|c| c.name).as_deref(),
            Some("bitcoin-blake2b")
        );
        cache.fiat_chain = ChainId::BitcoinBlake2bTestnet4;
        assert!(state.lookup_cube(&cache).map(|c| c.name).is_none());
        std::fs::remove_dir_all(root_path).unwrap();
    }

    use crate::services::fiat::{
        api::{ListCurrenciesResult, PriceApiError},
        PriceSource,
    };
    use coincube_ui::component::amount::BitcoinDisplayUnit;

    #[test]
    fn fork_backup_never_uses_bitcoin_session_or_seed() {
        use crate::chain::ChainId;
        use coincube_core::signer::MasterSigner;
        let _guard = crate::app::session::test_guard();
        crate::app::session::close();
        let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::signing_only();
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let root = std::env::temp_dir().join(format!("backup-chain-{}", uuid::Uuid::new_v4()));
            let id = uuid::Uuid::new_v4().to_string();
            let signer = MasterSigner::generate(chain.bitcoin_network()).unwrap();
            let fp = signer.fingerprint(&secp);
            signer
                .store_encrypted(
                    &root,
                    chain.bitcoin_network(),
                    &secp,
                    None,
                    "1234",
                    &id,
                    None,
                )
                .unwrap();
            crate::app::session::open(&id, zeroize::Zeroizing::new("1234".to_string()));
            crate::app::session::store_unlocked_signer(&id, fp, signer.try_clone().unwrap());
            assert!(load_mnemonic_words(&root, chain.bitcoin_network(), fp, "1234", &id).is_ok());
            assert!(load_mnemonic_words(&root, chain, fp, "1234", &id).is_err());
            signer
                .store_encrypted_for_chain(&root, chain, &secp, None, "5678", &id, None)
                .unwrap();
            assert!(load_mnemonic_words(&root, chain, fp, "1234", &id).is_err());
            assert_eq!(
                load_mnemonic_words(&root, chain, fp, "5678", &id)
                    .unwrap()
                    .join(" "),
                signer.words().join(" ")
            );
            crate::app::session::close();
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test]
    async fn btcb2_price_preferences_never_write_bitcoin_settings() {
        use crate::chain::ChainId;
        let temp = std::env::temp_dir().join(format!(
            "coincube-btcb2-price-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let root = CoincubeDirectory::new(temp.clone());
        for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
            let cube =
                settings::CubeSettings::new_with_raw_id("same-id".into(), "Fixture".into(), chain);
            update_settings_file(&root.network_directory(chain), move |mut value| {
                value.cubes.push(cube);
                Some(value)
            })
            .await
            .unwrap();
        }
        let bitcoin_path = root
            .network_directory(ChainId::Bitcoin)
            .path()
            .join(settings::SETTINGS_FILE_NAME);
        let before = std::fs::read(&bitcoin_path).unwrap();
        update_price_setting(
            root.clone(),
            ChainId::BitcoinBlake2b,
            "same-id".into(),
            PriceSetting {
                source: PriceSource::CoinGecko,
                currency: Currency::EUR,
                is_enabled: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(bitcoin_path).unwrap(), before);
        let persisted =
            settings::Settings::from_file(&root.network_directory(ChainId::BitcoinBlake2b))
                .unwrap();
        let price = persisted.cubes[0].fiat_price.as_ref().unwrap();
        assert_eq!(price.currency, Currency::EUR);
        assert_eq!(price.source, PriceSource::Coincube);
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn btcb2_source_edits_cannot_select_an_aggregator() {
        let mut state = state();
        let cache = Cache {
            fiat_chain: crate::chain::ChainId::BitcoinBlake2b,
            ..Default::default()
        };
        let _ = state.update(
            None,
            &cache,
            Message::View(view::Message::Settings(view::SettingsMessage::Fiat(
                view::FiatMessage::SourceEdited(PriceSource::CoinGecko),
            ))),
        );
        assert_eq!(state.new_price_setting.source, PriceSource::Coincube);
    }

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
    fn backup_stale_seed_result_after_cancel_cannot_reopen_flow() {
        let mut state = state();
        let attempt = crate::app::cache::AppGeneration::next();
        state.backup_state = BackupSeedState::PinEntry { error: None };
        state.backup_pin_attempt = Some(attempt);
        state.backup_pin.digits = ["9".into(), "9".into(), "9".into(), "9".into()];
        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::PreviousStep),
        );
        let _ = state.update(
            None,
            &Cache::default(),
            backup_msg(view::BackupWalletMessage::PinVerifiedAttempt(
                attempt,
                Ok(Zeroizing::new(words())),
            )),
        );
        assert_eq!(state.backup_state, BackupSeedState::None);
        assert!(state.backup_pin_attempt.is_none());
        assert!(state.backup_mnemonic.is_none());
        assert_eq!(state.backup_pin.value().as_str(), "");
    }

    #[test]
    fn backup_second_submit_cannot_start_another_pin_trial() {
        let mut state = state();
        let attempt = crate::app::cache::AppGeneration::next();
        state.backup_state = BackupSeedState::PinEntry { error: None };
        state.backup_pin_attempt = Some(attempt);
        state.backup_pin.digits = ["1".into(), "2".into(), "3".into(), "4".into()];
        assert_eq!(
            state
                .update(
                    None,
                    &Cache::default(),
                    backup_msg(view::BackupWalletMessage::VerifyPin)
                )
                .units(),
            0
        );
        assert_eq!(state.backup_pin_attempt, Some(attempt));
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

        // The inline error keeps the user informed without echoing the
        // underlying failure ("disk full" is the tame case; the same path
        // carries IO paths and keyring errors). The detail goes to the log
        // under the reference shown here.
        let BackupSeedState::Verification {
            error: Some(error),
            saving: false,
            ..
        } = &state.backup_state
        else {
            panic!(
                "expected a verification error, got {:?}",
                state.backup_state
            );
        };
        assert!(
            !error.contains("disk full"),
            "raw detail reached the user: {}",
            error
        );
        assert!(
            error.contains(crate::user_error::CC_CONFIG),
            "no reference: {}",
            error
        );
        assert!(
            error.starts_with("Couldn't record that your seed is backed up"),
            "got {}",
            error
        );
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
    network: impl Into<crate::chain::ChainId>,
    cube_id: String,
    new_unit_setting: UnitSetting,
) -> Result<(), Error> {
    let network = network.into();
    let network_dir = data_dir.network_directory(network);
    let mut cube_found = false;
    let result = update_settings_file(&network_dir, |mut settings| {
        if let Some(cube) = settings
            .cubes
            .iter_mut()
            .find(|c| c.id == cube_id && c.network == network)
        {
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
    PaymentChecks,
    DataExports,
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
    saved: bool,
    /// The Cube name as typed on Display & appearance, until it is saved.
    /// `None` shows the Cube's current name.
    name_input: Option<String>,
    /// Payment checks is asking before turning recipient checks off.
    confirm_checks_off: bool,
    error: Option<Error>,
    /// Master seed backup flow state.
    pub backup_state: BackupSeedState,
    /// PIN re-entry input for the backup flow's PinEntry state.
    /// Held as a separate field because `PinInput` doesn't implement
    /// `Debug`/`Clone`/`PartialEq` (required by `BackupSeedState`).
    pub backup_pin: PinInput,
    /// Exact in-flight PIN attempt. A cancelled prompt cannot activate a
    /// later prompt if its blocking duress result arrives late.
    backup_pin_attempt: Option<crate::app::cache::AppGeneration>,
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
            saved: false,
            name_input: None,
            confirm_checks_off: false,
            error: None,
            backup_state: BackupSeedState::None,
            backup_pin: PinInput::new(),
            backup_pin_attempt: None,
            backup_mnemonic: None,
        }
    }

    /// Look up this Cube in the settings file on disk.
    ///
    /// Returns the stored `CubeSettings` (which contains the master signer
    /// fingerprint and PIN hash) or `None` if the cube can't be found.
    fn lookup_cube(&self, cache: &Cache) -> Option<settings::CubeSettings> {
        let network_dir = cache.datadir_path.network_directory(cache.chain());
        let settings = settings::Settings::from_file(&network_dir).ok()?;
        settings
            .cubes
            .into_iter()
            .find(|c| c.id == self.cube_id && c.network == cache.chain())
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
                self.backup_pin_attempt = None;
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
                if self.backup_pin_attempt.is_some() {
                    return Task::none();
                }
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
                let network = cache.chain();
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
                let attempt = crate::app::cache::AppGeneration::next();
                self.backup_pin_attempt = Some(attempt);
                let app_generation = cache.app_generation;
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
                            unlock::throttle::classify_seed_export_cube(
                                &throttle_root,
                                &cube,
                                app_generation,
                                &pin,
                                || {
                                    load_mnemonic_words(
                                        &datadir,
                                        network,
                                        fingerprint,
                                        &pin,
                                        &cube_id,
                                    )
                                },
                            )
                        })
                        .await
                        .map_err(|e| format!("PIN verification task failed: {}", e))?
                    },
                    move |res| {
                        let outcome = match res {
                            Ok(unlock::throttle::SeedExportCubeAttempt::Words(words)) => {
                                BackupWalletMessage::PinVerifiedAttempt(attempt, Ok(words))
                            }
                            Ok(unlock::throttle::SeedExportCubeAttempt::Duress(proof)) => {
                                return Message::View(view::Message::DuressActivateLocal(proof));
                            }
                            Ok(unlock::throttle::SeedExportCubeAttempt::Wrong(penalty)) => {
                                BackupWalletMessage::PinVerifiedAttempt(
                                    attempt,
                                    Err(if penalty.is_zero() {
                                        "Incorrect PIN. Please try again.".into()
                                    } else {
                                        unlock::throttle::lockout_message(penalty)
                                    }),
                                )
                            }
                            Ok(unlock::throttle::SeedExportCubeAttempt::Locked(remaining)) => {
                                BackupWalletMessage::PinVerifiedAttempt(
                                    attempt,
                                    Err(unlock::throttle::lockout_message(remaining)),
                                )
                            }
                            Err(error) => {
                                BackupWalletMessage::PinVerifiedAttempt(attempt, Err(error))
                            }
                        };
                        Message::View(view::Message::Settings(
                            view::SettingsMessage::BackupMasterSeed(outcome),
                        ))
                    },
                )
            }
            BackupWalletMessage::PinVerifiedAttempt(attempt, result) => {
                if self.backup_pin_attempt != Some(attempt)
                    || !matches!(self.backup_state, BackupSeedState::PinEntry { .. })
                {
                    return Task::none();
                }
                self.handle_backup_message(cache, BackupWalletMessage::PinVerified(result))
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
                self.backup_pin_attempt = None;
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
                self.backup_pin_attempt = None;
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
                    let network = cache.chain();
                    let datadir = cache.datadir_path.clone();
                    Task::perform(
                        async move {
                            let network_dir = datadir.network_directory(network);
                            update_settings_file(&network_dir, |mut s| {
                                if let Some(cube) = s
                                    .cubes
                                    .iter_mut()
                                    .find(|c| c.id == cube_id && c.network == network)
                                {
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
                        let msg = crate::user_error::UserError::logged(
                            "Couldn't record that your seed is backed up",
                            "Your seed is safe — only the reminder didn't save. Try again.",
                            crate::user_error::CC_CONFIG,
                            true,
                            &e,
                        )
                        .toast();
                        if let BackupSeedState::Verification {
                            word_indices,
                            word_inputs,
                            ..
                        } = &self.backup_state
                        {
                            self.backup_state = BackupSeedState::Verification {
                                word_indices: *word_indices,
                                word_inputs: word_inputs.clone(),
                                error: Some(msg.clone()),
                                saving: false,
                            };
                        }
                        Task::done(Message::View(view::Message::ShowError(msg)))
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
/// Always the seed file for this chain. A Bitcoin Blake2b Cube opens its
/// chain-bound seed. A Cube the installer just restored has no session, and an
/// already-unlocked Cube still re-derives so the PIN check is not cheaper than
/// unlock.
///
/// # What checks the PIN
///
/// The seed file. Every guess pays a full Argon2id trial decryption, including
/// when this Cube is already unlocked. The session cache is deliberately not
/// consulted: comparing the cached PIN is microseconds and would turn Backup
/// and Recovery Kit into a cheap oracle for a short PIN. The escalating lockout
/// in `services::unlock::throttle` still applies. Backup and Recovery Kit
/// callers run this inside its serialized check/trial/update helper; see
/// [`is_wrong_pin`].
///
/// The disk read goes through `services::unlock`, **not**
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
    network: impl Into<crate::chain::ChainId>,
    fingerprint: Fingerprint,
    pin: &str,
    cube_id: &str,
) -> Result<Vec<String>, SignerError> {
    let network = network.into();
    // Seed reveal always trial-decrypts the seed file. The session cache can
    // answer in microseconds, which would let someone at an unlocked Cube grind
    // a short PIN against Backup and Recovery Kit. One Argon2id pass per guess
    // is the same cost as unlock. Callers still charge `InvalidPassword` to the
    // UI throttle.
    let signer = crate::services::unlock::open_seed_by_fingerprint(
        datadir,
        network,
        fingerprint,
        pin,
        cube_id,
    )?;

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
#[cfg(test)]
pub(super) fn is_wrong_pin(e: &SignerError) -> bool {
    matches!(e, SignerError::InvalidPassword)
}

impl GeneralSettingsState {
    /// A typed Cube name that differs from the saved one, to save when the
    /// field is left by moving to another Settings section.
    pub fn pending_cube_name(&self, current: &str) -> Option<String> {
        self.name_input
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty() && *name != current)
            .map(str::to_owned)
    }

    /// The Display & appearance content without the dashboard, so the App
    /// can show the avatar flow inline beneath Cube identity.
    pub fn display_content<'a>(
        &'a self,
        cache: &'a Cache,
        avatar_flow: Option<Element<'a, view::Message>>,
    ) -> Element<'a, view::Message> {
        crate::app::view::settings::general::display_content(
            cache,
            &self.new_price_setting,
            &self.new_unit_setting,
            &self.currencies,
            self.show_direction_badges,
            self.saved,
            self.name_input.as_deref(),
            avatar_flow,
        )
    }

    pub fn section(&self) -> SettingsSection {
        self.section
    }

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
            self.saved,
            &self.backup_state,
            &self.backup_pin,
            self.backup_mnemonic.as_deref().map(|v| v.as_slice()),
            Some(rk),
            Some(ra),
            self.name_input.as_deref(),
            self.confirm_checks_off,
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
            self.saved,
            &self.backup_state,
            &self.backup_pin,
            self.backup_mnemonic.as_deref().map(|v| v.as_slice()),
            None,
            None,
            self.name_input.as_deref(),
            self.confirm_checks_off,
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
        if matches!(
            &message,
            Message::Fiat(FiatMessage::SaveChanges)
                | Message::View(view::Message::Settings(
                    view::SettingsMessage::DisplayUnitChanged(_)
                        | view::SettingsMessage::ToggleDirectionBadges(_)
                        | view::SettingsMessage::ToggleRecipientIdentityChecks(_)
                ))
        ) {
            self.saved = false;
        }
        match message {
            // The rename reports its own progress under the Cube name field
            // (`Cache::cube_name_sync`), not on Amounts & currency's "Saved".
            Message::View(view::Message::Settings(view::SettingsMessage::CubeNameEdited(name))) => {
                self.name_input = Some(name);
                Task::none()
            }
            // The App saved the name; show the Cube's name again.
            Message::View(view::Message::Settings(view::SettingsMessage::CubeNameSubmitted(_))) => {
                self.name_input = None;
                Task::none()
            }
            Message::Fiat(FiatMessage::SaveChanges) => {
                self.error = None;
                tracing::info!(
                    "Saving cube fiat price setting: {:?}",
                    self.new_price_setting
                );
                let price_setting = self.new_price_setting.clone();
                let network = cache.chain();
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
                self.saved = true;
                tracing::info!("GeneralSettingsState: SettingsSaved received");
                self.error = None;
                // Reload unit setting from disk to sync toggle state with what was saved
                let network_dir = cache.datadir_path.network_directory(cache.chain());
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
                    if let Some(cube) = settings
                        .cubes
                        .iter()
                        .find(|c| c.id == self.cube_id && c.network == cache.chain())
                    {
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
                self.saved = false;
                let err_msg = crate::user_error::report(&e);
                self.error = Some(e);
                // Show error in global toast
                let toast_task = Task::done(Message::View(view::Message::ShowError(err_msg)));
                // Reload settings from disk to revert toggle state to persisted value
                let network_dir = cache.datadir_path.network_directory(cache.chain());
                if let Ok(settings) = crate::app::settings::Settings::from_file(&network_dir) {
                    if let Some(cube) = settings
                        .cubes
                        .iter()
                        .find(|c| c.id == self.cube_id && c.network == cache.chain())
                    {
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
                        let err_msg = crate::user_error::report(&err);
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
                        let err_msg = crate::user_error::report(&err);
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
                        self.new_price_setting.source = if cache.fiat_chain.is_blake2b() {
                            crate::services::fiat::PriceSource::Coincube
                        } else {
                            source
                        };
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
                let network = cache.chain();
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
                view::SettingsMessage::AskRecipientChecksOff,
            )) => {
                self.confirm_checks_off = true;
                Task::none()
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::CancelRecipientChecksOff,
            )) => {
                self.confirm_checks_off = false;
                Task::none()
            }
            Message::View(view::Message::Settings(
                view::SettingsMessage::ToggleRecipientIdentityChecks(enabled),
            )) => {
                self.confirm_checks_off = false;
                // Enforce privacy before touching disk. Keep writes ordered with
                // user actions so rapid toggles cannot persist an older choice.
                crate::services::branta::set_enabled(enabled);
                use crate::app::settings::global::GlobalSettings;
                match GlobalSettings::update_recipient_identity_checks(
                    &GlobalSettings::path(&cache.datadir_path), enabled,
                ) {
                    Ok(()) => {
                        self.saved = true;
                        Task::none()
                    }
                    Err(_) => Task::done(Message::View(view::Message::ShowError(
                        "Couldn't save your privacy preference. Your choice applies for this session; please try again before restarting.".into(),
                    ))),
                }
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
