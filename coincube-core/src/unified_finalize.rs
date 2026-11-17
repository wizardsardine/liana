//! Finalisation of native P2WSH Vault spends that carry Bitcoin Blake2b unified
//! signatures (option (b) of the desktop plan's PR 6: a core finaliser that
//! assembles each input's witness from the miniscript satisfaction *after*
//! cryptographic verification, with no node involved).
//!
//! Why not the node's `finalizepsbt` (option (a)): it needs a local Bitcoin
//! Blake2b node, and the approved default backend for a BTCB2 Cube is Connect
//! Esplora with no local node (`#276` correction 4). rust-miniscript's own
//! finaliser is not an option either: it reads `PSBT_IN_PARTIAL_SIG`, whose
//! typed decoder rejects the `0x21` sighash byte, and its interpreter cannot
//! check a unified digest. So the witness is assembled here from the same
//! miniscript satisfaction logic, fed only with signatures this module has
//! verified itself.
//!
//! What a witness may contain, per key: a **unified** signature (`0x21`,
//! verified by [`verify_p2wsh_all_unified`]) or, failing that, a **legacy**
//! `SIGHASH_ALL` signature from `partial_sigs` (verified here against the
//! BIP-143 digest). Every signature the PSBT carries is verified before any
//! witness is chosen — a legacy record that would not be used still has to be
//! valid, or the whole call is refused: a PSBT that lies anywhere is not
//! finalised from the parts that happen to be true. A key that has a verified unified signature never
//! contributes its legacy one, an input is first satisfied from unified
//! signatures alone, and legacy signatures are offered only when that is
//! impossible. Offering them is not enough on its own: miniscript picks the
//! cheapest satisfaction, all ECDSA signatures weigh the same, and a `multi`
//! takes keys in script order — so with more signatures than the threshold
//! needs it can build an all-legacy witness for an input that had a unified
//! signature on a later key. [`satisfy_preferring_unified`] catches that case
//! and searches the legacy subsets for a satisfaction that keeps a unified
//! signature; the witness only falls back to legacy-only when **no** offered
//! subset lets the script use one (for instance a unified signature on a
//! recovery key whose timelock this transaction does not enable). That is the
//! "never drop a verified unified witness in favour of a legacy one" rule;
//! when the search cannot be run to completion the input is refused, not
//! degraded. Every other sighash type, `ANYONECANPAY` included, is refused
//! rather than warned about.
//!
//! Sighash *requests* (`PSBT_IN_SIGHASH_TYPE`) follow the same posture. The
//! adapter accepts an absent request, `SIGHASH_ALL` or `ALL|UNIFIED` and
//! refuses anything else at every boundary; on an input that carries a
//! unified record the verifier is stricter and accepts only absent or
//! `ALL|UNIFIED` — a `SIGHASH_ALL` request next to a unified signature is
//! refused, not reconciled. That strictness cannot strand a Coincube spend:
//! nothing in spend creation or signature merging writes the request field,
//! and the only writer, the unified signer, writes `0x21`; the case is
//! reachable only through an imported PSBT that pre-set `SIGHASH_ALL` and
//! then acquired a unified signature.
//!
//! The result reports, per input, how many unified and legacy signatures ended
//! up in the witness. That is the *only* basis a caller may use for a replay
//! statement (`#276` correction 1): an input whose final witness holds at least
//! one verified unified signature cannot be replayed on Bitcoin, because that
//! signature is invalid there; an input satisfied by legacy signatures alone is
//! replayable, whatever the PSBT claimed. Nothing here asserts chain inclusion,
//! finality, poison evidence or policy authorisation. It also does not assert
//! that no *other* witness exists: leftover or externally retained
//! `SIGHASH_ALL` signatures that independently meet the script threshold can
//! still be assembled by a standard PSBT consumer that never sees the
//! proprietary unified records (`#398`).
//!
//! [`ensure_no_unsafe_legacy_alternative`] is the local retention/export
//! policy layered on that fact (`#536`). It refuses an input when a
//! unified-bearing satisfaction is available and the verified legacy records
//! retained in the same PSBT independently satisfy the script. It preserves
//! legacy signatures required by a mixed witness and cannot revoke signatures
//! held in a separate device, file, or coordinator.

use std::{collections::BTreeMap, error, fmt};

use miniscript::{
    bitcoin::{
        absolute, ecdsa,
        hashes::{hash160, Hash},
        relative, secp256k1,
        sighash::{EcdsaSighashType, SighashCache},
        transaction, Amount, PublicKey, ScriptBuf, Sequence, Transaction, TxOut, Witness,
    },
    ExtParams, Miniscript, Satisfier, Segwitv0,
};

use crate::{
    psbt_unified::{unified_signatures, UnifiedPsbt, UnifiedPsbtError, UnifiedSignature},
    spend::{authenticate_previous_output, InputAuthError},
    unified_signing::{verify_p2wsh_all_unified, UnifiedSigningError},
};

/// `SIGHASH_ALL | SIGHASH_UNIFIED`, the only sighash a unified record carries.
const UNIFIED_SIGHASH_ALL: u8 = 0x21;

/// How an input's final witness was built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputWitnessReport {
    /// Verified unified (`0x21`) signatures placed in the witness.
    pub unified_used: usize,
    /// Verified legacy (`SIGHASH_ALL`) signatures placed in the witness.
    pub legacy_used: usize,
}

impl InputWitnessReport {
    /// Whether this input's witness is invalid on Bitcoin: true iff at least one
    /// verified unified signature is part of it. This is a statement about the
    /// bytes in the witness, not about intent or capability.
    pub fn replay_protected(&self) -> bool {
        self.unified_used > 0
    }
}

/// A fully signed transaction and, per input, what its witness is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedSpend {
    pub transaction: Transaction,
    pub inputs: Vec<InputWitnessReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifiedFinalizeError {
    /// The PSBT or one of its unified signatures failed verification.
    Signing(UnifiedSigningError),
    Adapter(UnifiedPsbtError),
    InputAuthentication {
        input: usize,
        reason: InputAuthError,
    },
    MissingWitnessScript {
        input: usize,
    },
    UnsupportedPrevoutScript {
        input: usize,
    },
    WitnessScriptCommitmentMismatch {
        input: usize,
    },
    UnsupportedWitnessScript {
        input: usize,
        reason: String,
    },
    /// A legacy partial signature with a sighash other than `SIGHASH_ALL`
    /// (`ANYONECANPAY` and friends). Refused, never assembled.
    UnsupportedLegacySighash {
        input: usize,
        public_key: PublicKey,
        sighash: u32,
    },
    /// A legacy partial signature that does not verify against the BIP-143
    /// digest of this transaction.
    InvalidLegacySignature {
        input: usize,
        public_key: PublicKey,
    },
    /// Not enough verified signatures (unified plus, where allowed, legacy) to
    /// satisfy the input's script.
    Unsatisfiable {
        input: usize,
        reason: String,
    },
    /// Internal invariant: the assembled witness contained a signature this
    /// module did not place. Never expected; refused rather than broadcast.
    UnexpectedWitnessElement {
        input: usize,
    },
    /// The input has a verified unified signature, the offered legacy
    /// signatures alone would satisfy it, and there were too many legacy
    /// candidates to search every subset for a satisfaction that keeps a
    /// unified one. Refused rather than finalised from legacy signatures
    /// alone — the caller can drop surplus legacy signatures and retry.
    RefusedToDropUnified {
        input: usize,
        legacy_candidates: usize,
    },
    /// A unified-bearing witness is available for this input, but the verified
    /// legacy signatures retained in the same PSBT can independently satisfy
    /// the script. A standard Bitcoin PSBT consumer can therefore assemble a
    /// different, Bitcoin-valid witness for the same unsigned transaction.
    UnsafeLegacyAlternative {
        input: usize,
        legacy_signatures: usize,
    },
}

impl fmt::Display for UnifiedFinalizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Signing(err) => write!(f, "unified signature verification failed: {err}"),
            Self::Adapter(err) => write!(f, "invalid unified PSBT: {err}"),
            Self::InputAuthentication { input, reason } => {
                write!(
                    f,
                    "input {input} previous output is not authenticated: {reason}"
                )
            }
            Self::MissingWitnessScript { input } => {
                write!(f, "input {input} is missing its witness script")
            }
            Self::UnsupportedPrevoutScript { input } => {
                write!(f, "input {input} does not spend native P2WSH")
            }
            Self::WitnessScriptCommitmentMismatch { input } => {
                write!(
                    f,
                    "input {input} witness script does not match its P2WSH output"
                )
            }
            Self::UnsupportedWitnessScript { input, reason } => {
                write!(
                    f,
                    "input {input} has an unsupported Vault witness script: {reason}"
                )
            }
            Self::UnsupportedLegacySighash {
                input,
                public_key,
                sighash,
            } => write!(
                f,
                "input {input} legacy signature for {public_key} uses refused sighash 0x{sighash:02x}"
            ),
            Self::InvalidLegacySignature { input, public_key } => write!(
                f,
                "input {input} legacy signature for {public_key} does not verify"
            ),
            Self::Unsatisfiable { input, reason } => {
                write!(f, "input {input} cannot be satisfied: {reason}")
            }
            Self::UnexpectedWitnessElement { input } => write!(
                f,
                "input {input} witness contained a signature this finaliser did not place"
            ),
            Self::RefusedToDropUnified {
                input,
                legacy_candidates,
            } => write!(
                f,
                "input {input} has a verified unified signature but {legacy_candidates} legacy \
                 signatures are too many to search for a witness that keeps it; refusing to \
                 finalise from legacy signatures alone"
            ),
            Self::UnsafeLegacyAlternative {
                input,
                legacy_signatures,
            } => write!(
                f,
                "input {input} retains {legacy_signatures} verified legacy signatures that can \
                 independently satisfy it while a unified-bearing witness is available; \
                 refusing local storage or export because a standard Bitcoin PSBT consumer can \
                 assemble an alternate witness"
            ),
        }
    }
}

impl error::Error for UnifiedFinalizeError {}

impl From<UnifiedSigningError> for UnifiedFinalizeError {
    fn from(value: UnifiedSigningError) -> Self {
        Self::Signing(value)
    }
}

impl From<UnifiedPsbtError> for UnifiedFinalizeError {
    fn from(value: UnifiedPsbtError) -> Self {
        Self::Adapter(value)
    }
}

/// One verified signature available to the satisfier, with the exact bytes it
/// must appear as in the witness.
#[derive(Clone)]
struct AvailableSignature {
    /// The DER-encoded signature (no sighash byte).
    der: ecdsa::Signature,
    /// The bytes to place in the witness: DER followed by the sighash byte.
    witness_bytes: Vec<u8>,
    unified: bool,
}

/// The transaction-level facts a timelock check needs, taken from the
/// unsigned transaction and the input being satisfied.
#[derive(Debug, Clone, Copy)]
struct InputLocks {
    version: transaction::Version,
    sequence: Sequence,
    lock_time: absolute::LockTime,
}

impl InputLocks {
    fn of(tx: &Transaction, input_index: usize) -> Self {
        Self {
            version: tx.version,
            sequence: tx.input[input_index].sequence,
            lock_time: tx.lock_time,
        }
    }
}

/// Offers exactly one signature per key, and remembers the placeholder bytes
/// miniscript will write for it so they can be swapped for the real encoding.
/// Answers timelock checks the way rust-miniscript's own `PsbtInputSatisfier`
/// does — with the transaction-level consensus guards, not the bare
/// `Sequence`/`LockTime` satisfiers: a CSV leaf needs transaction version ≥ 2
/// and a sequence that is a relative lock time; a CLTV leaf needs a sequence
/// that enables lock time. Without them the finaliser would assemble a
/// recovery witness for a transaction a node rejects.
struct KeyedSatisfier<'a> {
    available: &'a BTreeMap<PublicKey, AvailableSignature>,
    locks: InputLocks,
}

impl KeyedSatisfier<'_> {
    // Always `SIGHASH_ALL` here: miniscript cannot represent `0x21`, so the
    // placeholder is `DER || 0x01` and the real bytes are substituted after
    // satisfaction. The DER part is what identifies the element.
    fn placeholder(sig: &AvailableSignature) -> ecdsa::Signature {
        ecdsa::Signature {
            signature: sig.der.signature,
            sighash_type: EcdsaSighashType::All,
        }
    }
}

impl Satisfier<PublicKey> for KeyedSatisfier<'_> {
    fn lookup_ecdsa_sig(&self, pk: &PublicKey) -> Option<ecdsa::Signature> {
        self.available.get(pk).map(Self::placeholder)
    }

    // A witness script parsed back from bytes carries `pkh` leaves as raw key
    // hashes (the Vault recovery leaf is one), so the satisfier has to answer
    // by hash as well: the key whose hash matches, and its signature.
    fn lookup_raw_pkh_pk(&self, hash: &hash160::Hash) -> Option<PublicKey> {
        self.available
            .keys()
            .find(|pk| pk.pubkey_hash().as_byte_array() == hash.as_byte_array())
            .copied()
    }

    fn lookup_raw_pkh_ecdsa_sig(
        &self,
        hash: &hash160::Hash,
    ) -> Option<(PublicKey, ecdsa::Signature)> {
        self.available
            .iter()
            .find(|(pk, _)| pk.pubkey_hash().as_byte_array() == hash.as_byte_array())
            .map(|(pk, sig)| (*pk, Self::placeholder(sig)))
    }

    // Mirrors `miniscript::psbt::PsbtInputSatisfier::check_after`
    // (`TxIn::enables_lock_time`): an input whose sequence is final disables
    // the transaction's lock time.
    fn check_after(&self, n: absolute::LockTime) -> bool {
        if !self.locks.sequence.enables_absolute_lock_time() {
            return false;
        }
        <dyn Satisfier<PublicKey>>::check_after(&self.locks.lock_time, n)
    }

    // Mirrors `miniscript::psbt::PsbtInputSatisfier::check_older`: BIP-68 is
    // only enforced for version ≥ 2 transactions, and only a sequence with the
    // disable flag clear is a relative lock time.
    fn check_older(&self, n: relative::LockTime) -> bool {
        if self.locks.version < transaction::Version::TWO
            || !self.locks.sequence.is_relative_lock_time()
        {
            return false;
        }
        <dyn Satisfier<PublicKey>>::check_older(&self.locks.sequence, n)
    }
}

struct InputContext {
    spent_output: TxOut,
    witness_script: ScriptBuf,
    miniscript: Miniscript<PublicKey, Segwitv0>,
}

fn input_contexts(psbt: &UnifiedPsbt) -> Result<Vec<InputContext>, UnifiedFinalizeError> {
    let mut contexts = Vec::with_capacity(psbt.psbt().inputs.len());
    for (input_index, (txin, input)) in psbt
        .psbt()
        .unsigned_tx
        .input
        .iter()
        .zip(&psbt.psbt().inputs)
        .enumerate()
    {
        let spent_output = authenticate_previous_output(
            &txin.previous_output,
            input.non_witness_utxo.as_ref(),
            input.witness_utxo.as_ref(),
        )
        .map_err(|reason| UnifiedFinalizeError::InputAuthentication {
            input: input_index,
            reason,
        })?;
        if !spent_output.script_pubkey.is_p2wsh() {
            return Err(UnifiedFinalizeError::UnsupportedPrevoutScript { input: input_index });
        }
        let witness_script = input
            .witness_script
            .clone()
            .ok_or(UnifiedFinalizeError::MissingWitnessScript { input: input_index })?;
        if witness_script.to_p2wsh() != spent_output.script_pubkey {
            return Err(UnifiedFinalizeError::WitnessScriptCommitmentMismatch {
                input: input_index,
            });
        }
        let miniscript = Miniscript::<PublicKey, Segwitv0>::parse_with_ext(
            &witness_script,
            &ExtParams::sane().raw_pkh(),
        )
        .map_err(|err| UnifiedFinalizeError::UnsupportedWitnessScript {
            input: input_index,
            reason: err.to_string(),
        })?;
        contexts.push(InputContext {
            spent_output,
            witness_script,
            miniscript,
        });
    }
    Ok(contexts)
}

/// Verify a legacy `partial_sigs` entry against the BIP-143 digest. Only
/// `SIGHASH_ALL` is accepted; anything else — including every `ANYONECANPAY`
/// variant — is refused before any witness is built.
fn verify_legacy_signature<C: secp256k1::Verification>(
    secp: &secp256k1::Secp256k1<C>,
    cache: &mut SighashCache<&Transaction>,
    input_index: usize,
    context: &InputContext,
    public_key: &PublicKey,
    signature: &ecdsa::Signature,
) -> Result<(), UnifiedFinalizeError> {
    if signature.sighash_type != EcdsaSighashType::All {
        return Err(UnifiedFinalizeError::UnsupportedLegacySighash {
            input: input_index,
            public_key: *public_key,
            sighash: signature.sighash_type.to_u32(),
        });
    }
    let digest = cache
        .p2wsh_signature_hash(
            input_index,
            &context.witness_script,
            context.spent_output.value,
            EcdsaSighashType::All,
        )
        .map_err(|e| UnifiedFinalizeError::Unsatisfiable {
            input: input_index,
            reason: format!("legacy sighash: {e}"),
        })?;
    let message = secp256k1::Message::from_digest(digest.to_byte_array());
    secp.verify_ecdsa(&message, &signature.signature, &public_key.inner)
        .map_err(|_| UnifiedFinalizeError::InvalidLegacySignature {
            input: input_index,
            public_key: *public_key,
        })
}

/// Assemble the witness for one input from `available`, or explain why not.
fn satisfy_input(
    input_index: usize,
    context: &InputContext,
    available: &BTreeMap<PublicKey, AvailableSignature>,
    locks: InputLocks,
) -> Result<(Witness, InputWitnessReport), UnifiedFinalizeError> {
    let satisfier = KeyedSatisfier { available, locks };
    let stack =
        context
            .miniscript
            .satisfy(satisfier)
            .map_err(|e| UnifiedFinalizeError::Unsatisfiable {
                input: input_index,
                reason: e.to_string(),
            })?;

    // Swap every placeholder (`DER || 0x01`) for the bytes that key's signature
    // must actually carry, counting what was used. An element that looks like a
    // signature but is not one we offered is an invariant violation.
    let placeholders: BTreeMap<Vec<u8>, &AvailableSignature> = available
        .values()
        .map(|sig| {
            let mut placeholder = sig.der.signature.serialize_der().to_vec();
            placeholder.push(EcdsaSighashType::All as u8);
            (placeholder, sig)
        })
        .collect();
    let mut report = InputWitnessReport {
        unified_used: 0,
        legacy_used: 0,
    };
    let mut witness = Witness::new();
    for element in stack {
        if let Some(sig) = placeholders.get(&element) {
            if sig.unified {
                report.unified_used += 1;
            } else {
                report.legacy_used += 1;
            }
            witness.push(&sig.witness_bytes);
        } else if looks_like_der_signature(&element) {
            return Err(UnifiedFinalizeError::UnexpectedWitnessElement { input: input_index });
        } else {
            witness.push(&element);
        }
    }
    witness.push(context.witness_script.as_bytes());
    Ok((witness, report))
}

/// Largest legacy candidate set [`satisfy_preferring_unified`] will search
/// exhaustively: 2^12 satisfactions at ~15 µs each in a release build, about
/// 63 ms for the worst case on one input (measured by Elrond at `0eb042c2`;
/// a 2-of-3 searches four subsets). A Vault path has a handful of keys; a
/// PSBT carrying more legacy signatures than this on one input while also
/// holding a unified one is refused rather than degraded.
const MAX_LEGACY_KEYS_FOR_SEARCH: usize = 12;

/// Satisfy with unified signatures plus legacy ones, but never let a legacy
/// signature crowd out a verified unified one.
///
/// miniscript picks the cheapest satisfaction it can and all ECDSA signatures
/// cost the same, so with more signatures on offer than the threshold needs it
/// may take legacy keys in script order and leave a unified key unused — a
/// replayable witness for an input that had a replay-proof signature. When that
/// happens, and the input has unified signatures at all, every subset of the
/// legacy set is offered alongside all the unified signatures and the
/// satisfaction using the most unified signatures wins; ties go to the smaller
/// witness, then to the first subset in enumeration order. The enumeration is
/// over a `BTreeMap` of keys, so the outcome is a deterministic function of
/// the signatures present. The only way a legacy-only witness comes out of
/// here is when **no** offered subset lets the script use a unified signature
/// — the unified key is then genuinely unusable for this transaction (e.g. a
/// recovery key whose timelock is not enabled) and the input is reported as
/// replayable. Over [`MAX_LEGACY_KEYS_FOR_SEARCH`] the input is refused.
fn satisfy_preferring_unified(
    input_index: usize,
    context: &InputContext,
    unified: &BTreeMap<PublicKey, AvailableSignature>,
    legacy: &BTreeMap<PublicKey, AvailableSignature>,
    locks: InputLocks,
) -> Result<(Witness, InputWitnessReport), UnifiedFinalizeError> {
    let mut all: BTreeMap<PublicKey, AvailableSignature> = unified.clone();
    all.extend(legacy.iter().map(|(k, v)| (*k, v.clone())));
    let (witness, report) = satisfy_input(input_index, context, &all, locks)?;
    if unified.is_empty() || report.unified_used > 0 {
        return Ok((witness, report));
    }

    let legacy_keys: Vec<&PublicKey> = legacy.keys().collect();
    if legacy_keys.len() > MAX_LEGACY_KEYS_FOR_SEARCH {
        return Err(UnifiedFinalizeError::RefusedToDropUnified {
            input: input_index,
            legacy_candidates: legacy_keys.len(),
        });
    }
    let mut best: Option<(Witness, InputWitnessReport)> = None;
    for mask in 0u32..(1u32 << legacy_keys.len()) {
        let mut subset: BTreeMap<PublicKey, AvailableSignature> = unified.clone();
        for (bit, key) in legacy_keys.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                subset.insert(**key, legacy[*key].clone());
            }
        }
        consider_unified_satisfaction(input_index, context, &subset, locks, &mut best);
    }
    Ok(best.unwrap_or((witness, report)))
}

/// A witness element shaped like a DER ECDSA signature with a trailing sighash
/// byte — any trailing byte, so a stray `DER || 0x21` is caught as well as a
/// `DER || 0x01`. Used only to refuse elements the finaliser did not place;
/// script pushes (empty, `1`, hash preimages, keys) never look like this.
pub(crate) fn looks_like_der_signature(element: &[u8]) -> bool {
    match element.split_last() {
        Some((_, der)) => {
            der.len() > 8 && der[0] == 0x30 && secp256k1::ecdsa::Signature::from_der(der).is_ok()
        }
        None => false,
    }
}

/// How many signatures of each kind [`verify_all_signatures`] checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VerifiedSignatureCounts {
    pub unified: usize,
    pub legacy: usize,
}

/// Verify **every** signature the PSBT carries, without assembling a witness:
/// unified records through [`verify_p2wsh_all_unified`] (which also holds the
/// PSBT to the adapter's rules), legacy `partial_sigs` against the BIP-143
/// digest and restricted to `SIGHASH_ALL`. An unsigned or partially signed
/// PSBT passes; any signature that would make finalisation refuse fails here
/// with the same typed error.
///
/// This is the check for *persistence and merge boundaries* — the daemon's
/// first insert and merge, the desktop's merge — where the adapter alone is
/// not enough: it validates representation, not cryptographic validity, so a
/// legacy `ANYONECANPAY` signature or a signature made for another
/// transaction would otherwise be stored, refused by every later merge as a
/// conflict against its correct replacement, and refused again at
/// finalisation — a stored spend that cannot be completed through the API
/// that wrote it.
pub fn verify_all_signatures<C: secp256k1::Verification>(
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<VerifiedSignatureCounts, UnifiedFinalizeError> {
    let unified = verify_p2wsh_all_unified(psbt, secp)?;
    let contexts = input_contexts(psbt)?;
    let mut legacy_cache = SighashCache::new(&psbt.psbt().unsigned_tx);
    let mut legacy = 0;
    for (input_index, context) in contexts.iter().enumerate() {
        legacy += verified_legacy_signatures(
            secp,
            &mut legacy_cache,
            input_index,
            context,
            &psbt.psbt().inputs[input_index],
        )?
        .len();
    }
    Ok(VerifiedSignatureCounts { unified, legacy })
}

/// Every legacy `partial_sigs` entry of one input, verified against the
/// BIP-143 digest and restricted to `SIGHASH_ALL`, as signatures the
/// satisfier may offer. A key with a unified record cannot also have a legacy
/// one — the adapter's `validate_internal` refuses that — so each is a
/// distinct key.
fn verified_legacy_signatures<C: secp256k1::Verification>(
    secp: &secp256k1::Secp256k1<C>,
    cache: &mut SighashCache<&Transaction>,
    input_index: usize,
    context: &InputContext,
    input: &miniscript::bitcoin::psbt::Input,
) -> Result<BTreeMap<PublicKey, AvailableSignature>, UnifiedFinalizeError> {
    let mut legacy: BTreeMap<PublicKey, AvailableSignature> = BTreeMap::new();
    for (public_key, signature) in &input.partial_sigs {
        verify_legacy_signature(secp, cache, input_index, context, public_key, signature)?;
        let mut witness_bytes = signature.signature.serialize_der().to_vec();
        witness_bytes.push(EcdsaSighashType::All as u8);
        legacy.insert(
            *public_key,
            AvailableSignature {
                der: *signature,
                witness_bytes,
                unified: false,
            },
        );
    }
    Ok(legacy)
}

/// Verified unified records for one input in the representation expected by
/// the miniscript satisfier. [`verify_p2wsh_all_unified`] must have run first.
fn available_unified_signatures(
    unified: &[UnifiedSignature],
    input_index: usize,
) -> Result<BTreeMap<PublicKey, AvailableSignature>, UnifiedFinalizeError> {
    let mut available = BTreeMap::new();
    for record in unified
        .iter()
        .filter(|record| record.input_index == input_index)
    {
        debug_assert_eq!(record.signature.last(), Some(&UNIFIED_SIGHASH_ALL));
        let invalid = || {
            UnifiedFinalizeError::Signing(UnifiedSigningError::InvalidUnifiedSignature {
                input: input_index,
                public_key: record.public_key,
            })
        };
        let (_, der_part) = record.signature.split_last().ok_or(invalid())?;
        let der = ecdsa::Signature {
            signature: secp256k1::ecdsa::Signature::from_der(der_part).map_err(|_| invalid())?,
            sighash_type: EcdsaSighashType::All,
        };
        available.insert(
            record.public_key,
            AvailableSignature {
                der,
                witness_bytes: record.signature.clone(),
                unified: true,
            },
        );
    }
    Ok(available)
}

/// Refuse a locally retained PSBT when one of its inputs has both a
/// unified-bearing satisfaction and an independently sufficient legacy
/// signature set.
///
/// This is a local storage/export policy, not a claim about signatures already
/// held by another device, coordinator, or exported copy. It deliberately
/// permits mixed witnesses: legacy signatures needed alongside a unified
/// signature remain acceptable while they are insufficient on their own.
/// Inputs that are incomplete or finalise only through legacy signatures are
/// also left to the ordinary replay-status policy.
pub fn ensure_no_unsafe_legacy_alternative<C: secp256k1::Verification>(
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<(), UnifiedFinalizeError> {
    verify_p2wsh_all_unified(psbt, secp)?;
    let contexts = input_contexts(psbt)?;
    let unified = unified_signatures(psbt)?;
    let unsigned_tx = &psbt.psbt().unsigned_tx;
    let mut legacy_cache = SighashCache::new(unsigned_tx);

    for (input_index, context) in contexts.iter().enumerate() {
        let available = available_unified_signatures(&unified, input_index)?;
        let legacy = verified_legacy_signatures(
            secp,
            &mut legacy_cache,
            input_index,
            context,
            &psbt.psbt().inputs[input_index],
        )?;
        if available.is_empty() || legacy.is_empty() {
            continue;
        }

        let locks = InputLocks::of(unsigned_tx, input_index);
        let protected = match satisfy_input(input_index, context, &available, locks) {
            Ok(done) => Some(done),
            Err(UnifiedFinalizeError::Unsatisfiable { .. }) => {
                match satisfy_preferring_unified(input_index, context, &available, &legacy, locks) {
                    Ok(done) => Some(done),
                    Err(UnifiedFinalizeError::Unsatisfiable { .. }) => None,
                    Err(other) => return Err(other),
                }
            }
            Err(other) => return Err(other),
        };
        let Some((_, report)) = protected else {
            continue;
        };
        if !report.replay_protected() {
            continue;
        }

        match satisfy_input(input_index, context, &legacy, locks) {
            Ok(_) => {
                return Err(UnifiedFinalizeError::UnsafeLegacyAlternative {
                    input: input_index,
                    legacy_signatures: legacy.len(),
                })
            }
            Err(UnifiedFinalizeError::Unsatisfiable { .. }) => {}
            Err(other) => return Err(other),
        }
    }
    Ok(())
}

/// Finalise every input of `psbt` and return the transaction ready to
/// broadcast, together with what each witness is made of.
///
/// Every unified signature in the PSBT is verified first, then every legacy
/// `partial_sigs` entry of every input — including ones no witness will use
/// (an invalid or non-`SIGHASH_ALL` record anywhere fails the whole call:
/// nothing is assembled from a PSBT that lies). Then, per input: unified
/// signatures are offered alone; if the script cannot be satisfied from those,
/// the verified legacy signatures are added for keys that have **no** unified
/// signature, and the input is tried again with the preference pass of
/// [`satisfy_preferring_unified`], so a unified signature that *can* be part of
/// the witness always is. A caller that wants to refuse replayable inputs uses
/// the report; this function does not decide policy.
pub fn finalize_p2wsh_all_unified<C: secp256k1::Verification>(
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<FinalizedSpend, UnifiedFinalizeError> {
    // Cryptographic verification of every unified record, with the PSBT's own
    // authentication of prevouts and witness scripts.
    verify_p2wsh_all_unified(psbt, secp)?;
    let contexts = input_contexts(psbt)?;
    let unified = unified_signatures(psbt)?;

    let unsigned_tx = &psbt.psbt().unsigned_tx;
    let mut legacy_cache = SighashCache::new(unsigned_tx);
    let mut transaction = unsigned_tx.clone();
    let mut reports = Vec::with_capacity(contexts.len());

    for (input_index, context) in contexts.iter().enumerate() {
        let input = &psbt.psbt().inputs[input_index];

        // Unified first: verified above; the witness bytes are the record itself.
        let available = available_unified_signatures(&unified, input_index)?;

        // The input's own sighash *request* was already held to the adapter's
        // rule (absent, `SIGHASH_ALL` or `ALL|UNIFIED`) by `validate_internal`
        // inside the verifier above, and to the stricter rule (absent or
        // `ALL|UNIFIED`) where the input carries a unified record. Nothing to
        // repeat here: one definition, at the adapter, for every boundary.

        // Every legacy record is verified now, whether or not a witness will
        // use it (`verified_legacy_signatures`, the same check the persistence
        // boundaries run through `verify_all_signatures`).
        let legacy =
            verified_legacy_signatures(secp, &mut legacy_cache, input_index, context, input)?;

        let locks = InputLocks::of(unsigned_tx, input_index);
        let first = satisfy_input(input_index, context, &available, locks);
        let (witness, report) = match first {
            Ok(done) => done,
            Err(UnifiedFinalizeError::Unsatisfiable { .. }) => {
                satisfy_preferring_unified(input_index, context, &available, &legacy, locks)?
            }
            Err(other) => return Err(other),
        };
        transaction.input[input_index].witness = witness;
        reports.push(report);
    }

    Ok(FinalizedSpend {
        transaction,
        inputs: reports,
    })
}

/// The amount an input spends, for callers that report fees alongside the
/// witness composition. Authenticated the same way the finaliser does.
pub fn spent_amount(psbt: &UnifiedPsbt, input_index: usize) -> Option<Amount> {
    let txin = psbt.psbt().unsigned_tx.input.get(input_index)?;
    let input = psbt.psbt().inputs.get(input_index)?;
    authenticate_previous_output(
        &txin.previous_output,
        input.non_witness_utxo.as_ref(),
        input.witness_utxo.as_ref(),
    )
    .ok()
    .map(|o| o.value)
}

fn consider_unified_satisfaction(
    input_index: usize,
    context: &InputContext,
    subset: &BTreeMap<PublicKey, AvailableSignature>,
    locks: InputLocks,
    best: &mut Option<(Witness, InputWitnessReport)>,
) {
    if let Ok((w, r)) = satisfy_input(input_index, context, subset, locks) {
        if r.unified_used == 0 {
            return;
        }
        let better = match &*best {
            None => true,
            Some((bw, br)) => {
                r.unified_used > br.unified_used
                    || (r.unified_used == br.unified_used && w.size() < bw.size())
            }
        };
        if better {
            *best = Some((w, r));
        }
    }
}

#[cfg(test)]
#[path = "unified_finalize/tests.rs"]
mod tests;
