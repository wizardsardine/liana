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
        // Defense in depth: only build a config for networks with a real
        // Liquid Esplora backend (mainnet, testnet, signet). The loader
        // guards this too, but rejecting here guarantees `sdk_config`
        // never has to invent a localhost fallback URL.
        if !crate::app::features::liquid(network).is_available() {
            return Err(BreezError::NetworkNotSupported(network));
        }
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

        // `from_env` rejects every network without a real backend (see
        // `features::liquid` — mainnet only), so only Bitcoin reaches here.
        // The remaining arms exist solely to keep the match exhaustive: they
        // route to the Coincube-hosted testnet Esplora rather than a localhost
        // fallback, and are never hit in practice.
        let liquid_explorer_url = match self.network {
            bitcoin::Network::Bitcoin => format!("{}/api/v1/esplora/liquid/mainnet", coincube_base),
            bitcoin::Network::Signet => "https://blockstream.info/liquidtestnet/api".to_string(),
            bitcoin::Network::Testnet | bitcoin::Network::Testnet4 | bitcoin::Network::Regtest => {
                format!("{}/api/v1/esplora/liquid/testnet", coincube_base)
            }
        };
        let bitcoin_explorer_url = match self.network {
            bitcoin::Network::Bitcoin => {
                format!("{}/api/v1/esplora/bitcoin/mainnet", coincube_base)
            }
            bitcoin::Network::Signet => "https://blockstream.info/signet/api".to_string(),
            bitcoin::Network::Testnet | bitcoin::Network::Testnet4 | bitcoin::Network::Regtest => {
                format!("{}/api/v1/esplora/bitcoin/testnet", coincube_base)
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
                // Testnet + Signet (and the unreachable Testnet4/Regtest arms)
                // all map to the Breez Testnet network.
                bitcoin::Network::Testnet
                | bitcoin::Network::Signet
                | bitcoin::Network::Testnet4
                | bitcoin::Network::Regtest => breez::LiquidNetwork::Testnet,
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
