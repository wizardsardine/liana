//! Ordered pattern model for Border Wallet cell selection.
//!
//! Manages the user's ordered selection of 11 cells from the Word Grid,
//! and constructs a valid 12-word BIP39 mnemonic from the selection.

use crate::border_wallet::error::BorderWalletError;
use crate::border_wallet::grid::WordGrid;

use miniscript::bitcoin::hashes::{sha256, Hash};

/// Required number of cells in a complete pattern.
pub const PATTERN_LENGTH: usize = 11;

/// A reference to a single cell in the Word Grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellRef {
    pub row: u16,
    pub col: u8,
}

impl CellRef {
    pub fn new(row: u16, col: u8) -> Self {
        Self { row, col }
    }

    /// Check whether this cell is within the grid bounds.
    pub fn is_in_bounds(&self) -> bool {
        (self.row as usize) < WordGrid::ROWS && (self.col as usize) < WordGrid::COLS
    }
}

/// An ordered selection of cells from the Word Grid.
///
/// Tracks up to 11 unique cells in the order they were selected.
#[derive(Clone)]
pub struct OrderedPattern {
    cells: Vec<CellRef>,
}

impl OrderedPattern {
    /// Create an empty pattern.
    pub fn new() -> Self {
        Self {
            cells: Vec::with_capacity(PATTERN_LENGTH),
        }
    }

    /// The current number of selected cells.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether the pattern has no cells.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Whether the pattern is complete (exactly 11 cells).
    pub fn is_complete(&self) -> bool {
        self.cells.len() == PATTERN_LENGTH
    }

    /// The selected cells in order.
    pub fn cells(&self) -> &[CellRef] {
        &self.cells
    }

    /// Add a cell to the end of the pattern.
    ///
    /// Returns an error if:
    /// - the pattern is already complete (11 cells)
    /// - the cell is out of bounds
    /// - the cell is already in the pattern
    pub fn add(&mut self, cell: CellRef) -> Result<(), BorderWalletError> {
        if self.cells.len() >= PATTERN_LENGTH {
            return Err(BorderWalletError::InvalidPatternLength(
                self.cells.len() + 1,
            ));
        }
        if !cell.is_in_bounds() {
            return Err(BorderWalletError::CellOutOfBounds {
                row: cell.row,
                col: cell.col,
            });
        }
        if self.cells.contains(&cell) {
            return Err(BorderWalletError::DuplicateCell {
                row: cell.row,
                col: cell.col,
            });
        }
        self.cells.push(cell);
        Ok(())
    }

    /// Remove the cell at the given position in the selection order (0-indexed).
    ///
    /// Returns `None` if the index is out of range.
    pub fn remove_at(&mut self, index: usize) -> Option<CellRef> {
        if index < self.cells.len() {
            Some(self.cells.remove(index))
        } else {
            None
        }
    }

    /// Remove the last added cell.
    pub fn undo_last(&mut self) -> Option<CellRef> {
        self.cells.pop()
    }

    /// Clear all selected cells.
    pub fn clear(&mut self) {
        self.cells.clear();
    }

    /// Validate that this pattern is complete and well-formed.
    ///
    /// Returns `Ok(())` if the pattern has exactly 11 unique, in-bounds cells.
    pub fn validate(&self) -> Result<(), BorderWalletError> {
        if self.cells.len() != PATTERN_LENGTH {
            return Err(BorderWalletError::InvalidPatternLength(self.cells.len()));
        }
        for cell in &self.cells {
            if !cell.is_in_bounds() {
                return Err(BorderWalletError::CellOutOfBounds {
                    row: cell.row,
                    col: cell.col,
                });
            }
        }
        // Check for duplicates.
        for (i, a) in self.cells.iter().enumerate() {
            for b in &self.cells[i + 1..] {
                if a == b {
                    return Err(BorderWalletError::DuplicateCell {
                        row: a.row,
                        col: a.col,
                    });
                }
            }
        }
        Ok(())
    }
}

impl Default for OrderedPattern {
    fn default() -> Self {
        Self::new()
    }
}

/// Build a valid 12-word BIP39 mnemonic from an 11-cell pattern on a grid.
///
/// The 11 selected cells provide 11 BIP39 word indices (121 bits).
/// The remaining 7 entropy bits are set to zero, giving 128 bits of entropy.
/// The 12th word is computed from those 128 bits plus the BIP39 SHA256 checksum.
///
/// Returns the complete mnemonic and the checksum word separately for UI display.
pub fn build_mnemonic(
    grid: &WordGrid,
    pattern: &OrderedPattern,
) -> Result<(bip39::Mnemonic, &'static str), BorderWalletError> {
    pattern.validate()?;

    // Collect the 11 BIP39 word indices from the grid.
    let mut word_indices = [0u16; PATTERN_LENGTH];
    for (i, cell) in pattern.cells().iter().enumerate() {
        word_indices[i] = grid
            .word_index_at(cell.row as usize, cell.col as usize)
            .ok_or(BorderWalletError::CellOutOfBounds {
                row: cell.row,
                col: cell.col,
            })?;
    }

    // Pack 11 × 11-bit indices into a bit buffer.
    // Total bits from 11 words: 121 bits.
    // We need 128 bits of entropy (bits 121–127 set to 0).
    let mut entropy = [0u8; 16]; // 128 bits
    let mut bit_offset = 0usize;
    for &idx in &word_indices {
        write_bits(&mut entropy, bit_offset, idx as u32, 11);
        bit_offset += 11;
    }
    // Bits 121–127 are already zero (7 free entropy bits = 0).

    // Compute the checksum: first 4 bits of SHA256(entropy).
    let hash = sha256::Hash::hash(&entropy);
    let checksum_byte = hash.as_byte_array()[0]; // first byte
    let checksum_4bits = (checksum_byte >> 4) & 0x0F; // top 4 bits

    // Build the 12th word index: bits 121–127 (0000000) + 4 checksum bits = 11 bits.
    // bits 121–127 of entropy are zero, so the top 7 bits of the 12th word index are 0.
    let twelfth_word_index = checksum_4bits as u16; // 0b0000000_CCCC

    // Construct the mnemonic from entropy using bip39 crate.
    let mnemonic = bip39::Mnemonic::from_entropy(&entropy)
        .map_err(|e| BorderWalletError::MnemonicConstruction(e.to_string()))?;

    // Verify the 12th word matches our expectation.
    let words: Vec<&str> = mnemonic.words().collect();
    if words.len() != 12 {
        return Err(BorderWalletError::MnemonicConstruction(format!(
            "expected 12-word mnemonic, got {}",
            words.len()
        )));
    }

    let word_list = bip39::Language::English.word_list();
    let checksum_word = word_list[twelfth_word_index as usize];
    if words[11] != checksum_word {
        return Err(BorderWalletError::MnemonicConstruction(format!(
            "checksum word mismatch: expected '{}', got '{}'",
            checksum_word, words[11]
        )));
    }

    Ok((mnemonic, checksum_word))
}

/// Write `num_bits` bits from `value` into `buffer` at the given bit offset.
fn write_bits(buffer: &mut [u8], bit_offset: usize, value: u32, num_bits: usize) {
    for i in 0..num_bits {
        let bit = (value >> (num_bits - 1 - i)) & 1;
        let byte_idx = (bit_offset + i) / 8;
        let bit_idx = 7 - ((bit_offset + i) % 8);
        if bit == 1 {
            buffer[byte_idx] |= 1 << bit_idx;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_pattern() {
        let pat = OrderedPattern::new();
        assert!(pat.is_empty());
        assert!(!pat.is_complete());
        assert_eq!(pat.len(), 0);
    }

    #[test]
    fn test_add_cells() {
        let mut pat = OrderedPattern::new();
        for i in 0..11 {
            pat.add(CellRef::new(i, 0)).unwrap();
        }
        assert!(pat.is_complete());
        assert_eq!(pat.len(), 11);
    }

    #[test]
    fn test_add_beyond_11_fails() {
        let mut pat = OrderedPattern::new();
        for i in 0..11 {
            pat.add(CellRef::new(i, 0)).unwrap();
        }
        assert!(pat.add(CellRef::new(11, 0)).is_err());
    }

    #[test]
    fn test_duplicate_rejected() {
        let mut pat = OrderedPattern::new();
        pat.add(CellRef::new(5, 3)).unwrap();
        assert!(pat.add(CellRef::new(5, 3)).is_err());
    }

    #[test]
    fn test_out_of_bounds_rejected() {
        let mut pat = OrderedPattern::new();
        assert!(pat.add(CellRef::new(128, 0)).is_err()); // row out of bounds
        assert!(pat.add(CellRef::new(0, 16)).is_err()); // col out of bounds
    }

    #[test]
    fn test_undo_last() {
        let mut pat = OrderedPattern::new();
        pat.add(CellRef::new(0, 0)).unwrap();
        pat.add(CellRef::new(1, 1)).unwrap();
        let removed = pat.undo_last().unwrap();
        assert_eq!(removed, CellRef::new(1, 1));
        assert_eq!(pat.len(), 1);
    }

    #[test]
    fn test_clear() {
        let mut pat = OrderedPattern::new();
        pat.add(CellRef::new(0, 0)).unwrap();
        pat.add(CellRef::new(1, 1)).unwrap();
        pat.clear();
        assert!(pat.is_empty());
    }

    #[test]
    fn test_remove_at() {
        let mut pat = OrderedPattern::new();
        pat.add(CellRef::new(0, 0)).unwrap();
        pat.add(CellRef::new(1, 1)).unwrap();
        pat.add(CellRef::new(2, 2)).unwrap();
        let removed = pat.remove_at(1).unwrap();
        assert_eq!(removed, CellRef::new(1, 1));
        assert_eq!(pat.cells()[0], CellRef::new(0, 0));
        assert_eq!(pat.cells()[1], CellRef::new(2, 2));
    }

    #[test]
    fn test_validate_incomplete() {
        let mut pat = OrderedPattern::new();
        pat.add(CellRef::new(0, 0)).unwrap();
        assert!(pat.validate().is_err());
    }

    #[test]
    fn test_validate_complete() {
        let mut pat = OrderedPattern::new();
        for i in 0..11 {
            pat.add(CellRef::new(i, i as u8 % 16)).unwrap();
        }
        assert!(pat.validate().is_ok());
    }

    #[test]
    fn test_cell_in_bounds() {
        assert!(CellRef::new(0, 0).is_in_bounds());
        assert!(CellRef::new(127, 15).is_in_bounds());
        assert!(!CellRef::new(128, 0).is_in_bounds());
        assert!(!CellRef::new(0, 16).is_in_bounds());
    }

    // --- Milestone 3: mnemonic construction tests ---

    /// Helper: build a complete 11-cell pattern from the first 11 cells in row 0.
    fn test_pattern_row0() -> OrderedPattern {
        let mut pat = OrderedPattern::new();
        for col in 0..11u8 {
            pat.add(CellRef::new(0, col)).unwrap();
        }
        pat
    }

    #[test]
    fn test_build_mnemonic_produces_valid_bip39() {
        let grid = WordGrid::from_recovery_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ).unwrap();
        let pattern = test_pattern_row0();
        let (mnemonic, _checksum_word) = build_mnemonic(&grid, &pattern).unwrap();
        // Must be a valid 12-word mnemonic.
        assert_eq!(mnemonic.word_count(), 12);
        // Re-parsing must succeed.
        assert!(bip39::Mnemonic::parse_in(bip39::Language::English, mnemonic.to_string()).is_ok());
    }

    #[test]
    fn test_build_mnemonic_checksum_word_is_12th() {
        let grid = WordGrid::from_recovery_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ).unwrap();
        let pattern = test_pattern_row0();
        let (mnemonic, checksum_word) = build_mnemonic(&grid, &pattern).unwrap();
        let words: Vec<&str> = mnemonic.words().collect();
        assert_eq!(words[11], checksum_word);
    }

    #[test]
    fn test_build_mnemonic_deterministic() {
        let grid = WordGrid::from_recovery_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ).unwrap();
        let pattern = test_pattern_row0();
        let (m1, c1) = build_mnemonic(&grid, &pattern).unwrap();
        let (m2, c2) = build_mnemonic(&grid, &pattern).unwrap();
        assert_eq!(m1.to_string(), m2.to_string());
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_build_mnemonic_different_pattern_different_mnemonic() {
        let grid = WordGrid::from_recovery_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ).unwrap();
        let pat1 = test_pattern_row0();

        let mut pat2 = OrderedPattern::new();
        for col in 0..11u8 {
            pat2.add(CellRef::new(1, col)).unwrap(); // row 1 instead of row 0
        }

        let (m1, _) = build_mnemonic(&grid, &pat1).unwrap();
        let (m2, _) = build_mnemonic(&grid, &pat2).unwrap();
        assert_ne!(m1.to_string(), m2.to_string());
    }

    #[test]
    fn test_build_mnemonic_incomplete_pattern_fails() {
        let grid = WordGrid::from_recovery_phrase(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        ).unwrap();
        let mut pat = OrderedPattern::new();
        for col in 0..10u8 {
            pat.add(CellRef::new(0, col)).unwrap();
        }
        assert!(build_mnemonic(&grid, &pat).is_err());
    }

    #[test]
    fn test_build_mnemonic_first_11_words_match_grid() {
        let phrase = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let grid = WordGrid::from_recovery_phrase(phrase).unwrap();
        let pattern = test_pattern_row0();
        let (mnemonic, _) = build_mnemonic(&grid, &pattern).unwrap();
        let words: Vec<&str> = mnemonic.words().collect();

        // First 11 words should be the words at the selected grid cells.
        for (i, col) in (0..11u8).enumerate() {
            let grid_word = grid.word_at(0, col as usize).unwrap();
            assert_eq!(words[i], grid_word, "word {} mismatch", i);
        }
    }

    #[test]
    fn test_write_bits_roundtrip() {
        let mut buf = [0u8; 4];
        // Write 11-bit value 2047 (0x7FF) at bit offset 0.
        write_bits(&mut buf, 0, 2047, 11);
        // Bits 0–10 should be 11111111111.
        assert_eq!(buf[0], 0xFF); // bits 0-7
        assert_eq!(buf[1] & 0xE0, 0xE0); // bits 8-10

        // Write another 11-bit value 0 at bit offset 11.
        write_bits(&mut buf, 11, 0, 11);
        // Bits 11-21 should be all zero — verify byte 1 lower bits are 0.
        assert_eq!(buf[1] & 0x1F, 0x00);
    }
}
