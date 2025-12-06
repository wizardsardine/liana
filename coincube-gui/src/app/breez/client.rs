// breez/client.rs
// ============================================
// STUB IMPLEMENTATION - Real Breez SDK code is commented out below
// The real Breez SDK has compilation issues due to rand_core version conflicts.
//
// TO ENABLE REAL BREEZ SDK:
// 1. Uncomment breez-sdk-liquid in Cargo.toml
// 2. Comment out the STUB section below
// 3. Uncomment the REAL IMPLEMENTATION section
// ============================================

// ========== STUB IMPLEMENTATION (ACTIVE) ==========
use coincube_core::signer::HotSigner;
use std::sync::{Arc, Mutex};

use super::{BreezConfig, BreezError};

// Stub response types
#[derive(Debug, Clone)]
pub struct GetInfoResponse {
    pub balance_sat: u64,
    pub pending_send_sat: u64,
    pub pending_receive_sat: u64,
}

#[derive(Debug, Clone)]
pub struct ReceivePaymentResponse {
    pub destination: String, // BOLT11 invoice
}

#[derive(Debug, Clone)]
pub struct SendPaymentResponse {
    pub payment_hash: String,
}

pub struct BreezClient {
    _config: BreezConfig,
    signer: Arc<Mutex<HotSigner>>,
}

impl std::fmt::Debug for BreezClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BreezClient")
            .field("_config", &self._config)
            .field("signer", &"<HotSigner>")
            .finish()
    }
}

impl BreezClient {
    /// Create a stub BreezClient for testing/development
    /// Used when real BreezClient cannot be loaded
    pub fn stub() -> Self {
        use coincube_core::signer::HotSigner;
        let stub_signer = HotSigner::generate(coincube_core::miniscript::bitcoin::Network::Bitcoin)
            .expect("Failed to generate stub signer");
        Self {
            _config: BreezConfig {
                api_key: "stub".to_string(),
                network: coincube_core::miniscript::bitcoin::Network::Bitcoin,
                working_dir: std::path::PathBuf::from("/tmp/stub"),
            },
            signer: Arc::new(Mutex::new(stub_signer)),
        }
    }

    /// Connect to Breez SDK using an external signer (HotSigner)
    /// STUB: Returns a mock client without connecting to real SDK
    pub async fn connect_with_signer(
        cfg: BreezConfig,
        signer: Arc<Mutex<HotSigner>>,
    ) -> Result<Self, BreezError> {
        log::info!("STUB: BreezClient::connect_with_signer - using stub implementation");
        Ok(Self {
            _config: cfg,
            signer,
        })
    }

    /// STUB: Returns mock wallet info
    pub async fn info(&self) -> Result<GetInfoResponse, BreezError> {
        log::info!("STUB: BreezClient::info - returning mock balance");
        Ok(GetInfoResponse {
            balance_sat: 50000,
            pending_send_sat: 0,
            pending_receive_sat: 0,
        })
    }

    /// STUB: Returns a mock invoice
    pub async fn receive_invoice(
        &self,
        amount_sat: Option<u64>,
        description: Option<String>,
    ) -> Result<ReceivePaymentResponse, BreezError> {
        let amt = amount_sat.unwrap_or(0);
        let desc = description.unwrap_or_else(|| "stub invoice".to_string());
        log::info!("STUB: BreezClient::receive_invoice - amount: {}, desc: {}", amt, desc);
        Ok(ReceivePaymentResponse {
            destination: format!("lnbc{}1stub{}", amt, desc.chars().take(5).collect::<String>()),
        })
    }

    /// STUB: Returns a mock payment response
    pub async fn pay_invoice(
        &self,
        invoice: String,
        amount_sat: Option<u64>,
    ) -> Result<SendPaymentResponse, BreezError> {
        let amt = amount_sat.unwrap_or(0);
        log::info!("STUB: BreezClient::pay_invoice - invoice: {}, amount: {}", invoice, amt);
        Ok(SendPaymentResponse {
            payment_hash: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        })
    }
}

// ========== REAL IMPLEMENTATION (COMMENTED OUT) ==========
/*
use breez_sdk_liquid::prelude as breez;

/// Wrapper around HotSigner that implements Breez SDK's Signer trait
struct HotSignerAdapter {
    signer: Arc<Mutex<HotSigner>>,
}

impl HotSignerAdapter {
    fn new(signer: Arc<Mutex<HotSigner>>) -> Self {
        Self { signer }
    }
}

impl breez::Signer for HotSignerAdapter {
    fn sign_ecdsa(&self, msg: &[u8], derivation_path: &str) -> breez::SignerResult<Vec<u8>> {
        // Delegate to HotSigner's signing implementation
        // This will need to be implemented based on HotSigner's API
        let _signer = self.signer.lock().unwrap();
        // TODO: Implement actual signing using HotSigner
        // For now, return an error until we implement the signing logic
        Err(breez::SignerError::Generic {
            err: "ECDSA signing not yet implemented".to_string(),
        })
    }

    fn sign_ecdsa_recoverable(&self, msg: &[u8]) -> breez::SignerResult<Vec<u8>> {
        let _signer = self.signer.lock().unwrap();
        // TODO: Implement recoverable ECDSA signing
        Err(breez::SignerError::Generic {
            err: "Recoverable ECDSA signing not yet implemented".to_string(),
        })
    }

    fn derive_xpub(&self, derivation_path: &str) -> breez::SignerResult<Vec<u8>> {
        let signer = self.signer.lock().unwrap();
        // Derive extended public key at the given path
        let xpriv = signer
            .xpriv_at(derivation_path)
            .map_err(|e| breez::SignerError::Generic {
                err: format!("Failed to derive xpriv: {}", e),
            })?;

        let xpub =
            bitcoin::bip32::Xpub::from_priv(&breez::bitcoin::secp256k1::Secp256k1::new(), &xpriv);
        Ok(xpub.encode().to_vec())
    }

    fn xpub(&self) -> breez::SignerResult<Vec<u8>> {
        let signer = self.signer.lock().unwrap();
        let fingerprint = signer.fingerprint();
        // Return master xpub
        // TODO: Get the actual xpub from HotSigner
        Err(breez::SignerError::Generic {
            err: format!(
                "xpub retrieval not yet implemented for fingerprint {}",
                fingerprint
            ),
        })
    }

    fn slip77_master_blinding_key(&self) -> breez::SignerResult<Vec<u8>> {
        // SLIP77 is used for Liquid confidential transactions
        // Derive from the mnemonic
        let _signer = self.signer.lock().unwrap();
        // TODO: Implement SLIP77 key derivation
        Err(breez::SignerError::Generic {
            err: "SLIP77 master blinding key not yet implemented".to_string(),
        })
    }

    fn hmac_sha256(&self, msg: &[u8], derivation_path: &str) -> breez::SignerResult<Vec<u8>> {
        let _signer = self.signer.lock().unwrap();
        // TODO: Implement HMAC-SHA256
        let _ = (msg, derivation_path);
        Err(breez::SignerError::Generic {
            err: "HMAC-SHA256 not yet implemented".to_string(),
        })
    }

    fn ecies_encrypt(&self, msg: &[u8]) -> breez::SignerResult<Vec<u8>> {
        let _ = msg;
        Err(breez::SignerError::Generic {
            err: "ECIES encryption not yet implemented".to_string(),
        })
    }

    fn ecies_decrypt(&self, msg: &[u8]) -> breez::SignerResult<Vec<u8>> {
        let _ = msg;
        Err(breez::SignerError::Generic {
            err: "ECIES decryption not yet implemented".to_string(),
        })
    }
}

pub struct BreezClient {
    sdk: Arc<breez::LiquidSdk>,
    signer: Arc<Mutex<HotSigner>>,
}

impl BreezClient {
    /// Connect to Breez SDK using an external signer (HotSigner)
    pub async fn connect_with_signer(
        cfg: BreezConfig,
        signer: Arc<Mutex<HotSigner>>,
    ) -> Result<Self, BreezError> {
        let signer_adapter = HotSignerAdapter::new(signer.clone());

        let request = breez::ConnectWithSignerRequest {
            config: cfg.sdk_config(),
        };

        let sdk = breez::LiquidSdk::connect_with_signer(request, Box::new(signer_adapter))
            .await
            .map_err(|e| BreezError::Connection(e.to_string()))?;

        Ok(Self {
            sdk: Arc::new(sdk),
            signer,
        })
    }

    pub async fn info(&self) -> Result<breez::GetInfoResponse, BreezError> {
        self.sdk
            .get_info()
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn receive_invoice(
        &self,
        amount_sat: Option<u64>,
        description: Option<String>,
    ) -> Result<breez::ReceivePaymentResponse, BreezError> {
        let prepare = self
            .sdk
            .prepare_receive_payment(&breez::PrepareReceiveRequest {
                payment_method: breez::PaymentMethod::Bolt11Invoice,
                amount: amount_sat.map(|sat| breez::ReceiveAmount::Bitcoin {
                    payer_amount_sat: sat,
                }),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        self.sdk
            .receive_payment(&breez::ReceivePaymentRequest {
                prepare_response: prepare,
                description,
                ..Default::default()
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn pay_invoice(
        &self,
        invoice: String,
        amount_sat: Option<u64>,
    ) -> Result<breez::SendPaymentResponse, BreezError> {
        let prepare = self
            .sdk
            .prepare_send_payment(&breez::PrepareSendRequest {
                destination: invoice,
                amount: amount_sat.map(|sat| breez::PayAmount::Bitcoin {
                    receiver_amount_sat: sat,
                }),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        self.sdk
            .send_payment(&breez::SendPaymentRequest {
                prepare_response: prepare,
                ..Default::default()
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }
}
*/
