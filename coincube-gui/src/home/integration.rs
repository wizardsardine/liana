//! Opt-in driver for the real PIN Cube creation and backup ceremony.
//! The caller owns a durable private backup before invoking this driver.

use super::*;
use iced_runtime::futures::futures::StreamExt;

async fn drain(task: Task<Message>) -> Vec<Message> {
    match iced_runtime::task::into_stream(task) {
        Some(stream) => {
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_runtime::Action::Output(message) => Some(message),
                        _ => None,
                    }
                })
                .collect()
                .await
        }
        None => Vec::new(),
    }
}

async fn admitted_recovery_home(
    chain: ChainId,
    root: &CoincubeDirectory,
    client: Option<CoincubeClient>,
) -> Result<Home, &'static str> {
    let (mut home, _startup) = Home::new_for_chain(root.clone(), Some(chain));
    if let Some(client) = client {
        // Reuse the normal admitted-session initialization. In particular the
        // account feature grant is not the anonymous global rollout flag.
        let admitted = home.adopt_connect_client(client.clone());
        for message in drain(admitted).await {
            // This is the actual AdmittedUserLoaded response. Its follow-up
            // task batch includes unrelated global device-registration work;
            // do not execute it against the host's active personal profile.
            drop(home.update(message));
        }
        let features = client
            .get_connect_features()
            .await
            .map_err(|_| "recovery_account_features")?;
        let generation = home.connect_account.session_generation();
        drop(home.update(Message::View(ViewMessage::ConnectAccount(
            ConnectAccountMessage::FeaturesLoaded(Some(features), generation),
        ))));
        let duress = client
            .get_duress_state()
            .await
            .map_err(|_| "recovery_account_duress")?;
        if duress.active {
            return Err("recovery_account_duress");
        }
        drop(home.update(Message::View(ViewMessage::ConnectAccount(
            ConnectAccountMessage::DuressStateChecked(
                crate::services::coincube::DuressCheckOutcome::Ok(duress),
                generation,
                0,
            ),
        ))));
        if !home.connect_account.is_authenticated() {
            return Err("recovery_account_unadmitted");
        }
    } else if chain.is_blake2b() {
        return Err("recovery_account_required");
    }
    if home.network != chain || !home.chain_availability(chain).is_available() {
        return Err("recovery_chain_unavailable");
    }
    Ok(home)
}

/// Run the normal creation and backup state machine for a previously absent Cube.
#[allow(clippy::too_many_arguments)]
async fn create_new_testnet4_cube(
    chain: ChainId,
    root: &CoincubeDirectory,
    id: uuid::Uuid,
    name: String,
    pin: &str,
    saved_words: &[String],
    server_limit: usize,
    existing: Vec<CubeSettings>,
    client: Option<CoincubeClient>,
) -> Result<CubeSettings, &'static str> {
    let mut home = admitted_recovery_home(chain, root, client).await?;
    home.state = State::Cubes {
        cubes: existing,
        create_cube: true,
        source: chain,
    };
    home.server_cube_limit = Some(server_limit);
    home.passkey_mode = false;
    home.pending_cube_id = Some(id);
    home.create_cube_name.value = name;
    for (index, digit) in pin.chars().enumerate() {
        let message = pin_input::Message::DigitChanged(index, digit.to_string());
        let _ = home.create_cube_pin.update(message.clone());
        let _ = home.create_cube_pin_confirm.update(message);
    }
    let _ = drain(home.update(Message::View(ViewMessage::CreateCube))).await;
    if !matches!(
        home.state,
        State::CreationBackup(CreationBackupStep::Choice)
    ) {
        return Err("cube_creation_refused");
    }
    // Replace only the ephemeral pre-persistence words with the run's
    // already saved random phrase. This makes an interrupted finalize
    // retry use the same identity; no settings or sealed seed is replaced.
    let mnemonic = bip39::Mnemonic::parse_in(bip39::Language::English, saved_words.join(" "))
        .map_err(|_| "invalid_saved_backup")?;
    if mnemonic.word_count() != 12 {
        return Err("invalid_saved_backup");
    }
    home.creation_backup_words = Some(zeroize::Zeroizing::new(saved_words.to_vec()));
    let backup = |message| Message::View(ViewMessage::CreationBackup(message));
    let _ = home.update(backup(BackupWalletMessage::NextStep));
    let _ = home.update(backup(BackupWalletMessage::ToggleBackupIntroCheck));
    let _ = home.update(backup(BackupWalletMessage::NextStep));
    let _ = home.update(backup(BackupWalletMessage::NextStep));
    let State::CreationBackup(CreationBackupStep::Verification { word_indices, .. }) =
        home.state.clone()
    else {
        return Err("backup_verification_unavailable");
    };
    for index in word_indices {
        let _ = home.update(backup(BackupWalletMessage::WordInput {
            index: index as u8,
            input: saved_words[index - 1].clone(),
        }));
    }
    let verification = drain(home.update(backup(BackupWalletMessage::VerifyPhrase))).await;
    // Verification returns to the backup hub; only its explicit Continue
    // action persists the Cube. Never continue without the real challenge.
    if !verification.is_empty()
        || !home.creation_seed_confirmed
        || !matches!(
            home.state,
            State::CreationBackup(CreationBackupStep::Choice)
        )
    {
        return Err("backup_verification_failed");
    }
    let messages = drain(home.update(Message::View(ViewMessage::CreationBackupContinue))).await;
    let mut created = None;
    for message in messages {
        if let Message::CubeCreated(result) = message {
            created = Some(result.map_err(|_| "cube_persistence_failed")?);
        }
    }
    created.ok_or("cube_persistence_failed")
}

/// Admit normal Developer Mode only for an unused isolated QA profile.
/// Never overwrite an existing preference or adopt incomplete wallet state.
fn initialize_fresh_qa_preferences(root: &CoincubeDirectory) -> Result<(), &'static str> {
    let path = GlobalSettings::path(root);
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {
            return if GlobalSettings::load_developer_mode(&path) {
                Ok(())
            } else {
                Err("qa_developer_mode_disabled")
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err("qa_preferences_unreadable"),
    }
    for chain in ChainId::ALL {
        let directory = root.network_directory(chain);
        match std::fs::symlink_metadata(directory.path()) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            Ok(_) => return Err("qa_profile_not_unused"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err("qa_profile_unreadable"),
        }
        match settings::Settings::from_file(&directory) {
            // Fresh QA initialization does not adopt even an apparently empty
            // existing settings file. Its schema may contain unknown state.
            Ok(_) => return Err("qa_profile_not_unused"),
            Err(settings::SettingsError::NotFound) => (),
            Err(_) => return Err("qa_profile_unreadable"),
        }
        for entry in std::fs::read_dir(directory.path()).map_err(|_| "qa_profile_unreadable")? {
            let entry = entry.map_err(|_| "qa_profile_unreadable")?;
            let name = entry.file_name();
            let allowed = chain == ChainId::Testnet4
                && matches!(
                    name.to_str(),
                    Some("connect.json" | "connect_transport_key")
                );
            let metadata =
                std::fs::symlink_metadata(entry.path()).map_err(|_| "qa_profile_unreadable")?;
            if !allowed || !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("qa_profile_not_unused");
            }
        }
    }
    GlobalSettings::update_developer_mode(&path, true).map_err(|_| "qa_preferences_write")
}

/// Persist a run-owned Cube through Home's real validation/backup/finalization.
/// Re-entry verifies the existing sealed seed instead of replacing it.
pub async fn create_testnet4_cube(
    root: CoincubeDirectory,
    id: uuid::Uuid,
    name: String,
    pin: &str,
    saved_words: &[String],
    server_limit: usize,
) -> Result<CubeSettings, &'static str> {
    let network_dir = root.network_directory(Network::Testnet4);
    let existing = match settings::Settings::from_file(&network_dir) {
        Ok(settings) => settings.cubes,
        Err(settings::SettingsError::NotFound) => Vec::new(),
        Err(_) => return Err("cube_settings_unreadable"),
    };
    let cube = if let Some(cube) = existing.iter().find(|c| c.id == id.to_string()) {
        if cube.name != name || cube.network != ChainId::Testnet4 {
            return Err("cube_identity_mismatch");
        }
        cube.clone()
    } else {
        initialize_fresh_qa_preferences(&root)?;
        create_new_testnet4_cube(
            ChainId::Testnet4,
            &root,
            id,
            name,
            pin,
            saved_words,
            server_limit,
            existing,
            None,
        )
        .await?
    };
    if !cube.backed_up
        || cube.creation_backup_bypass.is_some()
        || creation_gate::evaluate_for_cube(&cube, None) != creation_gate::CreationGate::Satisfied
    {
        return Err("cube_backup_not_verified");
    }
    let location = unlock::CubeLocation::new(root.path(), &cube);
    let signer = match unlock::unlock_blocking(&location, pin).map_err(|_| "cube_unlock_failed")? {
        unlock::PinOutcome::Unlock(signer) => signer,
        _ => return Err("cube_unlock_failed"),
    };
    if signer.words() != saved_words {
        return Err("cube_backup_mismatch");
    }
    Ok(cube)
}

/// Fresh native Recovery Kit fixture only. Never adopts an existing UUID/profile.
/// Uses the same Home create/backup/persist ceremony on the two allowed chains.
pub async fn create_recovery_fixture_cube(
    chain: ChainId,
    root: CoincubeDirectory,
    id: uuid::Uuid,
    pin: &str,
    saved_words: &[String],
    server_limit: usize,
    client: CoincubeClient,
) -> Result<CubeSettings, &'static str> {
    if !matches!(chain, ChainId::Testnet4 | ChainId::BitcoinBlake2bTestnet4)
        || root.path().exists()
        || unlock::device_secret::load_optional(&id.to_string())
            .map_err(|_| "keystore_unavailable")?
            .is_some()
    {
        return Err("recovery_fixture_not_fresh");
    }
    if pin.len() != 4 || !pin.bytes().all(|b| b.is_ascii_digit()) {
        return Err("recovery_desktop_pin_length");
    }
    // Owner-authorized settings apply only to this initially absent QA profile.
    // These are normal product preference setters, never a visibility bypass.
    tokio::fs::create_dir(root.path())
        .await
        .map_err(|_| "recovery_profile_create")?;
    crate::recovery_native::initialize_preferences(&root, chain)?;
    crate::recovery_native::require_backend_feature(chain, &client, &root).await?;
    let name = format!("Native Recovery QA {}", &id.to_string()[..8]);
    create_new_testnet4_cube(
        chain,
        &root,
        id,
        name,
        pin,
        saved_words,
        server_limit,
        Vec::new(),
        Some(client),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::create_testnet4_cube;
    use crate::app::settings::{self, CubeSettings};
    use crate::chain::ChainId;
    use crate::dir::CoincubeDirectory;
    use crate::services::unlock::creation_gate::{self, CreationBackupBypass, CreationRecoveryKit};
    use coincube_core::miniscript::bitcoin::Network;

    fn temp_root(tag: &str) -> CoincubeDirectory {
        let path = std::env::temp_dir().join(format!(
            "coincube-testnet4-reentry-{tag}-{}",
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        CoincubeDirectory::new(path)
    }

    fn write_cubes(root: &CoincubeDirectory, cubes: Vec<CubeSettings>) -> Vec<u8> {
        let directory = root.network_directory(Network::Testnet4);
        std::fs::create_dir_all(directory.path()).unwrap();
        let settings = settings::Settings {
            cubes,
            ..settings::Settings::default()
        };
        let bytes = serde_json::to_vec_pretty(&settings).unwrap();
        std::fs::write(directory.path().join(settings::SETTINGS_FILE_NAME), &bytes).unwrap();
        bytes
    }

    fn reenter(
        root: CoincubeDirectory,
        id: uuid::Uuid,
        name: &str,
    ) -> Result<CubeSettings, &'static str> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(create_testnet4_cube(
            root,
            id,
            name.to_string(),
            "0000",
            &[],
            1,
        ))
    }

    fn assert_settings_unchanged(root: &CoincubeDirectory, before: &[u8]) {
        let path = root
            .network_directory(Network::Testnet4)
            .path()
            .join(settings::SETTINGS_FILE_NAME);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn unused_legacy_qa_profile_gets_normal_developer_preference_only() {
        let root = temp_root("fresh-preferences");
        assert!(!super::GlobalSettings::load_developer_mode(
            &super::GlobalSettings::path(&root)
        ));
        super::initialize_fresh_qa_preferences(&root).unwrap();
        assert!(super::GlobalSettings::load_developer_mode(
            &super::GlobalSettings::path(&root)
        ));
        assert!(!super::GlobalSettings::load_bitcoin_blake2b_beta(
            &super::GlobalSettings::path(&root)
        ));
        let (home, _) = super::Home::new_for_chain(root.clone(), Some(ChainId::Testnet4));
        assert_eq!(home.network, ChainId::Testnet4);
        assert!(home.displayed_networks.contains(&ChainId::Testnet4));
        std::fs::remove_dir_all(root.path()).unwrap();
    }

    #[test]
    fn qa_initialization_never_overwrites_existing_off_or_unknown_profile_state() {
        for scenario in [
            "off",
            "bad-preferences",
            "cube",
            "bad-settings",
            "unknown-settings",
            "unknown-wallet-file",
            "network-symlink",
        ] {
            let root = temp_root(scenario);
            let path = super::GlobalSettings::path(&root);
            match scenario {
                "off" => super::GlobalSettings::update_developer_mode(&path, false).unwrap(),
                "bad-preferences" => std::fs::write(&path, b"malformed").unwrap(),
                "cube" => {
                    let cube = CubeSettings::new_with_id(
                        uuid::Uuid::new_v4(),
                        "Unrelated".into(),
                        ChainId::Bitcoin,
                    );
                    let directory = root.network_directory(ChainId::Bitcoin);
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
                "bad-settings" | "unknown-settings" | "unknown-wallet-file" => {
                    let directory = root.network_directory(ChainId::Bitcoin);
                    std::fs::create_dir_all(directory.path()).unwrap();
                    std::fs::write(
                        directory.path().join(
                            if matches!(scenario, "bad-settings" | "unknown-settings") {
                                settings::SETTINGS_FILE_NAME
                            } else {
                                "daemon.toml"
                            },
                        ),
                        if scenario == "unknown-settings" {
                            b"{}".as_slice()
                        } else {
                            b"unknown"
                        },
                    )
                    .unwrap();
                }
                _ => {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(
                        root.path(),
                        root.network_directory(ChainId::Bitcoin).path(),
                    )
                    .unwrap();
                    #[cfg(windows)]
                    std::os::windows::fs::symlink_dir(
                        root.path(),
                        root.network_directory(ChainId::Bitcoin).path(),
                    )
                    .unwrap();
                    #[cfg(not(any(unix, windows)))]
                    panic!("network-symlink fixture unsupported on this platform");
                }
            }
            let before = std::fs::read(&path).ok();
            assert!(
                super::initialize_fresh_qa_preferences(&root).is_err(),
                "{}",
                scenario
            );
            assert_eq!(std::fs::read(&path).ok(), before);
            std::fs::remove_dir_all(root.path()).unwrap();
        }
    }

    #[tokio::test]
    async fn recovery_home_uses_account_grant_without_anonymous_global_flag_or_chain_reset() {
        use httpmock::{Method::GET, MockServer};
        for (chain, grant, expected) in [
            (ChainId::BitcoinBlake2bTestnet4, true, true),
            (ChainId::BitcoinBlake2bTestnet4, false, false),
            (ChainId::Testnet4, false, true),
        ] {
            let server = MockServer::start();
            let user = server.mock(|when, then| {
                when.method(GET)
                    .path("/api/v1/user")
                    .header("authorization", "Bearer offline-only");
                then.status(200).json_body(
                    serde_json::json!({"id":5,"email":"qa@example.invalid","email_verified":true}),
                );
            });
            let features = server.mock(|when, then| {
                when.method(GET).path("/api/v1/connect/features").header("authorization", "Bearer offline-only");
                then.status(200).json_body(serde_json::json!({"success":true,"data":{"plans":[],"bitcoinBlake2bEnabled":grant}}));
            });
            server.mock(|when, then| {
                when.method(GET).path("/api/v1/connect/duress");
                then.status(200).json_body(serde_json::json!({"success":true,"data":{"active":false,"unlockAt":null,"enrolled":false,"thisDeviceRegistered":false}}));
            });
            let root = temp_root("account-grant");
            crate::recovery_native::initialize_preferences(&root, chain).unwrap();
            let mut client = super::CoincubeClient::for_test(server.base_url());
            client.set_token("offline-only");
            let result = super::admitted_recovery_home(chain, &root, Some(client)).await;
            assert_eq!(result.is_ok(), expected);
            if let Ok(home) = result {
                assert_eq!(home.network, chain);
                assert!(!home.global_blake2b_enabled);
                assert!(home.displayed_networks.contains(&chain));
                assert_eq!(home.datadir_path, root);
                assert!(home.connect_account.is_authenticated());
            }
            assert_eq!(user.hits(), 1);
            assert_eq!(features.hits(), 1);
            std::fs::remove_dir_all(root.path()).unwrap();
        }
    }

    #[test]
    fn reentry_refuses_a_different_cube_identity_without_rewriting_settings() {
        let id = uuid::Uuid::new_v4();
        for (label, cube) in [
            (
                "name",
                CubeSettings::new_with_id(id, "Other".to_string(), ChainId::Testnet4),
            ),
            (
                "chain",
                CubeSettings::new_with_id(id, "E2E".to_string(), ChainId::Bitcoin),
            ),
        ] {
            let root = temp_root(label);
            let before = write_cubes(&root, vec![cube]);
            let error = reenter(root.clone(), id, "E2E").unwrap_err();
            assert_eq!(error, "cube_identity_mismatch", "{label}");
            assert_settings_unchanged(&root, &before);
            std::fs::remove_dir_all(root.path()).unwrap();
        }
    }

    #[test]
    fn reentry_requires_a_verified_phrase_and_refuses_a_bypass_or_kit_substitute() {
        let id = uuid::Uuid::new_v4();
        let mut unverified = CubeSettings::new_with_id(id, "E2E".to_string(), ChainId::Testnet4);
        unverified.creation_backup_required = true;

        let mut bypassed = unverified.clone();
        bypassed.backed_up = true;
        bypassed.creation_backup_bypass = Some(CreationBackupBypass {
            at: 1_700_000_000,
            acknowledged: creation_gate::BYPASS_ACKNOWLEDGEMENT.to_string(),
        });

        // A seed-bearing creation kit satisfies the open gate, and a legacy
        // cube is not gated at all. Neither is the phrase this driver re-reads.
        let mut kit_only = unverified.clone();
        kit_only.creation_recovery_kit = Some(CreationRecoveryKit {
            at: 1_700_000_000,
            cube_id: 7,
            has_seed: true,
        });
        let mut legacy = CubeSettings::new_with_id(id, "E2E".to_string(), ChainId::Testnet4);
        legacy.backed_up = false;

        for (label, cube) in [
            ("unverified", unverified),
            ("bypassed", bypassed),
            ("kit", kit_only),
            ("legacy", legacy),
        ] {
            let root = temp_root(label);
            let before = write_cubes(&root, vec![cube]);
            let error = reenter(root.clone(), id, "E2E").unwrap_err();
            assert_eq!(error, "cube_backup_not_verified", "{label}");
            assert_settings_unchanged(&root, &before);
            std::fs::remove_dir_all(root.path()).unwrap();
        }
    }

    #[test]
    fn reentry_refuses_unreadable_settings_without_creating_a_cube() {
        let root = temp_root("corrupt");
        let directory = root.network_directory(Network::Testnet4);
        std::fs::create_dir_all(directory.path()).unwrap();
        let path = directory.path().join(settings::SETTINGS_FILE_NAME);
        std::fs::write(&path, b"{").unwrap();

        let error = reenter(root.clone(), uuid::Uuid::new_v4(), "E2E").unwrap_err();
        assert_eq!(error, "cube_settings_unreadable");
        assert_eq!(std::fs::read(&path).unwrap(), b"{");
        std::fs::remove_dir_all(root.path()).unwrap();
    }
}
