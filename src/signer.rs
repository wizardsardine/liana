//! Signer module
//!
//! Some helpers to facilitate the usage of a signer in client of the Liana daemon. For now
//! only contains a hot signer.

use crate::random;

use std::{convert::TryInto, error, fmt, str::FromStr};

use miniscript::bitcoin::{
    self,
    util::bip32::{self, Error as Bip32Error},
};

/// An error related to using a signer.
#[derive(Debug)]
pub enum SignerError {
    Randomness(random::RandomnessError),
    Mnemonic(bip39::Error),
    Bip32(Bip32Error),
}

impl fmt::Display for SignerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Randomness(s) => write!(f, "Error related to getting randomness: {}", s),
            Self::Mnemonic(s) => write!(f, "Error when working with mnemonics: {}", s),
            Self::Bip32(e) => write!(f, "BIP32 error: {}", e),
        }
    }
}

impl error::Error for SignerError {}

// TODO: zeroize, mlock, etc.. For now we don't even encrypt the seed on disk so that'd be
// overkill.
/// A signer that keeps the key on the laptop. Based on BIP39.
pub struct HotSigner {
    mnemonic: bip39::Mnemonic,
    master_xpriv: bip32::ExtendedPrivKey,
}

impl HotSigner {
    fn from_mnemonic(
        network: bitcoin::Network,
        mnemonic: bip39::Mnemonic,
    ) -> Result<Self, SignerError> {
        let master_xpriv = bip32::ExtendedPrivKey::new_master(network, &mnemonic.to_seed(""))
            .map_err(SignerError::Bip32)?;
        Ok(Self {
            mnemonic,
            master_xpriv,
        })
    }

    /// Create a new hot signer from random bytes. Uses a 12-words mnemonics without a passphrase.
    pub fn generate(network: bitcoin::Network) -> Result<Self, SignerError> {
        // We want a 12-words mnemonic so we only use 16 of the 32 bytes.
        let random_32bytes = random::random_bytes().map_err(SignerError::Randomness)?;
        let mnemonic =
            bip39::Mnemonic::from_entropy(&random_32bytes[..16]).map_err(SignerError::Mnemonic)?;
        Self::from_mnemonic(network, mnemonic)
    }

    pub fn from_str(network: bitcoin::Network, s: &str) -> Result<Self, SignerError> {
        let mnemonic = bip39::Mnemonic::from_str(s).map_err(SignerError::Mnemonic)?;
        Self::from_mnemonic(network, mnemonic)
    }

    /// The BIP39 mnemonics from which the master key of this signer is derived.
    pub fn words(&self) -> [&'static str; 12] {
        let words: Vec<&'static str> = self.mnemonic.word_iter().collect();
        words.try_into().expect("Always 12 words")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hot_signer_gen() {
        // Entropy isn't completely broken.
        assert_ne!(
            HotSigner::generate(bitcoin::Network::Bitcoin)
                .unwrap()
                .words(),
            HotSigner::generate(bitcoin::Network::Bitcoin)
                .unwrap()
                .words()
        );

        // Roundtrips.
        let signer = HotSigner::generate(bitcoin::Network::Bitcoin).unwrap();
        let mnemonics_str = signer.words().iter().fold(String::new(), |mut s, w| {
            s += w;
            s += " ";
            s
        });
        assert_eq!(
            HotSigner::from_str(bitcoin::Network::Bitcoin, &mnemonics_str)
                .unwrap()
                .words(),
            signer.words()
        );
    }
}
