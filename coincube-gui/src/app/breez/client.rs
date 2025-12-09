use breez_sdk_liquid::prelude as breez;
use coincube_core::{
    miniscript::bitcoin::{
        bip32::DerivationPath,
        secp256k1::{All, Secp256k1},
    },
    signer::HotSigner,
};
use std::{
    str::FromStr,
    sync::{Arc, Mutex},
};

use super::{BreezConfig, BreezError};

/// Wrapper around HotSigner that implements Breez SDK's Signer trait
/// Based on SdkSigner from breez-sdk-liquid
struct HotSignerAdapter {
    signer: Arc<Mutex<HotSigner>>,
    secp: Secp256k1<All>,
}

impl HotSignerAdapter {
    fn new(signer: Arc<Mutex<HotSigner>>) -> Self {
        Self {
            signer,
            secp: Secp256k1::new(),
        }
    }
}

impl breez::Signer for HotSignerAdapter {
    fn sign_ecdsa(
        &self,
        msg: Vec<u8>,
        derivation_path: String,
    ) -> Result<Vec<u8>, breez::SignerError> {
        let signer = self.signer.lock().unwrap();

        // Parse the derivation path
        let path = DerivationPath::from_str(&derivation_path).map_err(|e| {
            breez::SignerError::Generic {
                err: format!("Invalid derivation path: {}", e),
            }
        })?;

        // Get private key at this derivation path
        let xpriv = signer.xpriv_at(&path, &self.secp);
        let privkey = xpriv.to_priv();

        // Sign the message hash (ECDSA)
        let msg_hash =
            coincube_core::miniscript::bitcoin::secp256k1::Message::from_digest_slice(&msg)
                .map_err(|e| breez::SignerError::Generic {
                    err: format!("Invalid message hash: {}", e),
                })?;

        let sig = self.secp.sign_ecdsa(&msg_hash, &privkey.inner);
        Ok(sig.serialize_compact().to_vec())
    }

    fn sign_ecdsa_recoverable(&self, msg: Vec<u8>) -> Result<Vec<u8>, breez::SignerError> {
        let signer = self.signer.lock().unwrap();

        // Use master key for recoverable signature (common in Lightning)
        let master_path = DerivationPath::master();
        let xpriv = signer.xpriv_at(&master_path, &self.secp);
        let privkey = xpriv.to_priv();

        // Sign the message hash (recoverable ECDSA)
        let msg_hash =
            coincube_core::miniscript::bitcoin::secp256k1::Message::from_digest_slice(&msg)
                .map_err(|e| breez::SignerError::Generic {
                    err: format!("Invalid message hash: {}", e),
                })?;

        let sig = self.secp.sign_ecdsa_recoverable(&msg_hash, &privkey.inner);
        let (recovery_id, sig_bytes) = sig.serialize_compact();

        // Format: recovery_id (1 byte) + signature (64 bytes)
        let mut result = Vec::with_capacity(65);
        result.push(recovery_id.to_i32() as u8);
        result.extend_from_slice(&sig_bytes);
        Ok(result)
    }

    fn derive_xpub(&self, derivation_path: String) -> Result<Vec<u8>, breez::SignerError> {
        let signer = self.signer.lock().unwrap();

        // Parse the derivation path
        let path = DerivationPath::from_str(&derivation_path).map_err(|e| {
            breez::SignerError::Generic {
                err: format!("Invalid derivation path: {}", e),
            }
        })?;

        // Get xpub at this path
        let xpub = signer.xpub_at(&path, &self.secp);

        // Encode as bytes (same format as SdkSigner)
        Ok(xpub.encode().to_vec())
    }

    fn xpub(&self) -> Result<Vec<u8>, breez::SignerError> {
        let signer = self.signer.lock().unwrap();

        // Get master xpub using public API (empty path = master)
        let empty_path = DerivationPath::master();
        let xpub = signer.xpub_at(&empty_path, &self.secp);

        // Encode as bytes
        Ok(xpub.encode().to_vec())
    }

    fn slip77_master_blinding_key(&self) -> Result<Vec<u8>, breez::SignerError> {
        let signer = self.signer.lock().unwrap();
        let key = signer.slip77_master_blinding_key();
        Ok(key.to_vec())
    }

    fn hmac_sha256(
        &self,
        msg: Vec<u8>,
        derivation_path: String,
    ) -> Result<Vec<u8>, breez::SignerError> {
        use coincube_core::miniscript::bitcoin::hashes::sha256::Hash as Sha256Hash;
        use coincube_core::miniscript::bitcoin::hashes::{Hash, HashEngine, Hmac, HmacEngine};

        let signer = self.signer.lock().unwrap();

        // Parse the derivation path
        let path = DerivationPath::from_str(&derivation_path).map_err(|e| {
            breez::SignerError::Generic {
                err: format!("Invalid derivation path: {}", e),
            }
        })?;

        // Get private key at this derivation path
        let xpriv = signer.xpriv_at(&path, &self.secp);
        let privkey = xpriv.to_priv();

        // Compute HMAC-SHA256 using the private key as the key
        let mut hmac_engine: HmacEngine<Sha256Hash> =
            HmacEngine::new(&privkey.inner.secret_bytes());
        hmac_engine.input(&msg);
        let hmac_result = Hmac::from_engine(hmac_engine);

        Ok(hmac_result.to_byte_array().to_vec())
    }

    fn ecies_encrypt(&self, msg: Vec<u8>) -> Result<Vec<u8>, breez::SignerError> {
        let _ = msg;
        // ECIES encryption not currently needed for external signer
        Err(breez::SignerError::Generic {
            err: "ECIES encryption not implemented for external signer".to_string(),
        })
    }

    fn ecies_decrypt(&self, msg: Vec<u8>) -> Result<Vec<u8>, breez::SignerError> {
        let _ = msg;
        // ECIES decryption not currently needed for external signer
        Err(breez::SignerError::Generic {
            err: "ECIES decryption not implemented for external signer".to_string(),
        })
    }
}

#[derive(Clone)]
pub struct BreezClient {
    sdk: Arc<breez::LiquidSdk>,
    signer: Arc<Mutex<HotSigner>>,
}

impl std::fmt::Debug for BreezClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BreezClient")
            .field("sdk", &"<LiquidSdk>")
            .field("signer", &"<HotSigner>")
            .finish()
    }
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

        Ok(Self { sdk, signer })
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
                payer_note: None,
                use_description_hash: Some(false),
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
                payer_note: None,
                use_asset_fees: None,
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }
}
