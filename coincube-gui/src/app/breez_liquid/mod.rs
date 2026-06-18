pub mod assets;
mod client;
mod config;
pub mod swap_status;

pub use client::BreezClient;
pub use config::BreezConfig;

// Re-export Breez SDK response types
pub use breez_sdk_liquid::prelude::{GetInfoResponse, ReceivePaymentResponse, SendPaymentResponse};

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use coincube_core::signer::MasterSigner;
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
///
/// The master signer is always loaded — its mnemonic backs seed-derived
/// features (Spark, and P2P/Mostro identity) that work on networks where the
/// Liquid SDK doesn't. The Liquid SDK itself is only *connected* where a real
/// Liquid Esplora backend exists (`crate::app::features::liquid` — mainnet,
/// testnet, signet). On the rest (Testnet4, Regtest) it returns a
/// disconnected client that still carries the signer, so the Liquid wallet UI
/// stays gated (no localhost Esplora) without taking down P2P.
pub async fn load_breez_client(
    datadir: &Path,
    network: Network,
    master_signer_fingerprint: Fingerprint,
    password: &str,
) -> Result<Arc<BreezClient>, BreezError> {
    let liquid_supported = crate::app::features::liquid(network).is_available();

    // Load only the specific signer by fingerprint (more efficient and secure).
    let liquid_signer = match MasterSigner::from_datadir_by_fingerprint(
        datadir,
        network,
        master_signer_fingerprint,
        Some(password),
    ) {
        Ok(signer) => Arc::new(Mutex::new(signer)),
        // On a network where Liquid isn't connected, a missing signer isn't
        // fatal — return a plain disconnected client so a seed-less cube
        // (e.g. watch-only) still loads. On a supported network the signer is
        // required to connect, so the error propagates.
        Err(e) if !liquid_supported => {
            log::info!(
                "No master signer for disconnected cube on {network}: {e}; \
                 using a signer-less disconnected client"
            );
            return Ok(Arc::new(BreezClient::disconnected(network)));
        }
        Err(e) => {
            return Err(match e {
                coincube_core::signer::SignerError::MnemonicStorage(io_err)
                    if io_err.kind() == std::io::ErrorKind::NotFound =>
                {
                    BreezError::SignerNotFound(master_signer_fingerprint)
                }
                coincube_core::signer::SignerError::SignerNotFound(fingerprint) => {
                    BreezError::SignerNotFound(fingerprint)
                }
                _ => BreezError::SignerError(e.to_string()),
            });
        }
    };

    // Liquid is only enabled where a real Liquid Esplora backend exists (see
    // `features::liquid`, the single source of truth for the gate). On
    // unsupported networks return a disconnected client that still carries
    // the signer — the Liquid UI stays gated (rail greyed, panels show
    // disconnected) while the mnemonic remains available to P2P/Spark.
    if !liquid_supported {
        return Ok(Arc::new(BreezClient::disconnected_with_signer(
            network,
            liquid_signer,
        )));
    }

    // Create Breez config
    let breez_config = BreezConfig::from_env(network, datadir)?;

    // Connect to Breez SDK with the signer
    let breez_client = BreezClient::connect_with_signer(breez_config, liquid_signer).await?;

    Ok(Arc::new(breez_client))
}
