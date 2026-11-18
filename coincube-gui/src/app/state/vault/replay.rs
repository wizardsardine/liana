//! Replay-protection model for a Bitcoin Blake2b Vault spend (`#276` I3/I4,
//! desktop plan PR 6 with the launch-GA corrections).
//!
//! Everything here is keyed on [`ChainId::is_blake2b`]. On a Bitcoin-family
//! Cube nothing in this module runs: signing, merging and readiness are the
//! prior code paths byte for byte (pinned by `bitcoin_paths_are_unchanged`).
//!
//! The replay statement a spend shows is derived from **one** source: the
//! core finaliser's per-input report — verified signatures that actually make
//! up the witness — never from which signers were asked, what the PSBT claims,
//! or a capability flag (`#276` correction 1). The four states:
//!
//! - *This witness is replay protected*: every input's witness carries at least
//!   one verified unified signature, and the locally retained legacy set cannot
//!   independently satisfy it. External legacy copies remain outside this
//!   guarantee.
//! - *Replayable*: at least one input would be finalised from legacy
//!   signatures alone. Amber; broadcasting needs the explicit acknowledgement
//!   [`REPLAYABLE_ACKNOWLEDGEMENT`] — **unless** one of those inputs spends a
//!   deposit Connect has positively confirmed also exists on Bitcoin (`#276`
//!   I13). Such an input *requires* a replay-capable signature (or, once Lane
//!   B1.5 exists, a poison split first); no acknowledgement clears it
//!   ([`blocked_entangled_inputs`]). An input whose entanglement is *Unknown*
//!   is amber and acknowledgeable, never blocked — a lookup that has not
//!   happened is not evidence either way.
//! - *Split — cannot replay*: the spend's inputs include a verified split key
//!   (a coin whose ancestry exists only on this chain), so Bitcoin rejects the
//!   whole transaction; set by [`ReplayReview::with_split_key`].
//! - *Unknown / not yet checked*: no signatures, not enough of them, or the
//!   verifier refused the PSBT.

use std::collections::BTreeSet;

use coincube_core::{
    descriptors::{CoincubeDescError, PartialSpendInfo},
    miniscript::bitcoin::{ecdsa, psbt::Psbt, secp256k1, sighash::EcdsaSighashType, Txid},
    psbt_unified::{unified_signatures, UnifiedPsbt},
    unified_finalize::{finalize_p2wsh_all_unified, UnifiedFinalizeError},
};

use crate::{chain::ChainId, services::entangled::Entanglement};
use coincube_ui::{locale::copy as localized_copy, localized_format};

/// The acknowledgement a user gives before broadcasting a replayable spend
/// (`#276` I4). Verbatim from the lane brief.
pub const REPLAYABLE_ACKNOWLEDGEMENT: &str = "I understand this can also spend my Bitcoin";

/// The exact-witness limit of [`ReplayStatus::Protected`]. Coincube can bound
/// signatures retained in this PSBT, but cannot revoke copies held elsewhere.
pub const PROTECTED_LIMITATION: &str =
    "This status applies only to the witness Coincube will broadcast. Legacy signatures held by \
     another device, exported copy, or coordinator may still be combined into a Bitcoin spend; \
     Coincube cannot revoke them.";

pub const UNSAFE_LEGACY_POLICY_COPY: &str =
    "Coincube refused this Bitcoin Blake2b PSBT because its retained legacy signatures can \
     independently form a Bitcoin-valid witness. Remove the surplus legacy signatures or create \
     a new spend. Signatures already held by another device, exported copy, or coordinator cannot \
     be revoked.";

pub fn unsafe_legacy_policy_copy(error: &UnifiedFinalizeError) -> Option<&'static str> {
    matches!(error, UnifiedFinalizeError::UnsafeLegacyAlternative { .. })
        .then_some(UNSAFE_LEGACY_POLICY_COPY)
}

/// Why a spend's replay status is not known yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnknownReason {
    /// No signature of any kind has been collected.
    NotYetChecked,
    /// Signatures exist but at least one input cannot be satisfied yet.
    Incomplete,
    /// The unified verifier or finaliser refused the PSBT; the text is the
    /// typed error's `Display`.
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayStatus {
    Unknown(UnknownReason),
    Protected,
    /// Inputs whose witness would hold legacy signatures only.
    Replayable {
        inputs: Vec<usize>,
    },
    Split,
}

impl ReplayStatus {
    /// Whether the finaliser would produce a transaction from the current
    /// signatures — the BTCB2 notion of "ready to broadcast".
    pub fn is_finalisable(&self) -> bool {
        matches!(
            self,
            Self::Protected | Self::Replayable { .. } | Self::Split
        )
    }

    /// Whether broadcasting needs the user's replay acknowledgement.
    pub fn needs_acknowledgement(&self) -> bool {
        matches!(self, Self::Replayable { .. })
    }
}

/// Derive the replay status of a Bitcoin Blake2b spend from its signatures.
///
/// Callers must gate on the chain: this is meaningless — and never shown —
/// for a Bitcoin-family Cube.
pub fn replay_status(
    psbt: &Psbt,
    secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
) -> ReplayStatus {
    let unified = match UnifiedPsbt::from_psbt(psbt.clone()) {
        Ok(unified) => unified,
        Err(e) => return ReplayStatus::Unknown(UnknownReason::Refused(e.to_string())),
    };
    let has_any_signature = unified.psbt().inputs.iter().any(|input| {
        !input.partial_sigs.is_empty()
            || !input.tap_script_sigs.is_empty()
            || input.tap_key_sig.is_some()
    }) || unified_signatures(&unified)
        .map(|records| !records.is_empty())
        .unwrap_or(false);
    if !has_any_signature {
        return ReplayStatus::Unknown(UnknownReason::NotYetChecked);
    }
    if let Err(error) =
        coincube_core::unified_finalize::ensure_no_unsafe_legacy_alternative(&unified, secp)
    {
        return ReplayStatus::Unknown(UnknownReason::Refused(error.to_string()));
    }
    match finalize_p2wsh_all_unified(&unified, secp) {
        Ok(finalized) => {
            let inputs: Vec<usize> = finalized
                .inputs
                .iter()
                .enumerate()
                .filter(|(_, report)| !report.replay_protected())
                .map(|(index, _)| index)
                .collect();
            if inputs.is_empty() {
                ReplayStatus::Protected
            } else {
                ReplayStatus::Replayable { inputs }
            }
        }
        Err(UnifiedFinalizeError::Unsatisfiable { .. }) => {
            ReplayStatus::Unknown(UnknownReason::Incomplete)
        }
        Err(e) => ReplayStatus::Unknown(UnknownReason::Refused(e.to_string())),
    }
}

/// Why a Bitcoin Blake2b PSBT must not be handed to any signer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchRefused {
    /// An input asks for, or already carries, an `ANYONECANPAY` sighash.
    /// `ANYONECANPAY` never occurs in a Vault flow, and on Bitcoin Blake2b it
    /// is refused outright rather than warned about (a unified `ANYONECANPAY`
    /// would commit to fewer inputs than the replay model reasons over).
    AnyoneCanPay { input: usize, sighash: u32 },
    /// A reserved unified record (`coincube`/0 proprietary entry) the adapter
    /// rejects — wrong trailing sighash byte (`0xa1` included), non-strict
    /// DER, a key that also has a legacy signature. Such a PSBT would be
    /// refused by every merge and by the finaliser anyway; refusing it here
    /// keeps "before any signer is dispatched" true, so no device or phone is
    /// prompted for a signature that would be thrown away.
    MalformedUnifiedRecord(String),
    /// An input carries Taproot signature data (`tap_key_sig` or
    /// `tap_script_sigs`). A Bitcoin Blake2b Vault is native P2WSH, so this
    /// is not a PSBT for it; and Taproot signatures carry their own sighash
    /// byte, which the ECDSA walk below cannot vet.
    TaprootSignatureData { input: usize },
}

impl std::fmt::Display for DispatchRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnyoneCanPay { input, sighash } => write!(
                f,
                "input {} asks for sighash 0x{:02x} (ANYONECANPAY), which a Bitcoin Blake2b \
                 Vault spend never uses; refusing to sign",
                input, sighash
            ),
            Self::MalformedUnifiedRecord(reason) => write!(
                f,
                "the PSBT carries a unified signature record no signer could add to: {}; \
                 refusing to sign",
                reason
            ),
            Self::TaprootSignatureData { input } => write!(
                f,
                "input {} carries Taproot signature data, which a Bitcoin Blake2b Vault spend \
                 never has; refusing to sign",
                input
            ),
        }
    }
}

/// Refuse, before any signer — local, device or Keychain — is dispatched on
/// Bitcoin Blake2b, a PSBT that asks for or carries an `ANYONECANPAY`
/// sighash on any input, that carries a reserved unified record the adapter
/// rejects, or that carries Taproot signature data at all (a Blake2b Vault
/// is P2WSH; Taproot signatures bring their own sighash byte).
pub fn refuse_before_dispatch(psbt: &Psbt) -> Result<(), DispatchRefused> {
    // The specific refusals first, so the user is told "ANYONECANPAY" or
    // "Taproot" rather than the adapter's generic wording; then the adapter's
    // own validation — the same check every merge and the finaliser apply —
    // which also covers the reserved records' sighash byte and the sighash
    // request rule the walk below does not restate.
    const ANYONECANPAY: u32 = 0x80;
    for (index, input) in psbt.inputs.iter().enumerate() {
        if input.tap_key_sig.is_some() || !input.tap_script_sigs.is_empty() {
            return Err(DispatchRefused::TaprootSignatureData { input: index });
        }
        if let Some(sighash) = input.sighash_type {
            let raw = sighash.to_u32();
            if raw & ANYONECANPAY != 0 {
                return Err(DispatchRefused::AnyoneCanPay {
                    input: index,
                    sighash: raw,
                });
            }
        }
        for signature in input.partial_sigs.values() {
            let raw = signature.sighash_type.to_u32();
            if raw & ANYONECANPAY != 0 {
                return Err(DispatchRefused::AnyoneCanPay {
                    input: index,
                    sighash: raw,
                });
            }
        }
    }
    UnifiedPsbt::from_psbt(psbt.clone())
        .map_err(|e| DispatchRefused::MalformedUnifiedRecord(e.to_string()))?;
    Ok(())
}

/// `CoincubeDescriptor::partial_spend_info` keyed on the chain.
///
/// The descriptor analysis reads `partial_sigs` only, so on Bitcoin Blake2b a
/// unified signature — stored in the proprietary records — would not count
/// toward the path threshold and the picker would never show its signer as
/// done. For BTCB2 the analysis runs over a **counting projection**: a clone
/// of the PSBT where each unified record is mirrored into `partial_sigs` as
/// its DER part with `SIGHASH_ALL`. The projection exists for this call only;
/// it is never persisted, exported or signed (a key with both encodings is
/// exactly what the adapter refuses). On every other chain this is the
/// unchanged analysis of the unchanged PSBT.
pub fn spend_info_for_chain(
    chain: ChainId,
    descriptor: &impl coincube_core::wallet_descriptor::SpendAnalysis,
    psbt: &Psbt,
) -> Result<PartialSpendInfo, CoincubeDescError> {
    if !chain.is_blake2b() {
        return descriptor.spend_info(psbt);
    }
    descriptor.spend_info(&counting_projection(psbt))
}

/// See [`spend_info_for_chain`]. Falls back to the PSBT as-is when the
/// proprietary records do not parse (the analysis then simply does not count
/// them, which errs toward "not signed yet").
pub(crate) fn counting_projection(psbt: &Psbt) -> Psbt {
    let mut projected = psbt.clone();
    let Ok(unified) = UnifiedPsbt::from_psbt(psbt.clone()) else {
        return projected;
    };
    let Ok(records) = unified_signatures(&unified) else {
        return projected;
    };
    for record in records {
        let Some((_, der)) = record.signature.split_last() else {
            continue;
        };
        let Ok(signature) = secp256k1::ecdsa::Signature::from_der(der) else {
            continue;
        };
        if let Some(input) = projected.inputs.get_mut(record.input_index) {
            input
                .partial_sigs
                .entry(record.public_key)
                .or_insert(ecdsa::Signature {
                    signature,
                    sighash_type: EcdsaSighashType::All,
                });
        }
    }
    projected
}

/// What the spend screen shows and gates on for a Bitcoin Blake2b spend.
/// `None` on every other chain — the screen then renders exactly as before.
///
/// Holds the (verified, therefore not free) status; entanglement of the
/// inputs is a cache lookup and is resolved at render time with
/// [`entangled_inputs`], so a lookup landing after a signature still shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayReview {
    status: ReplayStatus,
    /// The input that spends a split key: a coin whose verified ancestry
    /// proves it exists only on this chain, so Bitcoin rejects the whole
    /// transaction whatever its signatures. Set only while the spend screen
    /// holds a fresh re-qualification of the key
    /// ([`crate::app::state::vault::psbt::PsbtState::current_review`]).
    split_key: Option<(usize, coincube_core::miniscript::bitcoin::OutPoint)>,
    /// SHA-256 of the serialised PSBT the status was derived from: the exact
    /// signatures the user is looking at.
    psbt_digest: [u8; 32],
    /// The PSBT digest the user ticked [`REPLAYABLE_ACKNOWLEDGEMENT`] for, if
    /// any. Keyed to **content**, not to the status: a new signature can leave
    /// `Replayable { inputs: [0] }` unchanged while the PSBT changed, and an
    /// acknowledgement must never carry across a signature the user did not
    /// see. Keying to content also means a recompute with no new signature
    /// (every Keychain stream event emits one) keeps the tick.
    acknowledged_for: Option<[u8; 32]>,
}

impl ReplayReview {
    pub fn new(psbt: &Psbt, secp: &secp256k1::Secp256k1<impl secp256k1::Verification>) -> Self {
        Self {
            status: replay_status(psbt, secp),
            split_key: None,
            psbt_digest: psbt_digest(psbt),
            acknowledged_for: None,
        }
    }

    /// Views and dispatch gates must use this accessor on every read.
    pub fn status(&self) -> ReplayStatus {
        if self.split_key.is_some() && matches!(self.status, ReplayStatus::Replayable { .. }) {
            // Finalisable with legacy signatures, but it spends a coin
            // Bitcoin doesn't have: Bitcoin rejects it.
            ReplayStatus::Split
        } else {
            self.status.clone()
        }
    }

    /// The input marked as spending a split key, if any.
    pub fn split_key(&self) -> Option<(usize, coincube_core::miniscript::bitcoin::OutPoint)> {
        self.split_key
    }

    /// Mark the input (index, outpoint) that spends a verified split key.
    pub fn with_split_key(
        mut self,
        input: Option<(usize, coincube_core::miniscript::bitcoin::OutPoint)>,
    ) -> Self {
        self.split_key = input;
        self
    }

    /// Recompute for the current PSBT, carrying an acknowledgement over
    /// **only** if the PSBT is byte-identical to the one it was given for.
    pub fn refreshed(
        &self,
        psbt: &Psbt,
        secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
    ) -> Self {
        let mut next = Self::new(psbt, secp);
        // The same transaction spends the same key; only signatures change.
        next.split_key = self.split_key.filter(|(index, outpoint)| {
            psbt.unsigned_tx
                .input
                .get(*index)
                .is_some_and(|input| input.previous_output == *outpoint)
        });
        if self.acknowledged_for == Some(next.psbt_digest) {
            next.acknowledged_for = self.acknowledged_for;
        }
        next
    }

    /// Whether the user has acknowledged *these* signatures.
    pub fn acknowledged(&self) -> bool {
        self.acknowledged_for == Some(self.psbt_digest)
    }

    /// Record or clear the acknowledgement for the PSBT this review is of.
    pub fn set_acknowledged(&mut self, acknowledged: bool) {
        self.acknowledged_for = acknowledged.then_some(self.psbt_digest);
    }

    /// The digest of the PSBT this review describes.
    pub fn psbt_digest(&self) -> [u8; 32] {
        self.psbt_digest
    }

    /// Whether the signatures collected are enough: the finaliser would
    /// produce a transaction **and** no input the user is required to protect
    /// ([`blocked_entangled_inputs`]) is left replayable. This is what closes
    /// the signing picker on BTCB2 — a blocked input keeps it open so the
    /// replay-capable signature can be added. `entangled` is
    /// [`entangled_inputs`] resolved from the cache at the time of the check.
    pub fn signatures_complete(&self, entangled: &[(usize, Entanglement)]) -> bool {
        let status = self.status();
        status.is_finalisable() && blocked_entangled_inputs(&status, entangled).is_empty()
    }

    /// Whether the spend may be broadcast: [`Self::signatures_complete`], and
    /// acknowledged when it is replayable. The acknowledgement never
    /// substitutes for a required signature on a known-entangled input.
    pub fn broadcast_ready(&self, entangled: &[(usize, Entanglement)]) -> bool {
        let status = self.status();
        status.is_finalisable()
            && blocked_entangled_inputs(&status, entangled).is_empty()
            && (!status.needs_acknowledgement() || self.acknowledged())
    }
}

/// SHA-256 of the serialised PSBT — the identity of a set of signatures.
pub fn psbt_digest(psbt: &Psbt) -> [u8; 32] {
    use coincube_core::miniscript::bitcoin::hashes::{sha256, Hash};
    sha256::Hash::hash(&psbt.serialize()).to_byte_array()
}

/// Inputs (by index) that are replayable **and** spend a deposit Connect has
/// positively confirmed on the twin chain ([`Entanglement::Entangled`]). These
/// require a replay-capable signature in the witness (`#276` I13); the
/// acknowledgement does not apply to them. `Unknown` inputs are never in this
/// set: gating on an unanswered lookup would block every BTCB2 spend until a
/// sync completed, and "never reads as not entangled" asks for the honest
/// amber, not a hard stop.
pub fn blocked_entangled_inputs(
    status: &ReplayStatus,
    entangled: &[(usize, Entanglement)],
) -> Vec<usize> {
    let ReplayStatus::Replayable { inputs } = status else {
        return Vec::new();
    };
    let confirmed: BTreeSet<usize> = entangled
        .iter()
        .filter(|(_, status)| matches!(status, Entanglement::Entangled))
        .map(|(index, _)| *index)
        .collect();
    inputs
        .iter()
        .copied()
        .filter(|index| confirmed.contains(index))
        .collect()
}

/// Inputs (by index) that spend an entangled deposit, or one whose
/// entanglement is not known — `lookup` answers from the cache and returns
/// [`Entanglement::Unknown`] for a txid it has no answer for. These are the
/// inputs the replayable copy names as also existing on Bitcoin; an input
/// known *not* to be entangled is left out. Unknown is deliberately kept: a
/// lookup that has not happened is not evidence of safety.
pub fn entangled_inputs(
    psbt: &Psbt,
    lookup: impl Fn(&Txid) -> Entanglement,
) -> Vec<(usize, Entanglement)> {
    psbt.unsigned_tx
        .input
        .iter()
        .enumerate()
        .filter_map(|(index, txin)| match lookup(&txin.previous_output.txid) {
            Entanglement::NotEntangled => None,
            other => Some((index, other)),
        })
        .collect()
}

/// Whether the Broadcast action is available. One definition for the state
/// (broadcast handler) and the view (button), so they cannot disagree: on a
/// Bitcoin-family Cube it is the path threshold as before (`entangled` is
/// ignored — it is always empty there); on Bitcoin Blake2b it is the
/// finaliser's verdict, the I13 requirement on known-entangled inputs, and
/// the acknowledgement. `entangled` must be resolved from the cache at the
/// point of the check, so a lookup that lands after the last signature
/// tightens the gate instead of being missed by it.
pub fn broadcast_ready(
    path_ready: bool,
    review: Option<&ReplayReview>,
    entangled: &[(usize, Entanglement)],
) -> bool {
    match review {
        None => path_ready,
        Some(review) => review.broadcast_ready(entangled),
    }
}

/// Shown while the spend screen's entanglement re-check is in flight.
pub const CHECKING_COPY: &str =
    "Checking whether these coins also exist on Bitcoin before this can be sent…";

/// Why a spend that looked ready is not — for the toast shown when the
/// Broadcast dialog is closed under the user because the gate moved (a
/// lookup landed, a re-check started, a signature changed). Composed only
/// from the copy the spend screen already shows.
pub fn not_ready_reason(
    review: &ReplayReview,
    entangled: &[(usize, Entanglement)],
    checking: bool,
) -> String {
    if checking {
        return localized_copy(CHECKING_COPY).to_string();
    }
    let blocked = blocked_entangled_inputs(&review.status(), entangled);
    if let Some(required) = blocked_entangled_copy(&blocked) {
        return required;
    }
    let (label, _) = pill_copy(&review.status(), entangled);
    if review.status().needs_acknowledgement() && !review.acknowledged() {
        return localized_format!(
            "{}. Tick \"{}\" to send.",
            "{}. Marca «{}» para enviar.",
            label,
            localized_copy(REPLAYABLE_ACKNOWLEDGEMENT)
        );
    }
    label
}

/// The remedy line shown under the pill when [`blocked_entangled_inputs`] is
/// non-empty: a replay-capable signature (the Cube key or a Border Wallet
/// key), or splitting the coins first in Coin control (Lane B1.5), which
/// the view offers as **Split first**.
pub fn blocked_entangled_copy(blocked: &[usize]) -> Option<String> {
    if blocked.is_empty() {
        return None;
    }
    let list = blocked
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let (noun, verb, pronoun) = if blocked.len() == 1 {
        (
            localized_copy("Input"),
            localized_copy("exists"),
            localized_copy("it"),
        )
    } else {
        (
            localized_copy("Inputs"),
            localized_copy("exist"),
            localized_copy("them"),
        )
    };
    Some(localized_format!(
        "{noun} {list} also {verb} on Bitcoin, so this cannot be sent without a replay-capable \
         signature on {pronoun} (the Cube key or a Border Wallet key). If enough legacy \
         signatures were already collected to form a Bitcoin witness, create a new spend and \
         collect the replay-capable signature first; Coincube refuses to retain both complete \
         alternatives. Or split the coins first in Coin control.",
        "{noun} {list} también {verb} en Bitcoin. Para enviar se necesita una firma protegida contra repetición en {pronoun}, con la clave del Cube o de Border Wallet. Si ya hay suficientes firmas antiguas para formar un testigo válido en Bitcoin, crea un gasto nuevo y recopila primero la firma protegida. Coincube rechaza conservar ambas alternativas completas. También puedes separar primero las monedas en Control de monedas."
    ))
}

/// Visual weight of the status pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillTone {
    Success,
    Warning,
    Neutral,
}

/// The pill's label and tone. Pure so the copy is testable without a widget
/// tree; the view only lays it out. `entangled` is [`entangled_inputs`] for
/// the same PSBT.
pub fn pill_copy(status: &ReplayStatus, entangled: &[(usize, Entanglement)]) -> (String, PillTone) {
    match status {
        ReplayStatus::Protected => (
            localized_copy("This witness is replay protected").to_owned(),
            PillTone::Success,
        ),
        ReplayStatus::Split => (
            localized_copy("Split — cannot replay").to_owned(),
            PillTone::Success,
        ),
        ReplayStatus::Replayable { inputs } => {
            let blocked: BTreeSet<usize> = blocked_entangled_inputs(status, entangled)
                .into_iter()
                .collect();
            let entangled: BTreeSet<usize> = entangled.iter().map(|(index, _)| *index).collect();
            let list = inputs
                .iter()
                .map(|index| {
                    if blocked.contains(index) {
                        localized_format!(
                            "{index} (also exists on Bitcoin — signature required)",
                            "{index} (también existe en Bitcoin · requiere firma)"
                        )
                    } else if entangled.contains(index) {
                        localized_format!(
                            "{index} (not yet checked against Bitcoin)",
                            "{index} (aún sin comprobar en Bitcoin)"
                        )
                    } else {
                        index.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            let noun = localized_copy(if inputs.len() == 1 { "input" } else { "inputs" });
            (
                localized_format!(
                    "Replayable — no replay-capable signature on {noun} {list}",
                    "Repetible · sin firma protegida contra repetición en {noun} {list}"
                ),
                PillTone::Warning,
            )
        }
        ReplayStatus::Unknown(UnknownReason::NotYetChecked) => (
            localized_copy("Not yet checked").to_owned(),
            PillTone::Neutral,
        ),
        ReplayStatus::Unknown(UnknownReason::Incomplete) => (
            localized_copy("Unknown — not enough signatures to check yet").to_owned(),
            PillTone::Neutral,
        ),
        ReplayStatus::Unknown(UnknownReason::Refused(reason)) => (
            localized_format!("Unknown — {reason}", "Desconocido · {reason}"),
            PillTone::Neutral,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::vault::test_support::unified::{
        fixture, legacy, two_input_fixture, unified,
    };
    use coincube_core::miniscript::bitcoin::{ecdsa, sighash::EcdsaSighashType};

    fn secp() -> secp256k1::Secp256k1<secp256k1::VerifyOnly> {
        secp256k1::Secp256k1::verification_only()
    }

    #[test]
    fn ordinary_signatures_do_not_create_split_evidence() {
        let f = fixture();
        let signed = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
        assert!(!matches!(
            replay_status(&signed, &secp()),
            ReplayStatus::Split
        ));
        assert_eq!(
            pill_copy(&ReplayStatus::Split, &[]),
            ("Split — cannot replay".to_string(), PillTone::Success)
        );
        assert!(ReplayStatus::Split.is_finalisable());
        assert!(!ReplayStatus::Split.needs_acknowledgement());
    }

    /// A spend through a verified split key is split, not replayable: Bitcoin
    /// has no such coin. The mark follows the same input across refreshes and
    /// is dropped if the transaction no longer spends it.
    #[test]
    fn a_split_key_input_makes_a_legacy_spend_split() {
        let f = fixture();
        let secp = secp();
        let signed = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
        let key = (0, signed.unsigned_tx.input[0].previous_output);
        let plain = ReplayReview::new(&signed, &secp);
        assert!(matches!(plain.status(), ReplayStatus::Replayable { .. }));
        let review = plain.with_split_key(Some(key));
        assert_eq!(review.status(), ReplayStatus::Split);
        assert!(review.broadcast_ready(&[(0, Entanglement::Entangled)]));
        assert_eq!(
            review.refreshed(&signed, &secp).status(),
            ReplayStatus::Split
        );
        let mut other = signed.clone();
        other.unsigned_tx.input[0].previous_output.vout += 1;
        assert!(matches!(
            review.refreshed(&other, &secp).status(),
            ReplayStatus::Unknown(_) | ReplayStatus::Replayable { .. }
        ));
        // Unsigned stays unchecked: the key never manufactures a status.
        assert!(matches!(
            ReplayReview::new(&f.psbt, &secp)
                .with_split_key(Some(key))
                .status(),
            ReplayStatus::Unknown(_)
        ));
    }

    #[test]
    fn status_matrix_from_verified_signatures() {
        let f = fixture();
        let secp = secp();

        assert_eq!(
            replay_status(&f.psbt, &secp),
            ReplayStatus::Unknown(UnknownReason::NotYetChecked)
        );
        let one = unified(&f.psbt, &f.signers[0]);
        assert_eq!(
            replay_status(&one, &secp),
            ReplayStatus::Unknown(UnknownReason::Incomplete)
        );
        let two = unified(&one, &f.signers[1]);
        assert_eq!(replay_status(&two, &secp), ReplayStatus::Protected);

        // Legacy signatures alone finalise but are replayable.
        let legacy_only = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
        assert_eq!(
            replay_status(&legacy_only, &secp),
            ReplayStatus::Replayable { inputs: vec![0] }
        );
        // One unified + one legacy (different keys): protected — a legacy
        // signature alongside is fine.
        let mixed = legacy(&one, &f.signers[1]);
        assert_eq!(replay_status(&mixed, &secp), ReplayStatus::Protected);
        // Unified on the *third* key with two legacy ones: the finaliser would
        // keep the unified signature, but the retained legacy pair can also
        // satisfy the input. Local policy refuses the alternate set instead of
        // displaying a green status.
        let third = legacy(
            &legacy(&unified(&f.psbt, &f.signers[2]), &f.signers[0]),
            &f.signers[1],
        );
        assert!(matches!(
            replay_status(&third, &secp),
            ReplayStatus::Unknown(UnknownReason::Refused(reason))
                if reason.contains("independently satisfy")
        ));
    }

    #[test]
    fn a_refused_psbt_is_unknown_with_the_reason_never_protected() {
        let f = fixture();
        let secp = secp();
        // Tamper with a unified record: the verifier refuses the whole PSBT.
        let mut tampered = unified(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
        let key = tampered.inputs[0]
            .proprietary
            .keys()
            .next()
            .cloned()
            .unwrap();
        let value = tampered.inputs[0].proprietary.get_mut(&key).unwrap();
        value[10] ^= 0x01;
        match replay_status(&tampered, &secp) {
            ReplayStatus::Unknown(UnknownReason::Refused(reason)) => {
                assert!(reason.contains("unified"), "{}", reason)
            }
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn anyonecanpay_is_refused_before_signing_and_in_status() {
        let f = fixture();
        assert_eq!(refuse_before_dispatch(&f.psbt), Ok(()));

        // Requested on the input.
        let mut asked = f.psbt.clone();
        asked.inputs[0].sighash_type = Some(EcdsaSighashType::AllPlusAnyoneCanPay.into());
        assert_eq!(
            refuse_before_dispatch(&asked),
            Err(DispatchRefused::AnyoneCanPay {
                input: 0,
                sighash: 0x81
            })
        );
        assert!(refuse_before_dispatch(&asked)
            .unwrap_err()
            .to_string()
            .contains("ANYONECANPAY"));

        // Already carried by a signature: refused before signing, and the
        // status never reads it as protected or replayable — the finaliser
        // refuses it too.
        let mut carried = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
        let (pk, sig) = carried.inputs[0]
            .partial_sigs
            .iter()
            .map(|(pk, sig)| (*pk, *sig))
            .next()
            .unwrap();
        carried.inputs[0].partial_sigs.insert(
            pk,
            ecdsa::Signature {
                signature: sig.signature,
                sighash_type: EcdsaSighashType::AllPlusAnyoneCanPay,
            },
        );
        assert!(matches!(
            refuse_before_dispatch(&carried),
            Err(DispatchRefused::AnyoneCanPay { input: 0, .. })
        ));
        assert!(matches!(
            replay_status(&carried, &secp()),
            ReplayStatus::Unknown(UnknownReason::Refused(_))
        ));
    }

    /// Taproot signature data never belongs to a Blake2b Vault PSBT, and its
    /// own sighash byte is not something the ECDSA walk vets: refused at the
    /// dispatch boundary whatever its sighash says.
    #[test]
    fn taproot_signature_data_is_refused_before_dispatch() {
        use coincube_core::miniscript::bitcoin::{
            key::Secp256k1, secp256k1::SecretKey, taproot, TapSighashType, XOnlyPublicKey,
        };
        let f = fixture();
        let secp = Secp256k1::new();
        let secret = SecretKey::from_slice(&[5u8; 32]).unwrap();
        let keypair = secp256k1::Keypair::from_secret_key(&secp, &secret);
        let (xonly, _) = XOnlyPublicKey::from_keypair(&keypair);
        let signature =
            secp.sign_schnorr_no_aux_rand(&secp256k1::Message::from_digest([4u8; 32]), &keypair);
        for sighash in [TapSighashType::All, TapSighashType::AllPlusAnyoneCanPay] {
            let mut with_key_sig = f.psbt.clone();
            with_key_sig.inputs[0].tap_key_sig = Some(taproot::Signature {
                signature,
                sighash_type: sighash,
            });
            assert_eq!(
                refuse_before_dispatch(&with_key_sig),
                Err(DispatchRefused::TaprootSignatureData { input: 0 }),
                "{:?}",
                sighash
            );
            let mut with_script_sig = f.psbt.clone();
            with_script_sig.inputs[0].tap_script_sigs.insert(
                (
                    xonly,
                    coincube_core::miniscript::bitcoin::TapLeafHash::from_script(
                        &coincube_core::miniscript::bitcoin::ScriptBuf::new(),
                        coincube_core::miniscript::bitcoin::taproot::LeafVersion::TapScript,
                    ),
                ),
                taproot::Signature {
                    signature,
                    sighash_type: sighash,
                },
            );
            assert_eq!(
                refuse_before_dispatch(&with_script_sig),
                Err(DispatchRefused::TaprootSignatureData { input: 0 }),
                "{:?}",
                sighash
            );
        }
        assert_eq!(refuse_before_dispatch(&f.psbt), Ok(()));
    }

    /// A reserved unified record with an `ANYONECANPAY` sighash byte (`0xa1`)
    /// is not a `partial_sigs` entry and not the input's sighash field, so the
    /// plain walk would let it through to a device; adapter validation at the
    /// dispatch boundary refuses it. Not a funds risk — every merge and the
    /// finaliser reject such a record too — but a signer must not be prompted
    /// for a signature that will be thrown away.
    #[test]
    fn a_malformed_reserved_record_is_refused_before_dispatch() {
        let f = fixture();
        let signed = unified(&f.psbt, &f.signers[0]);
        assert_eq!(refuse_before_dispatch(&signed), Ok(()));
        let mut malformed = signed.clone();
        let key = malformed.inputs[0]
            .proprietary
            .keys()
            .next()
            .cloned()
            .unwrap();
        let record = malformed.inputs[0].proprietary.get_mut(&key).unwrap();
        *record.last_mut().unwrap() = 0xa1;
        match refuse_before_dispatch(&malformed) {
            Err(DispatchRefused::MalformedUnifiedRecord(reason)) => {
                assert!(
                    reason.contains("a1") || reason.contains("sighash"),
                    "{}",
                    reason
                )
            }
            other => panic!(
                "expected the reserved record to be refused, got {:?}",
                other
            ),
        }
        assert!(refuse_before_dispatch(&malformed)
            .unwrap_err()
            .to_string()
            .contains("refusing to sign"));
    }

    #[test]
    fn spend_info_counts_unified_records_on_blake2b_only() {
        let f = fixture();
        let two = unified(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
        assert!(two.inputs[0].partial_sigs.is_empty());

        let blake2b = spend_info_for_chain(ChainId::BitcoinBlake2b, &f.descriptor, &two).unwrap();
        assert_eq!(blake2b.primary_path().sigs_count, 2);
        assert_eq!(blake2b.primary_path().signed_pubkeys.len(), 2);

        // Bitcoin: the unchanged analysis of the unchanged PSBT.
        let bitcoin = spend_info_for_chain(ChainId::Bitcoin, &f.descriptor, &two).unwrap();
        assert_eq!(bitcoin.primary_path().sigs_count, 0);
        assert_eq!(bitcoin, f.descriptor.partial_spend_info(&two).unwrap());

        // The projection is a scratch copy: the PSBT handed in is untouched
        // and still carries no `partial_sigs`.
        let projected = counting_projection(&two);
        assert_eq!(projected.inputs[0].partial_sigs.len(), 2);
        assert!(two.inputs[0].partial_sigs.is_empty());
        assert_eq!(projected.inputs[0].proprietary, two.inputs[0].proprietary);
    }

    #[test]
    fn broadcast_ready_needs_the_acknowledgement_only_when_replayable() {
        let f = fixture();
        let secp = secp();
        let none: &[(usize, Entanglement)] = &[];
        // Bitcoin family: the path threshold, whatever it says — and whatever
        // the entangled set says.
        assert!(broadcast_ready(true, None, none));
        assert!(!broadcast_ready(false, None, none));
        assert!(broadcast_ready(true, None, &[(0, Entanglement::Entangled)]));

        let protected = ReplayReview::new(
            &unified(&unified(&f.psbt, &f.signers[0]), &f.signers[1]),
            &secp,
        );
        assert_eq!(protected.status, ReplayStatus::Protected);
        assert!(protected.broadcast_ready(none));
        // The path count is irrelevant on BTCB2: verified witness decides.
        assert!(broadcast_ready(false, Some(&protected), none));

        let mut replayable = ReplayReview::new(
            &legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]),
            &secp,
        );
        assert!(replayable.status.needs_acknowledgement());
        assert!(!replayable.broadcast_ready(none));
        replayable.set_acknowledged(true);
        assert!(replayable.broadcast_ready(none));

        let unknown = ReplayReview::new(&f.psbt, &secp);
        assert!(!unknown.broadcast_ready(none));
        assert!(!broadcast_ready(true, Some(&unknown), none));
    }

    /// `#276` I13: a replayable input that Connect has confirmed also exists
    /// on Bitcoin *requires* a replay-capable signature. The acknowledgement
    /// never clears it; an unanswered lookup never imposes it.
    #[test]
    fn a_known_entangled_replayable_input_cannot_be_acknowledged_away() {
        let f = fixture();
        let secp = secp();
        let legacy_only = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
        let mut review = ReplayReview::new(&legacy_only, &secp);
        assert_eq!(review.status, ReplayStatus::Replayable { inputs: vec![0] });
        let entangled = [(0, Entanglement::Entangled)];

        // 1. Known entangled + legacy-only: not ready, and still not ready
        //    after the tick — the check that distinguishes a requirement
        //    from a warning.
        assert_eq!(
            blocked_entangled_inputs(&review.status(), &entangled),
            vec![0]
        );
        assert!(!review.signatures_complete(&entangled));
        assert!(!review.broadcast_ready(&entangled));
        review.set_acknowledged(true);
        assert!(!review.broadcast_ready(&entangled));
        assert!(!broadcast_ready(true, Some(&review), &entangled));

        // 2. The same input with a verified unified signature in the witness
        //    is ready: the requirement is satisfiable in this build.
        let with_unified = legacy(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
        let protected = ReplayReview::new(&with_unified, &secp);
        assert_eq!(protected.status, ReplayStatus::Protected);
        assert!(blocked_entangled_inputs(&protected.status, &entangled).is_empty());
        assert!(protected.signatures_complete(&entangled));
        assert!(protected.broadcast_ready(&entangled));

        // 3. Unknown + replayable: unchanged amber, the tick still works —
        //    the gate must not over-block a spend nobody has looked up yet.
        let unknown = [(0, Entanglement::Unknown)];
        let mut review = ReplayReview::new(&legacy_only, &secp);
        assert!(blocked_entangled_inputs(&review.status(), &unknown).is_empty());
        assert!(review.signatures_complete(&unknown));
        assert!(!review.broadcast_ready(&unknown));
        review.set_acknowledged(true);
        assert!(review.broadcast_ready(&unknown));

        // 4. Not entangled + replayable: the unchanged acknowledgement path.
        let mut review = ReplayReview::new(&legacy_only, &secp);
        assert!(!review.broadcast_ready(&[]));
        review.set_acknowledged(true);
        assert!(review.broadcast_ready(&[]));
        // (`entangled_inputs` never yields NotEntangled entries, but the gate
        // ignores them if handed one.)
        assert!(review.broadcast_ready(&[(0, Entanglement::NotEntangled)]));

        // The requirement is on the witness, not on the coin: an entangled
        // input that is not replayable, or is not an input at all, is not
        // blocked.
        assert!(blocked_entangled_inputs(
            &ReplayStatus::Replayable { inputs: vec![1] },
            &[(0, Entanglement::Entangled)]
        )
        .is_empty());
        assert!(blocked_entangled_inputs(
            &ReplayStatus::Unknown(UnknownReason::Incomplete),
            &[(0, Entanglement::Entangled)]
        )
        .is_empty());
    }

    #[test]
    fn blocked_copy_names_the_input_and_the_honest_remedy() {
        assert_eq!(blocked_entangled_copy(&[]), None);
        let one = blocked_entangled_copy(&[0]).unwrap();
        assert!(one.starts_with("Input 0 also exists on Bitcoin"), "{}", one);
        assert!(one.contains("replay-capable signature on it"), "{}", one);
        assert!(one.contains("Cube key or a Border Wallet key"), "{}", one);
        assert!(one.contains("create a new spend"), "{}", one);
        assert!(
            one.ends_with("Or split the coins first in Coin control."),
            "{}",
            one
        );
        let two = blocked_entangled_copy(&[0, 2]).unwrap();
        assert!(
            two.starts_with("Inputs 0, 2 also exist on Bitcoin"),
            "{}",
            two
        );
        assert!(two.contains("signature on them"), "{}", two);
    }

    /// The acknowledgement is keyed to the PSBT's bytes: a recompute over the
    /// same bytes (a Keychain stream event emits one for every event) keeps
    /// it; any content change — even one that leaves the status enum equal —
    /// drops it; a brand-new review never has one.
    #[test]
    fn the_acknowledgement_follows_the_psbt_bytes_not_the_status() {
        let f = fixture();
        let secp = secp();
        let legacy_only = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
        let mut review = ReplayReview::new(&legacy_only, &secp);
        assert!(!review.acknowledged());
        review.set_acknowledged(true);
        assert!(review.acknowledged());

        // Same bytes: kept.
        let same = review.refreshed(&legacy_only, &secp);
        assert!(same.acknowledged());
        assert_eq!(same.status, review.status);

        // A third legacy signature: the status is still `Replayable { [0] }`
        // but the content changed, so the tick is gone.
        let three = legacy(&legacy_only, &f.signers[2]);
        let changed = review.refreshed(&three, &secp);
        assert_eq!(changed.status, review.status);
        assert!(!changed.acknowledged());
        assert_ne!(changed.psbt_digest(), review.psbt_digest());

        // Clearing works, and a fresh review starts unacknowledged.
        review.set_acknowledged(false);
        assert!(!review.acknowledged());
        assert!(!ReplayReview::new(&legacy_only, &secp).acknowledged());
        assert_eq!(psbt_digest(&legacy_only), review.psbt_digest());
    }

    #[test]
    fn pill_copy_names_replayable_inputs_and_entangled_ones() {
        assert_eq!(
            pill_copy(&ReplayStatus::Protected, &[]),
            (
                "This witness is replay protected".to_string(),
                PillTone::Success
            )
        );
        assert_eq!(
            pill_copy(&ReplayStatus::Replayable { inputs: vec![1] }, &[]),
            (
                "Replayable — no replay-capable signature on input 1".to_string(),
                PillTone::Warning
            )
        );
        assert_eq!(
            pill_copy(
                &ReplayStatus::Replayable { inputs: vec![0, 2] },
                &[(0, Entanglement::Entangled), (2, Entanglement::Unknown)]
            ),
            (
                "Replayable — no replay-capable signature on inputs \
                 0 (also exists on Bitcoin — signature required), \
                 2 (not yet checked against Bitcoin)"
                    .to_string(),
                PillTone::Warning
            )
        );
        // An entangled input that already carries a unified signature is not
        // named: the pill is about the witness, not the coin.
        assert_eq!(
            pill_copy(&ReplayStatus::Protected, &[(0, Entanglement::Entangled)]),
            (
                "This witness is replay protected".to_string(),
                PillTone::Success
            )
        );
        assert_eq!(
            pill_copy(&ReplayStatus::Unknown(UnknownReason::NotYetChecked), &[]),
            ("Not yet checked".to_string(), PillTone::Neutral)
        );
        assert_eq!(
            pill_copy(&ReplayStatus::Unknown(UnknownReason::Incomplete), &[]).1,
            PillTone::Neutral
        );
        assert_eq!(
            pill_copy(
                &ReplayStatus::Unknown(UnknownReason::Refused("bad".into())),
                &[]
            )
            .0,
            "Unknown — bad"
        );
        assert_eq!(
            REPLAYABLE_ACKNOWLEDGEMENT,
            "I understand this can also spend my Bitcoin"
        );
    }

    #[test]
    fn entangled_inputs_keep_unknown_and_drop_only_definite_no() {
        let f = fixture();
        let txid = f.psbt.unsigned_tx.input[0].previous_output.txid;
        assert_eq!(
            entangled_inputs(&f.psbt, |_| Entanglement::NotEntangled),
            vec![]
        );
        assert_eq!(
            entangled_inputs(&f.psbt, |_| Entanglement::Unknown),
            vec![(0, Entanglement::Unknown)]
        );
        assert_eq!(
            entangled_inputs(&f.psbt, |t| {
                assert_eq!(*t, txid);
                Entanglement::Entangled
            }),
            vec![(0, Entanglement::Entangled)]
        );
    }

    /// B4.2 Bitcoin-regression matrix, row I5 — a Bitcoin-family Cube still
    /// signs `SIGHASH_ALL` and nothing else, and the BTCB2 machinery is inert
    /// on it. The account-level `bitcoin_blake2b_enabled` feature is not an
    /// input to any of this: signing takes the descriptor and the PSBT, and
    /// the fork-specific paths key off [`ChainId`], so an account with the
    /// feature on cannot change what a Bitcoin Cube produces.
    #[test]
    fn a_bitcoin_cube_still_signs_sighash_all_and_carries_no_unified_records() {
        let f = fixture();
        let secp = secp();
        let signed = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);

        assert_eq!(signed.inputs[0].partial_sigs.len(), 2);
        for (pubkey, sig) in &signed.inputs[0].partial_sigs {
            assert_eq!(
                sig.sighash_type,
                EcdsaSighashType::All,
                "key {pubkey} signed with {:?}",
                sig.sighash_type
            );
        }
        // No unified record is added on the Bitcoin path, and the reserved
        // proprietary namespace is untouched.
        assert!(signed.inputs[0].proprietary.is_empty());
        assert!(signed.inputs[0].tap_script_sigs.is_empty());
        assert!(signed.inputs[0].tap_key_sig.is_none());

        // The spend analysis a Bitcoin Cube renders is the unchanged one.
        assert_eq!(
            spend_info_for_chain(ChainId::Bitcoin, &f.descriptor, &signed).unwrap(),
            f.descriptor.partial_spend_info(&signed).unwrap()
        );
        // And the pre-dispatch refusal is a Blake2b-only gate: an ordinary
        // Bitcoin PSBT passes it untouched.
        assert!(refuse_before_dispatch(&signed).is_ok());
        let _ = &secp;
    }

    /// B4.2 replay matrix — the mixed-input row. One unified signature *per
    /// input* is what protects a transaction; a PSBT whose inputs disagree must
    /// report the unprotected ones by index and must never read `Protected`.
    #[test]
    fn a_mixed_input_psbt_names_the_unprotected_input_and_is_never_protected() {
        let f = two_input_fixture();
        let secp = secp();
        assert_eq!(f.psbt.unsigned_tx.input.len(), 2);

        // Both inputs signed by two keys the unified way: protected.
        let both = unified(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
        assert_eq!(replay_status(&both, &secp), ReplayStatus::Protected);

        // Now assemble a PSBT whose inputs disagree: input 0 keeps its unified
        // signatures, input 1 carries legacy ones only. Built per input rather
        // than by re-signing the whole PSBT, because an input holding *both*
        // encodings for one key is a different case the model refuses outright.
        let legacy_both = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
        let mut mixed = both.clone();
        mixed.inputs[1] = legacy_both.inputs[1].clone();
        assert!(mixed.inputs[0].partial_sigs.is_empty());
        assert!(mixed.inputs[1].proprietary.is_empty());
        assert_eq!(mixed.inputs[1].partial_sigs.len(), 2);
        assert_eq!(
            replay_status(&mixed, &secp),
            ReplayStatus::Replayable { inputs: vec![1] }
        );
        assert!(!matches!(
            replay_status(&mixed, &secp),
            ReplayStatus::Protected
        ));

        // The copy names the input the user has to act on, not just "some".
        let (copy, tone) = pill_copy(&replay_status(&mixed, &secp), &[]);
        assert!(copy.contains("input 1"), "{}", copy);
        assert_eq!(tone, PillTone::Warning);

        // And the acknowledgement gate follows the same per-input answer.
        assert!(replay_status(&mixed, &secp).needs_acknowledgement());
        assert!(!replay_status(&both, &secp).needs_acknowledgement());
    }

    /// B4.2 replay matrix — the tampered-signature row. A signature that is
    /// well-formed but does not verify must fall to unknown or replayable;
    /// `Protected` is the one answer it may never produce, because that is the
    /// answer the user acts on.
    #[test]
    fn a_wellformed_but_invalid_unified_signature_never_reads_protected() {
        let f = fixture();
        let secp = secp();
        let signed = unified(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
        assert_eq!(replay_status(&signed, &secp), ReplayStatus::Protected);

        // Flip bits inside the signature bytes of one unified record, leaving
        // the record's shape and the PSBT's structure intact. Every byte offset
        // is tried: whichever way the tamper lands — a parse refusal, a
        // verification failure, or a record that no longer satisfies the
        // threshold — the answer must not be `Protected`.
        let key = signed.inputs[0].proprietary.keys().next().cloned().unwrap();
        let original = signed.inputs[0].proprietary.get(&key).unwrap().clone();
        let mut seen_non_protected = 0usize;
        for offset in 0..original.len() {
            let mut tampered = signed.clone();
            tampered.inputs[0].proprietary.get_mut(&key).unwrap()[offset] ^= 0x01;
            let status = replay_status(&tampered, &secp);
            assert_ne!(
                status,
                ReplayStatus::Protected,
                "tampering byte {offset} of a unified record still read Protected"
            );
            seen_non_protected += 1;
        }
        assert_eq!(seen_non_protected, original.len());
        assert!(original.len() >= 64, "record too short to be a signature");

        // The same for a legacy `partial_sigs` entry. Here `!= Protected` would
        // prove nothing: a legacy-only input is never replay-protected to begin
        // with, so that assertion holds identically on the untampered PSBT.
        // Assert what the corruption actually changes — the model verifies the
        // signature, so a well-formed one that does not verify moves the whole
        // PSBT from `Replayable` to a refusal that names the reason.
        let legacy_signed = legacy(&legacy(&f.psbt, &f.signers[0]), &f.signers[1]);
        let pk = *legacy_signed.inputs[0].partial_sigs.keys().next().unwrap();
        let mut bad_legacy = legacy_signed.clone();
        let sig = bad_legacy.inputs[0].partial_sigs.get_mut(&pk).unwrap();
        let mut bytes = sig.signature.serialize_der().to_vec();
        bytes[10] ^= 0x01;
        let parsed = secp256k1::ecdsa::Signature::from_der(&bytes)
            .expect("flipping one byte of the DER body keeps it parseable");
        *sig = ecdsa::Signature {
            signature: parsed,
            sighash_type: EcdsaSighashType::All,
        };
        let clean = replay_status(&legacy_signed, &secp);
        let tampered = replay_status(&bad_legacy, &secp);
        assert_eq!(clean, ReplayStatus::Replayable { inputs: vec![0] });
        assert!(
            matches!(
                &tampered,
                ReplayStatus::Unknown(UnknownReason::Refused(reason))
                    if reason.contains("does not verify")
            ),
            "{:?}",
            tampered
        );
        assert_ne!(clean, tampered);
    }

    #[test]
    fn a_malformed_proprietary_record_is_unknown_not_protected() {
        use coincube_core::miniscript::bitcoin::psbt::raw::ProprietaryKey;
        let f = fixture();
        let mut bad = unified(&unified(&f.psbt, &f.signers[0]), &f.signers[1]);
        bad.inputs[0].proprietary.insert(
            ProprietaryKey {
                prefix: b"coincube".to_vec(),
                subtype: 0,
                key: vec![0x02; 33],
            },
            vec![0x30, 0x00],
        );
        assert!(matches!(
            replay_status(&bad, &secp()),
            ReplayStatus::Unknown(UnknownReason::Refused(_))
        ));
    }
}
