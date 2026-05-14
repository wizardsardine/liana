use std::fmt;

/// Errors produced by the Cube Recovery Kit envelope codec.
///
/// `BadPasswordOrCorrupt` intentionally collapses "wrong password" and
/// "ciphertext mutated" into a single case — distinguishing them would
/// leak a timing/oracle signal to an offline bruteforcer of the recovery
/// password. See `PLAN-cube-recovery-kit-desktop.md` §2.1.
#[derive(Debug)]
pub enum RecoveryError {
    /// Argon2id KDF failed to produce a key (bad params, OOM, etc).
    Kdf(argon2::Error),
    /// AES-GCM key init refused the key slice (should never happen at 32 bytes).
    Cipher(aes_gcm::aes::cipher::InvalidLength),
    /// AES-GCM seal/unseal returned an opaque error. For unseal this is
    /// reported as `BadPasswordOrCorrupt` instead.
    Seal,
    /// Decrypt failed: either the password is wrong or the ciphertext /
    /// AAD bytes have been tampered with.
    BadPasswordOrCorrupt,
    /// Base64 decoding of the outer envelope failed.
    Base64(base64::DecodeError),
    /// Envelope byte buffer is shorter than the fixed header + salt + nonce
    /// + tag floor — the sender truncated or mis-encoded it.
    Truncated,
    /// Envelope header declares a version or kdf_id this client does not
    /// know how to process. Future-proofs the wire format.
    Unsupported { version: u8, kdf_id: u8 },
    /// Argon2 `Params` rejected the per-envelope cost parameters (e.g. a
    /// zero memory cost pulled from a malformed header). Separate from
    /// `Kdf` so the UI can distinguish "your params don't load" from
    /// "hashing ran but failed halfway".
    InvalidParams(argon2::Error),
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Kdf(e) => write!(f, "key derivation failed: {}", e),
            Self::Cipher(e) => write!(f, "AES-GCM key init failed: {}", e),
            Self::Seal => write!(f, "AES-GCM seal failed"),
            Self::BadPasswordOrCorrupt => write!(
                f,
                "recovery password is incorrect or the backup is corrupted"
            ),
            Self::Base64(e) => write!(f, "base64 decode failed: {}", e),
            Self::Truncated => write!(f, "recovery envelope is truncated"),
            Self::Unsupported { version, kdf_id } => write!(
                f,
                "recovery envelope version={} kdf_id={} is not supported by this client",
                version, kdf_id
            ),
            Self::InvalidParams(e) => write!(f, "invalid KDF params in envelope: {}", e),
        }
    }
}

impl std::error::Error for RecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kdf(e) | Self::InvalidParams(e) => Some(e),
            Self::Cipher(e) => Some(e),
            Self::Base64(e) => Some(e),
            _ => None,
        }
    }
}

impl From<base64::DecodeError> for RecoveryError {
    fn from(e: base64::DecodeError) -> Self {
        Self::Base64(e)
    }
}
