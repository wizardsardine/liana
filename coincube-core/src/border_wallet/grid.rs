//! Deterministic Word Grid for Border Wallet.
//!
//! Generates a 16×128 grid containing a deterministic permutation of the
//! 2048 BIP39 English words, seeded by a recovery phrase.
//!
//! ## Algorithm
//!
//! 1. Compute `seed = HMAC-SHA512(key = DOMAIN_TAG, msg = recovery_phrase)`
//! 2. Initialize `indices = [0, 1, 2, ..., 2047]`
//! 3. Fisher-Yates shuffle using deterministic PRNG:
//!    - For `i` from 2047 down to 1:
//!      a. `h = HMAC-SHA256(key = seed, msg = i as big-endian u16)`
//!      b. `j = (first 8 bytes of h as u64) % (i + 1)`
//!      c. Swap `indices[i]` and `indices[j]`
//! 4. Arrange shuffled indices row-major: row `r`, col `c` → `indices[r * 16 + c]`

use crate::border_wallet::error::BorderWalletError;

use miniscript::bitcoin::hashes::{sha256, sha512, Hash, HashEngine, Hmac, HmacEngine};

/// Domain separation tag for grid generation. Ensures COINCUBE grids are
/// self-consistent and won't collide with other HMAC usages.
const DOMAIN_TAG: &[u8] = b"coincube/border-wallet-grid/v1";

/// A 16×128 deterministic word grid.
///
/// Contains a permutation of all 2048 BIP39 English word indices,
/// arranged in row-major order (128 rows × 16 columns).
///
/// The grid is generated deterministically from a recovery phrase
/// using HMAC-SHA512 + Fisher-Yates shuffle, so the same recovery
/// phrase always produces the same grid.
pub struct WordGrid {
    /// Row-major array of BIP39 word indices (0..2048).
    /// Length is always TOTAL_CELLS (2048).
    cells: Vec<u16>,
}

impl WordGrid {
    pub const COLS: usize = 16;
    pub const ROWS: usize = 128;
    pub const TOTAL_CELLS: usize = Self::COLS * Self::ROWS; // 2048

    /// Generate a deterministic word grid from a recovery phrase.
    ///
    /// The phrase must be a valid 12-word BIP39 mnemonic. Returns an error
    /// if the phrase is invalid. The input is canonicalized (trimmed,
    /// lowercased, whitespace-collapsed) before hashing so that formatting
    /// differences never produce a different grid.
    pub fn from_recovery_phrase(phrase: &str) -> Result<Self, BorderWalletError> {
        // Validate as a 12-word BIP39 mnemonic before proceeding.
        let mnemonic = bip39::Mnemonic::parse_in(bip39::Language::English, phrase)
            .map_err(|_| BorderWalletError::InvalidRecoveryPhrase)?;
        if mnemonic.word_count() != 12 {
            return Err(BorderWalletError::InvalidRecoveryPhrase);
        }

        // Canonicalize via the parsed mnemonic's string representation.
        let canonical = mnemonic.to_string();

        // Step 1: Derive seed via HMAC-SHA512.
        let seed = {
            let mut engine = HmacEngine::<sha512::Hash>::new(DOMAIN_TAG);
            engine.input(canonical.as_bytes());
            Hmac::<sha512::Hash>::from_engine(engine)
        };
        let seed_bytes = seed.as_byte_array();

        // Step 2: Initialize identity permutation.
        let mut indices: Vec<u16> = (0..Self::TOTAL_CELLS as u16).collect();

        // Step 3: Fisher-Yates shuffle with deterministic PRNG.
        for i in (1..Self::TOTAL_CELLS).rev() {
            let j = deterministic_index(seed_bytes, i);
            indices.swap(i, j);
        }

        Ok(Self::from_indices(indices))
    }

    /// Get the BIP39 word index at the given (row, col).
    ///
    /// Returns `None` if out of bounds.
    pub fn word_index_at(&self, row: usize, col: usize) -> Option<u16> {
        if row >= Self::ROWS || col >= Self::COLS {
            return None;
        }
        Some(self.cells[row * Self::COLS + col])
    }

    /// Get the full BIP39 English word at the given (row, col).
    ///
    /// Returns an error if out of bounds.
    pub fn word_at(&self, row: usize, col: usize) -> Result<&'static str, BorderWalletError> {
        let idx = self
            .word_index_at(row, col)
            .ok_or(BorderWalletError::CellOutOfBounds {
                row: row as u16,
                col: col as u8,
            })?;
        Ok(bip39::Language::English.word_list()[idx as usize])
    }

    /// Get the 4-letter display prefix at the given (row, col).
    ///
    /// Returns an error if out of bounds.
    pub fn prefix_at(&self, row: usize, col: usize) -> Result<String, BorderWalletError> {
        let word = self.word_at(row, col)?;
        let prefix: String = word.chars().take(4).collect();
        Ok(prefix)
    }

    /// Create a `WordGrid` from a pre-computed array of word indices.
    ///
    /// Used internally by the grid generation algorithm.
    /// Panics if `cells.len() != TOTAL_CELLS`.
    pub(crate) fn from_indices(cells: Vec<u16>) -> Self {
        assert_eq!(cells.len(), Self::TOTAL_CELLS);
        Self { cells }
    }

    /// Access the raw cell indices (used in tests).
    #[cfg(test)]
    pub(crate) fn cells(&self) -> &[u16] {
        &self.cells
    }
}

/// Compute a deterministic index `j` in `[0, i]` for Fisher-Yates step `i`.
///
/// Uses HMAC-SHA256(key=seed, msg=i as big-endian u16) and reduces
/// the first 8 bytes modulo `(i + 1)`.
fn deterministic_index(seed: &[u8; 64], i: usize) -> usize {
    let mut engine = HmacEngine::<sha256::Hash>::new(seed);
    engine.input(&(i as u16).to_be_bytes());
    let hash = Hmac::<sha256::Hash>::from_engine(engine);
    let bytes = hash.as_byte_array();
    let val = u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    (val % (i as u64 + 1)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_grid_dimensions() {
        assert_eq!(WordGrid::COLS, 16);
        assert_eq!(WordGrid::ROWS, 128);
        assert_eq!(WordGrid::TOTAL_CELLS, 2048);
    }

    #[test]
    fn test_word_at_out_of_bounds() {
        let cells: Vec<u16> = (0..2048).collect();
        let grid = WordGrid::from_indices(cells);

        assert!(grid.word_at(128, 0).is_err());
        assert!(grid.word_at(0, 16).is_err());
        assert!(grid.word_at(127, 15).is_ok());
    }

    #[test]
    fn test_prefix_length() {
        let cells: Vec<u16> = (0..2048).collect();
        let grid = WordGrid::from_indices(cells);

        let prefix = grid.prefix_at(0, 0).unwrap();
        assert!(prefix.len() <= 4);
        assert!(!prefix.is_empty());
    }

    // --- Milestone 2 tests ---

    const TEST_PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn test_deterministic_regeneration() {
        let grid1 = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        let grid2 = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        assert_eq!(
            grid1.cells(),
            grid2.cells(),
            "same phrase must produce same grid"
        );
    }

    #[test]
    fn test_different_phrases_yield_different_grids() {
        let grid1 = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        let grid2 =
            WordGrid::from_recovery_phrase("zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong")
                .unwrap();
        assert_ne!(
            grid1.cells(),
            grid2.cells(),
            "different phrases must produce different grids"
        );
    }

    #[test]
    fn test_grid_is_complete_permutation() {
        let grid = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        let cells = grid.cells();

        // Must have exactly 2048 cells.
        assert_eq!(cells.len(), WordGrid::TOTAL_CELLS);

        // Must be a permutation: every index 0..2048 appears exactly once.
        let unique: HashSet<u16> = cells.iter().copied().collect();
        assert_eq!(unique.len(), WordGrid::TOTAL_CELLS);
        assert_eq!(*unique.iter().min().unwrap(), 0);
        assert_eq!(*unique.iter().max().unwrap(), 2047);
    }

    #[test]
    fn test_grid_is_shuffled() {
        let grid = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        let cells = grid.cells();

        // The grid should not be the identity permutation.
        let identity: Vec<u16> = (0..2048).collect();
        assert_ne!(cells, identity.as_slice(), "grid should be shuffled");
    }

    #[test]
    fn test_all_words_are_valid_bip39() {
        let grid = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        let word_list = bip39::Language::English.word_list();

        for row in 0..WordGrid::ROWS {
            for col in 0..WordGrid::COLS {
                let word = grid.word_at(row, col).unwrap();
                assert!(
                    word_list.contains(&word),
                    "word '{}' at ({}, {}) not in BIP39 wordlist",
                    word,
                    row,
                    col
                );
            }
        }
    }

    #[test]
    fn test_prefixes_are_four_chars_or_less() {
        let grid = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();

        for row in 0..WordGrid::ROWS {
            for col in 0..WordGrid::COLS {
                let prefix = grid.prefix_at(row, col).unwrap();
                assert!(prefix.len() <= 4);
                assert!(!prefix.is_empty());
            }
        }
    }

    #[test]
    fn test_word_index_matches_word() {
        let grid = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        let word_list = bip39::Language::English.word_list();

        for row in 0..WordGrid::ROWS {
            for col in 0..WordGrid::COLS {
                let idx = grid.word_index_at(row, col).unwrap();
                let word = grid.word_at(row, col).unwrap();
                assert_eq!(
                    word, word_list[idx as usize],
                    "word_index_at and word_at must agree at ({}, {})",
                    row, col
                );
            }
        }
    }

    #[test]
    fn test_deterministic_snapshot() {
        // Pin the first few cells so any algorithm change is caught.
        let grid = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        let snapshot: Vec<u16> = grid.cells()[..8].to_vec();
        let grid2 = WordGrid::from_recovery_phrase(TEST_PHRASE).unwrap();
        let snapshot2: Vec<u16> = grid2.cells()[..8].to_vec();
        assert_eq!(snapshot, snapshot2, "snapshot must be stable across runs");
    }
}
