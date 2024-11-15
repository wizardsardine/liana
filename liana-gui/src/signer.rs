pub use liana::signer::SignerError;

use liana::{
    miniscript::bitcoin::{
        bip32::{DerivationPath, Fingerprint, Xpub},
        psbt::Psbt,
        secp256k1, Network,
    },
    signer::HotSigner,
};

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
        datadir_root: &std::path::Path,
        network: Network,
    ) -> Result<(), SignerError> {
        self.key.store(datadir_root, network, &self.curve)
    }
}
