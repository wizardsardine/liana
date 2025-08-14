use log::warn;
use miniscript::{
    bitcoin::{
        self,
        bip32::{self, Fingerprint},
        constants::WITNESS_SCALE_FACTOR,
        psbt::{Input as PsbtIn, Output as PsbtOut, Psbt},
        secp256k1,
    },
    descriptor,
    miniscript::satisfy::Placeholder,
    plan::{Assets, CanSign},
    psbt::{PsbtInputExt, PsbtOutputExt},
    translate_hash_clone, ForEachKey, TranslatePk, Translator,
};

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    convert::TryInto,
    error, fmt,
    str::{self, FromStr},
};

use serde::{Deserialize, Serialize};

pub mod keys;
pub use keys::*;

pub mod analysis;
pub use analysis::*;

#[derive(Debug)]
pub enum LianaDescError {
    Miniscript(miniscript::Error),
    DescKey(DescKeyError),
    Policy(LianaPolicyError),
    /// Different number of PSBT vs tx inputs, etc..
    InsanePsbt,
    /// Not all inputs' sequence the same, not all inputs signed with the same key, ..
    InconsistentPsbt,
}

impl std::fmt::Display for LianaDescError {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Miniscript(e) => write!(f, "Miniscript error: '{}'.", e),
            Self::DescKey(e) => write!(f, "{}", e),
            Self::Policy(e) => write!(f, "{}", e),
            Self::InsanePsbt => write!(f, "Analyzed PSBT is empty or malformed."),
            Self::InconsistentPsbt => write!(f, "Analyzed PSBT is inconsistent across inputs."),
        }
    }
}

impl error::Error for LianaDescError {}

impl From<LianaPolicyError> for LianaDescError {
    fn from(e: LianaPolicyError) -> LianaDescError {
        LianaDescError::Policy(e)
    }
}

fn varint_len(n: usize) -> usize {
    bitcoin::VarInt(n as u64).size()
}

// Whether the key identified by its fingerprint+derivation path was derived from one of the xpubs
// for this spending path.
fn key_is_for_path(
    path_origins: &HashMap<bip32::Fingerprint, HashSet<bip32::DerivationPath>>,
    fg: &bip32::Fingerprint,
    der_path: &bip32::DerivationPath,
) -> bool {
    // Does it come from a signer used in this spending path?
    if let Some(der_paths) = path_origins.get(fg) {
        // Get the derivation path from the master fingerprint to the parent used to
        // derive this key, in order to check whether it's part of the derivation paths
        // used in this spending path (only checking the fingerprint isn't sufficient
        // as a single signer may be used in more than one spending path).
        // NOTE: this assumes there is only one derivation step after the key used in
        // the policy. This is fine, because the keys in the policy are normalized (so
        // the derivation path up to the wildcard is part of the origin).
        if let Some((_, der_path_no_wildcard)) = der_path[..].split_last() {
            return der_paths.contains(&der_path_no_wildcard.into());
        }
    }
    false
}

/// An [SinglePathLianaDesc] that contains multipath keys for (and only for) the receive keychain
/// and the change keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LianaDescriptor {
    multi_desc: descriptor::Descriptor<descriptor::DescriptorPublicKey>,
    receive_desc: SinglePathLianaDesc,
    change_desc: SinglePathLianaDesc,
}

/// A Miniscript descriptor with a main, unencombered, branch (the main owner of the coins)
/// and a timelocked branch (the heir). All keys in this descriptor are singlepath.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinglePathLianaDesc(descriptor::Descriptor<descriptor::DescriptorPublicKey>);

/// Derived (containing only raw Bitcoin public keys) version of the inheritance descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DerivedSinglePathLianaDesc(descriptor::Descriptor<DerivedPublicKey>);

impl fmt::Display for LianaDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.multi_desc)
    }
}

impl str::FromStr for LianaDescriptor {
    type Err = LianaDescError;

    fn from_str(s: &str) -> Result<LianaDescriptor, Self::Err> {
        // Parse a descriptor and check it is a multipath descriptor corresponding to a valid Liana
        // spending policy.
        // Sanity checks are not always performed when calling `Descriptor::from_str`, so we perform
        // them explicitly. See https://github.com/rust-bitcoin/rust-miniscript/issues/734.
        let desc = descriptor::Descriptor::<descriptor::DescriptorPublicKey>::from_str(s)
            .and_then(|desc| desc.sanity_check().map(|_| desc))
            .map_err(LianaDescError::Miniscript)?;
        LianaPolicy::from_multipath_descriptor(&desc)?;

        // Compute the receive and change "sub" descriptors right away. According to our pubkey
        // check above, there must be only two of those, 0 and 1.
        // We use /0/* for receiving and /1/* for change.
        // FIXME: don't rely on into_single_descs()'s ordering.
        let mut singlepath_descs = desc
            .clone()
            .into_single_descriptors()
            .expect("Can't error, all paths have the same length")
            .into_iter();
        assert_eq!(singlepath_descs.len(), 2);
        let receive_desc = SinglePathLianaDesc(singlepath_descs.next().expect("First of 2"));
        let change_desc = SinglePathLianaDesc(singlepath_descs.next().expect("Second of 2"));

        Ok(LianaDescriptor {
            multi_desc: desc,
            receive_desc,
            change_desc,
        })
    }
}

impl fmt::Display for SinglePathLianaDesc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl PartialEq<descriptor::Descriptor<descriptor::DescriptorPublicKey>> for SinglePathLianaDesc {
    fn eq(&self, other: &descriptor::Descriptor<descriptor::DescriptorPublicKey>) -> bool {
        self.0.eq(other)
    }
}

/// The index of a change output in a transaction's outputs list, differentiating between a change
/// output which uses a change address and one which uses a deposit address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChangeOutput {
    ChangeAddress { index: usize },
    DepositAddress { index: usize },
}

impl ChangeOutput {
    /// Get the index of the change output in the transaction's list of outputs regardless of its
    /// type.
    pub fn index(&self) -> usize {
        match self {
            Self::ChangeAddress { index } => *index,
            Self::DepositAddress { index } => *index,
        }
    }
}

impl LianaDescriptor {
    pub fn new(spending_policy: LianaPolicy) -> LianaDescriptor {
        // Get the descriptor from the chosen spending policy.
        let multi_desc = spending_policy.into_multipath_descriptor();

        // Compute the receive and change "sub" descriptors right away. According to our pubkey
        // check above, there must be only two of those, 0 and 1.
        // We use /0/* for receiving and /1/* for change.
        // FIXME: don't rely on into_single_descs()'s ordering.
        let mut singlepath_descs = multi_desc
            .clone()
            .into_single_descriptors()
            .expect("Can't error, all paths have the same length")
            .into_iter();
        assert_eq!(singlepath_descs.len(), 2);
        let receive_desc = SinglePathLianaDesc(singlepath_descs.next().expect("First of 2"));
        let change_desc = SinglePathLianaDesc(singlepath_descs.next().expect("Second of 2"));

        LianaDescriptor {
            multi_desc,
            receive_desc,
            change_desc,
        }
    }

    /// Whether all xpubs contained in this descriptor are for the passed expected network.
    pub fn all_xpubs_net_is(&self, expected_net: bitcoin::Network) -> bool {
        self.multi_desc.for_each_key(|xpub| {
            if let descriptor::DescriptorPublicKey::MultiXPub(xpub) = xpub {
                xpub.xkey.network == expected_net.into()
            } else {
                false
            }
        })
    }

    /// Whether a key matching this fingerprint is part of this descriptor
    pub fn contains_fingerprint(&self, fg: Fingerprint) -> bool {
        self.multi_desc
            .for_any_key(|k| k.master_fingerprint() == fg)
    }

    /// Determine whether the fingerprint is part of a specific path of this descriptor.
    /// If recovery_timelock is None, checks in the primary path.
    /// If recovery_timelock is Some(timelock), checks in the recovery path with specified timelock.
    pub fn contains_fingerprint_in_path(
        &self,
        fingerprint: Fingerprint,
        recovery_timelock: Option<u16>,
    ) -> bool {
        match recovery_timelock {
            None => self.contains_fingerprint_in_primary_path(fingerprint),
            Some(timelock) => self.contains_fingerprint_in_recovery_path(fingerprint, timelock),
        }
    }

    /// Determine whether the fingerprint is part of the primary path of this descriptor.
    fn contains_fingerprint_in_primary_path(&self, fingerprint: Fingerprint) -> bool {
        self.policy().primary_path.contains_fingerprint(fingerprint)
    }

    /// Determine whether the fingerprint is part of the recovery path of this descriptor for the
    /// specified timelock.
    fn contains_fingerprint_in_recovery_path(
        &self,
        fingerprint: Fingerprint,
        recovery_timelock: u16,
    ) -> bool {
        self.policy()
            .recovery_paths
            .get(&recovery_timelock)
            .map(|path_info| path_info.contains_fingerprint(fingerprint))
            .unwrap_or(false)
    }

    /// Get the descriptor for receiving addresses.
    pub fn receive_descriptor(&self) -> &SinglePathLianaDesc {
        &self.receive_desc
    }

    /// Get the descriptor for change addresses.
    pub fn change_descriptor(&self) -> &SinglePathLianaDesc {
        &self.change_desc
    }

    /// Get the spending policy of this descriptor.
    pub fn policy(&self) -> LianaPolicy {
        LianaPolicy::from_multipath_descriptor(&self.multi_desc)
            .expect("We never create a Liana descriptor with an invalid Liana policy.")
    }

    /// Get the value (in blocks) of the smallest relative timelock of the recovery paths.
    pub fn first_timelock_value(&self) -> u16 {
        *self
            .policy()
            .recovery_paths
            .iter()
            .next()
            .expect("There is always at least one recovery path")
            .0
    }

    /// Get the maximum size difference of a transaction input spending a Script derived from this
    /// descriptor before and after satisfaction. The returned value is in weight units.
    /// Callers are expected to account for the Segwit marker (2 WU). This takes into account the
    /// size of the witness stack length varint.
    pub fn max_sat_weight(&self, use_primary_path: bool) -> usize {
        if use_primary_path {
            // Get the keys from the primary path, to get a satisfaction size estimation only
            // considering those.
            let keys = self
                .policy()
                .primary_path
                .thresh_origins()
                .1
                .into_iter()
                .fold(BTreeSet::new(), |mut keys, (fg, der_paths)| {
                    for der_path in der_paths {
                        keys.insert(((fg, der_path), CanSign::default()));
                    }
                    keys
                });
            let assets = Assets {
                keys,
                ..Default::default()
            };

            // Unfortunately rust-miniscript satisfaction size estimation is inconsistent. For
            // Taproot it considers the whole witness (except the control block size + the
            // script size), while under P2WSH it does not consider the witscript! Therefore we
            // manually add the size of the witscript under P2WSH by means of the
            // `explicit_script()` helper, which gives an error for Taproot, and for Taproot
            // we add the sizes of the control block and script.
            let der_desc = self
                .receive_desc
                .0
                .at_derivation_index(0)
                .expect("unhardened index");
            let witscript_size = der_desc
                .explicit_script()
                .map(|s| varint_len(s.len()) + s.len());

            // Finally, compute the satisfaction template for the primary path and get its size.
            let plan = der_desc.plan(&assets).expect("Always satisfiable");
            plan.witness_size()
                + witscript_size.unwrap_or_else(|_| {
                    plan.witness_template()
                        .iter()
                        .map(|elem| match elem {
                            // We need to calculate the size manually before calculating the varint length.
                            // See https://docs.rs/miniscript/11.0.0/src/miniscript/util.rs.html#35-36.
                            Placeholder::TapScript(s) => varint_len(s.len()),
                            Placeholder::TapControlBlock(cb) => varint_len(cb.serialize().len()),
                            _ => 0,
                        })
                        .sum()
                })
        } else {
            // We add one to account for the witness stack size, as the values above give the
            // difference in size for a satisfied input that was *already* in a transaction
            // that spent one or more Segwit coins (and thus already have 1 WU accounted for the
            // empty witness). But this method is used to account between a completely "nude"
            // transaction (and therefore no Segwit marker nor empty witness in inputs) and a
            // satisfied transaction.
            (self
                .multi_desc
                .max_weight_to_satisfy()
                .expect("Always satisfiable")
                .to_wu()
                + 1)
            .try_into()
            .expect("Sat weight must fit in usize.")
        }
    }

    /// Get the maximum size difference of a transaction input spending a Script derived from this
    /// descriptor before and after satisfaction. The returned value is in (rounded up) virtual
    /// bytes.
    /// Callers are expected to account for the Segwit marker (2 WU). This takes into account the
    /// size of the witness stack length varint.
    pub fn max_sat_vbytes(&self, use_primary_path: bool) -> usize {
        self.max_sat_weight(use_primary_path)
            .checked_add(WITNESS_SCALE_FACTOR - 1)
            .unwrap()
            .checked_div(WITNESS_SCALE_FACTOR)
            .unwrap()
    }

    /// Get the maximum size in virtual bytes of the whole input in a transaction spending
    /// a coin with this Script.
    pub fn spender_input_size(&self, use_primary_path: bool) -> usize {
        // txid + vout + nSequence + empty scriptSig + witness
        32 + 4 + 4 + 1 + self.max_sat_vbytes(use_primary_path)
    }

    /// Whether this is a Taproot descriptor.
    pub fn is_taproot(&self) -> bool {
        matches!(self.multi_desc, descriptor::Descriptor::Tr(..))
    }

    /// Get some information about a PSBT input spending Liana coins.
    /// This analysis assumes that:
    /// - The PSBT input actually spend a Liana coin for this descriptor. Otherwise the analysis will be off.
    /// - The signatures contained in the PSBT input are valid for this script.
    pub fn partial_spend_info_txin(
        &self,
        psbt_in: &PsbtIn,
        txin: &bitcoin::TxIn,
    ) -> PartialSpendInfo {
        let is_taproot = self.is_taproot();
        // Get the origin ECDSA or Schnorr signatures, depending on the descriptor type.
        let pubkeys_signed = (!is_taproot)
            .then(|| {
                // ECDSA sigs.
                psbt_in
                    .partial_sigs
                    .iter()
                    .filter_map(|(pk, _)| psbt_in.bip32_derivation.get(&pk.inner))
            })
            .into_iter()
            .flatten()
            .chain(
                is_taproot
                    .then(|| {
                        // Tapscript Schnorr sigs.
                        psbt_in
                            .tap_script_sigs
                            .iter()
                            .filter_map(|((pk, _), _)| {
                                psbt_in.tap_key_origins.get(pk).map(|or| &or.1)
                            })
                            // Tapkey Schnorr sig.
                            .chain(psbt_in.tap_key_sig.and_then(|_| {
                                psbt_in
                                    .tap_internal_key
                                    .and_then(|pk| psbt_in.tap_key_origins.get(&pk).map(|or| &or.1))
                            }))
                    })
                    .into_iter()
                    .flatten(),
            );

        // Determine the structure of the descriptor. Then compute the spend info for the primary
        // and recovery paths. Only provide the spend info for the recovery path if it is available
        // (ie if the nSequence is >= to the chosen CSV value).
        let desc_info = self.policy();
        let primary_path = desc_info.primary_path.spend_info(pubkeys_signed.clone());
        let recovery_paths = desc_info
            .recovery_paths
            .iter()
            .filter_map(|(timelock, path_info)| {
                if txin.sequence.is_height_locked() && txin.sequence.0 >= *timelock as u32 {
                    Some((*timelock, path_info.spend_info(pubkeys_signed.clone())))
                } else {
                    None
                }
            })
            .collect();

        PartialSpendInfo {
            primary_path,
            recovery_paths,
        }
    }

    // TODO: decide whether we should check the signatures too. To be useful it should check pubkeys
    // correspond to those in the script. And we could be checking the witness scripts are all for
    // our descriptor too..
    /// Get some information about a PSBT spending Liana coins.
    /// This analysis assumes that:
    /// - The PSBT only contains input that spends Liana coins. Otherwise the analysis will be off.
    /// - The PSBT is consistent across inputs (the sequence is the same across inputs, the
    ///   signatures are either absent or present for all inputs, ..)
    /// - The provided signatures are valid for this script.
    pub fn partial_spend_info(&self, psbt: &Psbt) -> Result<PartialSpendInfo, LianaDescError> {
        // Check the PSBT isn't empty or malformed.
        if psbt.inputs.len() != psbt.unsigned_tx.input.len()
            || psbt.outputs.len() != psbt.unsigned_tx.output.len()
            || psbt.inputs.is_empty()
            || psbt.outputs.is_empty()
        {
            return Err(LianaDescError::InsanePsbt);
        }

        // We are doing this analysis at a transaction level. We assume that if an input
        // is set to use the recovery path, all are. If one input is signed with a key, all
        // must be.
        // This gets the information needed to analyze the number of signatures from the
        // first input, and checks that this info matches on all inputs.
        let (mut psbt_ins, mut txins) = (psbt.inputs.iter(), psbt.unsigned_tx.input.iter());
        let (first_psbt_in, first_txin) = (
            psbt_ins
                .next()
                .expect("We checked at least one is present."),
            txins.next().expect("We checked at least one is present."),
        );
        let spend_info = self.partial_spend_info_txin(first_psbt_in, first_txin);
        for (psbt_in, txin) in psbt_ins.zip(txins) {
            // TODO: maybe it's better to not error if one of the input has more, or different
            // signatures? Instead of erroring we could ignore the superfluous data?
            if txin.sequence != first_txin.sequence
                || spend_info != self.partial_spend_info_txin(psbt_in, txin)
            {
                // TODO(arturgontijo): Skip for now
                warn!("LianaDescError::InconsistentPsbt: Not throwing...");
                // return Err(LianaDescError::InconsistentPsbt);
            }
        }

        Ok(spend_info)
    }

    /// List the indexes of the change outputs in this PSBT. It relies on the PSBT to be
    /// well-formed: sane BIP32 derivations must be set for every change output, the inner
    /// transaction must have the same number of outputs as the PSBT.
    /// Will detect change outputs paying to either the change keychain or the deposit one.
    pub fn change_indexes(
        &self,
        psbt: &Psbt,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
    ) -> Vec<ChangeOutput> {
        let mut indexes = Vec::new();

        // We iterate through all the BIP32 derivations of each output, but note we only ever set
        // the BIP32 derivations for PSBT outputs which pay to ourselves.
        for (index, psbt_out) in psbt.outputs.iter().enumerate() {
            // We can only ever detect change on well-formed PSBTs. On such PSBTs, all keys in the
            // BIP32 derivations belong to us. And they all use the same last derivation index,
            // since that's where the wildcard is in the descriptor. So just pick the first one and
            // infer the derivation index to use to derive the spks below from it.
            let wsh_der_index = psbt_out
                .bip32_derivation
                .values()
                .next()
                .map(|(_, der_path)| der_path);
            let tap_der_index = psbt_out
                .tap_key_origins
                .values()
                .next()
                .map(|(_, (_, der_path))| der_path);
            let der_index = if let Some(i) = wsh_der_index
                .into_iter()
                .chain(tap_der_index.into_iter())
                .next()
                .and_then(|der_path| der_path.into_iter().last())
            {
                i
            } else {
                continue;
            };

            // If any of the change and deposit addresses at this derivation index match, count it
            // as a change output.
            if let Some(txo) = psbt.unsigned_tx.output.get(index) {
                let change_desc = self.change_desc.derive(*der_index, secp);
                if change_desc.script_pubkey() == txo.script_pubkey {
                    indexes.push(ChangeOutput::ChangeAddress { index });
                    continue;
                }
                let receive_desc = self.receive_desc.derive(*der_index, secp);
                if receive_desc.script_pubkey() == txo.script_pubkey {
                    indexes.push(ChangeOutput::DepositAddress { index });
                }
            } else {
                log::error!(
                    "Provided a PSBT with non-matching tx outputs count and PSBT outputs count."
                );
            }
        }

        indexes
    }

    /// Prune the BIP32 derivations in all the PSBT inputs for all the spending paths but the given
    /// one.
    pub fn prune_bip32_derivs(&self, mut psbt: Psbt, spending_path: &PathInfo) -> Psbt {
        // (Fingerprint, derivation path) pairs uniquely identify a key used in this spending path.
        let (_, path_origins) = spending_path.thresh_origins();

        // Go through all the PSBT inputs and drop the BIP32 derivations for keys that are not from
        // this spending path.
        for psbt_in in psbt.inputs.iter_mut() {
            // Perform it for both legacy and Taproot origins, as if one is set the other should be
            // empty so it's a noop.
            psbt_in
                .bip32_derivation
                .retain(|_, (fg, der_path)| key_is_for_path(&path_origins, fg, der_path));
            psbt_in
                .tap_key_origins
                .retain(|_, (_, (fg, der_path))| key_is_for_path(&path_origins, fg, der_path));
        }

        psbt
    }

    /// Prune the BIP32 derivations in all the PSBT inputs for all the spending paths but the
    /// latest available one. For instance:
    /// - If there is two recovery paths, and the PSBT's first input nSequence isn't set to unlock
    ///   any of them, prune all but the primary path's bip32 derivations.
    /// - If there is two recovery paths, and the PSBT's first input nSequence is set to unlock the
    ///     first one, prune all but the first recovery path's bip32 derivations.
    /// - Etc..
    pub fn prune_bip32_derivs_last_avail(&self, psbt: Psbt) -> Result<Psbt, LianaDescError> {
        let spend_info = self.partial_spend_info(&psbt)?;
        let policy = self.policy();
        let path_info = spend_info
            .recovery_paths
            .iter()
            .last()
            .map(|(tl, _)| {
                policy
                    .recovery_paths
                    .get(tl)
                    .expect("Same timelocks must be keys in both mappings.")
            })
            .unwrap_or(&policy.primary_path);
        Ok(self.prune_bip32_derivs(psbt, path_info))
    }

    /// Maximum possible weight in weight units of an unsigned transaction, `tx`,
    /// after satisfaction, assuming all inputs of `tx` are from this
    /// descriptor.
    fn unsigned_tx_max_weight(&self, tx: &bitcoin::Transaction, use_primary_path: bool) -> u64 {
        let num_inputs: u64 = tx.input.len().try_into().unwrap();
        let max_sat_weight: u64 = self.max_sat_weight(use_primary_path).try_into().unwrap();
        // Add weights together before converting to vbytes to avoid rounding up multiple times.
        tx.weight()
            .to_wu()
            .checked_add(max_sat_weight.checked_mul(num_inputs).unwrap())
            .and_then(|weight| {
                weight.checked_add(
                    // Make sure the Segwit marker and flag are included:
                    // https://docs.rs/bitcoin/0.31.0/src/bitcoin/blockdata/transaction.rs.html#752-753
                    // https://docs.rs/bitcoin/0.31.0/src/bitcoin/blockdata/transaction.rs.html#968-979
                    if num_inputs > 0 && tx.input.iter().all(|txin| txin.witness.is_empty()) {
                        2
                    } else {
                        0
                    },
                )
            })
            .unwrap()
    }

    /// Maximum possible size in vbytes of an unsigned transaction, `tx`,
    /// after satisfaction, assuming all inputs of `tx` are from this
    /// descriptor.
    pub fn unsigned_tx_max_vbytes(&self, tx: &bitcoin::Transaction, use_primary_path: bool) -> u64 {
        let witness_factor: u64 = WITNESS_SCALE_FACTOR.try_into().unwrap();
        self.unsigned_tx_max_weight(tx, use_primary_path)
            .checked_add(witness_factor.checked_sub(1).unwrap())
            .unwrap()
            .checked_div(witness_factor)
            .unwrap()
    }
}

impl SinglePathLianaDesc {
    /// Derive this descriptor at a given index for a receiving address.
    ///
    /// # Panics
    /// - If the given index is hardened.
    pub fn derive(
        &self,
        index: bip32::ChildNumber,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
    ) -> DerivedSinglePathLianaDesc {
        assert!(index.is_normal());

        // Unfortunately we can't just use `self.0.at_derivation_index().derived_descriptor()`
        // since it would return a raw public key, but we need the origin too.
        // TODO: upstream our DerivedPublicKey stuff to rust-miniscript.
        //
        // So we roll our own translation.
        struct Derivator<'a, C: secp256k1::Verification>(u32, &'a secp256k1::Secp256k1<C>);
        impl<'a, C: secp256k1::Verification>
            Translator<
                descriptor::DescriptorPublicKey,
                DerivedPublicKey,
                descriptor::ConversionError,
            > for Derivator<'a, C>
        {
            fn pk(
                &mut self,
                pk: &descriptor::DescriptorPublicKey,
            ) -> Result<DerivedPublicKey, descriptor::ConversionError> {
                let definite_key = pk
                    .clone()
                    .at_derivation_index(self.0)
                    .expect("We disallow multipath keys.");
                let origin = (
                    definite_key.master_fingerprint(),
                    definite_key
                        .full_derivation_path()
                        .expect("We disallow multipath keys."),
                );
                let key = definite_key.derive_public_key(self.1)?;
                Ok(DerivedPublicKey { origin, key })
            }
            translate_hash_clone!(
                descriptor::DescriptorPublicKey,
                DerivedPublicKey,
                descriptor::ConversionError
            );
        }

        DerivedSinglePathLianaDesc(
            self.0
                .translate_pk(&mut Derivator(index.into(), secp))
                .expect(
                    "May only fail on hardened derivation indexes, but we ruled out this case.",
                ),
        )
    }

    /// Reference to the underlying `Descriptor<descriptor::DescriptorPublicKey>`
    pub fn as_descriptor_public_key(
        &self,
    ) -> &descriptor::Descriptor<descriptor::DescriptorPublicKey> {
        &self.0
    }
}

pub enum DescKeysOrigins {
    Wsh(BTreeMap<secp256k1::PublicKey, (bip32::Fingerprint, bip32::DerivationPath)>),
    Tr(BTreeMap<secp256k1::PublicKey, (bip32::Fingerprint, bip32::DerivationPath)>),
}

/// Map of a raw public key to the xpub used to derive it and its derivation path
pub type Bip32Deriv = BTreeMap<secp256k1::PublicKey, (bip32::Fingerprint, bip32::DerivationPath)>;

impl DerivedSinglePathLianaDesc {
    pub fn address(&self, network: bitcoin::Network) -> bitcoin::Address {
        self.0
            .address(network)
            .expect("A P2WSH always has an address")
    }

    pub fn script_pubkey(&self) -> bitcoin::ScriptBuf {
        self.0.script_pubkey()
    }

    // NB: panics if called for a Taproot descriptor.
    fn witness_script(&self) -> bitcoin::ScriptBuf {
        self.0.explicit_script().expect("Not a Taproot descriptor")
    }

    // NB: panics if called for a Taproot descriptor.
    fn bip32_derivations(&self) -> Bip32Deriv {
        let ms = match self.0 {
            descriptor::Descriptor::Wsh(ref wsh) => match wsh.as_inner() {
                descriptor::WshInner::Ms(ms) => ms,
                descriptor::WshInner::SortedMulti(_) => {
                    unreachable!("None of our descriptors is a sorted multi")
                }
            },
            _ => unreachable!("Must never be called for a Taproot descriptor."),
        };

        // For DerivedPublicKey, Pk::Hash == Self.
        ms.iter_pk()
            .map(|k| (k.key.inner, (k.origin.0, k.origin.1)))
            .collect()
    }

    // FIXME: update_with_descriptor() needs a Descriptor<DefiniteKey>. This is a temporary hack to
    // avoid having to duplicate the cumbersome logic here. Could use translate_pk() instead in the
    // future.
    fn definite_desc(&self) -> descriptor::Descriptor<descriptor::DefiniteDescriptorKey> {
        descriptor::Descriptor::<_>::from_str(&self.0.to_string()).expect("Must roundtrip")
    }

    /// Update the PSBT input information with data from this derived descriptor.
    pub fn update_psbt_in(&self, psbtin: &mut PsbtIn) {
        match self.0 {
            descriptor::Descriptor::Wsh(_) => {
                psbtin.bip32_derivation = self.bip32_derivations();
                psbtin.witness_script = Some(self.witness_script());
            }
            descriptor::Descriptor::Tr(_) => {
                let desc = self.definite_desc();
                if let Err(e) = psbtin.update_with_descriptor_unchecked(&desc) {
                    log::error!("BUG! Please report this! Error when adding key origins for desc: {}. Descriptor: {}.", e, desc);
                }
            }
            _ => unreachable!("Only ever a wsh() or a tr() descriptor."),
        }
    }

    /// Update the info of a PSBT output for a change output with data from this derived
    /// descriptor.
    pub fn update_change_psbt_out(&self, psbtout: &mut PsbtOut) {
        match self.0 {
            descriptor::Descriptor::Wsh(_) => {
                psbtout.bip32_derivation = self.bip32_derivations();
            }
            descriptor::Descriptor::Tr(_) => {
                let desc = self.definite_desc();
                if let Err(e) = psbtout.update_with_descriptor_unchecked(&desc) {
                    log::error!("BUG! Please report this! Error when adding key origins for desc: {}. Descriptor: {}.", e, desc);
                }
            }
            _ => unreachable!("Only ever a wsh() or a tr() descriptor."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::signer::HotSigner;
    use bitcoin::{hashes::Hash, Sequence};
    use miniscript::bitcoin::bip32::Fingerprint;

    fn random_desc_key(
        secp: &secp256k1::Secp256k1<impl secp256k1::Signing>,
    ) -> descriptor::DescriptorPublicKey {
        let signer = HotSigner::generate(bitcoin::Network::Bitcoin).unwrap();
        let xpub_str = format!(
            "[{}]{}/<0;1>/*",
            signer.fingerprint(secp),
            signer.xpub_at(&bip32::DerivationPath::from_str("m").unwrap(), secp)
        );
        descriptor::DescriptorPublicKey::from_str(&xpub_str).unwrap()
    }

    // Convert a size in weight units to a size in virtual bytes, rounding up.
    fn wu_to_vb(vb: usize) -> usize {
        (vb + WITNESS_SCALE_FACTOR - 1)
            .checked_div(WITNESS_SCALE_FACTOR)
            .expect("Non 0")
    }

    #[test]
    fn descriptor_creation() {
        // Simple 1 primary key, 1 recovery key.
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap());
        let timelock = 52560;
        let policy = LianaPolicy::new_legacy(
            owner_key.clone(),
            [(timelock, heir_key.clone())].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "wsh(or_d(pk([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:pkh([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560))))#g7vk9r5l");

        // Same under Taproot.
        let policy = LianaPolicy::new(
            owner_key.clone(),
            [(timelock, heir_key.clone())].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "tr([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,and_v(v:pk([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560)))#0mt7e93c");

        // A 3-of-3 multisig decaying into a 2-of-3 multisig after 6 months. Trying to mimic a
        // real situation, we use keys from 3 different origins (in practice, 3 different devices
        // held by 3 different persons).
        // Since there is 2 keys per origin, we increase a unhardened derivation step we put before
        // the change/receive derivation step.
        let primary_keys = PathInfo::Multi(
            3,
            vec![
                descriptor::DescriptorPublicKey::from_str("[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*").unwrap()
            ]
        );
        let recovery_keys = PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*").unwrap(),
            ],
        );
        let policy = LianaPolicy::new_legacy(
            primary_keys.clone(),
            [(26352, recovery_keys.clone())].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "wsh(or_i(and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*)),older(26352)),and_v(v:and_v(v:pk([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*),pk([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*)),pk([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*))))#c7nf353n");

        // Same under Taproot.
        let policy = LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy.clone()).to_string(), "tr(xpub661MyMwAqRbcFERisZuMzFcfg3Ur3dKB17kb8iEG89ZJYMHTWqKQGRdLjTXC6Byr8kjKo6JabFfRCm3ETM4woq7DxUXuUxxRFHfog4Peh41/<0;1>/*,{and_v(v:multi_a(2,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*),older(26352)),and_v(v:and_v(v:pk([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*),pk([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*)),pk([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*))})#eey6zfhr");

        // Another derivation step before the wildcard is taken into account.
        // desc_b is the very same descriptor as desc_a, except the very first xpub's derivation
        // path is `/<0;1>/*` instead of `/0/<0;1>/*`.
        let secp = secp256k1::Secp256k1::verification_only();
        let desc_a = LianaDescriptor::from_str("wsh(or_d(multi(3,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*),and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*)),older(26352))))").unwrap();
        let desc_b = LianaDescriptor::from_str("wsh(or_d(multi(3,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*),and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*)),older(26352))))").unwrap();
        let a = desc_a.receive_descriptor().derive(0.into(), &secp);
        let b = desc_b.receive_descriptor().derive(0.into(), &secp);
        assert_ne!(a, b);
        assert_ne!(a.script_pubkey(), b.script_pubkey());

        // The same pseudo-realistic situation as above, but instead of using another derivation
        // depth to derive from the same xpub, we reuse the multipath step.
        // Note: this is required by Ledger and the wallet policies BIP
        // (https://github.com/bitcoin/bips/pull/1389)
        let primary_keys = PathInfo::Multi(
            3,
            vec![
                descriptor::DescriptorPublicKey::from_str("[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*").unwrap()
            ]
        );
        let recovery_keys = PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<2;3>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<2;3>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<2;3>/*").unwrap(),
            ],
        );
        let policy = LianaPolicy::new_legacy(
            primary_keys.clone(),
            [(26352, recovery_keys.clone())].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "wsh(or_i(and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<2;3>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<2;3>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<2;3>/*)),older(26352)),and_v(v:and_v(v:pk([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),pk([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*)),pk([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*))))#tjdnx6vm");

        // Same under Taproot.
        let policy = LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy.clone()).to_string(), "tr(xpub661MyMwAqRbcFERisZuMzFcfg3Ur3dKB17kb8iEG89ZJYMHTWqKQGRdLjTXC6Byr8kjKo6JabFfRCm3ETM4woq7DxUXuUxxRFHfog4Peh41/<0;1>/*,{and_v(v:multi_a(2,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<2;3>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<2;3>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<2;3>/*),older(26352)),and_v(v:and_v(v:pk([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),pk([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*)),pk([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*))})#d06ehu7c");

        // We prevent footguns with timelocks by requiring a u16. Note how the following wouldn't
        // compile:
        //LianaPolicy::new_legacy(owner_key.clone(), heir_key.clone(), 0x00_01_0f_00).unwrap_err();
        //LianaPolicy::new_legacy(owner_key.clone(), heir_key.clone(), (1 << 31) + 1).unwrap_err();
        //LianaPolicy::new_legacy(owner_key, heir_key, (1 << 22) + 1).unwrap_err();

        // You can't use a null timelock in Miniscript.
        LianaPolicy::new_legacy(owner_key, [(0, heir_key)].iter().cloned().collect()).unwrap_err();

        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/<0;1>/*").unwrap());
        let timelock = 57600;
        let policy = LianaPolicy::new_legacy(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "wsh(or_d(pk([aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/<0;1>/*),and_v(v:pkh([abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/<0;1>/*),older(57600))))#ak4cm093");

        // We can't pass a raw key, an xpub that is not deriveable, only hardened derivable,
        // without both the change and receive derivation paths, or with more than 2 different
        // derivation paths.
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/354").unwrap());
        LianaPolicy::new_legacy(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap_err();
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/*'").unwrap());
        LianaPolicy::new_legacy(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap_err();
        let heir_key = PathInfo::Single(
            descriptor::DescriptorPublicKey::from_str(
                "[abcdef01]02e24913be26dbcfdf8e8e94870b28725cdae09b448b6c127767bf0154e3a3c8e5",
            )
            .unwrap(),
        );
        LianaPolicy::new_legacy(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap_err();
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/*'").unwrap());
        LianaPolicy::new_legacy(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap_err();
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/<0;1;2>/*'").unwrap());
        LianaPolicy::new_legacy(owner_key, [(timelock, heir_key)].iter().cloned().collect())
            .unwrap_err();

        // And it's checked even in a multisig. For instance:
        let primary_keys = PathInfo::Multi(
            1,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/354").unwrap(),
            ]
        );
        let recovery_keys = PathInfo::Multi(
            1,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*").unwrap(),
            ],
        );
        LianaPolicy::new_legacy(
            primary_keys.clone(),
            [(26352, recovery_keys.clone())].iter().cloned().collect(),
        )
        .unwrap_err();

        // It's also checked under Taproot context.
        LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap_err();

        // You can't pass duplicate keys, even if they are encoded differently.
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        LianaPolicy::new_legacy(owner_key, [(timelock, heir_key)].iter().cloned().collect())
            .unwrap_err();
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[00aabb44]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        LianaPolicy::new_legacy(owner_key, [(timelock, heir_key)].iter().cloned().collect())
            .unwrap_err();
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[00aabb44]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[11223344/2/98]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        LianaPolicy::new_legacy(owner_key, [(timelock, heir_key)].iter().cloned().collect())
            .unwrap_err();

        // You can't pass duplicate keys, even across multisigs.
        let primary_keys = PathInfo::Multi(
            3,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef02]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef03]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*").unwrap()
            ]
        );
        let recovery_keys = PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef05]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef04]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef02]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
            ],
        );
        let err = LianaPolicy::new_legacy(
            primary_keys.clone(),
            [(26352, recovery_keys.clone())].iter().cloned().collect(),
        )
        .unwrap_err();
        assert!(matches!(err, LianaPolicyError::DuplicateKey(_)));

        // It's also checked under Taproot.
        let err = LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap_err();
        assert!(matches!(err, LianaPolicyError::DuplicateKey(_)));

        // You can't pass duplicate signers in the primary path.
        let primary_keys = PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
            ]
        );
        let recovery_keys = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef02]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*").unwrap());
        let err = LianaPolicy::new_legacy(
            primary_keys.clone(),
            [(26352, recovery_keys.clone())].iter().cloned().collect(),
        )
        .unwrap_err();
        assert!(matches!(err, LianaPolicyError::DuplicateOriginSamePath(_)));

        // It's also checked under Taproot.
        let err = LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap_err();
        assert!(matches!(err, LianaPolicyError::DuplicateOriginSamePath(_)));

        // You can't pass duplicate signers in the recovery path.
        let recovery_keys = PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
            ]
        );
        let primary_keys = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef02]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*").unwrap());
        let err = LianaPolicy::new_legacy(
            primary_keys.clone(),
            [(26352, recovery_keys.clone())].iter().cloned().collect(),
        )
        .unwrap_err();
        assert!(matches!(err, LianaPolicyError::DuplicateOriginSamePath(_)));

        // It's also checked under Taproot.
        let err = LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap_err();
        assert!(matches!(err, LianaPolicyError::DuplicateOriginSamePath(_)));

        // But the same signer can absolutely be used across spending paths.
        let primary_keys = PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[abcdef02]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*").unwrap(),
            ]
        );
        let recovery_keys = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*").unwrap());
        LianaPolicy::new_legacy(
            primary_keys.clone(),
            [(26352, recovery_keys.clone())].iter().cloned().collect(),
        )
        .unwrap();

        // It's also possible under Taproot.
        LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap();

        // No origin in one of the keys
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap());
        let timelock = 52560;
        LianaPolicy::new_legacy(
            owner_key.clone(),
            [(timelock, heir_key.clone())].iter().cloned().collect(),
        )
        .unwrap_err();
        LianaPolicy::new(owner_key, [(timelock, heir_key)].iter().cloned().collect()).unwrap_err();

        // One of the xpub isn't normalized.
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/42'/<0;1>/*").unwrap());
        let timelock = 52560;
        LianaPolicy::new_legacy(
            owner_key.clone(),
            [(timelock, heir_key.clone())].iter().cloned().collect(),
        )
        .unwrap_err();
        LianaPolicy::new(owner_key, [(timelock, heir_key)].iter().cloned().collect()).unwrap_err();

        // A 1-of-N multisig as primary path.
        LianaDescriptor::from_str("wsh(or_d(multi(1,[573fb35b/48'/1'/0'/2']tpubDFKp9T7WAYDcENSjoifkrpq1gMDF47KGJcJrpxzX23Qor8wuGbrEVs9utNq1MDS8E2WXJSBk1qoPQLpwyokW7DiUNPwFuxQkL7owNkLAb9W/<0;1>/*,[573fb35c/48'/1'/1'/2']tpubDFGezyzuHJPhdP3jHGW7v7Hwes4Hihqv5W2yyCmRY9VZJCRchETvxrMC8uECeJZdxQ14V4iD4DecoArkUSDwj8ogYE9WEv4MNZr12thNHCs/<0;1>/*),and_v(v:multi(2,[573fb35b/48'/1'/2'/2']tpubDDwxQauiaU964vPzt5Vd7jnDHEUtp2Vc34PaWpEXg5TQ3bRccxnc1MKKh88Hi7xiMeZo9Tm6fBcq4UGXqnDtGUniJLjqAD8SjQ8Eci3aSR7/<0;1>/*,[573fb35c/48'/1'/3'/2']tpubDE37XAVB5CQ1x85md3BQ5uHCoMwT5fgT8X13zzCUQ3x5o2jskYxKjj7Qcxt1Jpj4QB8tqspn2dooPCekRuQDYrDHov7J1ueUNu2wcvgRDxr/<0;1>/*),older(1000))))#fccaqlhh").unwrap();
    }

    #[test]
    fn descriptor_unspendable_internal_key() {
        // We correctly detect a deterministically derived unspendable internal key.
        LianaDescriptor::from_str("tr(tpubD6NzVbkrYhZ4YdBUPkUhDYj6Sd1QK8vgiCf5RwHnAnSNK5ozemAZzPTYZbgQq4diod7oxFJJYGa8FNRHzRo7URkixzQTuudh38xRRdSc4Hu/<0;1>/*,{and_v(v:multi_a(1,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<2;3>/*),older(2)),multi_a(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<0;1>/*)})").unwrap();
        // Even if it has an origin.
        LianaDescriptor::from_str("tr([00000000/1/2/3]tpubD6NzVbkrYhZ4YdBUPkUhDYj6Sd1QK8vgiCf5RwHnAnSNK5ozemAZzPTYZbgQq4diod7oxFJJYGa8FNRHzRo7URkixzQTuudh38xRRdSc4Hu/<0;1>/*,{and_v(v:multi_a(1,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<2;3>/*),older(2)),multi_a(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<0;1>/*)})").unwrap();
        // We'll correctly detect a non-deterministically derived unspendable internal key and
        // refuse to parse the descriptor (because it makes it have 2 primary spending paths).
        LianaDescriptor::from_str("tr(tpubDCaEmvN8YCgyfjNfX6j7r71h1Gx5pqVDAjT145hd46R4DhN8cuHUC39bqRXd43xnroUNKTUqFi9RGCLtxAxxwB6ysVhAh5k26q7AkNUxF7b/<0;1>/*,{and_v(v:multi_a(1,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<2;3>/*),older(2)),multi_a(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<0;1>/*)})").unwrap_err();
    }

    #[test]
    fn inheritance_descriptor_derivation() {
        let secp = secp256k1::Secp256k1::verification_only();
        let desc = LianaDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#2qj59a9y").unwrap();
        let der_desc = desc.receive_descriptor().derive(11.into(), &secp);
        assert_eq!(
            "bc1q26gtczlz03u6juf5cxppapk4sr4fyz53s3g4zs2cgactcahqv6yqc2t8e6",
            der_desc.address(bitcoin::Network::Bitcoin).to_string()
        );

        // Sanity check we can call the methods on the derived desc
        der_desc.script_pubkey();
        let mut psbt_in = PsbtIn::default();
        der_desc.update_psbt_in(&mut psbt_in);
        assert!(psbt_in.witness_script.is_some());
        assert!(!psbt_in.bip32_derivation.is_empty());
    }

    #[test]
    fn inheritance_descriptor_tl_value() {
        // Must always contain at least one timelocked path.
        LianaDescriptor::from_str("wsh(or_i(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap_err();

        let desc = LianaDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(1),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.first_timelock_value(), 1);

        let desc = LianaDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(42000),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.first_timelock_value(), 42000);

        let desc = LianaDescriptor::from_str("wsh(andor(pk([abcdef01]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(65535),pk([abcdef01]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))").unwrap();
        assert_eq!(desc.first_timelock_value(), 0xffff);
    }

    #[test]
    fn inheritance_descriptor_sat_size() {
        let desc = LianaDescriptor::from_str("wsh(or_d(pk([92162c45]tpubD6NzVbkrYhZ4WzTf9SsD6h7AH7oQEippXK2KP8qvhMMqFoNeN5YFVi7vRyeRSDGtgd2bPyMxUNmHui8t5yCgszxPPxMafu1VVzDpg9aruYW/<0;1>/*),and_v(v:pkh([abcdef01]tpubD6NzVbkrYhZ4Wdgu2yfdmrce5g4fiH1ZLmKhewsnNKupbi4sxjH1ZVAorkBLWSkhsjhg8kiq8C4BrBjMy3SjAKDyDdbuvUa1ToAHbiR98js/<0;1>/*),older(2))))#ravw7jw5").unwrap();
        // See the stack details below.
        assert_eq!(desc.max_sat_vbytes(true), (1 + 66 + 73 + 3) / 4);
        assert_eq!(desc.max_sat_vbytes(false), (1 + 66 + 1 + 34 + 73 + 3) / 4);

        // Maximum input size is (txid + vout + scriptsig + nSequence + max_sat).
        // Where max_sat is:
        // - Push the witness stack size
        // - Push the script
        // If recovery:
        // - Push an empty vector for using the recovery path
        // - Push the recovery key
        // EndIf
        // - Push a signature for the primary/recovery key
        // NOTE: The specific value is asserted because this was tested against a regtest
        // transaction.
        let stack = [vec![0; 65], vec![0; 72]];
        let witness_size = bitcoin::VarInt(stack.len() as u64).size()
            + stack
                .iter()
                .map(|item| bitcoin::VarInt(item.len() as u64).size() + item.len())
                .sum::<usize>();
        assert_eq!(
            desc.spender_input_size(true),
            32 + 4 + 1 + 4 + wu_to_vb(witness_size),
        );
        let stack = [vec![0; 65], vec![0; 0], vec![0; 33], vec![0; 72]];
        let witness_size = bitcoin::VarInt(stack.len() as u64).size()
            + stack
                .iter()
                .map(|item| bitcoin::VarInt(item.len() as u64).size() + item.len())
                .sum::<usize>();
        assert_eq!(
            desc.spender_input_size(false),
            32 + 4 + 1 + 4 + wu_to_vb(witness_size),
        );

        // Now perform the sanity checks under Taproot.
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap());
        let timelock = 52560;
        let desc = LianaDescriptor::new(
            LianaPolicy::new(
                owner_key.clone(),
                [(timelock, heir_key.clone())].iter().cloned().collect(),
            )
            .unwrap(),
        );

        // If using the primary path, it's a keypath spend.
        assert_eq!(desc.max_sat_vbytes(true), (1 + 65 + 3) / 4);
        // If using the recovery path, it's a script path spend. The script is 40 bytes long. The
        // control block is just the internal key and parity, so 33 bytes long.
        assert_eq!(
            desc.max_sat_vbytes(false),
            (1 + 65 + 1 + 40 + 1 + 33 + 3) / 4
        );

        // The same against the spender_input_size() helper, adding the size of the txin and
        // checking against a dummy witness stack.
        fn wit_size(stack: &[Vec<u8>]) -> usize {
            varint_len(stack.len())
                + stack
                    .iter()
                    .map(|item| varint_len(item.len()) + item.len())
                    .sum::<usize>()
        }
        let txin_boilerplate = 32 + 4 + 1 + 4;
        let stack = vec![vec![0; 64]];
        assert_eq!(
            desc.spender_input_size(true),
            txin_boilerplate + wu_to_vb(wit_size(&stack)),
        );
        let stack = vec![vec![0; 33], vec![0; 40], vec![0; 64]];
        assert_eq!(
            desc.spender_input_size(false),
            txin_boilerplate + wu_to_vb(wit_size(&stack)),
        );
    }

    #[test]
    fn taproot_multisig_descriptor_sat_weight() {
        // See https://mempool.space/signet/tx/84f09bddfe0f036d0390edf655636ad6092c3ab8f09b2bb1503caa393463f241
        // for an example spend from this descriptor.
        let desc = LianaDescriptor::from_str("tr(tpubD6NzVbkrYhZ4WUdbVsXDYBCXS8EPSYG1cAN9g4uP6uLQHMHXRvHSFkQBXy7MBeAvV8PDVJJ4o3AwYMKJHp45ci2g69UCAKteVSAJ61CnGEV/<0;1>/*,{and_v(v:pk([9e1c1983/48'/1'/0'/2']tpubDEWCLCMncbStq4BLXkQUAPqzzrh2tQUgYeQPt4NrB5D7gRraMyGbRqzPTmQGvqfdaFsXDVGSQBRgfXuNjDyfU626pxSjpQZszFNY6CzogxK/<2;3>/*),older(65535)),multi_a(2,[9e1c1983/48'/1'/0'/2']tpubDEWCLCMncbStq4BLXkQUAPqzzrh2tQUgYeQPt4NrB5D7gRraMyGbRqzPTmQGvqfdaFsXDVGSQBRgfXuNjDyfU626pxSjpQZszFNY6CzogxK/<0;1>/*,[3b1913e1/48'/1'/0'/2']tpubDFeZ2ezf4VUuTnjdhxJ1DKhLa2t6vzXZNz8NnEgeT2PN4pPqTCTeWUcaxKHPJcf1C8WzkLA71zSjDwuo4zqu4kkiL91ZUmJydC8f1gx89wM/<0;1>/*)})#ee0r4tw5").unwrap();
        // varint_len(num witness elements) = 1
        // varint_len(signature) + signature = 1 + 64
        // varint_len(script) + script = 1 + 70
        // varint_len(control block) + control block = 1 + 65
        assert_eq!(
            desc.max_sat_weight(true),
            1 + (1 + 64) + (1 + 64) + (1 + 70) + (1 + 65)
        );

        // See https://mempool.space/signet/tx/63095cf6b5a57e5f3a7f0af0e22c8234cc4a4c1531c3236b00bd2a009f70e801
        // for an example of a recovery transaction from the following descriptor:
        // tr(tpubD6NzVbkrYhZ4XcC4HC7TDGrhraymFg9xo31hVtc5sh3dtsXbB5ZXewwMXi6HSmR2PyLeG8VwD3anqavSJVtXYJAAJcaEGCZdkBnnWTmhz3X/<0;1>/*,{and_v(v:pk([9e1c1983/48'/1'/0'/2']tpubDEWCLCMncbStq4BLXkQUAPqzzrh2tQUgYeQPt4NrB5D7gRraMyGbRqzPTmQGvqfdaFsXDVGSQBRgfXuNjDyfU626pxSjpQZszFNY6CzogxK/<2;3>/*),older(1)),multi_a(2,[88d8b4b9/48'/1'/0'/2']tpubDENzCJsHPDzX1EAP9eUPumw2hFUyjuUtBK8CWNPkudZTQ1mchX1hiAwog3fd6BKbq1rdZbLW3Q1d79AcvQCCMdehuSZ8GcShDcHaYTosCRa/<0;1>/*,[9e1c1983/48'/1'/0'/2']tpubDEWCLCMncbStq4BLXkQUAPqzzrh2tQUgYeQPt4NrB5D7gRraMyGbRqzPTmQGvqfdaFsXDVGSQBRgfXuNjDyfU626pxSjpQZszFNY6CzogxK/<0;1>/*)})#pepfj0gd
        // Recovery path would use 1 + (1+64) + (1+36) + (1+65), but `max_sat_weight` considers all
        // spending paths when passing `false`. So it currently gives the same as passing `true`.
        // This `true` branch assumes a Schnorr signature of size 1+64+1, where the final +1 is for the sighash suffix:
        // https://docs.rs/miniscript/11.0.0/src/miniscript/descriptor/tr.rs.html#254-301
        // So we need to add 2, 1 for each signature.
        assert_eq!(desc.max_sat_weight(false), desc.max_sat_weight(true) + 2);
    }

    #[test]
    fn liana_desc_keys() {
        let secp = secp256k1::Secp256k1::signing_only();
        let prim_path = PathInfo::Single(random_desc_key(&secp));
        let twenty_eight_keys: Vec<descriptor::DescriptorPublicKey> =
            (0..28).map(|_| random_desc_key(&secp)).collect();
        let mut twenty_nine_keys = twenty_eight_keys.clone();
        twenty_nine_keys.push(random_desc_key(&secp));

        // Test various scenarii which should pass or fail on both Taproot and P2WSH.
        macro_rules! test_liana_desc_keys {
            ($constructor:expr) => {
                $constructor(
                    prim_path.clone(),
                    [(1, PathInfo::Multi(2, vec![random_desc_key(&secp)]))]
                        .iter()
                        .cloned()
                        .collect(),
                )
                .unwrap_err();
                $constructor(
                    prim_path.clone(),
                    [(
                        1,
                        PathInfo::Multi(1, vec![random_desc_key(&secp), random_desc_key(&secp)]),
                    )]
                    .iter()
                    .cloned()
                    .collect(),
                )
                .unwrap();
                $constructor(
                    prim_path.clone(),
                    [(
                        1,
                        PathInfo::Multi(0, vec![random_desc_key(&secp), random_desc_key(&secp)]),
                    )]
                    .iter()
                    .cloned()
                    .collect(),
                )
                .unwrap_err();
                $constructor(
                    prim_path.clone(),
                    [(
                        1,
                        PathInfo::Multi(2, vec![random_desc_key(&secp), random_desc_key(&secp)]),
                    )]
                    .iter()
                    .cloned()
                    .collect(),
                )
                .unwrap();
                $constructor(
                    prim_path.clone(),
                    [(
                        1,
                        PathInfo::Multi(3, vec![random_desc_key(&secp), random_desc_key(&secp)]),
                    )]
                    .iter()
                    .cloned()
                    .collect(),
                )
                .unwrap_err();
                $constructor(
                    prim_path.clone(),
                    [(1, PathInfo::Multi(3, twenty_eight_keys.clone()))]
                        .iter()
                        .cloned()
                        .collect(),
                )
                .unwrap();
                $constructor(
                    prim_path.clone(),
                    [(1, PathInfo::Multi(20, twenty_eight_keys.clone()))]
                        .iter()
                        .cloned()
                        .collect(),
                )
                .unwrap();
            };
        }
        test_liana_desc_keys!(LianaPolicy::new_legacy);
        test_liana_desc_keys!(LianaPolicy::new);

        // A 20-of-28 should pass on Taproot but fail on P2WSH.
        LianaPolicy::new(
            prim_path.clone(),
            [(1, PathInfo::Multi(20, twenty_nine_keys.clone()))]
                .iter()
                .cloned()
                .collect(),
        )
        .unwrap();
        LianaPolicy::new_legacy(
            prim_path,
            [(1, PathInfo::Multi(20, twenty_nine_keys))]
                .iter()
                .cloned()
                .collect(),
        )
        .unwrap_err();
    }

    fn roundtrip(desc_str: &str) {
        let desc = LianaDescriptor::from_str(desc_str).unwrap();
        assert_eq!(desc.to_string(), desc_str);
    }

    // Make sure the string representation of our descriptors roundtrip. The Taproot ones were
    // generated manually with our code because of the potential need to compute the internal key
    // deterministically.
    #[test]
    fn roundtrip_descriptor() {
        // A descriptor with single keys in both primary and recovery paths
        roundtrip("wsh(or_d(pk([aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:pkh([aabbccdd]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560))))#7437yjrs");
        roundtrip("tr([8344c025]xpub661MyMwAqRbcG2SYC6YSRsUGvcSxXEZm1kjiQRTEaAqart1PQk1N1hVTTEsGfaBx6xQ5gDYXXtbourodE6ZE5qZTnaMgmehNs8GGEEY9YK6/<0;1>/*,and_v(v:pk([158fd0ef]xpub661MyMwAqRbcF2KsCnvJ4mqWXXrwd3799wCyQrLk2iNDC6CfK8UcfnABdeTpXyoJnBhRTybmtBLDAuTuHye1eQMq43BSLtR2miA6t9KqmWU/<0;1>/*),older(4242)))#zy3kddhj");
        // One with a multisig in both paths
        roundtrip("wsh(or_d(multi(3,[aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0011/10/4893]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*,[aabb0022]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*),and_v(v:multi(2,[aabbccdd]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*,[aabb0011]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*,[aabb0022/10/4893]xpub6AyxexvxizZJffF153evmfqHcE9MV88fCNCAtP3jQjXJHwrAKri71Tq9jWUkPxj9pja4u6AkCPHY7atgxzSEa2HtDwJfrRWKK4fsfQg4o77/<0;1>/*),older(26352))))#csjdk94l");
        roundtrip("tr(xpub661MyMwAqRbcGg7oXkMMptXXJAGxQtVM7LZeqXNNdxPiWyEmuJdoyFD3NhRpL1YTo313XRWZmiUXTkcEK9EFQHrbie6NBNAvL2CZXp941Li/<0;1>/*,{and_v(v:multi_a(2,[6b882e01]xpub661MyMwAqRbcGRU9psMcDAPd2L2ShzwoenySSSjWpkd7u8Wv7PCPtH5fi6WYYbQAqSG4U3NbuYASCRMkWVYm7yb97iTY4MUKKZ3N8XwCERJ/<0;1>/*,[66b98303]xpub661MyMwAqRbcGicAwMZ5pHCWrB4DEMBGUtvkV2KMMLypR8dbr7g2uV9vzE9w3oDKRqtTV6HYTqHHvusxNwJUXAvRH6BFhKUPTgGMiLPnSmK/<0;1>/*,[9c04a03b]xpub661MyMwAqRbcFCBt8Gjs71UqoMe8V4PSHECuCowg1TR7EkGWLLbu2WanQtcWutzwahrTcicsuL25Q7r6EyfbEKF2jSoekmnw9soZfoiLZXu/<0;1>/*),older(42421)),multi_a(3,[30188cc2]xpub661MyMwAqRbcGeoYQgqUapNKDLBiNE7fcGs6ibKi39GjuiRmV1JgXcfAwHjt7PLLofmz4PPL66NTwAxaTwGtL8YB67RhRspAzbKgneqpenb/<0;1>/*,[aea08adc]xpub661MyMwAqRbcGVL3W5qKT8pjZ3BXcDEJghDj67rKLQYwmTaJLud8RWyYwZQ9LdzkcNtCSCHVypZdUUxd4z2k5hCfb6qprGgwAKqpmaKJTnS/<0;1>/*,[85e33ca4]xpub661MyMwAqRbcFHP9bmnRofzha8c4DHADC7ToPz3kYdov5DDDtgdBEQ3kVcwdjjqAGC8eJZ65CLF2cA9XHhUsJJqKxbE9asj8RUNmGjCJErX/<0;1>/*)})#zm4kj6yd");
        // A single key as primary path, a multisig as recovery
        roundtrip("wsh(or_d(pk([aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:multi(2,[aabbccdd]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*,[aabb0011]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*,[aabb0022/10/4893]xpub6AyxexvxizZJffF153evmfqHcE9MV88fCNCAtP3jQjXJHwrAKri71Tq9jWUkPxj9pja4u6AkCPHY7atgxzSEa2HtDwJfrRWKK4fsfQg4o77/<0;1>/*),older(26352))))#sc9gw0z0");
        roundtrip("tr([46f4cf22]xpub661MyMwAqRbcG22nAyuZc7MUXR559qKKkHVdi7TKy3Q8m91FKN9heKhP7jWj6SJdyAA9zfQgjQUNWkqjPGJdcH6uFD8mEWJnTps5emjoi9L/<0;1>/*,and_v(v:multi_a(2,[f9eb379c]xpub661MyMwAqRbcF2FpaYbnrN7K6uPhiwg5u1LiqmsMSTnphuhQzpPv9RGdERxDd7pnnrEC8hxttAPi4wbSVsKeJYiHYymfpuxSD7TALTXqjq6/<0;1>/*,[1fc462f2]xpub661MyMwAqRbcEtYavp2XsS9QfH93wyVQnkWenWxWuWdaxDtjBqfzFfWPY83z3da5oYv2XmwgTT97GhGwX9HUGDEP4FERzzgmwaGNAz1emZr/<0;1>/*,[ebcef2a0]xpub661MyMwAqRbcH3JihNDbpEqpmT9xjY5YWd9VwVWQqoWWrggurHs7wsvTXM7ggK5X3wwATxiijwJPe73y9beirtorQebMuL4hR7dbU7akrk7/<0;1>/*),older(124)))#rgrm8l4v");
        // The other way around
        roundtrip("wsh(or_d(multi(3,[aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0011/10/4893]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*,[aabb0022]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*),and_v(v:pk([aabbccdd]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*),older(26352))))#kjajav3j");
        roundtrip("tr(xpub661MyMwAqRbcFWPZkATtyZ3cZboVifGEpVoDNLRotSvymYNbb652s75MJs7x6Dsh1K4WidtHAvWiyWu6ufbvP3RG9ozXYhZA83rAniyTAcQ/<0;1>/*,{and_v(v:pk([5c1f5207]xpub661MyMwAqRbcGjhDHfE45ivpxoBGywTdSJa5vWgB5L5BjUjfdvfwQr618o6hjLSCdLruGwu8WFLbgQa7179EC3HEiEUAceLSHArRgsUPRFe/<0;1>/*),older(42)),multi_a(2,[f25498f5]xpub661MyMwAqRbcEZKeQW5gkVY61KyFdmR2ntRmHdnoyWPE5PrgcpAFhotgSftuQVkw1DXoeE7wGQpXkaijczBFNVSFYs4UN2ZsN8tiBR2cffw/<0;1>/*,[0adab7f3]xpub661MyMwAqRbcGrUYLnLEAqJEjmy9NejNXvjneoochu285TgBszNaS5usEnosxKXQxPS5ppWp923EXmi4JgYJWZa4cwEzTX6n6aacG1bN99j/<0;1>/*,[78428935]xpub661MyMwAqRbcGqCdcSemPFvTbhm5swDzRYxP9azdQGPwgcAYumXhFuQUdNFLxaf61uRB7UdpxsYoP3hMW3sqQF7ErWQm5RznBbbVMm6z6wc/<0;1>/*)})#s235c44f");
        // More than 2 spending paths.
        roundtrip("wsh(or_i(and_v(v:thresh(1,pkh([19e064b0]xpub661MyMwAqRbcGRgGoZDVccAfLzLuvxkXevrGCq66XGV9mmRLfJ1aiAZNtVUTfxFMoSmPNJLmEywZn8yQXBzVHVENMRbj3VpVvdzCteCkgq6/<0;1>/*),a:pkh([00454cc2]xpub661MyMwAqRbcExaDFtcC1pVGounzi9bmVb4nBxVr3reFEsFpCTn5VVwuDiUFeJkJtppEC7Gzk3cW8htEB9Q3DcXV28SAHioi2oJZv6oTobF/<0;1>/*)),older(1678)),or_i(and_v(v:thresh(2,pkh([0143d6e6]xpub661MyMwAqRbcFuLKDCSKk2r3KKN7FWXZnx9s7V1xScv7N1qs7xXPxjrarBaPkuzV9ji7Hquwf4G6G12pXFmaeqwXWhKzwSp1j8JgLHTKUDn/<0;1>/*),a:pkh([cd1f0cf2]xpub661MyMwAqRbcGcFwqwdNkHojx6ffeQEXPfopamNscuf4CXaLrKVMkCTpffiFNJ3okep7bgNVx13N1rryW3nQNiPAsrkr5zL9T3vo2ww4fC8/<0;1>/*),a:pkh([d76af68c]xpub661MyMwAqRbcGVFCMA5yqLiF9xj6G9QoqFDQdqmnyCZDTFTfpfUgzevrohjSrMTjoBYyB5YvKjtEqqX9U6yjgDCYRT8e7DeYqLnu6DbFAgj/<0;1>/*)),older(43)),or_d(multi(2,[5b016400]xpub661MyMwAqRbcGZFpjHB8mvxGnDGDdEBetsFu25nC69SrGAJKJVctsFwNNY5VwPMVx7aXL6m1LKAeA5qAE3Wheh5cAKBdxqSFrRBd3Vf7eTX/<0;1>/*,[6b0a6b3f]xpub661MyMwAqRbcGzZBMBU1evaZmfwmEVkzU8oRhu4y7DSaaHHoQeHDbM47JNqeEbJRGGhMNd1Hp9oP2NdYbRHxxcd7YfABiNfULyVW9vDg5cx/<0;1>/*,[1c4eb5d3]xpub661MyMwAqRbcGFr2mWaBr1rX3xfnv75FbzP1hPW7LEzYMzDZV5wPVgcrYEZWxwu8ALUTRJ5ioutE3mz5dTQBWKEkvxCytV3QeNdm4cDHr6p/<0;1>/*),and_v(v:pkh([5c055660]xpub661MyMwAqRbcEqgeH5cqyxRwY4UG21ey1MBJkNBX2xSTmGS9dCRmGQezqHE9mXUXzs9HqFzNEN2KkNw5o8xpqAXw2XxsVhGVm1LbRaEnxyT/<0;1>/*),older(42))))))#hd246u4a");
        roundtrip("tr(xpub661MyMwAqRbcGqmqNapgQ9kqrLcDeZLHPktzsBcZXTtNx7aEay8NKQPizKcpu2fUejNbZzhZQaZLeDWL3nt8zg9QbFLBUTRQu4qqcSzeEmF/<0;1>/*,{{and_v(v:multi_a(1,[b4e32970]xpub661MyMwAqRbcEbs6ohRoUqTckEfLeT3vB2EsuWuckrEuDSKqdFXV6so8xJb4kvA4ZxT6hCydyFKsKwJrDm2LgSfTCphVqZgQbLzF49KwaXc/<0;1>/*,[c318e87f]xpub661MyMwAqRbcG2qnrFJ2MhKFSHehbVkK38gFfG7zXwasN51dKrL4kffj1HRd2zFhAZeQsjYKS8YaiN4sC4gVPHR28qXdQf7pf7nbYoefg6T/<0;1>/*),older(1678)),{and_v(v:pk([6c0d38a3]xpub661MyMwAqRbcF87hAvenL8GHW7qxhtn8Y9zHVkQbuTsd6RVtWkhBY5gh6m4Rua9ENmYDx7jTb8kbiyVB9iaLAbyRudxPFVTFoGPp6rTqoZn/<0;1>/*),older(42)),and_v(v:multi_a(2,[2e1370a6]xpub661MyMwAqRbcGRzCgSNLW7VFUFdwvC1dFXmKgWbZwQERj2QfNQuy5diCQSHNXuQYSS9FwXykLeWKtnZ5yRJ4ZHZzYqWf13FUY4PbDpBhipr/<0;1>/*,[fae2633e]xpub661MyMwAqRbcG9qKwZ7F363Mx3Ai3H2aMXAWTjvYCZrH4wqDEDLnsVghWFrwTKwpDGGzsSDCL7vPTiaiY7DhhdV2bY6RdPNGd7bF9om1MFz/<0;1>/*,[2ae87e33]xpub661MyMwAqRbcGw8ZvGfdLEjhCk4YC9hZrrUceKipiH32ANDMQccYFqq91kH8RpcwGiPnCbUWFo1S6ZGY2GxbVdJFsMYXqzpL1byJ1D3G2Mh/<0;1>/*),older(43))}},multi_a(2,[40f48611]xpub661MyMwAqRbcGUkDb45NBcMYwaaSE3fhsMNwvdf2psYhrqhFmRJY9n8irJuEB3juhK5LQPBiiqdr2gixMmC7Nmtg3Mwu4C5wbeagaAzbb9W/<0;1>/*,[a2bdfbe5]xpub661MyMwAqRbcH228eUBaJvc7Va1y7cGyEH9DZ5vPneKgZDX8eMsSd8PHS3uRYCFySyHPy3VfGfS8vKb5FzcS2MbNorNVv2c3Hn7AvVJJZ73/<0;1>/*,[028ece7a]xpub661MyMwAqRbcG9W1pZzs7rvWVtHeW1anzABj8iQRBbnz8yLf7vgUmYkVsydLf1hLffibgfzUjTBcrNCDKaBNnuqLtsp1xyiLSZJyLDtEjkF/<0;1>/*)})#xgzxdvrv");
    }

    fn psbt_from_str(psbt_str: &str) -> Psbt {
        Psbt::from_str(psbt_str).unwrap()
    }

    #[test]
    fn partial_spend_info_p2wsh() {
        let secp = secp256k1::Secp256k1::signing_only();

        // A simple descriptor with 1 keys as primary path and 1 recovery key.
        let desc = LianaDescriptor::from_str("wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr").unwrap();
        let desc_info = desc.policy();
        let prim_key_fg = bip32::Fingerprint::from_str("f5acc2fd").unwrap();
        let recov_key_origin: (_, bip32::DerivationPath) = (
            bip32::Fingerprint::from_str("8a64f2a9").unwrap(),
            Vec::new().into(),
        );

        // A PSBT with a single input and output, no signature. nSequence is not set to use the
        // recovery path.
        let mut unsigned_single_psbt: Psbt = psbt_from_str("cHNidP8BAHECAAAAAUSHuliRtuCX1S6JxRuDRqDCKkWfKmWL5sV9ukZ/wzvfAAAAAAD9////AogTAAAAAAAAFgAUIxe7UY6LJ6y5mFBoWTOoVispDmdwFwAAAAAAABYAFKqO83TK+t/KdpAt21z2HGC7/Z2FAAAAAAABASsQJwAAAAAAACIAIIIySQjGCTeyx/rKUQx8qobjhJeNCiVCliBJPdyRX6XKAQVBIQI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDKxzZHapFNYASzIYkEdH9bJz6nnqUG3uBB8kiK1asmgiBgI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDAz1rML9AAAAAG8AAAAiBgMLcbOxsfLe6+3r1UcjQo77HY0As8OKE4l37yj0/qhIyQyKZPKpAAAAAG8AAAAAAAA=");
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 0);
        assert!(info.primary_path.signed_pubkeys.is_empty());
        assert!(info.recovery_paths.is_empty());

        // If we set the sequence too low we still won't have the recovery path info.
        unsigned_single_psbt.unsigned_tx.input[0].sequence =
            Sequence::from_height(desc_info.recovery_paths.keys().next().unwrap() - 1);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert!(info.recovery_paths.is_empty());

        // Now if we set the sequence at the right value we'll have it.
        let timelock = *desc_info.recovery_paths.keys().next().unwrap();
        unsigned_single_psbt.unsigned_tx.input[0].sequence = Sequence::from_height(timelock);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert!(info.recovery_paths.contains_key(&timelock));

        // Even if it's a bit too high (as long as it's still a block height and activated)
        unsigned_single_psbt.unsigned_tx.input[0].sequence = Sequence::from_height(timelock + 42);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        let recov_info = info.recovery_paths.get(&timelock).unwrap();
        assert_eq!(recov_info.threshold, 1);
        assert_eq!(recov_info.sigs_count, 0);
        assert!(recov_info.signed_pubkeys.is_empty());

        // The same PSBT but with an (invalid) signature for the primary key.
        let mut signed_single_psbt = psbt_from_str("cHNidP8BAHECAAAAAUSHuliRtuCX1S6JxRuDRqDCKkWfKmWL5sV9ukZ/wzvfAAAAAAD9////AogTAAAAAAAAFgAUIxe7UY6LJ6y5mFBoWTOoVispDmdwFwAAAAAAABYAFKqO83TK+t/KdpAt21z2HGC7/Z2FAAAAAAABASsQJwAAAAAAACIAIIIySQjGCTeyx/rKUQx8qobjhJeNCiVCliBJPdyRX6XKIgICNnKlqXPVAFtoGbdlpCo74vCqDArotNKUZS+sBt9rigxIMEUCIQCYZusUL8bdi2PnjWao4bIDDgMQ9Dj2Lcup3/VmkGbYJAIgX/wF5HsqugC5JzvU2cGOmUWtHr2Pg0N4912qogYgDH4BAQVBIQI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDKxzZHapFNYASzIYkEdH9bJz6nnqUG3uBB8kiK1asmgiBgI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDAz1rML9AAAAAG8AAAAiBgMLcbOxsfLe6+3r1UcjQo77HY0As8OKE4l37yj0/qhIyQyKZPKpAAAAAG8AAAAAAAA=");
        let info = desc.partial_spend_info(&signed_single_psbt).unwrap();
        assert_eq!(signed_single_psbt.inputs[0].partial_sigs.len(), 1);
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        assert!(info.recovery_paths.is_empty());

        // Now enable the recovery path and add a signature for the recovery key.
        signed_single_psbt.unsigned_tx.input[0].sequence = Sequence::from_height(timelock);
        let recov_pubkey = bitcoin::PublicKey {
            compressed: true,
            inner: *signed_single_psbt.inputs[0]
                .bip32_derivation
                .iter()
                .find(|(_, (fg, der_path))| {
                    fg == &recov_key_origin.0
                        && der_path[..der_path.len() - 2] == recov_key_origin.1[..]
                })
                .unwrap()
                .0,
        };
        let prim_key = *signed_single_psbt.inputs[0]
            .partial_sigs
            .iter()
            .next()
            .unwrap()
            .0;
        let sig = signed_single_psbt.inputs[0]
            .partial_sigs
            .remove(&prim_key)
            .unwrap();
        signed_single_psbt.inputs[0]
            .partial_sigs
            .insert(recov_pubkey, sig);
        let info = desc.partial_spend_info(&signed_single_psbt).unwrap();
        assert_eq!(signed_single_psbt.inputs[0].partial_sigs.len(), 1);
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 0);
        assert!(info.primary_path.signed_pubkeys.is_empty());
        let recov_info = info.recovery_paths.get(&timelock).unwrap();
        assert_eq!(recov_info.threshold, 1);
        assert_eq!(recov_info.sigs_count, 1);
        assert!(
            recov_info.signed_pubkeys.len() == 1
                && recov_info.signed_pubkeys.contains_key(&recov_key_origin.0)
        );

        // A PSBT with multiple inputs, all signed for the primary path.
        let psbt: Psbt = psbt_from_str("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");
        let info = desc.partial_spend_info(&psbt).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        assert!(info.recovery_paths.is_empty());

        // Enable the recovery path, it should show no recovery sig.
        let mut rec_psbt = psbt.clone();
        for txin in rec_psbt.unsigned_tx.input.iter_mut() {
            txin.sequence = Sequence::from_height(timelock);
        }
        let info = desc.partial_spend_info(&rec_psbt).unwrap();
        assert!(rec_psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        let recov_info = info.recovery_paths.get(&timelock).unwrap();
        assert_eq!(recov_info.threshold, 1);
        assert_eq!(recov_info.sigs_count, 0);
        assert!(recov_info.signed_pubkeys.is_empty());

        // If the sequence of one of the input is different from the other ones, it'll return
        // an error since the analysis is on the whole transaction.
        let mut inconsistent_psbt = psbt.clone();
        inconsistent_psbt.unsigned_tx.input[0].sequence = Sequence::from_height(timelock + 1);
        assert!(desc
            .partial_spend_info(&inconsistent_psbt)
            .unwrap_err()
            .to_string()
            .contains("Analyzed PSBT is inconsistent across inputs."));

        // Same if all inputs don't have the same number of signatures.
        let mut inconsistent_psbt = psbt.clone();
        inconsistent_psbt.inputs[0].partial_sigs.clear();
        assert!(desc
            .partial_spend_info(&inconsistent_psbt)
            .unwrap_err()
            .to_string()
            .contains("Analyzed PSBT is inconsistent across inputs."));

        // If we analyze a descriptor with a multisig we'll get the right threshold.
        let desc = LianaDescriptor::from_str("wsh(or_d(multi(2,[f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*,[00112233]xpub6FC8vmQGGfSuQGfKG5L73fZ7WjXit8TzfJYDKwTtHkhrbAhU5Kma41oenVq6aMnpgULJRXpQuxnVysyfdpRhVgD6vYe7XLbFDhmvYmDrAVq/<0;1>/*,[aabbccdd]xpub68XtbpvDM19d39wEKdvadHkZ4FGKf4tnryKzAacttp8BLX3uHj7eK8shRnFBhZ2UL83S9dwXe42Qm6eG6BkR1jy8XwUSNBcHKtET7j4V5FB/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#2kgxuax5").unwrap();
        let info = desc.partial_spend_info(&psbt).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        assert_eq!(info.primary_path.threshold, 2);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        assert!(info.recovery_paths.is_empty());

        let desc = LianaDescriptor::from_str("wsh(or_d(multi(2,[636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/<0;1>/*,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*),and_v(v:multi(2,[636adf3f/48'/1'/1'/2']tpubDDvF2khuoBBj8vcSjQfa7iKaxsQZE7YjJ7cJL8A8eaneadMPKbHSpoSr4JD1F5LUvWD82HCxdtSppGfrMUmiNbFxrA2EHEVLnrdCFNFe75D/<0;1>/*,[ffd63c8d/48'/1'/1'/2']tpubDFMs44FD4kFt3M7Z317cFh5tdKEGN8tyQRY6Q5gcSha4NtxZfGmTVRMbsD1bWN469LstXU4aVSARDxrvxFCUjHeegfEY2cLSazMBkNCmDPD/<0;1>/*),older(2))))#xcf6jr2r").unwrap();
        let info = desc.policy();
        assert_eq!(info.primary_path, PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*").unwrap(),
            ],
        ));
        assert_eq!(info.recovery_paths, [(2, PathInfo::Multi(
            2,
            vec![
                descriptor::DescriptorPublicKey::from_str("[636adf3f/48'/1'/1'/2']tpubDDvF2khuoBBj8vcSjQfa7iKaxsQZE7YjJ7cJL8A8eaneadMPKbHSpoSr4JD1F5LUvWD82HCxdtSppGfrMUmiNbFxrA2EHEVLnrdCFNFe75D/<0;1>/*").unwrap(),
                descriptor::DescriptorPublicKey::from_str("[ffd63c8d/48'/1'/1'/2']tpubDFMs44FD4kFt3M7Z317cFh5tdKEGN8tyQRY6Q5gcSha4NtxZfGmTVRMbsD1bWN469LstXU4aVSARDxrvxFCUjHeegfEY2cLSazMBkNCmDPD/<0;1>/*").unwrap(),
            ],
        ))].iter().cloned().collect());
        let mut psbt = psbt_from_str("cHNidP8BAIkCAAAAAWi3OFgkj1CqCDT3Swm8kbxZS9lxz4L3i4W2v9KGC7nqAQAAAAD9////AkANAwAAAAAAIgAg27lNc1rog+dOq80ohRuds4Hgg/RcpxVun2XwgpuLSrFYMwwAAAAAACIAIDyWveqaElWmFGkTbFojg1zXWHODtiipSNjfgi2DqBy9AAAAAAABAOoCAAAAAAEBsRWl70USoAFFozxc86pC7Dovttdg4kvja//3WMEJskEBAAAAAP7///8CWKmCIk4GAAAWABRKBWYWkCNS46jgF0r69Ehdnq+7T0BCDwAAAAAAIgAgTt5fs+CiB+FRzNC8lHcgWLH205sNjz1pT59ghXlG5tQCRzBEAiBXK9MF8z3bX/VnY2aefgBBmiAHPL4tyDbUOe7+KpYA4AIgL5kU0DFG8szKd+szRzz/OTUWJ0tZqij41h2eU9rSe1IBIQNBB1hy+jKsg1TihMT0dXw7etpu9TkO3NuvhBDFJlBj1cP2AQABAStAQg8AAAAAACIAIE7eX7PgogfhUczQvJR3IFix9tObDY89aU+fYIV5RubUIgICSKJsNs0zFJN58yd2aYQ+C3vhMbi0x7k0FV3wBhR4THlIMEUCIQCPWWWOhs2lThxOq/G8X2fYBRvM9MXSm7qPH+dRVYQZEwIgfut2vx3RvwZWcgEj4ohQJD5lNJlwOkA4PAiN1fjx6dABIgID3mvj1zerZKohOVhKCiskYk+3qrCum6PIwDhQ16ePACpHMEQCICZNR+0/1hPkrDQwPFmg5VjUHkh6aK9cXUu3kPbM8hirAiAyE/5NUXKfmFKij30isuyysJbq8HrURjivd+S9vdRGKQEBBZNSIQJIomw2zTMUk3nzJ3ZphD4Le+ExuLTHuTQVXfAGFHhMeSEC9OfCXl+sJOrxUFLBuMV4ZUlJYjuzNGZSld5ioY14y8FSrnNkUSED3mvj1zerZKohOVhKCiskYk+3qrCum6PIwDhQ16ePACohA+ECH+HlR+8Sf3pumaXH3IwSsoqSLCH7H1THiBP93z3ZUq9SsmgiBgJIomw2zTMUk3nzJ3ZphD4Le+ExuLTHuTQVXfAGFHhMeRxjat8/MAAAgAEAAIAAAACAAgAAgAAAAAABAAAAIgYC9OfCXl+sJOrxUFLBuMV4ZUlJYjuzNGZSld5ioY14y8Ec/9Y8jTAAAIABAACAAAAAgAIAAIAAAAAAAQAAACIGA95r49c3q2SqITlYSgorJGJPt6qwrpujyMA4UNenjwAqHGNq3z8wAACAAQAAgAEAAIACAACAAAAAAAEAAAAiBgPhAh/h5UfvEn96bpmlx9yMErKKkiwh+x9Ux4gT/d892Rz/1jyNMAAAgAEAAIABAACAAgAAgAAAAAABAAAAACICAlBQ7gGocg7eF3sXrCio+zusAC9+xfoyIV95AeR69DWvHGNq3z8wAACAAQAAgAEAAIACAACAAAAAAAMAAAAiAgMvVy984eg8Kgvj058PBHetFayWbRGb7L0DMnS9KHSJzBxjat8/MAAAgAEAAIAAAACAAgAAgAAAAAADAAAAIgIDSRIG1dn6njdjsDXenHa2lUvQHWGPLKBVrSzbQOhiIxgc/9Y8jTAAAIABAACAAAAAgAIAAIAAAAAAAwAAACICA0/epE59sVEj7Et0I4R9qJQNuX23RNvDZKCRL7eUps9FHP/WPI0wAACAAQAAgAEAAIACAACAAAAAAAMAAAAAIgICgldCOK6iHscv//2NipgaMABLV5TICU/zlP7HlQmlg08cY2rfPzAAAIABAACAAQAAgAIAAIABAAAAAQAAACICApb0p9rfpJshB3J186PGWrvzQdixcwQZWmebOUMdkquZHP/WPI0wAACAAQAAgAAAAIACAACAAQAAAAEAAAAiAgLY5q+unoDxC/HI5BaNiPq12ei1REZIcUAN304JfKXUwxz/1jyNMAAAgAEAAIABAACAAgAAgAEAAAABAAAAIgIDg6cUVCJB79cMcofiURHojxFARWyS4YEhJNRixuOZZRgcY2rfPzAAAIABAACAAAAAgAIAAIABAAAAAQAAAAA=");
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 2);
        assert_eq!(partial_info.primary_path.sigs_count, 1);
        assert_eq!(partial_info.primary_path.signed_pubkeys.len(), 1);
        assert!(partial_info.recovery_paths.is_empty());

        // A not very well thought-out decaying multisig.
        let prim_path = PathInfo::Multi(3, (0..3).map(|_| random_desc_key(&secp)).collect());
        let first_reco_path = PathInfo::Multi(3, (0..5).map(|_| random_desc_key(&secp)).collect());
        let sec_reco_path = PathInfo::Multi(2, (0..5).map(|_| random_desc_key(&secp)).collect());
        let third_reco_path = PathInfo::Multi(1, (0..5).map(|_| random_desc_key(&secp)).collect());
        let liana_policy = LianaPolicy::new_legacy(
            prim_path.clone(),
            [
                (26784, first_reco_path.clone()),
                (53568, sec_reco_path.clone()),
                (62496, third_reco_path.clone()),
            ]
            .iter()
            .cloned()
            .collect(),
        )
        .unwrap();
        let desc = LianaDescriptor::new(liana_policy.clone());
        let policy = desc.policy();
        assert_eq!(policy, liana_policy);
        let empty_partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(empty_partial_info.primary_path.threshold, 3);
        assert_eq!(empty_partial_info.primary_path.sigs_count, 0);
        assert_eq!(
            empty_partial_info.primary_path.sigs_count,
            empty_partial_info.primary_path.signed_pubkeys.len()
        );
        assert!(empty_partial_info.recovery_paths.is_empty());

        // Now set a signature for the primary path. All recovery paths still empty, a signature is
        // present for the primary path.
        let dummy_pubkey = bitcoin::PublicKey::from_str(
            "0282574238aea21ec72ffffd8d8a981a30004b5794c8094ff394fec79509a5834f",
        )
        .unwrap();
        let dummy_sig = bitcoin::ecdsa::Signature::from_str("30440220264d47ed3fd613e4ac34303c59a0e558d41e487a68af5c5d4bb790f6ccf218ab02203213fe4d51729f9852a28f7d22b2ecb2b096eaf07ad44638af77e4bdbdd4462901").unwrap();
        let dummy_der_path = bip32::DerivationPath::from_str("m/0/1").unwrap();
        let fingerprint = prim_path.thresh_origins().1.into_iter().next().unwrap().0;
        psbt.inputs[0]
            .bip32_derivation
            .insert(dummy_pubkey.inner, (fingerprint, dummy_der_path));
        psbt.inputs[0].partial_sigs.insert(dummy_pubkey, dummy_sig);
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 3);
        assert_eq!(partial_info.primary_path.sigs_count, 1);
        assert_eq!(
            partial_info.primary_path.sigs_count,
            partial_info.primary_path.signed_pubkeys.len()
        );
        assert!(partial_info.recovery_paths.is_empty());

        // Now enable the first recovery path and make the signature be for this path.
        let fingerprint = first_reco_path
            .thresh_origins()
            .1
            .into_iter()
            .next()
            .unwrap()
            .0;
        psbt.inputs[0]
            .bip32_derivation
            .get_mut(&dummy_pubkey.inner)
            .unwrap()
            .0 = fingerprint;
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 3);
        assert_eq!(partial_info.primary_path.sigs_count, 0);
        assert_eq!(
            partial_info.primary_path.sigs_count,
            partial_info.primary_path.signed_pubkeys.len()
        );
        assert!(partial_info.recovery_paths.is_empty());
        psbt.unsigned_tx.input[0].sequence = bitcoin::Sequence::from_height(26784);
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.recovery_paths.len(), 1);
        assert_eq!(partial_info.recovery_paths[&26784].threshold, 3);
        assert_eq!(partial_info.recovery_paths[&26784].sigs_count, 1);
        assert_eq!(
            partial_info.recovery_paths[&26784].signed_pubkeys.len(),
            partial_info.recovery_paths[&26784].sigs_count
        );

        // Now enable the second recovery path and make the signature be for this path.
        let fingerprint = sec_reco_path
            .thresh_origins()
            .1
            .into_iter()
            .next()
            .unwrap()
            .0;
        psbt.inputs[0]
            .bip32_derivation
            .get_mut(&dummy_pubkey.inner)
            .unwrap()
            .0 = fingerprint;
        psbt.unsigned_tx.input[0].sequence = bitcoin::Sequence::from_height(53568);
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 3);
        assert_eq!(partial_info.primary_path.sigs_count, 0);
        assert_eq!(
            partial_info.primary_path.sigs_count,
            partial_info.primary_path.signed_pubkeys.len()
        );
        assert_eq!(partial_info.recovery_paths.len(), 2);
        assert_eq!(partial_info.recovery_paths[&26784].threshold, 3);
        assert_eq!(partial_info.recovery_paths[&26784].sigs_count, 0);
        assert_eq!(partial_info.recovery_paths[&53568].threshold, 2);
        assert_eq!(partial_info.recovery_paths[&53568].sigs_count, 1);
        for rec_path in partial_info.recovery_paths.values() {
            assert_eq!(rec_path.sigs_count, rec_path.signed_pubkeys.len());
        }

        // Finally do the same for the third recovery path.
        let fingerprint = third_reco_path
            .thresh_origins()
            .1
            .into_iter()
            .next()
            .unwrap()
            .0;
        psbt.inputs[0]
            .bip32_derivation
            .get_mut(&dummy_pubkey.inner)
            .unwrap()
            .0 = fingerprint;
        psbt.unsigned_tx.input[0].sequence = bitcoin::Sequence::from_height(62496);
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 3);
        assert_eq!(partial_info.primary_path.sigs_count, 0);
        assert_eq!(
            partial_info.primary_path.sigs_count,
            partial_info.primary_path.signed_pubkeys.len()
        );
        assert_eq!(partial_info.recovery_paths.len(), 3);
        assert_eq!(partial_info.recovery_paths[&26784].threshold, 3);
        assert_eq!(partial_info.recovery_paths[&26784].sigs_count, 0);
        assert_eq!(partial_info.recovery_paths[&53568].threshold, 2);
        assert_eq!(partial_info.recovery_paths[&53568].sigs_count, 0);
        assert_eq!(partial_info.recovery_paths[&62496].threshold, 1);
        assert_eq!(partial_info.recovery_paths[&62496].sigs_count, 1);
        for rec_path in partial_info.recovery_paths.values() {
            assert_eq!(rec_path.sigs_count, rec_path.signed_pubkeys.len());
        }
    }

    // The same as above but adapted for Taproot.
    #[test]
    fn partial_spend_info_taproot() {
        let secp = secp256k1::Secp256k1::signing_only();
        let dummy_xonly_pubkeys = [
            bitcoin::XOnlyPublicKey::from_str(
                "85899827df71b16f2f0eed47cb920e3963f5204e171f5ef7ea4eeec3d5ecc607",
            )
            .unwrap(),
            bitcoin::XOnlyPublicKey::from_str(
                "0b71b3b1b1f2deebedebd54723428efb1d8d00b3c38a138977ef28f4fea848c9",
            )
            .unwrap(),
        ];
        let dummy_sig = bitcoin::taproot::Signature::from_slice(&[0; 64]).unwrap();
        let dummy_leafhash = bitcoin::TapLeafHash::from_slice(&[0; 32]).unwrap();
        let dummy_psbt = psbt_from_str("cHNidP8BAHECAAAAAUSHuliRtuCX1S6JxRuDRqDCKkWfKmWL5sV9ukZ/wzvfAAAAAAD9////AogTAAAAAAAAFgAUIxe7UY6LJ6y5mFBoWTOoVispDmdwFwAAAAAAABYAFKqO83TK+t/KdpAt21z2HGC7/Z2FAAAAAAABASsQJwAAAAAAACIAIIIySQjGCTeyx/rKUQx8qobjhJeNCiVCliBJPdyRX6XKAQVBIQI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDKxzZHapFNYASzIYkEdH9bJz6nnqUG3uBB8kiK1asmgAAAA=");

        // A simple descriptor with 1 keys as primary path and 1 recovery key.
        let desc = LianaDescriptor::from_str("tr([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*,and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10)))").unwrap();
        let desc_info = desc.policy();
        let prim_key_fg = bip32::Fingerprint::from_str("f5acc2fd").unwrap();
        let prim_key_origin = (prim_key_fg, [0.into(), 0.into()][..].into());
        let recov_key_origin: (_, bip32::DerivationPath) = (
            bip32::Fingerprint::from_str("8a64f2a9").unwrap(),
            [0.into(), 4242.into()][..].into(),
        );

        // A PSBT with a single input and output, no signature. nSequence is not set to use the
        // recovery path.
        let mut unsigned_single_psbt: Psbt = dummy_psbt.clone();
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 0);
        assert!(info.primary_path.signed_pubkeys.is_empty());
        assert!(info.recovery_paths.is_empty());

        // If we set the sequence too low we still won't have the recovery path info.
        unsigned_single_psbt.unsigned_tx.input[0].sequence =
            Sequence::from_height(desc_info.recovery_paths.keys().next().unwrap() - 1);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert!(info.recovery_paths.is_empty());

        // Now if we set the sequence at the right value we'll have it.
        let timelock = *desc_info.recovery_paths.keys().next().unwrap();
        unsigned_single_psbt.unsigned_tx.input[0].sequence = Sequence::from_height(timelock);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        assert!(info.recovery_paths.contains_key(&timelock));

        // Even if it's a bit too high (as long as it's still a block height and activated)
        unsigned_single_psbt.unsigned_tx.input[0].sequence = Sequence::from_height(timelock + 42);
        let info = desc.partial_spend_info(&unsigned_single_psbt).unwrap();
        let recov_info = info.recovery_paths.get(&timelock).unwrap();
        assert_eq!(recov_info.threshold, 1);
        assert_eq!(recov_info.sigs_count, 0);
        assert!(recov_info.signed_pubkeys.is_empty());

        // The same PSBT but with an (invalid) signature for the primary key.
        let mut signed_single_psbt = dummy_psbt.clone();
        signed_single_psbt.inputs[0].tap_internal_key = Some(dummy_xonly_pubkeys[0]);
        signed_single_psbt.inputs[0].tap_key_sig = Some(dummy_sig);
        signed_single_psbt.inputs[0].tap_key_origins = [(
            dummy_xonly_pubkeys[0],
            (vec![dummy_leafhash], prim_key_origin.clone()),
        )]
        .iter()
        .cloned()
        .collect();
        let info = desc.partial_spend_info(&signed_single_psbt).unwrap();
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        assert!(info.recovery_paths.is_empty());

        // Now enable the recovery path and add a signature for the recovery key.
        let mut signed_recov_psbt = dummy_psbt.clone();
        signed_recov_psbt.unsigned_tx.input[0].sequence = Sequence::from_height(timelock);
        let recov_key = dummy_xonly_pubkeys[1];
        signed_recov_psbt.inputs[0]
            .tap_script_sigs
            .insert((recov_key, dummy_leafhash), dummy_sig);
        signed_recov_psbt.inputs[0].tap_key_origins =
            [(recov_key, (vec![dummy_leafhash], recov_key_origin.clone()))]
                .iter()
                .cloned()
                .collect();
        let info = desc.partial_spend_info(&signed_recov_psbt).unwrap();
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 0);
        assert!(info.primary_path.signed_pubkeys.is_empty());
        let recov_info = info.recovery_paths.get(&timelock).unwrap();
        assert_eq!(recov_info.threshold, 1);
        assert_eq!(recov_info.sigs_count, 1);
        assert!(
            recov_info.signed_pubkeys.len() == 1
                && recov_info.signed_pubkeys.contains_key(&recov_key_origin.0)
        );

        // A PSBT with multiple inputs, all signed for the primary path but with an ECDSA
        // signature. We must not account for those signatures since this is a Taproot descriptor.
        let psbt: Psbt = psbt_from_str("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");
        let info = desc.partial_spend_info(&psbt).unwrap();
        assert!(psbt
            .inputs
            .iter()
            .all(|psbt_in| psbt_in.partial_sigs.len() == 1));
        assert_eq!(info.primary_path.threshold, 1);
        assert_eq!(info.primary_path.sigs_count, 0);
        assert!(info.primary_path.signed_pubkeys.is_empty());
        assert!(info.recovery_paths.is_empty());

        // If we analyze a descriptor with a multisig we'll get the right threshold.
        let desc = LianaDescriptor::new(
            LianaPolicy::new(
                PathInfo::Multi(
                    2,
                    vec![
                        descriptor::DescriptorPublicKey::from_str("[f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*").unwrap(),
                        random_desc_key(&secp),
                        random_desc_key(&secp),
                    ],
                ),
                [(
                    42,
                    PathInfo::Multi(
                        1,
                        (0..3).map(|_| random_desc_key(&secp)).collect()
                    ),
                )]
                .iter()
                .cloned()
                .collect(),
            )
            .unwrap(),
        );
        let prim_key = dummy_xonly_pubkeys[0];
        let mut psbt = dummy_psbt.clone();
        psbt.inputs[0]
            .tap_script_sigs
            .insert((prim_key, dummy_leafhash), dummy_sig);
        psbt.inputs[0].tap_key_origins =
            [(prim_key, (vec![dummy_leafhash], prim_key_origin.clone()))]
                .iter()
                .cloned()
                .collect();
        let info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(info.primary_path.threshold, 2);
        assert_eq!(info.primary_path.sigs_count, 1);
        assert!(
            info.primary_path.signed_pubkeys.len() == 1
                && info.primary_path.signed_pubkeys.contains_key(&prim_key_fg)
        );
        assert!(info.recovery_paths.is_empty());

        // A not very well thought-out decaying multisig.
        let prim_path = PathInfo::Multi(3, vec![
                        descriptor::DescriptorPublicKey::from_str("[f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*").unwrap(),
                        random_desc_key(&secp),
                        random_desc_key(&secp),
                    ]);
        let first_reco_path = PathInfo::Multi(3, (0..5).map(|_| random_desc_key(&secp)).collect());
        let sec_reco_path = PathInfo::Multi(2, (0..5).map(|_| random_desc_key(&secp)).collect());
        let third_reco_path = PathInfo::Multi(1, (0..5).map(|_| random_desc_key(&secp)).collect());
        let liana_policy = LianaPolicy::new(
            prim_path.clone(),
            [
                (26784, first_reco_path.clone()),
                (53568, sec_reco_path.clone()),
                (62496, third_reco_path.clone()),
            ]
            .iter()
            .cloned()
            .collect(),
        )
        .unwrap();
        let desc = LianaDescriptor::new(liana_policy.clone());
        let policy = desc.policy();
        assert_eq!(policy, liana_policy);
        let mut psbt = dummy_psbt.clone();
        let empty_partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(empty_partial_info.primary_path.threshold, 3);
        assert_eq!(empty_partial_info.primary_path.sigs_count, 0);
        assert_eq!(
            empty_partial_info.primary_path.sigs_count,
            empty_partial_info.primary_path.signed_pubkeys.len()
        );
        assert!(empty_partial_info.recovery_paths.is_empty());

        // Now set a signature for the primary path. All recovery paths still empty, a signature is
        // present for the primary path.
        let prim_key = dummy_xonly_pubkeys[0];
        psbt.inputs[0]
            .tap_script_sigs
            .insert((prim_key, dummy_leafhash), dummy_sig);
        psbt.inputs[0].tap_key_origins =
            [(prim_key, (vec![dummy_leafhash], prim_key_origin.clone()))]
                .iter()
                .cloned()
                .collect();
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 3);
        assert_eq!(partial_info.primary_path.sigs_count, 1);
        assert_eq!(
            partial_info.primary_path.sigs_count,
            partial_info.primary_path.signed_pubkeys.len()
        );
        assert!(partial_info.recovery_paths.is_empty());

        // Now enable the first recovery path and make the signature be for this path.
        let fingerprint = first_reco_path
            .thresh_origins()
            .1
            .into_iter()
            .next()
            .unwrap()
            .0;
        psbt.inputs[0]
            .tap_key_origins
            .get_mut(&prim_key)
            .unwrap()
            .1
             .0 = fingerprint;
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 3);
        assert_eq!(partial_info.primary_path.sigs_count, 0);
        assert_eq!(
            partial_info.primary_path.sigs_count,
            partial_info.primary_path.signed_pubkeys.len()
        );
        assert!(partial_info.recovery_paths.is_empty());
        psbt.unsigned_tx.input[0].sequence = bitcoin::Sequence::from_height(26784);
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.recovery_paths.len(), 1);
        assert_eq!(partial_info.recovery_paths[&26784].threshold, 3);
        assert_eq!(partial_info.recovery_paths[&26784].sigs_count, 1);
        assert_eq!(
            partial_info.recovery_paths[&26784].signed_pubkeys.len(),
            partial_info.recovery_paths[&26784].sigs_count
        );

        // Now enable the second recovery path and make the signature be for this path.
        let fingerprint = sec_reco_path
            .thresh_origins()
            .1
            .into_iter()
            .next()
            .unwrap()
            .0;
        psbt.inputs[0]
            .tap_key_origins
            .get_mut(&prim_key)
            .unwrap()
            .1
             .0 = fingerprint;
        psbt.unsigned_tx.input[0].sequence = bitcoin::Sequence::from_height(53568);
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 3);
        assert_eq!(partial_info.primary_path.sigs_count, 0);
        assert_eq!(
            partial_info.primary_path.sigs_count,
            partial_info.primary_path.signed_pubkeys.len()
        );
        assert_eq!(partial_info.recovery_paths.len(), 2);
        assert_eq!(partial_info.recovery_paths[&26784].threshold, 3);
        assert_eq!(partial_info.recovery_paths[&26784].sigs_count, 0);
        assert_eq!(partial_info.recovery_paths[&53568].threshold, 2);
        assert_eq!(partial_info.recovery_paths[&53568].sigs_count, 1);
        for rec_path in partial_info.recovery_paths.values() {
            assert_eq!(rec_path.sigs_count, rec_path.signed_pubkeys.len());
        }

        // Finally do the same for the third recovery path.
        let fingerprint = third_reco_path
            .thresh_origins()
            .1
            .into_iter()
            .next()
            .unwrap()
            .0;
        psbt.inputs[0]
            .tap_key_origins
            .get_mut(&prim_key)
            .unwrap()
            .1
             .0 = fingerprint;
        psbt.unsigned_tx.input[0].sequence = bitcoin::Sequence::from_height(53568);
        psbt.unsigned_tx.input[0].sequence = bitcoin::Sequence::from_height(62496);
        let partial_info = desc.partial_spend_info(&psbt).unwrap();
        assert_eq!(partial_info.primary_path.threshold, 3);
        assert_eq!(partial_info.primary_path.sigs_count, 0);
        assert_eq!(
            partial_info.primary_path.sigs_count,
            partial_info.primary_path.signed_pubkeys.len()
        );
        assert_eq!(partial_info.recovery_paths.len(), 3);
        assert_eq!(partial_info.recovery_paths[&26784].threshold, 3);
        assert_eq!(partial_info.recovery_paths[&26784].sigs_count, 0);
        assert_eq!(partial_info.recovery_paths[&53568].threshold, 2);
        assert_eq!(partial_info.recovery_paths[&53568].sigs_count, 0);
        assert_eq!(partial_info.recovery_paths[&62496].threshold, 1);
        assert_eq!(partial_info.recovery_paths[&62496].sigs_count, 1);
        for rec_path in partial_info.recovery_paths.values() {
            assert_eq!(rec_path.sigs_count, rec_path.signed_pubkeys.len());
        }
    }

    #[test]
    fn bip32_derivs_pruning() {
        let secp = secp256k1::Secp256k1::signing_only();
        let dummy_leafhash = bitcoin::TapLeafHash::from_slice(&[0; 32]).unwrap();

        // A signet descriptor created using Liana v2.
        let desc = LianaDescriptor::from_str("wsh(or_i(and_v(v:thresh(3,pkh([636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/<4;5>/*),a:pkh([172ba1bc/48'/1'/0'/2']tpubDEgTZEAraUrKmnbyKJuXYGFPzNCm82bjMqd2GRy2HKviJ1moLtEZrHoUeG2o6uyWLEGx4yBWpctAmxcBx1b5nrrrBo5LjskRxRMDmwkuKxq/<4;5>/*),a:pkh([903115ef/48'/1'/0'/2']tpubDF2Hqd3HXUn5bDMVa2gssqmdTjQsLm9Vc8CSSJFk4YwQg8PChCZiWopAeQ6ZCEWt21n1W8ApEGxEvtB8uPnWW6EG3fwPAFnFM8US4QmgKvp/<4;5>/*)),older(6)),or_d(multi(3,[636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/<0;1>/*,[172ba1bc/48'/1'/0'/2']tpubDEgTZEAraUrKmnbyKJuXYGFPzNCm82bjMqd2GRy2HKviJ1moLtEZrHoUeG2o6uyWLEGx4yBWpctAmxcBx1b5nrrrBo5LjskRxRMDmwkuKxq/<0;1>/*,[903115ef/48'/1'/0'/2']tpubDF2Hqd3HXUn5bDMVa2gssqmdTjQsLm9Vc8CSSJFk4YwQg8PChCZiWopAeQ6ZCEWt21n1W8ApEGxEvtB8uPnWW6EG3fwPAFnFM8US4QmgKvp/<0;1>/*),and_v(v:thresh(2,pkh([636adf3f/48'/1'/0'/2']tpubDEE9FvWbG4kg4gxDNrALgrWLiHwNMXNs8hk6nXNPw4VHKot16xd2251vwi2M6nsyQTkak5FJNHVHkCcuzmvpSbWHdumX3DxpDm89iTfSBaL/<2;3>/*),a:pkh([172ba1bc/48'/1'/0'/2']tpubDEgTZEAraUrKmnbyKJuXYGFPzNCm82bjMqd2GRy2HKviJ1moLtEZrHoUeG2o6uyWLEGx4yBWpctAmxcBx1b5nrrrBo5LjskRxRMDmwkuKxq/<2;3>/*),a:pkh([903115ef/48'/1'/0'/2']tpubDF2Hqd3HXUn5bDMVa2gssqmdTjQsLm9Vc8CSSJFk4YwQg8PChCZiWopAeQ6ZCEWt21n1W8ApEGxEvtB8uPnWW6EG3fwPAFnFM8US4QmgKvp/<2;3>/*)),older(3)))))#jxya9h7u").unwrap();
        // A spend PSBT created using Liana v2.
        let psbt = Psbt::from_str("cHNidP8BAFICAAAAAc+3IQFejOVro5Hlwy18au5Jr5mJX+tNMGk0ZE1hydIbAQAAAAD9////ARhzAQAAAAAAFgAUqJZUU7Fqu+bIvxjNw+TAtTwP9HQAAAAAAAEAzQIAAAAAAQEIoAeUdfZj04Ds8EspEK222TJdDNy1WZb/Mg1PJbQekwAAAAAA/f///wKQCQQAAAAAACJRIPJojBgnDc9oUS5lDNx/YJznYR2NPQue7h/d+o5Z+2FQoIYBAAAAAAAiACDZrCBvscZpg+S+IaoZBJjyKDdrNS3oXPaF17DNaB+4mAFAe9yuRS3Vn8A5NUglhwiX7vN0wpQ0Q43ClWtJRnC2HJ66h5HYJ/p8xHgHOhRDUWRzcXLLGl+brc5dW+k0OvIZEyuLAgABASughgEAAAAAACIAINmsIG+xxmmD5L4hqhkEmPIoN2s1Lehc9oXXsM1oH7iYAQX9GQFjdqkU2zK+b9oTL/KfnOSYtq3wmtf4qP6IrGt2qRTSNOD0U7fuHdAnKchIf8GmUO904YisbJNrdqkUE5TQk5mdyYtviaGAsIiOgc4y6wGIrGyTU4hWsmdTIQOirPI1KXBtP2Tg2FQxSo4BjFBTf+dCKtZwDQt056slgCEDDHE7Hpxq++JsjZdbfwsPiA6pmq0dV00tR3hc2sus8KkhA2nPUthIMe1SeFegiZEKZF69yJerP1RFVlyu66C5lOVVU65zZHapFEUmCTccyLJXczvUfPUOCXr7CN0uiKxrdqkUeJmVqUt1Q4aFREOUWKX9U/SuZZ2IrGyTa3apFBDmKn40ceTWVbwxRI21c2qji1tOiKxsk1KIU7JoaCIGAjCZLg7xtlG43xEvns0TRd5gHpPrZWzAaYjo3lheMw/hHJAxFe8wAACAAQAAgAAAAIACAACAAgAAAAgAAAAiBgI0Y2/HRNvXA3niUE3RvrzQcCDiJ4F6vVog0uIanRUWHhwXK6G8MAAAgAEAAIAAAACAAgAAgAIAAAAIAAAAIgYCQKZf/IBUWv4F4mGVTv5PlqCceXFtlhfOgW0kIAPI74scFyuhvDAAAIABAACAAAAAgAIAAIAEAAAACAAAACIGAkDfArY5kwHyHvKllcCMhQLErtDmT/A13vABH8PBQ6yIHGNq3z8wAACAAQAAgAAAAIACAACABAAAAAgAAAAiBgLp9dq4ku0u9UKpIRasIb5QEPgPkDcxdcSXYBfW7mUcqByQMRXvMAAAgAEAAIAAAACAAgAAgAQAAAAIAAAAIgYDDHE7Hpxq++JsjZdbfwsPiA6pmq0dV00tR3hc2sus8KkcFyuhvDAAAIABAACAAAAAgAIAAIAAAAAACAAAACIGA0SIq7IkQJYb7brFx54mPzwUl/DzCGja0pdwFFckfm6WHGNq3z8wAACAAQAAgAAAAIACAACAAgAAAAgAAAAiBgNpz1LYSDHtUnhXoImRCmRevciXqz9URVZcruuguZTlVRyQMRXvMAAAgAEAAIAAAACAAgAAgAAAAAAIAAAAIgYDoqzyNSlwbT9k4NhUMUqOAYxQU3/nQirWcA0LdOerJYAcY2rfPzAAAIABAACAAAAAgAIAAIAAAAAACAAAAAAA").unwrap();
        // The above PSBT with BIP32 derivs manually pruned using bip174.org.
        let pruned_psbt = Psbt::from_str("cHNidP8BAFICAAAAAc+3IQFejOVro5Hlwy18au5Jr5mJX+tNMGk0ZE1hydIbAQAAAAD9////ARhzAQAAAAAAFgAUqJZUU7Fqu+bIvxjNw+TAtTwP9HQAAAAAAAEAzQIAAAAAAQEIoAeUdfZj04Ds8EspEK222TJdDNy1WZb/Mg1PJbQekwAAAAAA/f///wKQCQQAAAAAACJRIPJojBgnDc9oUS5lDNx/YJznYR2NPQue7h/d+o5Z+2FQoIYBAAAAAAAiACDZrCBvscZpg+S+IaoZBJjyKDdrNS3oXPaF17DNaB+4mAFAe9yuRS3Vn8A5NUglhwiX7vN0wpQ0Q43ClWtJRnC2HJ66h5HYJ/p8xHgHOhRDUWRzcXLLGl+brc5dW+k0OvIZEyuLAgABASughgEAAAAAACIAINmsIG+xxmmD5L4hqhkEmPIoN2s1Lehc9oXXsM1oH7iYAQX9GQFjdqkU2zK+b9oTL/KfnOSYtq3wmtf4qP6IrGt2qRTSNOD0U7fuHdAnKchIf8GmUO904YisbJNrdqkUE5TQk5mdyYtviaGAsIiOgc4y6wGIrGyTU4hWsmdTIQOirPI1KXBtP2Tg2FQxSo4BjFBTf+dCKtZwDQt056slgCEDDHE7Hpxq++JsjZdbfwsPiA6pmq0dV00tR3hc2sus8KkhA2nPUthIMe1SeFegiZEKZF69yJerP1RFVlyu66C5lOVVU65zZHapFEUmCTccyLJXczvUfPUOCXr7CN0uiKxrdqkUeJmVqUt1Q4aFREOUWKX9U/SuZZ2IrGyTa3apFBDmKn40ceTWVbwxRI21c2qji1tOiKxsk1KIU7JoaCIGAwxxOx6cavvibI2XW38LD4gOqZqtHVdNLUd4XNrLrPCpHBcrobwwAACAAQAAgAAAAIACAACAAAAAAAgAAAAiBgNpz1LYSDHtUnhXoImRCmRevciXqz9URVZcruuguZTlVRyQMRXvMAAAgAEAAIAAAACAAgAAgAAAAAAIAAAAIgYDoqzyNSlwbT9k4NhUMUqOAYxQU3/nQirWcA0LdOerJYAcY2rfPzAAAIABAACAAAAAgAIAAIAAAAAACAAAAAAA").unwrap();

        // Before pruning it the PSBT has an entry per key in the descriptor.
        assert_eq!(psbt.inputs[0].bip32_derivation.len(), 9);

        // Prune the PSBT. It should result in the same as when manually pruned using bip174.org.
        assert_ne!(psbt, pruned_psbt);
        let prim_path_info = desc.policy().primary_path;
        let psbt = desc.prune_bip32_derivs(psbt, &prim_path_info);
        assert_eq!(psbt, pruned_psbt);

        // After pruning it the PSBT only has an entry per key in the primary path.
        assert_eq!(psbt.inputs[0].bip32_derivation.len(), 3);

        // Same but with recovery PSBTs.
        let psbt = Psbt::from_str("cHNidP8BAFICAAAAAc+3IQFejOVro5Hlwy18au5Jr5mJX+tNMGk0ZE1hydIbAQAAAAADAAAAAbSFAQAAAAAAFgAUBSY69rqtGQLCmhuT29Ep4ZO5Sk8AAAAAAAEAzQIAAAAAAQEIoAeUdfZj04Ds8EspEK222TJdDNy1WZb/Mg1PJbQekwAAAAAA/f///wKQCQQAAAAAACJRIPJojBgnDc9oUS5lDNx/YJznYR2NPQue7h/d+o5Z+2FQoIYBAAAAAAAiACDZrCBvscZpg+S+IaoZBJjyKDdrNS3oXPaF17DNaB+4mAFAe9yuRS3Vn8A5NUglhwiX7vN0wpQ0Q43ClWtJRnC2HJ66h5HYJ/p8xHgHOhRDUWRzcXLLGl+brc5dW+k0OvIZEyuLAgABASughgEAAAAAACIAINmsIG+xxmmD5L4hqhkEmPIoN2s1Lehc9oXXsM1oH7iYAQX9GQFjdqkU2zK+b9oTL/KfnOSYtq3wmtf4qP6IrGt2qRTSNOD0U7fuHdAnKchIf8GmUO904YisbJNrdqkUE5TQk5mdyYtviaGAsIiOgc4y6wGIrGyTU4hWsmdTIQOirPI1KXBtP2Tg2FQxSo4BjFBTf+dCKtZwDQt056slgCEDDHE7Hpxq++JsjZdbfwsPiA6pmq0dV00tR3hc2sus8KkhA2nPUthIMe1SeFegiZEKZF69yJerP1RFVlyu66C5lOVVU65zZHapFEUmCTccyLJXczvUfPUOCXr7CN0uiKxrdqkUeJmVqUt1Q4aFREOUWKX9U/SuZZ2IrGyTa3apFBDmKn40ceTWVbwxRI21c2qji1tOiKxsk1KIU7JoaCIGAjCZLg7xtlG43xEvns0TRd5gHpPrZWzAaYjo3lheMw/hHJAxFe8wAACAAQAAgAAAAIACAACAAgAAAAgAAAAiBgI0Y2/HRNvXA3niUE3RvrzQcCDiJ4F6vVog0uIanRUWHhwXK6G8MAAAgAEAAIAAAACAAgAAgAIAAAAIAAAAIgYCQKZf/IBUWv4F4mGVTv5PlqCceXFtlhfOgW0kIAPI74scFyuhvDAAAIABAACAAAAAgAIAAIAEAAAACAAAACIGAkDfArY5kwHyHvKllcCMhQLErtDmT/A13vABH8PBQ6yIHGNq3z8wAACAAQAAgAAAAIACAACABAAAAAgAAAAiBgLp9dq4ku0u9UKpIRasIb5QEPgPkDcxdcSXYBfW7mUcqByQMRXvMAAAgAEAAIAAAACAAgAAgAQAAAAIAAAAIgYDDHE7Hpxq++JsjZdbfwsPiA6pmq0dV00tR3hc2sus8KkcFyuhvDAAAIABAACAAAAAgAIAAIAAAAAACAAAACIGA0SIq7IkQJYb7brFx54mPzwUl/DzCGja0pdwFFckfm6WHGNq3z8wAACAAQAAgAAAAIACAACAAgAAAAgAAAAiBgNpz1LYSDHtUnhXoImRCmRevciXqz9URVZcruuguZTlVRyQMRXvMAAAgAEAAIAAAACAAgAAgAAAAAAIAAAAIgYDoqzyNSlwbT9k4NhUMUqOAYxQU3/nQirWcA0LdOerJYAcY2rfPzAAAIABAACAAAAAgAIAAIAAAAAACAAAAAAA").unwrap();
        let pruned_psbt = Psbt::from_str("cHNidP8BAFICAAAAAc+3IQFejOVro5Hlwy18au5Jr5mJX+tNMGk0ZE1hydIbAQAAAAADAAAAAbSFAQAAAAAAFgAUBSY69rqtGQLCmhuT29Ep4ZO5Sk8AAAAAAAEAzQIAAAAAAQEIoAeUdfZj04Ds8EspEK222TJdDNy1WZb/Mg1PJbQekwAAAAAA/f///wKQCQQAAAAAACJRIPJojBgnDc9oUS5lDNx/YJznYR2NPQue7h/d+o5Z+2FQoIYBAAAAAAAiACDZrCBvscZpg+S+IaoZBJjyKDdrNS3oXPaF17DNaB+4mAFAe9yuRS3Vn8A5NUglhwiX7vN0wpQ0Q43ClWtJRnC2HJ66h5HYJ/p8xHgHOhRDUWRzcXLLGl+brc5dW+k0OvIZEyuLAgABASughgEAAAAAACIAINmsIG+xxmmD5L4hqhkEmPIoN2s1Lehc9oXXsM1oH7iYAQX9GQFjdqkU2zK+b9oTL/KfnOSYtq3wmtf4qP6IrGt2qRTSNOD0U7fuHdAnKchIf8GmUO904YisbJNrdqkUE5TQk5mdyYtviaGAsIiOgc4y6wGIrGyTU4hWsmdTIQOirPI1KXBtP2Tg2FQxSo4BjFBTf+dCKtZwDQt056slgCEDDHE7Hpxq++JsjZdbfwsPiA6pmq0dV00tR3hc2sus8KkhA2nPUthIMe1SeFegiZEKZF69yJerP1RFVlyu66C5lOVVU65zZHapFEUmCTccyLJXczvUfPUOCXr7CN0uiKxrdqkUeJmVqUt1Q4aFREOUWKX9U/SuZZ2IrGyTa3apFBDmKn40ceTWVbwxRI21c2qji1tOiKxsk1KIU7JoaCIGAjCZLg7xtlG43xEvns0TRd5gHpPrZWzAaYjo3lheMw/hHJAxFe8wAACAAQAAgAAAAIACAACAAgAAAAgAAAAiBgI0Y2/HRNvXA3niUE3RvrzQcCDiJ4F6vVog0uIanRUWHhwXK6G8MAAAgAEAAIAAAACAAgAAgAIAAAAIAAAAIgYDRIirsiRAlhvtusXHniY/PBSX8PMIaNrSl3AUVyR+bpYcY2rfPzAAAIABAACAAAAAgAIAAIACAAAACAAAAAAA").unwrap();
        assert_ne!(psbt, pruned_psbt);
        assert_eq!(psbt.inputs[0].bip32_derivation.len(), 9);
        let psbt = desc.prune_bip32_derivs_last_avail(psbt).unwrap();
        assert_eq!(psbt.inputs[0].bip32_derivation.len(), 3);
        assert_eq!(psbt, pruned_psbt);

        // Now do the same but with a Taproot descriptor.
        let (prim_key, rec_key) = (random_desc_key(&secp), random_desc_key(&secp));
        let prim_origin =
            if let descriptor::DescriptorPublicKey::MultiXPub(descriptor::DescriptorMultiXKey {
                ref origin,
                ..
            }) = prim_key
            {
                (origin.as_ref().unwrap().0, [0.into(), 1.into()][..].into())
            } else {
                unreachable!();
            };
        let rec_origin =
            if let descriptor::DescriptorPublicKey::MultiXPub(descriptor::DescriptorMultiXKey {
                xkey,
                ..
            }) = rec_key
            {
                (xkey.fingerprint(), [0.into(), 1.into()][..].into())
            } else {
                unreachable!();
            };
        let tap_desc = LianaDescriptor::new(
            LianaPolicy::new(
                PathInfo::Single(prim_key),
                [(14, PathInfo::Single(rec_key))].iter().cloned().collect(),
            )
            .unwrap(),
        );
        let prim_path_info = tap_desc.policy().primary_path;
        let rec_path_info = &tap_desc.policy().recovery_paths[&14];
        let mut tap_psbt = psbt;
        let dummy_xonly_pubkey = bitcoin::XOnlyPublicKey::from_str(
            "85899827df71b16f2f0eed47cb920e3963f5204e171f5ef7ea4eeec3d5ecc607",
        )
        .unwrap();

        // Set the origins for the primary key. Pruning all but the primary origins should conserve
        // it. Pruning all but the recovery should drop it.
        tap_psbt.inputs[0].bip32_derivation.clear();
        tap_psbt.inputs[0].tap_key_origins =
            [(dummy_xonly_pubkey, (vec![dummy_leafhash], prim_origin))]
                .iter()
                .cloned()
                .collect();
        assert_eq!(tap_psbt.inputs[0].tap_key_origins.len(), 1);
        let tap_psbt = tap_desc.prune_bip32_derivs(tap_psbt, &prim_path_info);
        assert_eq!(tap_psbt.inputs[0].tap_key_origins.len(), 1);
        let mut tap_psbt = tap_desc.prune_bip32_derivs(tap_psbt, rec_path_info);
        assert!(tap_psbt.inputs[0].tap_key_origins.is_empty());

        // Do the opposite.
        tap_psbt.inputs[0].bip32_derivation.clear();
        tap_psbt.inputs[0].tap_key_origins =
            [(dummy_xonly_pubkey, (vec![dummy_leafhash], rec_origin))]
                .iter()
                .cloned()
                .collect();
        assert_eq!(tap_psbt.inputs[0].tap_key_origins.len(), 1);
        let tap_psbt = tap_desc.prune_bip32_derivs(tap_psbt, rec_path_info);
        assert_eq!(tap_psbt.inputs[0].tap_key_origins.len(), 1);
        let tap_psbt = tap_desc.prune_bip32_derivs(tap_psbt, &prim_path_info);
        assert!(tap_psbt.inputs[0].tap_key_origins.is_empty());
    }

    #[test]
    fn unsigned_tx_max_weight_and_vbytes() {
        let desc = LianaDescriptor::from_str("tr(tpubD6NzVbkrYhZ4WUdbVsXDYBCXS8EPSYG1cAN9g4uP6uLQHMHXRvHSFkQBXy7MBeAvV8PDVJJ4o3AwYMKJHp45ci2g69UCAKteVSAJ61CnGEV/<0;1>/*,{and_v(v:pk([9e1c1983/48'/1'/0'/2']tpubDEWCLCMncbStq4BLXkQUAPqzzrh2tQUgYeQPt4NrB5D7gRraMyGbRqzPTmQGvqfdaFsXDVGSQBRgfXuNjDyfU626pxSjpQZszFNY6CzogxK/<2;3>/*),older(65535)),multi_a(2,[9e1c1983/48'/1'/0'/2']tpubDEWCLCMncbStq4BLXkQUAPqzzrh2tQUgYeQPt4NrB5D7gRraMyGbRqzPTmQGvqfdaFsXDVGSQBRgfXuNjDyfU626pxSjpQZszFNY6CzogxK/<0;1>/*,[3b1913e1/48'/1'/0'/2']tpubDFeZ2ezf4VUuTnjdhxJ1DKhLa2t6vzXZNz8NnEgeT2PN4pPqTCTeWUcaxKHPJcf1C8WzkLA71zSjDwuo4zqu4kkiL91ZUmJydC8f1gx89wM/<0;1>/*)})#ee0r4tw5").unwrap();
        // The following PSBT was generated from this descriptor.
        // See https://mempool.space/signet/tx/a63c4a69be71fcba0e16f742a2697401fbc47ad7dff10a790b8f961004aa0ab4 for the corresponding tx:
        let psbt = Psbt::from_str("cHNidP8BAF4CAAAAAU2eiiiqTjQHmDarPBbpDO7b/jXeU3ABO20p0sZ3U7SoAAAAAAD9////AaQiAAAAAAAAIlEg+5nFiKkeVa9DFXLNvpIRcDNU7a4hN2QQhb7LHBad+AAVWgMAAAEBK+YjAAAAAAAAIlEgKA3Jqw7wXvY+ggshuLnufWEMZvDvz5fd7guPe74OFr9BFOwaZX+B87gSAqM66+YwA2L5da0h0+PPsDMXht+IcnRsHCjA6OkyoLbI1pYXZBVcKqW6G8fXOBHIUtxyVltu/VVAP8MQWa0ipMEXw6XfBexyPOQfb7TJpX6+KCiz2XA/mnwRyuYibz0aLl5/ZFNFvvgN5D+JYrmGACcafbXZOAtyw0IVwXv1NpDYfRDm2LstW9CzwDg86+y3PAi9ipB5m3acrIhsHCjA6OkyoLbI1pYXZBVcKqW6G8fXOBHIUtxyVltu/VUoIBYAvFRImNeU9Uegt66wQrOOwURL8+t2LjLQLCllgNHOrQP//wCywEIVwXv1NpDYfRDm2LstW9CzwDg86+y3PAi9ipB5m3acrIhsb1/MmmOazbHeRxkVrKn2+tW/CKyAMUbx3oUK+GaB3h1HIMjdaFdS9My6uOk2lBdGjnFLNSfvvRhvUGWk6VdVgyMLrCDsGmV/gfO4EgKjOuvmMANi+XWtIdPjz7AzF4bfiHJ0bLpSnMAhFhYAvFRImNeU9Uegt66wQrOOwURL8+t2LjLQLCllgNHOPQFvX8yaY5rNsd5HGRWsqfb61b8IrIAxRvHehQr4ZoHeHZ4cGYMwAACAAQAAgAAAAIACAACAAgAAAAAAAAAhFnv1NpDYfRDm2LstW9CzwDg86+y3PAi9ipB5m3acrIhsDQB8Rh5dAAAAAAAAAAAhFsjdaFdS9My6uOk2lBdGjnFLNSfvvRhvUGWk6VdVgyMLPQEcKMDo6TKgtsjWlhdkFVwqpbobx9c4EchS3HJWW279VZ4cGYMwAACAAQAAgAAAAIACAACAAAAAAAAAAAAhFuwaZX+B87gSAqM66+YwA2L5da0h0+PPsDMXht+IcnRsPQEcKMDo6TKgtsjWlhdkFVwqpbobx9c4EchS3HJWW279VTsZE+EwAACAAQAAgAAAAIACAACAAAAAAAAAAAABFyB79TaQ2H0Q5ti7LVvQs8A4POvstzwIvYqQeZt2nKyIbAEYIFHdAhNfvRTz8EPSAs+Gf/HrAULFx3vOs18D0PWq9kGwAAEFIDNwiV3+PJcHk97y59EcUfNkHjBBPZjvSN/Hgn0S01LQAQZzAcAnIClBgez9GIlLyjqN3NPltEhBDUjmDbsiVpWlba2IMurdrQP//wCyAcBGIBs6CbrR7SfZwJL6Q4beOPUvPbetXt/T/QmplPDbPQAwrCBGP2ABIvtgyIu7uSrKRE6rIY3VNZOEJ028nuqeWNSqTbpSnCEHGzoJutHtJ9nAkvpDht449S89t61e39P9CamU8Ns9ADA9AY7TqQXjhS0u6aC8jSA+/MN5WjE8uYIs5D4/oTu1eC9PnhwZgzAAAIABAACAAAAAgAIAAIABAAAAAQAAACEHKUGB7P0YiUvKOo3c0+W0SEENSOYNuyJWlaVtrYgy6t09ARHbEQ0ckrM/6qD8+TyaH5SmkKpv4e0rE07oC0VK4TxBnhwZgzAAAIABAACAAAAAgAIAAIADAAAAAQAAACEHM3CJXf48lweT3vLn0RxR82QeMEE9mO9I38eCfRLTUtANAHxGHl0BAAAAAQAAACEHRj9gASL7YMiLu7kqykROqyGN1TWThCdNvJ7qnljUqk09AY7TqQXjhS0u6aC8jSA+/MN5WjE8uYIs5D4/oTu1eC9POxkT4TAAAIABAACAAAAAgAIAAIABAAAAAQAAAAA=").unwrap();
        assert_eq!(desc.unsigned_tx_max_weight(&psbt.unsigned_tx, true), 646);
        assert_eq!(desc.unsigned_tx_max_vbytes(&psbt.unsigned_tx, true), 162); // 646/4=161.5

        // If `use_primary_path` is `false`, an extra 2 is added by `max_sat_weight` as it
        // includes the sighash suffix for each of the two signatures.
        assert_eq!(desc.unsigned_tx_max_weight(&psbt.unsigned_tx, false), 648);
        assert_eq!(desc.unsigned_tx_max_vbytes(&psbt.unsigned_tx, false), 162); // 648/4 = 162
    }

    fn run_change_detection(
        desc: LianaDescriptor,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
    ) {
        // Unrelated PSBT from another unit test above.
        let mut psbt = Psbt::from_str("cHNidP8BAFICAAAAAc+3IQFejOVro5Hlwy18au5Jr5mJX+tNMGk0ZE1hydIbAQAAAAD9////ARhzAQAAAAAAFgAUqJZUU7Fqu+bIvxjNw+TAtTwP9HQAAAAAAAEAzQIAAAAAAQEIoAeUdfZj04Ds8EspEK222TJdDNy1WZb/Mg1PJbQekwAAAAAA/f///wKQCQQAAAAAACJRIPJojBgnDc9oUS5lDNx/YJznYR2NPQue7h/d+o5Z+2FQoIYBAAAAAAAiACDZrCBvscZpg+S+IaoZBJjyKDdrNS3oXPaF17DNaB+4mAFAe9yuRS3Vn8A5NUglhwiX7vN0wpQ0Q43ClWtJRnC2HJ66h5HYJ/p8xHgHOhRDUWRzcXLLGl+brc5dW+k0OvIZEyuLAgABASughgEAAAAAACIAINmsIG+xxmmD5L4hqhkEmPIoN2s1Lehc9oXXsM1oH7iYAQX9GQFjdqkU2zK+b9oTL/KfnOSYtq3wmtf4qP6IrGt2qRTSNOD0U7fuHdAnKchIf8GmUO904YisbJNrdqkUE5TQk5mdyYtviaGAsIiOgc4y6wGIrGyTU4hWsmdTIQOirPI1KXBtP2Tg2FQxSo4BjFBTf+dCKtZwDQt056slgCEDDHE7Hpxq++JsjZdbfwsPiA6pmq0dV00tR3hc2sus8KkhA2nPUthIMe1SeFegiZEKZF69yJerP1RFVlyu66C5lOVVU65zZHapFEUmCTccyLJXczvUfPUOCXr7CN0uiKxrdqkUeJmVqUt1Q4aFREOUWKX9U/SuZZ2IrGyTa3apFBDmKn40ceTWVbwxRI21c2qji1tOiKxsk1KIU7JoaCIGAjCZLg7xtlG43xEvns0TRd5gHpPrZWzAaYjo3lheMw/hHJAxFe8wAACAAQAAgAAAAIACAACAAgAAAAgAAAAiBgI0Y2/HRNvXA3niUE3RvrzQcCDiJ4F6vVog0uIanRUWHhwXK6G8MAAAgAEAAIAAAACAAgAAgAIAAAAIAAAAIgYCQKZf/IBUWv4F4mGVTv5PlqCceXFtlhfOgW0kIAPI74scFyuhvDAAAIABAACAAAAAgAIAAIAEAAAACAAAACIGAkDfArY5kwHyHvKllcCMhQLErtDmT/A13vABH8PBQ6yIHGNq3z8wAACAAQAAgAAAAIACAACABAAAAAgAAAAiBgLp9dq4ku0u9UKpIRasIb5QEPgPkDcxdcSXYBfW7mUcqByQMRXvMAAAgAEAAIAAAACAAgAAgAQAAAAIAAAAIgYDDHE7Hpxq++JsjZdbfwsPiA6pmq0dV00tR3hc2sus8KkcFyuhvDAAAIABAACAAAAAgAIAAIAAAAAACAAAACIGA0SIq7IkQJYb7brFx54mPzwUl/DzCGja0pdwFFckfm6WHGNq3z8wAACAAQAAgAAAAIACAACAAgAAAAgAAAAiBgNpz1LYSDHtUnhXoImRCmRevciXqz9URVZcruuguZTlVRyQMRXvMAAAgAEAAIAAAACAAgAAgAAAAAAIAAAAIgYDoqzyNSlwbT9k4NhUMUqOAYxQU3/nQirWcA0LdOerJYAcY2rfPzAAAIABAACAAAAAgAIAAIAAAAAACAAAAAAA").unwrap();

        // The PSBT has unrelated outputs. Those aren't detected as change.
        assert!(!psbt.outputs.is_empty());
        assert_eq!(desc.change_indexes(&psbt, secp).len(), 0);

        // Add a change output, it's correctly detected as such.
        let der_desc = desc.change_descriptor().derive(999.into(), secp);
        let txo = bitcoin::TxOut {
            script_pubkey: der_desc.script_pubkey(),
            value: bitcoin::Amount::MAX_MONEY,
        };
        let mut psbt_out = Default::default();
        der_desc.update_change_psbt_out(&mut psbt_out);
        psbt.unsigned_tx.output.push(txo);
        psbt.outputs.push(psbt_out);
        let indexes = desc.change_indexes(&psbt, secp);
        assert_eq!(indexes.len(), 1);
        assert!(matches!(
            indexes[0],
            ChangeOutput::ChangeAddress { index: 1 }
        ));

        // Add another change output, but to a deposit address. Both change outputs are detected.
        let der_desc = desc.receive_descriptor().derive(424242.into(), secp);
        let txo = bitcoin::TxOut {
            script_pubkey: der_desc.script_pubkey(),
            value: bitcoin::Amount::MAX_MONEY,
        };
        let mut psbt_out = Default::default();
        der_desc.update_change_psbt_out(&mut psbt_out);
        psbt.unsigned_tx.output.push(txo);
        psbt.outputs.push(psbt_out);
        let indexes = desc.change_indexes(&psbt, secp);
        assert_eq!(indexes.len(), 2);
        assert!(matches!(
            indexes[0],
            ChangeOutput::ChangeAddress { index: 1 }
        ));
        assert!(matches!(
            indexes[1],
            ChangeOutput::DepositAddress { index: 2 }
        ));
    }

    #[test]
    fn change_detection() {
        let secp = secp256k1::Secp256k1::verification_only();

        // Check the change detection both under P2WSH and Taproot. We reuse descriptor from unit
        // tests above.
        let desc = LianaDescriptor::from_str("wsh(or_d(multi(3,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*),and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*)),older(26352))))").unwrap();
        run_change_detection(desc, &secp);
        let desc = LianaDescriptor::from_str("tr(tpubD6NzVbkrYhZ4YdBUPkUhDYj6Sd1QK8vgiCf5RwHnAnSNK5ozemAZzPTYZbgQq4diod7oxFJJYGa8FNRHzRo7URkixzQTuudh38xRRdSc4Hu/<0;1>/*,{and_v(v:multi_a(1,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<2;3>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<2;3>/*),older(2)),multi_a(2,[ffd63c8d/48'/1'/0'/2']tpubDExA3EC3iAsPxPhFn4j6gMiVup6V2eH3qKyk69RcTc9TTNRfFYVPad8bJD5FCHVQxyBT4izKsvr7Btd2R4xmQ1hZkvsqGBaeE82J71uTK4N/<0;1>/*,[da2ee873/48'/1'/0'/2']tpubDEbXY6RbN9mxAvQW797WxReGGkrdyRfdYcehVVaQQcQ3kyfhxSMcnU9qGpUVRHXXALvBtc99jcuxx5tkzcLaJbAukSNpP9h2ti4XFRosv1g/<0;1>/*)})").unwrap();
        run_change_detection(desc, &secp);
    }

    #[test]
    fn unliftable_miniscript() {
        LianaDescriptor::from_str("wsh(0)").unwrap_err();
    }

    #[test]
    fn descriptor_contains_fingerprint_in_primary_path_multi() {
        let descr = LianaDescriptor::from_str("wsh(or_d(multi(3,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*),and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*)),older(26352))))").unwrap();

        assert!(
            descr.contains_fingerprint_in_primary_path(Fingerprint::from_str("aabb0011").unwrap())
        );
        assert!(
            descr.contains_fingerprint_in_primary_path(Fingerprint::from_str("aabb0012").unwrap())
        );
        assert!(
            descr.contains_fingerprint_in_primary_path(Fingerprint::from_str("aabb0013").unwrap())
        );
        assert!(
            !descr.contains_fingerprint_in_primary_path(Fingerprint::from_str("aabb0014").unwrap())
        );
    }

    #[test]
    fn descriptor_contains_fingerprint_in_primary_path_single_key() {
        let descr = LianaDescriptor::from_str("wsh(or_d(pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*),and_v(v:thresh(1,pkh([bbcc2233/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*)),older(26352))))").unwrap();

        assert!(
            descr.contains_fingerprint_in_primary_path(Fingerprint::from_str("aabb0011").unwrap())
        );
        assert!(
            !descr.contains_fingerprint_in_primary_path(Fingerprint::from_str("bbcc2233").unwrap())
        );
        assert!(
            !descr.contains_fingerprint_in_primary_path(Fingerprint::from_str("ddeeff00").unwrap())
        );
    }

    #[test]
    fn descriptor_contains_fingerprint_in_recovery_path_multi() {
        let descr = LianaDescriptor::from_str("wsh(or_d(multi(3,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*),and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*)),older(26352))))").unwrap();

        assert!(descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("aabb0011").unwrap(),
            26352
        ));
        assert!(descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("aabb0012").unwrap(),
            26352
        ));
        assert!(descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("aabb0013").unwrap(),
            26352
        ));
        assert!(!descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("aabb0013").unwrap(),
            1000
        ));
        assert!(!descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("aabb0014").unwrap(),
            26352
        ));
    }

    #[test]
    fn descriptor_contains_fingerprint_in_recovery_path_single_key() {
        let descr = LianaDescriptor::from_str("wsh(or_d(pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*),and_v(v:thresh(1,pkh([bbcc2233/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*)),older(26352))))").unwrap();

        assert!(!descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("aabb0011").unwrap(),
            26352
        ));
        assert!(descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("bbcc2233").unwrap(),
            26352
        ));
        assert!(!descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("ddeeff00").unwrap(),
            26352
        ));
    }

    #[test]
    fn descriptor_contains_fingerprint_in_recovery_path_multiple_keys() {
        let descr = LianaDescriptor::from_str("wsh(or_d(c:or_i(and_v(v:older(38305),and_v(v:pkh([d6bba22a/84'/1'/0']tpubDCwMfgJWBGfJuZFmgTAP9qdSwJeC4fEKaYXQx7CNiTzsB5WrpVSmySdPFnKDu8ChWZweYNh7MoAoFsCNY7gTRFSGtDYbG9s6vAKNKzT1Hii/<0;1>/*),pk_h([e9e6c583/48'/1'/0'/2']tpubDEWn2LRKdyREaweKHxj7XzSjcxXGTVbFkL5Qi5AWsJzGvN28cKQwGqCND9TP6EPtPaE13eK9SnyuiQ4qsfy5UuGD3p32Ew36mWfKmYCJRcz/<0;1>/*))),pk_k([de8abde2/48'/1'/0'/2']tpubDES5ZQEwEuj7Fpe6d6wkwD8SdequEa2cqq57QHQ43pb1x2HxbLp6anHwutDNzrMhDAbx1YgxCFAbRi6EhWwQLaGMSSmxJRaAzCUgn6VwpVD/<0;1>/*)),and_v(v:thresh(1,pkh([d6bba22a/84'/1'/0']tpubDCwMfgJWBGfJuZFmgTAP9qdSwJeC4fEKaYXQx7CNiTzsB5WrpVSmySdPFnKDu8ChWZweYNh7MoAoFsCNY7gTRFSGtDYbG9s6vAKNKzT1Hii/<2;3>/*),a:pkh([e9e6c583/48'/1'/0'/2']tpubDEWn2LRKdyREaweKHxj7XzSjcxXGTVbFkL5Qi5AWsJzGvN28cKQwGqCND9TP6EPtPaE13eK9SnyuiQ4qsfy5UuGD3p32Ew36mWfKmYCJRcz/<2;3>/*)),older(52596))))").unwrap();

        assert!(descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("d6bba22a").unwrap(),
            38305
        ));
        assert!(descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("e9e6c583").unwrap(),
            38305
        ));
        assert!(descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("d6bba22a").unwrap(),
            52596
        ));
        assert!(descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("e9e6c583").unwrap(),
            52596
        ));

        assert!(!descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("d6bba22a").unwrap(),
            12345
        ));
        assert!(!descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("e9e6c583").unwrap(),
            12345
        ));

        assert!(!descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("ffffffff").unwrap(),
            38305
        ));
        assert!(!descr.contains_fingerprint_in_recovery_path(
            Fingerprint::from_str("ffffffff").unwrap(),
            52596
        ));
    }

    // TODO: test error conditions of deserialization.
}
