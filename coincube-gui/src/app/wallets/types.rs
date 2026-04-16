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
///
/// Default is [`Self::Spark`] as of Phase 5 — new cubes route incoming
/// Lightning Address invoices through the Spark bridge. Existing cubes
/// that previously deserialized with Liquid default keep their
/// explicit value (serde only applies the default when the field is
/// absent entirely). Users can override per-cube in Spark Settings.
#[derive(
    Debug, Clone, Copy, Default, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum WalletKind {
    /// Spark wallet — default for everyday Lightning UX and the
    /// Phase 5 routing target for incoming Lightning Address invoices.
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
                // precision. Convert back to minor units (same formula the UI used
                // before the refactor). The UI uses `USDT_PRECISION` for the only
                // non-L-BTC asset we currently support.
                let precision = USDT_PRECISION;
                let scale = 10_f64.powi(precision as i32);
                let amount_minor = (info.amount * scale).round() as u64;
                DomainLiquidAssetInfo {
                    amount_minor,
                    precision,
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
