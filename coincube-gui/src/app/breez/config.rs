// breez/config.rs
// ============================================
// STUB IMPLEMENTATION - Real Breez SDK code is commented out below
// The real Breez SDK has compilation issues due to rand_core version conflicts.
//
// TO ENABLE REAL BREEZ SDK:
// 1. Uncomment breez-sdk-liquid in Cargo.toml
// 2. Comment out the STUB section below
// 3. Uncomment the REAL IMPLEMENTATION section
// ============================================

// ========== STUB IMPLEMENTATION (ACTIVE) ==========
use coincube_core::miniscript::bitcoin;
use std::path::PathBuf;

use super::BreezError;

#[derive(Debug, Clone)]
pub struct BreezConfig {
    pub api_key: String,
    pub network: bitcoin::Network,
    pub working_dir: PathBuf,
}

impl BreezConfig {
    pub fn from_env(network: bitcoin::Network, datadir: &PathBuf) -> Result<Self, BreezError> {
        // In stub mode, we don't require the API key
        let api_key = std::env::var("BREEZ_API_KEY").unwrap_or_else(|_| "stub".to_string());
        Ok(Self {
            api_key,
            network,
            working_dir: datadir.join("breez"),
        })
    }
}

// ========== REAL IMPLEMENTATION (COMMENTED OUT) ==========
/*
use breez_sdk_liquid::prelude as breez;

impl BreezConfig {
    pub fn sdk_config(&self) -> breez::Config<'_> {
        breez::Config {
            breez_api_key: &self.api_key,
            network: match self.network {
                bitcoin::Network::Bitcoin => breez::NodeConfig::BitcoinMainnet,
                bitcoin::Network::Testnet | bitcoin::Network::Signet => {
                    breez::NodeConfig::BitcoinSignet
                }
                bitcoin::Network::Regtest => breez::NodeConfig::BitcoinRegtest,
            },
            working_dir: self.working_dir.clone(),
            ..Default::default()
        }
    }
}
*/
