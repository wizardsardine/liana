//! Errors for the inheritance ECIES codec.
//!
//! `BadKeyOrCorrupt` deliberately collapses "wrong key", "tampered
//! ciphertext", and "tampered AAD" into one case — distinguishing them would
//! hand an oracle to anyone probing the gated release endpoint. Mirrors the
//! Cube Recovery Kit's `RecoveryError::BadPasswordOrCorrupt`.

use std::fmt;

#[derive(Debug)]
pub enum EciesError {
    /// Deriving the dedicated encryption child from the keyholder xpub failed
    /// (e.g. a hardened step in the derivation, which an xpub cannot walk).
    Derivation(coincube_core::miniscript::bitcoin::bip32::Error),
    /// AES-GCM key init refused the key slice (never happens at 32 bytes).
    Cipher(aes_gcm::aes::cipher::InvalidLength),
    /// AES-GCM seal returned an opaque error.
    Seal,
    /// Open failed: wrong key, tampered ciphertext, or tampered AAD —
    /// indistinguishable by design.
    BadKeyOrCorrupt,
    /// The envelope's `scheme` isn't one this client understands.
    UnsupportedScheme(String),
    /// A structural field (nonce / ephemeral pubkey / ciphertext length) is
    /// the wrong size — a truncated or mis-encoded envelope. The `&str` names
    /// the offending field for logs (non-sensitive).
    MalformedEnvelope(&'static str),
}

impl fmt::Display for EciesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Derivation(e) => write!(f, "encryption-child derivation failed: {}", e),
            Self::Cipher(e) => write!(f, "AES-GCM key init failed: {}", e),
            Self::Seal => write!(f, "AES-GCM seal failed"),
            Self::BadKeyOrCorrupt => {
                write!(
                    f,
                    "recovery envelope could not be decrypted (wrong key or corrupted)"
                )
            }
            Self::UnsupportedScheme(s) => {
                write!(
                    f,
                    "recovery envelope scheme '{}' is not supported by this client",
                    s
                )
            }
            Self::MalformedEnvelope(field) => {
                write!(f, "recovery envelope is malformed: {}", field)
            }
        }
    }
}

impl std::error::Error for EciesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Derivation(e) => Some(e),
            Self::Cipher(e) => Some(e),
            _ => None,
        }
    }
}
