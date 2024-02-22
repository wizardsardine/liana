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
    self,
    bip32::{self, Error as Bip32Error},
    ecdsa,
    hashes::Hash,
    psbt::{Input as PsbtIn, Psbt},
    secp256k1, sighash,
};

/// An error related to using a signer.
#[derive(Debug)]
pub enum SignerError {
    Randomness(random::RandomnessError),
    Mnemonic(bip39::Error),
    Bip32(Bip32Error),
    MnemonicStorage(io::Error),
    InsanePsbt,
    IncompletePsbt,
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
        mnemonics_folder.push(format!("mnemonic-{:x}.txt", self.fingerprint(secp)));
        let mnemonic_path = mnemonics_folder;
        let mut mnemonic_file =
            create_file(&mnemonic_path).map_err(SignerError::MnemonicStorage)?;
        mnemonic_file
            .write_all(self.mnemonic_str().as_bytes())
            .map_err(SignerError::MnemonicStorage)?;

        Ok(())
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
        let sig_type = sighash::EcdsaSighashType::All;
        let sighash = sighash_cache
            .p2wsh_signature_hash(input_index, witscript, value, sig_type)
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
            let sig = secp.sign_ecdsa_low_r(&sighash, &privkey.inner);
            psbt_in.partial_sigs.insert(
                pubkey,
                ecdsa::Signature {
                    sig,
                    hash_ty: sig_type,
                },
            );
        }

        Ok(())
    }

    /// Sign all inputs of the given PSBT.
    ///
    /// **This does not perform any check. It will blindly sign anything that's passed.**
    pub fn sign_psbt(
        &self,
        mut psbt: Psbt,
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
    ) -> Result<Psbt, SignerError> {
        let master_fingerprint = self.fingerprint(secp);
        let mut sighash_cache = sighash::SighashCache::new(&psbt.unsigned_tx);

        // Sign each input in the PSBT.
        for i in 0..psbt.inputs.len() {
            self.sign_p2wsh(
                secp,
                &mut sighash_cache,
                master_fingerprint,
                &mut psbt.inputs[i],
                i,
            )?;
        }

        Ok(psbt)
    }

    /// Change the network of generated extended keys. Note this value only has to do with the
    /// BIP32 encoding of those keys (xpubs, tpubs, ..) but does not affect any data (whether it is
    /// the keys or the mnemonics).
    pub fn set_network(&mut self, network: bitcoin::Network) {
        self.master_xpriv.network = network;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{descriptors, testutils::*};
    use miniscript::{
        bitcoin::{locktime::absolute, psbt::Input as PsbtIn, Amount},
        descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, Wildcard},
    };
    use std::collections::{BTreeMap, HashSet};

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

    #[test]
    fn hot_signer_sign() {
        let secp = secp256k1::Secp256k1::new();
        let network = bitcoin::Network::Bitcoin;

        // Create a Liana descriptor with as primary path a 2-of-3 with three hot signers and a
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
        let policy = descriptors::LianaPolicy::new_legacy(
            prim_keys,
            [(46, recov_keys)].iter().cloned().collect(),
        )
        .unwrap();
        let desc = descriptors::LianaDescriptor::new(policy);

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
                    .payload()
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
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 1);
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 2);
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 4);

        // We can add another external output to the transaction, we can still sign without issue.
        // The output can be insane, we don't check it. It doesn't even need an accompanying PSBT
        // output.
        dummy_psbt.unsigned_tx.output.push(bitcoin::TxOut::NULL);
        let psbt = dummy_psbt.clone();
        assert!(psbt.inputs[0].partial_sigs.is_empty());
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 1);
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 2);
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert_eq!(psbt.inputs[0].partial_sigs.len(), 4);

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
        let psbt = prim_signer_a.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        let psbt = prim_signer_b.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 2));
        let psbt = recov_signer.sign_psbt(psbt, &secp).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 4));

        // If the witness script is missing for one of the inputs it'll tell us the PSBT is
        // incomplete.
        let mut psbt = dummy_psbt.clone();
        psbt.inputs[1].witness_script = None;
        assert!(prim_signer_a
            .sign_psbt(psbt, &secp)
            .unwrap_err()
            .to_string()
            .contains("The PSBT is missing some information necessary for signing."));

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
}
