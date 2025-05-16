pub use liana::signer::SignerError;
use std::str::FromStr;

use liana::{
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint, Xpub},
        psbt::Psbt,
        secp256k1, Network,
    },
    signer::{self, HotSigner},
};

use crate::dir::{LianaDirectory, NetworkDirectory};

pub struct Signer {
    curve: secp256k1::Secp256k1<secp256k1::All>,
    key: HotSigner,
    pub fingerprint: Fingerprint,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer").finish()
    }
}

impl Signer {
    pub fn new(key: HotSigner) -> Self {
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

    pub fn mnemonic(&self) -> [&'static str; 12] {
        self.key.words()
    }

    pub fn generate(network: Network) -> Result<Self, SignerError> {
        Ok(Self::new(HotSigner::generate(network)?))
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }

    pub fn get_extended_pubkey(&self, path: &DerivationPath) -> Xpub {
        self.key.xpub_at(path, &self.curve)
    }

    pub fn sign_psbt(&self, psbt: Psbt) -> Result<Psbt, SignerError> {
        self.key.sign_psbt(psbt, &self.curve)
    }

    pub fn store(
        &self,
        datadir_root: &LianaDirectory,
        network: Network,
        checksum: &str,
        timestamp: i64,
    ) -> Result<(), SignerError> {
        self.key.store(
            datadir_root.path(),
            network,
            &self.curve,
            Some((checksum.to_string(), timestamp)),
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
