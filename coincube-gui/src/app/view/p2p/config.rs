use std::path::PathBuf;

use coincube_core::miniscript::bitcoin;
use serde::{Deserialize, Serialize};

/// Compile-time fallbacks — overridden at runtime by env vars.
const FALLBACK_NODES: &[(&str, &str)] = &[(
    "Mostro (Official)",
    "82fa8cb978b43c79b2156585bac2c011176a21d2aead6d9f7c575c005be88390",
)];

const FALLBACK_RELAYS: &[&str] = &["wss://relay.mostro.network"];

/// Current on-disk schema version. Bumped when the persisted shape changes
/// so future migrations have a clean signal. Version 0 (the implicit value
/// for `config.json` written before network tagging) is treated as legacy
/// on load — every node without a `network` field is assumed `Mainnet`.
const CONFIG_VERSION: u32 = 1;

/// Which Bitcoin network a Mostro coordinator (and its relays) serve.
///
/// Coordinators escrow real funds, so a mainnet coordinator must never be
/// silently used on a test network (or vice-versa). Collapsing the five
/// Bitcoin networks to two kinds is enough for coordinator selection — a
/// coordinator is either for real money or for testing.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum NetworkKind {
    Mainnet,
    Test,
}

impl Default for NetworkKind {
    /// Legacy configs (and the official fallback) are mainnet.
    fn default() -> Self {
        NetworkKind::Mainnet
    }
}

impl NetworkKind {
    pub fn of(net: bitcoin::Network) -> Self {
        match net {
            bitcoin::Network::Bitcoin => NetworkKind::Mainnet,
            _ => NetworkKind::Test,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NetworkKind::Mainnet => "Mainnet",
            NetworkKind::Test => "Test",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct MostroNode {
    pub name: String,
    pub pubkey_hex: String,
    /// Which network this coordinator serves. Absent in legacy configs, in
    /// which case it defaults to `Mainnet` (the official coordinator).
    #[serde(default)]
    pub network: NetworkKind,
}

/// A coordinator + relay set resolved for a specific Bitcoin network.
/// Built by [`MostroConfig::active_for`] so callers never have to repeat
/// the "which node/relays apply on this network" logic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCoordinator {
    pub pubkey_hex: String,
    pub relays: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MostroConfig {
    /// On-disk schema version; see [`CONFIG_VERSION`]. Missing on legacy
    /// files (`#[serde(default)]` → 0).
    #[serde(default)]
    pub version: u32,
    pub nodes: Vec<MostroNode>,
    pub active_node_pubkey: String,
    pub relays: Vec<String>,
    /// Relays used when the wallet is on a test network. Falls back to
    /// `relays` when empty. Kept separate so a test relay set can be seeded
    /// (via `MOSTRO_TESTNET_RELAYS`) without disturbing the mainnet relays.
    #[serde(default)]
    pub test_relays: Vec<String>,
    #[serde(default)]
    pub blossom_url: Option<String>,
}

/// Parse `name=pubkey` comma-separated pairs into nodes tagged `kind`.
fn parse_nodes(val: &str, kind: NetworkKind) -> Vec<MostroNode> {
    val.split(',')
        .filter_map(|entry| {
            let (name, pubkey) = entry.split_once('=')?;
            let name = name.trim();
            let pubkey = pubkey.trim();
            if name.is_empty() || pubkey.is_empty() {
                return None;
            }
            Some(MostroNode {
                name: name.to_string(),
                pubkey_hex: pubkey.to_string(),
                network: kind,
            })
        })
        .collect()
}

/// Build default mainnet nodes from `MOSTRO_NODES` env var or compile-time
/// fallbacks.
///
/// Env format: comma-separated `name=pubkey` pairs, e.g.
/// `MOSTRO_NODES="Mostro (Official)=82fa8c...,TestNode=aabbcc..."`
fn default_nodes() -> Vec<MostroNode> {
    if let Ok(val) = std::env::var("MOSTRO_NODES") {
        let nodes = parse_nodes(&val, NetworkKind::Mainnet);
        if !nodes.is_empty() {
            return nodes;
        }
    }
    FALLBACK_NODES
        .iter()
        .map(|(name, pubkey)| MostroNode {
            name: name.to_string(),
            pubkey_hex: pubkey.to_string(),
            network: NetworkKind::Mainnet,
        })
        .collect()
}

/// Build test-network coordinator nodes from `MOSTRO_TESTNET_NODES`.
///
/// Same `name=pubkey` format as [`default_nodes`], but every node is tagged
/// `NetworkKind::Test`. There is no compile-time fallback — a test
/// coordinator must be explicitly provided (this is what gates P2P on test
/// networks, see [`MostroConfig::has_test_coordinator`]).
fn default_test_nodes() -> Vec<MostroNode> {
    match std::env::var("MOSTRO_TESTNET_NODES") {
        Ok(val) => parse_nodes(&val, NetworkKind::Test),
        Err(_) => Vec::new(),
    }
}

/// Parse comma-separated relay URLs.
fn parse_relays(val: &str) -> Vec<String> {
    val.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Build default mainnet relays from `MOSTRO_RELAYS` env var or fallbacks.
fn default_relays() -> Vec<String> {
    if let Ok(val) = std::env::var("MOSTRO_RELAYS") {
        let relays = parse_relays(&val);
        if !relays.is_empty() {
            return relays;
        }
    }
    FALLBACK_RELAYS.iter().map(|s| s.to_string()).collect()
}

/// Build test-network relays from `MOSTRO_TESTNET_RELAYS` (no fallback).
fn default_test_relays() -> Vec<String> {
    match std::env::var("MOSTRO_TESTNET_RELAYS") {
        Ok(val) => parse_relays(&val),
        Err(_) => Vec::new(),
    }
}

impl Default for MostroConfig {
    fn default() -> Self {
        let mut nodes = default_nodes();
        nodes.extend(default_test_nodes());
        let active = nodes
            .iter()
            .find(|n| n.network == NetworkKind::Mainnet)
            .or_else(|| nodes.first())
            .map(|n| n.pubkey_hex.clone())
            .unwrap_or_default();
        Self {
            version: CONFIG_VERSION,
            nodes,
            active_node_pubkey: active,
            relays: default_relays(),
            test_relays: default_test_relays(),
            blossom_url: None,
        }
    }
}

impl MostroConfig {
    pub fn active_node(&self) -> &MostroNode {
        static FALLBACK: std::sync::LazyLock<MostroNode> =
            std::sync::LazyLock::new(|| MostroNode {
                name: "Mostro (Default)".to_string(),
                pubkey_hex: "82fa8cb978b43c79b2156585bac2c011176a21d2aead6d9f7c575c005be88390"
                    .to_string(),
                network: NetworkKind::Mainnet,
            });
        self.nodes
            .iter()
            .find(|n| n.pubkey_hex == self.active_node_pubkey)
            .or_else(|| self.nodes.first())
            .unwrap_or(&FALLBACK)
    }

    pub fn active_pubkey_hex(&self) -> &str {
        &self.active_node().pubkey_hex
    }

    pub fn blossom_url(&self) -> &str {
        self.blossom_url
            .as_deref()
            .unwrap_or("https://blossom.primal.net")
    }

    /// Relay set for `kind` — the dedicated test relays when on a test
    /// network and any are configured, otherwise the shared relay set.
    fn relays_for(&self, kind: NetworkKind) -> Vec<String> {
        match kind {
            NetworkKind::Mainnet => self.relays.clone(),
            NetworkKind::Test if !self.test_relays.is_empty() => self.test_relays.clone(),
            NetworkKind::Test => self.relays.clone(),
        }
    }

    /// Resolve the coordinator + relays to use on `net`.
    ///
    /// Prefers the active node when it matches the network kind; otherwise
    /// falls back to the first node of that kind. Returns `None` when no
    /// coordinator is configured for the network — e.g. a test network with
    /// no test coordinator, which is exactly the case the P2P rail gate
    /// disables.
    pub fn active_for(&self, net: bitcoin::Network) -> Option<ResolvedCoordinator> {
        let kind = NetworkKind::of(net);
        let node = self
            .nodes
            .iter()
            .find(|n| n.pubkey_hex == self.active_node_pubkey && n.network == kind)
            .or_else(|| self.nodes.iter().find(|n| n.network == kind))?;
        Some(ResolvedCoordinator {
            pubkey_hex: node.pubkey_hex.clone(),
            relays: self.relays_for(kind),
        })
    }

    /// Drives the P2P network gate: is P2P trading actually usable on `net`?
    ///
    /// True only when (a) `net` is a test network, (b) a coordinator tagged
    /// for test networks is configured, and (c) the network has a usable
    /// Lightning rail for escrow. Mostro escrow is paid with HODL invoices
    /// over Spark, which only runs on mainnet + Regtest, so among test
    /// networks only Regtest can actually trade (COIN-370 §6 / Q1). Spark
    /// availability is taken from [`crate::app::features::spark`] so the
    /// rail dependency stays a single source of truth.
    pub fn has_test_coordinator(&self, net: bitcoin::Network) -> bool {
        NetworkKind::of(net) == NetworkKind::Test
            && self.nodes.iter().any(|n| n.network == NetworkKind::Test)
            && crate::app::features::spark(net).is_available()
    }

    /// Ensure the active selection suits `net`: if the active node isn't
    /// tagged for this network kind, switch to the first node that is.
    /// Never leaves a mainnet coordinator selected on a test network (or
    /// vice-versa), per COIN-371 §3.4. No-op when a matching node is already
    /// active, or when none exists for the network.
    pub fn select_default_for(&mut self, net: bitcoin::Network) {
        let kind = NetworkKind::of(net);
        let active_matches = self
            .nodes
            .iter()
            .any(|n| n.pubkey_hex == self.active_node_pubkey && n.network == kind);
        if active_matches {
            return;
        }
        if let Some(node) = self.nodes.iter().find(|n| n.network == kind) {
            self.active_node_pubkey = node.pubkey_hex.clone();
        }
    }

    /// Ensure there is always at least one node and one relay, and that the
    /// active selection points at an existing node.
    pub fn ensure_defaults(&mut self) {
        if self.nodes.is_empty() {
            self.nodes = default_nodes();
            self.nodes.extend(default_test_nodes());
        }
        if self.relays.is_empty() {
            self.relays = default_relays();
        }
        // If active node was removed, switch to first available
        if !self
            .nodes
            .iter()
            .any(|n| n.pubkey_hex == self.active_node_pubkey)
        {
            self.active_node_pubkey = self.nodes[0].pubkey_hex.clone();
        }
    }
}

fn config_file_path() -> Result<PathBuf, String> {
    Ok(super::mostro_dir()?.join("config.json"))
}

pub fn load_mostro_config() -> Result<MostroConfig, String> {
    let path = config_file_path()?;
    if !path.exists() {
        return Ok(MostroConfig::default());
    }
    let data = std::fs::read(&path)
        .map_err(|e| format!("Failed to read mostro config at {}: {e}", path.display()))?;
    let mut config: MostroConfig = serde_json::from_slice(&data)
        .map_err(|e| format!("Failed to parse mostro config at {}: {e}", path.display()))?;
    // Refuse a config written by a newer app version. Stamping it down to
    // CONFIG_VERSION (and a later write-back) would silently drop fields this
    // build doesn't understand, so fail loudly instead of downgrading.
    if config.version > CONFIG_VERSION {
        return Err(format!(
            "Mostro config at {} is version {}, newer than this app supports ({}). \
             Update the app to load it.",
            path.display(),
            config.version,
            CONFIG_VERSION
        ));
    }
    // Legacy files (version 0, nodes without a `network` tag) deserialize
    // with every node defaulting to Mainnet — the correct migration, since
    // the only pre-tagging coordinator was the mainnet official one. Stamp
    // the current version so the next write-back is up to date.
    config.version = CONFIG_VERSION;
    config.ensure_defaults();
    Ok(config)
}

pub fn save_mostro_config(config: &MostroConfig) -> Result<(), String> {
    let path = config_file_path()?;
    let dir = path
        .parent()
        .ok_or_else(|| "Config file has no parent directory".to_string())?;
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|e| format!("Failed to serialize mostro config: {e}"))?;
    let tmp_path = dir.join(".mostro_config.tmp");
    let mut tmp_file = std::fs::File::create(&tmp_path)
        .map_err(|e| format!("Failed to create temp config file: {e}"))?;
    use std::io::Write;
    tmp_file
        .write_all(&bytes)
        .map_err(|e| format!("Failed to write temp config file: {e}"))?;
    tmp_file
        .sync_all()
        .map_err(|e| format!("Failed to sync temp config file: {e}"))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("Failed to rename temp config file: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, pubkey: &str, network: NetworkKind) -> MostroNode {
        MostroNode {
            name: name.to_string(),
            pubkey_hex: pubkey.to_string(),
            network,
        }
    }

    /// A legacy `config.json` (no `version`, nodes without `network`) loads
    /// with every node tagged Mainnet.
    #[test]
    fn legacy_config_loads_as_mainnet() {
        let legacy = r#"{
            "nodes": [{"name": "Mostro (Official)", "pubkey_hex": "abc123"}],
            "active_node_pubkey": "abc123",
            "relays": ["wss://relay.mostro.network"]
        }"#;
        let config: MostroConfig = serde_json::from_str(legacy).expect("parse legacy");
        assert_eq!(config.version, 0, "legacy version defaults to 0");
        assert_eq!(config.nodes.len(), 1);
        assert_eq!(config.nodes[0].network, NetworkKind::Mainnet);
        assert!(config.test_relays.is_empty());
    }

    #[test]
    fn has_test_coordinator_requires_test_node_and_lightning_rail() {
        let mut config = MostroConfig {
            version: CONFIG_VERSION,
            nodes: vec![node("Official", "main1", NetworkKind::Mainnet)],
            active_node_pubkey: "main1".to_string(),
            relays: vec!["wss://relay.mostro.network".to_string()],
            test_relays: vec![],
            blossom_url: None,
        };

        // No test node → never a test coordinator, on any network.
        assert!(!config.has_test_coordinator(bitcoin::Network::Regtest));
        assert!(!config.has_test_coordinator(bitcoin::Network::Testnet));

        config.nodes.push(node("Local", "test1", NetworkKind::Test));

        // Mainnet is never a "test coordinator" network.
        assert!(!config.has_test_coordinator(bitcoin::Network::Bitcoin));
        // Regtest has a Lightning rail (Spark) → enabled with a test node.
        assert!(config.has_test_coordinator(bitcoin::Network::Regtest));
        // Testnet/Signet/Testnet4 have no escrow rail → still gated even
        // with a test coordinator configured.
        assert!(!config.has_test_coordinator(bitcoin::Network::Testnet));
        assert!(!config.has_test_coordinator(bitcoin::Network::Signet));
        assert!(!config.has_test_coordinator(bitcoin::Network::Testnet4));
    }

    #[test]
    fn active_for_resolves_per_network_kind() {
        let config = MostroConfig {
            version: CONFIG_VERSION,
            nodes: vec![
                node("Official", "main1", NetworkKind::Mainnet),
                node("Local", "test1", NetworkKind::Test),
            ],
            active_node_pubkey: "main1".to_string(),
            relays: vec!["wss://main.relay".to_string()],
            test_relays: vec!["wss://test.relay".to_string()],
            blossom_url: None,
        };

        // On mainnet: the active mainnet node + mainnet relays.
        let m = config
            .active_for(bitcoin::Network::Bitcoin)
            .expect("mainnet");
        assert_eq!(m.pubkey_hex, "main1");
        assert_eq!(m.relays, vec!["wss://main.relay".to_string()]);

        // On a test network: falls through to the test node + test relays,
        // never the mainnet coordinator.
        let t = config.active_for(bitcoin::Network::Regtest).expect("test");
        assert_eq!(t.pubkey_hex, "test1");
        assert_eq!(t.relays, vec!["wss://test.relay".to_string()]);
    }

    #[test]
    fn active_for_test_network_without_test_node_is_none() {
        let config = MostroConfig {
            version: CONFIG_VERSION,
            nodes: vec![node("Official", "main1", NetworkKind::Mainnet)],
            active_node_pubkey: "main1".to_string(),
            relays: vec!["wss://main.relay".to_string()],
            test_relays: vec![],
            blossom_url: None,
        };
        assert!(config.active_for(bitcoin::Network::Regtest).is_none());
    }

    #[test]
    fn select_default_for_avoids_cross_network_coordinator() {
        let mut config = MostroConfig {
            version: CONFIG_VERSION,
            nodes: vec![
                node("Official", "main1", NetworkKind::Mainnet),
                node("Local", "test1", NetworkKind::Test),
            ],
            // Persisted selection is the mainnet coordinator.
            active_node_pubkey: "main1".to_string(),
            relays: vec!["wss://main.relay".to_string()],
            test_relays: vec![],
            blossom_url: None,
        };

        // On a test network it switches off the mainnet coordinator…
        config.select_default_for(bitcoin::Network::Regtest);
        assert_eq!(config.active_node_pubkey, "test1");

        // …and back on mainnet it switches to a mainnet coordinator.
        config.select_default_for(bitcoin::Network::Bitcoin);
        assert_eq!(config.active_node_pubkey, "main1");

        // No matching node for the network → selection left untouched.
        config.nodes.retain(|n| n.network == NetworkKind::Mainnet);
        config.active_node_pubkey = "main1".to_string();
        config.select_default_for(bitcoin::Network::Regtest);
        assert_eq!(config.active_node_pubkey, "main1");
    }

    #[test]
    fn test_relays_fall_back_to_shared_when_empty() {
        let config = MostroConfig {
            version: CONFIG_VERSION,
            nodes: vec![node("Local", "test1", NetworkKind::Test)],
            active_node_pubkey: "test1".to_string(),
            relays: vec!["wss://shared.relay".to_string()],
            test_relays: vec![],
            blossom_url: None,
        };
        let t = config.active_for(bitcoin::Network::Regtest).expect("test");
        assert_eq!(t.relays, vec!["wss://shared.relay".to_string()]);
    }
}
