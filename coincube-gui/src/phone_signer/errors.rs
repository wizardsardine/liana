//! Typed pairing-failure categories.
//!
//! The pairing wizard's UI keys off these variants to render
//! category-specific copy (and, where applicable, a Try-Again
//! button) instead of dumping a raw error string at the user.
//!
//! Steady-state signer-list reachability uses
//! [`crate::hw::UnsupportedReason`] (`AppIsNotOpen`) — that path
//! covers "paired but offline / on different Wi-Fi" with a single
//! catch-all because the desktop can't usefully distinguish those
//! two cases from outside.

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

#[derive(Debug, Clone)]
pub enum PairingError {
    /// The offer's TTL elapsed before any phone connected. The user's
    /// best move is to start a fresh offer.
    OfferExpired,

    /// The phone reported a wallet fingerprint that isn't part of
    /// this wallet. Currently unreachable in practice (the offer's
    /// `wallet_fingerprint` is what gets validated, and that's
    /// always one of the local wallet's keys), but the variant is
    /// wired so it's ready when the wire format grows a
    /// phone-reported fingerprint.
    WalletFingerprintMismatch {
        expected: Vec<Fingerprint>,
        claimed: Fingerprint,
    },

    /// A second pairing attempt arrived after one had already
    /// completed in this listener window, or the phone failed a
    /// PSK-based proof-of-QR-scan check. Placeholder until the
    /// proto carries a PSK HMAC field — today the listener accepts
    /// exactly one connection per offer, so this variant isn't
    /// produced.
    ReplayRefused,

    /// Anything below the application layer: socket bind failure,
    /// mDNS registration error, TLS handshake failure, framed-read
    /// error, frame size limit hit. The string is the underlying
    /// rustls / io / mdns-sd error suitable for surfacing verbatim.
    NetworkError(String),

    /// Fall-through for category-less bugs. Surfaced as "Pairing
    /// failed: <message>" without a Try-Again button.
    InternalError(String),
}

impl std::fmt::Display for PairingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OfferExpired => write!(f, "Pairing offer expired before any phone scanned it."),
            Self::WalletFingerprintMismatch { expected, claimed } => write!(
                f,
                "Phone reported wallet fingerprint {} but this wallet expects {}.",
                claimed,
                expected
                    .iter()
                    .map(|fp| fp.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::ReplayRefused => write!(
                f,
                "Pairing rejected: the QR code has already been used."
            ),
            Self::NetworkError(s) => write!(f, "Network error during pairing: {}", s),
            Self::InternalError(s) => write!(f, "Pairing failed: {}", s),
        }
    }
}

impl PairingError {
    /// Whether the wizard should show a Try-Again button. Variants
    /// that point at a user action ("scan a new QR") get the
    /// button; transient / catch-all errors do too. Replay refused
    /// does not — the user shouldn't retry the same QR.
    pub fn is_retriable(&self) -> bool {
        !matches!(self, Self::ReplayRefused)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_retriable_returns_true_for_every_variant_except_replay() {
        assert!(PairingError::OfferExpired.is_retriable());
        assert!(PairingError::NetworkError("x".into()).is_retriable());
        assert!(PairingError::InternalError("x".into()).is_retriable());
        assert!(PairingError::WalletFingerprintMismatch {
            expected: vec![Fingerprint::default()],
            claimed: Fingerprint::default(),
        }
        .is_retriable());
        assert!(!PairingError::ReplayRefused.is_retriable());
    }

    #[test]
    fn display_mentions_the_underlying_cause() {
        let s = PairingError::OfferExpired.to_string();
        assert!(s.to_lowercase().contains("expired"), "got: {}", s);

        let s = PairingError::NetworkError("read len: EOF".into()).to_string();
        assert!(s.contains("read len: EOF"), "got: {}", s);

        let s = PairingError::InternalError("rustls config".into()).to_string();
        assert!(s.contains("rustls config"), "got: {}", s);

        let s = PairingError::ReplayRefused.to_string();
        assert!(s.to_lowercase().contains("already"), "got: {}", s);

        let s = PairingError::WalletFingerprintMismatch {
            expected: vec![Fingerprint::default()],
            claimed: Fingerprint::default(),
        }
        .to_string();
        assert!(s.contains("00000000"), "got: {}", s);
    }
}
