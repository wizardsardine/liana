use breez_sdk_liquid::prelude as breez;
use coincube_core::miniscript::bitcoin;
use std::path::PathBuf;

use super::BreezError;

#[derive(Debug, Clone)]
pub struct BreezConfig {
    pub api_key: &'static str,
    pub network: bitcoin::Network,
    pub working_dir: PathBuf,
}

impl BreezConfig {
    pub fn from_env(
        network: bitcoin::Network,
        datadir: &std::path::Path,
    ) -> Result<Self, BreezError> {
        Ok(Self {
            api_key: env!("BREEZ_API_KEY"),
            network,
            working_dir: datadir.join("breez"),
        })
    }

    pub fn sdk_config(&self) -> breez::Config {
        // Base URL for Coincube-hosted Esplora; resolved via the shared helper so
        // runtime `.env` overrides apply consistently with the REST/SSE clients.
        let coincube_base = crate::services::coincube_api_base_url();

        let liquid_explorer_url = match self.network {
            bitcoin::Network::Bitcoin => format!("{}/api/v1/esplora/liquid/mainnet", coincube_base),
            bitcoin::Network::Testnet => {
                format!("{}/api/v1/esplora/liquid/testnet", coincube_base)
            }
            bitcoin::Network::Signet => "https://blockstream.info/liquidtestnet/api".to_string(),
            bitcoin::Network::Regtest | bitcoin::Network::Testnet4 => {
                "http://localhost:4003/api".to_string()
            }
        };
        let bitcoin_explorer_url = match self.network {
            bitcoin::Network::Bitcoin => {
                format!("{}/api/v1/esplora/bitcoin/mainnet", coincube_base)
            }
            bitcoin::Network::Testnet => {
                format!("{}/api/v1/esplora/bitcoin/testnet", coincube_base)
            }
            bitcoin::Network::Signet => "https://blockstream.info/signet/api".to_string(),
            bitcoin::Network::Regtest | bitcoin::Network::Testnet4 => {
                "http://localhost:4002/api".to_string()
            }
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
                bitcoin::Network::Regtest => breez::LiquidNetwork::Regtest,
            },
            payment_timeout_sec: 60,
            sync_service_url: None,         // Use default
            zero_conf_max_amount_sat: None, // Use default
            breez_api_key: Some(self.api_key.to_string()),
            external_input_parsers: None,
            use_default_external_input_parsers: true,
            onchain_fee_rate_leeway_sat: None, // Use default
            asset_metadata: None,              // USDt is already a built-in default in the SDK DB
            sideswap_api_key: None,
            use_magic_routing_hints: true,
            // 10 minutes baseline — real-time payment events (PaymentPending,
            // PaymentSucceeded, etc.) are delivered instantly via websocket
            // regardless of this setting. This only controls how often the SDK
            // reconciles on-chain state with Esplora.
            onchain_sync_period_sec: 600,
            onchain_sync_request_timeout_sec: 10,
        }
    }
}
