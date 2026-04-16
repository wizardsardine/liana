//! Spark wallet backend — gui-side surface.
//!
//! The actual [`breez-sdk-spark`] integration runs in a sibling binary
//! (`coincube-spark-bridge`) because the Liquid and Spark SDKs can't be
//! linked into the same process (incompatible `rusqlite` / `libsqlite3-sys`
//! dep graphs). See [`coincube-spark-bridge/Cargo.toml`] for the
//! accompanying standalone workspace.
//!
//! Public API:
//! - [`config::SparkConfig`]: reads `BREEZ_API_KEY` from env at build
//!   time (same key covers both Liquid and Spark).
//! - [`assets::SparkAsset`]: placeholder asset registry — BTC and
//!   Lightning only for now.
//! - [`client::SparkClient`]: cloneable subprocess handle, async
//!   methods for `get_info` / `list_payments` / `shutdown`.
//! - [`load_spark_client`]: convenience loader that reads the cube's
//!   Spark [`HotSigner`] from disk, unlocks it with the user PIN,
//!   builds a [`SparkConfig`], and hands the mnemonic to the bridge.

pub mod assets;
pub mod client;
pub mod config;

pub use assets::SparkAsset;
pub use client::{SparkClient, SparkClientError, SparkClientEvent};
pub use config::{SparkConfig, SparkConfigError};

use std::path::Path;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use coincube_core::signer::HotSigner;
use zeroize::Zeroizing;

/// Load the Spark backend from the cube's datadir + fingerprint + PIN.
///
/// Mirrors [`crate::app::breez_liquid::load_breez_client`]. The Spark
/// SDK needs the raw mnemonic on `connect`, so this function:
/// 1. loads the [`HotSigner`] from disk by fingerprint;
/// 2. decrypts its mnemonic with the supplied PIN;
/// 3. passes the mnemonic to the bridge subprocess via stdin;
/// 4. drops the mnemonic string (zeroized) as soon as the bridge
///    confirms init success.
///
/// Returns [`SparkLoadError::NetworkNotSupported`] for non-mainnet/regtest
/// networks so the caller can skip Spark setup without hard-failing.
pub async fn load_spark_client(
    datadir: &Path,
    network: Network,
    spark_signer_fingerprint: Fingerprint,
    password: &str,
) -> Result<Arc<SparkClient>, SparkLoadError> {
    // Only mainnet and regtest are supported — reject unsupported
    // networks early rather than waiting for the bridge handshake
    // to error out.
    match network {
        Network::Bitcoin | Network::Regtest => {}
        _ => return Err(SparkLoadError::NetworkNotSupported(network)),
    }

    // Load the specific signer by fingerprint, decrypting the mnemonic
    // with the PIN.
    let signer = HotSigner::from_datadir_by_fingerprint(
        datadir,
        network,
        spark_signer_fingerprint,
        Some(password),
    )
    .map_err(|e| match e {
        coincube_core::signer::SignerError::MnemonicStorage(io_err)
            if io_err.kind() == std::io::ErrorKind::NotFound =>
        {
            SparkLoadError::SignerNotFound(spark_signer_fingerprint)
        }
        _ => SparkLoadError::SignerError(e.to_string()),
    })?;

    // Extract the mnemonic as a Zeroizing<String> so the buffer is
    // scrubbed after the bridge has accepted it.
    let mnemonic: Zeroizing<String> = Zeroizing::new(signer.mnemonic_str());

    // Namespace the Spark SDK storage by fingerprint so multiple cubes
    // on the same install each get their own database. Without this,
    // two cubes sharing a datadir would read each other's wallet state.
    let storage_dir = datadir
        .join("spark")
        .join(spark_signer_fingerprint.to_string());
    if let Err(e) = std::fs::create_dir_all(&storage_dir) {
        return Err(SparkLoadError::Config(format!(
            "failed to create Spark storage dir {}: {}",
            storage_dir.display(),
            e
        )));
    }

    let config = SparkConfig::for_network(network, storage_dir)
        .map_err(|e| SparkLoadError::Config(e.to_string()))?;

    let client = SparkClient::connect(config, mnemonic.as_str())
        .await
        .map_err(SparkLoadError::from)?;

    // `mnemonic` Zeroizing<String> is dropped here; the bridge has its
    // own copy for the session lifetime.
    Ok(Arc::new(client))
}

/// Error type for [`load_spark_client`].
#[derive(Debug, Clone)]
pub enum SparkLoadError {
    NetworkNotSupported(Network),
    SignerNotFound(Fingerprint),
    SignerError(String),
    Config(String),
    Client(SparkClientError),
}

impl std::fmt::Display for SparkLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NetworkNotSupported(n) => {
                write!(f, "Spark wallet is not supported on {} network", n)
            }
            Self::SignerNotFound(fp) => {
                write!(f, "Spark wallet signer not found for fingerprint: {}", fp)
            }
            Self::SignerError(msg) => write!(f, "Spark signer error: {}", msg),
            Self::Config(msg) => write!(f, "Spark config error: {}", msg),
            Self::Client(err) => write!(f, "Spark client error: {}", err),
        }
    }
}

impl std::error::Error for SparkLoadError {}

impl From<SparkClientError> for SparkLoadError {
    fn from(value: SparkClientError) -> Self {
        Self::Client(value)
    }
}
