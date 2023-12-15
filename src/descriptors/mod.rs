use miniscript::{
    bitcoin::{
        self, bip32,
        constants::WITNESS_SCALE_FACTOR,
        psbt::{Input as PsbtIn, Psbt},
        secp256k1,
    },
    descriptor, translate_hash_clone, ForEachKey, TranslatePk, Translator,
};

use std::{collections::BTreeMap, convert::TryInto, error, fmt, str};

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
        let desc = descriptor::Descriptor::<descriptor::DescriptorPublicKey>::from_str(s)
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
                xpub.xkey.network == expected_net
            } else {
                false
            }
        })
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
    pub fn max_sat_weight(&self) -> usize {
        // We add one to account for the witness stack size, as the `max_weight_to_satisfy` method
        // computes the difference in size for a satisfied input that was *already* in a
        // transaction that spent one or more Segwit coins (and thus already have 1 WU accounted
        // for the emtpy witness). But this method is used to account between a completely "nude"
        // transaction (and therefore no Segwit marker nor empty witness in inputs) and a satisfied
        // transaction.
        self.multi_desc
            .max_weight_to_satisfy()
            .expect("Always satisfiable")
            + 1
    }

    /// Get the maximum size difference of a transaction input spending a Script derived from this
    /// descriptor before and after satisfaction. The returned value is in (rounded up) virtual
    /// bytes.
    /// Callers are expected to account for the Segwit marker (2 WU). This takes into account the
    /// size of the witness stack length varint.
    pub fn max_sat_vbytes(&self) -> usize {
        self.max_sat_weight()
            .checked_add(WITNESS_SCALE_FACTOR - 1)
            .unwrap()
            .checked_div(WITNESS_SCALE_FACTOR)
            .unwrap()
    }

    /// Get the maximum size in virtual bytes of the whole input in a transaction spending
    /// a coin with this Script.
    pub fn spender_input_size(&self) -> usize {
        // txid + vout + nSequence + empty scriptSig + witness
        32 + 4 + 4 + 1 + self.max_sat_vbytes()
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
        // Get the identifier of all the keys that signed this transaction.
        let pubkeys_signed = psbt_in
            .partial_sigs
            .iter()
            .filter_map(|(pk, _)| psbt_in.bip32_derivation.get(&pk.inner));

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
    /// signatures are either absent or present for all inputs, ..)
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
                return Err(LianaDescError::InconsistentPsbt);
            }
        }

        Ok(spend_info)
    }

    /// Prune the BIP32 derivations in all the PSBT inputs for all the spending paths but the given
    /// one.
    pub fn prune_bip32_derivs(&self, mut psbt: Psbt, spending_path: &PathInfo) -> Psbt {
        // (Fingerprint, derivation path) pairs uniquely identify a key used in this spending path.
        let (_, path_origins) = spending_path.thresh_origins();

        // Go through all the PSBT inputs and drop the BIP32 derivations for keys that are not from
        // this spending path.
        for psbt_in in psbt.inputs.iter_mut() {
            psbt_in.bip32_derivation.retain(|_, (fg, der_path)| {
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
                        if der_paths.contains(&der_path_no_wildcard.into()) {
                            return true;
                        }
                    }
                }
                false
            });
        }

        psbt
    }

    /// Prune the BIP32 derivations in all the PSBT inputs for all the spending paths but the
    /// latest available one. For instance:
    /// - If there is two recovery paths, and the PSBT's first input nSequence isn't set to unlock
    /// any of them, prune all but the primary path's bip32 derivations.
    /// - If there is two recovery paths, and the PSBT's first input nSequence is set to unlock the
    /// first one, prune all but the first recovery path's bip32 derivations.
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

    /// Maximum possible size in vbytes of an unsigned transaction, `tx`,
    /// after satisfaction, assuming all inputs of `tx` are from this
    /// descriptor.
    pub fn unsigned_tx_max_vbytes(&self, tx: &bitcoin::Transaction) -> u64 {
        let witness_factor: u64 = WITNESS_SCALE_FACTOR.try_into().unwrap();
        let num_inputs: u64 = tx.input.len().try_into().unwrap();
        let max_sat_weight: u64 = self.max_sat_weight().try_into().unwrap();
        // Add weights together before converting to vbytes to avoid rounding up multiple times.
        let tx_wu = tx
            .weight()
            .to_wu()
            .checked_add(max_sat_weight.checked_mul(num_inputs).unwrap())
            .unwrap();
        tx_wu
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

    pub fn witness_script(&self) -> bitcoin::ScriptBuf {
        self.0.explicit_script().expect("Not a Taproot descriptor")
    }

    pub fn bip32_derivations(&self) -> Bip32Deriv {
        let ms = match self.0 {
            descriptor::Descriptor::Wsh(ref wsh) => match wsh.as_inner() {
                descriptor::WshInner::Ms(ms) => ms,
                descriptor::WshInner::SortedMulti(_) => {
                    unreachable!("None of our descriptors is a sorted multi")
                }
            },
            _ => unreachable!("All our descriptors are always P2WSH"),
        };

        // For DerivedPublicKey, Pk::Hash == Self.
        ms.iter_pk()
            .map(|k| (k.key.inner, (k.origin.0, k.origin.1)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bitcoin::Sequence;

    use std::str::FromStr;

    use crate::signer::HotSigner;

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
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap());
        let timelock = 52560;
        let policy = LianaPolicy::new(
            owner_key.clone(),
            [(timelock, heir_key.clone())].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "wsh(or_d(pk([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:pkh([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560))))#g7vk9r5l");

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
        let policy = LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "wsh(or_d(multi(3,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/0/<0;1>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/0/<0;1>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/0/<0;1>/*),and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/1/<0;1>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/1/<0;1>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/1/<0;1>/*)),older(26352))))#prj7nktq");

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
        let policy = LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "wsh(or_d(multi(3,[aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*,[aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*),and_v(v:thresh(2,pkh([aabb0011/48'/0'/0'/2']xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<2;3>/*),a:pkh([aabb0012/48'/0'/0'/2']xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<2;3>/*),a:pkh([aabb0013/48'/0'/0'/2']xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<2;3>/*)),older(26352))))#d2h994td");

        // We prevent footguns with timelocks by requiring a u16. Note how the following wouldn't
        // compile:
        //LianaPolicy::new(owner_key.clone(), heir_key.clone(), 0x00_01_0f_00).unwrap_err();
        //LianaPolicy::new(owner_key.clone(), heir_key.clone(), (1 << 31) + 1).unwrap_err();
        //LianaPolicy::new(owner_key, heir_key, (1 << 22) + 1).unwrap_err();

        // You can't use a null timelock in Miniscript.
        LianaPolicy::new(owner_key, [(0, heir_key)].iter().cloned().collect()).unwrap_err();

        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/<0;1>/*").unwrap());
        let timelock = 57600;
        let policy = LianaPolicy::new(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap();
        assert_eq!(LianaDescriptor::new(policy).to_string(), "wsh(or_d(pk([aabb0011/10/4893]xpub661MyMwAqRbcFG59fiikD8UV762quhruT8K8bdjqy6N2o3LG7yohoCdLg1m2HAY1W6rfBrtauHkBhbfA4AQ3iazaJj5wVPhwgaRCHBW2DBg/<0;1>/*),and_v(v:pkh([abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/24/32/<0;1>/*),older(57600))))#ak4cm093");

        // We can't pass a raw key, an xpub that is not deriveable, only hardened derivable,
        // without both the change and receive derivation paths, or with more than 2 different
        // derivation paths.
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/354").unwrap());
        LianaPolicy::new(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap_err();
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/<0;1>/*'").unwrap());
        LianaPolicy::new(
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
        LianaPolicy::new(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap_err();
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/0/*'").unwrap());
        LianaPolicy::new(
            owner_key.clone(),
            [(timelock, heir_key)].iter().cloned().collect(),
        )
        .unwrap_err();
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub661MyMwAqRbcFfxf71L4Dx4w5TmyNXrBicTEAM7vLzumxangwATWWgdJPb6xH1JHcJH9S3jNZx3fCnkkB1WyqrqGgavj1rehHcbythmruvZ/<0;1;2>/*'").unwrap());
        LianaPolicy::new(owner_key, [(timelock, heir_key)].iter().cloned().collect()).unwrap_err();

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
        LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap_err();

        // You can't pass duplicate keys, even if they are encoded differently.
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        LianaPolicy::new(owner_key, [(timelock, heir_key)].iter().cloned().collect()).unwrap_err();
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[00aabb44]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        LianaPolicy::new(owner_key, [(timelock, heir_key)].iter().cloned().collect()).unwrap_err();
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[00aabb44]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[11223344/2/98]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        LianaPolicy::new(owner_key, [(timelock, heir_key)].iter().cloned().collect()).unwrap_err();

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
        LianaPolicy::new(
            primary_keys,
            [(26352, recovery_keys)].iter().cloned().collect(),
        )
        .unwrap();

        // No origin in one of the keys
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*").unwrap());
        let timelock = 52560;
        LianaPolicy::new(owner_key, [(timelock, heir_key)].iter().cloned().collect()).unwrap_err();

        // One of the xpub isn't normalized.
        let owner_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*").unwrap());
        let heir_key = PathInfo::Single(descriptor::DescriptorPublicKey::from_str("[aabbccdd]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/42'/<0;1>/*").unwrap());
        let timelock = 52560;
        LianaPolicy::new(owner_key, [(timelock, heir_key)].iter().cloned().collect()).unwrap_err();

        // A 1-of-N multisig as primary path.
        LianaDescriptor::from_str("wsh(or_d(multi(1,[573fb35b/48'/1'/0'/2']tpubDFKp9T7WAYDcENSjoifkrpq1gMDF47KGJcJrpxzX23Qor8wuGbrEVs9utNq1MDS8E2WXJSBk1qoPQLpwyokW7DiUNPwFuxQkL7owNkLAb9W/<0;1>/*,[573fb35c/48'/1'/1'/2']tpubDFGezyzuHJPhdP3jHGW7v7Hwes4Hihqv5W2yyCmRY9VZJCRchETvxrMC8uECeJZdxQ14V4iD4DecoArkUSDwj8ogYE9WEv4MNZr12thNHCs/<0;1>/*),and_v(v:multi(2,[573fb35b/48'/1'/2'/2']tpubDDwxQauiaU964vPzt5Vd7jnDHEUtp2Vc34PaWpEXg5TQ3bRccxnc1MKKh88Hi7xiMeZo9Tm6fBcq4UGXqnDtGUniJLjqAD8SjQ8Eci3aSR7/<0;1>/*,[573fb35c/48'/1'/3'/2']tpubDE37XAVB5CQ1x85md3BQ5uHCoMwT5fgT8X13zzCUQ3x5o2jskYxKjj7Qcxt1Jpj4QB8tqspn2dooPCekRuQDYrDHov7J1ueUNu2wcvgRDxr/<0;1>/*),older(1000))))#fccaqlhh").unwrap();
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
        der_desc.witness_script();
        assert!(!der_desc.bip32_derivations().is_empty());
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
        assert_eq!(desc.max_sat_vbytes(), (1 + 66 + 1 + 34 + 73 + 3) / 4); // See the stack details below.

        // Maximum input size is (txid + vout + scriptsig + nSequence + max_sat).
        // Where max_sat is:
        // - Push the witness stack size
        // - Push the script
        // - Push an empty vector for using the recovery path
        // - Push the recovery key
        // - Push a signature for the recovery key
        // NOTE: The specific value is asserted because this was tested against a regtest
        // transaction.
        let stack = vec![vec![0; 65], vec![0; 0], vec![0; 33], vec![0; 72]];
        let witness_size = bitcoin::VarInt(stack.len() as u64).len()
            + stack
                .iter()
                .map(|item| bitcoin::VarInt(item.len() as u64).len() + item.len())
                .sum::<usize>();
        assert_eq!(
            desc.spender_input_size(),
            32 + 4 + 1 + 4 + wu_to_vb(witness_size),
        );
    }

    #[test]
    fn liana_desc_keys() {
        let secp = secp256k1::Secp256k1::signing_only();
        let prim_path = PathInfo::Single(random_desc_key(&secp));
        let twenty_eight_keys: Vec<descriptor::DescriptorPublicKey> =
            (0..28).map(|_| random_desc_key(&secp)).collect();
        let mut twenty_nine_keys = twenty_eight_keys.clone();
        twenty_nine_keys.push(random_desc_key(&secp));

        LianaPolicy::new(
            prim_path.clone(),
            [(1, PathInfo::Multi(2, vec![random_desc_key(&secp)]))]
                .iter()
                .cloned()
                .collect(),
        )
        .unwrap_err();
        LianaPolicy::new(
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
        LianaPolicy::new(
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
        LianaPolicy::new(
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
        LianaPolicy::new(
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
        LianaPolicy::new(
            prim_path.clone(),
            [(1, PathInfo::Multi(3, twenty_eight_keys.clone()))]
                .iter()
                .cloned()
                .collect(),
        )
        .unwrap();
        LianaPolicy::new(
            prim_path.clone(),
            [(1, PathInfo::Multi(20, twenty_eight_keys))]
                .iter()
                .cloned()
                .collect(),
        )
        .unwrap();
        LianaPolicy::new(
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

    #[test]
    fn roundtrip_descriptor() {
        // A descriptor with single keys in both primary and recovery paths
        roundtrip("wsh(or_d(pk([aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:pkh([aabbccdd]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560))))#7437yjrs");
        // One with a multisig in both paths
        roundtrip("wsh(or_d(multi(3,[aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0011/10/4893]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*,[aabb0022]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*),and_v(v:multi(2,[aabbccdd]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*,[aabb0011]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*,[aabb0022/10/4893]xpub6AyxexvxizZJffF153evmfqHcE9MV88fCNCAtP3jQjXJHwrAKri71Tq9jWUkPxj9pja4u6AkCPHY7atgxzSEa2HtDwJfrRWKK4fsfQg4o77/<0;1>/*),older(26352))))#csjdk94l");
        // A single key as primary path, a multisig as recovery
        roundtrip("wsh(or_d(pk([aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*),and_v(v:multi(2,[aabbccdd]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*,[aabb0011]xpub6AA2N8RALRYgLD6jT1iXYCEDkndTeZndMtWPbtNX6sY5dPiLtf2T88ahdxrGXMUPoNadgR86sFhBXWQVgifPzDYbY9ZtwK4gqzx4y5Da1DW/<0;1>/*,[aabb0022/10/4893]xpub6AyxexvxizZJffF153evmfqHcE9MV88fCNCAtP3jQjXJHwrAKri71Tq9jWUkPxj9pja4u6AkCPHY7atgxzSEa2HtDwJfrRWKK4fsfQg4o77/<0;1>/*),older(26352))))#sc9gw0z0");
        // The other way around
        roundtrip("wsh(or_d(multi(3,[aabbccdd]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,[aabb0011/10/4893]xpub6Bw79HbNSeS2xXw1sngPE3ehnk1U3iSPCgLYzC9LpN8m9nDuaKLZvkg8QXxL5pDmEmQtYscmUD8B9MkAAZbh6vxPzNXMaLfGQ9Sb3z85qhR/<0;1>/*,[aabb0022]xpub67zuTXF9Ln4731avKTBSawoVVNRuMfmRvkL7kLUaLBRqma9ZqdHBJg9qx8cPUm3oNQMiXT4TmGovXNoQPuwg17RFcVJ8YrnbcooN7pxVJqC/<0;1>/*),and_v(v:pk([aabbccdd]xpub69cP4Y7S9TWcbSNxmk6CEDBsoaqr3ZEdjHuZcHxEFFKGh569RsJNr2V27XGhsbH9FXgWUEmKXRN7c5wQfq2VPjt31xP9VsYnVUyU8HcVevm/<0;1>/*),older(26352))))#kjajav3j");
    }

    fn psbt_from_str(psbt_str: &str) -> Psbt {
        Psbt::from_str(psbt_str).unwrap()
    }

    #[test]
    fn partial_spend_info() {
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
        let psbt: Psbt = psbt_from_str("cHNidP8BAP0fAQIAAAAGAGo6V8K5MtKcQ8vRFedf5oJiOREiH4JJcEniyRv2800BAAAAAP3///9e3dVLjWKPAGwDeuUOmKFzOYEP5Ipu4LWdOPA+lITrRgAAAAAA/f///7cl9oeu9ssBXKnkWMCUnlgZPXhb+qQO2+OPeLEsbdGkAQAAAAD9////idkxRErbs34vsHUZ7QCYaiVaAFDV9gxNvvtwQLozwHsAAAAAAP3///9EakyJhd2PjwYh1I7zT2cmcTFI5g1nBd3srLeL7wKEewIAAAAA/f///7BcaP77nMaA2NjT/hyI6zueB/2jU/jK4oxmSqMaFkAzAQAAAAD9////AUAfAAAAAAAAFgAUqo7zdMr638p2kC3bXPYcYLv9nYUAAAAAAAEA/X4BAgAAAAABApEoe5xCmSi8hNTtIFwsy46aj3hlcLrtFrug39v5wy+EAQAAAGpHMEQCIDeI8JTWCTyX6opCCJBhWc4FytH8g6fxDaH+Wa/QqUoMAiAgbITpz8TBhwxhv/W4xEXzehZpOjOTjKnPw36GIy6SHAEhA6QnYCHUbU045FVh6ZwRwYTVineqRrB9tbqagxjaaBKh/v///+v1seDE9gGsZiWwewQs3TKuh0KSBIHiEtG8ABbz2DpAAQAAAAD+////Aqhaex4AAAAAFgAUkcVOEjVMct0jyCzhZN6zBT+lvTQvIAAAAAAAACIAIKKDUd/GWjAnwU99llS9TAK2dK80/nSRNLjmrhj0odUEAAJHMEQCICSn+boh4ItAa3/b4gRUpdfblKdcWtMLKZrgSEFFrC+zAiBtXCx/Dq0NutLSu1qmzFF1lpwSCB3w3MAxp5W90z7b/QEhA51S2ERUi0bg+l+bnJMJeAfDknaetMTagfQR9+AOrVKlxdMkAAEBKy8gAAAAAAAAIgAgooNR38ZaMCfBT32WVL1MArZ0rzT+dJE0uOauGPSh1QQiAgN+zbSfdr8oJBtlKomnQTHynF2b/UhovAwf0eS8awRSqUgwRQIhAJhm6xQvxt2LY+eNZqjhsgMOAxD0OPYty6nf9WaQZtgkAiBf/AXkeyq6ALknO9TZwY6ZRa0evY+DQ3j3XaqiBiAMfgEBBUEhA37NtJ92vygkG2UqiadBMfKcXZv9SGi8DB/R5LxrBFKprHNkdqkUxttmGj2sqzzaxSaacJTnJPDCbY6IrVqyaCIGAv9qeBDEB+5kvM/sZ8jQ7QApfZcDrqtq5OAe2gQ1V+pmDIpk8qkAAAAA0AAAACIGA37NtJ92vygkG2UqiadBMfKcXZv9SGi8DB/R5LxrBFKpDPWswv0AAAAA0AAAAAABAOoCAAAAAAEB0OPoVJs9ihvnAwjO16k/wGJuEus1IEE1Yo2KBjC2NSEAAAAAAP7///8C6AMAAAAAAAAiACBfeUS9jQv6O1a96Aw/mPV6gHxHl3mfj+f0frfAs2sMpP1QGgAAAAAAFgAUDS4UAIpdm1RlFYmg0OoCxW0yBT4CRzBEAiAPvbNlnhiUxLNshxN83AuK/lGWwlpXOvmcqoxsMLzIKwIgWwATJuYPf9buLe9z5SnXVnPVL0q6UZaWE5mjCvEl1RUBIQI54LFZmq9Lw0pxKpEGeqI74NnIfQmLMDcv5ySplUS1/wDMJAABASvoAwAAAAAAACIAIF95RL2NC/o7Vr3oDD+Y9XqAfEeXeZ+P5/R+t8CzawykIgICYn4eZbb6KGoxB1PEv/XPiujZFDhfoi/rJPtfHPVML2lHMEQCIDOHEqKdBozXIPLVgtBj3eWC1MeIxcKYDADe4zw0DbcMAiAq4+dbkTNCAjyCxJi0TKz5DWrPulxrqOdjMRHWngXHsQEBBUEhAmJ+HmW2+ihqMQdTxL/1z4ro2RQ4X6Iv6yT7Xxz1TC9prHNkdqkUzc/gCLoe6rQw63CGXhIR3YRz1qCIrVqyaCIGAmJ+HmW2+ihqMQdTxL/1z4ro2RQ4X6Iv6yT7Xxz1TC9pDPWswv0AAAAAqgAAACIGA8JCTIzdSoTJhiKN1pn+NnlkyuKOndiTgH2NIX+yNsYqDIpk8qkAAAAAqgAAAAABAOoCAAAAAAEBRGpMiYXdj48GIdSO809nJnExSOYNZwXd7Ky3i+8ChHsAAAAAAP7///8COMMQAAAAAAAWABQ5rnyuG5T8iuhqfaGAmpzlybo3t+gDAAAAAAAAIgAg7Kz3CX1RBjIvbK9LBYztmi7F1XIxQpX6mtCUkflvvl8CRzBEAiBaYx4sOHckEZwDnSrbb1ivc6seX4Puasm1PBGnBWgSTQIgCeUiXvd90ajI3F4/BHifLUI4fVIgVQFCqLTbbeXQD5oBIQOmGm+gTRx1slzF+wn8NhZoR1xfSYgoKX6bpRSVRjLcEXrOJAABASvoAwAAAAAAACIAIOys9wl9UQYyL2yvSwWM7ZouxdVyMUKV+prQlJH5b75fIgID0X2UJhC5+2jgJqUrihxZxDZHK7jgPFlrUYzoSHQTmP9HMEQCIEM4K8lVACvE2oSMZHDJiOeD81qsYgAvgpRgcSYgKc3AAiAQjdDr2COBea69W+2iVbnODuH3QwacgShW3dS4yeggJAEBBUEhA9F9lCYQufto4CalK4ocWcQ2Ryu44DxZa1GM6Eh0E5j/rHNkdqkU0DTexcgOQQ+BFjgS031OTxcWiH2IrVqyaCIGA9F9lCYQufto4CalK4ocWcQ2Ryu44DxZa1GM6Eh0E5j/DPWswv0AAAAAvwAAACIGA/xg4Uvem3JHVPpyTLP5JWiUH/yk3Y/uUI6JkZasCmHhDIpk8qkAAAAAvwAAAAABAOoCAAAAAAEBmG+mPq0O6QSWEMctsMjvv5LzWHGoT8wsA9Oa05kxIxsBAAAAAP7///8C6AMAAAAAAAAiACDUvIILFr0OxybADV3fB7ms7+ufnFZgicHR0nbI+LFCw1UoGwAAAAAAFgAUC+1ZjCC1lmMcvJ/4JkevqoZF4igCRzBEAiA3d8o96CNgNWHUkaINWHTvAUinjUINvXq0KBeWcsSWuwIgKfzRNWFR2LDbnB/fMBsBY/ylVXcSYwLs8YC+kmko1zIBIQOpEfsLv0htuertA1sgzCwGvHB0vE4zFO69wWEoHClKmAfMJAABASvoAwAAAAAAACIAINS8ggsWvQ7HJsANXd8Huazv65+cVmCJwdHSdsj4sULDIgID96jZc0sCi0IIXf2CpfE7tY+9LRmMsOdSTTHelFxfCwJHMEQCIHlaiMMznx8Cag8Y3X2gXi9Qtg0ZuyHEC6DsOzipSGOKAiAV2eC+S3Mbq6ig5QtRvTBsq5M3hCBdEJQlOrLVhWWt6AEBBUEhA/eo2XNLAotCCF39gqXxO7WPvS0ZjLDnUk0x3pRcXwsCrHNkdqkUyJ+Cbx7vYVY665yjJnMNODyYrAuIrVqyaCIGAt8UyDXk+mW3Y6IZNIBuDJHkdOaZi/UEShkN5L3GiHR5DIpk8qkAAAAAuAAAACIGA/eo2XNLAotCCF39gqXxO7WPvS0ZjLDnUk0x3pRcXwsCDPWswv0AAAAAuAAAAAABAP0JAQIAAAAAAQG7Zoy4I3J9x+OybAlIhxVKcYRuPFrkDFJfxMiC3kIqIAEAAAAA/v///wO5xxAAAAAAABYAFHgBzs9wJNVk6YwR81IMKmckTmC56AMAAAAAAAAWABTQ/LmJix5JoHBOr8LcgEChXHdLROgDAAAAAAAAIgAg7Kz3CX1RBjIvbK9LBYztmi7F1XIxQpX6mtCUkflvvl8CRzBEAiA+sIKnWVE3SmngjUgJdu1K2teW6eqeolfGe0d11b+irAIgL20zSabXaFRNM8dqVlcFsfNJ0exukzvxEOKl/OcF8VsBIQJrUspHq45AMSwbm24//2a9JM8XHFWbOKpyV+gNCtW71nrOJAABASvoAwAAAAAAACIAIOys9wl9UQYyL2yvSwWM7ZouxdVyMUKV+prQlJH5b75fIgID0X2UJhC5+2jgJqUrihxZxDZHK7jgPFlrUYzoSHQTmP9IMEUCIQCmDhJ9fyhlQwPruoOUemDuldtRu3ZkiTM3DA0OhkguSQIgYerNaYdP43DcqI5tnnL3n4jEeMHFCs+TBkOd6hDnqAkBAQVBIQPRfZQmELn7aOAmpSuKHFnENkcruOA8WWtRjOhIdBOY/6xzZHapFNA03sXIDkEPgRY4EtN9Tk8XFoh9iK1asmgiBgPRfZQmELn7aOAmpSuKHFnENkcruOA8WWtRjOhIdBOY/wz1rML9AAAAAL8AAAAiBgP8YOFL3ptyR1T6ckyz+SVolB/8pN2P7lCOiZGWrAph4QyKZPKpAAAAAL8AAAAAAQDqAgAAAAABAT6/vc6qBRzhQyjVtkC25NS2BvGyl2XjjEsw3e8vAesjAAAAAAD+////AgPBAO4HAAAAFgAUEwiWd/qI1ergMUw0F1+qLys5G/foAwAAAAAAACIAIOOPEiwmp2ZXR7ciyrveITXw0tn6zbQUA1Eikd9QlHRhAkcwRAIgJMZdO5A5u2UIMrAOgrR4NcxfNgZI6OfY7GKlZP0O8yUCIDFujbBRnamLEbf0887qidnXo6UgQA9IwTx6Zomd4RvJASEDoNmR2/XcqSyCWrE1tjGJ1oLWlKt4zsFekK9oyB4Hl0HF0yQAAQEr6AMAAAAAAAAiACDjjxIsJqdmV0e3Isq73iE18NLZ+s20FANRIpHfUJR0YSICAo3uyJxKHR9Z8fwvU7cywQCnZyPvtMl3nv54wPW1GSGqSDBFAiEAlLY98zqEL/xTUvm9ZKy5kBa4UWfr4Ryu6BmSZjseXPQCIGy7efKbZLQSDq8RhgNNjl1384gWFTN7nPwWV//SGriyAQEFQSECje7InEodH1nx/C9TtzLBAKdnI++0yXee/njA9bUZIaqsc2R2qRQhPRlaLsh/M/K/9fvbjxF/M20cNoitWrJoIgYCF7Rj5jFhe5L6VDzP5m2BeaG0mA9e7+6fMeWkWxLwpbAMimTyqQAAAADNAAAAIgYCje7InEodH1nx/C9TtzLBAKdnI++0yXee/njA9bUZIaoM9azC/QAAAADNAAAAAAA=");
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

    #[test]
    fn bip32_derivs_pruning() {
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
    }

    // TODO: test error conditions of deserialization.
}
