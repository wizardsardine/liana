//! Domain types for wallet backends.
//!
//! These types decouple the panels from SDK-specific payment representations
//! (e.g. [`breez_sdk_liquid::prelude::Payment`]). Each wallet backend maps its
//! native types into these domain types at the boundary.

use breez_sdk_liquid::model::{
    PaymentDetails as LiquidPaymentDetails, PaymentState as LiquidPaymentState,
    PaymentType as LiquidPaymentType,
};
use breez_sdk_liquid::prelude::{Payment as LiquidPayment, RefundableSwap as LiquidRefundableSwap};

use crate::app::breez_liquid::assets::USDT_PRECISION;

/// Identifies a wallet backend.
#[derive(
    Debug, Clone, Copy, Default, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WalletKind {
    /// Spark wallet — default for everyday Lightning UX.
    #[default]
    Spark,
    /// Liquid wallet — advanced wallet for L-BTC, USDt, and
    /// Liquid-specific receive flows.
    Liquid,
}

/// Direction of a payment from the wallet's point of view.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DomainPaymentDirection {
    Send,
    Receive,
}

/// Composite status of a payment, mirroring the states the UI distinguishes.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DomainPaymentStatus {
    Created,
    Pending,
    Complete,
    Failed,
    TimedOut,
    Refundable,
    RefundPending,
    WaitingFeeAcceptance,
}

impl DomainPaymentStatus {
    /// `true` for states that show as destructive (red) in the UI.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Failed | Self::TimedOut | Self::Refundable)
    }

    /// `true` for in-flight states that should not contribute to confirmed balance.
    pub fn is_in_flight(self) -> bool {
        matches!(
            self,
            Self::Created | Self::Pending | Self::RefundPending | Self::WaitingFeeAcceptance
        )
    }
}

/// Liquid-only asset info carried on `DomainPaymentDetails::LiquidAsset`.
///
/// Amounts are carried as base units (`amount_minor`) so the UI doesn't have to
/// re-derive them from the SDK's `f64` field.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainLiquidAssetInfo {
    pub amount_minor: u64,
    pub precision: u8,
}

/// Payment-type-specific details carried by a [`DomainPayment`].
///
/// Only fields actually read by the UI are modeled here. Additional SDK fields
/// can be added as needs arise.
#[derive(Debug, Clone, PartialEq)]
pub enum DomainPaymentDetails {
    /// A Lightning payment (BOLT11 / BOLT12 / LNURL).
    Lightning {
        description: String,
        payer_note: Option<String>,
    },
    /// A direct on-chain Liquid payment, possibly for a non-L-BTC asset.
    LiquidAsset {
        asset_id: String,
        asset_info: Option<DomainLiquidAssetInfo>,
        description: String,
        payer_note: Option<String>,
    },
    /// A swap to or from the Bitcoin chain (Liquid backend: boltz-style swap).
    OnChainBitcoin {
        swap_id: Option<String>,
        bitcoin_address: Option<String>,
        description: String,
        auto_accepted_fees: bool,
        liquid_expiration_blockheight: u32,
        bitcoin_expiration_blockheight: u32,
        lockup_tx_id: Option<String>,
        claim_tx_id: Option<String>,
        refund_tx_id: Option<String>,
        refund_tx_amount_sat: Option<u64>,
    },
}

impl DomainPaymentDetails {
    /// Best-effort human description for the payment, preferring the payer note
    /// over the invoice description.
    pub fn description(&self) -> &str {
        match self {
            Self::Lightning {
                description,
                payer_note,
            }
            | Self::LiquidAsset {
                description,
                payer_note,
                ..
            } => payer_note
                .as_deref()
                .filter(|s| !s.is_empty())
                .unwrap_or(description),
            Self::OnChainBitcoin { description, .. } => description,
        }
    }
}

/// A payment presented to the UI, decoupled from any SDK-specific type.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainPayment {
    pub tx_id: Option<String>,
    pub destination: Option<String>,
    pub timestamp: u32,
    pub amount_sat: u64,
    pub fees_sat: u64,
    pub direction: DomainPaymentDirection,
    pub status: DomainPaymentStatus,
    pub details: DomainPaymentDetails,
}

impl DomainPayment {
    pub fn is_incoming(&self) -> bool {
        matches!(self.direction, DomainPaymentDirection::Receive)
    }
}

/// A refundable swap surfaced by the backend's read API.
#[derive(Debug, Clone, PartialEq)]
pub struct DomainRefundableSwap {
    pub swap_address: String,
    pub timestamp: u32,
    pub amount_sat: u64,
}

// ---------------------------------------------------------------------------
// Mapping from breez_sdk_liquid types into domain types.
// ---------------------------------------------------------------------------

impl From<LiquidPaymentState> for DomainPaymentStatus {
    fn from(s: LiquidPaymentState) -> Self {
        match s {
            LiquidPaymentState::Created => Self::Created,
            LiquidPaymentState::Pending => Self::Pending,
            LiquidPaymentState::Complete => Self::Complete,
            LiquidPaymentState::Failed => Self::Failed,
            LiquidPaymentState::TimedOut => Self::TimedOut,
            LiquidPaymentState::Refundable => Self::Refundable,
            LiquidPaymentState::RefundPending => Self::RefundPending,
            LiquidPaymentState::WaitingFeeAcceptance => Self::WaitingFeeAcceptance,
        }
    }
}

impl From<LiquidPaymentType> for DomainPaymentDirection {
    fn from(t: LiquidPaymentType) -> Self {
        match t {
            LiquidPaymentType::Send => Self::Send,
            LiquidPaymentType::Receive => Self::Receive,
        }
    }
}

fn map_liquid_details(details: LiquidPaymentDetails) -> DomainPaymentDetails {
    match details {
        LiquidPaymentDetails::Lightning {
            description,
            payer_note,
            ..
        } => DomainPaymentDetails::Lightning {
            description,
            payer_note,
        },
        LiquidPaymentDetails::Liquid {
            asset_id,
            asset_info,
            description,
            payer_note,
            ..
        } => {
            let asset_info = asset_info.map(|info| {
                // The SDK exposes `amount` as an f64 already shifted by the asset
                // precision. The UI uses `USDT_PRECISION` for the only non-L-BTC
                // asset we currently support — `usdt_amount_to_minor` centralises
                // the conversion formula.
                DomainLiquidAssetInfo {
                    amount_minor: crate::app::breez_liquid::assets::usdt_amount_to_minor(
                        info.amount,
                    ),
                    precision: USDT_PRECISION,
                }
            });
            DomainPaymentDetails::LiquidAsset {
                asset_id,
                asset_info,
                description,
                payer_note,
            }
        }
        LiquidPaymentDetails::Bitcoin {
            swap_id,
            bitcoin_address,
            description,
            auto_accepted_fees,
            liquid_expiration_blockheight,
            bitcoin_expiration_blockheight,
            lockup_tx_id,
            claim_tx_id,
            refund_tx_id,
            refund_tx_amount_sat,
        } => DomainPaymentDetails::OnChainBitcoin {
            swap_id: Some(swap_id),
            bitcoin_address: Some(bitcoin_address),
            description,
            auto_accepted_fees,
            liquid_expiration_blockheight,
            bitcoin_expiration_blockheight,
            lockup_tx_id,
            claim_tx_id,
            refund_tx_id,
            refund_tx_amount_sat,
        },
    }
}

impl From<LiquidPayment> for DomainPayment {
    fn from(p: LiquidPayment) -> Self {
        Self {
            tx_id: p.tx_id,
            destination: p.destination,
            timestamp: p.timestamp,
            amount_sat: p.amount_sat,
            fees_sat: p.fees_sat,
            direction: p.payment_type.into(),
            status: p.status.into(),
            details: map_liquid_details(p.details),
        }
    }
}

impl From<LiquidRefundableSwap> for DomainRefundableSwap {
    fn from(r: LiquidRefundableSwap) -> Self {
        Self {
            swap_address: r.swap_address,
            timestamp: r.timestamp,
            amount_sat: r.amount_sat,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use breez_sdk_liquid::model::{
        AssetInfo as LiquidAssetInfo, PaymentDetails as LiquidPaymentDetails,
        PaymentState as LiquidPaymentState, PaymentType as LiquidPaymentType,
    };
    use breez_sdk_liquid::prelude::{
        Payment as LiquidPayment, RefundableSwap as LiquidRefundableSwap,
    };

    fn liquid_payment_with(details: LiquidPaymentDetails) -> LiquidPayment {
        LiquidPayment {
            destination: Some("destination".to_string()),
            tx_id: Some("txid".to_string()),
            unblinding_data: Some("unblind".to_string()),
            timestamp: 1_722_000_000,
            amount_sat: 25_000,
            fees_sat: 125,
            swapper_fees_sat: Some(50),
            payment_type: LiquidPaymentType::Receive,
            status: LiquidPaymentState::Pending,
            details,
        }
    }

    #[test]
    fn wallet_kind_defaults_to_spark_and_serializes_as_snake_case() {
        assert_eq!(WalletKind::default(), WalletKind::Spark);
        assert_eq!(
            serde_json::to_string(&WalletKind::Spark).unwrap(),
            "\"spark\""
        );
        assert_eq!(
            serde_json::to_string(&WalletKind::Liquid).unwrap(),
            "\"liquid\""
        );

        let decoded: WalletKind = serde_json::from_str("\"liquid\"").unwrap();
        assert_eq!(decoded, WalletKind::Liquid);
    }

    #[test]
    fn payment_status_predicates_match_ui_state_groups() {
        for status in [
            DomainPaymentStatus::Failed,
            DomainPaymentStatus::TimedOut,
            DomainPaymentStatus::Refundable,
        ] {
            assert!(status.is_destructive());
        }

        for status in [
            DomainPaymentStatus::Created,
            DomainPaymentStatus::Pending,
            DomainPaymentStatus::RefundPending,
            DomainPaymentStatus::WaitingFeeAcceptance,
        ] {
            assert!(status.is_in_flight());
        }

        assert!(!DomainPaymentStatus::Complete.is_destructive());
        assert!(!DomainPaymentStatus::Complete.is_in_flight());
    }

    #[test]
    fn liquid_payment_states_map_to_domain_statuses() {
        let cases = [
            (LiquidPaymentState::Created, DomainPaymentStatus::Created),
            (LiquidPaymentState::Pending, DomainPaymentStatus::Pending),
            (LiquidPaymentState::Complete, DomainPaymentStatus::Complete),
            (LiquidPaymentState::Failed, DomainPaymentStatus::Failed),
            (LiquidPaymentState::TimedOut, DomainPaymentStatus::TimedOut),
            (
                LiquidPaymentState::Refundable,
                DomainPaymentStatus::Refundable,
            ),
            (
                LiquidPaymentState::RefundPending,
                DomainPaymentStatus::RefundPending,
            ),
            (
                LiquidPaymentState::WaitingFeeAcceptance,
                DomainPaymentStatus::WaitingFeeAcceptance,
            ),
        ];

        for (sdk_status, expected) in cases {
            assert_eq!(DomainPaymentStatus::from(sdk_status), expected);
        }
    }

    #[test]
    fn payment_details_description_prefers_non_empty_payer_note() {
        let lightning = DomainPaymentDetails::Lightning {
            description: "invoice description".to_string(),
            payer_note: Some("payer note".to_string()),
        };
        assert_eq!(lightning.description(), "payer note");

        let liquid = DomainPaymentDetails::LiquidAsset {
            asset_id: "asset".to_string(),
            asset_info: None,
            description: "asset description".to_string(),
            payer_note: Some(String::new()),
        };
        assert_eq!(liquid.description(), "asset description");

        let bitcoin = DomainPaymentDetails::OnChainBitcoin {
            swap_id: None,
            bitcoin_address: None,
            description: "bitcoin description".to_string(),
            auto_accepted_fees: false,
            liquid_expiration_blockheight: 0,
            bitcoin_expiration_blockheight: 0,
            lockup_tx_id: None,
            claim_tx_id: None,
            refund_tx_id: None,
            refund_tx_amount_sat: None,
        };
        assert_eq!(bitcoin.description(), "bitcoin description");
    }

    #[test]
    fn domain_payment_is_incoming_only_for_receives() {
        let mut payment = DomainPayment {
            tx_id: None,
            destination: None,
            timestamp: 0,
            amount_sat: 0,
            fees_sat: 0,
            direction: DomainPaymentDirection::Receive,
            status: DomainPaymentStatus::Complete,
            details: DomainPaymentDetails::Lightning {
                description: String::new(),
                payer_note: None,
            },
        };

        assert!(payment.is_incoming());
        payment.direction = DomainPaymentDirection::Send;
        assert!(!payment.is_incoming());
    }

    #[test]
    fn liquid_lightning_payment_maps_to_domain_payment() {
        let sdk_payment = liquid_payment_with(LiquidPaymentDetails::Lightning {
            swap_id: "swap".to_string(),
            description: "invoice".to_string(),
            liquid_expiration_blockheight: 100,
            preimage: Some("preimage".to_string()),
            invoice: Some("lnbc".to_string()),
            bolt12_offer: None,
            payment_hash: Some("hash".to_string()),
            destination_pubkey: None,
            lnurl_info: None,
            bip353_address: None,
            payer_note: Some("thanks".to_string()),
            claim_tx_id: None,
            refund_tx_id: Some("refund".to_string()),
            refund_tx_amount_sat: Some(10),
            settled_at: None,
        });

        let domain = DomainPayment::from(sdk_payment);

        assert_eq!(domain.tx_id.as_deref(), Some("txid"));
        assert_eq!(domain.destination.as_deref(), Some("destination"));
        assert_eq!(domain.timestamp, 1_722_000_000);
        assert_eq!(domain.amount_sat, 25_000);
        assert_eq!(domain.fees_sat, 125);
        assert_eq!(domain.direction, DomainPaymentDirection::Receive);
        assert_eq!(domain.status, DomainPaymentStatus::Pending);
        assert_eq!(
            domain.details,
            DomainPaymentDetails::Lightning {
                description: "invoice".to_string(),
                payer_note: Some("thanks".to_string()),
            }
        );
    }

    #[test]
    fn liquid_asset_payment_maps_asset_amount_to_minor_units() {
        let sdk_payment = liquid_payment_with(LiquidPaymentDetails::Liquid {
            destination: "liquid-address".to_string(),
            description: "asset transfer".to_string(),
            asset_id: "usdt-asset".to_string(),
            asset_info: Some(LiquidAssetInfo {
                name: "Tether USD".to_string(),
                ticker: "USDt".to_string(),
                amount: 1.23456789,
                fees: Some(0.00000001),
            }),
            lnurl_info: None,
            bip353_address: None,
            payer_note: Some("asset note".to_string()),
        });

        let domain = DomainPayment::from(sdk_payment);

        assert_eq!(
            domain.details,
            DomainPaymentDetails::LiquidAsset {
                asset_id: "usdt-asset".to_string(),
                asset_info: Some(DomainLiquidAssetInfo {
                    amount_minor: 123_456_789,
                    precision: USDT_PRECISION,
                }),
                description: "asset transfer".to_string(),
                payer_note: Some("asset note".to_string()),
            }
        );
    }

    #[test]
    fn liquid_bitcoin_payment_maps_swap_fields_to_domain_details() {
        let sdk_payment = liquid_payment_with(LiquidPaymentDetails::Bitcoin {
            swap_id: "swap-id".to_string(),
            bitcoin_address: "bc1qaddress".to_string(),
            description: "chain swap".to_string(),
            auto_accepted_fees: true,
            liquid_expiration_blockheight: 123,
            bitcoin_expiration_blockheight: 456,
            lockup_tx_id: Some("lockup".to_string()),
            claim_tx_id: Some("claim".to_string()),
            refund_tx_id: Some("refund".to_string()),
            refund_tx_amount_sat: Some(900),
        });

        let domain = DomainPayment::from(sdk_payment);

        assert_eq!(
            domain.details,
            DomainPaymentDetails::OnChainBitcoin {
                swap_id: Some("swap-id".to_string()),
                bitcoin_address: Some("bc1qaddress".to_string()),
                description: "chain swap".to_string(),
                auto_accepted_fees: true,
                liquid_expiration_blockheight: 123,
                bitcoin_expiration_blockheight: 456,
                lockup_tx_id: Some("lockup".to_string()),
                claim_tx_id: Some("claim".to_string()),
                refund_tx_id: Some("refund".to_string()),
                refund_tx_amount_sat: Some(900),
            }
        );
    }

    #[test]
    fn refundable_swap_maps_only_ui_fields() {
        let domain = DomainRefundableSwap::from(LiquidRefundableSwap {
            swap_address: "swap-address".to_string(),
            timestamp: 1_722_000_001,
            amount_sat: 42_000,
            last_refund_tx_id: Some("ignored".to_string()),
        });

        assert_eq!(
            domain,
            DomainRefundableSwap {
                swap_address: "swap-address".to_string(),
                timestamp: 1_722_000_001,
                amount_sat: 42_000,
            }
        );
    }
}
