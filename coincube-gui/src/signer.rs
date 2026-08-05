pub use coincube_core::signer::SignerError;
use std::str::FromStr;

use coincube_core::{
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint, Xpub},
        psbt::Psbt,
        secp256k1, Network,
    },
    signer::{self, MasterSigner},
};

use crate::dir::{CoincubeDirectory, NetworkDirectory};

pub struct Signer {
    curve: secp256k1::Secp256k1<secp256k1::All>,
    key: MasterSigner,
    pub fingerprint: Fingerprint,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer").finish()
    }
}

impl Signer {
    pub fn new(key: MasterSigner) -> Self {
        let curve = secp256k1::Secp256k1::new();
        let fingerprint = key.fingerprint(&curve);
        Self {
            key,
            curve,
            fingerprint,
        }
    }

    pub fn set_network(&mut self, network: Network) {
        self.key.set_network(network)
    }

    /// 12 words for a generated or user-entered seed, 24 for a
    /// passkey-derived one.
    pub fn mnemonic(&self) -> Vec<&'static str> {
        self.key.words()
    }

    pub fn generate(network: Network) -> Result<Self, SignerError> {
        Ok(Self::new(MasterSigner::generate(network)?))
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub fn get_extended_pubkey(&self, path: &DerivationPath) -> Xpub {
        self.key.xpub_at(path, &self.curve)
    }

    /// Derive a Border Wallet GridRecoveryPhrase from the master key via BIP-85.
    pub fn derive_grid_recovery_phrase(
        &self,
    ) -> Result<
        coincube_core::border_wallet::GridRecoveryPhrase,
        coincube_core::border_wallet::BorderWalletError,
    > {
        coincube_core::border_wallet::GridRecoveryPhrase::from_master_signer(&self.key, &self.curve)
    }

    pub fn sign_psbt(&self, psbt: Psbt) -> Result<Psbt, SignerError> {
        self.key.sign_psbt(psbt, &self.curve)
    }

    /// Write the mnemonic file, encrypted under `password` (Argon2id-derived
    /// key, AES-256-GCM).
    ///
    /// Every stored mnemonic takes this path — the unencrypted `store()` that
    /// used to sit alongside it is gone, along with the four installer call
    /// sites that used it (one of which wrote the Cube's *master* seed in the
    /// clear in developer mode). `password` is not optional, so plaintext is no
    /// longer expressible.
    ///
    /// The PIN-encrypted layout is also what the rest of the app expects: the
    /// Liquid / Spark BreezClient decrypts via the Cube's PIN on every open,
    /// and `load_breez_client` refuses to decrypt an unencrypted blob with a
    /// password — a restored Cube written in the clear used to hang the app on
    /// "Starting daemon…".
    ///
    /// `cube_id` binds the file to its Cube through the AEAD's AAD; pass `""`
    /// where the Cube does not exist yet (see
    /// [`coincube_core::seed_crypt`]).
    #[allow(clippy::too_many_arguments)]
    pub fn store_encrypted(
        &self,
        datadir_root: &CoincubeDirectory,
        network: Network,
        checksum: &str,
        timestamp: i64,
        password: &str,
        cube_id: &str,
        device_secret: Option<&coincube_core::seed_crypt::DeviceSecret>,
    ) -> Result<(), SignerError> {
        self.key.store_encrypted(
            datadir_root.path(),
            network,
            &self.curve,
            Some((checksum.to_string(), timestamp)),
            password,
            cube_id,
            device_secret,
        )
    }

    /// Variant of [`Signer::store_encrypted`] for Seed-Only Cubes that
    /// do not have a Vault Descriptor. The BreezClient still requires the
    /// seed to be saved in the datadir so it can connect to the Liquid/Spark networks.
    pub fn store_encrypted_seed_only(
        &self,
        datadir_root: &CoincubeDirectory,
        network: Network,
        password: &str,
        cube_id: &str,
        device_secret: Option<&coincube_core::seed_crypt::DeviceSecret>,
    ) -> Result<(), SignerError> {
        self.key.store_encrypted(
            datadir_root.path(),
            network,
            &self.curve,
            None,
            password,
            cube_id,
            device_secret,
        )
    }
}

pub fn delete_wallet_mnemonics(
    network_directory: &NetworkDirectory,
    descriptor_checksum: &str,
    pinned_at: Option<i64>,
) -> Result<(), std::io::Error> {
    let folder = network_directory
        .path()
        .join(signer::MNEMONICS_FOLDER_NAME)
        .to_path_buf();
    if folder.exists() {
        for entry in std::fs::read_dir(&folder)? {
            let path = entry?.path();
            if let Some(filename) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|s| signer::MnemonicFileName::from_str(s).ok())
            {
                match (pinned_at, filename.descriptor_info) {
                    // legacy wallet, we delete any mnemonic-{}.txt
                    (None, None) => {
                        std::fs::remove_file(&path)?;
                    }
                    //  we delete any mnemonic-fg-sum-tim.txt that matches the descriptor_checksum
                    //  and timestamp
                    (Some(t), Some(info)) => {
                        if info.0 == descriptor_checksum && t == info.1 {
                            std::fs::remove_file(&path)?;
                        }
                    }
                    _ => { // The file is not related to the wallet}
                    }
                }
            }
        }
    }
    Ok(())
}
