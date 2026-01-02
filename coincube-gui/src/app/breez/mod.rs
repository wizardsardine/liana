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
    MissingApiKey,
    Connection(String),
    Sdk(String),
    SignerNotFound(Fingerprint),
    SignerError(String),
}

impl std::fmt::Display for BreezError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BreezError::MissingApiKey => write!(f, "Breez API key missing (set BREEZ_API_KEY)"),
            BreezError::Connection(msg) => write!(f, "failed to connect Breez SDK: {}", msg),
            BreezError::Sdk(msg) => write!(f, "SDK request failed: {}", msg),
            BreezError::SignerNotFound(fp) => {
                write!(f, "Active wallet signer not found for fingerprint: {}", fp)
            }
            BreezError::SignerError(msg) => write!(f, "Signer error: {}", msg),
        }
    }
}

impl std::error::Error for BreezError {}

/// Load BreezClient from datadir using the Active wallet signer fingerprint
pub async fn load_breez_client(
    datadir: &Path,
    network: Network,
    active_signer_fingerprint: Fingerprint,
    password: &str,
) -> Result<Arc<BreezClient>, BreezError> {
    // Load only the specific signer by fingerprint (more efficient and secure)
    let active_signer = HotSigner::from_datadir_by_fingerprint(
        datadir,
        network,
        active_signer_fingerprint,
        password,
    )
    .map_err(|e| match e {
        coincube_core::signer::SignerError::MnemonicStorage(io_err)
            if io_err.kind() == std::io::ErrorKind::NotFound =>
        {
            BreezError::SignerNotFound(active_signer_fingerprint)
        }
        _ => BreezError::SignerError(e.to_string()),
    })?;

    // Create Breez config
    let breez_config = BreezConfig::from_env(network, datadir)?;

    // Connect to Breez SDK with the signer
    let breez_client =
        BreezClient::connect_with_signer(breez_config, Arc::new(Mutex::new(active_signer))).await?;

    Ok(Arc::new(breez_client))
}
