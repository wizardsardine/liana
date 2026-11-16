//! Validate the signature delta against the original request and local descriptor.
//! Response metadata and claimed key IDs confer no signing authority.
use super::pairing_store::SignerBinding;
use coincube_core::{
    descriptors::CoincubeDescriptor,
    imported_spend::verify_imported_signatures,
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
    wallet_descriptor::ImportedWalletDescriptor,
};
use std::{collections::BTreeSet, str::FromStr};

/// Authenticate every input's spent output through its complete previous
/// transaction (P2-A) and return those outputs in input order.
///
/// This is the fee-source rule the daemon enforces at spend creation and
/// broadcast, applied on the LAN rail: the amount a phone reviews and commits
/// to in its sighash must come from a transaction whose txid the outpoint
/// commits to, never from a `witness_utxo` the requester could have written.
/// The error names the first offending input so the user can act on it.
pub(super) fn authenticated_prevouts(
    psbt: &Psbt,
) -> Result<Vec<coincube_core::miniscript::bitcoin::TxOut>, String> {
    if psbt.inputs.len() != psbt.unsigned_tx.input.len() {
        return Err("The transaction's PSBT inputs do not match its inputs.".to_owned());
    }
    psbt.inputs
        .iter()
        .zip(&psbt.unsigned_tx.input)
        .enumerate()
        .map(|(i, (input, txin))| {
            coincube_core::spend::authenticate_previous_output(
                &txin.previous_output,
                input.non_witness_utxo.as_ref(),
                input.witness_utxo.as_ref(),
            )
            .map_err(|e| {
                format!(
                    "Input {} of this transaction cannot be verified ({}), so its amount \
                     cannot be shown to the phone for review. Recreate the spend in \
                     COINCUBE and try again.",
                    i, e
                )
            })
        })
        .collect()
}

pub(super) fn merge_verified(
    original: &mut Psbt,
    returned: &Psbt,
    descriptor: &str,
    binding: &SignerBinding,
) -> Result<(), String> {
    let reject =
        || "COINCUBE Mobile returned an invalid or unauthorized signature delta.".to_owned();
    if original.unsigned_tx != returned.unsigned_tx
        || original.inputs.len() != returned.inputs.len()
        || original.inputs.len() != original.unsigned_tx.input.len()
    {
        return Err(reject());
    }
    let descriptor = CoincubeDescriptor::from_str(descriptor).map_err(|_| reject())?;
    let secp = Secp256k1::verification_only();
    // Only request UTXOs enter the sighash; response UTXO/derivation edits are
    // ignored. Each prevout comes from the input's complete previous transaction
    // (P2-A): a `witness_utxo` alone is requester-supplied and never accepted.
    let prevouts = authenticated_prevouts(original).map_err(|_| reject())?;
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

/// Accept only the exact imported phone account's new ECDSA partials. BDK may
/// return final script fields as well; those fields have no authority and are
/// never copied. All request metadata stays on the original PSBT, and core
/// verifies every retained partial against that original before the atomic
/// assignment.
pub(super) fn merge_imported_verified(
    original: &mut Psbt,
    returned: &Psbt,
    wallet: &ImportedWalletDescriptor,
    binding: &SignerBinding,
) -> Result<(), String> {
    let reject = || {
        "COINCUBE Mobile returned an invalid or unauthorized imported signature delta.".to_owned()
    };
    let scope = binding.imported_scope.as_ref().ok_or_else(reject)?;
    let expected_wallet_id =
        crate::app::settings::WalletId::for_imported(wallet, scope.chain).map_err(|_| reject())?;
    if scope.chain != wallet.chain()
        || scope.wallet_id != expected_wallet_id
        || original.unsigned_tx != returned.unsigned_tx
        || original.inputs.len() != returned.inputs.len()
        || original.outputs.len() != returned.outputs.len()
    {
        return Err(reject());
    }
    let prevouts = authenticated_prevouts(original).map_err(|_| reject())?;
    let secp = Secp256k1::verification_only();
    let mut candidate = original.clone();
    let mut normalized = returned.clone();
    let mut additions = 0usize;
    for (i, (before, after)) in original.inputs.iter().zip(&returned.inputs).enumerate() {
        if before
            .partial_sigs
            .iter()
            .any(|(key, sig)| after.partial_sigs.get(key) != Some(sig))
            || after.tap_key_sig != before.tap_key_sig
            || after.tap_script_sigs != before.tap_script_sigs
            || after.tap_key_origins != before.tap_key_origins
            || after.sighash_type != before.sighash_type
        {
            return Err(reject());
        }
        // The sole permitted response changes are partial signatures and
        // disposable finalization fields. Reject changed prevouts, derivations,
        // scripts, output metadata, global maps and unsigned transactions.
        normalized.inputs[i].partial_sigs = before.partial_sigs.clone();
        normalized.inputs[i].final_script_sig = before.final_script_sig.clone();
        normalized.inputs[i].final_script_witness = before.final_script_witness.clone();
        for (key, signature) in &after.partial_sigs {
            if before.partial_sigs.contains_key(key) {
                continue;
            }
            if signature.sighash_type != EcdsaSighashType::All
                || before
                    .sighash_type
                    .is_some_and(|kind| kind != EcdsaSighashType::All.into())
            {
                return Err(reject());
            }
            let (fingerprint, path) = before.bip32_derivation.get(&key.inner).ok_or_else(reject)?;
            let account_len = scope.account_origin.len();
            if *fingerprint != binding.fingerprint
                || path.len() != account_len + 2
                || !path.as_ref().starts_with(scope.account_origin.as_ref())
            {
                return Err(reject());
            }
            let branch = match path[account_len] {
                ChildNumber::Normal { index: 0 } => wallet.receive_descriptor(),
                ChildNumber::Normal { index: 1 } => wallet.change_descriptor(),
                _ => return Err(reject()),
            };
            let ChildNumber::Normal { index } = path[account_len + 1] else {
                return Err(reject());
            };
            let derived = branch.at_derivation_index(index).map_err(|_| reject())?;
            if derived.script_pubkey() != prevouts[i].script_pubkey {
                return Err(reject());
            }
            let mut owned = false;
            branch.for_each_key(|descriptor_key| {
                if let DescriptorPublicKey::XPub(account) = descriptor_key {
                    if account.xkey.to_string() == binding.xpub
                        && account.origin.as_ref()
                            == Some(&(binding.fingerprint, scope.account_origin.clone()))
                        && descriptor_key
                            .clone()
                            .at_derivation_index(index)
                            .and_then(|derived_key| derived_key.derive_public_key(&secp))
                            .ok()
                            == Some(*key)
                    {
                        owned = true;
                    }
                }
                true
            });
            if !owned {
                return Err(reject());
            }
            candidate.inputs[i].partial_sigs.insert(*key, *signature);
            additions += 1;
        }
    }
    if additions == 0 || normalized != *original {
        return Err(reject());
    }
    // The core verifier takes the unsigned construction as its first PSBT.
    // The LAN request may already contain another cosigner's valid partial;
    // remove only those partials in the comparison baseline, then verify all
    // retained old and new signatures together in `candidate`.
    let mut construction = original.clone();
    for input in &mut construction.inputs {
        input.partial_sigs.clear();
    }
    verify_imported_signatures(wallet, &construction, &candidate, &secp).map_err(|_| reject())?;
    *original = candidate;
    Ok(())
}
