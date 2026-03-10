use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Compile-time fallbacks — overridden at runtime by env vars.
const FALLBACK_NODES: &[(&str, &str)] = &[(
    "Mostro (Official)",
    "82fa8cb978b43c79b2156585bac2c011176a21d2aead6d9f7c575c005be88390",
)];

const FALLBACK_RELAYS: &[&str] = &["wss://relay.mostro.network"];

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct MostroNode {
    pub name: String,
    pub pubkey_hex: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MostroConfig {
    pub nodes: Vec<MostroNode>,
    pub active_node_pubkey: String,
    pub relays: Vec<String>,
}

/// Build default nodes from `MOSTRO_NODES` env var or compile-time fallbacks.
///
/// Env format: comma-separated `name=pubkey` pairs, e.g.
/// `MOSTRO_NODES="Mostro (Official)=82fa8c...,TestNode=aabbcc..."`
fn default_nodes() -> Vec<MostroNode> {
    if let Ok(val) = std::env::var("MOSTRO_NODES") {
        let nodes: Vec<MostroNode> = val
            .split(',')
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
                })
            })
            .collect();
        if !nodes.is_empty() {
            return nodes;
        }
    }
    FALLBACK_NODES
        .iter()
        .map(|(name, pubkey)| MostroNode {
            name: name.to_string(),
            pubkey_hex: pubkey.to_string(),
        })
        .collect()
}

/// Build default relays from `MOSTRO_RELAYS` env var or compile-time fallbacks.
///
/// Env format: comma-separated URLs, e.g.
/// `MOSTRO_RELAYS="wss://relay.mostro.network,wss://relay2.example.com"`
fn default_relays() -> Vec<String> {
    if let Ok(val) = std::env::var("MOSTRO_RELAYS") {
        let relays: Vec<String> = val
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !relays.is_empty() {
            return relays;
        }
    }
    FALLBACK_RELAYS.iter().map(|s| s.to_string()).collect()
}

impl Default for MostroConfig {
    fn default() -> Self {
        let nodes = default_nodes();
        let active = nodes[0].pubkey_hex.clone();
        Self {
            nodes,
            active_node_pubkey: active,
            relays: default_relays(),
        }
    }
}

impl MostroConfig {
    pub fn active_node(&self) -> &MostroNode {
        self.nodes
            .iter()
            .find(|n| n.pubkey_hex == self.active_node_pubkey)
            .or_else(|| self.nodes.first())
            .expect("MostroConfig must always have at least one node")
    }

    pub fn active_pubkey_hex(&self) -> &str {
        &self.active_node().pubkey_hex
    }

    /// Ensure there is always at least one node and one relay.
    pub fn ensure_defaults(&mut self) {
        if self.nodes.is_empty() {
            self.nodes = default_nodes();
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
    let data_dir = dirs::data_dir().ok_or_else(|| "Cannot determine data directory".to_string())?;
    let mostro_dir = data_dir.join("coincube").join("mostro");
    std::fs::create_dir_all(&mostro_dir)
        .map_err(|e| format!("Failed to create mostro data dir: {e}"))?;
    Ok(mostro_dir.join("config.json"))
}

pub fn load_mostro_config() -> MostroConfig {
    let path = match config_file_path() {
        Ok(p) => p,
        Err(_) => return MostroConfig::default(),
    };
    if !path.exists() {
        return MostroConfig::default();
    }
    let data = match std::fs::read(&path) {
        Ok(d) => d,
        Err(_) => return MostroConfig::default(),
    };
    let mut config: MostroConfig = serde_json::from_slice(&data).unwrap_or_default();
    config.ensure_defaults();
    config
}

pub fn save_mostro_config(config: &MostroConfig) -> Result<(), String> {
    let path = config_file_path()?;
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|e| format!("Failed to serialize mostro config: {e}"))?;
    std::fs::write(&path, bytes).map_err(|e| format!("Failed to write mostro config: {e}"))?;
    Ok(())
}
