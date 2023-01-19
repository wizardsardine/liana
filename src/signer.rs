//! Signer module
//!
//! Some helpers to facilitate the usage of a signer in client of the Liana daemon. For now
//! only contains a hot signer.

use crate::random;

use std::{convert::TryInto, error, fmt, str};

/// An error related to using a signer.
#[derive(Debug)]
pub enum SignerError {
    Randomness(random::RandomnessError),
    Mnemonic(bip39::Error),
}

impl fmt::Display for SignerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Randomness(s) => write!(f, "Error related to getting randomness: {}", s),
            Self::Mnemonic(s) => write!(f, "Error when working with mnemonics: {}", s),
        }
    }
}

impl error::Error for SignerError {}

/// A signer that keeps the key on the laptop. Based on BIP39.
pub struct HotSigner {
    mnemonic: bip39::Mnemonic,
}

impl HotSigner {
    /// Create a new hot signer from random bytes. Uses a 12-words mnemonics without a passphrase.
    pub fn generate() -> Result<Self, SignerError> {
        // We want a 12-words mnemonic so we only use 16 of the 32 bytes.
        let random_32bytes = random::random_bytes().map_err(SignerError::Randomness)?;
        let mnemonic =
            bip39::Mnemonic::from_entropy(&random_32bytes[..16]).map_err(SignerError::Mnemonic)?;
        Ok(Self { mnemonic })
    }

    /// The BIP39 mnemonics from which the master key of this signer is derived.
    pub fn words(&self) -> [&'static str; 12] {
        let words: Vec<&'static str> = self.mnemonic.word_iter().collect();
        words.try_into().expect("Always 12 words")
    }
}

impl str::FromStr for HotSigner {
    type Err = SignerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mnemonic = bip39::Mnemonic::from_str(s).map_err(SignerError::Mnemonic)?;
        Ok(Self { mnemonic })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn hot_signer_gen() {
        // Entropy isn't completely broken.
        assert_ne!(
            HotSigner::generate().unwrap().words(),
            HotSigner::generate().unwrap().words()
        );

        // Roundtrips.
        let signer = HotSigner::generate().unwrap();
        let mnemonics_str = signer.words().iter().fold(String::new(), |mut s, w| {
            s += w;
            s += " ";
            s
        });
        assert_eq!(
            HotSigner::from_str(&mnemonics_str).unwrap().words(),
            signer.words()
        );
    }
}
