//! Signer module
//!
//! Some helpers to facilitate the usage of a signer in client of the Liana daemon. For now
//! only contains a hot signer.

use crate::random;

use std::{
    convert::TryInto,
    error, fmt, fs,
    io::{self, Write},
    path,
    str::FromStr,
};

use miniscript::bitcoin::{
    self, secp256k1,
    util::bip32::{self, Error as Bip32Error},
};

/// An error related to using a signer.
#[derive(Debug)]
pub enum SignerError {
    Randomness(random::RandomnessError),
    Mnemonic(bip39::Error),
    Bip32(Bip32Error),
    MnemonicStorage(io::Error),
}

impl fmt::Display for SignerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Randomness(s) => write!(f, "Error related to getting randomness: {}", s),
            Self::Mnemonic(s) => write!(f, "Error when working with mnemonics: {}", s),
            Self::Bip32(e) => write!(f, "BIP32 error: {}", e),
            Self::MnemonicStorage(e) => write!(f, "BIP39 mnemonic storage error: {}", e),
        }
    }
}

impl error::Error for SignerError {}

pub const MNEMONICS_FOLDER_NAME: &str = "mnemonics";

// TODO: zeroize, mlock, etc.. For now we don't even encrypt the seed on disk so that'd be
// overkill.
/// A signer that keeps the key on the laptop. Based on BIP39.
pub struct HotSigner {
    mnemonic: bip39::Mnemonic,
    master_xpriv: bip32::ExtendedPrivKey,
}

// TODO: instead of copying them here we could have a util module with those helpers.
// Create a directory with no permission for group and other users.
fn create_dir(path: &path::Path) -> io::Result<()> {
    #[cfg(unix)]
    return {
        use fs::DirBuilder;
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = DirBuilder::new();
        builder.mode(0o700).recursive(true).create(path)
    };

    // TODO: permissions on Windows..
    #[cfg(not(unix))]
    return { fs::create_dir_all(path) };
}

// Create a file with no permission for the group and other users, and only read permissions for
// the current user.
fn create_file(path: &path::Path) -> Result<fs::File, std::io::Error> {
    let mut options = fs::OpenOptions::new();
    let options = options.read(true).write(true).create_new(true);

    #[cfg(unix)]
    return {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o400).open(path)
    };

    #[cfg(not(unix))]
    return {
        // TODO: permissions for Windows...
        options.open(path)
    };
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

    fn mnemonics_folder(datadir_root: &path::Path, network: bitcoin::Network) -> path::PathBuf {
        [
            datadir_root,
            path::Path::new(&network.to_string()),
            path::Path::new(MNEMONICS_FOLDER_NAME),
        ]
        .iter()
        .collect()
    }

    /// Read all the mnemonics from the datadir for the given network.
    pub fn from_datadir(
        datadir_root: &path::Path,
        network: bitcoin::Network,
    ) -> Result<Vec<Self>, SignerError> {
        let mut signers = Vec::new();

        let mnemonic_paths = fs::read_dir(Self::mnemonics_folder(datadir_root, network))
            .map_err(SignerError::MnemonicStorage)?;
        for entry in mnemonic_paths {
            let mnemonic = fs::read_to_string(entry.map_err(SignerError::MnemonicStorage)?.path())
                .map_err(SignerError::MnemonicStorage)?;
            signers.push(Self::from_str(network, &mnemonic)?);
        }

        Ok(signers)
    }

    /// The BIP39 mnemonics from which the master key of this signer is derived.
    pub fn words(&self) -> [&'static str; 12] {
        let words: Vec<&'static str> = self.mnemonic.word_iter().collect();
        words.try_into().expect("Always 12 words")
    }

    /// The BIP39 mnemonic words as a string.
    pub fn mnemonic_str(&self) -> String {
        let mut mnemonic_str = String::with_capacity(12 * 7);
        let words = self.words();

        for (i, word) in words.iter().enumerate() {
            mnemonic_str += word;
            if i < words.len() - 1 {
                mnemonic_str += " ";
            }
        }

        mnemonic_str
    }

    /// Store the mnemonic in a file within the given "data directory".
    /// The file is stored within a "mnemonics" folder, with the filename set to the fingerprint of
    /// the master xpub corresponding to this mnemonic.
    pub fn store(
        &self,
        datadir_root: &path::Path,
        network: bitcoin::Network,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
    ) -> Result<(), SignerError> {
        let mut mnemonics_folder = Self::mnemonics_folder(datadir_root, network);
        if !mnemonics_folder.exists() {
            create_dir(&mnemonics_folder).map_err(SignerError::MnemonicStorage)?;
        }

        // This will fail if a file with this fingerprint exists already.
        mnemonics_folder.push(format!(
            "mnemonic-{:x}.txt",
            self.master_xpriv.fingerprint(secp)
        ));
        let mnemonic_path = mnemonics_folder;
        let mut mnemonic_file =
            create_file(&mnemonic_path).map_err(SignerError::MnemonicStorage)?;
        mnemonic_file
            .write_all(self.mnemonic_str().as_bytes())
            .map_err(SignerError::MnemonicStorage)?;

        Ok(())
    }

    /// Get the extended public key at the given derivation path.
    pub fn xpub_at(
        &self,
        der_path: &bip32::DerivationPath,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
    ) -> bip32::ExtendedPubKey {
        let xpriv = self
            .master_xpriv
            .derive_priv(secp, der_path)
            .expect("Never fails");
        bip32::ExtendedPubKey::from_priv(secp, &xpriv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::*;
    use std::collections::HashSet;

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
        let mnemonics_str = signer.mnemonic_str();
        assert_eq!(
            HotSigner::from_str(bitcoin::Network::Bitcoin, &mnemonics_str)
                .unwrap()
                .words(),
            signer.words()
        );

        // We can get an xpub for it.
        let secp = secp256k1::Secp256k1::signing_only();
        let _ = signer.xpub_at(
            &bip32::DerivationPath::from_str("m/42'/43/0987'/0/2").unwrap(),
            &secp,
        );
    }

    #[test]
    fn hot_signer_storage() {
        let secp = secp256k1::Secp256k1::signing_only();
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        let network = bitcoin::Network::Bitcoin;

        let words_set: HashSet<_> = (0..10)
            .map(|_| {
                let signer = HotSigner::generate(network).unwrap();
                signer.store(&tmp_dir, network, &secp).unwrap();
                signer.words()
            })
            .collect();
        let words_read: HashSet<_> = HotSigner::from_datadir(&tmp_dir, network)
            .unwrap()
            .into_iter()
            .map(|signer| signer.words())
            .collect();
        assert_eq!(words_set, words_read);

        fs::remove_dir_all(tmp_dir).unwrap();
    }
}
