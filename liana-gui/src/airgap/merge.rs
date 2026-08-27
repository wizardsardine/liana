//! Folding a signer's answer back into Liana's own PSBT.
//!
//! A returned PSBT never replaces Liana's record: only signatures move across,
//! and every one of them is checked against the original transaction first. So
//! a signer cannot rewrite amounts, outputs or metadata on the way back.

use std::convert::TryFrom;

use bwk_qr::protocol::response::{SignatureEntry, Signed};
use liana::miniscript::{
    bitcoin::{
        ecdsa,
        hashes::Hash,
        psbt::{Psbt, PsbtSighashType},
        secp256k1::{self, Secp256k1},
        sighash::SighashCache,
        taproot::{self, TapLeafHash},
        PublicKey,
    },
    psbt::PsbtExt,
};

use crate::airgap::Error;

pub fn apply(original: &Psbt, signed: Signed) -> Result<Psbt, Error> {
    match signed {
        Signed::Psbt(bytes) => {
            let returned = Psbt::deserialize(&bytes)
                .map_err(|error| Error::InvalidPsbt(format!("unreadable PSBT: {error}")))?;
            merge_psbt(original, &returned)
        }
        Signed::Signatures(entries) => merge_signatures(original, &entries),
    }
}

/// Validate an air-gapped signing response and merge only signature fields into
/// Liana's canonical PSBT.
fn merge_psbt(original: &Psbt, returned: &Psbt) -> Result<Psbt, Error> {
    if original.unsigned_tx != returned.unsigned_tx
        || original.inputs.len() != returned.inputs.len()
        || original.outputs.len() != returned.outputs.len()
    {
        return Err(Error::InvalidPsbt(
            "returned transaction does not match".to_owned(),
        ));
    }

    let mut merged = original.clone();
    let mut added = 0usize;
    for (index, (canonical, signed)) in original
        .inputs
        .iter()
        .zip(returned.inputs.iter())
        .enumerate()
    {
        for (public_key, signature) in &canonical.partial_sigs {
            if signed.partial_sigs.get(public_key) != Some(signature) {
                return Err(dropped("signature", index));
            }
        }
        for (public_key, signature) in &signed.partial_sigs {
            if canonical.partial_sigs.contains_key(public_key) {
                continue;
            }
            add_ecdsa(&mut merged, original, index, public_key, signature)?;
            added += 1;
        }

        for (key, signature) in &canonical.tap_script_sigs {
            if signed.tap_script_sigs.get(key) != Some(signature) {
                return Err(dropped("Taproot script signature", index));
            }
        }
        for (key, signature) in &signed.tap_script_sigs {
            if canonical.tap_script_sigs.contains_key(key) {
                continue;
            }
            add_tap_script(&mut merged, original, index, key.0, key.1, signature)?;
            added += 1;
        }

        match (canonical.tap_key_sig, signed.tap_key_sig) {
            (Some(existing), Some(returned)) if existing == returned => {}
            (Some(_), _) => return Err(dropped("Taproot key-path signature", index)),
            (None, Some(signature)) => {
                add_tap_key(&mut merged, original, index, &signature)?;
                added += 1;
            }
            (None, None) => {}
        }
    }

    if added == 0 {
        return Err(Error::InvalidPsbt("signature was not added".to_owned()));
    }
    Ok(merged)
}

/// The signatures-only response form. The same checks apply: each signature must
/// name a key the input expects, and verify against the original transaction.
fn merge_signatures(original: &Psbt, entries: &[SignatureEntry]) -> Result<Psbt, Error> {
    if entries.is_empty() {
        return Err(Error::InvalidPsbt("signature was not added".to_owned()));
    }
    let mut merged = original.clone();
    for entry in entries {
        match entry {
            SignatureEntry::Ecdsa {
                input_index,
                public_key,
                signature,
            } => {
                let index = input(original, *input_index)?;
                let public_key = PublicKey::try_from(public_key).map_err(|error| {
                    Error::InvalidPsbt(format!("unusable public key on input {index}: {error}"))
                })?;
                let signature = ecdsa::Signature::from_slice(signature).map_err(|error| {
                    Error::InvalidPsbt(format!("unreadable signature on input {index}: {error}"))
                })?;
                add_ecdsa(&mut merged, original, index, &public_key, &signature)?;
            }
            SignatureEntry::TapKey {
                input_index,
                signature,
            } => {
                let index = input(original, *input_index)?;
                let signature = taproot_signature(signature, index)?;
                add_tap_key(&mut merged, original, index, &signature)?;
            }
            SignatureEntry::TapScript {
                input_index,
                xonly_public_key,
                tap_leaf_hash,
                signature,
            } => {
                let index = input(original, *input_index)?;
                let public_key =
                    secp256k1::XOnlyPublicKey::from_slice(xonly_public_key).map_err(|error| {
                        Error::InvalidPsbt(format!("unusable key on input {index}: {error}"))
                    })?;
                let leaf_hash = TapLeafHash::from_byte_array(*tap_leaf_hash);
                let signature = taproot_signature(signature, index)?;
                add_tap_script(
                    &mut merged,
                    original,
                    index,
                    public_key,
                    leaf_hash,
                    &signature,
                )?;
            }
        }
    }
    Ok(merged)
}

fn add_ecdsa(
    merged: &mut Psbt,
    original: &Psbt,
    index: usize,
    public_key: &PublicKey,
    signature: &ecdsa::Signature,
) -> Result<(), Error> {
    if !original.inputs[index]
        .bip32_derivation
        .contains_key(&public_key.inner)
    {
        return Err(unexpected("public key", index));
    }
    verify_ecdsa(original, index, public_key, signature)?;
    merged.inputs[index]
        .partial_sigs
        .insert(*public_key, *signature);
    Ok(())
}

fn add_tap_script(
    merged: &mut Psbt,
    original: &Psbt,
    index: usize,
    public_key: secp256k1::XOnlyPublicKey,
    leaf_hash: TapLeafHash,
    signature: &taproot::Signature,
) -> Result<(), Error> {
    let Some((leaf_hashes, _)) = original.inputs[index].tap_key_origins.get(&public_key) else {
        return Err(unexpected("public key", index));
    };
    if !leaf_hashes.contains(&leaf_hash) {
        return Err(unexpected("script leaf", index));
    }
    verify_taproot(original, index, public_key, Some(leaf_hash), signature)?;
    merged.inputs[index]
        .tap_script_sigs
        .insert((public_key, leaf_hash), *signature);
    Ok(())
}

fn add_tap_key(
    merged: &mut Psbt,
    original: &Psbt,
    index: usize,
    signature: &taproot::Signature,
) -> Result<(), Error> {
    let internal_key = original.inputs[index]
        .tap_internal_key
        .ok_or_else(|| unexpected("internal key", index))?;
    if !original.inputs[index]
        .tap_key_origins
        .contains_key(&internal_key)
    {
        return Err(unexpected("public key", index));
    }
    // verify against the key actually committed to in the output being spent,
    // rather than against what the PSBT claims the tree is
    let spent = original.spend_utxo(index).map_err(|error| {
        Error::InvalidPsbt(format!("missing input value at input {index}: {error}"))
    })?;
    let script = spent.script_pubkey.as_bytes();
    if !spent.script_pubkey.is_p2tr() || script.len() != 34 {
        return Err(Error::InvalidPsbt(format!(
            "Taproot key-path signature is not for a Taproot output on input {index}"
        )));
    }
    let output_key = secp256k1::XOnlyPublicKey::from_slice(&script[2..])
        .map_err(|_| Error::InvalidPsbt(format!("invalid Taproot output key on input {index}")))?;
    verify_taproot(original, index, output_key, None, signature)?;
    merged.inputs[index].tap_key_sig = Some(*signature);
    Ok(())
}

fn verify_ecdsa(
    psbt: &Psbt,
    index: usize,
    public_key: &PublicKey,
    signature: &ecdsa::Signature,
) -> Result<(), Error> {
    let psbt = with_sighash_type(psbt, index, signature.sighash_type.into(), |declared| {
        declared
            .ecdsa_hash_ty()
            .ok()
            .map(|declared| declared == signature.sighash_type)
    })?;
    let mut cache = SighashCache::new(&psbt.unsigned_tx);
    let message = psbt
        .sighash_msg(index, &mut cache, None)
        .map_err(|error| sighash_failed(index, error))?
        .to_secp_msg();
    Secp256k1::verification_only()
        .verify_ecdsa(&message, &signature.signature, &public_key.inner)
        .map_err(|_| invalid(index))
}

fn verify_taproot(
    psbt: &Psbt,
    index: usize,
    public_key: secp256k1::XOnlyPublicKey,
    leaf_hash: Option<TapLeafHash>,
    signature: &taproot::Signature,
) -> Result<(), Error> {
    let psbt = with_sighash_type(psbt, index, signature.sighash_type.into(), |declared| {
        declared
            .taproot_hash_ty()
            .ok()
            .map(|declared| declared == signature.sighash_type)
    })?;
    let mut cache = SighashCache::new(&psbt.unsigned_tx);
    let message = psbt
        .sighash_msg(index, &mut cache, leaf_hash)
        .map_err(|error| sighash_failed(index, error))?
        .to_secp_msg();
    Secp256k1::verification_only()
        .verify_schnorr(&signature.signature, &message, &public_key)
        .map_err(|_| invalid(index))
}

/// A signature is only verifiable under a sighash type. Take the input's own if
/// it declares one, and reject a signature that disagrees with it; otherwise use
/// the signature's, on a copy so the caller's PSBT is untouched.
fn with_sighash_type(
    psbt: &Psbt,
    index: usize,
    signature_type: PsbtSighashType,
    declared_matches: impl Fn(PsbtSighashType) -> Option<bool>,
) -> Result<Psbt, Error> {
    let mut psbt = psbt.clone();
    match psbt.inputs[index].sighash_type {
        Some(declared) => match declared_matches(declared) {
            Some(true) => {}
            Some(false) => {
                return Err(Error::InvalidPsbt(format!(
                    "signature sighash type does not match input {index}"
                )))
            }
            None => {
                return Err(Error::InvalidPsbt(format!(
                    "invalid sighash type on input {index}"
                )))
            }
        },
        None => psbt.inputs[index].sighash_type = Some(signature_type),
    }
    Ok(psbt)
}

fn taproot_signature(bytes: &[u8], index: usize) -> Result<taproot::Signature, Error> {
    taproot::Signature::from_slice(bytes).map_err(|error| {
        Error::InvalidPsbt(format!(
            "unreadable Taproot signature on input {index}: {error}"
        ))
    })
}

fn input(psbt: &Psbt, input_index: u32) -> Result<usize, Error> {
    let index = input_index as usize;
    if index >= psbt.inputs.len() {
        return Err(Error::InvalidPsbt(format!(
            "signature names input {index}, which the transaction does not have"
        )));
    }
    Ok(index)
}

fn dropped(what: &str, index: usize) -> Error {
    Error::InvalidPsbt(format!(
        "existing {what} disappeared or changed on input {index}"
    ))
}

fn unexpected(what: &str, index: usize) -> Error {
    Error::InvalidPsbt(format!(
        "signature uses an unexpected {what} on input {index}"
    ))
}

fn invalid(index: usize) -> Error {
    Error::InvalidPsbt(format!("invalid signature on input {index}"))
}

fn sighash_failed(index: usize, error: impl std::fmt::Display) -> Error {
    Error::InvalidPsbt(format!(
        "could not calculate the signature hash for input {index}: {error}"
    ))
}

#[cfg(test)]
mod tests {
    use liana::miniscript::bitcoin::base64::{prelude::BASE64_STANDARD, Engine};

    use super::*;

    fn psbt(base64: &str) -> Psbt {
        Psbt::deserialize(&BASE64_STANDARD.decode(base64.trim()).unwrap()).unwrap()
    }

    fn unsigned() -> Psbt {
        psbt(include_str!(
            "../../test_assets/airgap/taproot-unsigned.psbt.base64"
        ))
    }

    /// The signer's answer as a whole PSBT, the form it may fall back to.
    fn returned_psbt() -> Psbt {
        psbt(include_str!(
            "../../test_assets/airgap/taproot-key-path-signed.psbt.base64"
        ))
    }

    fn returned_signature() -> taproot::Signature {
        returned_psbt().inputs[0]
            .tap_key_sig
            .expect("the fixture carries a key-path signature")
    }

    /// Liana asks for signatures alone, so this is the form it normally gets.
    #[test]
    fn a_key_path_signature_merges() {
        let merged = apply(
            &unsigned(),
            Signed::Signatures(vec![SignatureEntry::TapKey {
                input_index: 0,
                signature: returned_signature().to_vec(),
            }]),
        )
        .expect("a valid key-path signature must merge");
        assert_eq!(merged.inputs[0].tap_key_sig, Some(returned_signature()));
    }

    /// A signer may ignore the preference and answer with the whole PSBT. Both
    /// forms carry the same signature, so both must land on the same result.
    #[test]
    fn both_answer_forms_agree() {
        let from_signatures = apply(
            &unsigned(),
            Signed::Signatures(vec![SignatureEntry::TapKey {
                input_index: 0,
                signature: returned_signature().to_vec(),
            }]),
        )
        .unwrap();
        let from_psbt = apply(&unsigned(), Signed::Psbt(returned_psbt().serialize())).unwrap();
        assert_eq!(from_signatures, from_psbt);
    }

    /// The signature is checked against the transaction, whichever form carried
    /// it, so a forged one is refused rather than merged.
    #[test]
    fn a_signature_that_does_not_verify_is_refused() {
        let mut forged = returned_signature().to_vec();
        forged[0] ^= 0xff;
        assert!(matches!(
            apply(
                &unsigned(),
                Signed::Signatures(vec![SignatureEntry::TapKey {
                    input_index: 0,
                    signature: forged,
                }]),
            ),
            Err(Error::InvalidPsbt(_))
        ));
    }
}
