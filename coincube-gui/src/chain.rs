//! Chain identity as the GUI sees it: the shared [`ChainId`] representation
//! plus the *policy* this build applies to it.
//!
//! The identity type itself — seven variants, serde / directory / Connect
//! spellings, the lossy `bitcoin::Network` projection — lives in
//! [`coincube_core::chain`] so that the GUI, the daemon and core all agree on
//! one type and one wire encoding; it is re-exported here unchanged, and
//! every existing `crate::chain::ChainId` path keeps resolving to it. What
//! stays in this module is what only the GUI decides: which chains the
//! launcher offers, the user-facing label and ticker, and whether this build
//! can run a Cube on a chain at all ([`ChainIdExt`]).
//!
//! # Explicit fork capabilities
//!
//! Generic runtime support remains dormant for BTCB2 so managed-node,
//! migration and duress entry points stay closed. The separately checked
//! authenticated Connect Vault path uses `authenticated_connect_support`;
//! its account flag, exact-chain anchor and dedicated provider are all
//! required before daemon writes. The isolated managed local path instead uses
//! the anonymous global flag and saved beta preference, with exact local-chain
//! admission. Neither path enables generic startup.
use coincube_ui::locale::Text as CopyText;

pub use coincube_core::chain::{ChainId, UnknownChainId};

/// Whether this build can actually run a Cube on a chain, or merely knows
/// the chain's identity.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuntimeSupport {
    /// Daemon, node, providers and signing are wired for this chain.
    Supported,
    /// The identity is known (so its settings and directories are kept
    /// distinct and intact) but the runtime behind it is not in this build.
    /// Generic start paths refuse with `reason`; separately authenticated
    /// capabilities must enforce their own admission before writes.
    Dormant { reason: &'static str },
}

impl RuntimeSupport {
    pub fn is_supported(self) -> bool {
        matches!(self, RuntimeSupport::Supported)
    }
}

/// User-facing copy for refusing to open a Bitcoin Blake2b Cube in a build
/// that only carries its identity. Says what is true — the Cube and its
/// settings are intact, this version can't run it — and nothing about the
/// sender/desktop being at fault.
pub const BTCB2_DORMANT_REASON: &str = "This Cube is on Bitcoin Blake2b, which this version of \
     COINCUBE Desktop can't open yet. Its settings are untouched; update COINCUBE Desktop to a version with \
     Bitcoin Blake2b support to use it.";

/// Capability for the explicit authenticated Connect Vault path only.
/// Generic startup, managed nodes, duress and migration still consult
/// `runtime_support()` and remain dormant for the fork.
pub(crate) fn authenticated_connect_support(chain: ChainId) -> RuntimeSupport {
    if chain.is_blake2b() {
        RuntimeSupport::Supported
    } else {
        chain.runtime_support()
    }
}

/// Recheck the current account flag before a fork daemon may create files.
/// The authenticated anchor subsequently validates the exact selected chain.
pub(crate) async fn require_connect_feature(
    chain: ChainId,
    client: &crate::services::coincube::CoincubeClient,
    root: &crate::dir::CoincubeDirectory,
) -> Result<(), String> {
    if !chain.is_blake2b() || client.token().is_none() {
        return Err("An authenticated Bitcoin Blake2b Connect session is required".into());
    }
    let settings_path = crate::app::settings::global::GlobalSettings::path(root);
    if !crate::app::settings::global::GlobalSettings::load_bitcoin_blake2b_beta(&settings_path) {
        return Err("Enable Bitcoin Blake2b - Beta in Global Settings to open this Cube".into());
    }
    let features = client
        .get_connect_features()
        .await
        .map_err(|_| "Bitcoin Blake2b availability could not be verified".to_string())?;
    if !crate::app::features::bitcoin_blake2b_enabled(
        features.bitcoin_blake2b_enabled == Some(true),
        crate::app::settings::global::GlobalSettings::load_bitcoin_blake2b_beta(&settings_path),
    ) {
        return Err("Bitcoin Blake2b isn't enabled for this account".into());
    }
    Ok(())
}

/// Read only the globally published fork capability, with no account bearer.
pub(crate) async fn global_blake2b_enabled(
    mut client: crate::services::coincube::CoincubeClient,
) -> bool {
    client.clear_token();
    client
        .get_connect_features()
        .await
        .ok()
        .and_then(|features| features.bitcoin_blake2b_enabled)
        == Some(true)
}

/// Local node admission still requires the saved beta preference and global flag.
pub(crate) async fn require_local_feature(
    chain: ChainId,
    client: Option<crate::services::coincube::CoincubeClient>,
    root: &crate::dir::CoincubeDirectory,
) -> Result<(), String> {
    let path = crate::app::settings::global::GlobalSettings::path(root);
    if !chain.is_blake2b()
        || !crate::app::settings::global::GlobalSettings::load_bitcoin_blake2b_beta(&path)
    {
        return Err("Enable Bitcoin Blake2b - Beta in Global Settings to open this Cube".into());
    }
    if !global_blake2b_enabled(client.unwrap_or_default()).await
        || !crate::app::settings::global::GlobalSettings::load_bitcoin_blake2b_beta(&path)
    {
        return Err("Bitcoin Blake2b availability could not be verified".into());
    }
    Ok(())
}

/// Classify only the isolated, managed local fork backend. Esplora companions,
/// arbitrary RPC nodes and generic sockets never acquire this capability.
pub(crate) fn is_managed_local_fork(
    cfg: &coincubed::config::Config,
    root: &crate::dir::CoincubeDirectory,
) -> bool {
    let chain = cfg.bitcoin_config.chain;
    let directory = crate::node::bitcoind::internal_bitcoind_datadir_for(
        root,
        crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
    );
    let Ok(managed) = crate::node::bitcoind::InternalBitcoindConfig::from_file(
        &crate::node::bitcoind::internal_bitcoind_config_path(&directory),
    ) else {
        return false;
    };
    let Ok(ledger) = crate::node::revalidate::ManagedNodeState::try_load_for(
        root,
        crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
    ) else {
        return false;
    };
    let flavor = ledger
        .configured_flavor
        .or(ledger.last_run_flavor)
        .unwrap_or(crate::node::bitcoind::NodeFlavor::KnotsBlake2b);
    chain.is_blake2b()
        && cfg.bitcoin_config.check_chain_encoding().is_ok()
        && flavor == crate::node::bitcoind::NodeFlavor::KnotsBlake2b
        && cfg.pending_bitcoind.is_none()
        && matches!(&cfg.bitcoin_backend,
            Some(coincubed::config::BitcoinBackend::Bitcoind(node))
                if node.addr.ip().is_loopback()
                    && managed.networks.get(&chain.bitcoin_network()).is_some_and(|network| network.rpc_port == node.addr.port())
                    && matches!(&node.rpc_auth, coincubed::config::BitcoindRpcAuth::CookieFile(cookie)
                        if same_path(cookie, &crate::node::bitcoind::internal_bitcoind_cookie_path(
                            &directory, &chain.bitcoin_network()))))
}

pub(crate) fn same_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    normalized_path(a) == normalized_path(b)
}

/// `path` canonicalized through its nearest existing ancestor, so one that
/// does not exist (yet, or any more) still compares equal to its canonical
/// spelling (`\\?\C:\…` on Windows, through symlinks elsewhere).
pub(crate) fn normalized_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        if let Ok(mut normalized) = std::fs::canonicalize(ancestor) {
            for name in suffix.iter().rev() {
                normalized.push(name);
            }
            return normalized;
        }
        let Some(name) = ancestor.file_name() else {
            return path.to_path_buf();
        };
        suffix.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            return path.to_path_buf();
        };
        ancestor = parent;
    }
}

/// Read the Cube's own config before deciding whether its unlock needs Connect.
pub(crate) fn has_managed_local_fork(
    root: &crate::dir::CoincubeDirectory,
    chain: ChainId,
    wallet: Option<&crate::app::settings::WalletSettings>,
) -> bool {
    let Some(wallet) = wallet.filter(|w| w.remote_backend_auth.is_none()) else {
        return false;
    };
    let directory = root
        .network_directory(chain)
        .coincubed_data_directory(&wallet.wallet_id());
    let Ok(cfg) = coincubed::config::Config::from_file(Some(directory.path().join("daemon.toml")))
    else {
        return false;
    };
    cfg.bitcoin_config.chain == chain
        && std::path::absolute(directory.path())
            .ok()
            .is_some_and(|expected| {
                cfg.data_directory()
                    .is_some_and(|found| same_path(found.path(), &expected))
            })
        && is_managed_local_fork(&cfg, root)
}

/// GUI policy over the shared [`ChainId`]: launcher selection, presentation
/// and runtime support. An extension trait rather than a second enum so the
/// identity — and its wire encoding — stays the one type core defines; bring
/// it into scope (`use crate::chain::ChainIdExt`) where these are called.
pub trait ChainIdExt {
    /// Baseline Bitcoin-family launcher entries. Home adds BTCB2 only when
    /// the saved beta preference and global or authenticated availability allow it.
    const LAUNCHER: [ChainId; 5];

    /// Neutral, descriptive user-facing name (brand posture: no claim about
    /// which chain "is Bitcoin").
    fn label(self) -> &'static str;

    /// The unit ticker shown next to amounts.
    fn ticker(self) -> &'static str;

    /// Generic runtime capability. BTCB2 remains dormant here; only the
    /// separately admitted authenticated Connect Vault route may run it.
    fn runtime_support(self) -> RuntimeSupport;
}

impl ChainIdExt for ChainId {
    const LAUNCHER: [ChainId; 5] = [
        ChainId::Bitcoin,
        ChainId::Testnet,
        ChainId::Testnet4,
        ChainId::Signet,
        ChainId::Regtest,
    ];

    fn label(self) -> &'static str {
        match self {
            ChainId::Bitcoin => "Bitcoin",
            ChainId::Testnet => "Testnet",
            ChainId::Testnet4 => "Testnet4",
            ChainId::Signet => "Signet",
            ChainId::Regtest => "Regtest",
            ChainId::BitcoinBlake2b => "Bitcoin Blake2b",
            ChainId::BitcoinBlake2bTestnet4 => "Bitcoin Blake2b Testnet4",
        }
    }

    fn ticker(self) -> &'static str {
        match self {
            ChainId::Bitcoin
            | ChainId::Testnet
            | ChainId::Testnet4
            | ChainId::Signet
            | ChainId::Regtest => "BTC",
            ChainId::BitcoinBlake2b | ChainId::BitcoinBlake2bTestnet4 => "BTCB2",
        }
    }

    fn runtime_support(self) -> RuntimeSupport {
        if self.is_blake2b() {
            RuntimeSupport::Dormant {
                reason: BTCB2_DORMANT_REASON,
            }
        } else {
            RuntimeSupport::Supported
        }
    }
}

/// Stable technical identity for transaction, signing, and address review.
pub(crate) fn canonical_network_name(chain: ChainId) -> &'static str {
    match chain {
        ChainId::Bitcoin => CopyText::BitcoinSha256Mainnet.get(),
        ChainId::BitcoinBlake2b => CopyText::BitcoinBlake2bMainnet.get(),
        ChainId::Testnet => "Bitcoin SHA-256 · Testnet",
        ChainId::Testnet4 => "Bitcoin SHA-256 · Testnet4",
        ChainId::Signet => "Bitcoin SHA-256 · Signet",
        ChainId::Regtest => "Bitcoin SHA-256 · Regtest",
        ChainId::BitcoinBlake2bTestnet4 => "Bitcoin BLAKE2b · Testnet4",
    }
}

/// Optional flavor for passive chain labels. The result is display copy only.
pub(crate) fn worldview_chain_label(
    chain: ChainId,
    worldview: crate::app::settings::preferences::WorldviewMode,
) -> &'static str {
    use crate::app::settings::preferences::WorldviewMode::{Luke, Samurai, Suitcoiner};

    match (chain, worldview) {
        (ChainId::Bitcoin, Samurai | Suitcoiner) => "Bitcoin (BTC)",
        (ChainId::Bitcoin, Luke) => "Spamcoin (BTC)",
        (ChainId::BitcoinBlake2b, Samurai) => "Bitcoin BLAKE2b (BTCB2)",
        (ChainId::BitcoinBlake2b, Suitcoiner) => "LukeCoin (BTCB2)",
        (ChainId::BitcoinBlake2b, Luke) => "Bitcoin (XBT)",
        _ => chain.label(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::Network;

    #[test]
    fn worldview_aliases_and_canonical_identity_are_distinct() {
        use crate::app::settings::preferences::WorldviewMode::{Luke, Samurai, Suitcoiner};
        let chains = [
            (
                ChainId::Bitcoin,
                "Bitcoin SHA-256 · Mainnet",
                ["Bitcoin (BTC)", "Bitcoin (BTC)", "Spamcoin (BTC)"],
            ),
            (
                ChainId::BitcoinBlake2b,
                "Bitcoin BLAKE2b · Mainnet",
                [
                    "Bitcoin BLAKE2b (BTCB2)",
                    "LukeCoin (BTCB2)",
                    "Bitcoin (XBT)",
                ],
            ),
            (
                ChainId::Testnet,
                "Bitcoin SHA-256 · Testnet",
                ["Testnet"; 3],
            ),
            (
                ChainId::Testnet4,
                "Bitcoin SHA-256 · Testnet4",
                ["Testnet4"; 3],
            ),
            (ChainId::Signet, "Bitcoin SHA-256 · Signet", ["Signet"; 3]),
            (
                ChainId::Regtest,
                "Bitcoin SHA-256 · Regtest",
                ["Regtest"; 3],
            ),
            (
                ChainId::BitcoinBlake2bTestnet4,
                "Bitcoin BLAKE2b · Testnet4",
                ["Bitcoin Blake2b Testnet4"; 3],
            ),
        ];
        for (chain, canonical, aliases) in chains {
            assert_eq!(canonical_network_name(chain), canonical);
            for (worldview, alias) in [Samurai, Suitcoiner, Luke].iter().copied().zip(aliases) {
                assert_eq!(worldview_chain_label(chain, worldview), alias);
                assert_eq!(chain.dir_name().parse::<ChainId>(), Ok(chain));
                assert_eq!(ChainId::from_api_str(chain.api_str()), Some(chain));
            }
        }
        assert_eq!(
            ChainId::Bitcoin.bitcoin_network(),
            ChainId::BitcoinBlake2b.bitcoin_network()
        );
        assert_ne!(
            canonical_network_name(ChainId::Bitcoin),
            canonical_network_name(ChainId::BitcoinBlake2b)
        );
    }

    /// Compile-time proof that the GUI and core hand around one type: a
    /// value built through the core path is accepted where the GUI path is
    /// expected, and both name the same `TypeId`. If anyone reintroduces a
    /// GUI-local enum, this stops compiling.
    #[test]
    fn gui_and_core_share_one_chain_id_type() {
        fn same_type<T>(_: T, _: T) {}
        same_type(
            crate::chain::ChainId::BitcoinBlake2b,
            coincube_core::chain::ChainId::BitcoinBlake2b,
        );
        assert_eq!(
            std::any::TypeId::of::<crate::chain::ChainId>(),
            std::any::TypeId::of::<coincube_core::chain::ChainId>()
        );
        assert_eq!(
            std::any::TypeId::of::<crate::chain::UnknownChainId>(),
            std::any::TypeId::of::<coincube_core::chain::UnknownChainId>()
        );
        // The wire encoding the GUI writes is the one core defines; a
        // settings file written through either path reads back through the
        // other, including the fork spellings older builds refuse.
        for chain in ChainId::ALL {
            let json = serde_json::to_string(&chain).unwrap();
            assert_eq!(
                serde_json::from_str::<coincube_core::chain::ChainId>(&json).unwrap(),
                chain
            );
            assert_eq!(json, format!("\"{}\"", chain.dir_name()));
        }
    }

    #[test]
    fn launcher_offers_the_bitcoin_family_only_and_keeps_its_wire_form() {
        // What every existing settings.json already contains.
        for chain in ChainId::LAUNCHER {
            assert!(!chain.is_blake2b(), "{:?}", chain);
            assert!(chain.runtime_support().is_supported(), "{:?}", chain);
            let ours = serde_json::to_string(&chain).unwrap();
            let theirs = serde_json::to_string(&chain.bitcoin_network()).unwrap();
            assert_eq!(ours, theirs, "{:?}", chain);
            assert_eq!(chain.dir_name(), chain.bitcoin_network().to_string());
        }
        // LAUNCHER is exactly ALL minus the dormant fork identities, in order.
        let expected: Vec<ChainId> = ChainId::ALL
            .iter()
            .copied()
            .filter(|c| !c.is_blake2b())
            .collect();
        assert_eq!(ChainId::LAUNCHER.to_vec(), expected);
        assert!(ChainId::ALL
            .iter()
            .filter(|c| c.is_blake2b())
            .all(|c| !ChainId::LAUNCHER.contains(c)));
    }

    #[test]
    fn labels_and_tickers() {
        assert_eq!(ChainId::BitcoinBlake2b.label(), "Bitcoin Blake2b");
        assert_eq!(
            ChainId::BitcoinBlake2bTestnet4.label(),
            "Bitcoin Blake2b Testnet4"
        );
        assert_eq!(ChainId::Bitcoin.label(), "Bitcoin");
        assert_eq!(ChainId::BitcoinBlake2b.ticker(), "BTCB2");
        assert_eq!(ChainId::BitcoinBlake2bTestnet4.ticker(), "BTCB2");
        assert_eq!(ChainId::Bitcoin.ticker(), "BTC");
        assert!(ChainId::LAUNCHER.iter().all(|c| c.ticker() == "BTC"));
        // A label never repeats: the two fork identities must not be
        // mistaken for their encoding twins in any list.
        let labels: std::collections::HashSet<_> = ChainId::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(labels.len(), ChainId::ALL.len());
    }

    #[test]
    fn runtime_support_is_dormant_for_both_fork_variants_only() {
        for chain in ChainId::ALL {
            let support = chain.runtime_support();
            assert_eq!(support.is_supported(), !chain.is_blake2b(), "{:?}", chain);
            if let RuntimeSupport::Dormant { reason } = support {
                assert_eq!(reason, BTCB2_DORMANT_REASON);
                assert!(reason.contains("Bitcoin Blake2b"));
                assert!(!reason.contains("sender"));
            }
        }
        // A `Network` never reaches a dormant identity: the conversion the
        // Bitcoin-family flows rely on lands on a supported chain every time.
        for network in [
            Network::Bitcoin,
            Network::Testnet,
            Network::Testnet4,
            Network::Signet,
            Network::Regtest,
        ] {
            assert!(ChainId::from(network).runtime_support().is_supported());
        }
    }
}

#[cfg(test)]
mod global_beta_tests {
    #[tokio::test]
    async fn local_beta_off_refuses_admission_before_an_api_request() {
        let dir =
            std::env::temp_dir().join(format!("coincube-beta-admission-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = crate::dir::CoincubeDirectory::new(dir.clone());
        let mut client = crate::services::coincube::CoincubeClient::new();
        client.base_url = "http://127.0.0.1:1".into();
        client.set_token("synthetic-beta-token");
        let error = super::require_connect_feature(super::ChainId::BitcoinBlake2b, &client, &root)
            .await
            .unwrap_err();
        assert!(error.contains("Global Settings"));
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
pub(crate) fn managed_local_fixture(
    root: &crate::dir::CoincubeDirectory,
    chain: ChainId,
) -> (
    coincubed::config::Config,
    crate::app::settings::WalletSettings,
) {
    use coincube_core::miniscript::bitcoin;
    use std::str::FromStr;
    let mut wallet = crate::app::settings::WalletSettings {
        name: "Local".into(),
        alias: None,
        descriptor_checksum: "d72le4dr".into(),
        pinned_at: Some(1_720_000_000),
        keys: vec![],
        hardware_wallets: vec![],
        remote_backend_auth: None,
        start_internal_bitcoind: Some(true),
        pending_rescan: None,
        keychain_keys_recorded: false,
    };
    let directory = crate::node::bitcoind::internal_bitcoind_datadir_for(
        root,
        crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
    );
    std::fs::create_dir_all(&directory).unwrap();
    let mut managed = crate::node::bitcoind::InternalBitcoindConfig::for_flavor(
        crate::node::bitcoind::NodeFlavor::KnotsBlake2b,
    );
    managed.networks.insert(
        chain.bitcoin_network(),
        crate::node::bitcoind::InternalBitcoindNetworkConfig {
            rpc_port: 18444,
            p2p_port: 18445,
            prune: 550,
            rpc_auth: None,
        },
    );
    managed
        .to_ini()
        .write_to_file(crate::node::bitcoind::internal_bitcoind_config_path(
            &directory,
        ))
        .unwrap();
    let mainnet_descriptor = "tr([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,and_v(v:pk([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560)))#0mt7e93c";
    if chain.bitcoin_network() == bitcoin::Network::Bitcoin {
        wallet.descriptor_checksum = crate::app::wallet::Wallet::new(
            coincube_core::descriptors::CoincubeDescriptor::from_str(mainnet_descriptor).unwrap(),
        )
        .descriptor_checksum;
    }
    let data = root
        .network_directory(chain)
        .coincubed_data_directory(&wallet.wallet_id());
    std::fs::create_dir_all(data.path()).unwrap();
    let cfg = coincubed::config::Config::new(
        coincubed::config::BitcoinConfig::new(chain, std::time::Duration::from_secs(30)),
        Some(coincubed::config::BitcoinBackend::Bitcoind(coincubed::config::BitcoindConfig {
            addr: "127.0.0.1:18444".parse().unwrap(), rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                crate::node::bitcoind::internal_bitcoind_cookie_path(&directory, &chain.bitcoin_network())),
        })), log::LevelFilter::Off,
        coincube_core::descriptors::CoincubeDescriptor::from_str(if chain.bitcoin_network() == bitcoin::Network::Bitcoin { mainnet_descriptor } else { "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr" }).unwrap(),
        coincubed::datadir::DataDirectory::new(data.path().to_path_buf()));
    std::fs::write(
        data.path().join("daemon.toml"),
        toml::to_string(&cfg).unwrap(),
    )
    .unwrap();
    (cfg, wallet)
}

#[cfg(test)]
mod local_capability_tests {
    use super::*;
    use crate::{dir::CoincubeDirectory, services::coincube::CoincubeClient};
    use httpmock::prelude::*;

    #[tokio::test]
    async fn global_capability_is_anonymous_and_does_not_borrow_account_overrides() {
        let server = MockServer::start_async().await;
        let anonymous = server.mock_async(|when, then| {
            when.method(GET).path("/api/v1/connect/features").matches(|request| request.headers.as_ref().is_none_or(|headers| headers.iter().all(|(name,_)| !name.eq_ignore_ascii_case("authorization"))));
            then.status(200).json_body(serde_json::json!({"data":{"plans":[],"bitcoin_blake2b_enabled":true,"liquidEnabled":true}}));
        }).await;
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-account-token");
        assert!(global_blake2b_enabled(client.clone()).await);
        assert_eq!(client.token(), Some("synthetic-account-token"));
        anonymous.assert_async().await;
        anonymous.delete_async().await;
        for body in [
            serde_json::json!({"plans":[],"bitcoin_blake2b_enabled":false}),
            serde_json::json!({"plans":[]}),
        ] {
            let denied = server
                .mock_async(|when, then| {
                    when.method(GET);
                    then.status(200).json_body(serde_json::json!({"data":body}));
                })
                .await;
            assert!(!global_blake2b_enabled(client.clone()).await);
            denied.delete_async().await;
        }
        assert!(!global_blake2b_enabled(CoincubeClient::for_test("http://127.0.0.1:1")).await);
    }

    #[test]
    fn local_capability_refuses_foreign_cookie_port_flavor_and_pending_backend() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let path =
                std::env::temp_dir().join(format!("local-capability-{}", uuid::Uuid::new_v4()));
            let root = CoincubeDirectory::new(path.clone());
            let (cfg, wallet) = managed_local_fixture(&root, chain);
            assert!(is_managed_local_fork(&cfg, &root));
            assert!(has_managed_local_fork(&root, chain, Some(&wallet)));
            let mut wrong = cfg.clone();
            let Some(coincubed::config::BitcoinBackend::Bitcoind(node)) =
                &mut wrong.bitcoin_backend
            else {
                unreachable!()
            };
            node.addr.set_port(18446);
            assert!(!is_managed_local_fork(&wrong, &root));
            let Some(coincubed::config::BitcoinBackend::Bitcoind(node)) =
                &mut wrong.bitcoin_backend
            else {
                unreachable!()
            };
            node.addr = "192.0.2.1:18444".parse().unwrap();
            assert!(!is_managed_local_fork(&wrong, &root));
            wrong = cfg.clone();
            let Some(coincubed::config::BitcoinBackend::Bitcoind(node)) =
                &mut wrong.bitcoin_backend
            else {
                unreachable!()
            };
            node.rpc_auth =
                coincubed::config::BitcoindRpcAuth::CookieFile(path.join("bitcoin/.cookie"));
            assert!(!is_managed_local_fork(&wrong, &root));
            wrong = cfg.clone();
            let Some(coincubed::config::BitcoinBackend::Bitcoind(node)) = &wrong.bitcoin_backend
            else {
                unreachable!()
            };
            wrong.pending_bitcoind = Some(node.clone());
            assert!(!is_managed_local_fork(&wrong, &root));
            crate::node::revalidate::ManagedNodeState {
                configured_flavor: Some(crate::node::bitcoind::NodeFlavor::Core),
                ..Default::default()
            }
            .save_for(
                &root,
                crate::node::bitcoind::NodeChainFamily::BitcoinBlake2b,
            )
            .unwrap();
            assert!(!is_managed_local_fork(&cfg, &root));
            std::fs::remove_dir_all(path).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_cube_reopens_through_a_datadir_alias() {
        let path = std::env::temp_dir().join(format!("local-alias-{}", uuid::Uuid::new_v4()));
        let root = CoincubeDirectory::new(path.join("real"));
        let (cfg, wallet) = managed_local_fixture(&root, ChainId::BitcoinBlake2bTestnet4);
        std::os::unix::fs::symlink(root.path(), path.join("alias")).unwrap();
        let alias = CoincubeDirectory::new(path.join("alias"));
        assert!(is_managed_local_fork(&cfg, &alias));
        assert!(has_managed_local_fork(
            &alias,
            ChainId::BitcoinBlake2bTestnet4,
            Some(&wallet)
        ));
        std::fs::remove_dir_all(path).unwrap();
    }
}
