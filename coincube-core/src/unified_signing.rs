//! Explicit Bitcoin Blake2b P2WSH signing and signature verification.
//!
//! This API must be selected from an authenticated chain identity. A
//! [`bitcoin::Network`] value cannot distinguish Bitcoin from Bitcoin Blake2b,
//! because both chains use the same address and key encodings. Callers must also
//! obtain previous transactions from a trusted source for the selected chain:
//! matching a PSBT outpoint proves transaction linkage, not chain inclusion or
//! that an output is currently unspent.
//! Every input must be a supported native P2WSH Vault input, even when this
//! signer has no matching key for that input.
//!
//! Verification here means that every supplied unified signature is valid for
//! the transaction, authenticated PSBT prevouts, and supported Vault witness
//! script. It does not establish a sufficient signature threshold, finalization,
//! authorization by a wallet policy, or replay protection. A valid PSBT with no
//! unified signatures returns `Ok(0)` from [`verify_p2wsh_all_unified`].

use std::{collections::BTreeSet, error, fmt};

use miniscript::{
    bitcoin::{
        bip32::{ChildNumber, DerivationPath, Xpub},
        hashes::Hash,
        psbt::{raw::ProprietaryKey, Psbt, PsbtSighashType},
        secp256k1, Network, PublicKey, ScriptBuf, TxOut,
    },
    ExtParams, Miniscript, Segwitv0, Terminal,
};

use crate::{
    psbt_unified::{
        merge_signatures, unified_signatures, validate_internal, UnifiedPsbt, UnifiedPsbtError,
    },
    signer::MasterSigner,
    spend::{authenticate_previous_output, InputAuthError},
    unified_sighash::{UnifiedSighashCache, UnifiedSighashError, SCRIPT_TYPE_WITNESS_V0},
};

const UNIFIED_SIGHASH_ALL: u8 = 0x21;
const PROPRIETARY_PREFIX: &[u8] = b"coincube";
const PROPRIETARY_SUBTYPE: u8 = 0;

/// Failures from the explicit Bitcoin Blake2b P2WSH signing boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifiedSigningError {
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
    /// The input carries Taproot signature data (`tap_key_sig` or
    /// `tap_script_sigs`) although it spends native P2WSH. Nothing here could
    /// act on it; it would be stored and finalised around, ignored, which is
    /// the unsupported-data contract failing. Refused wherever the P2WSH
    /// context is established, so every verifying boundary — finaliser,
    /// daemon insert and merge, desktop merge — inherits it.
    TaprootSignatureData {
        input: usize,
    },
    WitnessScriptCommitmentMismatch {
        input: usize,
    },
    UnsupportedWitnessScript {
        input: usize,
        reason: String,
    },
    IncompatibleSighash {
        input: usize,
        actual: u32,
    },
    SignerTargetPathTooDeep {
        depth: usize,
    },
    SignerTargetMismatch {
        expected: Xpub,
        actual: Xpub,
    },
    DerivationPathTooDeep {
        input: usize,
        public_key: PublicKey,
        depth: usize,
    },
    DerivedPublicKeyMismatch {
        input: usize,
        public_key: PublicKey,
    },
    KeyNotInWitnessScript {
        input: usize,
        public_key: PublicKey,
    },
    InvalidUnifiedSignature {
        input: usize,
        public_key: PublicKey,
    },
    Sighash(UnifiedSighashError),
    PsbtConstruction(String),
}

/// Refusals while binding an authenticated Keychain record to a BIP-48 account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifiedSignerTargetError {
    InvalidAccountPath {
        path: DerivationPath,
    },
    CoinTypeMismatch {
        network: Network,
        expected: ChildNumber,
        actual: ChildNumber,
    },
    XpubNetworkMismatch,
    XpubDepthMismatch {
        expected: u8,
        actual: u8,
    },
    XpubChildNumberMismatch {
        expected: ChildNumber,
        actual: ChildNumber,
    },
}

impl fmt::Display for UnifiedSignerTargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAccountPath { path } => write!(
                f,
                "signer target path {path} must have the hardened BIP-48 account shape m/48'/<coin>'/<account>'/2'"
            ),
            Self::CoinTypeMismatch {
                network,
                expected,
                actual,
            } => write!(
                f,
                "signer target coin type {actual} does not match {network}; expected {expected}"
            ),
            Self::XpubNetworkMismatch => {
                write!(f, "signer target xpub network does not match the supplied network")
            }
            Self::XpubDepthMismatch { expected, actual } => write!(
                f,
                "signer target xpub depth is {actual}; expected {expected} for a BIP-48 account"
            ),
            Self::XpubChildNumberMismatch { expected, actual } => write!(
                f,
                "signer target xpub child number is {actual}; expected account path leaf {expected}"
            ),
        }
    }
}

impl error::Error for UnifiedSignerTargetError {}

impl fmt::Display for UnifiedSigningError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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
            Self::TaprootSignatureData { input } => write!(
                f,
                "input {input} spends native P2WSH but carries Taproot signature data"
            ),
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
            Self::IncompatibleSighash { input, actual } => write!(
                f,
                "input {input} requests incompatible sighash 0x{actual:08x}"
            ),
            Self::SignerTargetPathTooDeep { depth } => write!(
                f,
                "signer target derivation path has depth {depth}, exceeding BIP32's maximum of 255"
            ),
            Self::SignerTargetMismatch { expected, actual } => write!(
                f,
                "signer target xpub mismatch: expected {expected}, derived {actual}"
            ),
            Self::DerivationPathTooDeep {
                input,
                public_key,
                depth,
            } => write!(
                f,
                "input {input} derivation path for {public_key} has depth {depth}, exceeding BIP32's maximum of 255"
            ),
            Self::DerivedPublicKeyMismatch { input, public_key } => write!(
                f,
                "input {input} derivation does not produce public key {public_key}"
            ),
            Self::KeyNotInWitnessScript { input, public_key } => {
                write!(
                    f,
                    "public key {public_key} is not used by input {input}'s witness script"
                )
            }
            Self::InvalidUnifiedSignature { input, public_key } => write!(
                f,
                "unified signature for {public_key} in input {input} is cryptographically invalid"
            ),
            Self::Sighash(err) => write!(f, "unified sighash failed: {err}"),
            Self::PsbtConstruction(err) => write!(f, "could not construct signature delta: {err}"),
        }
    }
}

impl error::Error for UnifiedSigningError {}

impl From<UnifiedPsbtError> for UnifiedSigningError {
    fn from(value: UnifiedPsbtError) -> Self {
        Self::Adapter(value)
    }
}

impl From<UnifiedSighashError> for UnifiedSigningError {
    fn from(value: UnifiedSighashError) -> Self {
        Self::Sighash(value)
    }
}

struct InputContext {
    spent_output: TxOut,
    witness_script: ScriptBuf,
    miniscript: Miniscript<PublicKey, Segwitv0>,
}

/// The authenticated Keychain account that one signing session approved.
///
/// Keychain signer records are account-scoped: the path identifies the BIP-48
/// account and the xpub is the public key committed by the authenticated
/// session descriptor. Both are required so a sibling account sharing the same
/// master fingerprint cannot be selected by PSBT metadata alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnifiedSignerTarget {
    account_path: DerivationPath,
    account_xpub: Xpub,
}

impl UnifiedSignerTarget {
    pub fn new(
        network: Network,
        account_path: DerivationPath,
        account_xpub: Xpub,
    ) -> Result<Self, UnifiedSignerTargetError> {
        let components = account_path.as_ref();
        let purpose = ChildNumber::from_hardened_idx(48).expect("48 is a valid child index");
        let expected_coin =
            ChildNumber::from_hardened_idx(if network == Network::Bitcoin { 0 } else { 1 })
                .expect("BIP-44 coin types are valid child indices");
        let script = ChildNumber::from_hardened_idx(2).expect("2 is a valid child index");
        if components.len() != 4
            || components[0] != purpose
            || !components[1].is_hardened()
            || !components[2].is_hardened()
            || components[3] != script
        {
            return Err(UnifiedSignerTargetError::InvalidAccountPath { path: account_path });
        }
        if components[1] != expected_coin {
            return Err(UnifiedSignerTargetError::CoinTypeMismatch {
                network,
                expected: expected_coin,
                actual: components[1],
            });
        }
        if account_xpub.network != network.into() {
            return Err(UnifiedSignerTargetError::XpubNetworkMismatch);
        }
        let expected_depth = components.len() as u8;
        if account_xpub.depth != expected_depth {
            return Err(UnifiedSignerTargetError::XpubDepthMismatch {
                expected: expected_depth,
                actual: account_xpub.depth,
            });
        }
        if account_xpub.child_number != components[3] {
            return Err(UnifiedSignerTargetError::XpubChildNumberMismatch {
                expected: components[3],
                actual: account_xpub.child_number,
            });
        }
        Ok(Self {
            account_path,
            account_xpub,
        })
    }

    pub fn account_path(&self) -> &DerivationPath {
        &self.account_path
    }

    pub fn account_xpub(&self) -> Xpub {
        self.account_xpub
    }

    fn authorizes(
        &self,
        path: &DerivationPath,
        public_key: &secp256k1::PublicKey,
        secp: &secp256k1::Secp256k1<secp256k1::All>,
    ) -> bool {
        let Some(relative_path) = path.as_ref().strip_prefix(self.account_path.as_ref()) else {
            return false;
        };
        if relative_path.is_empty() || relative_path.iter().any(ChildNumber::is_hardened) {
            return false;
        }
        self.account_xpub
            .derive_pub(secp, &relative_path)
            .is_ok_and(|derived| derived.public_key == *public_key)
    }
}

#[derive(Clone, Copy)]
enum PrevoutPolicy {
    FullPreviousTransaction,
    KeychainSegwit,
}

/// Sign every supported input key derived from `signer` with ALL|UNIFIED.
///
/// The input is immutable. All inputs and all existing unified signatures are
/// validated before a local signature delta is created. The delta is applied
/// through the adapter's checked merge, and only inputs receiving a signature
/// have their PSBT sighash request set to raw `0x21`. Every input must be a
/// supported native P2WSH Vault input, including inputs for which this signer
/// has no matching key.
pub fn sign_p2wsh_all_unified(
    signer: &MasterSigner,
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<secp256k1::All>,
) -> Result<UnifiedPsbt, UnifiedSigningError> {
    sign_p2wsh_all_unified_inner(
        signer,
        None,
        psbt,
        secp,
        PrevoutPolicy::FullPreviousTransaction,
    )
}

/// Sign only keys below an authenticated Keychain account target.
///
/// Unlike the desktop signing entry, this accepts a native-P2WSH input backed
/// by `witness_utxo` when the full previous transaction is absent. If both are
/// present they must agree. Inputs outside the approved account path are left
/// byte-identical even when they share the signer's master fingerprint.
pub fn sign_p2wsh_all_unified_for_target(
    signer: &MasterSigner,
    target: &UnifiedSignerTarget,
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<secp256k1::All>,
) -> Result<UnifiedPsbt, UnifiedSigningError> {
    if target.account_path.len() > usize::from(u8::MAX) {
        return Err(UnifiedSigningError::SignerTargetPathTooDeep {
            depth: target.account_path.len(),
        });
    }
    let actual = signer.xpub_at(&target.account_path, secp);
    if actual != target.account_xpub {
        return Err(UnifiedSigningError::SignerTargetMismatch {
            expected: target.account_xpub,
            actual,
        });
    }
    sign_p2wsh_all_unified_inner(
        signer,
        Some(target),
        psbt,
        secp,
        PrevoutPolicy::KeychainSegwit,
    )
}

fn sign_p2wsh_all_unified_inner(
    signer: &MasterSigner,
    target: Option<&UnifiedSignerTarget>,
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<secp256k1::All>,
    prevout_policy: PrevoutPolicy,
) -> Result<UnifiedPsbt, UnifiedSigningError> {
    let contexts = validate_inputs(psbt, prevout_policy)?;
    verify_with_contexts(psbt, secp, &contexts)?;

    let fingerprint = signer.fingerprint(secp);
    let mut signatures = Vec::new();
    for (input_index, input) in psbt.psbt().inputs.iter().enumerate() {
        let context = &contexts[input_index];
        for (raw_public_key, (origin, path)) in &input.bip32_derivation {
            if *origin != fingerprint {
                continue;
            }
            if target.is_some_and(|target| !target.authorizes(path, raw_public_key, secp)) {
                continue;
            }
            let public_key = PublicKey::new(*raw_public_key);
            validate_signing_key(signer, secp, input_index, input, context, public_key, path)?;
            signatures.push((input_index, public_key, path.clone()));
        }
    }

    if signatures.is_empty() {
        return Ok(psbt.clone());
    }

    let spent_outputs: Vec<_> = contexts
        .iter()
        .map(|ctx| ctx.spent_output.clone())
        .collect();
    let cache = UnifiedSighashCache::new(&psbt.psbt().unsigned_tx, &spent_outputs)?;
    let mut delta = empty_signature_delta(psbt)?;
    let mut signed_inputs = BTreeSet::new();
    for (input_index, public_key, path) in signatures {
        let digest = cache.signature_hash(
            input_index,
            UNIFIED_SIGHASH_ALL,
            SCRIPT_TYPE_WITNESS_V0,
            &contexts[input_index].witness_script,
        )?;
        let message = secp256k1::Message::from_digest(digest);
        let private_key = signer.xpriv_at(&path, secp).to_priv();
        let signature = secp.sign_ecdsa_low_r(&message, &private_key.inner);
        let mut encoded = signature.serialize_der().to_vec();
        encoded.push(UNIFIED_SIGHASH_ALL);
        delta.psbt_mut().inputs[input_index]
            .proprietary
            .insert(proprietary_key(&public_key), encoded);
        signed_inputs.insert(input_index);
    }
    validate_internal(&delta)?;

    let mut result = psbt.clone();
    merge_signatures(&mut result, &delta)?;
    for input_index in signed_inputs {
        result.psbt_mut().inputs[input_index].sighash_type =
            Some(PsbtSighashType::from_u32(u32::from(UNIFIED_SIGHASH_ALL)));
    }
    validate_internal(&result)?;
    verify_with_contexts(&result, secp, &contexts)?;
    Ok(result)
}

/// Verify every supplied unified record and return the number verified.
///
/// `Ok(0)` explicitly means that the PSBT and its supported P2WSH inputs were
/// validated but it contained no unified signatures. It does not mean the PSBT
/// is sufficiently signed or finalizable.
pub fn verify_p2wsh_all_unified<C: secp256k1::Verification>(
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<usize, UnifiedSigningError> {
    let contexts = validate_inputs(psbt, PrevoutPolicy::FullPreviousTransaction)?;
    verify_with_contexts(psbt, secp, &contexts)
}

/// Verify unified signatures using Keychain's SegWit prevout contract.
///
/// Native P2WSH inputs may use `witness_utxo` without a full previous
/// transaction. Every other validation and signature rule is identical to
/// [`verify_p2wsh_all_unified`].
pub fn verify_keychain_p2wsh_all_unified<C: secp256k1::Verification>(
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<usize, UnifiedSigningError> {
    let contexts = validate_inputs(psbt, PrevoutPolicy::KeychainSegwit)?;
    verify_with_contexts(psbt, secp, &contexts)
}

/// Compute one Keychain production digest after validating every PSBT input
/// and every unified signature already present.
pub fn keychain_p2wsh_all_unified_digest<C: secp256k1::Verification>(
    psbt: &UnifiedPsbt,
    input_index: usize,
    secp: &secp256k1::Secp256k1<C>,
) -> Result<[u8; 32], UnifiedSigningError> {
    let contexts = validate_inputs(psbt, PrevoutPolicy::KeychainSegwit)?;
    verify_with_contexts(psbt, secp, &contexts)?;
    let context = contexts.get(input_index).ok_or({
        UnifiedSigningError::Sighash(UnifiedSighashError::InputIndexOutOfBounds {
            index: input_index,
            inputs: contexts.len(),
        })
    })?;
    let spent_outputs: Vec<_> = contexts
        .iter()
        .map(|ctx| ctx.spent_output.clone())
        .collect();
    UnifiedSighashCache::new(&psbt.psbt().unsigned_tx, &spent_outputs)?
        .signature_hash(
            input_index,
            UNIFIED_SIGHASH_ALL,
            SCRIPT_TYPE_WITNESS_V0,
            &context.witness_script,
        )
        .map_err(Into::into)
}

fn validate_inputs(
    psbt: &UnifiedPsbt,
    prevout_policy: PrevoutPolicy,
) -> Result<Vec<InputContext>, UnifiedSigningError> {
    validate_internal(psbt)?;
    let mut contexts = Vec::with_capacity(psbt.psbt().inputs.len());
    for (input_index, (txin, input)) in psbt
        .psbt()
        .unsigned_tx
        .input
        .iter()
        .zip(&psbt.psbt().inputs)
        .enumerate()
    {
        let spent_output = authenticate_unified_input(
            &txin.previous_output,
            input.non_witness_utxo.as_ref(),
            input.witness_utxo.as_ref(),
            prevout_policy,
        )
        .map_err(|reason| UnifiedSigningError::InputAuthentication {
            input: input_index,
            reason,
        })?;
        if !spent_output.script_pubkey.is_p2wsh() {
            return Err(UnifiedSigningError::UnsupportedPrevoutScript { input: input_index });
        }
        if input.tap_key_sig.is_some() || !input.tap_script_sigs.is_empty() {
            return Err(UnifiedSigningError::TaprootSignatureData { input: input_index });
        }
        let witness_script = input
            .witness_script
            .clone()
            .ok_or(UnifiedSigningError::MissingWitnessScript { input: input_index })?;
        if witness_script.to_p2wsh() != spent_output.script_pubkey {
            return Err(UnifiedSigningError::WitnessScriptCommitmentMismatch {
                input: input_index,
            });
        }
        // Decoding a compiled `pkh()` leaf necessarily recovers only its hash,
        // so permit that one representation gap while retaining every other
        // Miniscript sanity rule.
        let miniscript = Miniscript::<PublicKey, Segwitv0>::parse_with_ext(
            &witness_script,
            &ExtParams::sane().raw_pkh(),
        )
        .map_err(|err| UnifiedSigningError::UnsupportedWitnessScript {
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

fn authenticate_unified_input(
    outpoint: &miniscript::bitcoin::OutPoint,
    previous_tx: Option<&miniscript::bitcoin::Transaction>,
    witness_utxo: Option<&TxOut>,
    policy: PrevoutPolicy,
) -> Result<TxOut, InputAuthError> {
    if previous_tx.is_some() || matches!(policy, PrevoutPolicy::FullPreviousTransaction) {
        return authenticate_previous_output(outpoint, previous_tx, witness_utxo);
    }
    let output = witness_utxo.ok_or(InputAuthError::MissingPreviousTransaction)?;
    if output.value > miniscript::bitcoin::Amount::MAX_MONEY {
        return Err(InputAuthError::InvalidAmount(output.value));
    }
    Ok(output.clone())
}

fn verify_with_contexts<C: secp256k1::Verification>(
    psbt: &UnifiedPsbt,
    secp: &secp256k1::Secp256k1<C>,
    contexts: &[InputContext],
) -> Result<usize, UnifiedSigningError> {
    let signatures = unified_signatures(psbt)?;
    for signature in &signatures {
        require_compatible_sighash(
            signature.input_index,
            psbt.psbt().inputs[signature.input_index].sighash_type,
        )?;
        if !script_uses_key(
            &contexts[signature.input_index].miniscript,
            &signature.public_key,
        ) {
            return Err(UnifiedSigningError::KeyNotInWitnessScript {
                input: signature.input_index,
                public_key: signature.public_key,
            });
        }
    }

    let spent_outputs: Vec<_> = contexts
        .iter()
        .map(|ctx| ctx.spent_output.clone())
        .collect();
    let cache = UnifiedSighashCache::new(&psbt.psbt().unsigned_tx, &spent_outputs)?;
    for record in &signatures {
        let digest = cache.signature_hash(
            record.input_index,
            UNIFIED_SIGHASH_ALL,
            SCRIPT_TYPE_WITNESS_V0,
            &contexts[record.input_index].witness_script,
        )?;
        let signature =
            secp256k1::ecdsa::Signature::from_der(&record.signature[..record.signature.len() - 1])
                .map_err(|_| UnifiedSigningError::InvalidUnifiedSignature {
                    input: record.input_index,
                    public_key: record.public_key,
                })?;
        let message = secp256k1::Message::from_digest(digest);
        secp.verify_ecdsa(&message, &signature, &record.public_key.inner)
            .map_err(|_| UnifiedSigningError::InvalidUnifiedSignature {
                input: record.input_index,
                public_key: record.public_key,
            })?;
    }
    Ok(signatures.len())
}

fn require_compatible_sighash(
    input: usize,
    requested: Option<PsbtSighashType>,
) -> Result<(), UnifiedSigningError> {
    if let Some(requested) = requested {
        let actual = requested.to_u32();
        if actual != u32::from(UNIFIED_SIGHASH_ALL) {
            return Err(UnifiedSigningError::IncompatibleSighash { input, actual });
        }
    }
    Ok(())
}

pub(crate) fn script_uses_key(
    miniscript: &Miniscript<PublicKey, Segwitv0>,
    public_key: &PublicKey,
) -> bool {
    miniscript.iter().any(|node| match &node.node {
        Terminal::PkK(key) | Terminal::PkH(key) => key == public_key,
        Terminal::RawPkH(hash) => public_key.pubkey_hash().as_byte_array() == hash.as_byte_array(),
        Terminal::Multi(keys) => keys.iter().any(|key| key == public_key),
        _ => false,
    })
}

fn proprietary_key(public_key: &PublicKey) -> ProprietaryKey {
    ProprietaryKey {
        prefix: PROPRIETARY_PREFIX.to_vec(),
        subtype: PROPRIETARY_SUBTYPE,
        key: public_key.to_bytes(),
    }
}

fn empty_signature_delta(psbt: &UnifiedPsbt) -> Result<UnifiedPsbt, UnifiedSigningError> {
    let mut raw = Psbt::from_unsigned_tx(psbt.psbt().unsigned_tx.clone())
        .map_err(|err| UnifiedSigningError::PsbtConstruction(err.to_string()))?;
    for (destination, source) in raw.inputs.iter_mut().zip(&psbt.psbt().inputs) {
        destination.non_witness_utxo = source.non_witness_utxo.clone();
        destination.witness_utxo = source.witness_utxo.clone();
        destination.witness_script = source.witness_script.clone();
        destination.bip32_derivation = source.bip32_derivation.clone();
    }
    UnifiedPsbt::from_psbt(raw).map_err(Into::into)
}

fn validate_signing_key(
    signer: &MasterSigner,
    secp: &secp256k1::Secp256k1<secp256k1::All>,
    input_index: usize,
    input: &miniscript::bitcoin::psbt::Input,
    context: &InputContext,
    public_key: PublicKey,
    path: &DerivationPath,
) -> Result<(), UnifiedSigningError> {
    if path.len() > usize::from(u8::MAX) {
        return Err(UnifiedSigningError::DerivationPathTooDeep {
            input: input_index,
            public_key,
            depth: path.len(),
        });
    }
    let derived = signer.xpriv_at(path, secp).to_priv().public_key(secp);
    if derived != public_key {
        return Err(UnifiedSigningError::DerivedPublicKeyMismatch {
            input: input_index,
            public_key,
        });
    }
    if !script_uses_key(&context.miniscript, &public_key) {
        return Err(UnifiedSigningError::KeyNotInWitnessScript {
            input: input_index,
            public_key,
        });
    }
    require_compatible_sighash(input_index, input.sighash_type)?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
