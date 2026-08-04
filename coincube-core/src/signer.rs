//! Signer module
//!
//! Some helpers to facilitate the usage of a signer in client of the Coincube daemon. For now
//! only contains a master signer.

use crate::random;
use crate::seed_crypt;

use zeroize::Zeroizing;

use std::{
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

// The seed file's wire format, AEAD, and KDF live in [`crate::seed_crypt`].
// Both the seed file and the duress marker go through that one codec so they
// cost exactly the same to try — see PLAN-cube-unlock-hardening, invariant I2.

/// HKDF salt for WebAuthn-PRF-derived master seeds.
///
/// **Treat as stable.** This is a wire constant: every passkey Cube's master
/// key depends on it, so changing it silently orphans every such Cube. It is
/// registered in the PRF domain registry alongside Keychain's, and the two are
/// deliberately different — see [`MasterSigner::from_prf_output`].
pub const PRF_HKDF_SALT: &[u8] = b"coincube-tenshu/v1";

/// HKDF info string for WebAuthn-PRF-derived master seeds. **Treat as stable**
/// for the same reason as [`PRF_HKDF_SALT`].
pub const PRF_HKDF_INFO: &[u8] = b"master-seed-entropy/v1";

/// An error related to using a signer.
#[derive(Debug)]
pub enum SignerError {
    Randomness(random::RandomnessError),
    Mnemonic(bip39::Error),
    Bip32(Bip32Error),
    MnemonicStorage(io::Error),
    InsanePsbt,
    IncompletePsbt,
    SignerNotFound(Fingerprint),
    // Encryption specific errors
    SaltEncodingError(String),
    ArgonParamsError(String),
    PasswordHashError(String),
    KeyDerivationFailed,
    CipherCreationError(String),
    EncryptionFailed(String),

    //Decryption specific errors
    NotEncryptedFile,
    /// The file is `ENCRYPTED_V3` but no device secret was supplied. This is
    /// **not** a wrong PIN, and must never be reported as one — a user whose
    /// system keychain is locked, or whose keyring entry has been removed, has
    /// a very different problem and a very different remedy (invariant I7).
    DeviceSecretRequired,
    InvalidFileFormat,
    DecryptionFailed(String),
    InvalidPassword,
    // Password requirement
    PasswordRequired,
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
            Self::SignerNotFound(fp) => write!(f, "Signer with fingerprint {} not found", fp),
            // Encryption Errors
            Self::SaltEncodingError(e) => write!(f, "Failed to encode salt: {}", e),
            Self::ArgonParamsError(e) => write!(f, "Invalid Argon2 parameters: {}", e),
            Self::PasswordHashError(e) => write!(f, "Failed to hash password: {}", e),
            Self::KeyDerivationFailed => write!(f, "Failed to derive encryption key"),
            Self::CipherCreationError(e) => write!(f, "Failed to create cipher: {}", e),
            Self::EncryptionFailed(e) => write!(f, "Failed to encrypt mnemonic: {}", e),

            // Decryption errors
            Self::NotEncryptedFile => write!(f, "Not an encrypted mnemonic file"),
            Self::DeviceSecretRequired => write!(
                f,
                "This Cube's seed is sealed to this device's system keychain, \
                 which wasn't available"
            ),
            Self::InvalidFileFormat => write!(f, "Invalid encrypted file format"),
            Self::DecryptionFailed(e) => write!(f, "Failed to decrypt mnemonic: {}", e),
            Self::InvalidPassword => write!(f, "Invalid password for encrypted mnemonic"),
            // Password required errors
            Self::PasswordRequired => write!(f, "Password required for encrypted mnemonic"),
        }
    }
}

impl error::Error for SignerError {}

pub const MNEMONICS_FOLDER_NAME: &str = "mnemonics";
/// Label embedded in the checksum portion of mnemonic filenames for master seeds.
pub const MASTER_SEED_LABEL: &str = "master_";
/// Legacy label kept for backward-compat reading of old Liquid-signer files.
pub const LEGACY_LIQUID_SEED_LABEL: &str = "liquid_";

/// A signer that keeps the key on the laptop. Based on BIP39.
///
/// Both secrets it holds are scrubbed on drop: `bip39::Mnemonic` via the crate's
/// `zeroize` feature, and the derived `Xpriv` by the [`Drop`] impl below.
/// Neither happened before — a dropped signer left a full seed phrase and a
/// master private key on the residual heap for the rest of the process's life,
/// and the app constructs signers on every Cube open.
pub struct MasterSigner {
    mnemonic: bip39::Mnemonic,
    master_xpriv: bip32::Xpriv,
    network: bitcoin::Network,
}

/// Overwrite an extended private key in place.
///
/// Split out of [`Drop`] so it can be tested: the effect of a `Drop` impl is by
/// definition unobservable afterwards, and the bug this replaced was invisible
/// for exactly that reason.
///
/// `SecretKey::non_secure_erase` overwrites the key through
/// `ptr::write_volatile` followed by a `SeqCst` compiler fence — the same
/// technique `zeroize` uses, so the write survives optimisation. It is called
/// "non-secure" because it cannot undo copies the compiler may already have
/// made elsewhere, which is inherent and not something any API can fix.
fn scrub_xpriv(xpriv: &mut bip32::Xpriv) {
    xpriv.private_key.non_secure_erase();
    // The chain code is not secret on its own — it is half of the *xpub* — so
    // this is tidiness rather than a control, and a plain assignment is enough.
    // The private key above is the part that matters.
    xpriv.chain_code = bip32::ChainCode::from([0u8; 32]);
}

impl Drop for MasterSigner {
    fn drop(&mut self) {
        scrub_xpriv(&mut self.master_xpriv);
        // `bip39::Mnemonic` scrubs itself (the crate's `zeroize` feature).
    }
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

    #[cfg(windows)]
    return {
        // Windows has no mode bits. The datadir already lives under the user's
        // profile, whose ACL is owner-only by default, so the meaningful thing
        // left is to mark the file read-only: it stops an in-place edit of the
        // seed file (the AAD binding would reject a tampered one anyway, but a
        // rewrite that destroys it is still a way to lose a wallet).
        //
        // Setting the attribute has to happen *after* creation — a read-only
        // file cannot be opened for writing — so the caller writes first and
        // the attribute is applied by `restrict_permissions` afterwards. This
        // arm therefore just creates the file.
        options.open(path)
    };

    #[cfg(not(any(unix, windows)))]
    return options.open(path);
}

/// Tighten permissions on a file that already exists and has been written.
///
/// Unix files are created 0o400 by [`create_file`] and need nothing more; this
/// is where Windows gets its read-only attribute, which cannot be applied at
/// creation time without making the file unwritable.
pub fn restrict_permissions(path: &path::Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let meta = fs::metadata(path)?;
        let mut perms = meta.permissions();
        perms.set_readonly(true);
        fs::set_permissions(path, perms)?;
    }
    #[cfg(not(windows))]
    let _ = path;
    Ok(())
}

impl MasterSigner {
    pub fn from_mnemonic(
        network: bitcoin::Network,
        mnemonic: bip39::Mnemonic,
    ) -> Result<Self, SignerError> {
        let master_xpriv =
            bip32::Xpriv::new_master(network, &mnemonic.to_seed("")).map_err(SignerError::Bip32)?;
        Ok(Self {
            mnemonic,
            master_xpriv,
            network,
        })
    }

    /// Create a new master signer from random bytes. Uses a 12-words mnemonics without a passphrase.
    pub fn generate(network: bitcoin::Network) -> Result<Self, SignerError> {
        // We want a 12-words mnemonic so we only use 16 of the 32 bytes.
        let random_32bytes = random::random_bytes().map_err(SignerError::Randomness)?;
        let mnemonic =
            bip39::Mnemonic::from_entropy(&random_32bytes[..16]).map_err(SignerError::Mnemonic)?;
        Self::from_mnemonic(network, mnemonic)
    }

    /// Create a MasterSigner from a 32-byte WebAuthn PRF extension output.
    ///
    /// The PRF output is run through HKDF-SHA256 with a Coincube-specific salt
    /// and info string, and the full 32-byte result becomes BIP39 entropy — a
    /// deterministic 24-word mnemonic. The same PRF output always yields the
    /// same mnemonic and master key.
    ///
    /// # Why not `from_entropy(&prf_output[..16])`
    ///
    /// That was the original implementation and it was wrong twice over. It
    /// **discarded 128 bits** of a 256-bit secret for no reason, and it applied
    /// **no domain separation**, so the same authenticator PRF output fed to
    /// two different Coincube-family apps would derive the *same* master key.
    /// Keychain derives from the same PRF extension; two related keys landing
    /// in one Vault descriptor is exactly the failure that makes a multisig
    /// quorum a single point of failure.
    ///
    /// The salt and info strings below are **load-bearing wire constants**.
    /// Changing either changes every key derived from every passkey — treat
    /// them as stable, exactly as `keychain-app` treats its own
    /// (`passkey_prf_entropy_source.dart`). The two must never match; there is
    /// a cross-app negative-vector test asserting they don't.
    pub fn from_prf_output(
        network: bitcoin::Network,
        prf_output: &[u8; 32],
    ) -> Result<Self, SignerError> {
        let entropy = Self::prf_entropy(prf_output)?;
        let mnemonic =
            bip39::Mnemonic::from_entropy(entropy.as_ref()).map_err(SignerError::Mnemonic)?;
        Self::from_mnemonic(network, mnemonic)
    }

    /// HKDF-SHA256 extract-and-expand over a WebAuthn PRF output. Split out so
    /// the domain constants can be pinned by a known-answer test without
    /// building a whole signer.
    pub fn prf_entropy(prf_output: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>, SignerError> {
        let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(PRF_HKDF_SALT), prf_output);
        let mut entropy = Zeroizing::new([0u8; 32]);
        // Only fails for an output longer than 255*32 bytes; 32 is fine.
        hk.expand(PRF_HKDF_INFO, entropy.as_mut())
            .map_err(|_| SignerError::KeyDerivationFailed)?;
        Ok(entropy)
    }

    pub fn from_str(network: bitcoin::Network, s: &str) -> Result<Self, SignerError> {
        let mnemonic = bip39::Mnemonic::from_str(s).map_err(SignerError::Mnemonic)?;
        Self::from_mnemonic(network, mnemonic)
    }

    /// Check if a file contains an encrypted mnemonic (any wire version).
    pub fn is_encrypted(data: &[u8]) -> bool {
        seed_crypt::is_encrypted(data)
    }

    pub fn mnemonics_folder(datadir_root: &path::Path, network: bitcoin::Network) -> path::PathBuf {
        [
            datadir_root,
            path::Path::new(&network.to_string()),
            path::Path::new(MNEMONICS_FOLDER_NAME),
        ]
        .iter()
        .collect()
    }

    /// Decode one mnemonic file's bytes. `cube_id` is the Cube the caller
    /// believes owns the file — it forms part of the v2 AAD (see
    /// [`crate::seed_crypt`]); pass `""` when there is no Cube context.
    fn read_mnemonic_bytes(
        data: Vec<u8>,
        password: Option<&str>,
        cube_id: &str,
    ) -> Result<Zeroizing<String>, SignerError> {
        if Self::is_encrypted(&data) {
            let pwd = password.ok_or(SignerError::PasswordRequired)?;
            // No device secret here: `coincube-core` has no keystore access.
            // A v3 file therefore surfaces `DeviceSecretRequired`, and the GUI
            // (`services::unlock`) is the layer that supplies the secret.
            let plaintext = seed_crypt::decrypt(&data, pwd, cube_id)?;
            Ok(Zeroizing::new(
                String::from_utf8(plaintext.to_vec()).map_err(|e| {
                    SignerError::MnemonicStorage(io::Error::new(io::ErrorKind::InvalidData, e))
                })?,
            ))
        } else {
            // Unencrypted file. Reading these is retained for one release so a
            // datadir written by a pre-hardening build still opens; the write
            // path is gone (see `store_encrypted`) and the startup migration
            // re-encrypts anything it finds.
            Ok(Zeroizing::new(String::from_utf8(data).map_err(|e| {
                SignerError::MnemonicStorage(io::Error::new(io::ErrorKind::InvalidData, e))
            })?))
        }
    }

    /// Read mnemonics from datadir (with optional password for encrypted files).
    /// To exclude Liquid/master-seed files, use [`Self::from_datadir_with_password_filtered`].
    pub fn from_datadir_with_password(
        datadir_root: &path::Path,
        network: bitcoin::Network,
        password: Option<&str>,
        cube_id: &str,
    ) -> Result<Vec<Self>, SignerError> {
        Self::from_datadir_with_password_filtered(datadir_root, network, password, cube_id, false)
    }

    /// Read mnemonics from datadir, optionally filtering out Liquid-wallet and master-seed mnemonics.
    pub fn from_datadir_with_password_filtered(
        datadir_root: &path::Path,
        network: bitcoin::Network,
        password: Option<&str>,
        cube_id: &str,
        vault_only: bool,
    ) -> Result<Vec<Self>, SignerError> {
        let mut signers = Vec::new();

        let mnemonics_folder = Self::mnemonics_folder(datadir_root, network);
        let mnemonic_paths =
            fs::read_dir(mnemonics_folder).map_err(SignerError::MnemonicStorage)?;

        for entry in mnemonic_paths {
            let path = entry.map_err(SignerError::MnemonicStorage)?.path();

            // Skip Liquid and master-seed mnemonics when in vault-only mode.
            if vault_only {
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    if filename.contains(&format!("-{}", LEGACY_LIQUID_SEED_LABEL))
                        || filename.contains(&format!("-{}", MASTER_SEED_LABEL))
                    {
                        continue;
                    }
                }
            }

            let data = fs::read(&path).map_err(SignerError::MnemonicStorage)?;

            if Self::is_encrypted(&data) && password.is_none() {
                continue;
            }
            let mnemonic_str = Self::read_mnemonic_bytes(data, password, cube_id)?;

            signers.push(Self::from_str(network, &mnemonic_str)?);
        }

        Ok(signers)
    }

    /// Load a specific signer by fingerprint from datadir
    pub fn from_datadir_by_fingerprint(
        datadir_root: &path::Path,
        network: bitcoin::Network,
        target_fingerprint: Fingerprint,
        password: Option<&str>,
        cube_id: &str,
    ) -> Result<Self, SignerError> {
        let mnemonics_folder = Self::mnemonics_folder(datadir_root, network);
        let mnemonic_paths =
            fs::read_dir(&mnemonics_folder).map_err(SignerError::MnemonicStorage)?;

        // First, try to find a file with the fingerprint in its name (fast path)
        let fingerprint_str = target_fingerprint.to_string();
        for entry in mnemonic_paths {
            let path = entry.map_err(SignerError::MnemonicStorage)?.path();

            // Check if filename contains the target fingerprint
            if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                if filename.contains(&fingerprint_str) {
                    // Found a potential match, try to load it
                    let data = fs::read(&path).map_err(SignerError::MnemonicStorage)?;
                    let mnemonic_str = Self::read_mnemonic_bytes(data, password, cube_id)?;
                    let signer = Self::from_str(network, &mnemonic_str)?;

                    // Verify the fingerprint matches
                    let secp = secp256k1::Secp256k1::signing_only();
                    if signer.fingerprint(&secp) == target_fingerprint {
                        return Ok(signer);
                    }
                }
            }
        }
        Err(SignerError::SignerNotFound(target_fingerprint))
    }

    /// Legacy method (backward compatible)
    /// Loads all mnemonics from datadir without password
    pub fn from_datadir(
        datadir_root: &path::Path,
        network: bitcoin::Network,
    ) -> Result<Vec<Self>, SignerError> {
        Self::from_datadir_with_password_filtered(datadir_root, network, None, "", false)
    }

    /// Load only Vault mnemonics (skip Liquid wallet mnemonics)
    pub fn from_datadir_vault_only(
        datadir_root: &path::Path,
        network: bitcoin::Network,
    ) -> Result<Vec<Self>, SignerError> {
        Self::from_datadir_with_password_filtered(datadir_root, network, None, "", true)
    }

    /// The BIP39 mnemonics from which the master key of this signer is derived.
    ///
    /// Length is 12 for a generated or user-entered seed and 24 for a
    /// passkey-derived one (`from_prf_output` feeds BIP39 the full 32 bytes of
    /// HKDF output). It used to return `[&str; 12]` with an `.expect("Always
    /// 12 words")`; that would now panic on a passkey Cube.
    pub fn words(&self) -> Vec<&'static str> {
        self.mnemonic.words().collect()
    }

    /// The BIP39 mnemonic words as a string.
    pub fn mnemonic_str(&self) -> Zeroizing<String> {
        let words = self.words();
        let mut mnemonic_str = Zeroizing::new(String::with_capacity(words.len() * 9));

        for (i, word) in words.iter().enumerate() {
            *mnemonic_str += word;
            if i < words.len() - 1 {
                *mnemonic_str += " ";
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

    /// Derive the SLIP-0077 master blinding key for Liquid/Elements confidential transactions.
    /// Returns the 32-byte master blinding key derived from the BIP39 seed.
    pub fn slip77_master_blinding_key(&self) -> [u8; 32] {
        use bitcoin::hashes::{sha512, Hash, HashEngine, Hmac, HmacEngine};

        // Get BIP39 seed (without passphrase)
        let seed = self.mnemonic.to_seed("");

        // Step 1: Derive SLIP-0021 root node
        // root = HMAC-SHA512(key="Symmetric key seed", msg=seed)
        let mut engine = HmacEngine::<sha512::Hash>::new(b"Symmetric key seed");
        engine.input(&seed);
        let root = Hmac::<sha512::Hash>::from_engine(engine);
        let root_bytes = root.as_byte_array();
        // Step 2: Derive SLIP-0077 node from root
        // The derivation uses root[0:32] as the key and "\x00SLIP-0077" as the message
        // The \x00 prefix is required by SLIP-0021 for child derivation
        let mut engine = HmacEngine::<sha512::Hash>::new(&root_bytes[..32]);
        engine.input(b"\x00SLIP-0077");
        let node = Hmac::<sha512::Hash>::from_engine(engine);
        let node_bytes = node.as_byte_array();
        // Step 3: Master blinding key is bytes [32:64] of the node (the "chain code" portion)
        let mut result = [0u8; 32];
        result.copy_from_slice(&node_bytes[32..64]);
        result
    }

    /// Reconstructs an equivalent signer by re-deriving from this signer's mnemonic.
    /// Useful when a second owner needs the same key material (e.g., the installer
    /// re-using the cube's master seed as the vault hot-signer in dev mode).
    pub fn try_clone(&self) -> Result<Self, SignerError> {
        Self::from_mnemonic(self.network, self.mnemonic.clone())
    }

    /// Store the mnemonic in a file within the given "data directory".
    ///
    /// The file is stored within a "mnemonics" folder, with the filename set to
    /// the fingerprint of the master xpub corresponding to this mnemonic. It is
    /// **always** encrypted: `password` is a `&str`, not an `Option`, so there
    /// is no reachable code path that writes a seed to disk in the clear. That
    /// is enforced by the type rather than by convention — the previous
    /// signature took `Option<&str>` and four production call sites passed
    /// `None`, one of them writing the Cube's *master* seed in the clear in
    /// developer mode.
    ///
    /// `cube_id` binds the file to its owning Cube through the AEAD's AAD. Pass
    /// `""` only where no Cube exists yet (the Vault installer runs before
    /// `find_or_create_cube` mints one); such a file stays readable afterwards.
    #[allow(clippy::too_many_arguments)]
    pub fn store_encrypted(
        &self,
        datadir_root: &path::Path,
        network: bitcoin::Network,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
        descriptor_info: Option<(String, i64)>,
        password: &str,
        cube_id: &str,
        device_secret: Option<&seed_crypt::DeviceSecret>,
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

        let plaintext = self.mnemonic_str();
        let data = seed_crypt::encrypt(plaintext.as_bytes(), password, cube_id, device_secret)?;

        let mut mnemonic_file = create_file(&file_path).map_err(SignerError::MnemonicStorage)?;
        mnemonic_file
            .write_all(&data)
            .map_err(SignerError::MnemonicStorage)?;
        drop(mnemonic_file);
        restrict_permissions(&file_path).map_err(SignerError::MnemonicStorage)?;

        Ok(())
    }

    pub fn xpriv_at(
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
        self.network = network;
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
    fn master_signer_gen() {
        // Entropy isn't completely broken.
        assert_ne!(
            MasterSigner::generate(bitcoin::Network::Bitcoin)
                .unwrap()
                .words(),
            MasterSigner::generate(bitcoin::Network::Bitcoin)
                .unwrap()
                .words()
        );

        // Roundtrips.
        let signer = MasterSigner::generate(bitcoin::Network::Bitcoin).unwrap();
        let mnemonics_str = signer.mnemonic_str();
        assert_eq!(
            MasterSigner::from_str(bitcoin::Network::Bitcoin, &mnemonics_str)
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

    /// The registered PRF eval input for Tenshu's Cube master seed:
    /// `SHA-256("coincube-tenshu/v1/master-seed")`. Pinned here as a literal so
    /// the value can be checked against the PRF domain registry by eye, and so
    /// a change to the domain string cannot pass silently. Mirrors
    /// `keychain-app/test/services/key_manager/passkey_prf_entropy_source_test.dart:40`.
    const TENSHU_PRF_SALT_HEX: &str =
        "4168dc1cd488a37de30cdfca8742130242cdee47283242feabcb9811f2399c72";

    const FIXED_PRF_OUTPUT: [u8; 32] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ];

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn tenshu_prf_salt_matches_the_domain_registry() {
        use sha2::Digest;
        let computed = sha2::Sha256::digest(b"coincube-tenshu/v1/master-seed");
        assert_eq!(
            hex(&computed),
            TENSHU_PRF_SALT_HEX,
            "the registered PRF eval input changed — every passkey Cube's master \
             key depends on it; update the domain registry deliberately or revert"
        );
    }

    #[test]
    fn master_signer_from_prf_output_deterministic() {
        let signer1 =
            MasterSigner::from_prf_output(bitcoin::Network::Bitcoin, &FIXED_PRF_OUTPUT).unwrap();
        let signer2 =
            MasterSigner::from_prf_output(bitcoin::Network::Bitcoin, &FIXED_PRF_OUTPUT).unwrap();

        // Same PRF output must produce the same mnemonic and fingerprint.
        assert_eq!(signer1.words(), signer2.words());
        let secp = secp256k1::Secp256k1::signing_only();
        assert_eq!(signer1.fingerprint(&secp), signer2.fingerprint(&secp));

        // 32 bytes of HKDF output → 24 words. The old implementation truncated
        // to 16 bytes and produced 12, discarding half the PRF secret.
        assert_eq!(signer1.words().len(), 24);

        // Different PRF output must produce a different mnemonic.
        let other_prf: [u8; 32] = [0xff; 32];
        let signer3 = MasterSigner::from_prf_output(bitcoin::Network::Bitcoin, &other_prf).unwrap();
        assert_ne!(signer1.words(), signer3.words());
    }

    #[test]
    fn changing_the_hkdf_domain_changes_the_mnemonic() {
        // Domain separation is only real if the domain is actually mixed in.
        // Derive with a deliberately different salt/info and require a
        // different result — this is what fails if someone "simplifies"
        // `prf_entropy` into a plain hash of the PRF output.
        let ours = MasterSigner::prf_entropy(&FIXED_PRF_OUTPUT).unwrap();

        let mut other = [0u8; 32];
        hkdf::Hkdf::<sha2::Sha256>::new(Some(b"coincube-tenshu/v2"), &FIXED_PRF_OUTPUT)
            .expand(PRF_HKDF_INFO, &mut other)
            .unwrap();
        assert_ne!(&ours[..], &other[..], "HKDF salt is not being mixed in");

        let mut other = [0u8; 32];
        hkdf::Hkdf::<sha2::Sha256>::new(Some(PRF_HKDF_SALT), &FIXED_PRF_OUTPUT)
            .expand(b"some-other-purpose/v1", &mut other)
            .unwrap();
        assert_ne!(&ours[..], &other[..], "HKDF info is not being mixed in");
    }

    /// Invariant I9: no two keys in one Vault descriptor may share a
    /// cryptographic root.
    ///
    /// Keychain and Tenshu are registered under one WebAuthn relying party, so
    /// **the same credential can be evaluated by both apps** — an iCloud-synced
    /// passkey minted on the user's iPhone is offerable to Tenshu on their Mac.
    /// The only thing keeping the two derived keys independent is the domain
    /// separation in the PRF registry. A Keychain key is a cosigner in the
    /// Vault descriptor and the Tenshu seed derives the Vault hot key; if these
    /// ever collide, an n-of-m multisig quietly becomes 1-of-1.
    ///
    /// This test is the guard that survives a future "unify our PRF handling"
    /// refactor — which is exactly the refactor that would cause the collision.
    /// Keychain's side is reimplemented here from the frozen constants in
    /// `passkey_prf_entropy_source.dart` rather than imported, because the
    /// point is to detect Tenshu drifting *towards* it.
    ///
    /// **If this test fails, do not "fix" it by making the two agree.**
    #[test]
    fn tenshu_and_keychain_derive_different_mnemonics_from_one_prf_output() {
        // Keychain, frozen (I8): HKDF-SHA256(salt="coincube-keychain/v1",
        // info="master-mnemonic-entropy/v1", L=16) → 12 words.
        let mut keychain_entropy = [0u8; 16];
        hkdf::Hkdf::<sha2::Sha256>::new(Some(b"coincube-keychain/v1"), &FIXED_PRF_OUTPUT)
            .expand(b"master-mnemonic-entropy/v1", &mut keychain_entropy)
            .unwrap();
        let keychain_mnemonic = bip39::Mnemonic::from_entropy(&keychain_entropy).unwrap();

        // Tenshu, this crate.
        let tenshu =
            MasterSigner::from_prf_output(bitcoin::Network::Bitcoin, &FIXED_PRF_OUTPUT).unwrap();

        assert_ne!(
            keychain_mnemonic.to_string(),
            tenshu.mnemonic_str().to_string(),
            "Tenshu and Keychain derived the SAME mnemonic from one PRF output — \
             a Vault cosigner and the Vault hot key now share a root (I9)"
        );

        // Also assert the entropy differs, not just the word count, so the
        // assertion still means something if Keychain ever moves to L=32.
        let tenshu_entropy = MasterSigner::prf_entropy(&FIXED_PRF_OUTPUT).unwrap();
        assert_ne!(&tenshu_entropy[..16], &keychain_entropy[..]);

        // The checked-in expected value for Keychain's side. It is here so that
        // someone "fixing" a drift has to consciously edit a constant that says
        // it must not match, rather than silently re-record a golden file.
        assert_eq!(
            hex(&keychain_entropy),
            "4364b33331fd6c7d4a632ba3ae91c88b",
            "Keychain's frozen derivation changed — it has live cosigner keys (I8)"
        );
        assert_eq!(
            hex(&tenshu_entropy[..]),
            "c3f9338a02357e3ebca6185fe158dee87cf6c5c38ff0f7066a010d53bbee1750",
            "Tenshu's derivation changed — every passkey Cube is re-keyed"
        );
    }

    /// The private key must actually be overwritten, not merely *appear* to be.
    ///
    /// This previously read the key out with `secret_bytes()` — which returns
    /// `[u8; 32]` **by value** — and zeroized that. It scrubbed a stack copy
    /// that was about to be dropped anyway, left the real key untouched, and
    /// created one more copy of the secret in the process. The `Drop` impl read
    /// as if it worked and did nothing.
    #[test]
    fn scrubbing_an_xpriv_overwrites_the_private_key() {
        let network = bitcoin::Network::Bitcoin;
        let signer = MasterSigner::generate(network).unwrap();
        let mut xpriv = signer.master_xpriv;

        let key_before = xpriv.private_key.secret_bytes();
        let chain_before = xpriv.chain_code;

        scrub_xpriv(&mut xpriv);

        assert_ne!(
            xpriv.private_key.secret_bytes(),
            key_before,
            "the private key survived the scrub — `secret_bytes()` returns a copy, \
             so zeroizing its result does nothing to the key itself"
        );
        assert_ne!(xpriv.chain_code, chain_before);
        assert_eq!(xpriv.chain_code, bip32::ChainCode::from([0u8; 32]));
    }

    #[test]
    fn master_signer_storage() {
        let secp = secp256k1::Secp256k1::signing_only();
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        let network = bitcoin::Network::Bitcoin;

        let words_set: HashSet<_> = (0..10)
            .map(|_| {
                let signer = MasterSigner::generate(network).unwrap();
                signer
                    .store_encrypted(&tmp_dir, network, &secp, None, "1234", "cube-a", None)
                    .unwrap();
                signer.words()
            })
            .collect();
        let words_read: HashSet<_> =
            MasterSigner::from_datadir_with_password(&tmp_dir, network, Some("1234"), "cube-a")
                .unwrap()
                .into_iter()
                .map(|signer| signer.words())
                .collect();
        assert_eq!(words_set, words_read);

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    /// I5: there is no reachable code path that writes a seed in the clear.
    /// `store_encrypted` takes `&str`, so this is a compile-time property — the
    /// test asserts the resulting bytes, which is the part a type can't state.
    #[test]
    fn stored_seed_files_are_always_encrypted() {
        let secp = secp256k1::Secp256k1::signing_only();
        let tmp_dir = tmp_dir();
        fs::create_dir_all(&tmp_dir).unwrap();
        let network = bitcoin::Network::Bitcoin;

        let signer = MasterSigner::generate(network).unwrap();
        signer
            .store_encrypted(&tmp_dir, network, &secp, None, "1234", "cube-a", None)
            .unwrap();

        let folder = MasterSigner::mnemonics_folder(&tmp_dir, network);
        let mut checked = 0;
        for entry in fs::read_dir(&folder).unwrap() {
            let data = fs::read(entry.unwrap().path()).unwrap();
            assert_eq!(
                crate::seed_crypt::format_version(&data),
                Some(2),
                "a seed file was written at the wrong version (or in the clear)"
            );
            // Belt and braces: the words must not appear anywhere in the file.
            let words = signer.words();
            assert!(!data
                .windows(words[0].len())
                .any(|w| w == words[0].as_bytes()));
            checked += 1;
        }
        assert_eq!(checked, 1);

        fs::remove_dir_all(tmp_dir).unwrap();
    }

    #[test]
    fn master_signer_sign_p2wsh() {
        let secp = secp256k1::Secp256k1::new();
        let network = bitcoin::Network::Bitcoin;

        // Create a Coincube descriptor with as primary path a 2-of-3 with three master signers and a
        // single master signer as recovery path. (The recovery path signer is also used in the
        // primary path.) Use various random derivation paths.
        let (prim_signer_a, prim_signer_b, recov_signer) = (
            MasterSigner::generate(network).unwrap(),
            MasterSigner::generate(network).unwrap(),
            MasterSigner::generate(network).unwrap(),
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
    fn master_signer_sign_taproot() {
        let secp = secp256k1::Secp256k1::new();
        let network = bitcoin::Network::Bitcoin;

        // Create a Coincube descriptor with as primary path a 2-of-3 with three master signers and a
        // single master signer as recovery path. (The recovery path signer is also used in the
        // primary path.) Use various random derivation paths.
        let (prim_signer_a, prim_signer_b, recov_signer) = (
            MasterSigner::generate(network).unwrap(),
            MasterSigner::generate(network).unwrap(),
            MasterSigner::generate(network).unwrap(),
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
        let mut signer = MasterSigner::from_str(
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

    #[test]
    fn test_try_clone_fingerprint_matches() {
        let secp = secp256k1::Secp256k1::new();
        let signer = MasterSigner::generate(bitcoin::Network::Bitcoin).unwrap();
        let cloned = signer.try_clone().unwrap();
        assert_eq!(signer.fingerprint(&secp), cloned.fingerprint(&secp));
    }
}
