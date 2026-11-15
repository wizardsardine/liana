//! Synthetic end-to-end installer/PIN/embedded reopen boundary. No OS secrets.
use super::*;
use crate::chain::ChainId;
use crate::services::coincube::CoincubeClient;
use httpmock::prelude::*;
use serde_json::json;

const DESCRIPTOR: &str = concat!(
    "wsh(andor(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4z",
    "LqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*),older(10000),pk([aabbccdd]xpub68JJT",
    "Xc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8Uut",
    "BsBbgKHzaD5HkTkifK/<0;1>/*)))#3xh8xmhn"
);

fn enable_beta(root: &CoincubeDirectory) {
    std::fs::create_dir_all(root.path()).unwrap();
    let path = crate::app::settings::global::GlobalSettings::path(root);
    crate::app::settings::global::GlobalSettings::update_bitcoin_blake2b_beta(&path, true).unwrap();
}

/// An open Bitcoin Blake2b Cube: its PIN-encrypted master seed file and the
/// settings that record it. Vault creation runs inside such a Cube; it never
/// mints a master of its own.
fn fork_cube(root: &CoincubeDirectory, chain: ChainId) -> crate::app::settings::CubeSettings {
    let master = crate::signer::Signer::generate(chain.bitcoin_network()).unwrap();
    let mut cube = crate::app::settings::CubeSettings::new("Synthetic".into(), chain)
        .with_master_signer(master.fingerprint());
    cube.backed_up = true;
    master
        .store_encrypted_seed_only_for_chain(root, chain, "2468", &cube.id, None)
        .unwrap();
    cube
}

/// The Vault installer for `cube`, admitted from that Cube's open session.
/// The session is closed again before any `await`; the installer keeps the
/// Cube's PIN in its context.
fn in_cube_installer(
    root: &CoincubeDirectory,
    cube: &crate::app::settings::CubeSettings,
    client: CoincubeClient,
) -> Installer {
    let _guard = crate::app::session::test_guard();
    crate::app::session::open(cube.id.clone(), zeroize::Zeroizing::new("2468".into()));
    let built = Installer::try_new_for_chain(
        root.clone(),
        cube.network,
        UserFlow::CreateWallet,
        true,
        Some(cube.clone()),
        None,
        None,
        false,
        Some(client),
    );
    crate::app::session::close();
    let (installer, _) = built.unwrap();
    assert!(installer.context.in_cube_fork_vault);
    installer
}

/// Only the global settings, the optional admission lock and, when given, the
/// Cube's own master seed exist: no wallet, daemon or chain state.
fn assert_only_beta_settings_files(
    root: &CoincubeDirectory,
    cube: Option<&str>,
    cube_seed: Option<ChainId>,
) {
    let mut names: Vec<_> = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let mut expected = vec![
        "global_settings.json".to_owned(),
        "global_settings.lock".to_owned(),
    ];
    if let Some(chain) = cube_seed {
        let chain_dir = root.network_directory(chain);
        let entries: Vec<_> = std::fs::read_dir(chain_dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["mnemonics".to_owned()]);
        let seeds = std::fs::read_dir(
            coincube_core::signer::MasterSigner::mnemonics_folder_for_chain(root.path(), chain),
        )
        .unwrap()
        .count();
        assert_eq!(seeds, 1, "only the Cube's own master seed");
        expected.push(chain.dir_name().to_owned());
    }
    #[cfg(feature = "mcp-proposals")]
    if let Some(cube) = cube {
        use coincube_core::miniscript::bitcoin::hashes::{sha256, Hash};
        let key = sha256::Hash::hash(cube.as_bytes());
        let name = format!(".vault-creation-{key}.lock");
        // The shared admission mutex is metadata only. Feature refusal must
        // leave it empty: no proposal tombstone or wallet/chain state.
        let metadata = std::fs::metadata(root.path().join(&name)).unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.len(), 0);
        expected.push(name);
    }
    #[cfg(not(feature = "mcp-proposals"))]
    let _ = cube;
    names.sort();
    expected.sort();
    assert_eq!(names, expected);
}

fn client(server: &MockServer) -> CoincubeClient {
    let mut client = CoincubeClient::new();
    client.base_url = server.base_url();
    client.set_token("synthetic-activation-token");
    client
}

async fn output<T: Send + 'static>(task: Task<T>) -> Vec<T> {
    use iced_runtime::futures::futures::StreamExt;
    match iced_runtime::task::into_stream(task) {
        Some(stream) => {
            stream
                .filter_map(|action| async move {
                    match action {
                        iced_runtime::Action::Output(value) => Some(value),
                        _ => None,
                    }
                })
                .collect()
                .await
        }
        None => Vec::new(),
    }
}

/// This one end-to-end fixture needs a response-time clock. httpmock's JSON
/// response is fixed at registration; PIN encryption/decryption can take longer
/// than the anchor freshness window on loaded runners (#495, #521).
struct ActivationServer {
    base_url: String,
    features: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    bitcoin: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ActivationServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ActivationServer {
    async fn start(clock: impl Fn() -> u64 + Send + 'static) -> Self {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let features = Arc::new(AtomicUsize::new(0));
        let bitcoin = Arc::new(AtomicUsize::new(0));
        let feature_hits = features.clone();
        let bitcoin_hits = bitcoin.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let byte =
                        tokio::time::timeout(std::time::Duration::from_secs(5), stream.read_u8())
                            .await
                            .unwrap()
                            .unwrap();
                    request.push(byte);
                    assert!(request.len() <= 8192, "oversized synthetic HTTP request");
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let authorized = request.lines().any(|line| {
                    line.split_once(':').is_some_and(|(name, value)| {
                        name.eq_ignore_ascii_case("authorization")
                            && value.trim() == "Bearer synthetic-activation-token"
                    })
                });
                let anonymous_proxy = path.starts_with("/api/v1/esplora/");
                if anonymous_proxy {
                    assert!(
                        !request.lines().any(|line| line
                            .split_once(':')
                            .is_some_and(|(name, _)| name.eq_ignore_ascii_case("authorization"))),
                        "Connect credentials must not reach anonymous Esplora routes"
                    );
                }
                let (status, body) = if !anonymous_proxy && !authorized {
                    (401, String::new())
                } else if path == "/api/v1/connect/features" {
                    feature_hits.fetch_add(1, Ordering::Relaxed);
                    (
                        200,
                        json!({"success":true,"data":{"plans":[],"bitcoinBlake2bEnabled":true}})
                            .to_string(),
                    )
                } else if path == "/api/v1/connect/networks/bitcoin-blake2b/anchor" {
                    (200, json!({"success":true,"data":{"network":"bitcoin-blake2b","state":"available","anchor":{
                        "tip_hash":"11".repeat(32),"tip_height":973029,"tip_median_time_past":1800000000,
                        "observed_at":clock(),
                        "observation":{"tip_height":973029,"fork":{"height":972000,"active":true},
                            "rdts":{"state":"flagday","flagday":{"height":972000,"expiry_time":1800010000_i64,"active":false}}}
                    }}}).to_string())
                } else if let Some(route) =
                    path.strip_prefix("/api/v1/esplora/bitcoin-blake2b/mainnet")
                {
                    match route {
                        "/block-height/0" => (
                            200,
                            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
                                .into(),
                        ),
                        "/block-height/973029" | "/blocks/tip/hash" => (200, "11".repeat(32)),
                        route if route == format!("/block/{}/status", "11".repeat(32)) => (
                            200,
                            json!({"in_best_chain":true,"height":973029,"next_best":null})
                                .to_string(),
                        ),
                        _ => (404, String::new()),
                    }
                } else {
                    if path.contains("/esplora/bitcoin/") {
                        bitcoin_hits.fetch_add(1, Ordering::Relaxed);
                    }
                    (404, String::new())
                };
                let response = format!("HTTP/1.1 {status} Synthetic\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        Self {
            base_url,
            features,
            bitcoin,
            task,
        }
    }
}

#[tokio::test]
async fn activation_anchor_timestamp_is_generated_for_each_request() {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    let clock = Arc::new(AtomicU64::new(1000));
    let response_clock = clock.clone();
    let server = ActivationServer::start(move || response_clock.load(Ordering::Relaxed)).await;
    let http = reqwest::Client::new();
    for now in [1000, 2000] {
        clock.store(now, Ordering::Relaxed);
        let response: serde_json::Value = http
            .get(format!(
                "{}/api/v1/connect/networks/bitcoin-blake2b/anchor",
                server.base_url
            ))
            .bearer_auth("synthetic-activation-token")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["data"]["anchor"]["observed_at"], now);
    }
}

#[tokio::test]
async fn actual_fork_install_pin_unlock_and_authenticated_reopen_are_chain_bound() {
    let server = ActivationServer::start(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    })
    .await;
    let prefix = "/api/v1/esplora/bitcoin-blake2b/mainnet";
    let root_path =
        std::env::temp_dir().join(format!("btcb2-create-reopen-{}", uuid::Uuid::new_v4()));
    let root = CoincubeDirectory::new(root_path.clone());
    enable_beta(&root);
    let mut client = CoincubeClient::new();
    client.base_url = server.base_url.clone();
    client.set_token("synthetic-activation-token");
    let cube = fork_cube(&root, ChainId::BitcoinBlake2b);
    let mut installer = in_cube_installer(&root, &cube, client.clone());
    installer.context.descriptor = Some(DESCRIPTOR.parse().unwrap());
    installer.context.bitcoin_backend =
        Some(BitcoinBackend::Esplora(coincubed::config::EsploraConfig {
            addr: format!("{}{prefix}", server.base_url),
            token: None,
            fallback_addr: None,
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        }));
    assert_eq!(installer.context.seed_cube_id(), cube.id);
    let fingerprint = installer.master_signer_fingerprint();
    assert_eq!(Some(fingerprint), cube.master_signer_fingerprint);
    let wallet_id = WalletId::generate(installer.context.descriptor.as_ref().unwrap());
    assert_only_beta_settings_files(&root, None, Some(ChainId::BitcoinBlake2b));
    let settings = install_local_wallet(
        installer.context.clone(),
        wallet_id,
        installer.signer.clone(),
    )
    .await
    .unwrap();
    assert!(!root_path.join("bitcoin").exists());
    let cfg_path = root
        .network_directory(ChainId::BitcoinBlake2b)
        .coincubed_data_directory(&settings.wallet_id())
        .path()
        .join("daemon.toml");
    let database_path = cfg_path.with_file_name("coincubed.sqlite3");
    assert!(!std::fs::read_to_string(cfg_path)
        .unwrap()
        .contains("synthetic-activation-token"));

    let mut entry = crate::pin_entry::PinEntry::new(
        cube,
        root_path.clone(),
        crate::pin_entry::PinEntrySuccess::LoadApp {
            datadir: root.clone(),
            config: crate::app::Config::new(false),
            network: bitcoin::Network::Bitcoin,
            internal_bitcoind: None,
            backup: None,
            wallet_settings: Some(settings.clone()),
            connect_client: Some(client.clone()),
        },
        None,
    );
    for (i, digit) in "2468".chars().enumerate() {
        let _ = entry.update(crate::pin_entry::Message::PinInput(
            crate::pin_input::Message::DigitChanged(i, digit.to_string()),
        ));
    }
    let messages = output(entry.update(crate::pin_entry::Message::Submit)).await;
    assert!(matches!(
        messages.as_slice(),
        [crate::pin_entry::Message::ForkClassified(
            _,
            Ok(crate::pin_entry::Verdict::Unlock)
        )]
    ));
    for message in messages {
        assert!(matches!(
            output(entry.update(message)).await.as_slice(),
            [crate::pin_entry::Message::PinVerified]
        ));
    }
    let unlocked = entry.take_fork_signer().unwrap();
    assert_eq!(
        unlocked.fingerprint(&bitcoin::secp256k1::Secp256k1::signing_only()),
        fingerprint
    );
    assert!(entry.take_fork_signer().is_none());
    let (daemon, node, info) = crate::loader::start_connect_daemon(
        root.clone(),
        ChainId::BitcoinBlake2b,
        settings,
        client,
    )
    .await
    .unwrap();
    assert!(node.is_none());
    assert_eq!(info.network, bitcoin::Network::Bitcoin);
    daemon.stop().await.unwrap();
    // Reopening exercises daemon preflight against the stored exact-chain identity.
    assert!(database_path.is_file());
    assert_eq!(
        server.features.load(std::sync::atomic::Ordering::Relaxed),
        3
    );
    assert_eq!(server.bitcoin.load(std::sync::atomic::Ordering::Relaxed), 0);
    std::fs::remove_dir_all(root_path).unwrap();
}

#[tokio::test]
async fn feature_refusals_precede_anchor_requests_and_all_fork_filesystem_writes() {
    for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
        for (status, flag) in [
            (200, None),
            (200, Some(false)),
            (401, None),
            (429, None),
            (503, None),
        ] {
            let server = MockServer::start_async().await;
            let features = server
                .mock_async(|when, then| {
                    when.method(GET).path("/api/v1/connect/features");
                    then.status(status).json_body(
                        json!({"success":true,"data":{"plans":[],"bitcoinBlake2bEnabled":flag}}),
                    );
                })
                .await;
            let anchor = server
                .mock_async(|when, then| {
                    when.path_contains("/anchor");
                    then.status(500);
                })
                .await;
            let root_path =
                std::env::temp_dir().join(format!("btcb2-no-write-{}", uuid::Uuid::new_v4()));
            let root = CoincubeDirectory::new(root_path.clone());
            enable_beta(&root);
            let settings_path = crate::app::settings::global::GlobalSettings::path(&root);
            let settings_before = std::fs::read(&settings_path).unwrap();
            let cube = fork_cube(&root, chain);
            let mut installer = in_cube_installer(&root, &cube, client(&server));
            installer.context.descriptor = Some(DESCRIPTOR.parse().unwrap());
            installer.context.bitcoin_backend =
                Some(BitcoinBackend::Esplora(coincubed::config::EsploraConfig {
                    addr: format!(
                        "{}/api/v1/esplora/{}",
                        server.base_url(),
                        connect_esplora_path(chain)
                    ),
                    token: None,
                    fallback_addr: None,
                    fallback_token: None,
                    secondary_fallback_addr: None,
                    secondary_fallback_token: None,
                }));
            let wallet_id = WalletId::generate(installer.context.descriptor.as_ref().unwrap());
            let cube_id = installer.context.cube_id.clone();
            assert!(
                install_local_wallet(installer.context, wallet_id, installer.signer)
                    .await
                    .is_err()
            );
            assert_only_beta_settings_files(
                &CoincubeDirectory::new(root_path.clone()),
                cube_id.as_deref(),
                Some(chain),
            );
            assert_eq!(std::fs::read(&settings_path).unwrap(), settings_before);
            std::fs::remove_dir_all(root_path).unwrap();
            features.assert_hits_async(1).await;
            anchor.assert_hits_async(0).await;
        }
    }
}
