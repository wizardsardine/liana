use std::fmt;

/// Errors that can occur during Border Wallet operations.
#[derive(Debug)]
pub enum BorderWalletError {
    /// The recovery phrase is not a valid 12-word BIP39 mnemonic.
    InvalidRecoveryPhrase,
    /// The pattern does not contain exactly 11 cells.
    InvalidPatternLength(usize),
    /// A cell reference is out of bounds for the grid.
    CellOutOfBounds { row: u16, col: u8 },
    /// The pattern contains duplicate cell selections.
    DuplicateCell { row: u16, col: u8 },
    /// Failed to construct a valid BIP39 mnemonic from the pattern.
    MnemonicConstruction(String),
    /// BIP32 key derivation failed.
    KeyDerivation(String),
    /// Reconstructed fingerprint does not match the expected enrollment fingerprint.
    FingerprintMismatch {
        expected: crate::miniscript::bitcoin::bip32::Fingerprint,
        got: crate::miniscript::bitcoin::bip32::Fingerprint,
    },
    /// PSBT signing failed.
    SigningFailed(String),
}

impl fmt::Display for BorderWalletError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRecoveryPhrase => {
                write!(
                    f,
                    "invalid recovery phrase: must be a valid 12-word BIP39 mnemonic"
                )
            }
            Self::InvalidPatternLength(n) => {
                write!(f, "invalid pattern length: expected 11 cells, got {}", n)
            }
            Self::CellOutOfBounds { row, col } => {
                write!(f, "cell out of bounds: row={}, col={}", row, col)
            }
            Self::DuplicateCell { row, col } => {
                write!(f, "duplicate cell in pattern: row={}, col={}", row, col)
            }
            Self::MnemonicConstruction(msg) => {
                write!(f, "mnemonic construction failed: {}", msg)
            }
            Self::KeyDerivation(msg) => {
                write!(f, "key derivation failed: {}", msg)
            }
            Self::FingerprintMismatch { expected, got } => {
                write!(
                    f,
                    "fingerprint mismatch: expected {}, got {}",
                    expected, got
                )
            }
            Self::SigningFailed(msg) => {
                write!(f, "PSBT signing failed: {}", msg)
            }
        }
    }
}

impl std::error::Error for BorderWalletError {}
