use async_trait::async_trait;
use breez_sdk_liquid::{
    bitcoin::Network,
    model::{LightningPaymentLimitsResponse, OnchainPaymentLimitsResponse, RefundResponse},
    prelude as breez, InputType,
};
use coincube_core::{
    miniscript::bitcoin::{
        bip32::DerivationPath,
        secp256k1::{All, Secp256k1},
        Amount,
    },
    signer::HotSigner,
};
use std::{
    str::FromStr,
    sync::{Arc, Mutex},
};

use iced::futures::{SinkExt, Stream};

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

        let sig = self.secp.sign_ecdsa_low_r(&msg_hash, &privkey.inner);
        Ok(sig.serialize_der().to_vec())
    }

    fn sign_ecdsa_recoverable(&self, msg: Vec<u8>) -> Result<Vec<u8>, breez::SignerError> {
        let secp = Secp256k1::new();
        let signer = self.signer.lock().unwrap();
        let master_xpriv = signer.xpriv_at(&"m".parse().unwrap(), &secp);
        let keypair = master_xpriv.to_keypair(&secp);
        let s = msg.as_slice();

        let msg: coincube_core::miniscript::bitcoin::secp256k1::Message =
            coincube_core::miniscript::bitcoin::secp256k1::Message::from_digest_slice(s)
                .map_err(|e| breez::SignerError::Generic { err: e.to_string() })?;

        let recoverable_sig = secp.sign_ecdsa_recoverable(&msg, &keypair.secret_key());
        let (recovery_id, sig) = recoverable_sig.serialize_compact();
        let mut complete_signature = vec![31 + recovery_id.to_i32() as u8];
        complete_signature.extend_from_slice(&sig);
        Ok(complete_signature)
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
    sdk: Option<Arc<breez::LiquidSdk>>,
    signer: Option<Arc<Mutex<HotSigner>>>,
    network: Network,
}

impl std::fmt::Debug for BreezClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BreezClient")
            .field("sdk", &self.sdk.as_ref().map(|_| "<LiquidSdk>"))
            .field("signer", &self.signer.as_ref().map(|_| "<HotSigner>"))
            .field("network", &self.network)
            .finish()
    }
}

impl BreezClient {
    /// Create a disconnected client for networks where Breez SDK is not supported.
    /// All SDK methods will return `BreezError::NetworkNotSupported`.
    pub fn disconnected(network: Network) -> Self {
        Self {
            sdk: None,
            signer: None,
            network,
        }
    }

    /// Returns a reference to the inner SDK, or `NetworkNotSupported` if disconnected.
    fn get_sdk(&self) -> Result<&Arc<breez::LiquidSdk>, BreezError> {
        self.sdk
            .as_ref()
            .ok_or(BreezError::NetworkNotSupported(self.network))
    }

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
            sdk: Some(sdk),
            signer: Some(signer),
            network: cfg.network,
        })
    }

    pub async fn info(&self) -> Result<breez::GetInfoResponse, BreezError> {
        self.get_sdk()?
            .get_info()
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn receive_invoice(
        &self,
        amount: Option<Amount>,
        description: Option<String>,
    ) -> Result<breez::ReceivePaymentResponse, BreezError> {
        let sdk = self.get_sdk()?;
        let prepare = sdk
            .prepare_receive_payment(&breez::PrepareReceiveRequest {
                payment_method: breez::PaymentMethod::Bolt11Invoice,
                amount: amount.map(|a| breez::ReceiveAmount::Bitcoin {
                    payer_amount_sat: a.to_sat(),
                }),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        sdk.receive_payment(&breez::ReceivePaymentRequest {
            prepare_response: prepare,
            description,
            payer_note: None,
            description_hash: None,
        })
        .await
        .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    /// Generate a BOLT11 invoice for an LNURL-pay request.
    /// Uses `description_hash` (required by LNURL spec) instead of `description`.
    pub async fn receive_lnurl_invoice(
        &self,
        payer_amount_sat: u64,
        description_hash: String,
    ) -> Result<breez::ReceivePaymentResponse, BreezError> {
        if description_hash.len() != 64 || !description_hash.chars().all(|c| c.is_ascii_hexdigit())
        {
            return Err(BreezError::Sdk(format!(
                "invalid description_hash: expected 64-char hex SHA256, got \"{}\"",
                description_hash
            )));
        }

        let sdk = self.get_sdk()?;
        let prepare = sdk
            .prepare_receive_payment(&breez::PrepareReceiveRequest {
                payment_method: breez::PaymentMethod::Bolt11Invoice,
                amount: Some(breez::ReceiveAmount::Bitcoin { payer_amount_sat }),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        sdk.receive_payment(&breez::ReceivePaymentRequest {
            prepare_response: prepare,
            description: None,
            payer_note: None,
            description_hash: Some(breez::DescriptionHash::Custom {
                hash: description_hash,
            }),
        })
        .await
        .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn receive_onchain(
        &self,
        amount_sat: Option<u64>,
    ) -> Result<breez::ReceivePaymentResponse, BreezError> {
        let sdk = self.get_sdk()?;
        let prepare = sdk
            .prepare_receive_payment(&breez::PrepareReceiveRequest {
                payment_method: breez::PaymentMethod::BitcoinAddress,
                amount: amount_sat.map(|sat| breez::ReceiveAmount::Bitcoin {
                    payer_amount_sat: sat,
                }),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        sdk.receive_payment(&breez::ReceivePaymentRequest {
            prepare_response: prepare,
            description: None,
            payer_note: None,
            description_hash: None,
        })
        .await
        .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn receive_liquid(&self) -> Result<breez::ReceivePaymentResponse, BreezError> {
        let sdk = self.get_sdk()?;
        let prepare = sdk
            .prepare_receive_payment(&breez::PrepareReceiveRequest {
                payment_method: breez::PaymentMethod::LiquidAddress,
                amount: None,
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        sdk.receive_payment(&breez::ReceivePaymentRequest {
            prepare_response: prepare,
            description: None,
            payer_note: None,
            description_hash: None,
        })
        .await
        .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    /// Get (or create) the node's BOLT12 offer string.
    /// The SDK caches offers by description, so using a consistent description
    /// ensures the same offer is reused across calls.
    pub async fn receive_bolt12_offer(&self) -> Result<String, BreezError> {
        let sdk = self.get_sdk()?;

        let prepare = sdk
            .prepare_receive_payment(&breez::PrepareReceiveRequest {
                payment_method: breez::PaymentMethod::Bolt12Offer,
                amount: None,
            })
            .await;

        match prepare {
            Ok(p) => {
                // Normal path: prepare succeeded, create or retrieve offer.
                let response = sdk
                    .receive_payment(&breez::ReceivePaymentRequest {
                        prepare_response: p,
                        description: Some("coincube".to_string()),
                        payer_note: None,
                        description_hash: None,
                    })
                    .await
                    .map_err(|e| BreezError::Sdk(e.to_string()))?;

                Ok(response.destination)
            }
            Err(prepare_err) => {
                // Prepare failed (e.g. Boltz API down). Try to retrieve a cached
                // offer using a minimal prepare response. If no cached offer
                // exists, surface the original prepare error rather than creating
                // a new offer with fabricated (zero) fee parameters.
                log::warn!(
                    "[BREEZ] prepare_receive_payment for Bolt12 failed: {}. \
                     Attempting to retrieve cached offer.",
                    prepare_err
                );

                let fallback = breez::PrepareReceiveResponse {
                    payment_method: breez::PaymentMethod::Bolt12Offer,
                    amount: None,
                    fees_sat: 0,
                    min_payer_amount_sat: None,
                    max_payer_amount_sat: None,
                    swapper_feerate: None,
                };

                match sdk
                    .receive_payment(&breez::ReceivePaymentRequest {
                        prepare_response: fallback,
                        description: Some("coincube".to_string()),
                        payer_note: None,
                        description_hash: None,
                    })
                    .await
                {
                    Ok(response) => Ok(response.destination),
                    Err(receive_err) => {
                        // No cached offer available — return the original prepare
                        // error which is more actionable for the user.
                        log::error!(
                            "[BREEZ] Cached offer retrieval also failed: {}",
                            receive_err
                        );
                        Err(BreezError::Sdk(prepare_err.to_string()))
                    }
                }
            }
        }
    }

    /// Generate a Liquid address for receiving USDt (or any Liquid asset).
    /// `amount` is in base units (e.g. 100_000_000 = 1 USDt); pass `None` for amountless.
    /// `precision` is the asset's decimal precision (8 for USDt).
    pub async fn receive_usdt(
        &self,
        asset_id: &str,
        amount: Option<u64>,
        precision: u8,
    ) -> Result<breez::ReceivePaymentResponse, BreezError> {
        let sdk = self.get_sdk()?;
        let payer_amount = amount
            .map(|a| safe_base_units_to_f64(a, precision))
            .transpose()?;
        let prepare = sdk
            .prepare_receive_payment(&breez::PrepareReceiveRequest {
                payment_method: breez::PaymentMethod::LiquidAddress,
                amount: Some(breez::ReceiveAmount::Asset {
                    asset_id: asset_id.to_string(),
                    payer_amount,
                }),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        sdk.receive_payment(&breez::ReceivePaymentRequest {
            prepare_response: prepare,
            description: None,
            payer_note: None,
            description_hash: None,
        })
        .await
        .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    /// Prepare a USDt (or any Liquid asset) send payment.
    /// `amount` is in base units; `precision` is the asset's decimal precision (8 for USDt).
    /// `from_asset` enables cross-asset swaps via SideSwap when it differs from `asset_id`.
    /// Returns a `PrepareSendResponse` that must be passed to `send_payment()`.
    pub async fn prepare_send_asset(
        &self,
        destination: String,
        to_asset_id: &str,
        amount: u64,
        precision: u8,
        from_asset_id: Option<&str>,
    ) -> Result<breez::PrepareSendResponse, BreezError> {
        let receiver_amount = safe_base_units_to_f64(amount, precision)?;
        // Cross-asset swaps (from_asset != to_asset) cannot use asset fees per SDK constraint
        let is_cross_asset = from_asset_id.is_some_and(|from| from != to_asset_id);
        self.get_sdk()?
            .prepare_send_payment(&breez::PrepareSendRequest {
                destination,
                amount: Some(breez::PayAmount::Asset {
                    to_asset: to_asset_id.to_string(),
                    receiver_amount,
                    estimate_asset_fees: if is_cross_asset { None } else { Some(true) },
                    from_asset: from_asset_id.map(|s| s.to_string()),
                }),
                disable_mrh: None,
                payment_timeout_sec: None,
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn pay_invoice(
        &self,
        invoice: String,
        amount_sat: Option<u64>,
    ) -> Result<breez::SendPaymentResponse, BreezError> {
        let sdk = self.get_sdk()?;
        let prepare = sdk
            .prepare_send_payment(&breez::PrepareSendRequest {
                destination: invoice,
                amount: amount_sat.map(|sat| breez::PayAmount::Bitcoin {
                    receiver_amount_sat: sat,
                }),
                disable_mrh: None,
                payment_timeout_sec: None,
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        sdk.send_payment(&breez::SendPaymentRequest {
            prepare_response: prepare,
            payer_note: None,
            use_asset_fees: None,
        })
        .await
        .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn prepare_send_payment(
        &self,
        request: &breez::PrepareSendRequest,
    ) -> Result<breez::PrepareSendResponse, BreezError> {
        self.get_sdk()?
            .prepare_send_payment(request)
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn prepare_pay_onchain(
        &self,
        request: &breez::PreparePayOnchainRequest,
    ) -> Result<breez::PreparePayOnchainResponse, BreezError> {
        self.get_sdk()?
            .prepare_pay_onchain(request)
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn pay_onchain(
        &self,
        request: &breez::PayOnchainRequest,
    ) -> Result<breez::SendPaymentResponse, BreezError> {
        self.get_sdk()?
            .pay_onchain(request)
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn send_payment(
        &self,
        request: &breez::SendPaymentRequest,
    ) -> Result<breez::SendPaymentResponse, BreezError> {
        self.get_sdk()?
            .send_payment(request)
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn list_payments(
        &self,
        limit: Option<u32>,
    ) -> Result<Vec<breez::Payment>, BreezError> {
        self.get_sdk()?
            .list_payments(&breez::ListPaymentsRequest {
                filters: None,
                states: None,
                from_timestamp: None,
                to_timestamp: None,
                offset: None,
                limit,
                details: None,
                sort_ascending: Some(false), // Most recent first
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn list_refundables(&self) -> Result<Vec<breez::RefundableSwap>, BreezError> {
        self.get_sdk()?
            .list_refundables()
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn fetch_payment_proposed_fees(
        &self,
        swap_id: &str,
    ) -> Result<breez::FetchPaymentProposedFeesResponse, BreezError> {
        self.get_sdk()?
            .fetch_payment_proposed_fees(&breez::FetchPaymentProposedFeesRequest {
                swap_id: swap_id.to_string(),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn accept_payment_proposed_fees(
        &self,
        response: breez::FetchPaymentProposedFeesResponse,
    ) -> Result<(), BreezError> {
        self.get_sdk()?
            .accept_payment_proposed_fees(&breez::AcceptPaymentProposedFeesRequest { response })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn validate_input(&self, input: String) -> Option<InputType> {
        self.sdk.as_ref()?.parse(&input).await.ok()
    }

    pub async fn fetch_lightning_limits(
        &self,
    ) -> Result<LightningPaymentLimitsResponse, BreezError> {
        self.get_sdk()?
            .fetch_lightning_limits()
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn fetch_onchain_limits(&self) -> Result<OnchainPaymentLimitsResponse, BreezError> {
        self.get_sdk()?
            .fetch_onchain_limits()
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    /// Manually trigger wallet synchronization with the blockchain
    /// This is useful after payments to immediately update the balance
    pub async fn sync(&self) -> Result<(), BreezError> {
        self.get_sdk()?
            .sync(false)
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }
    pub async fn rescan_onchain_swaps(&self) -> Result<(), BreezError> {
        self.get_sdk()?
            .rescan_onchain_swaps()
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn refund_onchain_tx(
        &self,
        refund_request: breez::RefundRequest,
    ) -> Result<RefundResponse, BreezError> {
        self.get_sdk()?
            .refund(&refund_request)
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub fn liquid_signer(&self) -> Option<Arc<Mutex<HotSigner>>> {
        self.signer.clone()
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn subscription(&self) -> iced::Subscription<breez::SdkEvent> {
        iced::Subscription::run_with(
            BreezSubscriptionState {
                client: self.clone(),
            },
            make_breez_stream,
        )
    }
}

struct BreezSubscriptionState {
    client: BreezClient,
}

impl std::hash::Hash for BreezSubscriptionState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.client.network.hash(state);
    }
}

fn make_breez_stream(state: &BreezSubscriptionState) -> impl Stream<Item = breez::SdkEvent> {
    let client = state.client.clone();
    iced::stream::channel(
        100,
        move |mut output: iced::futures::channel::mpsc::Sender<breez::SdkEvent>| async move {
            let Some(sdk) = client.sdk.clone() else {
                std::future::pending::<()>().await;
                return;
            };

            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let listener = BreezEventListener { sender };

            if let Ok(id) = sdk.add_event_listener(Box::new(listener)).await {
                while let Some(event) = receiver.recv().await {
                    let _ = output.send(event).await;
                }

                let _ = sdk.remove_event_listener(id).await;
            }

            std::future::pending().await
        },
    )
}

/// Converts `amount` base-units to an `f64` display value by dividing by `10^precision`.
///
/// Returns `Err(BreezError::Sdk)` if:
/// - `amount` exceeds 2^53 (largest exactly-representable f64 integer), or
/// - `precision` is so large that `10^precision` overflows `u64` (precision > 19).
fn safe_base_units_to_f64(amount: u64, precision: u8) -> Result<f64, BreezError> {
    const MAX_EXACT_F64_INT: u64 = 1u64 << 53;
    if amount > MAX_EXACT_F64_INT {
        return Err(BreezError::Sdk(format!(
            "amount {} exceeds maximum exactly-representable f64 integer (2^53 = {})",
            amount, MAX_EXACT_F64_INT,
        )));
    }
    let divisor = 10_u64.checked_pow(precision as u32).ok_or_else(|| {
        BreezError::Sdk(format!(
            "precision {} causes 10^precision to overflow u64",
            precision,
        ))
    })?;
    Ok(amount as f64 / divisor as f64)
}

struct BreezEventListener {
    sender: tokio::sync::mpsc::UnboundedSender<breez::SdkEvent>,
}

#[async_trait]
impl breez::EventListener for BreezEventListener {
    async fn on_event(&self, e: breez::SdkEvent) {
        let _ = self.sender.send(e);
    }
}
