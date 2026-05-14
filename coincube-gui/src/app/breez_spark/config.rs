//! Spark SDK configuration — read at **compile time** from env vars.
//!
//! Mirrors the pattern used by [`crate::app::breez_liquid::config`]:
//! the API key is baked into the binary via `env!(...)` so the packaged
//! app doesn't need a runtime `.env` to connect. Set `BREEZ_API_KEY`
//! before running `cargo build` — a single Breez key covers both the
//! Liquid and Spark SDKs.

use coincube_core::miniscript::bitcoin;
use coincube_spark_protocol::Network as ProtocolNetwork;

/// Resolved Spark configuration for a cube.
///
/// The gui builds one of these at cube load time from the compile-time
/// API key and the cube's network / storage dir, then hands it to
/// [`crate::app::breez_spark::SparkClient::connect`] to drive the bridge
/// subprocess handshake.
#[derive(Debug, Clone)]
pub struct SparkConfig {
    pub api_key: String,
    pub network: ProtocolNetwork,
    pub storage_dir: std::path::PathBuf,
}

impl SparkConfig {
    /// Build a Spark config for the given bitcoin network, pulling the
    /// API key from the compile-time environment.
    ///
    /// Returns `Err` for networks Spark doesn't support (testnet/signet)
    /// so callers can fall back to a disconnected placeholder, matching
    /// the behavior of the Liquid loader for unsupported networks.
    pub fn for_network(
        network: bitcoin::Network,
        storage_dir: std::path::PathBuf,
    ) -> Result<Self, SparkConfigError> {
        let api_key = api_key_from_env();
        let protocol_network = match network {
            bitcoin::Network::Bitcoin => ProtocolNetwork::Mainnet,
            bitcoin::Network::Regtest => ProtocolNetwork::Regtest,
            other => return Err(SparkConfigError::UnsupportedNetwork(other)),
        };

        Ok(Self {
            api_key,
            network: protocol_network,
            storage_dir,
        })
    }
}

/// The compile-time API key value — reads `BREEZ_API_KEY`.
///
/// Uses `env!` so a missing key is a hard compile error, matching
/// the Liquid config at [`crate::app::breez_liquid::config`].
fn api_key_from_env() -> String {
    env!("BREEZ_API_KEY").to_string()
}

#[derive(Debug, Clone)]
pub enum SparkConfigError {
    UnsupportedNetwork(bitcoin::Network),
}

impl std::fmt::Display for SparkConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedNetwork(n) => {
                write!(f, "Spark wallet is not supported on {} network", n)
            }
        }
    }
}

impl std::error::Error for SparkConfigError {}
