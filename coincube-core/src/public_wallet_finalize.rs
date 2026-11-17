//! Signature verification and finalization for PSBTs that spend from a public
//! (non-Cube) descriptor wallet: the imported Bitcoin wallet's ordinary spends.
//!
//! The caller supplies the exact unsigned construction and, for each input,
//! the derived descriptor it must pay. Every supplied partial signature is
//! verified, Miniscript finalizes, and an interpreter replay of each retained
//! scriptSig and witness counts the signatures actually used. Only implicit or
//! explicit ECDSA `SIGHASH_ALL` is accepted. Construction and economic rules
//! stay with the caller ([`crate::imported_spend`]).

use std::fmt;

use miniscript::{
    bitcoin::{
        self,
        hashes::Hash,
        psbt::Psbt,
        secp256k1,
        sighash::{EcdsaSighashType, Prevouts, SighashCache},
        ScriptBuf, Transaction, TxOut,
    },
    descriptor::DefiniteDescriptorKey,
    interpreter::{Interpreter, KeySigPair, SatisfiedConstraint},
    psbt::PsbtExt,
    Descriptor,
};

use crate::spend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizeError {
    /// The signed PSBT is not the exact construction plus signatures.
    ConstructionChanged,
    /// A sighash other than implicit or explicit `SIGHASH_ALL`.
    UnsupportedSighash,
    InputAuthentication,
    InvalidSignature {
        input: usize,
    },
    Economics,
    /// Not enough valid signatures to satisfy every input.
    Unsatisfied,
    InvalidWitness,
}
impl fmt::Display for FinalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Finalization refused: {self:?}")
    }
}
impl std::error::Error for FinalizeError {}

/// The signed PSBT must be the exact construction plus `partial_sigs`, with
/// every input request absent or `SIGHASH_ALL` and every signature byte
/// `SIGHASH_ALL`.
pub(crate) fn check_signed_construction(
    original: &Psbt,
    signed: &Psbt,
) -> Result<(), FinalizeError> {
    if signed.unsigned_tx != original.unsigned_tx
        || signed.inputs.len() != original.inputs.len()
        || signed.outputs.len() != original.outputs.len()
    {
        return Err(FinalizeError::ConstructionChanged);
    }
    let mut normalized = signed.clone();
    for (index, input) in signed.inputs.iter().enumerate() {
        if input
            .sighash_type
            .is_some_and(|s| s.ecdsa_hash_ty() != Ok(EcdsaSighashType::All))
            || input
                .partial_sigs
                .values()
                .any(|s| s.sighash_type != EcdsaSighashType::All)
        {
            return Err(FinalizeError::UnsupportedSighash);
        }
        normalized.inputs[index].partial_sigs.clear();
        normalized.inputs[index].sighash_type = original.inputs[index].sighash_type;
    }
    if normalized != *original {
        return Err(FinalizeError::ConstructionChanged);
    }
    Ok(())
}

/// Shared, shape-independent signature verification for public descriptor
/// sources. The caller supplies the expected derived descriptor for each
/// authenticated input and imposes its own construction and economic rules
/// around it.
pub(crate) fn verify_partial_signatures_for<C, F>(
    signed: &Psbt,
    secp: &secp256k1::Secp256k1<C>,
    mut derive: F,
) -> Result<Vec<TxOut>, FinalizeError>
where
    C: secp256k1::Verification,
    F: FnMut(usize, &bitcoin::psbt::Input, &TxOut) -> Option<Descriptor<DefiniteDescriptorKey>>,
{
    let mut prevouts = Vec::with_capacity(signed.inputs.len());
    let mut cache = SighashCache::new(&signed.unsigned_tx);
    for (index, input) in signed.inputs.iter().enumerate() {
        let output = spend::authenticate_previous_output(
            &signed.unsigned_tx.input[index].previous_output,
            input.non_witness_utxo.as_ref(),
            input.witness_utxo.as_ref(),
        )
        .map_err(|_| FinalizeError::InputAuthentication)?;
        let definite = derive(index, input, &output).ok_or(FinalizeError::InputAuthentication)?;
        if definite.script_pubkey() != output.script_pubkey {
            return Err(FinalizeError::InputAuthentication);
        }
        let digest = sighash_all(&mut cache, index, &definite, input, &output)
            .ok_or(FinalizeError::InvalidSignature { input: index })?;
        let message = secp256k1::Message::from_digest(digest);
        let segwit = is_segwit(&definite);
        for (key, signature) in &input.partial_sigs {
            if !input.bip32_derivation.contains_key(&key.inner)
                || (segwit && !key.compressed)
                || secp
                    .verify_ecdsa(&message, &signature.signature, &key.inner)
                    .is_err()
            {
                return Err(FinalizeError::InvalidSignature { input: index });
            }
        }
        prevouts.push(output);
    }
    Ok(prevouts)
}

/// Miniscript's finalizer and extractor, then an interpreter replay of every
/// retained witness. Returns the transaction and signatures per input.
pub(crate) fn finalize_and_replay<C: secp256k1::Verification>(
    original: &Psbt,
    signed: &Psbt,
    prevouts: &[TxOut],
    secp: &secp256k1::Secp256k1<C>,
) -> Result<(Transaction, Vec<usize>), FinalizeError> {
    let mut finalized = signed.clone();
    finalized
        .finalize_mut(secp)
        .map_err(|_| FinalizeError::Unsatisfied)?;
    // extract() also runs the library interpreter; no unchecked extraction.
    let transaction = finalized
        .extract(secp)
        .map_err(|_| FinalizeError::InvalidWitness)?;
    let signatures_per_input = verify_retained_witness(&transaction, original, prevouts, secp)?;
    Ok((transaction, signatures_per_input))
}

/// The `SIGHASH_ALL` digest for one input of a supported shape, with the
/// scriptCode chosen from the construction's own descriptor.
fn sighash_all(
    cache: &mut SighashCache<&Transaction>,
    index: usize,
    definite: &Descriptor<DefiniteDescriptorKey>,
    input: &bitcoin::psbt::Input,
    output: &TxOut,
) -> Option<[u8; 32]> {
    let all = EcdsaSighashType::All;
    match definite {
        Descriptor::Pkh(_) => cache
            .legacy_signature_hash(index, &output.script_pubkey, all.to_u32())
            .ok()
            .map(|hash| hash.to_byte_array()),
        Descriptor::Wpkh(_) => cache
            .p2wpkh_signature_hash(index, &output.script_pubkey, output.value, all)
            .ok()
            .map(|hash| hash.to_byte_array()),
        Descriptor::Sh(_) => {
            let redeem = input.redeem_script.as_ref()?;
            if !redeem.is_p2wpkh() || redeem.to_p2sh() != output.script_pubkey {
                return None;
            }
            cache
                .p2wpkh_signature_hash(index, redeem, output.value, all)
                .ok()
                .map(|hash| hash.to_byte_array())
        }
        Descriptor::Wsh(_) => {
            let script = input.witness_script.as_ref()?;
            if script.to_p2wsh() != output.script_pubkey {
                return None;
            }
            cache
                .p2wsh_signature_hash(index, script, output.value, all)
                .ok()
                .map(|hash| hash.to_byte_array())
        }
        Descriptor::Bare(_) | Descriptor::Tr(_) => None,
    }
}

/// Replay each finalized input through Miniscript's interpreter and count the
/// `SIGHASH_ALL` signatures by construction keys it actually used.
fn verify_retained_witness<C: secp256k1::Verification>(
    transaction: &Transaction,
    original: &Psbt,
    prevouts: &[TxOut],
    secp: &secp256k1::Secp256k1<C>,
) -> Result<Vec<usize>, FinalizeError> {
    let mut unsigned = transaction.clone();
    for input in &mut unsigned.input {
        input.script_sig = ScriptBuf::new();
        input.witness.clear();
    }
    if unsigned != original.unsigned_tx {
        return Err(FinalizeError::ConstructionChanged);
    }
    let mut counts = Vec::with_capacity(transaction.input.len());
    for (index, input) in transaction.input.iter().enumerate() {
        let signatures =
            replay_retained_input(index, input, transaction, original, prevouts, secp)?;
        counts.push(signatures);
    }
    Ok(counts)
}

fn replay_retained_input<C: secp256k1::Verification>(
    index: usize,
    input: &bitcoin::TxIn,
    transaction: &Transaction,
    original: &Psbt,
    prevouts: &[TxOut],
    secp: &secp256k1::Secp256k1<C>,
) -> Result<usize, FinalizeError> {
    let interpreter = Interpreter::from_txdata(
        &prevouts[index].script_pubkey,
        &input.script_sig,
        &input.witness,
        input.sequence,
        transaction.lock_time,
    )
    .map_err(|_| FinalizeError::InvalidWitness)?;
    let mut signatures = 0;
    for constraint in interpreter.iter(secp, transaction, index, &Prevouts::All(prevouts)) {
        let pair = match constraint.map_err(|_| FinalizeError::InvalidWitness)? {
            SatisfiedConstraint::PublicKey { key_sig }
            | SatisfiedConstraint::PublicKeyHash { key_sig, .. } => Some(key_sig),
            _ => None,
        };
        if let Some(pair) = pair {
            match pair {
                KeySigPair::Ecdsa(key, sig)
                    if sig.sighash_type == EcdsaSighashType::All
                        && original.inputs[index]
                            .bip32_derivation
                            .contains_key(&key.inner) =>
                {
                    signatures += 1
                }
                _ => return Err(FinalizeError::InvalidWitness),
            }
        }
    }
    if signatures == 0 {
        return Err(FinalizeError::InvalidWitness);
    }
    Ok(signatures)
}

/// Legacy P2PKH is the only supported shape without a witness.
fn is_segwit(descriptor: &Descriptor<DefiniteDescriptorKey>) -> bool {
    !matches!(descriptor, Descriptor::Pkh(_))
}
