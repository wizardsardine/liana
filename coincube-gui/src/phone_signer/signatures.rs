//! Validate the signature delta against the original request and local descriptor.
//! Response metadata and claimed key IDs confer no signing authority.
use super::pairing_store::SignerBinding;
use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::{
        bitcoin::{
            bip32::ChildNumber,
            hashes::Hash,
            psbt::{Input, Psbt},
            secp256k1::{Message, Secp256k1, XOnlyPublicKey},
            sighash::{EcdsaSighashType, Prevouts, SighashCache, TapSighashType},
            PublicKey,
        },
        descriptor::DescriptorPublicKey,
        ForEachKey,
    },
};
use std::{collections::BTreeSet, str::FromStr};

pub(super) fn merge_verified(
    original: &mut Psbt,
    returned: &Psbt,
    descriptor: &str,
    binding: &SignerBinding,
) -> Result<(), String> {
    let reject = || "Keychain returned an invalid or unauthorized signature delta.".to_owned();
    if original.unsigned_tx != returned.unsigned_tx
        || original.inputs.len() != returned.inputs.len()
        || original.inputs.len() != original.unsigned_tx.input.len()
    {
        return Err(reject());
    }
    let descriptor = CoincubeDescriptor::from_str(descriptor).map_err(|_| reject())?;
    let secp = Secp256k1::verification_only();
    // Only request UTXOs enter the sighash; response UTXO/derivation edits are ignored.
    let prevouts = original
        .inputs
        .iter()
        .zip(&original.unsigned_tx.input)
        .map(|(input, txin)| {
            let previous = if let Some(tx) = &input.non_witness_utxo {
                if tx.compute_txid() != txin.previous_output.txid {
                    return Err(reject());
                }
                Some(
                    tx.output
                        .get(txin.previous_output.vout as usize)
                        .ok_or_else(reject)?
                        .clone(),
                )
            } else {
                None
            };
            if let (Some(a), Some(b)) = (&previous, &input.witness_utxo) {
                if a != b {
                    return Err(reject());
                }
            }
            previous
                .or_else(|| input.witness_utxo.clone())
                .ok_or_else(reject)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut cache = SighashCache::new(&original.unsigned_tx);
    let mut additions = 0;
    for (i, (before, after)) in original.inputs.iter().zip(&returned.inputs).enumerate() {
        if before
            .partial_sigs
            .iter()
            .any(|(k, v)| after.partial_sigs.get(k) != Some(v))
            || before
                .tap_script_sigs
                .iter()
                .any(|(k, v)| after.tap_script_sigs.get(k) != Some(v))
            || (before.tap_key_sig.is_some() && before.tap_key_sig != after.tap_key_sig)
            || (before.final_script_sig.is_some()
                && before.final_script_sig != after.final_script_sig)
            || (before.final_script_witness.is_some()
                && before.final_script_witness != after.final_script_witness)
        {
            return Err(reject());
        }
        if before.partial_sigs == after.partial_sigs
            && before.tap_key_sig == after.tap_key_sig
            && before.tap_script_sigs == after.tap_script_sigs
        {
            continue;
        }

        // Origins suggest an index, never ownership. Re-derive both allowed branches
        // with the core policy parser and require the actual spent script to match.
        let indices: BTreeSet<ChildNumber> = before
            .bip32_derivation
            .values()
            .map(|(_, p)| p)
            .chain(before.tap_key_origins.values().map(|(_, (_, p))| p))
            .filter_map(|p| p.into_iter().last().copied())
            .filter(|i| i.is_normal())
            .collect();
        let mut resolved = None;
        for index in indices {
            for branch in [
                descriptor.receive_descriptor(),
                descriptor.change_descriptor(),
            ] {
                let derived = branch.derive(index, &secp);
                if derived.script_pubkey() != prevouts[i].script_pubkey {
                    continue;
                }
                let mut expected = Input::default();
                derived.update_psbt_in(&mut expected);
                let mut owned = BTreeSet::<PublicKey>::new();
                branch.as_descriptor_public_key().for_each_key(|key| {
                    if let DescriptorPublicKey::XPub(x) = key {
                        if x.xkey.to_string() == binding.xpub
                            && x.origin.as_ref().map(|o| o.0) == Some(binding.fingerprint)
                        {
                            if let Ok(pk) = key
                                .clone()
                                .at_derivation_index(index.into())
                                .and_then(|k| k.derive_public_key(&secp))
                            {
                                owned.insert(pk);
                            }
                        }
                    }
                    true
                });
                resolved = Some((expected, owned));
            }
        }
        let (expected, owned) = resolved.ok_or_else(reject)?;
        let owned_xonly: BTreeSet<_> = owned
            .iter()
            .map(|p| p.inner.x_only_public_key().0)
            .collect();
        for (pk, sig) in &after.partial_sigs {
            if before.partial_sigs.contains_key(pk) {
                continue;
            }
            if !owned.contains(pk)
                || !expected.bip32_derivation.contains_key(&pk.inner)
                || expected.bip32_derivation.get(&pk.inner)
                    != before.bip32_derivation.get(&pk.inner)
                || sig.sighash_type != EcdsaSighashType::All
                || before
                    .sighash_type
                    .is_some_and(|s| s != EcdsaSighashType::All.into())
            {
                return Err(reject());
            }
            let script = expected.witness_script.as_ref().ok_or_else(reject)?;
            let hash = cache
                .p2wsh_signature_hash(i, script, prevouts[i].value, sig.sighash_type)
                .map_err(|_| reject())?;
            secp.verify_ecdsa(
                &Message::from_digest(hash.to_byte_array()),
                &sig.signature,
                &pk.inner,
            )
            .map_err(|_| reject())?;
            additions += 1;
        }
        if let Some(sig) = after.tap_key_sig.filter(|_| before.tap_key_sig.is_none()) {
            let internal = expected.tap_internal_key.ok_or_else(reject)?;
            if !owned_xonly.contains(&internal)
                || expected.tap_key_origins.get(&internal).map(|v| &v.1)
                    != before.tap_key_origins.get(&internal).map(|v| &v.1)
                || sig.sighash_type != TapSighashType::Default
                || before
                    .sighash_type
                    .is_some_and(|s| s != TapSighashType::Default.into())
            {
                return Err(reject());
            }
            let output_key = XOnlyPublicKey::from_slice(&prevouts[i].script_pubkey.as_bytes()[2..])
                .map_err(|_| reject())?;
            let hash = cache
                .taproot_key_spend_signature_hash(i, &Prevouts::All(&prevouts), sig.sighash_type)
                .map_err(|_| reject())?;
            secp.verify_schnorr(
                &sig.signature,
                &Message::from_digest(hash.to_byte_array()),
                &output_key,
            )
            .map_err(|_| reject())?;
            additions += 1;
        }
        for ((pk, leaf), sig) in &after.tap_script_sigs {
            if before.tap_script_sigs.contains_key(&(*pk, *leaf)) {
                continue;
            }
            let (leaves, origin) = expected.tap_key_origins.get(pk).ok_or_else(reject)?;
            if !owned_xonly.contains(pk)
                || !leaves.contains(leaf)
                || before.tap_key_origins.get(pk).map(|v| &v.1) != Some(origin)
                || sig.sighash_type != TapSighashType::Default
                || before
                    .sighash_type
                    .is_some_and(|s| s != TapSighashType::Default.into())
            {
                return Err(reject());
            }
            let hash = cache
                .taproot_script_spend_signature_hash(
                    i,
                    &Prevouts::All(&prevouts),
                    *leaf,
                    sig.sighash_type,
                )
                .map_err(|_| reject())?;
            secp.verify_schnorr(
                &sig.signature,
                &Message::from_digest(hash.to_byte_array()),
                pk,
            )
            .map_err(|_| reject())?;
            additions += 1;
        }
    }
    if additions == 0 {
        return Err(reject());
    }
    // BDK may also finalize while retaining partial signatures. Never import
    // response final scripts/witnesses (or any other metadata). The desktop
    // finalizes from the individually validated signature maps instead.
    // Atomic merge: nothing is written until every addition has passed.
    for (before, after) in original.inputs.iter_mut().zip(&returned.inputs) {
        before.partial_sigs = after.partial_sigs.clone();
        before.tap_key_sig = after.tap_key_sig;
        before.tap_script_sigs = after.tap_script_sigs.clone();
    }
    Ok(())
}
