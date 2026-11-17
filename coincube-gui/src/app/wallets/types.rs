//! Domain types for wallet backends.
//!
//! These types decouple the panels from SDK-specific payment representations.
//! Each wallet backend maps its native types into these domain types at the
//! boundary.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
