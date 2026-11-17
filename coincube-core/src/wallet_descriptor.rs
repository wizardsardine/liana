//! Validated public descriptors for existing Vaults and imported Bitcoin wallets.
//! An imported wallet keeps its original spending policy and signer origins;
//! the Cube seed does not become one of its signers.

use std::{fmt, str::FromStr};

use miniscript::{
    bitcoin::{
        self,
        hashes::{sha256, Hash, HashEngine},
        NetworkKind, ScriptBuf,
    },
    descriptor::{DefiniteDescriptorKey, Wildcard},
    Descriptor, DescriptorPublicKey, ForEachKey,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    chain::ChainId,
    descriptors::{CoincubeDescriptor, SinglePathCoincubeDesc},
    public_wallet_pair::{canonical_pair, PairError},
    public_wallet_source::{check_signable_shape, has_duplicate_key, same_wallet},
};

const IMPORTED_ID_TAG: &[u8] = b"coincube/imported-wallet/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportedWalletError {
    UnsupportedChain,
    Descriptor,
    MissingChange,
    WrongBranch,
    MissingOrigin,
    OriginMismatch,
    WrongNetwork,
    DuplicateKey,
    Unrelated,
    ChainMismatch,
}

impl fmt::Display for ImportedWalletError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedChain => {
                "This Bitcoin chain has no supported paired Bitcoin Blake2b chain"
            }
            Self::Descriptor => "Unsupported public wallet descriptor",
            Self::MissingChange => "A complete receive/change descriptor pair is required",
            Self::WrongBranch => "Receive and change keys must end in /0/* and /1/*",
            Self::MissingOrigin => "Every wallet key needs its master fingerprint and path",
            Self::OriginMismatch => "A wallet key origin does not match its account key depth",
            Self::WrongNetwork => "A wallet key belongs to another Bitcoin network",
            Self::DuplicateKey => "The same account key appears twice in a descriptor",
            Self::Unrelated => "Receive and change descriptors do not name the same wallet",
            Self::ChainMismatch => "The wallet is bound to another Bitcoin chain",
        })
    }
}

impl std::error::Error for ImportedWalletError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedWalletDescriptor {
    chain: ChainId,
    receive: Descriptor<DescriptorPublicKey>,
    change: Descriptor<DescriptorPublicKey>,
}

impl ImportedWalletDescriptor {
    /// Parse either an explicit pair or a conventional `/<0;1>/*` descriptor.
    /// The latter expands through Miniscript before both branches are checked.
    pub fn parse(
        receive: &str,
        change: Option<&str>,
        chain: ChainId,
    ) -> Result<Self, ImportedWalletError> {
        if receive.len() > 16_384 || change.is_some_and(|text| text.len() > 16_384) {
            return Err(ImportedWalletError::Descriptor);
        }
        let receive = Descriptor::<DescriptorPublicKey>::from_str(receive)
            .map_err(|_| ImportedWalletError::Descriptor)?;
        let (receive, change) = match change {
            Some(text) if !text.trim().is_empty() => {
                let change = Descriptor::<DescriptorPublicKey>::from_str(text)
                    .map_err(|_| ImportedWalletError::Descriptor)?;
                (receive, change)
            }
            Some(_) => return Err(ImportedWalletError::MissingChange),
            None => {
                let mut branches = receive
                    .into_single_descriptors()
                    .map_err(|_| ImportedWalletError::MissingChange)?;
                if branches.len() != 2 {
                    return Err(ImportedWalletError::MissingChange);
                }
                (branches.remove(0), branches.remove(0))
            }
        };
        Self::from_descriptors(receive, change, chain)
    }

    /// Admit only a complete, origin-tagged, ranged wallet with explicit
    /// receive/change branches. No signer or spend authority is created here.
    pub fn from_descriptors(
        receive: Descriptor<DescriptorPublicKey>,
        change: Descriptor<DescriptorPublicKey>,
        chain: ChainId,
    ) -> Result<Self, ImportedWalletError> {
        let network = if chain.bitcoin_network() == bitcoin::Network::Bitcoin {
            NetworkKind::Main
        } else {
            NetworkKind::Test
        };
        for descriptor in [&receive, &change] {
            check_signable_shape(descriptor).map_err(|_| ImportedWalletError::Descriptor)?;
            if has_duplicate_key(descriptor) {
                return Err(ImportedWalletError::DuplicateKey);
            }
        }
        let (receive, change) = canonical_pair(&receive, &change).map_err(|error| match error {
            PairError::BranchInXpub => ImportedWalletError::WrongBranch,
            PairError::DuplicateKey => ImportedWalletError::DuplicateKey,
            _ => ImportedWalletError::Unrelated,
        })?;
        check_keys(&receive, 0, network)?;
        check_keys(&change, 1, network)?;
        if !same_wallet(&receive, &change) {
            return Err(ImportedWalletError::Unrelated);
        }
        Ok(Self {
            chain,
            receive,
            change,
        })
    }

    /// Reuse this wallet's exact public branches on its corresponding fork.
    /// The Cube unlock seed grants no signing authority over this wallet.
    pub fn paired_blake2b(&self) -> Result<Self, ImportedWalletError> {
        let chain = match self.chain {
            ChainId::Bitcoin => ChainId::BitcoinBlake2b,
            ChainId::Testnet4 => ChainId::BitcoinBlake2bTestnet4,
            _ => return Err(ImportedWalletError::UnsupportedChain),
        };
        Ok(Self {
            chain,
            receive: self.receive.clone(),
            change: self.change.clone(),
        })
    }

    pub fn chain(&self) -> ChainId {
        self.chain
    }

    pub fn receive_descriptor(&self) -> &Descriptor<DescriptorPublicKey> {
        &self.receive
    }

    pub fn change_descriptor(&self) -> &Descriptor<DescriptorPublicKey> {
        &self.change
    }

    /// Full, domain-separated identity over chain and both canonical branches.
    pub fn identity(&self, chain: ChainId) -> Result<sha256::Hash, ImportedWalletError> {
        if chain != self.chain {
            return Err(ImportedWalletError::ChainMismatch);
        }
        let mut engine = sha256::Hash::engine();
        engine.input(IMPORTED_ID_TAG);
        for text in [
            chain.api_str(),
            &self.receive.to_string(),
            &self.change.to_string(),
        ] {
            engine.input(&(text.len() as u64).to_be_bytes());
            engine.input(text.as_bytes());
        }
        Ok(sha256::Hash::from_engine(engine))
    }
}

fn check_keys(
    descriptor: &Descriptor<DescriptorPublicKey>,
    branch: u32,
    network: NetworkKind,
) -> Result<(), ImportedWalletError> {
    let mut error = None;
    descriptor.for_each_key(|key| {
        let result = match key {
            DescriptorPublicKey::XPub(xpub) => {
                if xpub.xkey.network != network {
                    Err(ImportedWalletError::WrongNetwork)
                } else if xpub.origin.is_none() {
                    Err(ImportedWalletError::MissingOrigin)
                } else if xpub.origin.as_ref().is_some_and(|(_, path)| {
                    path.len() != usize::from(xpub.xkey.depth)
                        || path
                            .as_ref()
                            .last()
                            .is_some_and(|last| *last != xpub.xkey.child_number)
                }) {
                    Err(ImportedWalletError::OriginMismatch)
                } else if xpub.wildcard != Wildcard::Unhardened
                    || xpub.derivation_path.as_ref()
                        != [bitcoin::bip32::ChildNumber::Normal { index: branch }]
                {
                    Err(ImportedWalletError::WrongBranch)
                } else {
                    Ok(())
                }
            }
            DescriptorPublicKey::Single(_) | DescriptorPublicKey::MultiXPub(_) => {
                Err(ImportedWalletError::Descriptor)
            }
        };
        match result {
            Ok(()) => true,
            Err(reason) => {
                error = Some(reason);
                false
            }
        }
    });
    error.map_or(Ok(()), Err)
}

/// A borrowed public branch for wallet code that does not need Vault policy.
#[derive(Debug, Clone, Copy)]
pub struct PublicBranchRef<'a>(&'a Descriptor<DescriptorPublicKey>);

impl<'a> PublicBranchRef<'a> {
    pub fn as_descriptor(self) -> &'a Descriptor<DescriptorPublicKey> {
        self.0
    }

    pub fn derive(
        self,
        index: u32,
    ) -> Result<Descriptor<DefiniteDescriptorKey>, ImportedWalletError> {
        self.0
            .at_derivation_index(index)
            .map_err(|_| ImportedWalletError::Descriptor)
    }

    pub fn script_pubkey(self, index: u32) -> Result<ScriptBuf, ImportedWalletError> {
        self.derive(index)
            .map(|descriptor| descriptor.script_pubkey())
    }
}

impl fmt::Display for PublicBranchRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.0, f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletDescriptor {
    Vault(CoincubeDescriptor),
    Imported(ImportedWalletDescriptor),
}

impl fmt::Display for WalletDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vault(vault) => fmt::Display::fmt(vault, f),
            Self::Imported(wallet) => write!(
                f,
                "imported-v1:{}:{}:{}",
                wallet.chain, wallet.receive, wallet.change
            ),
        }
    }
}

impl From<CoincubeDescriptor> for WalletDescriptor {
    fn from(value: CoincubeDescriptor) -> Self {
        Self::Vault(value)
    }
}

impl From<ImportedWalletDescriptor> for WalletDescriptor {
    fn from(value: ImportedWalletDescriptor) -> Self {
        Self::Imported(value)
    }
}

impl WalletDescriptor {
    pub fn as_vault(&self) -> Option<&CoincubeDescriptor> {
        match self {
            Self::Vault(vault) => Some(vault),
            Self::Imported(_) => None,
        }
    }

    pub fn as_imported(&self) -> Option<&ImportedWalletDescriptor> {
        match self {
            Self::Vault(_) => None,
            Self::Imported(imported) => Some(imported),
        }
    }

    pub fn receive_descriptor(&self) -> PublicBranchRef<'_> {
        PublicBranchRef(self.branches_public().0)
    }

    pub fn change_descriptor(&self) -> PublicBranchRef<'_> {
        PublicBranchRef(self.branches_public().1)
    }

    pub fn branches_public(
        &self,
    ) -> (
        &Descriptor<DescriptorPublicKey>,
        &Descriptor<DescriptorPublicKey>,
    ) {
        match self {
            Self::Vault(vault) => (
                vault.receive_descriptor().as_descriptor_public_key(),
                vault.change_descriptor().as_descriptor_public_key(),
            ),
            Self::Imported(imported) => (&imported.receive, &imported.change),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportedRecord {
    kind: String,
    version: u8,
    chain: ChainId,
    receive: String,
    change: String,
}

impl Serialize for ImportedWalletDescriptor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ImportedRecord {
            kind: "imported".to_owned(),
            version: 1,
            chain: self.chain,
            receive: self.receive.to_string(),
            change: self.change.to_string(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ImportedWalletDescriptor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let record = ImportedRecord::deserialize(deserializer)?;
        if record.kind != "imported" || record.version != 1 {
            return Err(D::Error::custom("unsupported imported wallet record"));
        }
        Self::parse(&record.receive, Some(&record.change), record.chain).map_err(D::Error::custom)
    }
}

impl Serialize for WalletDescriptor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Vault(vault) => vault.serialize(serializer),
            Self::Imported(imported) => imported.serialize(serializer),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WalletDescriptorRecord {
    Imported(ImportedWalletDescriptor),
    Vault(LegacyVaultRecord),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyVaultRecord {
    multi_desc: Descriptor<DescriptorPublicKey>,
    receive_desc: SinglePathCoincubeDesc,
    change_desc: SinglePathCoincubeDesc,
}

impl<'de> Deserialize<'de> for WalletDescriptor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match WalletDescriptorRecord::deserialize(deserializer)? {
            WalletDescriptorRecord::Imported(imported) => Ok(Self::Imported(imported)),
            WalletDescriptorRecord::Vault(record) => {
                let vault = CoincubeDescriptor::from_str(&record.multi_desc.to_string())
                    .map_err(D::Error::custom)?;
                if vault.receive_descriptor() != &record.receive_desc
                    || vault.change_descriptor() != &record.change_desc
                {
                    return Err(D::Error::custom("inconsistent Vault descriptor branches"));
                }
                Ok(Self::Vault(vault))
            }
        }
    }
}

/// An imported wallet seen through the Vault's policy and signing-status
/// types, so the Vault screens can show, sign and track it. It has one
/// spending path and no recovery paths.
impl ImportedWalletDescriptor {
    /// Both branches as one `<0;1>/*` descriptor, the form a Vault keeps and a
    /// hardware wallet registers. Every key was checked at construction to be
    /// an origin-tagged xpub on its `/0/*` (receive) branch.
    pub fn multipath_descriptor(&self) -> Descriptor<DescriptorPublicKey> {
        use miniscript::{
            bitcoin::bip32::{ChildNumber, DerivationPath},
            descriptor::{DerivPaths, DescriptorMultiXKey},
            translate_hash_clone, TranslatePk, Translator,
        };
        struct Merge;
        impl Translator<DescriptorPublicKey, DescriptorPublicKey, ()> for Merge {
            fn pk(&mut self, pk: &DescriptorPublicKey) -> Result<DescriptorPublicKey, ()> {
                let DescriptorPublicKey::XPub(xkey) = pk else {
                    return Err(());
                };
                let branch = |index| DerivationPath::from(vec![ChildNumber::Normal { index }]);
                Ok(DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
                    origin: xkey.origin.clone(),
                    xkey: xkey.xkey,
                    derivation_paths: DerivPaths::new(vec![branch(0), branch(1)]).ok_or(())?,
                    wildcard: Wildcard::Unhardened,
                }))
            }
            translate_hash_clone!(DescriptorPublicKey, DescriptorPublicKey, ());
        }
        self.receive
            .translate_pk(&mut Merge)
            .expect("imported keys are origin-tagged xpubs, checked at construction")
    }

    /// The wallet's policy: its single key or its k-of-n keys as the primary
    /// path, with no recovery paths.
    pub fn policy(
        &self,
    ) -> Result<crate::descriptors::CoincubePolicy, crate::descriptors::CoincubePolicyError> {
        use miniscript::policy::Liftable;
        let lifted = self
            .multipath_descriptor()
            .lift()
            .map_err(|_| crate::descriptors::CoincubePolicyError::IncompatibleDesc)?;
        crate::descriptors::PathInfo::from_primary_path(lifted)
            .map(crate::descriptors::CoincubePolicy::primary_only)
    }

    /// Signatures claimed in `psbt`, as the Vault counts them: per key origin,
    /// against the primary path's threshold. A signer counts only where it
    /// signed every input, so a PSBT signed on some inputs reads as less
    /// signed rather than failing. This is progress only;
    /// [`crate::imported_spend::finalize_imported_spend`] verifies signatures
    /// before a broadcast.
    pub fn partial_spend_info(
        &self,
        psbt: &bitcoin::Psbt,
    ) -> Result<crate::descriptors::PartialSpendInfo, crate::descriptors::CoincubeDescError> {
        use crate::descriptors::CoincubeDescError;
        if psbt.inputs.len() != psbt.unsigned_tx.input.len()
            || psbt.outputs.len() != psbt.unsigned_tx.output.len()
            || psbt.inputs.is_empty()
            || psbt.outputs.is_empty()
        {
            return Err(CoincubeDescError::InsanePsbt);
        }
        let policy = self.policy().map_err(|_| CoincubeDescError::InsanePsbt)?;
        let mut infos = psbt.inputs.iter().map(|input| {
            policy.primary_path().spend_info(
                input
                    .partial_sigs
                    .keys()
                    .filter_map(|key| input.bip32_derivation.get(&key.inner)),
            )
        });
        let mut common = infos.next().expect("checked at least one input");
        for info in infos {
            common.signed_pubkeys.retain(|fingerprint, count| {
                info.signed_pubkeys.get(fingerprint).is_some_and(|other| {
                    *count = (*count).min(*other);
                    true
                })
            });
        }
        common.sigs_count = common.signed_pubkeys.values().sum();
        Ok(crate::descriptors::PartialSpendInfo::primary_only(common))
    }

    /// Outputs of `psbt` paying this wallet (its change or receive branch).
    /// Construction sets a BIP32 derivation on each; an output counts only if
    /// the script derived at that index matches.
    pub fn change_indexes(&self, psbt: &bitcoin::Psbt) -> Vec<usize> {
        psbt.outputs
            .iter()
            .enumerate()
            .filter_map(|(index, output)| {
                let path = output
                    .bip32_derivation
                    .values()
                    .map(|(_, path)| path)
                    .next()?;
                let child = match path.into_iter().last()? {
                    bitcoin::bip32::ChildNumber::Normal { index } => *index,
                    bitcoin::bip32::ChildNumber::Hardened { .. } => return None,
                };
                let script = &psbt.unsigned_tx.output.get(index)?.script_pubkey;
                [&self.change, &self.receive]
                    .iter()
                    .any(|branch| {
                        branch
                            .at_derivation_index(child)
                            .is_ok_and(|derived| &derived.script_pubkey() == script)
                    })
                    .then_some(index)
            })
            .collect()
    }

    /// Largest size in vbytes `tx` can reach once every input, all from this
    /// wallet, is satisfied. Mirrors the Vault's estimate for fee rates.
    pub fn unsigned_tx_max_vbytes(&self, tx: &bitcoin::Transaction) -> u64 {
        let witness_factor = bitcoin::constants::WITNESS_SCALE_FACTOR as u64;
        let inputs = tx.input.len() as u64;
        let segwit = !matches!(self.receive, Descriptor::Pkh(_));
        // As `imported_spend` sizes its inputs: the satisfaction plus, for
        // segwit, the witness item count.
        let satisfaction = self
            .receive
            .max_weight_to_satisfy()
            .map(|weight| weight.to_wu())
            .unwrap_or(0)
            + u64::from(segwit);
        let segwit_marker =
            if inputs > 0 && segwit && tx.input.iter().all(|txin| txin.witness.is_empty()) {
                2
            } else {
                0
            };
        let weight = tx.weight().to_wu() + satisfaction * inputs + segwit_marker;
        weight.div_ceil(witness_factor)
    }
}

/// What a saved transaction needs from the wallet that spends it: its own
/// outputs, a size estimate for the fee rate, and signature progress. A Vault
/// answers from its inheritance policy, an imported wallet from its single
/// primary path, so one transaction view serves both.
pub trait SpendAnalysis {
    fn spend_change_indexes(
        &self,
        psbt: &bitcoin::Psbt,
        secp: &bitcoin::secp256k1::Secp256k1<impl bitcoin::secp256k1::Verification>,
    ) -> Vec<usize>;
    fn spend_max_vbytes(&self, tx: &bitcoin::Transaction, use_primary_path: bool) -> u64;
    fn spend_info(
        &self,
        psbt: &bitcoin::Psbt,
    ) -> Result<crate::descriptors::PartialSpendInfo, crate::descriptors::CoincubeDescError>;
}

impl SpendAnalysis for CoincubeDescriptor {
    fn spend_change_indexes(
        &self,
        psbt: &bitcoin::Psbt,
        secp: &bitcoin::secp256k1::Secp256k1<impl bitcoin::secp256k1::Verification>,
    ) -> Vec<usize> {
        self.change_indexes(psbt, secp)
            .into_iter()
            .map(|change| change.index())
            .collect()
    }
    fn spend_max_vbytes(&self, tx: &bitcoin::Transaction, use_primary_path: bool) -> u64 {
        self.unsigned_tx_max_vbytes(tx, use_primary_path)
    }
    fn spend_info(
        &self,
        psbt: &bitcoin::Psbt,
    ) -> Result<crate::descriptors::PartialSpendInfo, crate::descriptors::CoincubeDescError> {
        self.partial_spend_info(psbt)
    }
}

impl SpendAnalysis for ImportedWalletDescriptor {
    fn spend_change_indexes(
        &self,
        psbt: &bitcoin::Psbt,
        _secp: &bitcoin::secp256k1::Secp256k1<impl bitcoin::secp256k1::Verification>,
    ) -> Vec<usize> {
        self.change_indexes(psbt)
    }
    /// An imported wallet has one path, so `use_primary_path` changes nothing.
    fn spend_max_vbytes(&self, tx: &bitcoin::Transaction, _use_primary_path: bool) -> u64 {
        self.unsigned_tx_max_vbytes(tx)
    }
    fn spend_info(
        &self,
        psbt: &bitcoin::Psbt,
    ) -> Result<crate::descriptors::PartialSpendInfo, crate::descriptors::CoincubeDescError> {
        self.partial_spend_info(psbt)
    }
}

impl SpendAnalysis for WalletDescriptor {
    fn spend_change_indexes(
        &self,
        psbt: &bitcoin::Psbt,
        secp: &bitcoin::secp256k1::Secp256k1<impl bitcoin::secp256k1::Verification>,
    ) -> Vec<usize> {
        match self {
            Self::Vault(vault) => vault.spend_change_indexes(psbt, secp),
            Self::Imported(imported) => imported.spend_change_indexes(psbt, secp),
        }
    }
    fn spend_max_vbytes(&self, tx: &bitcoin::Transaction, use_primary_path: bool) -> u64 {
        match self {
            Self::Vault(vault) => vault.spend_max_vbytes(tx, use_primary_path),
            Self::Imported(imported) => imported.spend_max_vbytes(tx, use_primary_path),
        }
    }
    fn spend_info(
        &self,
        psbt: &bitcoin::Psbt,
    ) -> Result<crate::descriptors::PartialSpendInfo, crate::descriptors::CoincubeDescError> {
        match self {
            Self::Vault(vault) => vault.spend_info(psbt),
            Self::Imported(imported) => imported.spend_info(psbt),
        }
    }
}

impl WalletDescriptor {
    /// The wallet's spending policy: a Vault's inheritance policy, or an
    /// imported wallet's single primary path.
    pub fn policy(
        &self,
    ) -> Result<crate::descriptors::CoincubePolicy, crate::descriptors::CoincubePolicyError> {
        match self {
            Self::Vault(vault) => Ok(vault.policy()),
            Self::Imported(imported) => imported.policy(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniscript::bitcoin::{
        bip32::{DerivationPath, Xpriv, Xpub},
        secp256k1::Secp256k1,
        Network,
    };

    fn key(seed: u8, path: &str, network: Network, origin: bool) -> String {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(network, &[seed; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str(path).unwrap())
            .unwrap();
        let xpub = Xpub::from_priv(&secp, &account);
        if origin {
            format!(
                "[{}/{}]{xpub}",
                master.fingerprint(&secp),
                path.trim_start_matches("m/")
            )
        } else {
            xpub.to_string()
        }
    }

    fn single(branch: u32, network: Network, origin: bool) -> String {
        let coin = if network == Network::Bitcoin { 0 } else { 1 };
        let key = key(1, &format!("m/84h/{coin}h/0h"), network, origin);
        format!("wpkh({key}/{branch}/*)")
    }

    fn multi(kind: &str, order: [usize; 3], branch: u32, omit_origin: Option<usize>) -> String {
        let keys: Vec<String> = (1..=3)
            .map(|seed| {
                key(
                    seed,
                    "m/48h/0h/0h/2h",
                    Network::Bitcoin,
                    omit_origin != Some((seed - 1) as usize),
                )
            })
            .collect();
        let keys = order
            .iter()
            .map(|index| format!("{}/{branch}/*", keys[*index]))
            .collect::<Vec<_>>()
            .join(",");
        format!("wsh({kind}(2,{keys}))")
    }

    #[test]
    fn explicit_and_combined_descriptor_preserve_addresses_and_identity() {
        let receive = single(0, Network::Bitcoin, true);
        let change = single(1, Network::Bitcoin, true);
        let pair =
            ImportedWalletDescriptor::parse(&receive, Some(&change), ChainId::Bitcoin).unwrap();
        let combined = receive.replace("/0/*", "/<0;1>/*");
        let expanded = ImportedWalletDescriptor::parse(&combined, None, ChainId::Bitcoin).unwrap();
        assert_eq!(pair, expanded);
        let wrapper = WalletDescriptor::Imported(pair.clone());
        assert!(wrapper.as_vault().is_none());
        assert_eq!(wrapper.as_imported(), Some(&pair));
        for index in [0, 1, 1_000] {
            assert_eq!(
                wrapper.receive_descriptor().script_pubkey(index).unwrap(),
                pair.receive_descriptor()
                    .at_derivation_index(index)
                    .unwrap()
                    .script_pubkey()
            );
            assert_eq!(
                wrapper.change_descriptor().script_pubkey(index).unwrap(),
                pair.change_descriptor()
                    .at_derivation_index(index)
                    .unwrap()
                    .script_pubkey()
            );
        }
        assert_eq!(
            pair.identity(ChainId::Testnet4),
            Err(ImportedWalletError::ChainMismatch)
        );
        assert_ne!(
            pair.identity(ChainId::Bitcoin).unwrap(),
            sha256::Hash::all_zeros()
        );
    }

    #[test]
    fn paired_fork_preserves_scripts_and_signers_with_separate_identity() {
        for (source_chain, network, fork_chain) in [
            (ChainId::Bitcoin, Network::Bitcoin, ChainId::BitcoinBlake2b),
            (
                ChainId::Testnet4,
                Network::Testnet4,
                ChainId::BitcoinBlake2bTestnet4,
            ),
        ] {
            let source = ImportedWalletDescriptor::parse(
                &single(0, network, true),
                Some(&single(1, network, true)),
                source_chain,
            )
            .unwrap();
            let fork = source.paired_blake2b().unwrap();
            assert_eq!(fork.chain(), fork_chain);
            assert_eq!(source.receive_descriptor(), fork.receive_descriptor());
            assert_eq!(source.change_descriptor(), fork.change_descriptor());
            assert_ne!(
                source.identity(source_chain).unwrap(),
                fork.identity(fork_chain).unwrap()
            );
            assert!(WalletDescriptor::Imported(fork.clone())
                .as_vault()
                .is_none());
            let restored: ImportedWalletDescriptor =
                serde_json::from_str(&serde_json::to_string(&fork).unwrap()).unwrap();
            assert_eq!(restored, fork);
            for index in [0, 1, 1000] {
                assert_eq!(
                    WalletDescriptor::Imported(source.clone())
                        .receive_descriptor()
                        .script_pubkey(index)
                        .unwrap(),
                    WalletDescriptor::Imported(fork.clone())
                        .receive_descriptor()
                        .script_pubkey(index)
                        .unwrap()
                );
                assert_eq!(
                    WalletDescriptor::Imported(source.clone())
                        .change_descriptor()
                        .script_pubkey(index)
                        .unwrap(),
                    WalletDescriptor::Imported(fork.clone())
                        .change_descriptor()
                        .script_pubkey(index)
                        .unwrap()
                );
            }
            assert_eq!(
                fork.paired_blake2b(),
                Err(ImportedWalletError::UnsupportedChain)
            );
        }
        let source = ImportedWalletDescriptor::parse(
            &single(0, Network::Regtest, true),
            Some(&single(1, Network::Regtest, true)),
            ChainId::Regtest,
        )
        .unwrap();
        assert_eq!(
            source.paired_blake2b(),
            Err(ImportedWalletError::UnsupportedChain)
        );
        assert_eq!(
            ImportedWalletDescriptor::parse(
                &single(0, Network::Bitcoin, true),
                Some(&single(1, Network::Bitcoin, true)),
                ChainId::BitcoinBlake2bTestnet4
            ),
            Err(ImportedWalletError::WrongNetwork)
        );
    }

    #[test]
    fn admission_rejects_missing_change_wrong_branch_origin_network_and_checksum() {
        let receive = single(0, Network::Bitcoin, true);
        let change = single(1, Network::Bitcoin, true);
        assert_eq!(
            ImportedWalletDescriptor::parse(&receive, None, ChainId::Bitcoin),
            Err(ImportedWalletError::MissingChange)
        );
        assert_eq!(
            ImportedWalletDescriptor::parse(&receive, Some(&receive), ChainId::Bitcoin),
            Err(ImportedWalletError::WrongBranch)
        );
        assert_eq!(
            ImportedWalletDescriptor::parse(
                &single(0, Network::Bitcoin, false),
                Some(&single(1, Network::Bitcoin, false)),
                ChainId::Bitcoin,
            ),
            Err(ImportedWalletError::MissingOrigin)
        );
        assert_eq!(
            ImportedWalletDescriptor::parse(&receive, Some(&change), ChainId::Regtest),
            Err(ImportedWalletError::WrongNetwork)
        );
        let wrong_origin_receive = receive.replace("/84h/0h/0h]", "/84h/0h/1h]");
        let wrong_origin_change = change.replace("/84h/0h/0h]", "/84h/0h/1h]");
        assert_eq!(
            ImportedWalletDescriptor::parse(
                &wrong_origin_receive,
                Some(&wrong_origin_change),
                ChainId::Bitcoin,
            ),
            Err(ImportedWalletError::OriginMismatch)
        );
        let short_origin_receive = receive.replace("/84h/0h/0h]", "/84h/0h]");
        let short_origin_change = change.replace("/84h/0h/0h]", "/84h/0h]");
        assert_eq!(
            ImportedWalletDescriptor::parse(
                &short_origin_receive,
                Some(&short_origin_change),
                ChainId::Bitcoin
            ),
            Err(ImportedWalletError::OriginMismatch)
        );
        let checksum_bad = format!("{}#00000000", receive);
        assert_eq!(
            ImportedWalletDescriptor::parse(&checksum_bad, Some(&change), ChainId::Bitcoin),
            Err(ImportedWalletError::Descriptor)
        );
    }

    #[test]
    fn sortedmulti_pair_copies_matching_origin_but_multi_order_is_preserved() {
        let receive = multi("sortedmulti", [0, 1, 2], 0, None);
        let change = multi("sortedmulti", [2, 0, 1], 1, Some(1));
        let pair =
            ImportedWalletDescriptor::parse(&receive, Some(&change), ChainId::Bitcoin).unwrap();
        assert!(pair.change_descriptor().to_string().contains("/1/*"));
        let mut count = 0;
        pair.change_descriptor().for_each_key(|key| {
            if let DescriptorPublicKey::XPub(xpub) = key {
                assert_eq!(xpub.origin.as_ref().unwrap().1.len(), 4);
                count += 1;
            }
            true
        });
        assert_eq!(count, 3);
        assert_eq!(
            ImportedWalletDescriptor::parse(
                &multi("multi", [0, 1, 2], 0, None),
                Some(&multi("multi", [2, 0, 1], 1, None)),
                ChainId::Bitcoin,
            ),
            Err(ImportedWalletError::Unrelated)
        );
        assert_eq!(
            ImportedWalletDescriptor::parse(
                &multi("multi", [0, 0, 2], 0, None),
                Some(&multi("multi", [0, 0, 2], 1, None)),
                ChainId::Bitcoin,
            ),
            Err(ImportedWalletError::Descriptor)
        );
    }

    #[test]
    fn versioned_record_revalidates_before_deserialization() {
        let receive = single(0, Network::Regtest, true);
        let change = single(1, Network::Regtest, true);
        let imported =
            ImportedWalletDescriptor::parse(&receive, Some(&change), ChainId::Regtest).unwrap();
        let encoded = serde_json::to_value(WalletDescriptor::Imported(imported.clone())).unwrap();
        assert_eq!(encoded["kind"], "imported");
        assert_eq!(encoded["version"], 1);
        let restored: WalletDescriptor = serde_json::from_value(encoded.clone()).unwrap();
        assert_eq!(restored.as_imported(), Some(&imported));
        let mut invalid = encoded;
        invalid["change"] = invalid["receive"].clone();
        assert!(serde_json::from_value::<WalletDescriptor>(invalid).is_err());
        let mut unsupported = serde_json::to_value(imported).unwrap();
        unsupported["version"] = 2.into();
        assert!(serde_json::from_value::<WalletDescriptor>(unsupported.clone()).is_err());
        unsupported["multi_desc"] = unsupported["receive"].clone();
        unsupported["receive_desc"] = unsupported["receive"].clone();
        unsupported["change_desc"] = unsupported["change"].clone();
        assert!(serde_json::from_value::<WalletDescriptor>(unsupported).is_err());
    }

    #[test]
    fn every_supported_shape_roundtrips_exact_scripts_at_multiple_indexes() {
        let key = key(1, "m/84h/1h/0h", Network::Regtest, true);
        let cases = [
            (format!("pkh({key}/0/*)"), format!("pkh({key}/1/*)")),
            (format!("wpkh({key}/0/*)"), format!("wpkh({key}/1/*)")),
            (
                format!("sh(wpkh({key}/0/*))"),
                format!("sh(wpkh({key}/1/*))"),
            ),
            (
                multi("multi", [0, 1, 2], 0, None),
                multi("multi", [0, 1, 2], 1, None),
            ),
            (
                multi("sortedmulti", [0, 1, 2], 0, None),
                multi("sortedmulti", [0, 1, 2], 1, None),
            ),
        ];
        for (receive, change) in cases {
            let chain = if receive.starts_with("wsh") {
                ChainId::Bitcoin
            } else {
                ChainId::Regtest
            };
            let imported = ImportedWalletDescriptor::parse(&receive, Some(&change), chain).unwrap();
            let encoded = serde_json::to_value(&imported).unwrap();
            let restored: ImportedWalletDescriptor = serde_json::from_value(encoded).unwrap();
            assert_eq!(restored.identity(chain), imported.identity(chain));
            for index in [0, 1, 1_000] {
                assert_eq!(
                    restored
                        .receive_descriptor()
                        .at_derivation_index(index)
                        .unwrap()
                        .script_pubkey(),
                    imported
                        .receive_descriptor()
                        .at_derivation_index(index)
                        .unwrap()
                        .script_pubkey(),
                );
                assert_eq!(
                    restored
                        .change_descriptor()
                        .at_derivation_index(index)
                        .unwrap()
                        .script_pubkey(),
                    imported
                        .change_descriptor()
                        .at_derivation_index(index)
                        .unwrap()
                        .script_pubkey(),
                );
            }
        }
    }

    #[test]
    fn full_identity_binds_both_branches_and_exact_chain() {
        let receive = single(0, Network::Regtest, true);
        let change = single(1, Network::Regtest, true);
        let regtest =
            ImportedWalletDescriptor::parse(&receive, Some(&change), ChainId::Regtest).unwrap();
        let testnet4 =
            ImportedWalletDescriptor::parse(&receive, Some(&change), ChainId::Testnet4).unwrap();
        assert_ne!(
            regtest.identity(ChainId::Regtest).unwrap(),
            testnet4.identity(ChainId::Testnet4).unwrap()
        );
        assert_eq!(
            regtest.identity(ChainId::Regtest).unwrap().to_string(),
            "01b5fac36b1f4772dfdd17edbf238b83b076b3b6b9901b2fca92279f1750f641",
        );
    }

    #[test]
    fn vault_serde_stays_the_legacy_object_and_rejects_inconsistent_branches() {
        let vault = CoincubeDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(1),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        let legacy = serde_json::to_value(&vault).unwrap();
        let wrapped = serde_json::to_value(WalletDescriptor::Vault(vault.clone())).unwrap();
        assert_eq!(wrapped, legacy);
        let restored: WalletDescriptor = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(restored.as_vault(), Some(&vault));
        let mut bad = legacy;
        bad["change_desc"] = bad["receive_desc"].clone();
        assert!(serde_json::from_value::<WalletDescriptor>(bad).is_err());
    }
}
