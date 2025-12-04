// breez/config.rs
use breez_sdk_liquid::prelude as breez;
use liana::miniscript::bitcoin;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct BreezConfig {
    pub api_key: String,
    pub network: bitcoin::Network,
    pub working_dir: PathBuf,
}

impl BreezConfig {
    pub fn from_env(network: bitcoin::Network, datadir: &PathBuf) -> Result<Self, BreezError> {
        let api_key = std::env::var("BREEZ_API_KEY").map_err(|_| BreezError::MissingApiKey)?;
        Ok(Self {
            api_key,
            network,
            working_dir: datadir.join("breez"),
        })
    }

    pub fn sdk_config(&self) -> breez::Config<'_> {
        breez::Config {
            breez_api_key: &self.api_key,
            network: match self.network {
                bitcoin::Network::Bitcoin => breez::NodeConfig::BitcoinMainnet,
                bitcoin::Network::Testnet | bitcoin::Network::Signet => breez::NodeConfig::BitcoinSignet,
                bitcoin::Network::Regtest => breez::NodeConfig::BitcoinRegtest,
            },
            working_dir: self.working_dir.clone(),
            ..Default::default()
        }
    }
}