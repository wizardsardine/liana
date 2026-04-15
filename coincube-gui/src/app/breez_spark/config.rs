//! Spark SDK configuration — read at **compile time** from env vars.
//!
//! Mirrors the pattern used by [`crate::app::breez_liquid::config`]:
//! the API key is baked into the binary via `env!(...)` so the packaged
//! app doesn't need a runtime `.env` to connect. Override the value by
//! setting `BREEZ_SPARK_API_KEY` (or `BREEZ_API_KEY`, which is the
//! fallback — the same issued Breez key currently covers both the Liquid
//! and Spark SDKs) before running `cargo build`.

use coincube_spark_protocol::Network as ProtocolNetwork;
use coincube_core::miniscript::bitcoin;

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

/// The compile-time API key value — prefers `BREEZ_SPARK_API_KEY` over
/// the shared `BREEZ_API_KEY`.
///
/// This is a free function rather than a `const` so a future refactor can
/// swap it to runtime lookup if we ever decide to un-bake the key from
/// the binary.
fn api_key_from_env() -> String {
    // `option_env!` is a compile-time lookup, so these unwraps happen at
    // build time — no runtime branch cost. If neither is set the baked
    // string will be empty, which `SparkClient::connect` will reject
    // with a clear error at handshake time.
    option_env!("BREEZ_SPARK_API_KEY")
        .or(option_env!("BREEZ_API_KEY"))
        .unwrap_or("")
        .to_string()
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
