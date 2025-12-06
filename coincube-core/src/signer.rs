//! Signer module
//!
//! Some helpers to facilitate the usage of a signer in client of the Coincube daemon. For now
//! only contains a hot signer.

use crate::random;

use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};

use argon2::{
    password_hash::{PasswordHasher, SaltString},
    Argon2,
};

use zeroize::Zeroizing;

use std::{
    convert::TryInto,
    error, fmt, fs,
    io::{self, Write},
    path,
    str::FromStr,
};

use miniscript::bitcoin::{
    self,
    bip32::{self, Error as Bip32Error, Fingerprint},
    ecdsa,
    hashes::Hash,
    key::TapTweak,
    psbt::{Input as PsbtIn, Psbt},
    secp256k1, sighash,
};

const NONCE_LEN: usize = 12; // AES-GCM standard nonce
const SALT_LEN: usize = 16;
const ENCRYPTED_FILE_MARKER: &[u8] = b"ENCRYPTED_V1"; // 12 bytes
const ENCRYPTED_FILE_MARKER_LEN: usize = 12;

/// An error related to using a signer.
#[derive(Debug)]
pub enum SignerError {
    Randomness(random::RandomnessError),
    Mnemonic(bip39::Error),
    Bip32(Bip32Error),
    MnemonicStorage(io::Error),
    InsanePsbt,
    IncompletePsbt,
    Encryption(String),
    Decryption(String),
    InvalidPassword,
}

impl fmt::Display for SignerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Randomness(s) => write!(f, "Error related to getting randomness: {}", s),
            Self::Mnemonic(s) => write!(f, "Error when working with mnemonics: {}", s),
            Self::Bip32(e) => write!(f, "BIP32 error: {}", e),
            Self::MnemonicStorage(e) => write!(f, "BIP39 mnemonic storage error: {}", e),
            Self::InsanePsbt => write!(f, "Information contained in the PSBT is wrong."),
            Self::IncompletePsbt => write!(
                f,
                "The PSBT is missing some information necessary for signing."
            ),
            Self::Encryption(e) => write!(f, "Encryption error: {}", e),
            Self::Decryption(e) => write!(f, "Decryption error: {}", e),
            Self::InvalidPassword => write!(f, "Invalid password for encrypted mnemonic"),
        }
    }
}

impl error::Error for SignerError {}

pub const MNEMONICS_FOLDER_NAME: &str = "mnemonics";

/// A signer that keeps the key on the laptop. Based on BIP39.
pub struct HotSigner {
    mnemonic: bip39::Mnemonic,
    master_xpriv: bip32::Xpriv,
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
    fs::create_dir_all(path)
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
        let master_xpriv =
            bip32::Xpriv::new_master(network, &mnemonic.to_seed("")).map_err(SignerError::Bip32)?;
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

    /// Check if a file contains an encrypted mnemonic
    fn is_encrypted(data: &[u8]) -> bool {
        data.starts_with(ENCRYPTED_FILE_MARKER)
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

    /// Read mnemonics from datadir (with optional password for encrypted files)
    pub fn from_datadir_with_password(
        datadir_root: &path::Path,
        network: bitcoin::Network,
        password: Option<&str>,
    ) -> Result<Vec<Self>, SignerError> {
        let mut signers = Vec::new();

        let mnemonics_folder = Self::mnemonics_folder(datadir_root, network);
        let mnemonic_paths =
            fs::read_dir(mnemonics_folder).map_err(SignerError::MnemonicStorage)?;

        for entry in mnemonic_paths {
            let path = entry.map_err(SignerError::MnemonicStorage)?.path();
            let data = fs::read(&path).map_err(SignerError::MnemonicStorage)?;

            let mnemonic_str = if Self::is_encrypted(&data) {
                // Encrypted file
                let pwd = password.ok_or_else(|| {
                    SignerError::Decryption("Password required for encrypted mnemonic".to_string())
                })?;
                Self::decrypt_mnemonic(&data, pwd)?
            } else {
                // Unencrypted file (backward compatibility)
                String::from_utf8(data).map_err(|e| {
                    SignerError::MnemonicStorage(io::Error::new(io::ErrorKind::InvalidData, e))
                })?
            };

            signers.push(Self::from_str(network, &mnemonic_str)?);
        }

        Ok(signers)
    }

    /// Legacy method (backward compatible)
    pub fn from_datadir(
        datadir_root: &path::Path,
        network: bitcoin::Network,
    ) -> Result<Vec<Self>, SignerError> {
        Self::from_datadir_with_password(datadir_root, network, None)
    }

    /// The BIP39 mnemonics from which the master key of this signer is derived.
    pub fn words(&self) -> [&'static str; 12] {
        let words: Vec<&'static str> = self.mnemonic.words().collect();
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

    /// Get the fingerprint of the master xpub for this signer.
    pub fn fingerprint(
        &self,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
    ) -> bip32::Fingerprint {
        self.master_xpriv.fingerprint(secp)
    }

    /// Store the mnemonic in a file within the given "data directory".
    /// The file is stored within a "mnemonics" folder, with the filename set to the fingerprint of
    /// the master xpub corresponding to this mnemonic.
    /// Store the mnemonic (encrypted if password provided)
    pub fn store_encrypted(
        &self,
        datadir_root: &path::Path,
        network: bitcoin::Network,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
        descriptor_info: Option<(String, i64)>,
        password: Option<&str>,
    ) -> Result<(), SignerError> {
        let mnemonics_folder = Self::mnemonics_folder(datadir_root, network);
        if !mnemonics_folder.exists() {
            create_dir(&mnemonics_folder).map_err(SignerError::MnemonicStorage)?;
        }

        let filename = MnemonicFileName {
            fingerprint: self.fingerprint(secp),
            descriptor_info,
        };
        let file_path = mnemonics_folder.join(filename.to_string());

        let data = if let Some(pwd) = password {
            // Encrypt the mnemonic
            self.encrypt_mnemonic(pwd)?
        } else {
            // Store unencrypted (backward compatibility)
            self.mnemonic_str().as_bytes().to_vec()
        };

        let mut mnemonic_file = create_file(&file_path).map_err(SignerError::MnemonicStorage)?;
        mnemonic_file
            .write_all(&data)
            .map_err(SignerError::MnemonicStorage)?;

        Ok(())
    }

    /// Legacy store method (unencrypted) for backward compatibility
    pub fn store(
        &self,
        datadir_root: &path::Path,
        network: bitcoin::Network,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
        descriptor_info: Option<(String, i64)>,
    ) -> Result<(), SignerError> {
        self.store_encrypted(datadir_root, network, secp, descriptor_info, None)
    }

    /// Encrypt the mnemonic using Argon2 + AES-256-GCM
    fn encrypt_mnemonic(&self, password: &str) -> Result<Vec<u8>, SignerError> {
        // Generate random salt bytes
        let mut salt_bytes = [0u8; SALT_LEN];
        OsRng.fill_bytes(&mut salt_bytes);

        // Create SaltString from the raw bytes for password hashing
        let salt = SaltString::encode_b64(&salt_bytes)
            .map_err(|e| SignerError::Encryption(e.to_string()))?;

        // Derive key from password using Argon2
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| SignerError::Encryption(e.to_string()))?;

        let hash_output = password_hash
            .hash
            .ok_or_else(|| SignerError::Encryption("Failed to derive key".to_string()))?;

        // Use Zeroizing to automatically clear key_bytes when dropped
        // Take the first 32 bytes for AES-256 (hash output is typically longer)
        let key_bytes = Zeroizing::new({
            let hash_bytes = hash_output.as_bytes();
            if hash_bytes.len() < 32 {
                return Err(SignerError::Encryption(
                    "Hash output too short for AES-256 key".to_string(),
                ));
            }
            hash_bytes[..32].to_vec()
        });

        let cipher = Aes256Gcm::new_from_slice(&key_bytes)
            .map_err(|e| SignerError::Encryption(e.to_string()))?;

        // Generate nonce
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt mnemonic - use Zeroizing for the plaintext
        let plaintext = Zeroizing::new(self.mnemonic_str());
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| SignerError::Encryption(e.to_string()))?;

        // Format: MARKER + SALT + NONCE + CIPHERTEXT
        let mut result = Vec::new();
        result.extend_from_slice(ENCRYPTED_FILE_MARKER);
        result.extend_from_slice(&salt_bytes); // Use raw salt bytes
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(result)
        // key_bytes and plaintext are automatically zeroized when dropped here
    }

    /// Decrypt a mnemonic
    fn decrypt_mnemonic(data: &[u8], password: &str) -> Result<String, SignerError> {
        // Check marker
        if !data.starts_with(ENCRYPTED_FILE_MARKER) {
            return Err(SignerError::Decryption(
                "Not an encrypted mnemonic file".to_string(),
            ));
        }

        let data = &data[ENCRYPTED_FILE_MARKER_LEN..];

        if data.len() < SALT_LEN + NONCE_LEN {
            return Err(SignerError::Decryption("Invalid file format".to_string()));
        }

        let salt_bytes = &data[..SALT_LEN];
        let nonce_bytes = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
        let ciphertext = &data[SALT_LEN + NONCE_LEN..];

        // Derive key from password
        let salt = SaltString::encode_b64(salt_bytes)
            .map_err(|e| SignerError::Decryption(e.to_string()))?;

        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|_| SignerError::InvalidPassword)?;

        let hash_output = password_hash.hash.ok_or(SignerError::InvalidPassword)?;

        // Use Zeroizing to automatically clear key_bytes when dropped
        // Take the first 32 bytes for AES-256
        let key_bytes = Zeroizing::new({
            let hash_bytes = hash_output.as_bytes();
            if hash_bytes.len() < 32 {
                return Err(SignerError::InvalidPassword);
            }
            hash_bytes[..32].to_vec()
        });

        let cipher =
            Aes256Gcm::new_from_slice(&key_bytes).map_err(|_| SignerError::InvalidPassword)?;

        // Decrypt - use Zeroizing for the plaintext bytes
        let plaintext_bytes = Zeroizing::new(
            cipher
                .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
                .map_err(|_| SignerError::InvalidPassword)?,
        );

        let result = String::from_utf8(plaintext_bytes.to_vec())
            .map_err(|e| SignerError::Decryption(e.to_string()))?;

        Ok(result)
        // key_bytes and plaintext_bytes are automatically zeroized when dropped here
    }

    fn xpriv_at(
        &self,
        der_path: &bip32::DerivationPath,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
    ) -> bip32::Xpriv {
        self.master_xpriv
            .derive_priv(secp, der_path)
            .expect("Never fails")
    }

    /// Get the extended public key at the given derivation path.
    pub fn xpub_at(
        &self,
        der_path: &bip32::DerivationPath,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
    ) -> bip32::Xpub {
        let xpriv = self.xpriv_at(der_path, secp);
        bip32::Xpub::from_priv(secp, &xpriv)
    }

    // Provide an ECDSA signature for this transaction input from the PSBT input information.
    fn sign_p2wsh(
        &self,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
        sighash_cache: &mut sighash::SighashCache<&bitcoin::Transaction>,
        master_fingerprint: bip32::Fingerprint,
        psbt_in: &mut PsbtIn,
        input_index: usize,
    ) -> Result<(), SignerError> {
        // First of all compute the sighash for this input. We assume P2WSH spend: the sighash
        // script code is always the witness script.
        let witscript = psbt_in
            .witness_script
            .as_ref()
            .ok_or(SignerError::IncompletePsbt)?;
        let value = psbt_in
            .witness_utxo
            .as_ref()
            .ok_or(SignerError::IncompletePsbt)?
            .value;
        let sighash_type = sighash::EcdsaSighashType::All;
        let sighash = sighash_cache
            .p2wsh_signature_hash(input_index, witscript, value, sighash_type)
            .map_err(|_| SignerError::InsanePsbt)?;
        let sighash = secp256k1::Message::from_digest_slice(sighash.as_byte_array())
            .expect("Sighash is always 32 bytes.");

        // Then provide a signature for all the keys they asked for.
        for (curr_pubkey, (fingerprint, der_path)) in psbt_in.bip32_derivation.iter() {
            if *fingerprint != master_fingerprint {
                continue;
            }
            let privkey = self.xpriv_at(der_path, secp).to_priv();
            let pubkey = privkey.public_key(secp);
            if pubkey.inner != *curr_pubkey {
                return Err(SignerError::InsanePsbt);
            }
            let signature = secp.sign_ecdsa_low_r(&sighash, &privkey.inner);
            psbt_in.partial_sigs.insert(
                pubkey,
                ecdsa::Signature {
                    signature,
                    sighash_type,
                },
            );
        }

        Ok(())
    }

    // Provide a BIP340 signature for this transaction input from the PSBT input information.
    fn sign_taproot(
        &self,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
        sighash_cache: &mut sighash::SighashCache<&bitcoin::Transaction>,
        master_fingerprint: bip32::Fingerprint,
        prevouts: &[bitcoin::TxOut],
        psbt_in: &mut PsbtIn,
        input_index: usize,
    ) -> Result<(), SignerError> {
        let sighash_type = sighash::TapSighashType::Default;
        let prevouts = sighash::Prevouts::All(prevouts);

        // If the details of the internal key are filled, provide a keypath signature.
        if let Some(ref int_key) = psbt_in.tap_internal_key {
            // NB: we don't check for empty leaf hashes on purpose, in case the internal key also
            // appears in a leaf.
            if let Some((_, (fg, der_path))) = psbt_in.tap_key_origins.get(int_key) {
                if *fg == master_fingerprint {
                    let privkey = self.xpriv_at(der_path, secp).to_priv();
                    let keypair = secp256k1::Keypair::from_secret_key(secp, &privkey.inner);
                    if keypair.x_only_public_key().0 != *int_key {
                        return Err(SignerError::InsanePsbt);
                    }
                    let keypair = keypair
                        .tap_tweak(secp, psbt_in.tap_merkle_root)
                        .to_keypair();
                    let sighash = sighash_cache
                        .taproot_key_spend_signature_hash(input_index, &prevouts, sighash_type)
                        .map_err(|_| SignerError::InsanePsbt)?;
                    let sighash = secp256k1::Message::from_digest_slice(sighash.as_byte_array())
                        .expect("Sighash is always 32 bytes.");
                    let signature = secp.sign_schnorr_no_aux_rand(&sighash, &keypair);
                    let sig = bitcoin::taproot::Signature {
                        signature,
                        sighash_type,
                    };
                    psbt_in.tap_key_sig = Some(sig);
                }
            }
        }

        // Now sign for all the public keys derived from our master secret, in all the leaves where
        // they are present.
        for (pubkey, (leaf_hashes, (fg, der_path))) in &psbt_in.tap_key_origins {
            if *fg != master_fingerprint {
                continue;
            }

            for leaf_hash in leaf_hashes {
                let privkey = self.xpriv_at(der_path, secp).to_priv();
                let keypair = secp256k1::Keypair::from_secret_key(secp, &privkey.inner);
                let sighash = sighash_cache
                    .taproot_script_spend_signature_hash(
                        input_index,
                        &prevouts,
                        *leaf_hash,
                        sighash_type,
                    )
                    .map_err(|_| SignerError::InsanePsbt)?;
                let sighash = secp256k1::Message::from_digest_slice(sighash.as_byte_array())
                    .expect("Sighash is always 32 bytes.");
                let signature = secp.sign_schnorr_no_aux_rand(&sighash, &keypair);
                let sig = bitcoin::taproot::Signature {
                    signature,
                    sighash_type,
                };
                psbt_in.tap_script_sigs.insert((*pubkey, *leaf_hash), sig);
            }
        }

        Ok(())
    }

    /// Sign all inputs of the given PSBT.
    ///
    /// **This does not perform any check. It will blindly sign anything that's passed.**
    pub fn sign_psbt(
        &self,
        mut psbt: Psbt,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
    ) -> Result<Psbt, SignerError> {
        let master_fingerprint = self.fingerprint(secp);
        let mut sighash_cache = sighash::SighashCache::new(&psbt.unsigned_tx);

        let prevouts: Vec<_> = psbt
            .inputs
            .iter()
            .filter_map(|psbt_in| psbt_in.witness_utxo.clone())
            .collect();
        if prevouts.len() != psbt.inputs.len() {
            return Err(SignerError::IncompletePsbt);
        }

        // Sign each input in the PSBT.
        for i in 0..psbt.inputs.len() {
            if psbt.inputs[i].witness_script.is_some() {
                self.sign_p2wsh(
                    secp,
                    &mut sighash_cache,
                    master_fingerprint,
                    &mut psbt.inputs[i],
                    i,
                )?;
            } else {
                self.sign_taproot(
                    secp,
                    &mut sighash_cache,
                    master_fingerprint,
                    &prevouts,
                    &mut psbt.inputs[i],
                    i,
                )?;
            }
        }

        Ok(psbt)
    }

    /// Change the network of generated extended keys. Note this value only has to do with the
    /// BIP32 encoding of those keys (xpubs, tpubs, ..) but does not affect any data (whether it is
    /// the keys or the mnemonics).
    pub fn set_network(&mut self, network: bitcoin::Network) {
        self.master_xpriv.network = network.into();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MnemonicFileName {
    pub fingerprint: Fingerprint,
    pub descriptor_info: Option<(String, i64)>, // (descriptor_checksum, timestamp)
}

impl fmt::Display for MnemonicFileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.descriptor_info {
            Some((checksum, timestamp)) => {
                write!(
                    f,
                    "mnemonic-{}-{}-{}.txt",
                    self.fingerprint, checksum, timestamp
                )
            }
            None => {
                write!(f, "mnemonic-{}.txt", self.fingerprint)
            }
        }
    }
}

#[derive(Debug)]
pub enum MnemonicFileNameError {
    InvalidFormat,
    InvalidFingerprint,
    InvalidTimestamp,
}

impl fmt::Display for MnemonicFileNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MnemonicFileNameError::InvalidFormat => write!(f, "Invalid mnemonic file name format"),
            MnemonicFileNameError::InvalidFingerprint => write!(f, "Invalid fingerprint format"),
            MnemonicFileNameError::InvalidTimestamp => write!(f, "Invalid timestamp format"),
        }
    }
}

impl std::error::Error for MnemonicFileNameError {}

// Implementation of FromStr for MnemonicFileName
impl FromStr for MnemonicFileName {
    type Err = MnemonicFileNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Check if the string starts with "mnemonic-" and ends with ".txt"
        if !s.starts_with("mnemonic-") || !s.ends_with(".txt") {
            return Err(MnemonicFileNameError::InvalidFormat);
        }

        let content = s
            .strip_prefix("mnemonic-")
            .expect("Already checked")
            .strip_suffix(".txt")
            .expect("Already checked");

        let parts: Vec<&str> = content.split('-').collect();
        match parts.len() {
            1 => {
                // Only fingerprint
                let fingerprint = Fingerprint::from_str(parts[0])
                    .map_err(|_| MnemonicFileNameError::InvalidFingerprint)?;

                Ok(MnemonicFileName {
                    fingerprint,
                    descriptor_info: None,
                })
            }
            3 => {
                // Fingerprint + checksum + timestamp
                let fingerprint = Fingerprint::from_str(parts[0])
                    .map_err(|_| MnemonicFileNameError::InvalidFingerprint)?;

                let timestamp = parts[2]
                    .parse::<i64>()
                    .map_err(|_| MnemonicFileNameError::InvalidTimestamp)?;

                Ok(MnemonicFileName {
                    fingerprint,
                    descriptor_info: Some((parts[1].to_string(), timestamp)),
                })
            }
            _ => Err(MnemonicFileNameError::InvalidFormat),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptors;
    use miniscript::{
        bitcoin::{locktime::absolute, psbt::Input as PsbtIn, Amount},
        descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, Wildcard},
    };
    use std::collections::{BTreeMap, HashSet};

    fn uid() -> usize {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn tmp_dir() -> path::PathBuf {
        std::env::temp_dir().join(format!(
            "coincubed-{}-{:?}-{}",
            std::process::id(),
            std::thread::current().id(),
            uid(),
        ))
    }

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
                signer.store(&tmp_dir, network, &secp, None).unwrap();
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

    #[test]
    fn hot_signer_sign_p2wsh() {
        let secp = secp256k1::Secp256k1::new();
        let network = bitcoin::Network::Bitcoin;

        // Create a Coincube descriptor with as primary path a 2-of-3 with three hot signers and a
        // single hot signer as recovery path. (The recovery path signer is also used in the
        // primary path.) Use various random derivation paths.
        let (prim_signer_a, prim_signer_b, recov_signer) = (
            HotSigner::generate(network).unwrap(),
            HotSigner::generate(network).unwrap(),
            HotSigner::generate(network).unwrap(),
        );
        let origin_der = bip32::DerivationPath::from_str("m/0'/12'/42").unwrap();
        let xkey = prim_signer_a.xpub_at(&origin_der, &secp);
        let prim_key_a = DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: Some((prim_signer_a.fingerprint(&secp), origin_der)),
            xkey,
            derivation_paths: DerivPaths::new(vec![
                bip32::DerivationPath::from_str("m/420/56/0").unwrap(),
                bip32::DerivationPath::from_str("m/420/56/1").unwrap(),
            ])
            .unwrap(),
            wildcard: Wildcard::Unhardened,
        });
        let origin_der = bip32::DerivationPath::from_str("m/18'/24'").unwrap();
        let xkey = prim_signer_b.xpub_at(&origin_der, &secp);
        let prim_key_b = DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: Some((prim_signer_b.fingerprint(&secp), origin_der)),
            xkey,
            derivation_paths: DerivPaths::new(vec![
                bip32::DerivationPath::from_str("m/31/0").unwrap(),
                bip32::DerivationPath::from_str("m/31/1").unwrap(),
            ])
            .unwrap(),
            wildcard: Wildcard::Unhardened,
        });
        let origin_der = bip32::DerivationPath::from_str("m/18'/25'").unwrap();
        let xkey = recov_signer.xpub_at(&origin_der, &secp);
        let prim_key_c = DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: Some((recov_signer.fingerprint(&secp), origin_der)),
            xkey,
            derivation_paths: DerivPaths::new(vec![
                bip32::DerivationPath::from_str("m/0").unwrap(),
                bip32::DerivationPath::from_str("m/1").unwrap(),
            ])
            .unwrap(),
            wildcard: Wildcard::Unhardened,
        });
        let prim_keys = descriptors::PathInfo::Multi(2, vec![prim_key_a, prim_key_b, prim_key_c]);
        let origin_der = bip32::DerivationPath::from_str("m/1/2'/3/4'").unwrap();
        let xkey = recov_signer.xpub_at(&origin_der, &secp);
        let recov_key = DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: Some((recov_signer.fingerprint(&secp), origin_der)),
            xkey,
            derivation_paths: DerivPaths::new(vec![
                bip32::DerivationPath::from_str("m/5/6/0").unwrap(),
                bip32::DerivationPath::from_str("m/5/6/1").unwrap(),
            ])
            .unwrap(),
            wildcard: Wildcard::Unhardened,
        });
        let recov_keys = descriptors::PathInfo::Single(recov_key);
        let policy = descriptors::CoincubePolicy::new_legacy(
            prim_keys,
            [(46, recov_keys)].iter().cloned().collect(),
        )
        .unwrap();
        let desc = descriptors::CoincubeDescriptor::new(policy);

        // Create a dummy PSBT spending a coin from this descriptor with a single input and single
        // (external) output. We'll be modifying it as we go.
        let spent_coin_desc = desc.receive_descriptor().derive(42.into(), &secp);
        let mut psbt_in = PsbtIn::default();
        spent_coin_desc.update_psbt_in(&mut psbt_in);
        psbt_in.witness_utxo = Some(bitcoin::TxOut {
            value: Amount::from_sat(19_000),
            script_pubkey: spent_coin_desc.script_pubkey(),
        });
        let mut dummy_psbt = Psbt {
            unsigned_tx: bitcoin::Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
                input: vec![bitcoin::TxIn {
                    sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                    previous_output: bitcoin::OutPoint::from_str(
                        "4613e078e4cdbb0fce1bc6e44b028f0e11621a134a1605efdc456c32d155c922:19",
                    )
                    .unwrap(),
                    ..bitcoin::TxIn::default()
                }],
                output: vec![bitcoin::TxOut {
                    value: Amount::from_sat(18_420),
                    script_pubkey: bitcoin::Address::from_str(
                        "bc1qvklensptw5lk7d470ds60pcpsr0psdpgyvwepv",
                    )
                    .unwrap()
                    .assume_checked()
                    .script_pubkey(),
                }],
            },
            version: 0,
            xpub: BTreeMap::new(),
            proprietary: BTreeMap::new(),
            unknown: BTreeMap::new(),
            inputs: vec![psbt_in],
            outputs: Vec::new(),
        };

        // Sign the PSBT with the two primary signers. The recovery signer will sign for the two keys
        // that it manages.
        let psbt = dummy_psbt.clone();
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 1);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 2);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 4);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());

        // We can add another external output to the transaction, we can still sign without issue.
        // The output can be insane, we don't check it. It doesn't even need an accompanying PSBT
        // output.
        dummy_psbt.unsigned_tx.output.push(bitcoin::TxOut::NULL);
        let psbt = dummy_psbt.clone();
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 1);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 2);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 4);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());

        // We can add another input to the PSBT. If we don't attach also another transaction input
        // it will fail.
        let other_spent_coin_desc = desc.receive_descriptor().derive(84.into(), &secp);
        let mut psbt_in = PsbtIn::default();
        other_spent_coin_desc.update_psbt_in(&mut psbt_in);
        psbt_in.witness_utxo = Some(bitcoin::TxOut {
            value: Amount::from_sat(19_000),
            script_pubkey: other_spent_coin_desc.script_pubkey(),
        });
        dummy_psbt.inputs.push(psbt_in);
        let psbt = dummy_psbt.clone();
        assert!(prim_signer_a
            .sign_psbt(psbt, &secp)
            .unwrap_err()
            .to_string()
            .contains("Information contained in the PSBT is wrong"));

        // But now if we add the inputs also to the transaction itself, it will have signed both
        // inputs.
        dummy_psbt.unsigned_tx.input.push(bitcoin::TxIn {
            // Note the sequence can be different. We don't care.
            sequence: bitcoin::Sequence::ENABLE_LOCKTIME_NO_RBF,
            previous_output: bitcoin::OutPoint::from_str(
                "5613e078e4cdbb0fce1bc6e44b028f0e11621a134a1605efdc456c32d155c922:0",
            )
            .unwrap(),
            ..bitcoin::TxIn::default()
        });
        let psbt = dummy_psbt.clone();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 2));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 4));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));

        // If the witness script is missing for one of the inputs it'll assume it's a Taproot input
        // and provide Taproot signatures. But since we haven't provided any Taproot details it
        // won't fill anything.
        let mut psbt = dummy_psbt.clone();
        psbt.inputs[1].witness_script = None;
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt.inputs[1].partial_sigs.is_empty());
        assert!(psbt.inputs[1].tap_key_sig.is_none());
        assert!(psbt.inputs[1].tap_script_sigs.is_empty());

        // If the witness utxo is missing for one of the inputs it'll tell us the PSBT is
        // incomplete.
        let mut psbt = dummy_psbt.clone();
        psbt.inputs[1].witness_utxo = None;
        assert!(prim_signer_a
            .sign_psbt(psbt, &secp)
            .unwrap_err()
            .to_string()
            .contains("The PSBT is missing some information necessary for signing."));

        // If we remove the BIP32 derivations for the first input it will only provide signatures
        // for the second one.
        let mut psbt = dummy_psbt.clone();
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        assert!(psbt.inputs[1].partial_sigs.is_empty());
        psbt.inputs[0].bip32_derivation.clear();
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        assert_eq!(psbt.inputs[1].partial_sigs.len(), 1);
    }

    #[test]
    fn hot_signer_sign_taproot() {
        let secp = secp256k1::Secp256k1::new();
        let network = bitcoin::Network::Bitcoin;

        // Create a Coincube descriptor with as primary path a 2-of-3 with three hot signers and a
        // single hot signer as recovery path. (The recovery path signer is also used in the
        // primary path.) Use various random derivation paths.
        let (prim_signer_a, prim_signer_b, recov_signer) = (
            HotSigner::generate(network).unwrap(),
            HotSigner::generate(network).unwrap(),
            HotSigner::generate(network).unwrap(),
        );
        let origin_der = bip32::DerivationPath::from_str("m/0'/12'/42").unwrap();
        let xkey = prim_signer_a.xpub_at(&origin_der, &secp);
        let prim_key_a = DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: Some((prim_signer_a.fingerprint(&secp), origin_der)),
            xkey,
            derivation_paths: DerivPaths::new(vec![
                bip32::DerivationPath::from_str("m/420/56/0").unwrap(),
                bip32::DerivationPath::from_str("m/420/56/1").unwrap(),
            ])
            .unwrap(),
            wildcard: Wildcard::Unhardened,
        });
        let origin_der = bip32::DerivationPath::from_str("m/18'/24'").unwrap();
        let xkey = prim_signer_b.xpub_at(&origin_der, &secp);
        let prim_key_b = DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: Some((prim_signer_b.fingerprint(&secp), origin_der)),
            xkey,
            derivation_paths: DerivPaths::new(vec![
                bip32::DerivationPath::from_str("m/31/0").unwrap(),
                bip32::DerivationPath::from_str("m/31/1").unwrap(),
            ])
            .unwrap(),
            wildcard: Wildcard::Unhardened,
        });
        let origin_der = bip32::DerivationPath::from_str("m/18'/25'").unwrap();
        let xkey = recov_signer.xpub_at(&origin_der, &secp);
        let prim_key_c = DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: Some((recov_signer.fingerprint(&secp), origin_der)),
            xkey,
            derivation_paths: DerivPaths::new(vec![
                bip32::DerivationPath::from_str("m/0").unwrap(),
                bip32::DerivationPath::from_str("m/1").unwrap(),
            ])
            .unwrap(),
            wildcard: Wildcard::Unhardened,
        });
        let prim_keys =
            descriptors::PathInfo::Multi(2, vec![prim_key_a.clone(), prim_key_b, prim_key_c]);
        let origin_der = bip32::DerivationPath::from_str("m/1/2'/3/4'").unwrap();
        let xkey = recov_signer.xpub_at(&origin_der, &secp);
        let recov_key = DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
            origin: Some((recov_signer.fingerprint(&secp), origin_der)),
            xkey,
            derivation_paths: DerivPaths::new(vec![
                bip32::DerivationPath::from_str("m/5/6/0").unwrap(),
                bip32::DerivationPath::from_str("m/5/6/1").unwrap(),
            ])
            .unwrap(),
            wildcard: Wildcard::Unhardened,
        });
        let recov_keys = descriptors::PathInfo::Single(recov_key.clone());
        let policy = descriptors::CoincubePolicy::new(
            prim_keys,
            [(46, recov_keys)].iter().cloned().collect(),
        )
        .unwrap();
        let desc = descriptors::CoincubeDescriptor::new(policy);

        // Create a dummy PSBT spending a coin from this descriptor with a single input and single
        // (external) output. We'll be modifying it as we go.
        let spent_coin_desc = desc.receive_descriptor().derive(42.into(), &secp);
        let mut psbt_in = PsbtIn::default();
        spent_coin_desc.update_psbt_in(&mut psbt_in);
        psbt_in.witness_utxo = Some(bitcoin::TxOut {
            value: Amount::from_sat(19_000),
            script_pubkey: spent_coin_desc.script_pubkey(),
        });
        let mut dummy_psbt = Psbt {
            unsigned_tx: bitcoin::Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
                input: vec![bitcoin::TxIn {
                    sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                    previous_output: bitcoin::OutPoint::from_str(
                        "4613e078e4cdbb0fce1bc6e44b028f0e11621a134a1605efdc456c32d155c922:19",
                    )
                    .unwrap(),
                    ..bitcoin::TxIn::default()
                }],
                output: vec![bitcoin::TxOut {
                    value: Amount::from_sat(18_420),
                    script_pubkey: bitcoin::Address::from_str(
                        "bc1qvklensptw5lk7d470ds60pcpsr0psdpgyvwepv",
                    )
                    .unwrap()
                    .assume_checked()
                    .script_pubkey(),
                }],
            },
            version: 0,
            xpub: BTreeMap::new(),
            proprietary: BTreeMap::new(),
            unknown: BTreeMap::new(),
            inputs: vec![psbt_in],
            outputs: Vec::new(),
        };

        // Sign the PSBT with the two primary signers. The recovery signer will sign for the two keys
        // that it manages.
        let psbt = dummy_psbt.clone();
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 1);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 2);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 4);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].partial_sigs.is_empty());

        // We can add another external output to the transaction, we can still sign without issue.
        // The output can be insane, we don't check it. It doesn't even need an accompanying PSBT
        // output.
        dummy_psbt.unsigned_tx.output.push(bitcoin::TxOut::NULL);
        let psbt = dummy_psbt.clone();
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 1);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 2);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].tap_script_sigs.len(), 4);
        assert!(psbt.inputs[0].tap_key_sig.is_none());
        assert!(psbt.inputs[0].partial_sigs.is_empty());

        // We can add another input to the PSBT. If we don't attach also another transaction input
        // it will fail.
        let other_spent_coin_desc = desc.receive_descriptor().derive(84.into(), &secp);
        let mut psbt_in = PsbtIn::default();
        other_spent_coin_desc.update_psbt_in(&mut psbt_in);
        psbt_in.witness_utxo = Some(bitcoin::TxOut {
            value: Amount::from_sat(19_000),
            script_pubkey: other_spent_coin_desc.script_pubkey(),
        });
        dummy_psbt.inputs.push(psbt_in);
        let psbt = dummy_psbt.clone();
        assert!(prim_signer_a
            .sign_psbt(psbt, &secp)
            .unwrap_err()
            .to_string()
            .contains("Information contained in the PSBT is wrong"));

        // But now if we add the inputs also to the transaction itself, it will have signed both
        // inputs.
        dummy_psbt.unsigned_tx.input.push(bitcoin::TxIn {
            // Note the sequence can be different. We don't care.
            sequence: bitcoin::Sequence::ENABLE_LOCKTIME_NO_RBF,
            previous_output: bitcoin::OutPoint::from_str(
                "5613e078e4cdbb0fce1bc6e44b028f0e11621a134a1605efdc456c32d155c922:0",
            )
            .unwrap(),
            ..bitcoin::TxIn::default()
        });
        let psbt = dummy_psbt.clone();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.len() == 1));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.len() == 2));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.len() == 4));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));

        // If the witness script is set it'll assume it's a P2WSH input and provide ECDSA sigs.
        // But since we haven't provided any P2WSH details it won't fill anything.
        let mut psbt = dummy_psbt.clone();
        psbt.inputs[1].witness_script = Some(Default::default());
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt.inputs[1].partial_sigs.is_empty());
        assert!(psbt.inputs[1].tap_key_sig.is_none());
        assert!(psbt.inputs[1].tap_script_sigs.is_empty());

        // If the witness utxo is missing for one of the inputs it'll tell us the PSBT is
        // incomplete.
        let mut psbt = dummy_psbt.clone();
        psbt.inputs[1].witness_utxo = None;
        assert!(prim_signer_a
            .sign_psbt(psbt, &secp)
            .unwrap_err()
            .to_string()
            .contains("The PSBT is missing some information necessary for signing."));

        // If we remove the BIP32 derivations for the first input it will only provide signatures
        // for the second one.
        let mut psbt = dummy_psbt.clone();
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        assert!(psbt.inputs[1].tap_script_sigs.is_empty());
        psbt.inputs[0].tap_key_origins.clear();
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt.inputs[0].tap_script_sigs.is_empty());
        assert_eq!(psbt.inputs[1].tap_script_sigs.len(), 1);

        // Now use a Taproot descriptor such as there is a single primary key as the internal key.
        let prim_keys = descriptors::PathInfo::Single(prim_key_a);
        let recov_keys = descriptors::PathInfo::Single(recov_key);
        let policy = descriptors::CoincubePolicy::new(
            prim_keys,
            [(42, recov_keys)].iter().cloned().collect(),
        )
        .unwrap();
        let desc = descriptors::CoincubeDescriptor::new(policy);
        let spent_coin_desc = desc.receive_descriptor().derive(412.into(), &secp);

        // Update the two inputs with the details for this descriptor.
        dummy_psbt.inputs[0].tap_key_origins.clear();
        spent_coin_desc.update_psbt_in(&mut dummy_psbt.inputs[0]);
        dummy_psbt.inputs[1].tap_key_origins.clear();
        spent_coin_desc.update_psbt_in(&mut dummy_psbt.inputs[1]);

        // Sign the PSBT with the primary and recovery signers. The prim signer will add a sig for
        // the key path and the recov signer for the script path.
        let psbt = dummy_psbt.clone();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_none()));
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_some()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.is_empty()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.is_empty()));
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_key_sig.is_some()));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.tap_script_sigs.len() == 1));
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.is_empty()));
    }

    #[test]
    fn signer_set_net() {
        let secp = secp256k1::Secp256k1::signing_only();
        let mut signer = HotSigner::from_str(
            bitcoin::Network::Bitcoin,
            "burger ball theme dog light account produce chest warrior swarm flip equip",
        )
        .unwrap();
        assert_eq!(signer.xpub_at(&bip32::DerivationPath::master(), &secp).to_string(), "xpub661MyMwAqRbcGKvR8dChsA92AHfJS6fJMR41jAASu5S79v65dac244iBd7PwqnfMQ9jWsmg8SqnNz3MjkwYF8Edzr2ttxt171Cr5RyJrvF2");

        let tpub = "tpubD6NzVbkrYhZ4Y87GapBo55UPVQkxRVAMu3eK5iDbEzBzuCknhoT7CWP1s9UjNHcbC4GRVMBzywcRgDrM9oPV1g6HudeCeQfLbASVBxpNJV3";
        for net in &[
            bitcoin::Network::Testnet,
            bitcoin::Network::Signet,
            bitcoin::Network::Regtest,
        ] {
            signer.set_network(*net);
            assert_eq!(
                signer
                    .xpub_at(&bip32::DerivationPath::master(), &secp)
                    .to_string(),
                tpub
            );
        }
    }

    #[test]
    fn test_mnemonic_filename() {
        // Test to_string with descriptor info
        let fingerprint = Fingerprint::from_str("abcd1234").unwrap();
        let filename_with_info = MnemonicFileName {
            fingerprint,
            descriptor_info: Some(("def456".to_string(), 1620000000)),
        };

        assert_eq!(
            filename_with_info.to_string(),
            "mnemonic-abcd1234-def456-1620000000.txt"
        );

        // Test to_string without descriptor info
        let filename_without_info = MnemonicFileName {
            fingerprint,
            descriptor_info: None,
        };

        assert_eq!(filename_without_info.to_string(), "mnemonic-abcd1234.txt");

        // Test from_str with descriptor info
        let input_with_info = "mnemonic-abcd1234-def456-1620000000.txt";
        let parsed_with_info = MnemonicFileName::from_str(input_with_info).unwrap();

        assert_eq!(parsed_with_info.fingerprint, fingerprint);
        assert_eq!(
            parsed_with_info.descriptor_info,
            Some(("def456".to_string(), 1620000000))
        );

        // Test from_str without descriptor info
        let input_without_info = "mnemonic-abcd1234.txt";
        let parsed_without_info = MnemonicFileName::from_str(input_without_info).unwrap();

        assert_eq!(parsed_without_info.fingerprint, fingerprint);
        assert_eq!(parsed_without_info.descriptor_info, None);

        // Test roundtrip with descriptor info
        let roundtrip_with_info =
            MnemonicFileName::from_str(&filename_with_info.to_string()).unwrap();
        assert_eq!(filename_with_info, roundtrip_with_info);

        // Test roundtrip without descriptor info
        let roundtrip_without_info =
            MnemonicFileName::from_str(&filename_without_info.to_string()).unwrap();
        assert_eq!(filename_without_info, roundtrip_without_info);

        // Test error cases

        // Missing prefix
        assert!(MnemonicFileName::from_str("abcd1234.txt").is_err());

        // Missing suffix
        assert!(MnemonicFileName::from_str("mnemonic-abcd1234").is_err());

        // Wrong number of parts
        assert!(MnemonicFileName::from_str("mnemonic-abcd1234-def456.txt").is_err());

        // Invalid fingerprint (assuming Fingerprint::from_str fails for "invalid")
        assert!(MnemonicFileName::from_str("mnemonic-invalid-def456-1620000000.txt").is_err());

        // Invalid timestamp
        assert!(MnemonicFileName::from_str("mnemonic-abcd1234-def456-notanumber.txt").is_err());
    }
}
