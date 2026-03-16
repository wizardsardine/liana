//! Border Wallet signer for COINCUBE Vault delayed recovery.
//!
//! This module provides a clean-room implementation of a Border Wallet-style
//! deterministic word grid signer. It is designed exclusively for the Safety Net
//! (delayed recovery) path in a COINCUBE Vault.
//!
//! # Security Model
//!
//! - Secret material (mnemonic, seed, private keys) exists only transiently in memory
//! - Only non-secret enrollment data (fingerprint, xpub, derivation path) may be persisted
//! - Secret types implemented here are zeroized on `Drop` where possible;
//!   however, upstream types outside our control (notably `bip32::Xpriv`, which
//!   is `Copy`) may remain on the stack and cannot be guaranteed to be zeroized.
//!   Zeroization of such external types is therefore best-effort — we mitigate
//!   by confining their use to the smallest possible scope.
//! - No `Debug` impl for secret-bearing types
//!
//! # Usage
//!
//! 1. Generate a `GridRecoveryPhrase` (random 12-word BIP39 mnemonic)
//! 2. Generate a `WordGrid` from the recovery phrase
//! 3. Build an `OrderedPattern` by selecting 11 cells
//! 4. Derive the mnemonic and enrollment data
//! 5. Persist only the `BorderWalletEnrollment`; wipe everything else

pub mod error;
pub mod grid;
pub mod pattern;

pub use error::BorderWalletError;
pub use grid::WordGrid;
pub use pattern::{build_mnemonic, CellRef, OrderedPattern, PATTERN_LENGTH};

use miniscript::bitcoin::{
    bip32::{self, Fingerprint},
    psbt::Psbt,
    secp256k1, Network,
};
use zeroize::Zeroizing;

use crate::signer::HotSigner;

/// A secret-bearing grid recovery phrase.
///
/// This is a 12-word BIP39 mnemonic used to deterministically generate a Word Grid.
/// It must never be persisted, logged, or included in error messages.
///
/// Zeroizes the underlying string on drop.
pub struct GridRecoveryPhrase {
    phrase: Zeroizing<String>,
}

// No Debug impl — prevent accidental logging of secret.
impl GridRecoveryPhrase {
    /// Create a new `GridRecoveryPhrase` from a validated mnemonic string.
    ///
    /// Returns an error if the phrase is not a valid 12-word BIP39 mnemonic.
    pub fn from_phrase(phrase: &str) -> Result<Self, BorderWalletError> {
        let mnemonic = bip39::Mnemonic::parse_in(bip39::Language::English, phrase)
            .map_err(|_| BorderWalletError::InvalidRecoveryPhrase)?;
        if mnemonic.word_count() != 12 {
            return Err(BorderWalletError::InvalidRecoveryPhrase);
        }
        Ok(Self {
            phrase: Zeroizing::new(mnemonic.to_string()),
        })
    }

    /// Generate a new random recovery phrase using secure randomness.
    pub fn generate() -> Result<Self, BorderWalletError> {
        let mut entropy = Zeroizing::new([0u8; 16]); // 128 bits for 12-word mnemonic
        getrandom::fill(&mut *entropy).map_err(|_| BorderWalletError::InvalidRecoveryPhrase)?;
        let mnemonic = bip39::Mnemonic::from_entropy(&*entropy)
            .map_err(|_| BorderWalletError::InvalidRecoveryPhrase)?;
        Ok(Self {
            phrase: Zeroizing::new(mnemonic.to_string()),
        })
    }

    /// Access the phrase as a string slice. Use only for grid generation and display.
    pub fn as_str(&self) -> &str {
        &self.phrase
    }

    /// Generate the deterministic Word Grid from this recovery phrase.
    ///
    /// This cannot fail because the phrase was already validated on construction.
    pub fn generate_grid(&self) -> WordGrid {
        WordGrid::from_recovery_phrase(&self.phrase)
            .expect("GridRecoveryPhrase always holds a valid 12-word mnemonic")
    }
}

/// Non-secret enrollment data for a Border Wallet signer.
///
/// This is the only data that may be persisted after signer creation.
/// It contains no secret material.
#[derive(Debug, Clone)]
pub struct BorderWalletEnrollment {
    pub fingerprint: Fingerprint,
    pub xpub: bip32::Xpub,
    pub derivation_path: bip32::DerivationPath,
    pub network: Network,
}

/// Derive non-secret enrollment data from a Border Wallet mnemonic.
///
/// Computes the master fingerprint and xpub at the default derivation path.
/// All secret material (seed, xpriv) is zeroized when this function returns.
///
/// This is the only function that touches private key material, and it
/// ensures nothing secret escapes into the returned `BorderWalletEnrollment`.
pub fn derive_enrollment(
    mnemonic: &bip39::Mnemonic,
    network: Network,
    secp: &secp256k1::Secp256k1<secp256k1::All>,
) -> Result<BorderWalletEnrollment, BorderWalletError> {
    let derivation_path = default_derivation_path(network);

    // Derive seed — zeroize on drop.
    let seed = Zeroizing::new(mnemonic.to_seed(""));

    // SECURITY NOTE: `bip32::Xpriv` is a `Copy` type, so we cannot guarantee
    // zeroization of private key material on the stack. The Rust compiler may
    // copy these values freely. We mitigate this by:
    // 1. Confining Xpriv usage to the smallest possible scope
    // 2. Converting to Xpub (non-secret) immediately
    // 3. Never storing or returning Xpriv values
    // This is a known limitation of the upstream `bitcoin` crate's Xpriv type.
    let (fingerprint, xpub) = {
        let master_xpriv = bip32::Xpriv::new_master(network, &*seed)
            .map_err(|e| BorderWalletError::KeyDerivation(e.to_string()))?;
        let fingerprint = master_xpriv.fingerprint(secp);
        let xpub = bip32::Xpub::from_priv(
            secp,
            &master_xpriv
                .derive_priv(secp, &derivation_path)
                .map_err(|e| BorderWalletError::KeyDerivation(e.to_string()))?,
        );
        (fingerprint, xpub)
    };
    // master_xpriv and derived child are now out of scope.
    // seed is Zeroizing and will be wiped on drop.

    Ok(BorderWalletEnrollment {
        fingerprint,
        xpub,
        derivation_path,
        network,
    })
}

/// Sign a PSBT using a transiently reconstructed Border Wallet key.
///
/// This function:
/// 1. Creates a transient `HotSigner` from the reconstructed mnemonic
/// 2. Verifies the fingerprint matches the expected enrollment fingerprint
/// 3. Signs the PSBT
/// 4. All secret material is dropped when this function returns
///
/// Returns the fingerprint and signed PSBT on success.
pub fn sign_psbt_with_border_wallet(
    mnemonic: bip39::Mnemonic,
    expected_fingerprint: Fingerprint,
    network: Network,
    psbt: Psbt,
) -> Result<(Fingerprint, Psbt), BorderWalletError> {
    let secp = secp256k1::Secp256k1::new();

    let signer = HotSigner::from_mnemonic(network, mnemonic)
        .map_err(|e| BorderWalletError::KeyDerivation(e.to_string()))?;

    let actual_fingerprint = signer.fingerprint(&secp);
    if actual_fingerprint != expected_fingerprint {
        return Err(BorderWalletError::FingerprintMismatch {
            expected: expected_fingerprint,
            got: actual_fingerprint,
        });
    }

    let signed_psbt = signer
        .sign_psbt(psbt, &secp)
        .map_err(|e| BorderWalletError::SigningFailed(e.to_string()))?;

    Ok((actual_fingerprint, signed_psbt))
}

/// The standard derivation path for Border Wallet signers.
///
/// Uses BIP-48 multisig path with script type 2 (Taproot):
/// - Mainnet: m/48'/0'/0'/2'
/// - Testnet/Signet: m/48'/1'/0'/2'
pub fn default_derivation_path(network: Network) -> bip32::DerivationPath {
    let coin_type = match network {
        Network::Bitcoin => 0,
        _ => 1,
    };
    let path = format!("m/48'/{coin_type}'/0'/2'");
    path.parse().expect("hardcoded derivation path is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_recovery_phrase() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let rp = GridRecoveryPhrase::from_phrase(phrase);
        assert!(rp.is_ok());
        assert_eq!(rp.unwrap().as_str(), phrase);
    }

    #[test]
    fn test_invalid_recovery_phrase() {
        assert!(GridRecoveryPhrase::from_phrase("not a valid mnemonic").is_err());
    }

    #[test]
    fn test_24_word_phrase_rejected() {
        // 24-word mnemonic should be rejected (we only accept 12-word)
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art";
        assert!(GridRecoveryPhrase::from_phrase(phrase).is_err());
    }

    #[test]
    fn test_generate_recovery_phrase() {
        let rp = GridRecoveryPhrase::generate().unwrap();
        let words: Vec<&str> = rp.as_str().split_whitespace().collect();
        assert_eq!(words.len(), 12);
        // Should be parseable as a valid BIP39 mnemonic.
        assert!(bip39::Mnemonic::parse_in(bip39::Language::English, rp.as_str()).is_ok());
    }

    #[test]
    fn test_default_derivation_path_mainnet() {
        let path = default_derivation_path(Network::Bitcoin);
        assert_eq!(path.to_string(), "48'/0'/0'/2'");
    }

    #[test]
    fn test_default_derivation_path_testnet() {
        let path = default_derivation_path(Network::Testnet);
        assert_eq!(path.to_string(), "48'/1'/0'/2'");
    }

    #[test]
    fn test_generate_grid_from_phrase() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let rp = GridRecoveryPhrase::from_phrase(phrase).unwrap();
        let grid = rp.generate_grid();
        // Grid should be a valid 2048-cell permutation.
        assert_eq!(grid.cells().len(), WordGrid::TOTAL_CELLS);
        // Same phrase via direct call should match.
        let grid2 = WordGrid::from_recovery_phrase(phrase).unwrap();
        assert_eq!(grid.cells(), grid2.cells());
    }

    // --- Milestone 3: enrollment derivation tests ---

    #[test]
    fn test_derive_enrollment_produces_valid_result() {
        let secp = secp256k1::Secp256k1::new();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let rp = GridRecoveryPhrase::from_phrase(phrase).unwrap();
        let grid = rp.generate_grid();

        let mut pattern = OrderedPattern::new();
        for col in 0..11u8 {
            pattern.add(CellRef::new(0, col)).unwrap();
        }
        let (mnemonic, _) = build_mnemonic(&grid, &pattern).unwrap();
        let enrollment = derive_enrollment(&mnemonic, Network::Testnet, &secp).unwrap();

        // Fingerprint should be non-zero.
        assert_ne!(enrollment.fingerprint, Fingerprint::default());
        // Derivation path should match default.
        assert_eq!(
            enrollment.derivation_path,
            default_derivation_path(Network::Testnet)
        );
        assert_eq!(enrollment.network, Network::Testnet);
    }

    #[test]
    fn test_derive_enrollment_deterministic() {
        let secp = secp256k1::Secp256k1::new();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let rp = GridRecoveryPhrase::from_phrase(phrase).unwrap();
        let grid = rp.generate_grid();

        let mut pattern = OrderedPattern::new();
        for col in 0..11u8 {
            pattern.add(CellRef::new(0, col)).unwrap();
        }
        let (mnemonic, _) = build_mnemonic(&grid, &pattern).unwrap();

        let e1 = derive_enrollment(&mnemonic, Network::Testnet, &secp).unwrap();
        let e2 = derive_enrollment(&mnemonic, Network::Testnet, &secp).unwrap();

        assert_eq!(e1.fingerprint, e2.fingerprint);
        assert_eq!(e1.xpub, e2.xpub);
        assert_eq!(e1.derivation_path, e2.derivation_path);
    }

    #[test]
    fn test_derive_enrollment_different_mnemonic_different_result() {
        let secp = secp256k1::Secp256k1::new();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let grid = WordGrid::from_recovery_phrase(phrase).unwrap();

        let mut pat1 = OrderedPattern::new();
        for col in 0..11u8 {
            pat1.add(CellRef::new(0, col)).unwrap();
        }
        let mut pat2 = OrderedPattern::new();
        for col in 0..11u8 {
            pat2.add(CellRef::new(1, col)).unwrap();
        }

        let (m1, _) = build_mnemonic(&grid, &pat1).unwrap();
        let (m2, _) = build_mnemonic(&grid, &pat2).unwrap();
        let e1 = derive_enrollment(&m1, Network::Testnet, &secp).unwrap();
        let e2 = derive_enrollment(&m2, Network::Testnet, &secp).unwrap();

        assert_ne!(e1.fingerprint, e2.fingerprint);
        assert_ne!(e1.xpub, e2.xpub);
    }

    #[test]
    fn test_full_end_to_end_create_and_reconstruct() {
        let secp = secp256k1::Secp256k1::new();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

        // Creation: generate grid, select pattern, derive enrollment.
        let rp = GridRecoveryPhrase::from_phrase(phrase).unwrap();
        let grid = rp.generate_grid();
        let mut pattern = OrderedPattern::new();
        for col in 0..11u8 {
            pattern.add(CellRef::new(0, col)).unwrap();
        }
        let (mnemonic, checksum) = build_mnemonic(&grid, &pattern).unwrap();
        let enrollment = derive_enrollment(&mnemonic, Network::Testnet, &secp).unwrap();

        // Reconstruction: same phrase, same grid, same pattern → same result.
        let rp2 = GridRecoveryPhrase::from_phrase(phrase).unwrap();
        let grid2 = rp2.generate_grid();
        let mut pattern2 = OrderedPattern::new();
        for col in 0..11u8 {
            pattern2.add(CellRef::new(0, col)).unwrap();
        }
        let (mnemonic2, checksum2) = build_mnemonic(&grid2, &pattern2).unwrap();
        let enrollment2 = derive_enrollment(&mnemonic2, Network::Testnet, &secp).unwrap();

        assert_eq!(mnemonic.to_string(), mnemonic2.to_string());
        assert_eq!(checksum, checksum2);
        assert_eq!(enrollment.fingerprint, enrollment2.fingerprint);
        assert_eq!(enrollment.xpub, enrollment2.xpub);
    }

    #[test]
    fn test_reconstruction_wrong_phrase_fails_match() {
        let secp = secp256k1::Secp256k1::new();

        // Original creation.
        let grid1 = WordGrid::from_recovery_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ).unwrap();
        let mut pattern = OrderedPattern::new();
        for col in 0..11u8 {
            pattern.add(CellRef::new(0, col)).unwrap();
        }
        let (m1, _) = build_mnemonic(&grid1, &pattern).unwrap();
        let e1 = derive_enrollment(&m1, Network::Testnet, &secp).unwrap();

        // Reconstruction with wrong phrase → different grid → different mnemonic.
        let grid2 =
            WordGrid::from_recovery_phrase("zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong")
                .unwrap();
        let mut pattern2 = OrderedPattern::new();
        for col in 0..11u8 {
            pattern2.add(CellRef::new(0, col)).unwrap();
        }
        let (m2, _) = build_mnemonic(&grid2, &pattern2).unwrap();
        let e2 = derive_enrollment(&m2, Network::Testnet, &secp).unwrap();

        assert_ne!(e1.fingerprint, e2.fingerprint);
    }

    #[test]
    fn test_reconstruction_wrong_pattern_fails_match() {
        let secp = secp256k1::Secp256k1::new();
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let grid = WordGrid::from_recovery_phrase(phrase).unwrap();

        // Original: row 0.
        let mut pat1 = OrderedPattern::new();
        for col in 0..11u8 {
            pat1.add(CellRef::new(0, col)).unwrap();
        }
        let (m1, _) = build_mnemonic(&grid, &pat1).unwrap();
        let e1 = derive_enrollment(&m1, Network::Testnet, &secp).unwrap();

        // Reconstruction with wrong pattern: row 5.
        let mut pat2 = OrderedPattern::new();
        for col in 0..11u8 {
            pat2.add(CellRef::new(5, col)).unwrap();
        }
        let (m2, _) = build_mnemonic(&grid, &pat2).unwrap();
        let e2 = derive_enrollment(&m2, Network::Testnet, &secp).unwrap();

        assert_ne!(e1.fingerprint, e2.fingerprint);
    }

    #[test]
    fn test_enrollment_contains_no_secret_fields() {
        // BorderWalletEnrollment should only have non-secret fields.
        // This is a compile-time structural test — if someone adds a field
        // containing secret data, this test should be updated to verify
        // it's not present.
        let secp = secp256k1::Secp256k1::new();
        let grid = WordGrid::from_recovery_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ).unwrap();
        let mut pattern = OrderedPattern::new();
        for col in 0..11u8 {
            pattern.add(CellRef::new(0, col)).unwrap();
        }
        let (mnemonic, _) = build_mnemonic(&grid, &pattern).unwrap();
        let enrollment = derive_enrollment(&mnemonic, Network::Testnet, &secp).unwrap();

        // Debug output should not contain any mnemonic words or seed hex.
        let debug = format!("{:?}", enrollment);
        assert!(!debug.contains("abandon"));
        assert!(!debug.contains("mnemonic"));
        assert!(!debug.contains("seed"));
        assert!(!debug.contains("xprv"));
        // Should contain expected non-secret data.
        assert!(debug.contains("fingerprint"));
        assert!(debug.contains("xpub"));
    }

    #[test]
    fn test_enrollment_is_debug() {
        // BorderWalletEnrollment should be Debug (non-secret).
        use std::str::FromStr;
        let path = default_derivation_path(Network::Testnet);
        let enrollment = BorderWalletEnrollment {
            fingerprint: Fingerprint::default(),
            xpub: bip32::Xpub::from_str("tpubD6NzVbkrYhZ4Y87GapBo55UPVQkxRVAMu3eK5iDbEzBzuCknhoT7CWP1s9UjNHcbC4GRVMBzywcRgDrM9oPV1g6HudeCeQfLbASVBxpNJV3").unwrap(),
            derivation_path: path,
            network: Network::Testnet,
        };
        let debug_str = format!("{:?}", enrollment);
        assert!(debug_str.contains("BorderWalletEnrollment"));
    }
}
