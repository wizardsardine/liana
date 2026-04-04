pub mod assets;
mod client;
mod config;

pub use client::BreezClient;
pub use config::BreezConfig;

// Re-export Breez SDK response types
pub use breez_sdk_liquid::prelude::{GetInfoResponse, ReceivePaymentResponse, SendPaymentResponse};

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use coincube_core::signer::HotSigner;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub enum BreezError {
    NetworkNotSupported(Network),
    Connection(String),
    Sdk(String),
    SignerNotFound(Fingerprint),
    SignerError(String),
}

impl std::fmt::Display for BreezError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BreezError::NetworkNotSupported(n) => {
                write!(f, "Liquid wallet is not supported on {} network", n)
            }
            BreezError::Connection(msg) => write!(f, "failed to connect Breez SDK: {}", msg),
            BreezError::Sdk(msg) => write!(f, "SDK request failed: {}", msg),
            BreezError::SignerNotFound(fp) => {
                write!(f, "Liquid wallet signer not found for fingerprint: {}", fp)
            }
            BreezError::SignerError(msg) => write!(f, "Signer error: {}", msg),
        }
    }
}

impl std::error::Error for BreezError {}

/// Load BreezClient from datadir using the master signer fingerprint.
/// Returns `Err(BreezError::NetworkNotSupported)` for non-mainnet/retest networks so
/// the caller can create a disconnected `BreezClient` instead of an error.
pub async fn load_breez_client(
    datadir: &Path,
    network: Network,
    master_signer_fingerprint: Fingerprint,
    password: &str,
) -> Result<Arc<BreezClient>, BreezError> {
    // Breez SDK (Liquid) supports mainnet and regtest.  Testnet, Testnet4 and
    // Signet are not supported — return NetworkNotSupported so the caller can
    // create a disconnected client and keep the rest of the app running normally.
    match network {
        Network::Bitcoin | Network::Regtest => {}
        _ => return Err(BreezError::NetworkNotSupported(network)),
    }

    // Load only the specific signer by fingerprint (more efficient and secure)
    let liquid_signer = HotSigner::from_datadir_by_fingerprint(
        datadir,
        network,
        master_signer_fingerprint,
        Some(password),
    )
    .map_err(|e| match e {
        coincube_core::signer::SignerError::MnemonicStorage(io_err)
            if io_err.kind() == std::io::ErrorKind::NotFound =>
        {
            BreezError::SignerNotFound(master_signer_fingerprint)
        }
        _ => BreezError::SignerError(e.to_string()),
    })?;

    // Create Breez config
    let breez_config = BreezConfig::from_env(network, datadir)?;

    // Connect to Breez SDK with the signer
    let breez_client =
        BreezClient::connect_with_signer(breez_config, Arc::new(Mutex::new(liquid_signer))).await?;

    Ok(Arc::new(breez_client))
}
