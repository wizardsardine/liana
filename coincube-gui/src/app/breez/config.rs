use breez_sdk_liquid::prelude as breez;
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
        let api_key = std::env::var("BREEZ_API_KEY").map_err(|_| BreezError::MissingApiKey)?;
        Ok(Self {
            api_key,
            network,
            working_dir: datadir.join("breez"),
        })
    }

    pub fn sdk_config(&self) -> breez::Config {
        let (liquid_explorer_url, bitcoin_explorer_url) = match self.network {
            bitcoin::Network::Bitcoin => (
                "https://blockstream.info/liquid/api",
                "https://blockstream.info/api",
            ),
            bitcoin::Network::Testnet | bitcoin::Network::Testnet4 | bitcoin::Network::Regtest => (
                "https://blockstream.info/liquidtestnet/api",
                "https://blockstream.info/testnet/api",
            ),
            bitcoin::Network::Signet => (
                "https://blockstream.info/liquidtestnet/api",
                "https://blockstream.info/signet/api",
            ),
        };

        breez::Config {
            liquid_explorer: breez::BlockchainExplorer::Esplora {
                url: liquid_explorer_url.to_string(),
                use_waterfalls: false,
            },
            bitcoin_explorer: breez::BlockchainExplorer::Esplora {
                url: bitcoin_explorer_url.to_string(),
                use_waterfalls: false,
            },
            working_dir: self.working_dir.to_string_lossy().to_string(),
            network: match self.network {
                bitcoin::Network::Bitcoin => breez::LiquidNetwork::Mainnet,
                bitcoin::Network::Testnet | bitcoin::Network::Signet => {
                    breez::LiquidNetwork::Testnet
                }
                bitcoin::Network::Testnet4 => breez::LiquidNetwork::Testnet,
                bitcoin::Network::Regtest => breez::LiquidNetwork::Testnet,
            },
            payment_timeout_sec: 60,
            sync_service_url: None,         // Use default
            zero_conf_max_amount_sat: None, // Use default
            breez_api_key: Some(self.api_key.clone()),
            external_input_parsers: None,
            use_default_external_input_parsers: true,
            onchain_fee_rate_leeway_sat: None, // Use default
            asset_metadata: None,
            sideswap_api_key: None,
            use_magic_routing_hints: true,
            onchain_sync_period_sec: 10,
            onchain_sync_request_timeout_sec: 7,
        }
    }
}
