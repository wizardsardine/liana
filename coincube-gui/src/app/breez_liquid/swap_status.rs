//! BTC → L-BTC swap receive status model.
//!
//! The Breez Liquid SDK exposes low-level `PaymentState` and `SdkEvent` values
//! that the GUI must translate into user-friendly lifecycle stages. Classifying
//! those states in one place lets the receive and transactions views ask a
//! single question — "what should I show for this swap?" — rather than
//! re-deriving the answer from raw SDK fields in every screen.
//!
//! This module is scoped to the **Liquid wallet's BTC onchain receive** path
//! (which is a Boltz-style swap from native BTC to L-BTC). The Vault wallet is
//! natively Bitcoin and does not use this model.

use breez_sdk_liquid::prelude::RefundableSwap;

use crate::app::wallets::{
    DomainPayment, DomainPaymentDetails, DomainPaymentDirection, DomainPaymentStatus,
};

/// Lifecycle stage of a single BTC→L-BTC swap receive, as surfaced in the
/// Liquid wallet's receive flow.
///
/// This is a UI-facing projection of `PaymentState` combined with the presence
/// of the swap in `list_refundables()`. The mapping lives in
/// [`classify_payment`] / [`classify_refundable`] — extend those functions when
/// the SDK adds new states or events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BtcSwapReceiveStatus {
    /// Address has been generated, no BTC deposit has been seen on chain yet.
    AwaitingDeposit,
    /// The BTC lockup tx has been seen in the mempool, waiting on on-chain
    /// confirmations before the swap can progress.
    PendingConfirmation,
    /// BTC is confirmed on chain; the Boltz swap to L-BTC is in flight and we
    /// are waiting for the claim tx.
    PendingSwapCompletion,
    /// The swapper proposed new fees that must be accepted before the payment
    /// can proceed. User action is required.
    WaitingFeeAcceptance,
    /// The swap failed or timed out and the funds can be recovered via an
    /// in-app BTC refund transaction. Appears in `list_refundables()`.
    Refundable,
    /// The user has submitted a refund; the refund tx is being broadcast but
    /// not yet confirmed.
    Refunding,
    /// The refund tx has been confirmed.
    Refunded,
    /// The swap settled and L-BTC has been credited to the Liquid wallet.
    Completed,
    /// Terminal failure with no refundable funds (e.g. timed out before the
    /// lockup tx was broadcast).
    Failed,
}

impl BtcSwapReceiveStatus {
    /// Short, user-facing label for this status.
    pub fn label(&self) -> &'static str {
        match self {
            Self::AwaitingDeposit => "Awaiting deposit",
            Self::PendingConfirmation => "Pending confirmation",
            Self::PendingSwapCompletion => "Swap in progress",
            Self::WaitingFeeAcceptance => "Fee review needed",
            Self::Refundable => "Refundable",
            Self::Refunding => "Refund broadcasting",
            Self::Refunded => "Refunded",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }
}

/// Classify a `Payment` that originated from a BTC onchain receive swap.
///
/// Mapping (SDK → internal):
/// - `Created` → `AwaitingDeposit`
/// - `Pending` (no lockup tx seen) → `PendingConfirmation`
/// - `Pending` (lockup tx seen) → `PendingSwapCompletion`
/// - `WaitingFeeAcceptance` → `WaitingFeeAcceptance`
/// - `Refundable` → `Refundable`
/// - `RefundPending` → `Refunding`
/// - `Failed` → `Refundable` if the swap still appears in `list_refundables`,
///   otherwise `Failed`. Since that fact lives outside a single `Payment`,
///   callers pass `refundable_swap_addresses` to resolve it.
/// - `TimedOut` → `Failed`
/// - `Complete` (receive) → `Completed`
/// - `Complete` (send, `refund_tx_id` set — the refund leg) → `Refunded`
/// - `Complete` (send, no `refund_tx_id` — an L-BTC→BTC chain swap send,
///   which shares `PaymentDetails::Bitcoin` but is NOT a receive swap) →
///   `Completed`. This enum is scoped to the receive path, so the best we
///   can report for a successful outgoing chain swap is "completed" rather
///   than misreporting it as a refund.
///
/// `refundable_swap_addresses` is the set of `swap_address` strings currently
/// returned by `BreezClient::list_refundables()`. Pass an empty slice when
/// that context is unavailable (the classifier will fall back to `Failed`).
pub fn classify_payment(
    p: &DomainPayment,
    refundable_swap_addresses: &[String],
) -> BtcSwapReceiveStatus {
    let bitcoin_addr = match &p.details {
        DomainPaymentDetails::OnChainBitcoin {
            bitcoin_address,
            lockup_tx_id,
            ..
        } => Some((bitcoin_address.clone(), lockup_tx_id.is_some())),
        _ => None,
    };

    match p.status {
        DomainPaymentStatus::Created => BtcSwapReceiveStatus::AwaitingDeposit,
        DomainPaymentStatus::Pending => match bitcoin_addr {
            Some((_, lockup_seen)) if lockup_seen => BtcSwapReceiveStatus::PendingSwapCompletion,
            _ => BtcSwapReceiveStatus::PendingConfirmation,
        },
        DomainPaymentStatus::WaitingFeeAcceptance => BtcSwapReceiveStatus::WaitingFeeAcceptance,
        DomainPaymentStatus::Refundable => BtcSwapReceiveStatus::Refundable,
        DomainPaymentStatus::RefundPending => BtcSwapReceiveStatus::Refunding,
        DomainPaymentStatus::Failed => {
            if let Some((Some(addr), _)) = bitcoin_addr {
                if refundable_swap_addresses.contains(&addr) {
                    return BtcSwapReceiveStatus::Refundable;
                }
            }
            BtcSwapReceiveStatus::Failed
        }
        DomainPaymentStatus::TimedOut => BtcSwapReceiveStatus::Failed,
        DomainPaymentStatus::Complete => match p.direction {
            DomainPaymentDirection::Receive => BtcSwapReceiveStatus::Completed,
            // A Complete send with `refund_tx_id` set is the refund leg of a
            // failed BTC→L-BTC receive swap. A Complete send *without* a
            // refund txid is an L-BTC→BTC chain swap send (a different swap
            // direction that happens to share DomainPaymentDetails::OnChainBitcoin) — it
            // is a successful outgoing payment, not a refund.
            DomainPaymentDirection::Send => {
                let is_refund_leg = matches!(
                    &p.details,
                    DomainPaymentDetails::OnChainBitcoin {
                        refund_tx_id: Some(_),
                        ..
                    }
                );
                if is_refund_leg {
                    BtcSwapReceiveStatus::Refunded
                } else {
                    BtcSwapReceiveStatus::Completed
                }
            }
        },
    }
}

/// A `RefundableSwap` coming back from `list_refundables()` is always in the
/// refundable state — this is the obvious classifier, kept as a function so
/// callers can treat refundables and payments uniformly.
pub fn classify_refundable(_swap: &RefundableSwap) -> BtcSwapReceiveStatus {
    BtcSwapReceiveStatus::Refundable
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::wallets::DomainPaymentDirection;

    fn btc_payment(
        status: DomainPaymentStatus,
        direction: DomainPaymentDirection,
        lockup: Option<String>,
    ) -> DomainPayment {
        btc_payment_with_refund(status, direction, lockup, None)
    }

    fn btc_payment_with_refund(
        status: DomainPaymentStatus,
        direction: DomainPaymentDirection,
        lockup: Option<String>,
        refund_tx_id: Option<String>,
    ) -> DomainPayment {
        DomainPayment {
            destination: Some("bc1qtest".into()),
            tx_id: None,
            timestamp: 0,
            amount_sat: 50_000,
            fees_sat: 0,
            direction,
            status,
            details: DomainPaymentDetails::OnChainBitcoin {
                swap_id: Some("swap-xyz".into()),
                bitcoin_address: Some("bc1qtest".into()),
                description: String::new(),
                auto_accepted_fees: false,
                liquid_expiration_blockheight: 0,
                bitcoin_expiration_blockheight: 0,
                lockup_tx_id: lockup,
                claim_tx_id: None,
                refund_tx_id,
                refund_tx_amount_sat: None,
            },
        }
    }

    #[test]
    fn created_maps_to_awaiting_deposit() {
        let p = btc_payment(
            DomainPaymentStatus::Created,
            DomainPaymentDirection::Receive,
            None,
        );
        assert_eq!(
            classify_payment(&p, &[]),
            BtcSwapReceiveStatus::AwaitingDeposit
        );
    }

    #[test]
    fn pending_without_lockup_is_pending_confirmation() {
        let p = btc_payment(
            DomainPaymentStatus::Pending,
            DomainPaymentDirection::Receive,
            None,
        );
        assert_eq!(
            classify_payment(&p, &[]),
            BtcSwapReceiveStatus::PendingConfirmation
        );
    }

    #[test]
    fn pending_with_lockup_is_pending_swap_completion() {
        let p = btc_payment(
            DomainPaymentStatus::Pending,
            DomainPaymentDirection::Receive,
            Some("txid".into()),
        );
        assert_eq!(
            classify_payment(&p, &[]),
            BtcSwapReceiveStatus::PendingSwapCompletion
        );
    }

    #[test]
    fn refundable_state_is_refundable() {
        let p = btc_payment(
            DomainPaymentStatus::Refundable,
            DomainPaymentDirection::Receive,
            None,
        );
        assert_eq!(classify_payment(&p, &[]), BtcSwapReceiveStatus::Refundable);
    }

    #[test]
    fn refund_pending_is_refunding() {
        let p = btc_payment(
            DomainPaymentStatus::RefundPending,
            DomainPaymentDirection::Send,
            None,
        );
        assert_eq!(classify_payment(&p, &[]), BtcSwapReceiveStatus::Refunding);
    }

    #[test]
    fn failed_with_refundable_address_upgrades_to_refundable() {
        let p = btc_payment(
            DomainPaymentStatus::Failed,
            DomainPaymentDirection::Receive,
            None,
        );
        let refundables = vec!["bc1qtest".to_string()];
        assert_eq!(
            classify_payment(&p, &refundables),
            BtcSwapReceiveStatus::Refundable
        );
    }

    #[test]
    fn failed_without_refundable_address_is_failed() {
        let p = btc_payment(
            DomainPaymentStatus::Failed,
            DomainPaymentDirection::Receive,
            None,
        );
        assert_eq!(classify_payment(&p, &[]), BtcSwapReceiveStatus::Failed);
    }

    #[test]
    fn timed_out_is_failed() {
        let p = btc_payment(
            DomainPaymentStatus::TimedOut,
            DomainPaymentDirection::Receive,
            None,
        );
        assert_eq!(classify_payment(&p, &[]), BtcSwapReceiveStatus::Failed);
    }

    #[test]
    fn complete_receive_is_completed() {
        let p = btc_payment(
            DomainPaymentStatus::Complete,
            DomainPaymentDirection::Receive,
            None,
        );
        assert_eq!(classify_payment(&p, &[]), BtcSwapReceiveStatus::Completed);
    }

    #[test]
    fn complete_send_with_refund_tx_is_refunded() {
        // The refund leg of a failed BTC→L-BTC receive swap.
        let p = btc_payment_with_refund(
            DomainPaymentStatus::Complete,
            DomainPaymentDirection::Send,
            None,
            Some("refund-txid".into()),
        );
        assert_eq!(classify_payment(&p, &[]), BtcSwapReceiveStatus::Refunded);
    }

    #[test]
    fn complete_send_without_refund_tx_is_completed() {
        // Regression: an L-BTC→BTC chain swap send also uses
        // DomainPaymentDetails::OnChainBitcoin + Send direction, but is NOT a refund.
        // Previously classify_payment reported it as Refunded.
        let p = btc_payment(
            DomainPaymentStatus::Complete,
            DomainPaymentDirection::Send,
            None,
        );
        assert_eq!(classify_payment(&p, &[]), BtcSwapReceiveStatus::Completed);
    }

    #[test]
    fn waiting_fee_acceptance_maps() {
        let p = btc_payment(
            DomainPaymentStatus::WaitingFeeAcceptance,
            DomainPaymentDirection::Receive,
            None,
        );
        assert_eq!(
            classify_payment(&p, &[]),
            BtcSwapReceiveStatus::WaitingFeeAcceptance
        );
    }

    #[test]
    fn classify_refundable_always_refundable() {
        let r = RefundableSwap {
            swap_address: "bc1q".into(),
            timestamp: 0,
            amount_sat: 10_000,
            last_refund_tx_id: None,
        };
        assert_eq!(classify_refundable(&r), BtcSwapReceiveStatus::Refundable);
    }
}
