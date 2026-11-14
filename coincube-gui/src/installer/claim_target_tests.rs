//! The Bitcoin Blake2b **claim target**: what the flow admits, what it hands
//! into the fork construction path, and what lands on disk.
//!
//! Synthetic datadirs and a mock Connect server only — no OS secrets, no real
//! node, no existing wallet.
// The session slot is process-global, so every test here holds
// `session::test_guard()` for its whole body — including across `.await`. That
// is the point: the guard is what stops two tests sharing the one slot, and
// these are `#[tokio::test]` single-threaded runtimes, so there is no other
// task to starve.
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::chain::ChainId;
use crate::services::coincube::CoincubeClient;
use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use httpmock::prelude::*;

/// A real mainnet Vault descriptor. A claim reuses the source Cube's verbatim,
/// so the test asserts on the string rather than on its contents.
const SOURCE_DESCRIPTOR: &str = concat!(
    "wsh(andor(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4z",
    "LqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*),older(10000),pk([aabbccdd]xpub68JJT",
    "Xc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8Uut",
    "BsBbgKHzaD5HkTkifK/<0;1>/*)))#3xh8xmhn"
);

/// The source Cube's PIN. The target's seed file is written under it — that is
/// the whole of invariant I10's "unlock behaves identically".
const SOURCE_PIN: &str = "2468";

fn authenticated_client(server: &MockServer) -> CoincubeClient {
    let mut client = CoincubeClient::new();
    client.base_url = server.base_url();
    client.set_token("synthetic-claim-token");
    client
}

/// The Connect surface `install_local_wallet` actually calls on the fork
/// chain: the account flag, the chain anchor, and the Esplora endpoints the
/// daemon check probes. Mirrors `connect_activation_tests`, which is the
/// fixture that proves the fork install path end to end.
async fn fork_install_server() -> (MockServer, String) {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path("/api/v1/connect/features");
            then.status(200).json_body(serde_json::json!(
                {"success":true,"data":{"plans":[],"bitcoinBlake2bEnabled":true}}
            ));
        })
        .await;
    let prefix = "/api/v1/esplora/bitcoin-blake2b/mainnet";
    for (path, body) in [
        (
            format!("{prefix}/block-height/0"),
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_string(),
        ),
        (format!("{prefix}/block-height/973029"), "11".repeat(32)),
        (format!("{prefix}/blocks/tip/hash"), "11".repeat(32)),
        (
            format!("{prefix}/block/{}/status", "11".repeat(32)),
            "{\"in_best_chain\":true,\"height\":973029,\"next_best\":null}".to_string(),
        ),
    ] {
        server
            .mock_async(|when, then| {
                when.method(GET).path(path);
                then.status(200).body(body);
            })
            .await;
    }
    let base = server.base_url();
    (server, format!("{base}{prefix}"))
}

/// The anchor mock, registered with a timestamp read **now**.
///
/// `then.json_body(...)` is static — the body is built at registration and
/// replayed verbatim — while production refuses an anchor older than
/// `MAX_ANCHOR_AGE` (90 s, `coincubed/src/connect.rs:31`). A test that
/// registers once and then spends an Argon2id unlock and two installs against
/// it passes on an idle machine and fails under load, which is a time bomb
/// rather than a test. So each install re-registers immediately before it runs.
async fn register_anchor(server: &MockServer) -> httpmock::Mock<'_> {
    server.mock_async(|when, then| {
        when.method(GET).path("/api/v1/connect/networks/bitcoin-blake2b/anchor");
        then.status(200).json_body(serde_json::json!({"success":true,"data":{"network":"bitcoin-blake2b","state":"available","anchor":{
            "tip_hash":"11".repeat(32),"tip_height":973029,"tip_median_time_past":1800000000,
            "observed_at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
            "observation":{"tip_height":973029,"fork":{"height":972000,"active":true},
                "rdts":{"state":"flagday","flagday":{"height":972000,"expiry_time":1800010000_i64,"active":false}}}
        }}}));
    }).await
}

/// A source Cube with an open session, which is what a claim launches from —
/// `try_new_for_chain` refuses one without a PIN in this session.
fn source(name: &str) -> (ClaimSource, Fingerprint) {
    let signer = Signer::generate(Network::Bitcoin).unwrap();
    let fingerprint = signer.fingerprint();
    let cube_id = uuid::Uuid::new_v4().to_string();
    crate::app::session::open(
        cube_id.clone(),
        zeroize::Zeroizing::new(SOURCE_PIN.to_string()),
    );
    (
        ClaimSource {
            cube: {
                let mut cube = crate::app::settings::CubeSettings::new_with_raw_id(
                    cube_id,
                    name.to_string(),
                    ChainId::Bitcoin,
                );
                // A source Cube whose mnemonic the user has written down: the
                // target inherits that rather than nagging for the same words.
                cube.backed_up = true;
                cube
            },
            descriptor: SOURCE_DESCRIPTOR.parse().unwrap(),
            signer: Arc::new(signer),
        },
        fingerprint,
    )
}

fn enable_beta(root: &CoincubeDirectory) {
    std::fs::create_dir_all(root.path()).unwrap();
    let path = crate::app::settings::global::GlobalSettings::path(root);
    crate::app::settings::global::GlobalSettings::update_bitcoin_blake2b_beta(&path, true).unwrap();
}

fn temp_root(tag: &str) -> CoincubeDirectory {
    CoincubeDirectory::new(
        std::env::temp_dir().join(format!("claim-{tag}-{}", uuid::Uuid::new_v4())),
    )
}

/// The target is built from the source Cube's descriptor and gets an identity
/// of its own: its own Cube id (I7) and an alias naming the chain.
#[tokio::test]
async fn a_claim_target_reuses_the_descriptor_and_takes_its_own_cube_identity() {
    // One process-global session slot: serialise with every other test that
    // opens one, or a sibling's `open` replaces this source's PIN.
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (source, _) = source("Savings");
    let source_cube_id = source.cube_id().to_string();
    let root = temp_root("identity");

    let (installer, _) = Installer::try_new_for_chain(
        root,
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .expect("an authenticated claim is admitted on the fork chain");

    assert_eq!(
        installer.context.descriptor.as_ref().unwrap().to_string(),
        SOURCE_DESCRIPTOR,
        "a claim that changed the descriptor would watch different addresses"
    );
    assert_eq!(installer.context.wallet_alias, "Savings · BTCB2");
    let target_cube_id = installer.context.seed_cube_id();
    assert!(!target_cube_id.is_empty(), "the target is its own Cube");
    assert_ne!(
        target_cube_id, source_cube_id,
        "sharing the source Cube's id would collapse the two Cubes (I7)"
    );
    assert!(
        installer.context.claim_source.is_some(),
        "a claim target's seed comes from the source Cube, not from a fresh one"
    );
}

/// The seed decision, asserted where it lands rather than where it is made:
/// the file written under the fork chain decrypts to the **source** Cube's
/// mnemonic, and sits beside — never inside — the Bitcoin folder.
#[test]
fn the_target_master_seed_file_is_the_source_cube_mnemonic_on_the_fork_chain() {
    // One process-global session slot: serialise with every other test that
    // opens one, or a sibling's `open` replaces this source's PIN.
    let _guard = crate::app::session::test_guard();
    let root = temp_root("seed");
    let (source, fingerprint) = source("Savings");
    let expected = zeroize::Zeroizing::new(source.signer.mnemonic().join(" "));
    let target_cube_id = uuid::Uuid::new_v4().to_string();

    let seed = claim::target_master_seed(&source).expect("the recorded decision reuses the seed");
    persist_cube_master_seed(
        &seed,
        &root,
        ChainId::BitcoinBlake2b,
        "1234",
        &target_cube_id,
        None,
    )
    .unwrap();

    let path = coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(
        root.path(),
        ChainId::BitcoinBlake2b,
    )
    .join(
        coincube_core::signer::MnemonicFileName {
            fingerprint,
            descriptor_info: None,
        }
        .to_string(),
    );
    let plaintext = coincube_core::seed_crypt::decrypt_with(
        &std::fs::read(&path).unwrap(),
        "1234",
        &target_cube_id,
        None,
    )
    .unwrap();
    assert_eq!(
        plaintext.as_slice(),
        expected.as_bytes(),
        "the target must be able to sign for the source descriptor"
    );
    assert!(
        !root
            .path()
            .join("bitcoin")
            .join("mnemonics")
            .join(
                coincube_core::signer::MnemonicFileName {
                    fingerprint,
                    descriptor_info: None,
                }
                .to_string()
            )
            .exists(),
        "the fork's copy must never be written into the Bitcoin family's folder"
    );
    let _ = std::fs::remove_dir_all(root.path());
}

/// Item 6, at the construction boundary rather than through an extracted
/// predicate: a caller that holds live Breez and Spark handles — which every
/// running Bitcoin Cube does — must not have them reach the Cube being built
/// on the fork chain. Deleting the narrowing in `try_new_for_chain` fails this
/// test. What the installer *may* hold is the source Cube's own handles,
/// parked for the return trip; that is asserted here too, because the two are
/// easy to conflate and one of them is an invariant.
#[tokio::test]
async fn a_fork_installer_is_never_handed_the_callers_breez_or_spark_clients() {
    // One process-global session slot: serialise with every other test that
    // opens one, or a sibling's `open` replaces this source's PIN.
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let breez = Arc::new(crate::app::breez_liquid::BreezClient::disconnected(
        Network::Bitcoin,
    ));
    let (source, _) = source("Savings");

    let (installer, _) = Installer::try_new_for_chain(
        temp_root("clients"),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        Some(breez.clone()),
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .expect("holding a Liquid client is what a running Bitcoin Cube looks like");

    assert!(
        installer.breez_client.is_none(),
        "a Bitcoin Blake2b Cube must never be given a Liquid client"
    );
    assert!(installer.spark_backend.is_none());
    assert!(
        installer.context.cube_encryption_key.is_none(),
        "nor anything derived from the source Cube's Liquid signer"
    );
    // Parked, not handed in — and the distinction is the whole point. The
    // installer holds the source Cube's handle so backing out can rebuild that
    // Cube; what it must never do is give it to the Cube being built. On the
    // success path returns these handles to the original Bitcoin Cube for
    // the split transaction; they never enter the fork Cube.
    assert_eq!(
        Arc::strong_count(&breez),
        2,
        "the caller's handle and the parked one"
    );
    assert!(installer
        .source_cube
        .as_ref()
        .is_some_and(|s| s.breez_client.is_some()));
}

/// The account gate is re-checked at admission. A card press, or a rail item
/// that was rendered a moment ago, is not a permission.
#[tokio::test]
async fn an_unauthenticated_claim_is_refused_before_anything_is_built() {
    // One process-global session slot: serialise with every other test that
    // opens one, or a sibling's `open` replaces this source's PIN.
    let _guard = crate::app::session::test_guard();
    let (source, _) = source("Savings");
    let root = temp_root("unauth");
    let root_path = root.path().to_path_buf();

    let refused = Installer::try_new_for_chain(
        root,
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        None,
        None,
        false,
        None,
    );
    assert!(refused.is_err(), "no Connect session, no claim");
    assert!(!root_path.exists(), "and nothing on disk");
}

/// Every other flow stays refused on the fork chain: beyond creation and
/// claim, only a whole Cube's restore (Recovery Kit, or phone) is admitted.
#[tokio::test]
async fn the_fork_chain_refuses_every_flow_but_creation_claim_and_cube_restore() {
    // One process-global session slot: serialise with every other test that
    // opens one, or a sibling's `open` replaces this source's PIN.
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    for flow in [
        UserFlow::AddWallet,
        UserFlow::RestoreVaultFromRecoveryKit,
        UserFlow::RecoverInheritedVault {
            cube_id: 1,
            full_cube: true,
        },
        UserFlow::RecoverOwnCubeWithPhone {
            cube_id: 1,
            full_cube: false,
        },
    ] {
        let refused = Installer::try_new_for_chain(
            temp_root("flows"),
            ChainId::BitcoinBlake2b,
            flow.clone(),
            false,
            None,
            None,
            None,
            false,
            Some(authenticated_client(&server)),
        );
        assert!(refused.is_err(), "{:?} must stay refused on the fork", flow);
    }
    for flow in [
        UserFlow::RestoreFromRecoveryKit { cube_uuid: None },
        UserFlow::RecoverOwnCubeWithPhone {
            cube_id: 1,
            full_cube: true,
        },
    ] {
        let root = temp_root("restore");
        let (installer, _) = Installer::try_new_for_chain(
            root.clone(),
            ChainId::BitcoinBlake2b,
            flow.clone(),
            false,
            None,
            None,
            None,
            false,
            Some(authenticated_client(&server)),
        )
        .unwrap_or_else(|e| panic!("{:?} restores on the fork: {}", flow, e));
        // A restored Cube is never handed a Breez or Spark client, and
        // building it touches nothing on disk.
        assert!(installer.breez_client.is_none());
        assert!(installer.spark_backend.is_none());
        assert!(!root.path().exists());
    }
}

/// A claim target's flow collects nothing the source Cube already answers, and
/// in particular never shows the mnemonic again: the source Cube backed it up,
/// and a second showing is a second physical copy of one secret.
#[tokio::test]
async fn the_claim_flow_never_asks_for_a_pin_a_descriptor_or_the_mnemonic_again() {
    // One process-global session slot: serialise with every other test that
    // opens one, or a sibling's `open` replaces this source's PIN.
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (source, _) = source("Savings");
    let (mut installer, _) = Installer::try_new_for_chain(
        temp_root("steps"),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();

    // Asserted through the context rather than step identity: the descriptor
    // and the alias are already settled, and no PIN step can run because the
    // target inherits the source Cube's.
    assert_eq!(
        installer.steps.len(),
        7,
        "overview → descriptor → Connect → node → optional local node → alias → done"
    );
    assert_eq!(
        installer.current, 0,
        "Claim opens on the overview, before node setup"
    );
    assert!(installer.context.descriptor.is_some());
    assert!(installer.context.restore_pin.is_none());
    let _ = installer.view();
    let _ = installer.update(Message::Next);
    assert!(installer.current > 0, "Continue advances to target setup");
    let _ = installer.update(Message::Previous);
    assert_eq!(
        installer.current, 0,
        "Back returns to the Claim explanation"
    );
}

/// The "already claimed" input to the visibility matrix, read from the fork
/// chain's settings file rather than from the Connect session — a target that
/// exists must not look absent because the API is unreachable.
#[test]
fn an_existing_target_is_found_by_descriptor_on_the_fork_chain_only() {
    // One process-global session slot: serialise with every other test that
    // opens one, or a sibling's `open` replaces this source's PIN.
    let _guard = crate::app::session::test_guard();
    let root = temp_root("already");
    let fork_dir = root.network_directory(ChainId::BitcoinBlake2b);
    let bitcoin_dir = root.network_directory(ChainId::Bitcoin);
    let descriptor: coincube_core::descriptors::CoincubeDescriptor =
        SOURCE_DESCRIPTOR.parse().unwrap();
    let checksum = WalletId::generate(&descriptor).descriptor_checksum;

    // Nothing on disk at all.
    assert!(!crate::app::claim_target_exists(&root, &checksum));

    // The source Cube's own Vault, on the Bitcoin side, is not a claim target.
    let wallet = crate::app::settings::WalletSettings {
        name: "Vault".to_string(),
        alias: None,
        descriptor_checksum: checksum.clone(),
        pinned_at: None,
        keys: Vec::new(),
        hardware_wallets: Vec::new(),
        remote_backend_auth: None,
        start_internal_bitcoind: None,
        pending_rescan: None,
        keychain_keys_recorded: false,
    };
    for dir in [&bitcoin_dir, &fork_dir] {
        std::fs::create_dir_all(dir.path()).unwrap();
    }
    let settings = crate::app::settings::Settings {
        wallets: vec![wallet.clone()],
        ..Default::default()
    };
    std::fs::write(
        bitcoin_dir
            .path()
            .join(crate::app::settings::SETTINGS_FILE_NAME),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    assert!(
        !crate::app::claim_target_exists(&root, &checksum),
        "the Bitcoin Cube's own Vault is the source, not a target"
    );

    // A wallet record on the fork chain with **no Cube** is an interrupted
    // install, not a claim: `install_local_wallet` writes the wallet before the
    // exit seam writes the Cube. Treating it as claimed would hide the retry
    // while Home shows no Cube to open — the user stranded between two screens
    // that each think the other has it.
    std::fs::write(
        fork_dir
            .path()
            .join(crate::app::settings::SETTINGS_FILE_NAME),
        serde_json::to_vec(&settings).unwrap(),
    )
    .unwrap();
    assert!(
        !crate::app::claim_target_exists(&root, &checksum),
        "a stranded wallet record must leave the claim retryable"
    );

    // The Cube that can actually be opened is what counts.
    let mut target = crate::app::settings::CubeSettings::new_with_raw_id(
        uuid::Uuid::new_v4().to_string(),
        "Savings · BTCB2".to_string(),
        ChainId::BitcoinBlake2b,
    );
    target.vault_wallet_id = Some(WalletId::new(checksum.clone(), Some(1)));
    let with_cube = crate::app::settings::Settings {
        wallets: vec![wallet],
        cubes: vec![target],
        ..Default::default()
    };
    std::fs::write(
        fork_dir
            .path()
            .join(crate::app::settings::SETTINGS_FILE_NAME),
        serde_json::to_vec(&with_cube).unwrap(),
    )
    .unwrap();
    assert!(crate::app::claim_target_exists(&root, &checksum));
    assert!(
        !crate::app::claim_target_exists(&root, "someone-elses-checksum"),
        "answered per descriptor, not per device"
    );
    let _ = std::fs::remove_dir_all(root.path());
}

/// **The end-to-end producer path.** Every other test here asserts a piece;
/// this one runs the install the user's press actually triggers, then the exit
/// seam that mints the Cube, and checks the two ends agree.
///
/// The gap this closes: the earlier seed test handed `persist_cube_master_seed`
/// a literal PIN, so it could not see that a claim reached `seed_password` with
/// no credentials at all — the source Cube's PIN is resolved from its session,
/// not from `cube_settings`, which a claim deliberately leaves `None`.
#[tokio::test]
async fn a_claim_install_writes_a_seed_the_minted_cube_can_open() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path_contains("/connect/");
            then.status(200)
                .json_body(serde_json::json!({"success": true, "data": {}}));
        })
        .await;
    let (source, source_fingerprint) = source("Savings");
    let root = temp_root("e2e");

    let (installer, _) = Installer::try_new_for_chain(
        root.clone(),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();

    // 1. The credentials the install will encrypt under exist at all. This is
    //    the assertion whose absence let the gap through.
    let password = installer
        .context
        .seed_password()
        .expect("a claim must reach the seed write with the source Cube's PIN");
    assert_eq!(password.as_str(), SOURCE_PIN);

    // 2. The seed the install writes, through the production helper and the
    //    production context — not a literal.
    let target_cube_id = installer.context.seed_cube_id().to_owned();
    let seed = claim::target_master_seed(installer.context.claim_source.as_ref().unwrap()).unwrap();
    persist_cube_master_seed(
        &seed,
        &installer.context.coincube_directory,
        ChainId::BitcoinBlake2b,
        installer.context.seed_password().unwrap().as_str(),
        &target_cube_id,
        None,
    )
    .unwrap();

    // 3. The Cube the exit seam would mint. `find_or_create_cube` falls back to
    //    a fresh UUID when the identity is absent, which would leave the seed
    //    written above bound to a Cube id nothing has.
    let identity = crate::gui::tab::installer_exit_identity(&installer)
        .expect("the claim target's own identity must reach the exit seam");
    assert_eq!(
        identity.uuid, target_cube_id,
        "the minted Cube must be the Cube the seed file is bound to"
    );
    assert_eq!(identity.name, "Savings · BTCB2");

    let exit_seed = crate::gui::tab::installer_exit_seed(&installer)
        .expect("the minted Cube must record which master signer it holds");
    assert_eq!(exit_seed.master_signer_fingerprint, source_fingerprint);
    assert_eq!(exit_seed.pin.as_str(), SOURCE_PIN);
    assert!(
        exit_seed.seed_backed_up,
        "the source Cube's backup state is inherited, not reset"
    );

    // 4. And the file opens with exactly those credentials.
    let path = coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(
        root.path(),
        ChainId::BitcoinBlake2b,
    )
    .join(
        coincube_core::signer::MnemonicFileName {
            fingerprint: source_fingerprint,
            descriptor_info: None,
        }
        .to_string(),
    );
    assert!(coincube_core::seed_crypt::decrypt_with(
        &std::fs::read(&path).unwrap(),
        exit_seed.pin.as_str(),
        &identity.uuid,
        None,
    )
    .is_ok());
    let _ = std::fs::remove_dir_all(root.path());
}

/// A source Cube with no unlock credential in this session — a passkey Cube
/// whose derived seed is not parked, or a PIN Cube whose session lapsed —
/// would mint a copy nothing could open. Refused at admission rather than at
/// the seed write, with advice that works for each.
#[tokio::test]
async fn a_claim_is_refused_when_the_source_cube_has_no_pin_in_this_session() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (mut passkey_source, _) = source("Passkey Cube");
    passkey_source.cube.passkey_metadata = Some(crate::app::settings::PasskeyMetadata {
        credential_id: "synthetic-credential".to_string(),
        rp_id: "coincube.io".to_string(),
        created_at: 0,
        label: None,
    });
    assert!(passkey_source.cube.is_passkey_cube());
    // A passkey Cube's session: unlocked, but `pin_for` answers `None`.
    crate::app::session::open_without_pin(passkey_source.cube_id().to_string());
    let root = temp_root("nopin");
    let root_path = root.path().to_path_buf();

    let refused = Installer::try_new_for_chain(
        root,
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(passkey_source),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    );
    let message = refused.err().unwrap().to_string();
    assert!(
        message.contains("passkey"),
        "a passkey owner must not be told to reopen their Cube — there is no \
         PIN to reopen it with, so that advice loops forever: {}",
        message
    );
    assert!(!root_path.exists(), "and nothing on disk");

    // The same condition, the other audience: a PIN Cube whose session lapsed
    // gets advice that works.
    let (lapsed, _) = source("PIN Cube");
    crate::app::session::close();
    let root = temp_root("lapsed");
    let refused = Installer::try_new_for_chain(
        root,
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(lapsed),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    );
    let message = refused.err().unwrap().to_string();
    assert!(message.contains("Reopen this Cube"), "{}", message);
    assert!(!message.contains("passkey"), "{}", message);
}

/// A passkey source Cube's copy unlocks with the same passkey (decision
/// 2026-10-29): admitted once the source's derived seed is in the session,
/// its seed file written under the password derived from that seed, and the
/// minted copy records the source's passkey and master fingerprint.
#[tokio::test]
async fn a_passkey_source_cube_can_create_its_copy() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (mut passkey_source, fingerprint) = source("Passkey Cube");
    let metadata = crate::app::settings::PasskeyMetadata {
        credential_id: "synthetic-credential".to_string(),
        rp_id: "coincube.io".to_string(),
        created_at: 0,
        label: None,
    };
    passkey_source.cube.passkey_metadata = Some(metadata.clone());
    passkey_source.cube.master_signer_fingerprint = Some(fingerprint);
    let source_id = passkey_source.cube_id().to_string();
    crate::app::session::open_without_pin(source_id.clone());
    crate::app::session::store_unlocked_signer(
        &source_id,
        fingerprint,
        coincube_core::signer::MasterSigner::generate(Network::Bitcoin).unwrap(),
    );
    let expected_password =
        crate::app::session::seed_file_password(&passkey_source.cube).expect("derivable");

    let (installer, _) = Installer::try_new_for_chain(
        temp_root("passkey-copy"),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(passkey_source),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();
    assert!(installer.context.cube_pin.is_none());
    assert_eq!(
        installer.context.seed_password().map(|p| p.as_str()),
        Some(expected_password.as_str())
    );
    let exit = crate::gui::tab::installer_exit_seed(&installer).expect("passkey exit seed");
    assert_eq!(
        exit.passkey.map(|p| p.credential_id),
        Some(metadata.credential_id)
    );
    assert_eq!(exit.master_signer_fingerprint, fingerprint);
}

/// Backing out of a claim must put the user back in the Cube they started in.
///
/// `BackToApp` restores from `cube_settings`, which a claim deliberately leaves
/// `None` (the installer is building a Cube on another chain), so without the
/// parked source it falls through to Home — the user presses Claim, changes
/// their mind, and is logged out of their Cube for it. Driven through the real
/// `Tab::update` arm, not through the parked field.
#[tokio::test]
async fn backing_out_of_a_claim_returns_to_the_source_cube_not_to_home() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (source, _) = source("Savings");
    let source_cube_id = source.cube_id().to_string();
    let root = temp_root("cancel");
    // `BackToApp` reads the source network's gui config.
    let source_dir = root.network_directory(ChainId::Bitcoin);
    std::fs::create_dir_all(source_dir.path()).unwrap();
    std::fs::write(
        source_dir
            .path()
            .join(crate::app::config::DEFAULT_FILE_NAME),
        b"",
    )
    .unwrap();

    let (installer, _) = Installer::try_new_for_chain(
        root.clone(),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        Some(Arc::new(
            crate::app::breez_liquid::BreezClient::disconnected(Network::Bitcoin),
        )),
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();

    // The fork build still sees nothing: parking is not handing in.
    assert!(installer.breez_client.is_none());
    assert!(installer.spark_backend.is_none());
    assert!(installer.context.cube_encryption_key.is_none());
    assert!(
        installer
            .source_cube
            .as_ref()
            .is_some_and(|s| s.breez_client.is_some()),
        "the source Cube's handle is parked for the return trip"
    );

    let mut tab =
        crate::gui::tab::Tab::new(0, crate::gui::tab::State::Installer(Box::new(installer)));
    let _ = tab.update(crate::gui::tab::Message::Install(Message::BackToApp(
        Network::Bitcoin,
    )));

    match &tab.state {
        crate::gui::tab::State::Loader(loader) => assert_eq!(
            loader.cube_settings.id, source_cube_id,
            "cancel must land back in the source Cube"
        ),
        other => panic!(
            "cancelling a claim must return to the source Cube, not {:?}",
            std::mem::discriminant(other)
        ),
    }
    let _ = std::fs::remove_dir_all(root.path());
}

/// **The production install, not a rehearsal of it.** Drives
/// `install_local_wallet` — the function the user's last click calls — and
/// then opens the seed file it wrote with the credential the exit seam
/// reports, which is the pair that has to agree for the target to be openable.
///
/// This is the test that would have caught P1a. The earlier one asked the
/// context for a password and then supplied it to the writer itself; this one
/// never names a credential, so if the installer cannot obtain one the install
/// fails here exactly as it would for the user.
#[tokio::test]
async fn the_real_install_writes_a_target_the_exit_seams_credential_opens() {
    let _guard = crate::app::session::test_guard();
    let (server, esplora) = fork_install_server().await;
    let (source, source_fingerprint) = source("Savings");
    let root = temp_root("real-install");
    enable_beta(&root);
    let root_path = root.path().to_path_buf();

    let (mut installer, _) = Installer::try_new_for_chain(
        root.clone(),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();
    installer.context.bitcoin_backend =
        Some(BitcoinBackend::Esplora(coincubed::config::EsploraConfig {
            addr: esplora,
            token: None,
            fallback_addr: None,
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        }));

    // Read before the install so the assertions below cannot be written from
    // its output.
    let identity = crate::gui::tab::installer_exit_identity(&installer)
        .expect("the claim target's own identity reaches the exit seam");
    let exit_seed = crate::gui::tab::installer_exit_seed(&installer)
        .expect("the exit seam records which master signer the target holds");
    let wallet_id = WalletId::generate(installer.context.descriptor.as_ref().unwrap());

    let _anchor = register_anchor(&server).await;
    let settings = install_local_wallet(
        installer.context.clone(),
        wallet_id,
        installer.signer.clone(),
    )
    .await
    .expect("a claim install must complete on the production path");

    // The target is a Cube of the fork chain's own, with the reused descriptor.
    assert_eq!(
        settings.descriptor_checksum,
        WalletId::generate(installer.context.descriptor.as_ref().unwrap()).descriptor_checksum
    );
    assert!(root_path.join("bitcoin-blake2b").exists());
    assert!(
        !root_path.join("bitcoin").exists(),
        "a claim must not write into the Bitcoin family's directory"
    );

    // And the seed it wrote opens with the credential the exit seam hands the
    // Cube it is about to mint — the two ends of the defect, checked against
    // each other rather than against a literal.
    let path = coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(
        root.path(),
        ChainId::BitcoinBlake2b,
    )
    .join(
        coincube_core::signer::MnemonicFileName {
            fingerprint: exit_seed.master_signer_fingerprint,
            descriptor_info: None,
        }
        .to_string(),
    );
    let plaintext = coincube_core::seed_crypt::decrypt_with(
        &std::fs::read(&path).expect("the install wrote the target's master seed"),
        exit_seed.pin.as_str(),
        &identity.uuid,
        None,
    )
    .expect("the minted Cube's credential must open the seed the install wrote");
    assert_eq!(exit_seed.master_signer_fingerprint, source_fingerprint);
    assert!(
        !plaintext.is_empty(),
        "and it decrypts to the source Cube's mnemonic"
    );
    let _ = std::fs::remove_dir_all(&root_path);
}

/// The parked source handle is a new `Arc` owner, created by this repair. Every
/// exit has to release it: hold it and the Spark bridge outlives the flow,
/// release it early and cancel loses what it needs to rebuild the source Cube.
///
/// Measured rather than argued — the last handle-lifetime claim in this PR was
/// wrong because nobody traced the owner.
#[tokio::test]
async fn every_exit_from_a_claim_releases_the_parked_source_handle() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let breez = Arc::new(crate::app::breez_liquid::BreezClient::disconnected(
        Network::Bitcoin,
    ));

    let build = |source: ClaimSource, root: CoincubeDirectory| {
        Installer::try_new_for_chain(
            root,
            ChainId::BitcoinBlake2b,
            UserFlow::ClaimBlake2b {
                from_cube: Box::new(source),
            },
            true,
            None,
            Some(breez.clone()),
            None,
            false,
            Some(authenticated_client(&server)),
        )
        .unwrap()
    };

    // 1. Success / teardown: the installer is replaced by the target's screen,
    //    so dropping it must return the handle to its single caller-side owner.
    let (first, _) = source("Savings");
    let (installer, _) = build(first, temp_root("release-success"));
    assert_eq!(
        Arc::strong_count(&breez),
        2,
        "parked while the flow is live"
    );
    drop(installer);
    assert_eq!(
        Arc::strong_count(&breez),
        1,
        "a dropped installer must not keep the source Cube's SDK client alive"
    );

    // 2. Cancel: ownership moves to the Loader that rebuilds the source Cube —
    //    still exactly one extra owner, and it is the one that needs it.
    let (second, _) = source("Savings");
    let root = temp_root("release-cancel");
    let source_dir = root.network_directory(ChainId::Bitcoin);
    std::fs::create_dir_all(source_dir.path()).unwrap();
    std::fs::write(
        source_dir
            .path()
            .join(crate::app::config::DEFAULT_FILE_NAME),
        b"",
    )
    .unwrap();
    let (installer, _) = build(second, root.clone());
    let mut tab =
        crate::gui::tab::Tab::new(0, crate::gui::tab::State::Installer(Box::new(installer)));
    let _ = tab.update(crate::gui::tab::Message::Install(Message::BackToApp(
        Network::Bitcoin,
    )));
    assert_eq!(
        Arc::strong_count(&breez),
        2,
        "the Loader rebuilding the source Cube owns it now"
    );
    drop(tab);
    assert_eq!(
        Arc::strong_count(&breez),
        1,
        "and closing that screen releases it"
    );
    let _ = std::fs::remove_dir_all(root.path());
}

/// **Failure after the seed write, then a restart.** The requested regression
/// for the identity-recovery finding.
///
/// An install that dies after `persist_cube_master_seed` leaves a seed file on
/// disk named by fingerprint alone and encrypted bound to that attempt's Cube
/// id. If the retry minted a fresh id it would find that file, fail to decrypt
/// it, and refuse — and so would every attempt after it, permanently blocking
/// claims from that source Cube on that device. Here the first attempt fails
/// for a reason the user cannot control (the daemon config cannot be written),
/// and the second — a fresh installer, as a restart would build — completes.
#[tokio::test]
async fn a_claim_interrupted_after_the_seed_write_can_be_retried_after_a_restart() {
    let _guard = crate::app::session::test_guard();
    let (server, esplora) = fork_install_server().await;
    let (source, source_fingerprint) = source("Savings");
    let source_cube_id = source.cube_id().to_string();
    let root = temp_root("interrupted");
    enable_beta(&root);

    let backend = BitcoinBackend::Esplora(coincubed::config::EsploraConfig {
        addr: esplora,
        token: None,
        fallback_addr: None,
        fallback_token: None,
        secondary_fallback_addr: None,
        secondary_fallback_token: None,
    });
    let build = |src: ClaimSource| {
        let (mut installer, _) = Installer::try_new_for_chain(
            root.clone(),
            ChainId::BitcoinBlake2b,
            UserFlow::ClaimBlake2b {
                from_cube: Box::new(src),
            },
            true,
            None,
            None,
            None,
            false,
            Some(authenticated_client(&server)),
        )
        .unwrap();
        installer.context.bitcoin_backend = Some(backend.clone());
        installer
    };

    // Attempt 1: make the post-seed daemon-config write fail by occupying its
    // path with a directory. Nothing about the seed write itself is touched.
    let first = build(source.clone());
    let wallet_id = WalletId::generate(first.context.descriptor.as_ref().unwrap());
    let daemon_toml = root
        .network_directory(ChainId::BitcoinBlake2b)
        .coincubed_data_directory(&wallet_id)
        .path()
        .join("daemon.toml");
    std::fs::create_dir_all(&daemon_toml).unwrap();
    // Registered immediately before the install, never once at the top: see
    // `register_anchor`.
    let anchor = register_anchor(&server).await;
    let failed = install_local_wallet(
        first.context.clone(),
        wallet_id.clone(),
        first.signer.clone(),
    )
    .await;
    assert!(
        failed.is_err(),
        "the injected failure must land after the seed write"
    );
    let seed_path = coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(
        root.path(),
        ChainId::BitcoinBlake2b,
    )
    .join(
        coincube_core::signer::MnemonicFileName {
            fingerprint: source_fingerprint,
            descriptor_info: None,
        }
        .to_string(),
    );
    assert!(
        seed_path.exists(),
        "the failed attempt left its seed file behind — that is the condition \
         under test, not an accident of this fixture"
    );

    // Attempt 2: what a restart builds. Clear the injected failure first.
    std::fs::remove_dir(&daemon_toml).unwrap();
    let second = build(source);
    assert_eq!(
        second.context.seed_cube_id(),
        first.context.seed_cube_id(),
        "a retry must land on the identity the leftover seed file was written \
         for, or that file blocks every future attempt"
    );
    // A restart drops the old installer and its creation lease. Keeping it
    // alive would correctly block a second installer in the same process.
    drop(first);
    let identity = crate::gui::tab::installer_exit_identity(&second).unwrap();
    let exit_seed = crate::gui::tab::installer_exit_seed(&second).unwrap();
    anchor.delete_async().await;
    let _anchor = register_anchor(&server).await;
    install_local_wallet(
        second.context.clone(),
        WalletId::generate(second.context.descriptor.as_ref().unwrap()),
        second.signer.clone(),
    )
    .await
    .expect("the retry must complete rather than refuse the leftover seed");

    // And the Cube the retry mints still opens that file.
    coincube_core::seed_crypt::decrypt_with(
        &std::fs::read(&seed_path).unwrap(),
        exit_seed.pin.as_str(),
        &identity.uuid,
        None,
    )
    .expect("the retried Cube's credential opens the seed the first attempt wrote");
    assert_ne!(identity.uuid, source_cube_id, "still its own Cube (I7)");
    let _ = std::fs::remove_dir_all(root.path());
}

/// The seed file is named by `(chain, fingerprint)` with no Cube component, so
/// two claims can meet on one path. Both meetings are pinned here.
///
/// *Same source Cube twice* — a retry, or a second claim — derives the same
/// target identity, so the second attempt opens the first's file and continues.
/// *Two different source Cubes that share one master seed* — restoring one
/// mnemonic into two Cubes — derive different identities, so the second is
/// **refused** rather than allowed to overwrite or adopt the first target's
/// only seed. A refusal is the right outcome: the alternative is one Cube's
/// seed file answering for another Cube.
#[test]
fn two_claims_meeting_on_one_seed_path_reuse_or_refuse_but_never_adopt() {
    let _guard = crate::app::session::test_guard();
    let root = temp_root("collision");
    let (first, fingerprint) = source("Savings");

    // Same source, second attempt: same identity, same credentials, accepted.
    let again = first.clone();
    assert_eq!(
        claim::target_cube_id(&first),
        claim::target_cube_id(&again),
        "a second attempt from one Cube is the same target"
    );
    let seed = claim::target_master_seed(&first).unwrap();
    let id = claim::target_cube_id(&first);
    persist_cube_master_seed(&seed, &root, ChainId::BitcoinBlake2b, SOURCE_PIN, &id, None).unwrap();
    persist_cube_master_seed(&seed, &root, ChainId::BitcoinBlake2b, SOURCE_PIN, &id, None)
        .expect("the same target may re-open its own seed file");

    // A different Cube holding the same seed: different identity, refused.
    let mut sibling = first.clone();
    sibling.cube = crate::app::settings::CubeSettings::new_with_raw_id(
        uuid::Uuid::new_v4().to_string(),
        "Restored twin".to_string(),
        ChainId::Bitcoin,
    );
    assert_ne!(claim::target_cube_id(&sibling), id);
    let refused = persist_cube_master_seed(
        &claim::target_master_seed(&sibling).unwrap(),
        &root,
        ChainId::BitcoinBlake2b,
        SOURCE_PIN,
        &claim::target_cube_id(&sibling),
        None,
    );
    assert!(
        refused.is_err(),
        "a second Cube must not adopt or overwrite the first target's seed"
    );
    // And the claim path replaces the generic storage message with one that
    // says what happened and what to do.
    // The production mapper the claim install calls, not a copy of it. Both
    // storage messages must map: which one a collision produces depends on
    // whether the existing file decrypts at all under this attempt's
    // credentials, and only one of the two is obvious from reading the code.
    let raw = refused.unwrap_err().to_string();
    let claim_facing = claim_seed_error(crate::installer::Error::Unexpected(raw.clone()));
    assert!(
        claim_facing
            .to_string()
            .contains("already has a Bitcoin Blake2b claim"),
        "unmapped storage message reached the user: {}",
        raw
    );
    for message in [
        "Existing seed file conflicted with unlock credentials: Invalid password",
        "Existing Cube master seed does not match this installation",
    ] {
        assert!(
            claim_seed_error(crate::installer::Error::Unexpected(message.to_string()))
                .to_string()
                .contains("already has a Bitcoin Blake2b claim"),
            "{}",
            message
        );
    }
    // And the first target's file is untouched.
    let path = coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(
        root.path(),
        ChainId::BitcoinBlake2b,
    )
    .join(
        coincube_core::signer::MnemonicFileName {
            fingerprint,
            descriptor_info: None,
        }
        .to_string(),
    );
    assert!(
        coincube_core::seed_crypt::decrypt_with(
            &std::fs::read(&path).unwrap(),
            SOURCE_PIN,
            &id,
            None
        )
        .is_ok(),
        "the first target still opens its own seed"
    );
    let _ = std::fs::remove_dir_all(root.path());
}

/// **The write boundary itself**, not a proxy for it. `install_local_wallet`
/// persists the wallet; the exit seam persists the Cube. An interruption
/// between the two used to leave the claim entry hidden (a wallet checksum is
/// present, so "already claimed") while fork Home had no Cube to open — locked
/// out in both directions, and the derived identity could not help because
/// admission refuses before the id is ever consulted.
#[tokio::test]
async fn an_install_interrupted_between_the_wallet_and_the_cube_stays_retryable() {
    let _guard = crate::app::session::test_guard();
    let (server, esplora) = fork_install_server().await;
    let (source, _) = source("Savings");
    let root = temp_root("write-boundary");
    enable_beta(&root);
    let backend = BitcoinBackend::Esplora(coincubed::config::EsploraConfig {
        addr: esplora,
        token: None,
        fallback_addr: None,
        fallback_token: None,
        secondary_fallback_addr: None,
        secondary_fallback_token: None,
    });
    let build = |src: ClaimSource| {
        let (mut installer, _) = Installer::try_new_for_chain(
            root.clone(),
            ChainId::BitcoinBlake2b,
            UserFlow::ClaimBlake2b {
                from_cube: Box::new(src),
            },
            true,
            None,
            None,
            None,
            false,
            Some(authenticated_client(&server)),
        )
        .unwrap();
        installer.context.bitcoin_backend = Some(backend.clone());
        installer
    };

    // Stop here: the wallet is written, the Cube is not.
    let first = build(source.clone());
    let checksum = first.context.descriptor.as_ref().unwrap();
    let checksum = WalletId::generate(checksum).descriptor_checksum;
    let anchor = register_anchor(&server).await;
    install_local_wallet(
        first.context.clone(),
        WalletId::generate(first.context.descriptor.as_ref().unwrap()),
        first.signer.clone(),
    )
    .await
    .unwrap();

    let fork_settings =
        crate::app::settings::Settings::from_file(&root.network_directory(ChainId::BitcoinBlake2b))
            .unwrap();
    assert!(
        fork_settings
            .wallets
            .iter()
            .any(|w| w.descriptor_checksum == checksum),
        "the wallet is on disk — this is the state an interruption leaves"
    );
    assert!(
        fork_settings.cubes.is_empty(),
        "and no Cube yet: that is the boundary under test"
    );
    assert!(
        !crate::app::claim_target_exists(&root, &checksum),
        "so the claim must still be offered — a wallet with no Cube is nothing \
         the user can open"
    );

    assert!(
        crate::app::features::claim_blake2b(crate::app::features::ClaimSourceCube {
            chain: ChainId::Bitcoin,
            has_vault: true,
            server_enabled: true,
            already_claimed: crate::app::claim_target_exists(&root, &checksum),
        })
        .is_available()
    );

    // Restart and complete: same derived identity, and now a Cube exists.
    anchor.delete_async().await;
    let _anchor = register_anchor(&server).await;
    let second = build(source);
    // A restart drops the old installer and its creation lease. Keeping it
    // alive would correctly block a second installer in the same process.
    drop(first);
    let identity = crate::gui::tab::installer_exit_identity(&second).unwrap();
    let exit_seed = crate::gui::tab::installer_exit_seed(&second).unwrap();
    let wallet_id = WalletId::generate(second.context.descriptor.as_ref().unwrap());
    install_local_wallet(
        second.context.clone(),
        wallet_id.clone(),
        second.signer.clone(),
    )
    .await
    .expect("the retry completes");
    let cube = crate::gui::tab::find_or_create_cube(
        &root.network_directory(ChainId::BitcoinBlake2b),
        Some(&crate::app::settings::VaultIdentity {
            wallet_id,
            fingerprint: None,
        }),
        &Some(second.context.wallet_alias.clone()),
        ChainId::BitcoinBlake2b,
        None,
        Some(identity),
        Some(&exit_seed),
    )
    .await
    .expect("the exit seam mints the Cube");

    assert_eq!(cube.id, second.context.seed_cube_id());
    assert!(
        crate::app::claim_target_exists(&root, &checksum),
        "and only now is the source Cube claimed"
    );

    // Finish where the user would: the Cube reloaded from disk, and the seed
    // opened using the reloaded Cube's identity. The lookup below also
    // verifies that the installer persisted the expected id.
    let persisted =
        crate::app::settings::Settings::from_file(&root.network_directory(ChainId::BitcoinBlake2b))
            .unwrap()
            .cubes
            .into_iter()
            .find(|c| c.id == cube.id)
            .expect("the target Cube is on disk");
    let seed_path = coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(
        root.path(),
        ChainId::BitcoinBlake2b,
    )
    .join(
        coincube_core::signer::MnemonicFileName {
            fingerprint: exit_seed.master_signer_fingerprint,
            descriptor_info: None,
        }
        .to_string(),
    );
    coincube_core::seed_crypt::decrypt_with(
        &std::fs::read(&seed_path).unwrap(),
        exit_seed.pin.as_str(),
        &persisted.id,
        None,
    )
    .expect("the persisted Cube's own id opens the seed the install wrote");
    let _ = std::fs::remove_dir_all(root.path());
}

/// A source Vault on the retired Liana Connect backend has no local daemon, so
/// returning it through the Loader would try to bring up a daemon for a Vault
/// that lives on that backend. Cancel goes back to the Launcher and says where
/// the wallet still lives.
#[tokio::test]
async fn cancelling_a_claim_from_a_legacy_liana_connect_source_returns_to_the_launcher() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (mut source, _) = source("Connect Cube");
    let root = temp_root("cancel-remote");
    let source_dir = root.network_directory(ChainId::Bitcoin);
    std::fs::create_dir_all(source_dir.path()).unwrap();
    std::fs::write(
        source_dir
            .path()
            .join(crate::app::config::DEFAULT_FILE_NAME),
        b"",
    )
    .unwrap();

    // A Vault on the retired Liana Connect backend, recorded the way the source Cube's
    // own settings record it.
    let wallet_id = WalletId::generate(&source.descriptor);
    source.cube.vault_wallet_id = Some(wallet_id.clone());
    let mut wallet = crate::app::settings::WalletSettings {
        name: "Vault".to_string(),
        alias: None,
        descriptor_checksum: wallet_id.descriptor_checksum.clone(),
        pinned_at: wallet_id.timestamp,
        keys: Vec::new(),
        hardware_wallets: Vec::new(),
        remote_backend_auth: None,
        start_internal_bitcoind: None,
        pending_rescan: None,
        keychain_keys_recorded: false,
    };
    wallet.remote_backend_auth = Some(crate::app::settings::AuthConfig::new(
        "user@example.test".to_string(),
        "remote-wallet-id".to_string(),
    ));
    std::fs::write(
        source_dir
            .path()
            .join(crate::app::settings::SETTINGS_FILE_NAME),
        serde_json::to_vec(&crate::app::settings::Settings {
            wallets: vec![wallet],
            cubes: vec![source.cube.clone()],
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let (installer, _) = Installer::try_new_for_chain(
        root.clone(),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        Some(Arc::new(
            crate::app::breez_liquid::BreezClient::disconnected(Network::Bitcoin),
        )),
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();

    let mut tab =
        crate::gui::tab::Tab::new(0, crate::gui::tab::State::Installer(Box::new(installer)));
    let _ = tab.update(crate::gui::tab::Message::Install(Message::BackToApp(
        Network::Bitcoin,
    )));
    let crate::gui::tab::State::Home(home) = &tab.state else {
        panic!(
            "a legacy Liana Connect source must return to the Launcher, not a local daemon startup"
        );
    };
    assert!(home
        .error()
        .is_some_and(|e| e.contains("user@example.test")));
    let _ = std::fs::remove_dir_all(root.path());
}

/// Cancel must restore the Vault the source Cube actually points at.
///
/// `WalletId` is the descriptor checksum **and** the timestamp, so two Vaults
/// built from one descriptor — a re-created one, a pinned sibling — share a
/// checksum and differ only in `pinned_at`. Matching on the checksum alone
/// selected whichever came first in the file; the ordinary open path has always
/// matched the whole id (`vault_settings_for_cube`, used at `tab.rs:1043`), and
/// cancel now uses the same helper.
#[tokio::test]
async fn cancelling_a_claim_restores_the_vault_the_source_cube_points_at() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (mut source, _) = source("Savings");
    let root = temp_root("cancel-walletid");
    let source_dir = root.network_directory(ChainId::Bitcoin);
    std::fs::create_dir_all(source_dir.path()).unwrap();
    std::fs::write(
        source_dir
            .path()
            .join(crate::app::config::DEFAULT_FILE_NAME),
        b"",
    )
    .unwrap();

    // Two Vaults, one descriptor: the decoy is written first, so a
    // checksum-only lookup returns it.
    let checksum = WalletId::generate(&source.descriptor).descriptor_checksum;
    let wanted = WalletId::new(checksum.clone(), Some(222));
    let decoy = WalletId::new(checksum.clone(), Some(111));
    let wallet = |id: &WalletId, name: &str| crate::app::settings::WalletSettings {
        name: name.to_string(),
        alias: None,
        descriptor_checksum: id.descriptor_checksum.clone(),
        pinned_at: id.timestamp,
        keys: Vec::new(),
        hardware_wallets: Vec::new(),
        remote_backend_auth: None,
        start_internal_bitcoind: None,
        pending_rescan: None,
        keychain_keys_recorded: false,
    };
    source.cube.vault_wallet_id = Some(wanted.clone());
    std::fs::write(
        source_dir
            .path()
            .join(crate::app::settings::SETTINGS_FILE_NAME),
        serde_json::to_vec(&crate::app::settings::Settings {
            wallets: vec![wallet(&decoy, "Decoy"), wallet(&wanted, "Wanted")],
            cubes: vec![source.cube.clone()],
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let (installer, _) = Installer::try_new_for_chain(
        root.clone(),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        Some(Arc::new(
            crate::app::breez_liquid::BreezClient::disconnected(Network::Bitcoin),
        )),
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();

    let mut tab =
        crate::gui::tab::Tab::new(0, crate::gui::tab::State::Installer(Box::new(installer)));
    let _ = tab.update(crate::gui::tab::Message::Install(Message::BackToApp(
        Network::Bitcoin,
    )));
    match &tab.state {
        crate::gui::tab::State::Loader(loader) => {
            let restored = loader
                .wallet_settings
                .as_ref()
                .expect("cancel restores the source Cube's Vault");
            assert_eq!(
                restored.wallet_id(),
                wanted,
                "cancel selected a different Vault of the same descriptor"
            );
            assert_eq!(restored.name, "Wanted");
        }
        other => panic!(
            "expected the source Loader, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    let _ = std::fs::remove_dir_all(root.path());
}

async fn tab_outputs(task: iced::Task<crate::gui::tab::Message>) -> Vec<crate::gui::tab::Message> {
    use iced_runtime::futures::futures::StreamExt;
    match iced_runtime::task::into_stream(task) {
        Some(stream) => {
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_runtime::Action::Output(msg) => Some(msg),
                        _ => None,
                    }
                })
                .collect()
                .await
        }
        None => Vec::new(),
    }
}

/// "Create copy" (Tools › Split coins) runs this installer in a tab the
/// source App's pane owns. Saving the copy hands control back to the pane
/// for that source App (which closes this tab and returns to Tools › Split
/// coins); nothing is unlocked here, the source is not reopened in this tab
/// and no Claim step follows. The copy opens later from the Launcher.
#[tokio::test]
async fn completing_copy_setup_returns_to_the_source_without_a_claim_handoff() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (source, source_fingerprint) = source("Savings");
    let source_id = source.cube_id().to_string();
    let root = temp_root("copy-handoff");
    for chain in [ChainId::Bitcoin, ChainId::BitcoinBlake2b] {
        let dir = root.network_directory(chain);
        dir.init().unwrap();
        crate::app::Config::new(false)
            .to_file(&dir.path().join(crate::app::config::DEFAULT_FILE_NAME))
            .unwrap();
    }
    crate::app::session::store_unlocked_signer(
        &source_id,
        source_fingerprint,
        coincube_core::signer::MasterSigner::generate(Network::Bitcoin).unwrap(),
    );

    let (installer, _) = Installer::try_new_for_chain(
        root.clone(),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();

    let target = crate::app::settings::CubeSettings::new_with_raw_id(
        installer.context.seed_cube_id().to_string(),
        "Savings · BTCB2".to_string(),
        ChainId::BitcoinBlake2b,
    );
    let owner = crate::app::cache::AppGeneration::next();
    let mut tab =
        crate::gui::tab::Tab::new(0, crate::gui::tab::State::Installer(Box::new(installer)));
    tab.set_copy_owner(owner);
    assert_eq!(tab.copy_owner(), Some(owner));
    let messages = tab_outputs(
        tab.update(crate::gui::tab::Message::Install(Message::CubeSaved(
            Ok((target.clone(), None, None)),
            None,
            None,
        ))),
    )
    .await;
    assert!(
        messages.iter().any(|m| matches!(
            m,
            crate::gui::tab::Message::CopyInstallFinished { owner: o, created: true } if *o == owner
        )),
        "the pane is told the copy exists, for the exact source App"
    );
    assert!(
        matches!(tab.state, crate::gui::tab::State::Installer(_)),
        "nothing is unlocked or loaded in this tab"
    );
    assert_eq!(tab.copy_owner(), None, "the save ends the ownership");
    assert!(
        crate::app::session::unlocked_signer(&source_id, source_fingerprint).is_some(),
        "the source stays unlocked in its own tab"
    );
    crate::app::session::close();
    let _ = std::fs::remove_dir_all(root.path());
}

/// Backing out of an owned copy installer hands control back to the pane,
/// which closes this tab and focuses the source App.
#[tokio::test]
async fn backing_out_of_an_owned_copy_returns_to_the_pane() {
    let _guard = crate::app::session::test_guard();
    let server = MockServer::start_async().await;
    let (source, _) = source("Savings");
    let root = temp_root("copy-cancel");
    let (installer, _) = Installer::try_new_for_chain(
        root.clone(),
        ChainId::BitcoinBlake2b,
        UserFlow::ClaimBlake2b {
            from_cube: Box::new(source),
        },
        true,
        None,
        None,
        None,
        false,
        Some(authenticated_client(&server)),
    )
    .unwrap();
    let mut tab =
        crate::gui::tab::Tab::new(0, crate::gui::tab::State::Installer(Box::new(installer)));
    let owner = crate::app::cache::AppGeneration::next();
    tab.set_copy_owner(owner);
    let messages = tab_outputs(
        tab.update(crate::gui::tab::Message::Install(Message::BackToApp(
            Network::Bitcoin,
        ))),
    )
    .await;
    assert!(messages.iter().any(|m| matches!(
        m,
        crate::gui::tab::Message::CopyInstallFinished { owner: o, created: false } if *o == owner
    )));
    assert_eq!(tab.copy_owner(), None);
    assert!(matches!(tab.state, crate::gui::tab::State::Installer(_)));
    let _ = std::fs::remove_dir_all(root.path());
}

/// `claim_target_exists` runs whenever Tools › Split coins opens, and the
/// fork settings file does not exist until the first copy is created.
///
/// `Settings::from_file` treats `NotFound` as possibly-transient and sleeps
/// between five attempts — right for a file that is supposed to exist, wrong
/// for a question whose ordinary answer is "nothing here yet". The retry floor
/// is 20+40+60+80+100 ms = 300 ms by construction, so a run under 100 ms can only
/// mean the retry was not entered.
#[test]
fn an_absent_fork_settings_file_answers_immediately_rather_than_retrying() {
    let root = temp_root("no-fork-settings");
    assert!(
        !root
            .network_directory(ChainId::BitcoinBlake2b)
            .path()
            .join(crate::app::settings::SETTINGS_FILE_NAME)
            .is_file(),
        "the fork settings file does not exist before the first claim"
    );

    let started = std::time::Instant::now();
    let answered = crate::app::claim_target_exists(&root, "any-checksum");
    let elapsed = started.elapsed();

    assert!(!answered);
    assert!(
        elapsed < std::time::Duration::from_millis(100),
        "answering \"no targets\" took {:?} — the NotFound retry budget is at \
         least 300 ms, so this went through it",
        elapsed
    );
}
