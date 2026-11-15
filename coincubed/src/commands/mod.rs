//! # Coincube commands
//!
//! External interface to the Coincube daemon.

mod utils;

pub use crate::bitcoin::{HistorySync, PollFailure};
// Real cache transitions are used by opt-in native integration guards/tests.
#[doc(hidden)]
pub use crate::bitcoin::HistorySyncCache;

use crate::{
    bitcoin::BitcoinInterface,
    database::{Coin, DatabaseConnection, DatabaseInterface, ReservationError},
    miniscript::bitcoin::absolute::LockTime,
    poller::PollerMessage,
    DaemonControl, VERSION,
};

pub use crate::database::{CoinStatus, LabelItem};

use coincube_core::spend::{
    self, create_spend, AddrInfo, AncestorInfo, CandidateCoin, CreateSpendRes, SpendCreationError,
    SpendOutputAddress, SpendTxFees, TxGetter,
};
use coincube_core::{imported_spend, wallet_descriptor::WalletDescriptor};

use utils::{
    deser_addr_assume_checked, deser_amount_from_sats, deser_fromstr, deser_hex, ser_amount,
    ser_hex, ser_to_string,
};

use std::{
    collections::{hash_map, HashMap, HashSet},
    convert::TryInto,
    fmt,
    sync::{self, mpsc},
    time::SystemTime,
};

/// Stable JSON-RPC server error code for [`CommandError::TxBroadcast`], a failure
/// to broadcast a transaction to the P2P network. The desktop uses it to tell a
/// refused broadcast apart from other failures, whichever side of an external
/// daemon boundary it happened on.
pub const TX_BROADCAST_ERROR: i64 = 1_000;

/// Stable JSON-RPC server error code for [`CommandError::UnsafeLegacyAlternative`].
/// The desktop uses it to preserve the typed policy refusal across an external
/// daemon boundary instead of treating it as generic invalid input.
pub const UNSAFE_LEGACY_ALTERNATIVE_ERROR: i64 = 1_002;

use miniscript::{
    bitcoin::{
        self, address,
        bip32::{self, ChildNumber},
        psbt::Psbt,
    },
    psbt::PsbtExt,
};
use serde::{Deserialize, Serialize};

const CHANGE_COMMIT_CONFLICT_LIMIT: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    NoOutpointForSelfSend,
    InvalidFeerate(/* sats/vb */ u64),
    UnknownOutpoint(bitcoin::OutPoint),
    AlreadySpent(bitcoin::OutPoint),
    ImmatureCoinbase(bitcoin::OutPoint),
    Address(bitcoin::address::ParseError),
    SpendCreation(SpendCreationError),
    InsufficientFunds(
        /* in value */ bitcoin::Amount,
        /* out value */ Option<bitcoin::Amount>,
        /* target feerate */ u64,
    ),
    UnknownSpend(bitcoin::Txid),
    /// A stored spend input has no previous transaction and the wallet holds
    /// none for its outpoint, so its amount cannot be authenticated (P2-A).
    SpendMissingPreviousTransaction(bitcoin::OutPoint),
    // FIXME: when upgrading Miniscript put the actual error there
    SpendFinalization(String),
    /// A Bitcoin Blake2b spend could not be finalised from its verified unified
    /// (and, where allowed, legacy) signatures. Nothing was broadcast.
    UnifiedSpendFinalization(String),
    /// Signatures for a Bitcoin Blake2b spend could not be merged into the stored
    /// PSBT: the incoming unified records conflict with, or are malformed next to,
    /// the ones already held. The stored spend is unchanged.
    UnifiedSignatureMerge(String),
    /// A Bitcoin Blake2b PSBT handed to the daemon is not one it will hold: a
    /// malformed reserved unified record, a key with both encodings, an
    /// unsupported sighash request, or a signature that does not verify (an
    /// `ANYONECANPAY` legacy signature, a signature made for another
    /// transaction). Refused before the first insert as well as before a
    /// merge, so the database never stores a spend that every later update
    /// and the finaliser would refuse. Nothing was stored; an existing row is
    /// unchanged.
    UnifiedSpendValidation(String),
    /// Imported Bitcoin PSBT changed construction or supplied invalid signer data.
    ImportedSpendValidation(String),
    /// The locally retained Bitcoin Blake2b PSBT has a unified-bearing
    /// satisfaction, but its legacy signatures can independently satisfy an
    /// input. Refused before storage, export, or broadcast so Coincube does not
    /// persist or present a locally reproducible alternate Bitcoin witness.
    UnsafeLegacyAlternative {
        input: usize,
        legacy_signatures: usize,
    },
    TxBroadcast(String),
    AlreadyRescanning,
    InsaneRescanTimestamp(u32),
    /// An error that might occur in the racy rescan triggering logic.
    RescanTrigger(String),
    /// The backend could not establish the genesis timestamp; no rescan started.
    RescanGenesis(String),
    RecoveryNotAvailable,
    /// The backend could not report its chain tip; nothing was created. Retryable.
    ChainTipUnavailable(String),
    /// The backend could not report its mempool; nothing was created. Retryable.
    MempoolUnavailable(String),
    // Include timelock in error as it may not have been set explicitly by the user.
    OutpointNotRecoverable(bitcoin::OutPoint, /* timelock */ u16),
    /// Overflowing or unhardened derivation index.
    InvalidDerivationIndex,
    ChangeReservation(crate::database::ReservationError),
    /// Other durable reservations repeatedly won after this builder selected change.
    /// The caller may retry the complete command.
    ChangeReservationContended,
    /// Caller fee ceiling rejected the construction before any change index commit.
    FeeCeilingExceeded,
    /// Measured sat/vB exceeded the requested rate before any change index commit.
    FeeRateExceeded,
    /// The PSBT fee could not be measured, so nothing was committed.
    FeeMeasurement,
    RbfError(RbfErrorInfo),
    EmptyFilterList,
    /// A vault change address is not a change-branch script of this descriptor,
    /// and the spend is not a sweep whose only output is that address.
    ExternalChangeNotAllowed,
    /// `updatespend` carried signatures for a different unsigned transaction
    /// than the one already stored.
    SignatureForDifferentTransaction,
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoOutpointForSelfSend => {
                write!(f, "No provided outpoint for self-send. Need at least one.")
            }
            Self::InvalidFeerate(sats_vb) => write!(f, "Invalid feerate: {} sats/vb.", sats_vb),
            Self::AlreadySpent(op) => write!(f, "Coin at '{}' is already spent.", op),
            Self::ImmatureCoinbase(op) => write!(
                f,
                "Coin at '{}' is from an immature coinbase transaction.",
                op
            ),
            Self::UnknownOutpoint(op) => write!(f, "Unknown outpoint '{}'.", op),
            Self::Address(e) => write!(f, "Address error: {}", e),
            Self::SpendCreation(e) => write!(f, "Creating spend: {}", e),
            Self::InsufficientFunds(in_val, out_val, feerate) => {
                if let Some(out_val) = out_val {
                    write!(
                        f,
                        "Cannot create a {} sat/vb transaction with input value {} and output value {}",
                        feerate, in_val, out_val
                    )
                } else {
                    write!(
                        f,
                        "Not enough fund to create a {} sat/vb transaction with input value {}",
                        feerate, in_val
                    )
                }
            }
            Self::UnknownSpend(txid) => write!(f, "Unknown spend transaction '{}'.", txid),
            Self::SpendMissingPreviousTransaction(op) => write!(
                f,
                "Spend input {} has no previous transaction and the wallet does not hold one, \
                so its amount cannot be authenticated. Delete this spend and recreate it.",
                op
            ),
            Self::SpendFinalization(e) => {
                write!(f, "Failed to finalize the spend transaction PSBT: '{}'.", e)
            }
            Self::UnifiedSpendFinalization(e) => write!(
                f,
                "Failed to finalize the Bitcoin Blake2b spend from its verified signatures: '{}'.",
                e
            ),
            Self::UnifiedSignatureMerge(e) => write!(
                f,
                "Refused to merge signatures into the stored Bitcoin Blake2b spend: '{}'.",
                e
            ),
            Self::UnifiedSpendValidation(e) => {
                write!(f, "Refused to store the Bitcoin Blake2b spend: '{}'.", e)
            }
            Self::ImportedSpendValidation(e) => {
                write!(f, "Refused imported Bitcoin spend: '{e}'.")
            }
            Self::UnsafeLegacyAlternative {
                input,
                legacy_signatures,
            } => write!(
                f,
                "Refused Bitcoin Blake2b spend input {input}: its {legacy_signatures} retained \
                 legacy signatures can independently form a Bitcoin-valid witness. Remove the \
                 surplus legacy signatures or create a new spend. Signatures already held by \
                 another device or exported copy cannot be revoked."
            ),
            Self::TxBroadcast(e) => write!(f, "Failed to broadcast transaction: {}", e),
            Self::AlreadyRescanning => write!(
                f,
                "There is already a rescan ongoing. Please wait for it to complete first."
            ),
            Self::InsaneRescanTimestamp(t) => write!(f, "Insane timestamp '{}'.", t),
            Self::RescanGenesis(e) => write!(f, "Cannot determine rescan lower bound: {e}"),
            Self::RescanTrigger(e) => write!(f, "Error while starting rescan: '{}'", e),
            Self::ChainTipUnavailable(e) => write!(
                f,
                "Cannot read the current chain tip; retry when the backend is available: {e}"
            ),
            Self::MempoolUnavailable(e) => write!(
                f,
                "Cannot read the backend's mempool; retry when the backend is available: {e}"
            ),
            Self::RecoveryNotAvailable => write!(
                f,
                "No coin currently spendable through this timelocked recovery path."
            ),
            Self::OutpointNotRecoverable(op, t) => write!(
                f,
                "Coin at '{}' is not recoverable with timelock '{}'",
                op, t
            ),
            Self::InvalidDerivationIndex => {
                write!(f, "Unhardened or overflowing BIP32 derivation index.")
            }
            Self::ChangeReservation(e) => write!(f, "{}", e),
            Self::ChangeReservationContended => write!(
                f,
                "Change address reservations are busy. Retry the command."
            ),
            Self::FeeCeilingExceeded => {
                write!(f, "Actual fee exceeds the supplied ceiling.")
            }
            Self::FeeRateExceeded => {
                write!(f, "Measured fee rate exceeds the supplied rate.")
            }
            Self::FeeMeasurement => write!(f, "Could not measure the transaction fee."),
            Self::RbfError(e) => write!(f, "RBF error: '{}'.", e),
            Self::EmptyFilterList => write!(f, "Filter list is empty, should supply None instead."),
            Self::ExternalChangeNotAllowed => write!(
                f,
                "Change address is not a wallet change address. A vault spend can send change to an external address only when that address is the sole output (a sweep)."
            ),
            Self::SignatureForDifferentTransaction => write!(
                f,
                "Refusing to merge signatures for a different unsigned transaction."
            ),
        }
    }
}

impl std::error::Error for CommandError {}

impl From<SpendCreationError> for CommandError {
    fn from(e: SpendCreationError) -> Self {
        CommandError::SpendCreation(e)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RbfErrorInfo {
    MissingFeerate,
    SuperfluousFeerate,
    TooLowFeerate(u64, u64),
    NotSignaling,
}

impl fmt::Display for RbfErrorInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Self::MissingFeerate => {
                write!(f, "A feerate must be provided if not creating a cancel.")
            }
            Self::SuperfluousFeerate => {
                write!(
                    f,
                    "A feerate must not be provided if creating a cancel. We'll always use the smallest one which satisfies the RBF rules."
                )
            }
            Self::TooLowFeerate(r, m) => {
                write!(f, "Feerate {} too low for minimum feerate {}.", r, m)
            }
            Self::NotSignaling => write!(f, "Replacement candidate does not signal for RBF."),
        }
    }
}

/// A wallet transaction getter which fetches the transaction from our database backend with a cache
/// to avoid needless redundant calls. Note the cache holds an Option<> so we also avoid redundant
/// calls when the txid isn't known by our database backend.
struct DbTxGetter<'a> {
    db: &'a sync::Arc<sync::Mutex<dyn DatabaseInterface>>,
    cache: HashMap<bitcoin::Txid, Option<bitcoin::Transaction>>,
}

impl<'a> DbTxGetter<'a> {
    pub fn new(db: &'a sync::Arc<sync::Mutex<dyn DatabaseInterface>>) -> Self {
        Self {
            db,
            cache: HashMap::new(),
        }
    }
}

impl<'a> TxGetter for DbTxGetter<'a> {
    fn get_tx(&mut self, txid: &bitcoin::Txid) -> Option<bitcoin::Transaction> {
        if let hash_map::Entry::Vacant(entry) = self.cache.entry(*txid) {
            let tx = self
                .db
                .connection()
                .list_wallet_transactions(&[*txid])
                .pop()
                .map(|(tx, _, _)| tx);
            entry.insert(tx);
        }
        self.cache.get(txid).cloned().flatten()
    }
}

/// Fill in every missing `non_witness_utxo` of a stored spend PSBT from the
/// wallet's own copy of the previous transaction, so a PSBT that predates
/// fee-source authentication (P2-A) or was imported without its previous
/// transactions can still be re-verified before broadcast. This adds no trust:
/// [`spend::reverify_spend_before_broadcast`] authenticates whatever is set here
/// through the txid committed by the outpoint. An input whose previous
/// transaction the wallet does not hold cannot be authenticated at all and the
/// spend must be recreated.
/// Hold an incoming PSBT to what the chain will ever finalise before anything
/// is stored: on Bitcoin Blake2b the adapter's `validate_internal` (well-formed
/// reserved records, no key with both encodings, a supported sighash request)
/// **and** cryptographic verification of every signature it carries
/// (`unified_finalize::verify_all_signatures`: unified records through the
/// verifier, legacy ones against the BIP-143 digest, `SIGHASH_ALL` only). The
/// adapter alone validates representation, not validity: a legacy
/// `ANYONECANPAY` signature or a signature made for another transaction
/// would pass it, be stored, be refused by every later update as a conflict
/// against its correct replacement, and be refused again at finalisation.
/// On a Bitcoin-family chain nothing, as before. Runs ahead of both branches
/// of `update_spend` — the first insert as much as a merge — so a row the
/// daemon could never complete or finalise is never written, and an invalid
/// update leaves the stored row byte-identical. An unsigned or partially
/// signed spend passes. The incoming PSBT must carry a committing
/// `witness_script` per input; `non_witness_utxo` is backfilled from the
/// wallet by `update_spend` before this runs, so a `witness_utxo`-only PSBT
/// — accepted by the public contract all along — still is.
fn validate_spend_for_chain(
    chain: coincube_core::chain::ChainId,
    psbt: &Psbt,
    secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::VerifyOnly>,
) -> Result<(), CommandError> {
    if !chain.is_blake2b() {
        return Ok(());
    }
    let unified = coincube_core::psbt_unified::UnifiedPsbt::from_psbt(psbt.clone())
        .map_err(|e| CommandError::UnifiedSpendValidation(e.to_string()))?;
    coincube_core::unified_finalize::verify_all_signatures(&unified, secp)
        .map_err(|e| CommandError::UnifiedSpendValidation(e.to_string()))?;
    enforce_legacy_signature_policy(&unified, secp)
}

fn imported_construction(psbt: &Psbt) -> Psbt {
    let mut construction = psbt.clone();
    for input in &mut construction.inputs {
        input.partial_sigs.clear();
    }
    construction
}

fn enforce_legacy_signature_policy(
    psbt: &coincube_core::psbt_unified::UnifiedPsbt,
    secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::VerifyOnly>,
) -> Result<(), CommandError> {
    use coincube_core::unified_finalize::{
        ensure_no_unsafe_legacy_alternative, UnifiedFinalizeError,
    };
    ensure_no_unsafe_legacy_alternative(psbt, secp).map_err(|error| match error {
        UnifiedFinalizeError::UnsafeLegacyAlternative {
            input,
            legacy_signatures,
        } => CommandError::UnsafeLegacyAlternative {
            input,
            legacy_signatures,
        },
        other => CommandError::UnifiedSpendValidation(other.to_string()),
    })
}

/// Economic and derivation checks for a vault PSBT that `updatespend` is about
/// to store for the first time. Input amounts come from the wallet's coins:
/// the public contract still accepts a PSBT whose previous transactions have
/// not been attached yet, and broadcast repeats full prevout authentication.
fn validate_new_vault_psbt(
    vault: &coincube_core::descriptors::CoincubeDescriptor,
    psbt: &Psbt,
    coins: &HashMap<bitcoin::OutPoint, Coin>,
    network: bitcoin::Network,
    secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::VerifyOnly>,
) -> Result<(), CommandError> {
    let tx = &psbt.unsigned_tx;
    if psbt.inputs.len() != tx.input.len()
        || psbt.outputs.len() != tx.output.len()
        || tx.output.is_empty()
    {
        return Err(SpendCreationError::SanityCheckFailure(psbt.clone()).into());
    }
    let mut value_in: u64 = 0;
    for txin in &tx.input {
        let coin = coins
            .get(&txin.previous_output)
            .ok_or(CommandError::UnknownOutpoint(txin.previous_output))?;
        value_in = value_in.checked_add(coin.amount.to_sat()).ok_or_else(|| {
            CommandError::from(SpendCreationError::SanityCheckFailure(psbt.clone()))
        })?;
    }
    let value_out: u64 = tx.output.iter().map(|o| o.value.to_sat()).sum();
    let fee = value_in.checked_sub(value_out).ok_or(CommandError::from(
        SpendCreationError::InsaneFees(spend::InsaneFeeInfo::NegativeFee),
    ))?;
    if fee > spend::MAX_FEE.to_sat() {
        return Err(SpendCreationError::InsaneFees(spend::InsaneFeeInfo::TooHighFee(fee)).into());
    }
    let feerate_ok = [true, false].iter().any(|primary| {
        let vb = vault.unsigned_tx_max_vbytes(tx, *primary);
        vb > 0 && (1..=spend::MAX_FEERATE).contains(&(fee / vb))
    });
    if !feerate_ok {
        return Err(SpendCreationError::InsaneFees(spend::InsaneFeeInfo::InvalidFeerate).into());
    }
    for txo in &tx.output {
        if txo.value < txo.script_pubkey.minimal_non_dust() {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()).into());
        }
    }
    for (index, psbt_out) in psbt.outputs.iter().enumerate() {
        let has_deriv =
            !psbt_out.bip32_derivation.is_empty() || !psbt_out.tap_key_origins.is_empty();
        if !has_deriv {
            continue;
        }
        let Some(child) = psbt_out
            .bip32_derivation
            .values()
            .next()
            .map(|(_, path)| path)
            .or_else(|| {
                psbt_out
                    .tap_key_origins
                    .values()
                    .next()
                    .map(|(_, (_, path))| path)
            })
            .and_then(|path| path.into_iter().last().copied())
        else {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()).into());
        };
        let txo = &tx.output[index];
        let Ok(index) = bitcoin::bip32::ChildNumber::from_normal_idx(u32::from(child)) else {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()).into());
        };
        let matches = |branch: &coincube_core::descriptors::SinglePathCoincubeDesc| {
            branch.derive(index, secp).address(network).script_pubkey() == txo.script_pubkey
        };
        if !matches(vault.change_descriptor()) && !matches(vault.receive_descriptor()) {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()).into());
        }
    }
    Ok(())
}

/// Merge the signatures of an incoming PSBT for a spend already stored, keyed on
/// the chain the daemon runs on.
///
/// The Bitcoin arm is the historical best-effort merge, unchanged: partial and
/// Taproot signatures of matching inputs are copied over (last write wins on a
/// key), nothing else. The Bitcoin Blake2b arm is the adapter merge instead —
/// `partial_sigs` plus the unified (`0x21`) records in the proprietary map,
/// which the copy would drop on the floor — run against the stored PSBT as it
/// is, so a merge that would leave a key with a conflicting or ambiguous
/// encoding is refused before anything is overwritten, and the merged
/// candidate is verified as a whole before it is returned, so a stored spend
/// never ends up as a PSBT its own verifier refuses. On refusal nothing is
/// stored.
fn merge_spend_signatures(
    chain: coincube_core::chain::ChainId,
    mut db_psbt: Psbt,
    psbt: &Psbt,
    secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::VerifyOnly>,
) -> Result<Psbt, CommandError> {
    if chain.is_blake2b() {
        // The checked adapter merge runs against the **stored** PSBT as it is,
        // never after a copy: the historical copy below is last-write-wins on
        // `partial_sigs`, and a conflicting legacy signature copied first would
        // have replaced the stored one before the adapter compared anything.
        // Refuses conflicting or ambiguous encodings, and an incoming PSBT for
        // a different unsigned transaction; nothing is stored on refusal. (A
        // Blake2b Vault is native P2WSH — Taproot is not offered on that chain
        // — so the adapter's ECDSA-only view of signatures is the whole
        // picture there.)
        use coincube_core::psbt_unified::{merge_signatures, UnifiedPsbt};
        let mut stored = UnifiedPsbt::from_psbt(db_psbt)
            .map_err(|e| CommandError::UnifiedSignatureMerge(e.to_string()))?;
        let incoming = UnifiedPsbt::from_psbt(psbt.clone())
            .map_err(|e| CommandError::UnifiedSignatureMerge(e.to_string()))?;
        merge_signatures(&mut stored, &incoming)
            .map_err(|e| CommandError::UnifiedSignatureMerge(e.to_string()))?;
        // The **merged candidate** has to verify too, not only each side on
        // its own: the adapter merge keeps the stored request field, so a
        // stored `SIGHASH_ALL` request plus an incoming unified record is a
        // PSBT the verifier refuses (`IncompatibleSighash`) — it must not be
        // stored, or every later update and the finaliser would refuse the
        // row. Refused atomically; no request reconciliation.
        coincube_core::unified_finalize::verify_all_signatures(&stored, secp)
            .map_err(|e| CommandError::UnifiedSignatureMerge(e.to_string()))?;
        enforce_legacy_signature_policy(&stored, secp)?;
        return Ok(stored.psbt().clone());
    }

    if psbt.unsigned_tx != db_psbt.unsigned_tx {
        return Err(CommandError::SignatureForDifferentTransaction);
    }
    let tx = &psbt.unsigned_tx;
    let db_tx = db_psbt.unsigned_tx.clone();
    for i in 0..db_tx.input.len() {
        if tx
            .input
            .get(i)
            .map(|tx_in| tx_in.previous_output == db_tx.input[i].previous_output)
            != Some(true)
        {
            continue;
        }
        let psbtin = match psbt.inputs.get(i) {
            Some(psbtin) => psbtin,
            None => continue,
        };
        let db_psbtin = match db_psbt.inputs.get_mut(i) {
            Some(db_psbtin) => db_psbtin,
            None => continue,
        };
        db_psbtin.partial_sigs.extend(psbtin.partial_sigs.clone());
        db_psbtin
            .tap_script_sigs
            .extend(psbtin.tap_script_sigs.clone());
        if db_psbtin.tap_key_sig.is_none() {
            db_psbtin.tap_key_sig = psbtin.tap_key_sig;
        }
    }
    Ok(db_psbt)
}

/// Finalise a stored spend into the transaction to broadcast, keyed on the chain
/// the daemon runs on.
///
/// The Bitcoin arm is rust-miniscript's finaliser, exactly as before. The Bitcoin
/// Blake2b arm is the core unified finaliser: every unified record is
/// cryptographically verified first and the witnesses are assembled from the
/// miniscript satisfaction with no node involved (so the Connect Esplora
/// backend needs nothing beyond broadcast). A verified unified signature that
/// the script *can* use always ends up in the witness, whichever key position
/// it sits on and however many legacy signatures are alongside — the finaliser
/// searches the legacy subsets for a satisfaction that keeps one, and refuses
/// rather than degrades if it cannot complete that search. An input comes out
/// legacy-only, and is logged as replayable below, only when no unified
/// signature on it is usable for this transaction. rust-miniscript's own
/// finaliser is never used here: it cannot see unified records and would
/// finalise a Blake2b spend from whatever legacy signatures are present.
fn finalize_spend_for_chain(
    chain: coincube_core::chain::ChainId,
    mut spend_psbt: Psbt,
    secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::VerifyOnly>,
) -> Result<bitcoin::Transaction, CommandError> {
    if !chain.is_blake2b() {
        spend_psbt.finalize_mut(secp).map_err(|e| {
            CommandError::SpendFinalization(
                e.into_iter()
                    .next()
                    .map(|e| e.to_string())
                    .unwrap_or_default(),
            )
        })?;
        return Ok(spend_psbt.extract_tx_unchecked_fee_rate());
    }

    use coincube_core::{
        psbt_unified::UnifiedPsbt,
        unified_finalize::{ensure_no_unsafe_legacy_alternative, finalize_p2wsh_all_unified},
    };
    let unified = UnifiedPsbt::from_psbt(spend_psbt)
        .map_err(|e| CommandError::UnifiedSpendFinalization(e.to_string()))?;
    ensure_no_unsafe_legacy_alternative(&unified, secp).map_err(|error| match error {
        coincube_core::unified_finalize::UnifiedFinalizeError::UnsafeLegacyAlternative {
            input,
            legacy_signatures,
        } => CommandError::UnsafeLegacyAlternative {
            input,
            legacy_signatures,
        },
        other => CommandError::UnifiedSpendFinalization(other.to_string()),
    })?;
    let finalized = finalize_p2wsh_all_unified(&unified, secp)
        .map_err(|e| CommandError::UnifiedSpendFinalization(e.to_string()))?;
    for (index, report) in finalized.inputs.iter().enumerate() {
        if report.replay_protected() {
            log::info!(
                "spend input {} finalised with {} unified signature(s): not replayable on Bitcoin",
                index,
                report.unified_used
            );
        } else {
            log::warn!(
                "spend input {} finalised from {} legacy signature(s) only: replayable on Bitcoin",
                index,
                report.legacy_used
            );
        }
    }
    Ok(finalized.transaction)
}

fn backfill_previous_transactions(
    psbt: &mut Psbt,
    tx_getter: &mut impl TxGetter,
) -> Result<(), CommandError> {
    for (psbt_in, txin) in psbt.inputs.iter_mut().zip(psbt.unsigned_tx.input.iter()) {
        if psbt_in.non_witness_utxo.is_none() {
            psbt_in.non_witness_utxo = tx_getter.get_tx(&txin.previous_output.txid);
        }
        if psbt_in.non_witness_utxo.is_none() {
            return Err(CommandError::SpendMissingPreviousTransaction(
                txin.previous_output,
            ));
        }
    }
    Ok(())
}

fn coin_to_candidate(
    coin: &Coin,
    must_select: bool,
    sequence: Option<bitcoin::Sequence>,
    ancestor_info: Option<AncestorInfo>,
) -> CandidateCoin {
    CandidateCoin {
        outpoint: coin.outpoint,
        amount: coin.amount,
        deriv_index: coin.derivation_index,
        is_change: coin.is_change,
        must_select,
        sequence,
        ancestor_info,
    }
}

/// Read-only metadata from the daemon's actual admitted chain/backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RescanTimeBounds {
    pub chain: coincube_core::chain::ChainId,
    pub genesis_timestamp: u32,
    pub tip_timestamp: u32,
}

impl DaemonControl {
    /// Ordinary imported-wallet spends have no BTCB2 authorization path yet.
    /// Refuse before reservations, PSBT mutation, database writes or broadcast.
    fn require_ordinary_imported_spend_support(&self) -> Result<(), CommandError> {
        if self.config.bitcoin_config.chain.is_blake2b()
            && self.config.main_descriptor.as_imported().is_some()
        {
            return Err(CommandError::ImportedSpendValidation(
                "Ordinary spending from an imported Bitcoin Blake2b wallet is not available yet"
                    .into(),
            ));
        }
        Ok(())
    }
    // Get the derived descriptor for this coin
    fn derived_desc(
        &self,
        coin: &Coin,
    ) -> miniscript::Descriptor<miniscript::DefiniteDescriptorKey> {
        let desc = if coin.is_change {
            self.config.main_descriptor.change_descriptor()
        } else {
            self.config.main_descriptor.receive_descriptor()
        };
        desc.derive(u32::from(coin.derivation_index))
            .expect("stored wallet derivation index is valid")
    }

    // Check whether this address is valid for the network we are operating on.
    fn validate_address(
        &self,
        addr: bitcoin::Address<address::NetworkUnchecked>,
    ) -> Result<bitcoin::Address, CommandError> {
        // NOTE: signet uses testnet addresses, and legacy addresses on regtest use testnet
        // encoding.
        addr.require_network(self.config.bitcoin_config.network)
            .map_err(CommandError::Address)
    }

    // Get details about this address, if we know about it.
    fn addr_info(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
        addr: &bitcoin::Address,
    ) -> Option<AddrInfo> {
        db_conn
            .derivation_index_by_address(addr)
            .map(|(index, is_change)| AddrInfo { index, is_change })
    }

    // Create an address to be used in an output of a spend transaction.
    fn spend_addr(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
        addr: bitcoin::Address,
    ) -> SpendOutputAddress {
        SpendOutputAddress {
            info: self.addr_info(db_conn, &addr),
            addr,
        }
    }

    /// Durably reserve a fresh local change index. Cancellation never releases it.
    /// The binding is checked against persisted chain and descriptor identity.
    pub fn reserve_change(&self) -> Result<crate::database::ChangeReservation, CommandError> {
        self.db
            .reserve_change(
                self.config.bitcoin_config.chain,
                self.config
                    .main_descriptor
                    .as_vault()
                    .ok_or(CommandError::RecoveryNotAvailable)?,
                &self.secp,
            )
            .map_err(CommandError::ChangeReservation)
    }

    fn change_addr_at(&self, index: bip32::ChildNumber) -> SpendOutputAddress {
        let desc = self
            .config
            .main_descriptor
            .change_descriptor()
            .derive(u32::from(index))
            .expect("change derivation index is valid");
        SpendOutputAddress {
            addr: desc
                .address(self.config.bitcoin_config.network)
                .expect("supported wallet address"),
            info: Some(AddrInfo {
                index,
                is_change: true,
            }),
        }
    }

    // Derive the next change address without persisting it. Ordinary spend builders
    // commit it only after producing a PSBT that actually carries change.
    fn next_change_addr(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
    ) -> Result<SpendOutputAddress, CommandError> {
        let index = db_conn
            .change_index()
            .increment()
            .map_err(|_| CommandError::ChangeReservation(ReservationError::Exhausted))?;
        Ok(self.change_addr_at(index))
    }

    // Commit a successfully built change output only if its peeked index is still
    // next. `reserve_change` (written for the removed Claim flow, and without a
    // production caller since) allocates directly; both serialize through
    // the same database writer lock, so a false result requires rebuilding with a
    // newly peeked address and never permits index reuse.
    fn commit_change_if_next(&self, change: &Option<AddrInfo>) -> Result<bool, CommandError> {
        let index = match change {
            Some(AddrInfo {
                index,
                is_change: true,
            }) => *index,
            _ => return Ok(true),
        };
        if let Some(imported) = self.config.main_descriptor.as_imported() {
            return self
                .db
                .commit_imported_change_if_next(self.config.bitcoin_config.chain, imported, index)
                .map_err(CommandError::ChangeReservation);
        }
        self.db
            .commit_change_if_next(
                self.config.bitcoin_config.chain,
                self.config
                    .main_descriptor
                    .as_vault()
                    .ok_or(CommandError::RecoveryNotAvailable)?,
                &self.secp,
                index,
            )
            .map_err(CommandError::ChangeReservation)
    }

    /// A vault change output must pay a change-branch script of this descriptor,
    /// unless the spend is a sweep (no other destinations), in which case the
    /// address is the sole payment and is shown as such.
    ///
    /// Change index 0 is accepted even when it is not stored yet. The desktop
    /// uses that address to size a preview without reserving a fresh index.
    /// Any change script at an index this wallet has already reached is also
    /// owned: the address table can miss it (the in-memory test database never
    /// stores addresses) and a row in that table is not itself proof.
    fn authorize_vault_change(
        &self,
        change: &SpendOutputAddress,
        destinations: &[(SpendOutputAddress, bitcoin::Amount)],
        last_change: bip32::ChildNumber,
    ) -> Result<(), CommandError> {
        if self.change_script_is_owned(change, last_change) {
            return Ok(());
        }
        if destinations.is_empty() {
            return Ok(());
        }
        Err(CommandError::ExternalChangeNotAllowed)
    }

    fn change_script_is_owned(
        &self,
        change: &SpendOutputAddress,
        last_change: bip32::ChildNumber,
    ) -> bool {
        let script = change.addr.script_pubkey();
        let matches_index = |index: u32| -> bool {
            let Ok(derived) = self
                .config
                .main_descriptor
                .change_descriptor()
                .derive(index)
            else {
                return false;
            };
            derived
                .address(self.config.bitcoin_config.network)
                .map(|addr| addr.script_pubkey() == script)
                .unwrap_or(false)
        };
        if let Some(AddrInfo {
            index,
            is_change: true,
        }) = change.info
        {
            // Re-derive the claimed index. A database row cannot bless a script
            // this descriptor does not produce.
            if matches_index(u32::from(index)) {
                return true;
            }
        }
        // Index 0 covers a preview. The rest covers a change address this
        // wallet already derived, including one the address table does not list.
        let last = u32::from(last_change).min(10_000);
        (0..=last).any(matches_index)
    }

    // If we detect the given address as ours, and it has a higher derivation index than our last
    // derivation index, update our last derivation index to the given value.
    fn maybe_increase_last_deriv_index(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
        addr_info: &Option<AddrInfo>,
    ) {
        if let Some(AddrInfo { index, is_change }) = addr_info {
            if *is_change && db_conn.change_index() < *index {
                db_conn.set_change_index(*index, &self.secp);
            } else if !is_change && db_conn.receive_index() < *index {
                db_conn.set_receive_index(*index, &self.secp);
            }
        }
    }

    // Pass relevant values to the spend module function of same name.
    fn anti_fee_sniping_locktime(&self) -> LockTime {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("time measured now cannot be before unix epoch");
        // A backend we cannot reach degrades like a stale tip does: no
        // anti-fee-sniping locktime, rather than a panic mid-spend (#589).
        let (tip_height, tip_time) = match self.bitcoin.chain_tip_result() {
            Ok(tip) => (
                tip.height.try_into().expect("block height must fit in u32"),
                self.bitcoin.tip_time(),
            ),
            Err(e) => {
                log::warn!(
                    "Cannot read the chain tip, not setting an anti-fee-sniping locktime: {}",
                    e
                );
                (0, None)
            }
        };
        spend::anti_fee_sniping_locktime(now, tip_height, tip_time)
    }

    /// Ancestor size and fees of an unconfirmed coin's transaction, for coin
    /// selection. `Ok(None)` means the backend says it is not in the mempool;
    /// an unreachable backend (#594) or an unusable answer (#597) is an error,
    /// not a panic.
    fn ancestor_info(&self, txid: &bitcoin::Txid) -> Result<Option<AncestorInfo>, CommandError> {
        self.bitcoin
            .mempool_entry_result(txid)
            .map_err(CommandError::MempoolUnavailable)?
            .map(|info| {
                let fee = info.fees.ancestor.to_sat();
                Ok(AncestorInfo {
                    vsize: info.ancestor_vsize,
                    fee: fee.try_into().map_err(|_| {
                        CommandError::MempoolUnavailable(format!(
                            "ancestor fee of {} is {} sat, more than coin selection supports",
                            txid, fee
                        ))
                    })?,
                })
            })
            .transpose()
    }

    /// The minimum feerate and the replaced fees for a replacement of `txid`, worked out from our
    /// own records, for a backend that cannot see the mempool
    /// ([`BitcoinInterface::sees_mempool`]).
    ///
    /// The replacement conflicts with whatever our records show spending the coins of `txid`
    /// (`txid` itself when they show nothing), and all of it is taken to still be in the mempool:
    /// if some of it was evicted the replacement pays more than it had to, whereas leaving out
    /// some that is there gets the replacement refused. The minimum feerate is one above the
    /// highest of theirs (rule #6). The replaced fees are theirs plus those of the descendants our
    /// records show, that is our own transactions spending their outputs (rule #3). A descendant
    /// someone else made from one of their outputs is not in our records, and can still get the
    /// replacement refused.
    ///
    /// A conflict spending a coin we have no value for cannot be priced, and could pay more than
    /// anything we would guess: that is a [`CommandError::MempoolUnavailable`], not a replacement
    /// built without it.
    fn replacement_minimums_from_records(
        &self,
        db_conn: &mut Box<dyn DatabaseConnection>,
        tx_getter: &mut DbTxGetter<'_>,
        txid: &bitcoin::Txid,
        prev_tx: &bitcoin::Transaction,
        prev_psbt: Option<&Psbt>,
        prev_coins: &HashMap<bitcoin::OutPoint, Coin>,
    ) -> Result<(u64, bitcoin::Amount), CommandError> {
        // Our coins spent by an unconfirmed transaction: the inputs of everything priced below,
        // and what links a transaction to its descendants.
        let spending = db_conn.coins(&[CoinStatus::Spending], &[]);
        let values: HashMap<bitcoin::OutPoint, bitcoin::Amount> = prev_coins
            .values()
            .chain(spending.values())
            .map(|coin| (coin.outpoint, coin.amount))
            .collect();
        // Fee and virtual size of a transaction, if it only spends coins we know the value of.
        let price = |tx: &bitcoin::Transaction| -> Option<(bitcoin::Amount, u64)> {
            let spent = tx
                .input
                .iter()
                .try_fold(bitcoin::Amount::ZERO, |sum, txin| {
                    sum.checked_add(*values.get(&txin.previous_output)?)
                })?;
            let created = tx
                .output
                .iter()
                .try_fold(bitcoin::Amount::ZERO, |sum, txo| sum.checked_add(txo.value))?;
            let fee = spent.checked_sub(created)?;
            let vsize: u64 = tx.vsize().try_into().ok()?;
            (fee <= bitcoin::Amount::MAX_MONEY).then_some((fee, vsize))
        };
        // Kept within the money supply, so that coin selection adding the replacement's own relay
        // fee to it cannot overflow.
        let add_fee = |total: bitcoin::Amount, fee: bitcoin::Amount| {
            total
                .checked_add(fee)
                .map_or(bitcoin::Amount::MAX_MONEY, |sum| {
                    sum.min(bitcoin::Amount::MAX_MONEY)
                })
        };

        let mut conflicts: Vec<bitcoin::Txid> = prev_coins
            .values()
            .filter_map(|coin| coin.spend_txid)
            .collect();
        conflicts.sort_unstable();
        conflicts.dedup();
        if conflicts.is_empty() {
            conflicts.push(*txid);
        }

        let mut min_feerate_vb = 1;
        let mut replaced_fee = bitcoin::Amount::ZERO;
        for conflict in &conflicts {
            let tx = if conflict == txid {
                // Sized as relayed, with its witnesses: the copy the poller stored, else the
                // stored PSBT finalized, for a broadcast the poller has not caught up with. Short
                // of both, the unsigned transaction understates its size and so overstates its
                // feerate: the replacement then pays more than it must, never less.
                tx_getter
                    .get_tx(txid)
                    .or_else(|| {
                        // Only finalized to be sized, by the plain finalizer: the Bitcoin Blake2b
                        // one logs replay warnings that are only meant for a broadcast.
                        let mut psbt = prev_psbt
                            .filter(|_| !self.config.bitcoin_config.chain.is_blake2b())?
                            .clone();
                        psbt.finalize_mut(&self.secp).ok()?;
                        Some(psbt.extract_tx_unchecked_fee_rate())
                    })
                    .or_else(|| Some(prev_tx.clone()))
            } else {
                tx_getter.get_tx(conflict)
            };
            match tx.as_ref().map(&price) {
                Some(Some((fee, vsize))) => {
                    // The feerate is rounded down, so one more is strictly above it.
                    if let Some(floor) = fee
                        .to_sat()
                        .checked_div(vsize)
                        .and_then(|feerate| feerate.checked_add(1))
                    {
                        min_feerate_vb = std::cmp::max(min_feerate_vb, floor);
                    }
                    replaced_fee = add_fee(replaced_fee, fee);
                }
                Some(None) => {
                    return Err(CommandError::MempoolUnavailable(format!(
                        "cannot work out the fee of {}, which the replacement of {} must outbid, \
                         from our records",
                        conflict, txid
                    )));
                }
                None => log::warn!(
                    "Cannot price {} from our records: the replacement of {} may pay too little \
                     to be relayed",
                    conflict,
                    txid
                ),
            }
        }

        // Our own transactions spending their outputs are evicted along with them, so their fees
        // count towards what the replacement must pay as well.
        let mut seen: HashSet<bitcoin::Txid> = conflicts.iter().copied().collect();
        let mut parents = conflicts;
        while let Some(parent) = parents.pop() {
            for child in spending
                .values()
                .filter(|coin| coin.outpoint.txid == parent)
                .filter_map(|coin| coin.spend_txid)
            {
                if !seen.insert(child) {
                    continue;
                }
                match tx_getter.get_tx(&child).as_ref().and_then(&price) {
                    Some((fee, _)) => replaced_fee = add_fee(replaced_fee, fee),
                    None => log::warn!(
                        "Cannot price {}, which spends from {}: the replacement of {} may pay too \
                         little to be relayed",
                        child,
                        parent,
                        txid
                    ),
                }
                parents.push(child);
            }
        }

        log::info!(
            "Backend cannot see the mempool: the replacement of {} must pay at least {} sat/vb \
             and more than {} sat of replaced fees, per our records",
            txid,
            min_feerate_vb,
            replaced_fee.to_sat()
        );
        Ok((min_feerate_vb, replaced_fee))
    }
}

impl DaemonControl {
    /// Ask the poller to run a sync **now**, bypassing both the
    /// remaining sleep on the poll-interval timer and the Esplora
    /// backend's smart-poll tip-guard. Fire-and-forget — returns
    /// immediately rather than waiting on the poller's completion
    /// signal; the GUI will see the result through the regular
    /// state-cache refresh.
    ///
    /// Intended UX hooks (called by the GUI):
    /// - The app regains focus after a stretch in the background.
    /// - The user opens the Receive panel.
    /// - A new receive address is generated.
    ///
    /// In all three cases the user has just expressed "I want
    /// fresh data" and the few extra HTTP requests this triggers
    /// (~80 against the active Esplora provider) easily fit
    /// inside the free-tier hourly budget at the current 10-min
    /// poll interval.
    pub fn request_sync(&self) {
        // Record the eager request outside the backend mutex. The poller holds
        // that mutex for the full history scan, so taking it here would make
        // this fire-and-forget RPC block and starve live get_info calls.
        // The poller transfers the flag to the backend before its next poll.
        self.history_sync_cache.request_eager_sync();
        // Non-blocking send: the poller channel is a `sync_channel(1)`,
        // so a plain `send` would block under burst pressure (e.g.
        // user opens Receive twice in rapid succession while a sync
        // is mid-flight). Blocking the JSON-RPC handler thread for
        // however long that sync takes is exactly what we're trying
        // to avoid for a UX-triggered "kick the poller" request.
        //
        // `try_send` semantics map cleanly:
        //   * Ok            – wake-up enqueued; poller will wake now.
        //   * Err(Full)     – a wake-up (ours or the broadcast-spend
        //                     flow's `PollNow(sender)`) is already in
        //                     flight. The eager flag is already set
        //                     above, so whichever tick fires next
        //                     honors it. Drop our signal silently —
        //                     it would be redundant.
        //   * Err(Disconnected) – poller is gone; nothing left to
        //                     do but log.
        match self.poller_sender.try_send(PollerMessage::PollNowNoAck) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                log::debug!(
                    "request_sync: poller channel full; eager flag will be \
                     honored by the already-pending wake-up"
                );
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                log::warn!("request_sync: poller channel disconnected");
            }
        }
    }

    /// Get information about the current state of the daemon
    pub fn get_info(&self) -> GetInfoResult {
        let mut db_conn = self.db.connection();
        let block_height = db_conn.chain_tip().map(|tip| tip.height).unwrap_or(0);
        let wallet = db_conn.wallet();
        let receive_index: u32 = db_conn.receive_index().into();
        let change_index: u32 = db_conn.change_index().into();
        let history_sync = self.history_sync_cache.snapshot();
        let rescan_progress = wallet.rescan_timestamp.map(|_| {
            // A running full scan holds the backend lock until it finishes, which over a
            // slow server is most of an hour. Report its progress from the lock-free
            // record instead of waiting for it.
            let backend_progress = match history_sync.full_scan_progress() {
                Some(progress) => Ok(Some(progress)),
                None => self.bitcoin.rescan_progress_result(),
            };
            match backend_progress {
                Ok(progress) => progress.unwrap_or(1.0),
                // The rescan is still recorded as pending. An unreachable node
                // is neither completion (1.0) nor a reason to panic (#589).
                Err(e) => {
                    log::warn!("Cannot read the rescan progress: {}", e);
                    0.0
                }
            }
        });
        // Split the poller's single chain alert into the two distinct fields it drives.
        // Read once so the two can never disagree, and keep an unknown divergence out of
        // `refused_reorg_depth`: it is not a rollback of a known depth.
        let (refused_reorg_depth, chain_divergence) = match self.reorg_alert_cache.load() {
            crate::bitcoin::ChainAlert::None => (None, false),
            crate::bitcoin::ChainAlert::RefusedReorg(depth) => (Some(depth), false),
            crate::bitcoin::ChainAlert::Diverged => (None, true),
        };
        GetInfoResult {
            version: VERSION.to_string(),
            network: self.config.bitcoin_config.network,
            chain_id: Some(self.config.bitcoin_config.chain),
            block_height,
            // Read the poller's lock-free mirror rather than
            // `self.bitcoin.sync_progress()`, which would block on the
            // `BitcoinInterface` mutex the poller holds across a full wallet
            // scan — the cause of the GUI's "Starting daemon…" stall.
            sync: self.sync_progress_cache.load().rounded_up_progress(),
            descriptors: GetInfoDescriptors {
                main: self.config.main_descriptor.clone(),
            },
            imported_wallet: self
                .config
                .main_descriptor
                .as_imported()
                .and_then(|imported| {
                    let state = db_conn.imported_history()?;
                    let wallet_identity = imported
                        .identity(self.config.bitcoin_config.chain)
                        .ok()?
                        .to_string();
                    Some(ImportedWalletSync {
                        wallet_identity,
                        chain_id: self.config.bitcoin_config.chain,
                        birthday: state.birthday,
                        initial_lookahead: state.initial_lookahead,
                        discovery_target: state.discovery_target,
                        receive_scanned: state.receive_scanned,
                        change_scanned: state.change_scanned,
                        history_complete: state.history_complete,
                    })
                }),
            rescan_progress,
            refused_reorg_depth,
            chain_divergence,
            timestamp: wallet.timestamp,
            last_poll_timestamp: wallet.last_poll_timestamp,
            history_sync,
            receive_index,
            change_index,
        }
    }

    /// Current wallet database tip identity for embedded, read-only callers.
    /// An empty chain has no identity and must not authorize a prepared draft.
    pub fn chain_tip_hash(&self) -> Option<bitcoin::BlockHash> {
        self.db.connection().chain_tip().map(|tip| tip.hash)
    }

    /// Get a new deposit address. This will always generate a new deposit address, regardless of
    /// whether it was actually used.
    pub fn get_new_address(&self) -> GetAddressResult {
        self.try_get_new_address()
            .expect("Vault receive allocation must succeed")
    }

    pub fn try_get_new_address(&self) -> Result<GetAddressResult, CommandError> {
        if let Some(imported) = self.config.main_descriptor.as_imported() {
            let (address, index) = self
                .db
                .reserve_imported_receive(self.config.bitcoin_config.chain, imported)
                .map_err(CommandError::ChangeReservation)?;
            return Ok(GetAddressResult::new(address, index));
        }
        let mut db_conn = self.db.connection();
        let index = db_conn.receive_index();
        let new_index = index
            .increment()
            .expect("Can't get into hardened territory");
        db_conn.set_receive_index(new_index, &self.secp);
        let address = self
            .config
            .main_descriptor
            .receive_descriptor()
            .derive(u32::from(new_index))
            .expect("receive derivation index is valid")
            .address(self.config.bitcoin_config.network)
            .expect("supported wallet address");
        Ok(GetAddressResult::new(address, new_index))
    }

    /// Update derivation indexes
    pub fn update_deriv_indexes(
        &self,
        receive: Option<u32>,
        change: Option<u32>,
    ) -> Result<UpdateDerivIndexesResult, CommandError> {
        let mut db_conn = self.db.connection();

        const MAX_INCREMENT_GAP: u32 = 1_000;

        let db_receive = db_conn.receive_index().into();
        let mut final_receive = db_receive;

        let db_change = db_conn.change_index().into();
        let mut final_change = db_change;

        if let Some(index) = receive {
            ChildNumber::from_normal_idx(index)
                .map_err(|_| CommandError::InvalidDerivationIndex)?;
            if index > db_receive {
                let delta = (index - db_receive).min(MAX_INCREMENT_GAP);
                let index = db_receive + delta;
                final_receive = index;
                match ChildNumber::from_normal_idx(index) {
                    Ok(i) => {
                        db_conn.set_receive_index(i, &self.secp);
                    }
                    Err(_) => return Err(CommandError::InvalidDerivationIndex),
                }
            }
        }

        if let Some(index) = change {
            ChildNumber::from_normal_idx(index)
                .map_err(|_| CommandError::InvalidDerivationIndex)?;
            if index > db_change {
                let delta = (index - db_change).min(MAX_INCREMENT_GAP);
                let index = db_change + delta;
                final_change = index;
                match ChildNumber::from_normal_idx(index) {
                    Ok(i) => {
                        db_conn.set_change_index(i, &self.secp);
                    }
                    Err(_) => return Err(CommandError::InvalidDerivationIndex),
                }
            }
        }

        Ok(UpdateDerivIndexesResult {
            receive: final_receive,
            change: final_change,
        })
    }

    /// list addresses
    pub fn list_addresses(
        &self,
        start_index: Option<u32>,
        count: Option<u32>,
    ) -> Result<ListAddressesResult, CommandError> {
        let mut db_conn = self.db.connection();
        let receive_index: u32 = db_conn.receive_index().into();
        let change_index: u32 = db_conn.change_index().into();

        // If a start index isn't provided, we derive from index 0. Make sure the provided index is
        // unhardened.
        let start_index = bip32::ChildNumber::from_normal_idx(start_index.unwrap_or(0))
            .map_err(|_| CommandError::InvalidDerivationIndex)?;
        let start_index_u32: u32 = start_index.into();

        // Derive the end index (ie, the first index to not be returned) from the count of
        // addresses to provide. If no count was passed, use the next derivation index between
        // change and receive as end index.
        let end_index = if let Some(c) = count {
            start_index_u32
                .checked_add(c)
                .ok_or(CommandError::InvalidDerivationIndex)?
        } else {
            // `end_index` will not be included so add 1 in order to include the last used index.
            receive_index
                .max(change_index)
                .checked_add(1)
                .ok_or(CommandError::InvalidDerivationIndex)?
        };

        // Derive all receive and change addresses for the queried range.
        let addresses: Result<Vec<AddressInfo>, CommandError> = (start_index_u32..end_index)
            .map(|index| {
                let child = bip32::ChildNumber::from_normal_idx(index)
                    .map_err(|_| CommandError::InvalidDerivationIndex)?;

                let receive = self
                    .config
                    .main_descriptor
                    .receive_descriptor()
                    .derive(u32::from(child))
                    .map_err(|_| CommandError::InvalidDerivationIndex)?
                    .address(self.config.bitcoin_config.network)
                    .map_err(|_| CommandError::InvalidDerivationIndex)?;

                let change = self
                    .config
                    .main_descriptor
                    .change_descriptor()
                    .derive(u32::from(child))
                    .map_err(|_| CommandError::InvalidDerivationIndex)?
                    .address(self.config.bitcoin_config.network)
                    .map_err(|_| CommandError::InvalidDerivationIndex)?;

                Ok(AddressInfo {
                    index,
                    receive,
                    change,
                })
            })
            .collect();
        Ok(ListAddressesResult::new(addresses?))
    }

    /// List revealed addresses. Addresses will be returned in order of
    /// descending derivation index.
    ///
    /// # Parameters
    ///
    /// - `is_change`: set to `false` to return receive addresses and `true` for change addresses.
    ///
    /// - `exclude_used`:  set to `true` to return only those revealed addresses that
    ///   are unused by any coins in the wallet.
    ///
    /// - `limit`: the maximum number of addresses to return.
    ///
    /// - `start_index`: the derivation index from which to start listing addresses. As addresses are
    ///   returned in descending order, `start_index` is the highest index that can be returned.
    ///   If set to `None`, then addresses will be returned starting from the last revealed index.
    ///   As there are no revealed addresses with a derivation index higher than the last revealed index,
    ///   setting this parameter to a higher value will be the same as setting it to `None`.
    pub fn list_revealed_addresses(
        &self,
        is_change: bool,
        exclude_used: bool,
        limit: usize,
        start_index: Option<ChildNumber>,
    ) -> Result<ListRevealedAddressesResult, CommandError> {
        let mut db_conn = self.db.connection();

        let (desc, last_revealed) = if is_change {
            (
                self.config.main_descriptor.change_descriptor(),
                db_conn.change_index(),
            )
        } else {
            (
                self.config.main_descriptor.receive_descriptor(),
                db_conn.receive_index(),
            )
        };

        // Determine the index to start deriving addresses from, ensuring it is not higher than the last revealed.
        let start_index = start_index.unwrap_or(last_revealed).min(last_revealed);

        // Count how many times each (used) address has been used.
        let mut used_counts = HashMap::<ChildNumber, u32>::new();
        // TODO: consider adding DB method to get coins or used indices by index range.
        for coin in db_conn.coins(&[], &[]).values() {
            if coin.is_change == is_change && coin.derivation_index <= start_index {
                *used_counts.entry(coin.derivation_index).or_insert(0) += 1;
            }
        }

        let mut addresses = Vec::<_>::with_capacity(limit);
        let mut continue_from = None;
        // This will store (index, address) pairs.
        let mut derived_addresses = Vec::<_>::with_capacity(limit);
        // Iterate in descending order.
        for i in (0..=start_index.into()).rev() {
            let index = ChildNumber::from(i);
            if derived_addresses.len() == limit {
                // We've reached the limit. There may be more addresses to list using pagination.
                continue_from = Some(index);
                break;
            }
            if !exclude_used || !used_counts.contains_key(&index) {
                let addr = desc
                    .derive(u32::from(index))
                    .map_err(|_| CommandError::InvalidDerivationIndex)?
                    .address(self.config.bitcoin_config.network)
                    .map_err(|_| CommandError::InvalidDerivationIndex)?;
                derived_addresses.push((index, addr));
            }
        }
        // Now get the labels from DB (in multiple chunks).
        let mut labels = HashMap::<String, String>::with_capacity(derived_addresses.len());
        const CHUNK_SIZE: usize = 100;
        for chunk in derived_addresses.chunks(CHUNK_SIZE) {
            let items = chunk
                .iter()
                .map(|(_, addr)| LabelItem::Address(addr.clone()))
                .collect::<HashSet<_>>();
            labels.extend(db_conn.labels(&items));
        }
        for (index, address) in derived_addresses {
            let label = labels.get(&address.to_string()).cloned();
            addresses.push(ListRevealedAddressesEntry {
                index,
                address,
                label,
                used_count: *used_counts.get(&index).unwrap_or(&0),
            });
        }
        Ok(ListRevealedAddressesResult {
            addresses,
            continue_from,
        })
    }

    /// Get a list of all known coins, optionally by status and/or outpoint.
    pub fn list_coins(
        &self,
        statuses: &[CoinStatus],
        outpoints: &[bitcoin::OutPoint],
    ) -> ListCoinsResult {
        let mut db_conn = self.db.connection();
        let coins: Vec<ListCoinsEntry> = db_conn
            .coins(statuses, outpoints)
            .into_values()
            .map(|coin| {
                let Coin {
                    amount,
                    outpoint,
                    block_info,
                    spend_txid,
                    spend_block,
                    is_immature,
                    is_change,
                    is_from_self,
                    derivation_index,
                    ..
                } = coin;
                let spend_info = spend_txid.map(|txid| LCSpendInfo {
                    txid,
                    height: spend_block.map(|b| b.height),
                });
                let block_height = block_info.map(|b| b.height);
                let address = self
                    .derived_desc(&coin)
                    .address(self.config.bitcoin_config.network)
                    .expect("supported wallet address");
                ListCoinsEntry {
                    address,
                    amount,
                    derivation_index,
                    outpoint,
                    block_height,
                    spend_info,
                    is_immature,
                    is_change,
                    is_from_self,
                }
            })
            .collect();
        ListCoinsResult { coins }
    }

    /// Sum of `witness_utxo` values. Used only when an MCP fee cap is present,
    /// and refuses to invent a fee when an input has no witness UTXO.
    fn witness_input_value(psbt: &Psbt) -> Result<u64, CommandError> {
        if psbt.inputs.is_empty() || psbt.inputs.len() != psbt.unsigned_tx.input.len() {
            return Err(CommandError::FeeMeasurement);
        }
        let mut total = 0u64;
        for input in &psbt.inputs {
            let value = input
                .witness_utxo
                .as_ref()
                .ok_or(CommandError::FeeMeasurement)?
                .value
                .to_sat();
            total = total
                .checked_add(value)
                .ok_or(CommandError::FeeMeasurement)?;
        }
        Ok(total)
    }

    /// Fee and virtual size of an unsigned PSBT. The virtual size includes the
    /// descriptor's maximum witness satisfaction, because the unsigned
    /// transaction's witness is still empty. Failure means the caller must
    /// not commit a change index for this construction.
    fn fee_and_vsize(&self, psbt: &Psbt) -> Result<(u64, u64), CommandError> {
        if psbt.inputs.is_empty() || psbt.inputs.len() != psbt.unsigned_tx.input.len() {
            return Err(CommandError::FeeMeasurement);
        }
        let mut total_in = 0u64;
        for input in &psbt.inputs {
            let value = input
                .witness_utxo
                .as_ref()
                .ok_or(CommandError::FeeMeasurement)?
                .value
                .to_sat();
            total_in = total_in
                .checked_add(value)
                .ok_or(CommandError::FeeMeasurement)?;
        }
        let mut total_out = 0u64;
        for output in &psbt.unsigned_tx.output {
            total_out = total_out
                .checked_add(output.value.to_sat())
                .ok_or(CommandError::FeeMeasurement)?;
        }
        let fee = total_in
            .checked_sub(total_out)
            .filter(|fee| *fee > 0)
            .ok_or(CommandError::FeeMeasurement)?;
        let vsize = match &self.config.main_descriptor {
            WalletDescriptor::Vault(vault) => vault.unsigned_tx_max_vbytes(&psbt.unsigned_tx, true),
            WalletDescriptor::Imported(_) => return Err(CommandError::FeeMeasurement),
        };
        if vsize == 0 {
            return Err(CommandError::FeeMeasurement);
        }
        Ok((fee, vsize))
    }

    fn verify_capped_broadcast_fee(
        &self,
        prepared_psbt: &Psbt,
        final_tx: &bitcoin::Transaction,
        (max_fee, max_rate): (u64, u64),
    ) -> Result<(), CommandError> {
        let total_in = Self::witness_input_value(prepared_psbt)?;
        let prepared_vsize = self.fee_and_vsize(prepared_psbt)?.1;
        let total_out = final_tx
            .output
            .iter()
            .try_fold(0u64, |total, output| {
                total.checked_add(output.value.to_sat())
            })
            .ok_or(CommandError::FeeMeasurement)?;
        let fee = total_in
            .checked_sub(total_out)
            .filter(|fee| *fee > 0)
            .ok_or(CommandError::FeeMeasurement)?;
        if fee > max_fee {
            return Err(CommandError::FeeCeilingExceeded);
        }
        if spend::signed_fee_exceeds_cap(fee, prepared_vsize, max_fee, max_rate) {
            return Err(CommandError::FeeRateExceeded);
        }
        Ok(())
    }

    pub fn create_spend(
        &self,
        destinations: &HashMap<bitcoin::Address<bitcoin::address::NetworkUnchecked>, u64>,
        coins_outpoints: &[bitcoin::OutPoint],
        feerate_vb: u64,
        change_address: Option<bitcoin::Address<bitcoin::address::NetworkUnchecked>>,
    ) -> Result<CreateSpendResult, CommandError> {
        self.create_spend_within_limits(
            destinations,
            coins_outpoints,
            feerate_vb,
            change_address,
            None,
        )
    }

    /// Build a spend and, when `max_fee_sats` is set, reject a fee or measured
    /// rate above the caller's limits before committing a change index.
    pub fn create_spend_within_limits(
        &self,
        destinations: &HashMap<bitcoin::Address<bitcoin::address::NetworkUnchecked>, u64>,
        coins_outpoints: &[bitcoin::OutPoint],
        feerate_vb: u64,
        change_address: Option<bitcoin::Address<bitcoin::address::NetworkUnchecked>>,
        max_fee_sats: Option<u64>,
    ) -> Result<CreateSpendResult, CommandError> {
        self.require_ordinary_imported_spend_support()?;
        let is_self_send = destinations.is_empty();
        // For self-send, the coins must be specified.
        if is_self_send && coins_outpoints.is_empty() {
            return Err(CommandError::NoOutpointForSelfSend);
        }
        if feerate_vb < 1 {
            return Err(CommandError::InvalidFeerate(feerate_vb));
        }
        let mut db_conn = self.db.connection();
        let mut tx_getter = DbTxGetter::new(&self.db);

        // Prepare the destination addresses.
        let mut destinations_checked = Vec::with_capacity(destinations.len());
        for (address, value_sat) in destinations {
            let address = self.validate_address(address.clone())?;
            let amount = bitcoin::Amount::from_sat(*value_sat);
            let address = self.spend_addr(&mut db_conn, address);
            destinations_checked.push((address, amount));
        }

        // The change address to be used if a change output needs to be created. It may be
        // specified by the caller (for instance for the purpose of a sweep, or to avoid us
        // creating a new change address on every call). A fresh address is only peeked here;
        // it is committed after a successful build that actually carries change.
        let fresh_change = change_address.is_none();
        let mut change_address = match change_address {
            Some(addr) => self.spend_addr(&mut db_conn, self.validate_address(addr)?),
            None => self.next_change_addr(&mut db_conn)?,
        };
        if self.config.main_descriptor.as_vault().is_some() {
            let last_change = db_conn.change_index();
            self.authorize_vault_change(&change_address, &destinations_checked, last_change)?;
        }

        // The candidate coins will be either all optional or all mandatory.
        // If no coins have been specified, then coins will be selected automatically for
        // the spend from a set of optional candidates.
        // Otherwise, only the specified coins will be used, all as mandatory candidates.
        let candidate_coins: Vec<CandidateCoin> = if coins_outpoints.is_empty() {
            // From our unconfirmed coins, we only include those that are from self
            // since unconfirmed external deposits are more at risk of being dropped
            // unexpectedly from the mempool as they are beyond the user's control.
            db_conn
                .coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                .into_iter()
                .filter_map(|(op, c)| {
                    if c.block_info.is_some() {
                        Some(Ok((c, None))) // confirmed coins have no ancestor info
                    } else if c.is_from_self {
                        // In case the mempool_entry is None, the coin will be included without
                        // any ancestor info.
                        Some(self.ancestor_info(&op.txid).map(|info| (c, info)))
                    } else {
                        None
                    }
                })
                .map(|res| {
                    res.map(|(c, ancestor_info)| {
                        coin_to_candidate(
                            &c,
                            /*must_select=*/ false,
                            /*sequence=*/ None,
                            ancestor_info,
                        )
                    })
                })
                .collect::<Result<_, _>>()?
        } else {
            // Query from DB and sanity check the provided coins to spend.
            let coins = db_conn.coins(&[], coins_outpoints);
            for op in coins_outpoints {
                let coin = coins.get(op).ok_or(CommandError::UnknownOutpoint(*op))?;
                if coin.is_spent() {
                    return Err(CommandError::AlreadySpent(*op));
                }
                if coin.is_immature {
                    return Err(CommandError::ImmatureCoinbase(*op));
                }
            }
            coins
                .into_iter()
                .map(|(op, c)| {
                    let ancestor_info = if c.block_info.is_none() {
                        // We include any non-change coins here as they have been selected by the caller.
                        // If the unconfirmed coin's transaction is no longer in the mempool, keep the
                        // coin as a candidate but without any ancestor info (same as confirmed candidate).
                        self.ancestor_info(&op.txid)?
                    } else {
                        None
                    };
                    Ok(coin_to_candidate(
                        &c,
                        /*must_select=*/ true,
                        /*sequence=*/ None,
                        ancestor_info,
                    ))
                })
                .collect::<Result<_, CommandError>>()?
        };

        // Create the PSBT. A concurrent Claim or ordinary spend may reserve the address
        // we peeked. In that case retry with the new next address; the compare-and-commit
        // operation itself leaves the database untouched on a lost race.
        let locktime = self.anti_fee_sniping_locktime();
        let mut change_commit_conflicts = 0;
        loop {
            let change_info = change_address.info;
            let CreateSpendRes {
                psbt,
                has_change,
                warnings,
            } = match match &self.config.main_descriptor {
                WalletDescriptor::Vault(vault) if max_fee_sats.is_some() => {
                    spend::create_spend_with_network_dust(
                        vault,
                        &self.secp,
                        &mut tx_getter,
                        &destinations_checked,
                        &candidate_coins,
                        SpendTxFees::Regular(feerate_vb),
                        change_address,
                        locktime,
                    )
                }
                WalletDescriptor::Vault(vault) => create_spend(
                    vault,
                    &self.secp,
                    &mut tx_getter,
                    &destinations_checked,
                    &candidate_coins,
                    SpendTxFees::Regular(feerate_vb),
                    change_address,
                    locktime,
                ),
                WalletDescriptor::Imported(imported) => imported_spend::create_imported_spend(
                    imported,
                    &self.secp,
                    &mut tx_getter,
                    &destinations_checked,
                    &candidate_coins,
                    SpendTxFees::Regular(feerate_vb),
                    change_address,
                    locktime,
                ),
            } {
                Ok(res) => res,
                Err(SpendCreationError::CoinSelection(e)) => {
                    return Ok(CreateSpendResult::InsufficientFunds { missing: e.missing });
                }
                Err(e) => {
                    return Err(e.into());
                }
            };
            if let Some(max_fee) = max_fee_sats {
                let (fee, vsize) = self.fee_and_vsize(&psbt)?;
                if fee > max_fee {
                    return Err(CommandError::FeeCeilingExceeded);
                }
                if fee.div_ceil(vsize) > feerate_vb {
                    return Err(CommandError::FeeRateExceeded);
                }
            }
            if has_change && fresh_change && !self.commit_change_if_next(&change_info)? {
                change_commit_conflicts += 1;
                if change_commit_conflicts >= CHANGE_COMMIT_CONFLICT_LIMIT {
                    return Err(CommandError::ChangeReservationContended);
                }
                change_address = self.next_change_addr(&mut db_conn)?;
                continue;
            }
            for (addr, _) in destinations_checked {
                self.maybe_increase_last_deriv_index(&mut db_conn, &addr.info);
            }
            if has_change {
                self.maybe_increase_last_deriv_index(&mut db_conn, &change_info);
            }

            // The imported spend's exact unsigned construction must exist
            // before any externally signed PSBT can be accepted against it.
            if self.config.main_descriptor.as_imported().is_some() {
                db_conn
                    .store_imported_construction(&psbt)
                    .map_err(CommandError::ImportedSpendValidation)?;
            }

            return Ok(CreateSpendResult::Success {
                psbt,
                warnings: warnings.iter().map(|w| w.to_string()).collect(),
            });
        }
    }

    pub fn update_spend(&self, mut psbt: Psbt) -> Result<(), CommandError> {
        self.require_ordinary_imported_spend_support()?;
        if let Some(imported) = self.config.main_descriptor.as_imported() {
            backfill_previous_transactions(&mut psbt, &mut DbTxGetter::new(&self.db))?;
            let mut db_conn = self.db.connection();
            let txid = psbt.unsigned_tx.compute_txid();
            let stored = db_conn.spend_tx(&txid).ok_or_else(|| {
                CommandError::ImportedSpendValidation(
                    "spend was not created and saved by this imported wallet".into(),
                )
            })?;
            let construction = imported_construction(&stored);
            imported_spend::verify_imported_signatures(imported, &construction, &psbt, &self.secp)
                .map_err(|error| CommandError::ImportedSpendValidation(error.to_string()))?;
            let merged = {
                let mut stored = stored.clone();
                for (old, incoming) in stored.inputs.iter_mut().zip(&psbt.inputs) {
                    for (key, signature) in &incoming.partial_sigs {
                        if old
                            .partial_sigs
                            .get(key)
                            .is_some_and(|existing| existing != signature)
                        {
                            return Err(CommandError::ImportedSpendValidation(
                                "conflicting signature for the same signer".into(),
                            ));
                        }
                        old.partial_sigs.insert(*key, *signature);
                    }
                }
                imported_spend::verify_imported_signatures(
                    imported,
                    &construction,
                    &stored,
                    &self.secp,
                )
                .map_err(|error| CommandError::ImportedSpendValidation(error.to_string()))?;
                stored
            };
            if db_conn
                .spend_broadcast_txid(&txid)
                .map_err(CommandError::ImportedSpendValidation)?
                .is_some()
            {
                if merged == stored {
                    return Ok(());
                }
                return Err(CommandError::ImportedSpendValidation(
                    "a broadcast spend cannot accept more signatures".into(),
                ));
            }
            db_conn.store_spend(&merged);
            return Ok(());
        }
        // On Bitcoin Blake2b the value verified is the value stored. The
        // verifier authenticates every prevout through `non_witness_utxo`,
        // which the public `updatespend` contract never required (a PSBT with
        // `witness_utxo` and a committing `witness_script` was, and stays,
        // accepted): backfill it from the wallet first — the same helper
        // `broadcast_spend` runs — then hold the PSBT to the chain's rules
        // before the existence check, so the first insert stores only what
        // every later update and the finaliser accept, and store that
        // backfilled, verified PSBT rather than the caller's bytes.
        if self.config.bitcoin_config.chain.is_blake2b() {
            backfill_previous_transactions(&mut psbt, &mut DbTxGetter::new(&self.db))?;
        }
        validate_spend_for_chain(self.config.bitcoin_config.chain, &psbt, &self.secp)?;
        let mut db_conn = self.db.connection();
        let tx = &psbt.unsigned_tx;

        // If the transaction already exists in DB, merge the signatures for each input on a best
        // effort basis.
        let txid = tx.compute_txid();
        if let Some(db_psbt) = db_conn.spend_tx(&txid) {
            psbt = merge_spend_signatures(
                self.config.bitcoin_config.chain,
                db_psbt,
                &psbt,
                &self.secp,
            )?;
        } else {
            // If the transaction doesn't exist in DB already, sanity check its inputs.
            // FIXME: should we allow for external inputs?
            let outpoints: Vec<bitcoin::OutPoint> =
                tx.input.iter().map(|txin| txin.previous_output).collect();
            let coins = db_conn.coins_by_outpoints(&outpoints);
            if coins.len() != outpoints.len() {
                for op in outpoints {
                    if !coins.contains_key(&op) {
                        return Err(CommandError::UnknownOutpoint(op));
                    }
                }
            }
            if let Some(vault) = self.config.main_descriptor.as_vault() {
                validate_new_vault_psbt(
                    vault,
                    &psbt,
                    &coins,
                    self.config.bitcoin_config.network,
                    &self.secp,
                )?;
            }
        }

        // Finally, insert (or update) the PSBT in database.
        db_conn.store_spend(&psbt);

        Ok(())
    }

    pub fn update_labels(&self, items: &HashMap<LabelItem, Option<String>>) {
        let mut db_conn = self.db.connection();
        db_conn.update_labels(items);
    }

    pub fn get_labels(&self, items: &HashSet<LabelItem>) -> GetLabelsResult {
        let mut db_conn = self.db.connection();
        GetLabelsResult {
            labels: db_conn.labels(items),
        }
    }

    pub fn get_labels_bip329(&self, offset: u32, limit: u32) -> GetLabelsBip329Result {
        let mut db_conn = self.db.connection();
        GetLabelsBip329Result {
            labels: db_conn.get_labels_bip329(offset, limit),
        }
    }

    pub fn list_spend(
        &self,
        txids: Option<Vec<bitcoin::Txid>>,
    ) -> Result<ListSpendResult, CommandError> {
        if let Some(ids) = &txids {
            if ids.is_empty() {
                return Err(CommandError::EmptyFilterList);
            }
        }

        let mut db_conn = self.db.connection();
        let spend_psbts = db_conn.list_spend();

        let txids_set: Option<HashSet<_>> = txids.as_ref().map(|list| list.iter().collect());
        let spend_txs = spend_psbts
            .into_iter()
            .filter_map(|(psbt, updated_at, broadcast_txid)| {
                if let Some(set) = &txids_set {
                    if !set.contains(&psbt.unsigned_tx.compute_txid()) {
                        return None;
                    }
                }
                Some(ListSpendEntry {
                    psbt,
                    updated_at,
                    broadcast_txid,
                })
            })
            .collect();
        Ok(ListSpendResult { spend_txs })
    }

    pub fn delete_spend(&self, txid: &bitcoin::Txid) {
        let mut db_conn = self.db.connection();
        db_conn.delete_spend(txid);
    }

    /// Finalize and broadcast this stored Spend transaction.
    pub fn broadcast_spend(&self, txid: &bitcoin::Txid) -> Result<(), CommandError> {
        self.require_ordinary_imported_spend_support()?;
        let mut db_conn = self.db.connection();

        // First, try to finalize the spending transaction with the elements contained
        // in the PSBT.
        let mut spend_psbt = db_conn
            .spend_tx(txid)
            .ok_or(CommandError::UnknownSpend(*txid))?;

        // A stored PSBT may predate fee-source authentication (P2-A) or have been
        // imported through `update_spend` without its previous transactions.
        // Backfill them from the wallet before re-verifying.
        backfill_previous_transactions(&mut spend_psbt, &mut DbTxGetter::new(&self.db))?;

        // CC-DESK-003: recheck the fee against the reviewed preparation size.
        // Actual ECDSA signatures may be shorter than the maximum witness used
        // when the fee was selected. Dividing the unchanged fee by final vsize
        // would reject an ordinary signed spend at the broadcast boundary.
        let fee_cap = spend::mcp_fee_cap(&spend_psbt);
        let capped_prepared_psbt = fee_cap.map(|_| spend_psbt.clone());
        let final_tx = match &self.config.main_descriptor {
            WalletDescriptor::Vault(vault) => {
                spend::reverify_spend_before_broadcast(vault, &spend_psbt)?;
                finalize_spend_for_chain(self.config.bitcoin_config.chain, spend_psbt, &self.secp)?
            }
            WalletDescriptor::Imported(imported) => {
                let construction = imported_construction(&spend_psbt);
                imported_spend::finalize_imported_spend(
                    imported,
                    &construction,
                    &spend_psbt,
                    &self.secp,
                )
                .map_err(|error| CommandError::ImportedSpendValidation(error.to_string()))?
                .transaction()
                .clone()
            }
        };
        if let (Some(cap), Some(prepared_psbt)) = (fee_cap, capped_prepared_psbt.as_ref()) {
            self.verify_capped_broadcast_fee(prepared_psbt, &final_tx, cap)?;
        }
        if self.config.main_descriptor.as_imported().is_some() {
            if let Some(recorded) = db_conn
                .spend_broadcast_txid(txid)
                .map_err(CommandError::ImportedSpendValidation)?
            {
                if recorded == final_tx.compute_txid() {
                    return Ok(());
                }
                return Err(CommandError::ImportedSpendValidation(
                    "stored broadcast transaction ID conflicts with finalized spend".into(),
                ));
            }
        }
        self.bitcoin
            .broadcast_tx(&final_tx)
            .map_err(CommandError::TxBroadcast)?;
        if self.config.main_descriptor.as_imported().is_some() {
            db_conn.record_broadcast_txid(txid, &final_tx.compute_txid()).map_err(|error|
                CommandError::ImportedSpendValidation(format!(
                    "broadcast may have succeeded, but saving the actual transaction ID failed: {error}"
                )))?;
        }

        // Finally, update our state with the changes from this transaction.
        let (tx, rx) = mpsc::sync_channel(0);
        if let Err(e) = self.poller_sender.send(PollerMessage::PollNow(tx)) {
            log::error!("Error requesting update from poller: {}", e);
        }
        if let Err(e) = rx.recv() {
            log::error!("Error receiving completion signal from poller: {}", e);
        }

        Ok(())
    }

    /// Create PSBT to replace the given transaction using RBF.
    ///
    /// `txid` must either point to a PSBT in our database (not necessarily broadcast) or an
    /// unconfirmed spend transaction (whether or not any associated PSBT is saved in our database).
    ///
    /// `is_cancel` indicates whether to "cancel" the transaction by including only a single (change)
    /// output in the replacement or otherwise to keep the same (non-change) outputs and simply
    /// bump the fee.
    /// If `true`, the only output of the RBF transaction will be change and the inputs will include
    /// at least one of the inputs from the previous transaction. If `false`, all inputs from the previous
    /// transaction will be used in the replacement.
    /// In both cases:
    /// - if the previous transaction includes a change output to one of our own change addresses,
    ///   this same address will be used for change in the RBF transaction, if required. If the previous
    ///   transaction pays to more than one of our change addresses, then the one receiving the highest
    ///   value will be used as a change address and the others will be treated as non-change outputs.
    /// - the RBF transaction may include additional confirmed coins as inputs if required
    ///   in order to pay the higher fee (this applies also when replacing a self-send).
    ///
    /// `feerate_vb` is the target feerate for the RBF transaction (in sat/vb). If `None`, it will be set
    /// to 1 sat/vb larger than the feerate of the previous transaction, which is the minimum value allowed
    /// when using RBF.
    pub fn rbf_psbt(
        &self,
        txid: &bitcoin::Txid,
        is_cancel: bool,
        feerate_vb: Option<u64>,
    ) -> Result<CreateSpendResult, CommandError> {
        self.require_ordinary_imported_spend_support()?;
        let mut db_conn = self.db.connection();
        let mut tx_getter = DbTxGetter::new(&self.db);

        if is_cancel && feerate_vb.is_some() {
            return Err(CommandError::RbfError(RbfErrorInfo::SuperfluousFeerate));
        }

        let prev_psbt = db_conn.spend_tx(txid);
        let prev_tx = if let Some(psbt) = &prev_psbt {
            psbt.unsigned_tx.clone()
        } else {
            db_conn
                .coins(&[CoinStatus::Spending], &[])
                .into_values()
                .find(|c| c.spend_txid == Some(*txid))
                .and_then(|_| tx_getter.get_tx(txid))
                .ok_or(CommandError::UnknownSpend(*txid))?
        };
        if !prev_tx.is_explicitly_rbf() {
            return Err(CommandError::RbfError(RbfErrorInfo::NotSignaling));
        }
        let prev_outpoints: Vec<bitcoin::OutPoint> = prev_tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect();
        let prev_coins = db_conn.coins_by_outpoints(&prev_outpoints);
        // Make sure all prev outpoints are coins in our DB.
        if let Some(op) = prev_outpoints
            .iter()
            .find(|op| !prev_coins.contains_key(op))
        {
            return Err(CommandError::UnknownOutpoint(*op));
        }
        if let Some(op) = prev_coins.iter().find_map(|(_, coin)| {
            if coin.spend_block.is_some() {
                Some(coin.outpoint)
            } else {
                None
            }
        }) {
            return Err(CommandError::AlreadySpent(op));
        }
        // Compute the minimal feerate and fee the replacement transaction must have to satisfy RBF
        // rules #3, #4 and #6 (see
        // https://github.com/bitcoin/bitcoin/blob/master/doc/policy/mempool-replacements.md). By
        // default (ie if the transaction we are replacing was dropped from the mempool) there is
        // no minimum absolute fee and the minimum feerate is 1, the minimum relay feerate.
        let (min_feerate_vb, descendant_fees) = if self.bitcoin.sees_mempool() {
            self.bitcoin
                .mempool_spenders_result(&prev_outpoints)
                // An unreachable node is not "the replaced transaction left the
                // mempool": that would drop the RBF minimums (#594).
                .map_err(CommandError::MempoolUnavailable)?
                .into_iter()
                .try_fold(
                    (1, bitcoin::Amount::from_sat(0)),
                    |(min_feerate, descendant_fee), entry| {
                        // A zero size or an overflowing fee is an unusable backend
                        // answer: an error, not a panic under the backend lock (#597).
                        let unusable = || {
                            CommandError::MempoolUnavailable(format!(
                                "unusable mempool entry while replacing {}: {:?}",
                                txid, entry
                            ))
                        };
                        let entry_feerate = entry
                            .fees
                            .base
                            .checked_div(entry.vsize)
                            .and_then(|feerate| feerate.to_sat().checked_add(1))
                            .ok_or_else(unusable)?;
                        let descendant_fee = descendant_fee
                            .checked_add(entry.fees.descendant)
                            .ok_or_else(unusable)?;
                        Ok::<_, CommandError>((
                            std::cmp::max(min_feerate, entry_feerate),
                            descendant_fee,
                        ))
                    },
                )?
        } else {
            // This backend's mempool always reads empty, which would pass for "dropped" and price
            // the replacement at 1 sat/vb, below the transaction it replaces: the network would
            // refuse it. Price it from what our own records show spending these coins instead.
            self.replacement_minimums_from_records(
                &mut db_conn,
                &mut tx_getter,
                txid,
                &prev_tx,
                prev_psbt.as_ref(),
                &prev_coins,
            )?
        };
        // Check replacement transaction's target feerate, if set, is high enough,
        // and otherwise set it to the min feerate found above.
        let feerate_vb = if is_cancel {
            min_feerate_vb
        } else {
            feerate_vb.ok_or(CommandError::RbfError(RbfErrorInfo::MissingFeerate))?
        };
        if feerate_vb < min_feerate_vb {
            return Err(CommandError::RbfError(RbfErrorInfo::TooLowFeerate(
                feerate_vb,
                min_feerate_vb,
            )));
        }
        // Get info about prev outputs to determine replacement outputs.
        let prev_derivs: Vec<_> = prev_tx
            .output
            .iter()
            .map(|txo| {
                let address = bitcoin::Address::from_script(
                    &txo.script_pubkey,
                    self.config.bitcoin_config.network,
                )
                .expect("address already used in finalized transaction");
                (
                    address.clone(),
                    txo.value,
                    db_conn.derivation_index_by_address(&address),
                )
            })
            .collect();
        // Set the previous change address to that of the change output with the largest value
        // and then largest index.
        let prev_change_address = prev_derivs
            .iter()
            .filter_map(|(addr, amt, deriv)| {
                if let Some((ind, true)) = &deriv {
                    Some((addr, amt, ind))
                } else {
                    None
                }
            })
            .max_by(|(_, amt_1, ind_1), (_, amt_2, ind_2)| amt_1.cmp(amt_2).then(ind_1.cmp(ind_2)))
            .map(|(addr, _, _)| addr)
            .cloned();
        // If not cancel, use all previous outputs as destinations, except for
        // the output corresponding to the change address we found above.
        // If cancel, the replacement will not have any destinations, only a change output.
        let destinations = if !is_cancel {
            prev_derivs
                .into_iter()
                .filter_map(|(addr, amt, _)| {
                    if prev_change_address.as_ref() != Some(&addr) {
                        Some((self.spend_addr(&mut db_conn, addr), amt))
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        // If there was no previous change address, peek at the next address for the
        // replacement. It is committed only after a successful replacement actually
        // carries change.
        let fresh_change = prev_change_address.is_none();
        let mut change_address = match prev_change_address {
            Some(addr) => self.spend_addr(&mut db_conn, addr),
            None => self.next_change_addr(&mut db_conn)?,
        };
        // If `!is_cancel`, we take the previous coins as mandatory candidates and add confirmed coins as optional.
        // Otherwise, we take the previous coins as optional candidates and let coin selection find the
        // best solution that includes at least one of these. If there are insufficient funds to create the replacement
        // transaction in this way, then we set candidates in the same way as for the `!is_cancel` case.
        let mut candidate_coins: Vec<CandidateCoin> = prev_coins
            .values()
            .map(|c| {
                // In case any previous coins are unconfirmed, we don't include their ancestor info
                // in the candidate as the replacement fee and feerate will be higher and any
                // additional fee to pay for ancestors should already have been taken into account
                // when including these coins in the previous transaction.
                coin_to_candidate(
                    c, /*must_select=*/ !is_cancel, /*sequence=*/ None,
                    /*ancestor_info=*/ None,
                )
            })
            .collect();
        let confirmed_cands: Vec<CandidateCoin> = db_conn
            .coins(&[CoinStatus::Confirmed], &[])
            .into_values()
            .filter_map(|c| {
                // Make sure we don't have duplicate candidates in case any of the coins are not
                // currently set as spending in the DB (and are therefore still confirmed).
                if !prev_coins.contains_key(&c.outpoint) {
                    Some(coin_to_candidate(
                        &c, /*must_select=*/ false, /*sequence=*/ None,
                        /*ancestor_info=*/ None,
                    ))
                } else {
                    None
                }
            })
            .collect();
        if !is_cancel {
            candidate_coins.extend(&confirmed_cands);
        }
        // The replaced fee is the fee of the transaction being replaced and its descendants. Coin selection
        // will ensure that the replacement transaction additionally pays for its own weight as per
        // RBF rule 4.
        let replaced_fee = descendant_fees.to_sat();
        let locktime = self.anti_fee_sniping_locktime();
        // Coin selection can add one retry for cancel. Change allocation conflicts
        // have their own finite budget so a reservation stream cannot starve this RPC.
        let mut change_commit_conflicts = 0;
        loop {
            match match &self.config.main_descriptor {
                WalletDescriptor::Vault(vault) => create_spend(
                    vault,
                    &self.secp,
                    &mut tx_getter,
                    &destinations,
                    &candidate_coins,
                    SpendTxFees::Rbf(feerate_vb, replaced_fee),
                    change_address.clone(),
                    locktime,
                ),
                WalletDescriptor::Imported(imported) => imported_spend::create_imported_spend(
                    imported,
                    &self.secp,
                    &mut tx_getter,
                    &destinations,
                    &candidate_coins,
                    SpendTxFees::Rbf(feerate_vb, replaced_fee),
                    change_address.clone(),
                    locktime,
                ),
            } {
                Ok(CreateSpendRes {
                    psbt,
                    has_change,
                    warnings,
                }) => {
                    let change_info = change_address.info;
                    if has_change && fresh_change && !self.commit_change_if_next(&change_info)? {
                        change_commit_conflicts += 1;
                        if change_commit_conflicts >= CHANGE_COMMIT_CONFLICT_LIMIT {
                            return Err(CommandError::ChangeReservationContended);
                        }
                        change_address = self.next_change_addr(&mut db_conn)?;
                        continue;
                    }
                    // In case of success, make sure to update our next derivation index if any address
                    // used in the transaction outputs was from the future.
                    for (addr, _) in destinations {
                        self.maybe_increase_last_deriv_index(&mut db_conn, &addr.info);
                    }
                    if has_change {
                        self.maybe_increase_last_deriv_index(&mut db_conn, &change_info);
                    }
                    if self.config.main_descriptor.as_imported().is_some() {
                        db_conn
                            .store_imported_construction(&psbt)
                            .map_err(CommandError::ImportedSpendValidation)?;
                    }

                    return Ok(CreateSpendResult::Success {
                        psbt,
                        warnings: warnings.iter().map(|w| w.to_string()).collect(),
                    });
                }
                Err(SpendCreationError::CoinSelection(e)) => {
                    // If we get a coin selection error due to insufficient funds and we want to cancel the
                    // transaction, then set all previous coins as mandatory and add confirmed coins as
                    // optional, unless we have already done this.
                    if is_cancel && candidate_coins.iter().all(|c| !c.must_select) {
                        for cand in candidate_coins.iter_mut() {
                            cand.must_select = true;
                        }
                        candidate_coins.extend(&confirmed_cands);
                        continue;
                    } else {
                        return Ok(CreateSpendResult::InsufficientFunds { missing: e.missing });
                    }
                }
                Err(e) => {
                    return Err(e.into());
                }
            };
        }
    }

    /// Read-only bounds from this daemon's already admitted backend. Callers
    /// must still use start_rescan, which independently revalidates its date.
    pub fn rescan_time_bounds(&self) -> Result<RescanTimeBounds, CommandError> {
        if self.reorg_alert_cache.load() != crate::bitcoin::ChainAlert::None {
            return Err(CommandError::RescanTrigger(
                "Chain alert prevents a safe rescan".into(),
            ));
        }
        let genesis = self
            .bitcoin
            .genesis_block_timestamp()
            .map_err(|error| CommandError::RescanGenesis(error.to_string()))?;
        let tip = self.bitcoin.tip_time().ok_or_else(|| {
            CommandError::RescanTrigger(
                "Cannot determine current tip timestamp; retry when the backend is available"
                    .into(),
            )
        })?;
        if genesis == 0 || tip <= genesis {
            return Err(CommandError::RescanTrigger(
                "Invalid admitted backend rescan bounds".into(),
            ));
        }
        Ok(RescanTimeBounds {
            chain: self.config.bitcoin_config.chain,
            genesis_timestamp: genesis,
            tip_timestamp: tip,
        })
    }

    /// Trigger a rescan of the block chain for transactions involving our main descriptor between
    /// the given date and the current tip.
    /// The date must be after the genesis block time and before the current tip blocktime.
    pub fn start_rescan(&mut self, timestamp: u32) -> Result<(), CommandError> {
        let mut db_conn = self.db.connection();
        let genesis_timestamp = self
            .bitcoin
            .genesis_block_timestamp()
            .map_err(|error| CommandError::RescanGenesis(error.to_string()))?;

        let tip_timestamp = self.bitcoin.tip_time().ok_or_else(|| {
            CommandError::RescanTrigger(
                "Cannot determine current tip timestamp; retry when the backend is available"
                    .into(),
            )
        })?;
        if timestamp < genesis_timestamp || timestamp >= tip_timestamp {
            return Err(CommandError::InsaneRescanTimestamp(timestamp));
        }
        let backend_rescanning = self
            .bitcoin
            .rescan_progress_result()
            .map_err(|e| {
                CommandError::RescanTrigger(format!(
                    "Cannot determine whether the backend is rescanning; retry when it is available: {e}"
                ))
            })?
            .is_some();
        if db_conn.rescan_timestamp().is_some() || backend_rescanning {
            return Err(CommandError::AlreadyRescanning);
        }

        // TODO: there is a race with the above check for whether the backend is already
        // rescanning. This could make us crash with the bitcoind backend if someone triggered a
        // rescan of the wallet just after we checked above and did now.
        self.bitcoin
            .start_rescan(
                self.config
                    .main_descriptor
                    .as_vault()
                    .ok_or(CommandError::RecoveryNotAvailable)?,
                timestamp,
            )
            .map_err(CommandError::RescanTrigger)?;
        db_conn.set_rescan(timestamp);

        Ok(())
    }

    /// Replay saved Core wallet transactions into our DB without a block rescan.
    /// Startup requests this too, so interruption never loses the obligation.
    pub fn replay_wallet_records(&mut self) -> Result<(), CommandError> {
        let ticket = self
            .bitcoin
            .request_wallet_record_replay()
            .map_err(CommandError::RescanTrigger)?;
        let (tx, rx) = mpsc::sync_channel(0);
        self.poller_sender
            .try_send(PollerMessage::PollNow(tx))
            .map_err(|e| CommandError::RescanTrigger(e.to_string()))?;
        rx.recv_timeout(std::time::Duration::from_secs(120))
            .map_err(|e| CommandError::RescanTrigger(e.to_string()))?;
        if self
            .bitcoin
            .wallet_record_replay_pending()
            .is_some_and(|pending| pending >= ticket)
        {
            return Err(CommandError::RescanTrigger(
                "The wallet record replay has not completed; retry".into(),
            ));
        }
        Ok(())
    }

    /// list_confirmed_transactions retrieves a limited list of transactions which occurred between two given dates.
    pub fn list_confirmed_transactions(
        &self,
        start: u32,
        end: u32,
        limit: u64,
    ) -> ListTransactionsResult {
        let mut db_conn = self.db.connection();
        // Note the result could in principle be retrieved in a single database query.
        let txids = db_conn.list_txids(start, end, limit);
        self.list_transactions(&txids)
    }

    /// list_transactions retrieves the transactions with the given txids.
    pub fn list_transactions(&self, txids: &[bitcoin::Txid]) -> ListTransactionsResult {
        let transactions = self
            .db
            .connection()
            .list_wallet_transactions(txids)
            .into_iter()
            .map(|(tx, height, time)| TransactionInfo { tx, height, time })
            .collect();
        ListTransactionsResult { transactions }
    }

    /// Create a transaction that sweeps coins using a timelocked recovery path to a
    /// provided address with the provided feerate.
    ///
    /// The `timelock` parameter can be used to specify which recovery path to use. By default,
    /// we'll use the first recovery path available.
    ///
    /// If `coins_outpoints` is empty, all coins for which the given recovery path is currently
    /// available will be used. Otherwise, only those specified will be considered. An error will
    /// be returned if any coins specified by `coins_outpoints` are unknown, already spent or
    /// otherwise not currently recoverable using the given recovery path.
    ///
    /// Note that not all coins may be spendable through a single recovery path at the same time.
    pub fn create_recovery(
        &self,
        address: bitcoin::Address<address::NetworkUnchecked>,
        coins_outpoints: &[bitcoin::OutPoint],
        feerate_vb: u64,
        timelock: Option<u16>,
    ) -> Result<CreateRecoveryResult, CommandError> {
        if feerate_vb < 1 {
            return Err(CommandError::InvalidFeerate(feerate_vb));
        }
        let mut tx_getter = DbTxGetter::new(&self.db);
        let mut db_conn = self.db.connection();
        let sweep_addr = self.spend_addr(&mut db_conn, self.validate_address(address)?);

        // Query the coins that we can spend through the specified recovery path (if no recovery
        // path specified, use the first available one) from the database.
        let current_height = self
            .bitcoin
            .chain_tip_result()
            .map_err(CommandError::ChainTipUnavailable)?
            .height;
        let vault = self
            .config
            .main_descriptor
            .as_vault()
            .ok_or(CommandError::RecoveryNotAvailable)?;
        let timelock = timelock.unwrap_or_else(|| vault.first_timelock_value());
        let height_delta: i32 = timelock.into();
        let coins = if coins_outpoints.is_empty() {
            db_conn.coins(&[CoinStatus::Confirmed], &[])
        } else {
            // We could have used the same DB call for both cases by specifying the status and outpoints,
            // but in order to give more helpful errors, we filter the DB call here only for outpoints
            // and then check for coin status separately.
            let coins_by_op = db_conn.coins(&[], coins_outpoints);
            for op in coins_outpoints {
                let coin = coins_by_op
                    .get(op)
                    .ok_or(CommandError::UnknownOutpoint(*op))?;
                // We only check for spent coins here. Unconfirmed coins (including immature)
                // will fail the check for recoverability further below.
                if coin.is_spent() {
                    return Err(CommandError::AlreadySpent(*op));
                }
            }
            coins_by_op
        };
        let mut sweepable_coins = Vec::with_capacity(coins.len());
        for (op, c) in coins {
            // We are interested in coins available at the *next* block
            if c.block_info
                .map(|b| current_height + 1 >= b.height + height_delta)
                .unwrap_or(false)
            {
                sweepable_coins.push(coin_to_candidate(
                    &c,
                    /*must_select=*/ true,
                    /*sequence=*/ Some(bitcoin::Sequence::from_height(timelock)),
                    /*ancestor_info=*/ None,
                ));
            } else if !coins_outpoints.is_empty() {
                return Err(CommandError::OutpointNotRecoverable(op, timelock));
            }
        }
        if sweepable_coins.is_empty() {
            return Err(CommandError::RecoveryNotAvailable);
        }

        let sweep_addr_info = sweep_addr.info;
        let locktime = self.anti_fee_sniping_locktime();
        let CreateSpendRes {
            psbt, has_change, ..
        } = create_spend(
            vault,
            &self.secp,
            &mut tx_getter,
            &[], // No destination, only the change address.
            &sweepable_coins,
            SpendTxFees::Regular(feerate_vb),
            sweep_addr,
            locktime,
        )?;
        if has_change {
            self.maybe_increase_last_deriv_index(&mut db_conn, &sweep_addr_info);
        }

        Ok(CreateRecoveryResult { psbt })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoDescriptors {
    pub main: coincube_core::wallet_descriptor::WalletDescriptor,
}

/// Durable discovery status for a descriptor-imported Bitcoin wallet. Scanned
/// counts are exclusive receive/change derivation bounds whose history was
/// successfully checked; requested bounds are never reported as coverage.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportedWalletSync {
    /// Full, chain-bound SHA256 wallet identity encoded as 64 hex digits.
    pub wallet_identity: String,
    pub chain_id: coincube_core::chain::ChainId,
    pub birthday: u32,
    pub initial_lookahead: u32,
    /// Durable wider historical scan request. Readiness requires both branch
    /// coverage counts to reach this bound, separately from allocation state.
    #[serde(default)]
    pub discovery_target: u32,
    pub receive_scanned: u32,
    pub change_scanned: u32,
    pub history_complete: bool,
}

/// Information about the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetInfoResult {
    pub version: String,
    pub network: bitcoin::Network,
    /// Explicit chain identity. Bitcoin and BTCB2 share address encoding, so
    /// `network` alone cannot safely identify the daemon's chain. Older
    /// daemons omit this field and must not be treated as Bitcoin by default.
    #[serde(default)]
    pub chain_id: Option<coincube_core::chain::ChainId>,
    pub block_height: i32,
    pub sync: f64,
    pub descriptors: GetInfoDescriptors,
    /// Absent for every historical Vault response and old daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_wallet: Option<ImportedWalletSync>,
    /// The progress as a percentage (between 0 and 1) of an ongoing rescan if there is any
    pub rescan_progress: Option<f64>,
    /// Depth, in blocks, of a block chain reorganization the poller refused to apply because
    /// it exceeded `MAX_REORG_DEPTH`. `Some` means our view of the chain is deliberately not
    /// being updated from the backend, because a rollback that deep is far likelier a
    /// misreporting backend than real history being undone — see
    /// [`crate::bitcoin::ChainAlert::RefusedReorg`]. This reports *only* a refused deep reorg;
    /// an unresolved chain divergence is reported separately by `chain_divergence` and must
    /// not be conflated with it. Absent from older daemons, hence `serde(default)`.
    #[serde(default)]
    pub refused_reorg_depth: Option<i32>,
    /// Whether the poller has paused synchronization because our tip is off the backend's chain
    /// and that backend (Electrum/Esplora) cannot be asked where the two chains parted. This is
    /// a *distinct* condition from `refused_reorg_depth`: there is no known fork point and hence
    /// no rollback depth to report, so unlike a refused deep reorg it carries no number — the
    /// tips' height gap is not one. `true` means wallet state is being held intact until the
    /// chains reconverge; see [`crate::bitcoin::ChainAlert::Diverged`]. Defaults to `false` for
    /// older daemons that don't report it.
    #[serde(default)]
    pub chain_divergence: bool,
    /// Timestamp at wallet creation date
    pub timestamp: u32,
    /// Timestamp of last poll, if any.
    pub last_poll_timestamp: Option<u32>,
    /// How far the wallet's view of its own history has got: whether a full scan is
    /// running and how far along, whether one has completed, and whether polls are
    /// failing. Absent from older daemons, which report none of it.
    #[serde(default)]
    pub history_sync: crate::bitcoin::HistorySync,
    /// Last index used to generate a receive address
    pub receive_index: u32,
    /// Last index used to generate a change address
    pub change_index: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateDerivIndexesResult {
    pub receive: u32,
    pub change: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetAddressResult {
    #[serde(deserialize_with = "deser_addr_assume_checked")]
    pub address: bitcoin::Address,
    pub derivation_index: bip32::ChildNumber,
}

impl GetAddressResult {
    pub fn new(address: bitcoin::Address, derivation_index: bip32::ChildNumber) -> Self {
        Self {
            address,
            derivation_index,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetLabelsResult {
    pub labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetLabelsBip329Result {
    pub labels: crate::bip329::Labels,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AddressInfo {
    index: u32,
    receive: bitcoin::Address,
    change: bitcoin::Address,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ListAddressesResult {
    addresses: Vec<AddressInfo>,
}

impl ListAddressesResult {
    pub fn new(addresses: Vec<AddressInfo>) -> Self {
        ListAddressesResult { addresses }
    }
}

/// A revealed address entry in the list returned by [`DaemonControl::list_revealed_addresses`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListRevealedAddressesEntry {
    /// The address's derivation index.
    pub index: ChildNumber,
    /// The address.
    #[serde(deserialize_with = "deser_addr_assume_checked")]
    pub address: bitcoin::Address,
    /// Label assigned to the address, if any.
    pub label: Option<String>,
    /// How many coins, including those unconfirmed, that are currently in the wallet are using this address.
    ///
    /// This count does not include any coins that may have been replaced or otherwise dropped
    /// from the mempool.
    pub used_count: u32,
}

/// Result of a [`DaemonControl::list_revealed_addresses`] request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListRevealedAddressesResult {
    /// Revealed addresses in order of descending derivation index.
    pub addresses: Vec<ListRevealedAddressesEntry>,
    /// `continue_from` being set to some value indicates that there may
    /// be more addresses that can be listed with pagination. The next
    /// [`DaemonControl::list_revealed_addresses`] request can be continued
    /// with this value passed to `start_index`.
    ///
    /// If `continue_from` is `None`, then there are no further
    /// addresses to be listed.
    pub continue_from: Option<ChildNumber>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LCSpendInfo {
    pub txid: bitcoin::Txid,
    /// The block height this spending transaction was confirmed at.
    pub height: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsEntry {
    #[serde(
        serialize_with = "ser_amount",
        deserialize_with = "deser_amount_from_sats"
    )]
    pub amount: bitcoin::Amount,
    pub outpoint: bitcoin::OutPoint,
    #[serde(
        serialize_with = "ser_to_string",
        deserialize_with = "deser_addr_assume_checked"
    )]
    pub address: bitcoin::Address,
    pub block_height: Option<i32>,
    /// Derivation index used to create the coin deposit address.
    pub derivation_index: bip32::ChildNumber,
    /// Information about the transaction spending this coin.
    pub spend_info: Option<LCSpendInfo>,
    /// Whether this coin was created by a coinbase transaction that is still immature.
    pub is_immature: bool,
    /// Whether the coin deposit address was derived from the change descriptor.
    pub is_change: bool,
    /// Whether the coin is the output of a transaction whose inputs are all from
    /// this same wallet. If the coin is unconfirmed, it also means that all its
    /// unconfirmed ancestors, if any, are also from self.
    pub is_from_self: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListCoinsResult {
    pub coins: Vec<ListCoinsEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum CreateSpendResult {
    Success {
        #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
        psbt: Psbt,
        warnings: Vec<String>,
    },
    InsufficientFunds {
        missing: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpendEntry {
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    pub psbt: Psbt,
    pub updated_at: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broadcast_txid: Option<bitcoin::Txid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpendResult {
    pub spend_txs: Vec<ListSpendEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListTransactionsResult {
    pub transactions: Vec<TransactionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionInfo {
    #[serde(serialize_with = "ser_hex", deserialize_with = "deser_hex")]
    pub tx: bitcoin::Transaction,
    pub height: Option<i32>,
    pub time: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateRecoveryResult {
    #[serde(serialize_with = "ser_to_string", deserialize_with = "deser_fromstr")]
    pub psbt: Psbt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bitcoin::Block, database::BlockInfo, testutils::*};
    use coincube_core::spend::InsaneFeeInfo;

    use bitcoin::{
        bip32::{self, ChildNumber},
        blockdata::transaction::{TxIn, TxOut, Version as TxVersion},
        locktime::absolute,
        Amount, OutPoint, ScriptBuf, Sequence, Transaction, Txid, Witness,
    };
    use spend::{InsufficientFunds, DUST_OUTPUT_SATS};
    use std::{collections::BTreeMap, str::FromStr};

    const DUST: u64 = DUST_OUTPUT_SATS;

    #[test]
    fn capped_spend_fee_rate_includes_segwit_framing() {
        let daemon = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = daemon.control();
        let vault = control.config.main_descriptor.as_vault().unwrap();
        // Pick an input count where the 2 WU marker/flag crosses a vbyte
        // boundary. The previous witness-empty calculation then overstates
        // a fee of exactly one sat/vB as two sats/vB.
        let mut boundary = None;
        for count in 1..=4u32 {
            let tx = Transaction {
                version: TxVersion::TWO,
                lock_time: absolute::LockTime::ZERO,
                input: (0..count)
                    .map(|vout| TxIn {
                        previous_output: OutPoint {
                            vout,
                            ..OutPoint::null()
                        },
                        script_sig: ScriptBuf::new(),
                        sequence: Sequence::MAX,
                        witness: Witness::new(),
                    })
                    .collect(),
                output: vec![TxOut {
                    value: Amount::from_sat(1),
                    script_pubkey: ScriptBuf::new(),
                }],
            };
            let vsize = vault.unsigned_tx_max_vbytes(&tx, true);
            let without_framing = (tx.weight().to_wu()
                + vault.max_sat_weight(true) as u64 * count as u64)
                .div_ceil(4);
            if vsize > without_framing {
                boundary = Some((tx, vsize, without_framing));
                break;
            }
        }
        let (mut tx, vsize, without_framing) = boundary.expect("SegWit framing boundary");
        tx.output[0].value = Amount::from_sat(20_000 * tx.input.len() as u64 - vsize);
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        for input in &mut psbt.inputs {
            input.witness_utxo = Some(TxOut {
                value: Amount::from_sat(20_000),
                script_pubkey: ScriptBuf::new(),
            });
        }
        let (fee, measured_vsize) = control.fee_and_vsize(&psbt).unwrap();
        assert_eq!((fee, measured_vsize), (vsize, vsize));
        assert_eq!(fee.div_ceil(measured_vsize), 1);
        assert_eq!(fee.div_ceil(without_framing), 2);
        daemon.shutdown();
    }

    #[test]
    fn capped_spend_shorter_final_witness_keeps_reviewed_fee_valid() {
        let daemon = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = daemon.control();
        let vault = control.config.main_descriptor.as_vault().unwrap();
        let mut tx = Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: (0..2)
                .map(|vout| TxIn {
                    previous_output: OutPoint {
                        vout,
                        ..OutPoint::null()
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::new(),
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(1),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let prepared_vsize = vault.unsigned_tx_max_vbytes(&tx, true);
        let fee = prepared_vsize * 2;
        tx.output[0].value = Amount::from_sat(40_000 - fee);
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        for input in &mut psbt.inputs {
            input.witness_utxo = Some(TxOut {
                value: Amount::from_sat(20_000),
                script_pubkey: ScriptBuf::new(),
            });
        }
        let (measured_fee, measured_vsize) = control.fee_and_vsize(&psbt).unwrap();
        assert_eq!((measured_fee, measured_vsize), (fee, prepared_vsize));

        // Sizing-only witness: a shorter signature changes network vsize but
        // does not change the fee that was prepared and reviewed.
        let mut finalized = psbt.unsigned_tx.clone();
        for input in &mut finalized.input {
            input.witness = Witness::from_slice(&[vec![0; 69], vec![0; 69]]);
        }
        assert!(
            finalized.vsize() < prepared_vsize as usize,
            "final={} prepared={}",
            finalized.vsize(),
            prepared_vsize
        );
        assert!(spend::signed_fee_exceeds_cap(
            fee,
            finalized.vsize() as u64,
            1_000,
            2
        ));
        assert!(control
            .verify_capped_broadcast_fee(&psbt, &finalized, (1_000, 2))
            .is_ok());
        assert!(matches!(
            control.verify_capped_broadcast_fee(&psbt, &finalized, (fee - 1, 2)),
            Err(CommandError::FeeCeilingExceeded)
        ));
        finalized.output[0].value = Amount::from_sat(finalized.output[0].value.to_sat() - 1);
        assert!(matches!(
            control.verify_capped_broadcast_fee(&psbt, &finalized, (1_000, 2)),
            Err(CommandError::FeeRateExceeded)
        ));
        daemon.shutdown();
    }

    struct ConflictDatabase {
        inner: DummyDatabase,
        conflicts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ConflictDatabase {
        fn new(conflicts: usize) -> (Self, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
            let conflicts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(conflicts));
            (
                Self {
                    inner: DummyDatabase::new(),
                    conflicts: conflicts.clone(),
                },
                conflicts,
            )
        }
    }

    impl DatabaseInterface for ConflictDatabase {
        fn reserve_change(
            &self,
            chain: coincube_core::chain::ChainId,
            descriptor: &coincube_core::descriptors::CoincubeDescriptor,
            secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::VerifyOnly>,
        ) -> Result<crate::database::ChangeReservation, ReservationError> {
            self.inner.reserve_change(chain, descriptor, secp)
        }

        fn commit_change_if_next(
            &self,
            chain: coincube_core::chain::ChainId,
            descriptor: &coincube_core::descriptors::CoincubeDescriptor,
            secp: &bitcoin::secp256k1::Secp256k1<bitcoin::secp256k1::VerifyOnly>,
            index: bip32::ChildNumber,
        ) -> Result<bool, ReservationError> {
            let forced = self
                .conflicts
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok();
            if forced {
                let claim = self.inner.reserve_change(chain, descriptor, secp)?;
                assert_eq!(claim.index(), index);
                return Ok(false);
            }
            self.inner
                .commit_change_if_next(chain, descriptor, secp, index)
        }

        fn connection(&self) -> Box<dyn DatabaseConnection> {
            self.inner.connection()
        }
    }

    #[test]
    fn rescan_bounds_are_actual_backend_metadata_and_zero_is_never_admitted() {
        for chain in [
            coincube_core::chain::ChainId::Testnet4,
            coincube_core::chain::ChainId::BitcoinBlake2bTestnet4,
        ] {
            let backend = std::sync::Arc::new(std::sync::Mutex::new(DummyBitcoind::new()));
            let interface: std::sync::Arc<std::sync::Mutex<dyn crate::bitcoin::BitcoinInterface>> =
                backend.clone();
            let daemon = DummyCoincube::new(interface, DummyDatabase::new());
            let mut control = daemon.control().clone();
            daemon.shutdown();
            control.config.bitcoin_config =
                crate::config::BitcoinConfig::new(chain, std::time::Duration::from_secs(1));
            let synthetic_genesis = if chain == coincube_core::chain::ChainId::Testnet4 {
                1_600_000_001
            } else {
                1_650_000_002
            };
            backend.lock().unwrap().genesis_timestamp_override = Some(synthetic_genesis);
            backend.lock().unwrap().tip_timestamp = Some(synthetic_genesis + 1000);
            let bounds = control.rescan_time_bounds().unwrap();
            assert_eq!(bounds.chain, chain);
            let actual_genesis = bounds.genesis_timestamp;
            assert_eq!(actual_genesis, synthetic_genesis);
            assert!(bounds.tip_timestamp > actual_genesis);
            assert!(matches!(
                control.start_rescan(0),
                Err(CommandError::InsaneRescanTimestamp(0))
            ));
            assert!(backend.lock().unwrap().rescan_requests.is_empty());
            control.start_rescan(actual_genesis).unwrap();
            assert_eq!(
                backend.lock().unwrap().rescan_requests,
                vec![actual_genesis]
            );
        }
    }

    #[test]
    fn rescan_genesis_failure_is_retryable_without_recording_success() {
        let backend = std::sync::Arc::new(std::sync::Mutex::new(DummyBitcoind::new()));
        let interface: std::sync::Arc<std::sync::Mutex<dyn crate::bitcoin::BitcoinInterface>> =
            backend.clone();
        let daemon = DummyCoincube::new(interface, DummyDatabase::new());
        let mut control = daemon.control().clone();
        daemon.shutdown();
        backend.lock().unwrap().genesis_error = Some(crate::connect::AdmissionError::Unavailable);
        assert!(matches!(
            control.start_rescan(1231006506),
            Err(CommandError::RescanGenesis(_))
        ));
        assert!(control.db.connection().rescan_timestamp().is_none());
        assert!(backend.lock().unwrap().rescan_requests.is_empty());
        backend.lock().unwrap().genesis_error = None;
        backend.lock().unwrap().tip_timestamp = Some(1231006600);
        assert!(matches!(
            control.start_rescan(1231006504),
            Err(CommandError::InsaneRescanTimestamp(_))
        ));
        control.start_rescan(1231006506).unwrap();
        assert_eq!(control.db.connection().rescan_timestamp(), Some(1231006506));
        assert_eq!(backend.lock().unwrap().rescan_requests, vec![1231006506]);
    }

    #[test]
    fn rescan_missing_tip_is_retryable_and_never_records_or_triggers_a_scan() {
        let backend = std::sync::Arc::new(std::sync::Mutex::new(DummyBitcoind::new()));
        let interface: std::sync::Arc<std::sync::Mutex<dyn crate::bitcoin::BitcoinInterface>> =
            backend.clone();
        let daemon = DummyCoincube::new(interface, DummyDatabase::new());
        let mut control = daemon.control().clone();
        daemon.shutdown();
        for timestamp in [1231006506, 1231006600, 1231006601] {
            assert!(matches!(
                control.start_rescan(timestamp),
                Err(CommandError::RescanTrigger(_))
            ));
            assert!(control.db.connection().rescan_timestamp().is_none());
            assert!(backend.lock().unwrap().rescan_requests.is_empty());
        }
        backend.lock().unwrap().tip_timestamp = Some(1231006600);
        for timestamp in [1231006600, 1231006601] {
            assert!(matches!(
                control.start_rescan(timestamp),
                Err(CommandError::InsaneRescanTimestamp(_))
            ));
            assert!(control.db.connection().rescan_timestamp().is_none());
            assert!(backend.lock().unwrap().rescan_requests.is_empty());
        }
        control.start_rescan(1231006506).unwrap();
        assert_eq!(control.db.connection().rescan_timestamp(), Some(1231006506));
        assert_eq!(backend.lock().unwrap().rescan_requests, vec![1231006506]);
    }

    #[test]
    fn getinfo() {
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        // We can query getinfo
        ms.control().get_info();
        ms.shutdown();
    }

    #[test]
    fn getnewaddress() {
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());

        let control = &ms.control();
        // We can get an address (it will have index 1)
        let addr = control.get_new_address().address;
        // $ bitcoin-cli deriveaddresses "wsh(or_d(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/0/*),and_v(v:pkh([aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/0/*),older(10000))))#wx6v3mks" 1
        // [
        // "bc1q9ksrc647hx8zp2cewl8p5f487dgux3777yees8rjcx46t4daqzzqt7yga8",
        // "bc1qm06ceyghltr8v5cmeckh6cquhy9nks626pahapc04xd98kwf478qwmhqew"
        // ]
        assert_eq!(
            addr,
            bitcoin::Address::from_str(
                "bc1qm06ceyghltr8v5cmeckh6cquhy9nks626pahapc04xd98kwf478qwmhqew"
            )
            .unwrap()
            .assume_checked()
        );
        // We won't get the same twice.
        let addr2 = control.get_new_address().address;
        assert_ne!(addr, addr2);

        ms.shutdown();
    }

    #[test]
    fn listaddresses() {
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());

        let control = &ms.control();

        let list = control.list_addresses(Some(2), Some(5)).unwrap();

        assert_eq!(list.addresses[0].index, 2);
        assert_eq!(list.addresses.last().unwrap().index, 6);

        let addr0 = control.get_new_address().address;
        let _addr1 = control.get_new_address().address;
        let addr2 = control.get_new_address().address;
        let _addr3 = control.get_new_address().address;
        let addr4 = control.get_new_address().address;

        let list = control.list_addresses(Some(0), None).unwrap();

        assert_eq!(list.addresses[0].index, 0);
        assert_eq!(list.addresses[1].receive, addr0); // first address has index 1
        assert_eq!(list.addresses.last().unwrap().index, 5);
        assert_eq!(list.addresses.last().unwrap().receive, addr4);

        let list = control.list_addresses(None, None).unwrap();

        assert_eq!(list.addresses[0].index, 0);
        assert_eq!(list.addresses[1].index, 1);
        assert_eq!(list.addresses[1].receive, addr0);
        assert_eq!(list.addresses.last().unwrap().index, 5);
        assert_eq!(list.addresses.last().unwrap().receive, addr4);

        let list = control.list_addresses(Some(1), Some(3)).unwrap();

        assert_eq!(list.addresses[0].index, 1);
        assert_eq!(list.addresses[0].receive, addr0);
        assert_eq!(list.addresses.last().unwrap().index, 3);
        assert_eq!(list.addresses.last().unwrap().receive, addr2);

        let list = control.list_addresses(Some(5), None).unwrap();

        assert_eq!(list.addresses.len(), 1);
        assert_eq!(list.addresses[0].index, 5);
        assert_eq!(list.addresses[0].receive, addr4);

        // We can get no address for the last unhardened index.
        let max_unhardened_index = 2u32.pow(31) - 1;
        let res = control
            .list_addresses(Some(max_unhardened_index), Some(0))
            .unwrap();
        // This is equivalent to not passing a count.
        assert_eq!(
            res,
            control
                .list_addresses(Some(max_unhardened_index), None)
                .unwrap()
        );
        // We can also get the one last unhardened index.
        control
            .list_addresses(Some(max_unhardened_index), Some(1))
            .unwrap();
        // However we can't get into hardened territory.
        assert_eq!(
            control
                .list_addresses(Some(max_unhardened_index), Some(2))
                .unwrap_err(),
            CommandError::InvalidDerivationIndex
        );

        // We also can't pass a hardened start index.
        let first_hardened_index = max_unhardened_index + 1;
        assert_eq!(
            control
                .list_addresses(Some(first_hardened_index), None)
                .unwrap_err(),
            CommandError::InvalidDerivationIndex
        );
        assert_eq!(
            control
                .list_addresses(Some(first_hardened_index), Some(0))
                .unwrap_err(),
            CommandError::InvalidDerivationIndex
        );
        assert_eq!(
            control
                .list_addresses(Some(first_hardened_index), Some(1))
                .unwrap_err(),
            CommandError::InvalidDerivationIndex
        );

        // Much less so overflow.
        assert_eq!(
            control.list_addresses(Some(u32::MAX), None).unwrap_err(),
            CommandError::InvalidDerivationIndex
        );
        assert_eq!(
            control.list_addresses(Some(u32::MAX), Some(0)).unwrap_err(),
            CommandError::InvalidDerivationIndex
        );
        assert_eq!(
            control.list_addresses(Some(u32::MAX), Some(1)).unwrap_err(),
            CommandError::InvalidDerivationIndex
        );

        // We won't crash if we pass a start index larger than the next derivation index without
        // passing a count. (ie no underflow.)
        let next_deriv_index = list.addresses.last().unwrap().index + 1;
        control
            .list_addresses(Some(next_deriv_index + 1), None)
            .unwrap();

        ms.shutdown();
    }

    #[test]
    fn list_revealed_addresses() {
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());

        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();

        // $ bitcoin-cli deriveaddresses "wsh(or_d(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/0/*),and_v(v:pkh([aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/0/*),older(10000))))#wx6v3mks" 0
        // [
        //   "bc1q9ksrc647hx8zp2cewl8p5f487dgux3777yees8rjcx46t4daqzzqt7yga8"
        // ]
        let addr0 = bitcoin::Address::from_str(
            "bc1q9ksrc647hx8zp2cewl8p5f487dgux3777yees8rjcx46t4daqzzqt7yga8",
        )
        .unwrap()
        .assume_checked();

        // The wallet starts with index 0 already revealed:
        let list = control
            .list_revealed_addresses(false, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 1);
        assert!(list.continue_from.is_none());
        let revealed = list.addresses.first().unwrap();
        assert_eq!(revealed.index, ChildNumber::from(0));
        assert_eq!(revealed.address, addr0);
        assert_eq!(revealed.used_count, 0);
        assert!(revealed.label.is_none());

        // Generate new addresses up to and including index 7:
        let addr1 = control.get_new_address().address;
        let addr2 = control.get_new_address().address;
        let addr3 = control.get_new_address().address;
        let addr4 = control.get_new_address().address;
        let addr5 = control.get_new_address().address;
        let addr6 = control.get_new_address().address;
        let addr7 = control.get_new_address().address;
        assert_eq!(control.get_info().receive_index, 7);

        // Set some labels.
        db_conn.update_labels(&HashMap::from([
            (
                LabelItem::Address(addr1.clone()),
                Some("my test label 1".to_string()),
            ),
            (
                LabelItem::Address(addr5.clone()),
                Some("my test label 5".to_string()),
            ),
        ]));

        // If we continue_from a value above our last index, we'll start from the last index.
        let list = control
            .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(100)))
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7)); // this is our last revealed index
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(5));
        assert_eq!(list.addresses[2].address, addr5);
        assert_eq!(list.addresses[2].used_count, 0);
        assert_eq!(list.addresses[2].label, Some("my test label 5".to_string()));

        // If we start from a hardened index, we'll get the same result again:
        assert_eq!(
            list,
            control
                .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(u32::MAX)))
                .unwrap()
        );

        // Passing `None` for `continue_from` will also start from the last revealed index:
        let list = control
            .list_revealed_addresses(false, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7));
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(5));
        assert_eq!(list.addresses[2].address, addr5);
        assert_eq!(list.addresses[2].used_count, 0);
        assert_eq!(list.addresses[2].label, Some("my test label 5".to_string()));

        // Now continue pagination using the `continue_from` value from the result above:
        let list = control
            .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(4)))
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(1)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(4));
        assert_eq!(list.addresses[0].address, addr4);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(3));
        assert_eq!(list.addresses[1].address, addr3);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(2));
        assert_eq!(list.addresses[2].address, addr2);
        assert_eq!(list.addresses[2].used_count, 0);
        assert!(list.addresses[2].label.is_none());

        // This is the final page:
        let list = control
            .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(1)))
            .unwrap();
        assert_eq!(list.addresses.len(), 2); // only two addresses even though limit was 3
        assert!(list.continue_from.is_none()); // there are no more addresses to derive

        assert_eq!(list.addresses[0].index, ChildNumber::from(1));
        assert_eq!(list.addresses[0].address, addr1);
        assert_eq!(list.addresses[0].used_count, 0);
        assert_eq!(list.addresses[0].label, Some("my test label 1".to_string()));
        assert_eq!(list.addresses[1].index, ChildNumber::from(0));
        assert_eq!(list.addresses[1].address, addr0);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());

        // Add a coin so that address with index 5 is used:
        db_conn.new_unspent_coins(&[Coin {
            outpoint: OutPoint::new(
                Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                    .unwrap(),
                0,
            ),
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(80_000),
            derivation_index: ChildNumber::from(5),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: true,
        }]);

        // If we don't exclude used, results will be same as before, except index 5 is marked as used:
        let list = control
            .list_revealed_addresses(false, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7));
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(5));
        assert_eq!(list.addresses[2].address, addr5);
        assert_eq!(list.addresses[2].used_count, 1); // used
        assert_eq!(list.addresses[2].label, Some("my test label 5".to_string()));

        // If we exclude used, index 5 will be skipped:
        let list = control
            .list_revealed_addresses(false, true, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(3)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7));
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(4));
        assert_eq!(list.addresses[2].address, addr4);
        assert_eq!(list.addresses[2].used_count, 0);
        assert!(list.addresses[2].label.is_none());

        // Similar behaviour if we continue from index 5. First without excluding used:
        let list = control
            .list_revealed_addresses(false, false, 3, Some(ChildNumber::from(5)))
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(2)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(5));
        assert_eq!(list.addresses[0].address, addr5);
        assert_eq!(list.addresses[0].used_count, 1); // used
        assert_eq!(list.addresses[0].label, Some("my test label 5".to_string()));
        assert_eq!(list.addresses[1].index, ChildNumber::from(4));
        assert_eq!(list.addresses[1].address, addr4);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(3));
        assert_eq!(list.addresses[2].address, addr3);
        assert_eq!(list.addresses[2].used_count, 0);
        assert!(list.addresses[2].label.is_none());

        // Now excluding used:
        let list = control
            .list_revealed_addresses(false, true, 3, Some(ChildNumber::from(5)))
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(1)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(4));
        assert_eq!(list.addresses[0].address, addr4);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(3));
        assert_eq!(list.addresses[1].address, addr3);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(2));
        assert_eq!(list.addresses[2].address, addr2);
        assert_eq!(list.addresses[2].used_count, 0);
        assert!(list.addresses[2].label.is_none());

        // If we add another coin using the same derivation index, the count will increase:
        db_conn.new_unspent_coins(&[Coin {
            outpoint: OutPoint::new(
                Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                    .unwrap(),
                1,
            ),
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(80_000),
            derivation_index: ChildNumber::from(5),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: true,
        }]);

        let list = control
            .list_revealed_addresses(false, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 3);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        assert_eq!(list.addresses[0].index, ChildNumber::from(7));
        assert_eq!(list.addresses[0].address, addr7);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());
        assert_eq!(list.addresses[1].index, ChildNumber::from(6));
        assert_eq!(list.addresses[1].address, addr6);
        assert_eq!(list.addresses[1].used_count, 0);
        assert!(list.addresses[1].label.is_none());
        assert_eq!(list.addresses[2].index, ChildNumber::from(5));
        assert_eq!(list.addresses[2].address, addr5);
        assert_eq!(list.addresses[2].used_count, 2); // count updated.
        assert_eq!(list.addresses[2].label, Some("my test label 5".to_string()));

        // Check change address
        // $ bitcoin-cli deriveaddresses "wsh(or_d(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/1/*),and_v(v:pkh([aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/1/*),older(10000))))#sqk8au7v" 0
        // [
        //   "bc1qd5m23jemr8mfj8x4q482zpspnxehg0jg0pyzrhxfg8xrrtyqewvqjrq3x6"
        // ]
        let change_addr0 = bitcoin::Address::from_str(
            "bc1qd5m23jemr8mfj8x4q482zpspnxehg0jg0pyzrhxfg8xrrtyqewvqjrq3x6",
        )
        .unwrap()
        .assume_checked();

        // If we ask for change addresses, we only have the initial one revealed:
        let list = control
            .list_revealed_addresses(true, false, 3, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 1);
        assert!(list.continue_from.is_none());

        assert_eq!(list.addresses[0].index, ChildNumber::from(0));
        assert_eq!(list.addresses[0].address, change_addr0);
        assert_eq!(list.addresses[0].used_count, 0);
        assert!(list.addresses[0].label.is_none());

        // Finally, passing limit 0 returns an empty vector and the continue_from is same as it started with.
        // First, passing None for continue from:
        let list = control
            .list_revealed_addresses(false, false, 0, None)
            .unwrap();
        assert_eq!(list.addresses.len(), 0);
        assert_eq!(list.continue_from, Some(ChildNumber::from(7)));

        // Now, continuing from 4:
        let list = control
            .list_revealed_addresses(false, false, 0, Some(ChildNumber::from(4)))
            .unwrap();
        assert_eq!(list.addresses.len(), 0);
        assert_eq!(list.continue_from, Some(ChildNumber::from(4)));

        ms.shutdown();
    }

    /// A real funding transaction paying the given coins at the given vouts, so
    /// spend creation can authenticate each selected input against it (P2-A).
    /// Unused vouts are filled with unrelated outputs.
    fn funding_tx(
        control: &DaemonControl,
        coins: &[(
            /* vout */ u32,
            /* amount */ u64,
            /* deriv index */ u32,
            /* is_change */ bool,
        )],
    ) -> bitcoin::Transaction {
        let max_vout = coins.iter().map(|c| c.0).max().unwrap_or(0);
        let mut output = (0..=max_vout)
            .map(|_| bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(546),
                script_pubkey: bitcoin::ScriptBuf::from_bytes(
                    [vec![0x51, 0x20], vec![0xab; 32]].concat(),
                ),
            })
            .collect::<Vec<_>>();
        for (vout, amount, index, is_change) in coins {
            let desc = if *is_change {
                control.config.main_descriptor.change_descriptor()
            } else {
                control.config.main_descriptor.receive_descriptor()
            };
            output[*vout as usize] = bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(*amount),
                script_pubkey: desc.derive(*index).unwrap().script_pubkey(),
            };
        }
        bitcoin::Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::null(),
                script_sig: bitcoin::ScriptBuf::from_bytes(vec![0x51]),
                sequence: bitcoin::Sequence::MAX,
                witness: bitcoin::Witness::default(),
            }],
            output,
        }
    }

    fn funded_conflict_control(
        conflicts: usize,
    ) -> (
        DummyCoincube,
        std::sync::Arc<std::sync::atomic::AtomicUsize>,
        bitcoin::OutPoint,
        bitcoin::Address<address::NetworkUnchecked>,
    ) {
        const COIN_VALUE: u64 = 100_000;
        let (database, conflict_count) = ConflictDatabase::new(conflicts);
        let daemon = DummyCoincube::new(DummyBitcoind::new(), database);
        let control = daemon.control();
        let funding = funding_tx(control, &[(0, COIN_VALUE, 13, false)]);
        let outpoint = bitcoin::OutPoint::new(funding.compute_txid(), 0);
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_txs(std::slice::from_ref(&funding));
        db_conn.new_unspent_coins(&[Coin {
            outpoint,
            is_immature: false,
            block_info: Some(BlockInfo { height: 1, time: 1 }),
            amount: bitcoin::Amount::from_sat(COIN_VALUE),
            derivation_index: 13.into(),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        let destination =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        (daemon, conflict_count, outpoint, destination)
    }

    #[test]
    fn create_spend_rebuilds_on_conflict_and_stops_at_the_budget() {
        let (daemon, conflicts, outpoint, destination) = funded_conflict_control(2);
        let control = daemon.control();
        let result = control
            .create_spend(
                &HashMap::from([(destination, 10_000)]),
                &[outpoint],
                1,
                None,
            )
            .unwrap();
        let psbt = match result {
            CreateSpendResult::Success { psbt, .. } => psbt,
            other => panic!("expected successful spend, got {:?}", other),
        };
        assert_eq!(conflicts.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            control.db().lock().unwrap().connection().change_index(),
            3.into()
        );
        let committed_script = control
            .config
            .main_descriptor
            .change_descriptor()
            .derive(3)
            .unwrap()
            .script_pubkey();
        assert!(psbt
            .unsigned_tx
            .output
            .iter()
            .any(|output| output.script_pubkey == committed_script));
        daemon.shutdown();

        let (daemon, conflicts, outpoint, destination) =
            funded_conflict_control(CHANGE_COMMIT_CONFLICT_LIMIT);
        let control = daemon.control();
        assert_eq!(
            control.create_spend(
                &HashMap::from([(destination, 10_000)]),
                &[outpoint],
                1,
                None,
            ),
            Err(CommandError::ChangeReservationContended)
        );
        assert_eq!(conflicts.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            control.db().lock().unwrap().connection().change_index(),
            bip32::ChildNumber::from(CHANGE_COMMIT_CONFLICT_LIMIT as u32)
        );
        assert_eq!(
            control.reserve_change().unwrap().index(),
            bip32::ChildNumber::from(CHANGE_COMMIT_CONFLICT_LIMIT as u32 + 1)
        );
        daemon.shutdown();
    }

    #[test]
    fn ordinary_failed_spend_does_not_consume_claim_reservation() {
        let daemon = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = daemon.control();
        let claim = control.reserve_change().unwrap();
        assert_eq!(claim.index(), 1.into());
        let mut db_conn = control.db().lock().unwrap().connection();
        let ordinary = control.next_change_addr(&mut db_conn).unwrap();
        assert_eq!(ordinary.info.unwrap().index, 2.into());
        let missing = bitcoin::OutPoint::null();
        assert!(matches!(
            control.create_spend(&HashMap::new(), &[missing], 1, None),
            Err(CommandError::UnknownOutpoint(_))
        ));
        // Neither peeking nor the failed builder consumed index 2. The next Claim
        // durably owns it and an ordinary spend must subsequently peek index 3.
        assert_eq!(control.reserve_change().unwrap().index(), 2.into());
        assert_eq!(
            control
                .next_change_addr(&mut db_conn)
                .unwrap()
                .info
                .unwrap()
                .index,
            3.into()
        );
    }

    /// The contract the GUI's transfer preview relies on: a change address the
    /// wallet already owns never reserves an index, while a fresh one (`None`)
    /// commits exactly one when the result carries change. A preview that only reads `psbt.fee()` can therefore
    /// pass its fixed sizing address and leave the change index alone.
    #[test]
    fn supplied_own_change_address_reserves_nothing_fresh_reserves_once() {
        const COIN_VALUE: u64 = 100_000;
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        let dummy_tx = funding_tx(control, &[(0, COIN_VALUE, 13, false)]);
        let dummy_op = bitcoin::OutPoint::new(dummy_tx.compute_txid(), 0);
        db_conn.new_txs(std::slice::from_ref(&dummy_tx));
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(COIN_VALUE),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        let mut destinations = <HashMap<bitcoin::Address<address::NetworkUnchecked>, u64>>::new();
        let destination =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        destinations.insert(destination.clone(), 10_000);
        let sizing = control
            .config
            .main_descriptor
            .change_descriptor()
            .derive(0)
            .unwrap()
            .address(control.config.bitcoin_config.network)
            .unwrap()
            .as_unchecked()
            .clone();
        let before = db_conn.change_index();
        // Three previews with the wallet's own change address, each producing a
        // change output the caller discards: nothing reserved.
        for _ in 0..3 {
            assert!(matches!(
                control.create_spend(&destinations, &[dummy_op], 1, Some(sizing.clone())),
                Ok(CreateSpendResult::Success { .. })
            ));
            assert_eq!(db_conn.change_index(), before);
        }
        // A fresh address is also only a peek when the successful spend has no
        // change output. The remaining value is fee rather than a second output.
        destinations.insert(destination.clone(), 99_500);
        let changeless = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap();
        match changeless {
            CreateSpendResult::Success { psbt, .. } => {
                assert_eq!(psbt.unsigned_tx.output.len(), 1);
            }
            other => panic!("expected changeless spend, got {:?}", other),
        }
        assert_eq!(db_conn.change_index(), before);

        // The real spend asks for a fresh change address and reserves exactly one.
        destinations.insert(destination, 10_000);
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Ok(CreateSpendResult::Success { .. })
        ));
        assert_eq!(db_conn.change_index(), before.increment().unwrap());
    }

    #[test]
    fn vault_change_must_be_owned_unless_the_spend_is_a_sweep() {
        const COIN_VALUE: u64 = 100_000;
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        let dummy_tx = funding_tx(control, &[(0, COIN_VALUE, 13, false)]);
        let dummy_op = bitcoin::OutPoint::new(dummy_tx.compute_txid(), 0);
        db_conn.new_txs(std::slice::from_ref(&dummy_tx));
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(COIN_VALUE),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        let mut destinations = <HashMap<bitcoin::Address<address::NetworkUnchecked>, u64>>::new();
        destinations.insert(
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap(),
            10_000,
        );
        let external =
            bitcoin::Address::from_str("bc1q39srgatmkp6k2ne3l52yhkjprdvunvspqydmkx").unwrap();
        let refused = control.create_spend(&destinations, &[dummy_op], 1, Some(external.clone()));
        assert!(matches!(
            refused,
            Err(CommandError::ExternalChangeNotAllowed)
        ));

        let swept = control.create_spend(&HashMap::new(), &[dummy_op], 1, Some(external));
        assert!(matches!(swept, Ok(CreateSpendResult::Success { .. })));
    }

    #[test]
    fn capped_vault_spend_accepts_network_dust_recipient() {
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        let funding = funding_tx(control, &[(0, 100_000, 13, false)]);
        let outpoint = bitcoin::OutPoint::new(funding.compute_txid(), 0);
        db_conn.new_txs(std::slice::from_ref(&funding));
        db_conn.new_unspent_coins(&[Coin {
            outpoint,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        let destination =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let network_dust = destination
            .assume_checked_ref()
            .script_pubkey()
            .minimal_non_dust()
            .to_sat();
        assert_eq!(network_dust, 294);
        let mut destinations = HashMap::from([(destination, network_dust - 1)]);
        let before = db_conn.change_index();
        assert!(matches!(
            control.create_spend_within_limits(&destinations, &[outpoint], 1, None, Some(10_000)),
            Err(CommandError::SpendCreation(
                SpendCreationError::InvalidOutputValue(_)
            ))
        ));
        assert_eq!(db_conn.change_index(), before);
        *destinations.values_mut().next().unwrap() = network_dust;
        assert!(matches!(
            control.create_spend(&destinations, &[outpoint], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::InvalidOutputValue(_)
            ))
        ));
        let created = control
            .create_spend_within_limits(&destinations, &[outpoint], 1, None, Some(10_000))
            .unwrap();
        let CreateSpendResult::Success { psbt, .. } = created else {
            panic!("expected successful capped spend");
        };
        assert_eq!(psbt.unsigned_tx.output[0].value.to_sat(), network_dust);

        // At 1 sat/vB this spend pays 161 sats and leaves exactly one
        // network-minimum P2WSH change output. The ordinary economic change
        // rule would add the 330 sats to fees and fail the capped draft.
        *destinations.values_mut().next().unwrap() = 99_509;
        let created = control
            .create_spend_within_limits(&destinations, &[outpoint], 1, None, Some(10_000))
            .unwrap();
        let CreateSpendResult::Success { psbt, .. } = created else {
            panic!("expected successful capped spend with minimum change");
        };
        assert_eq!(psbt.unsigned_tx.output.len(), 2);
        let change = &psbt.unsigned_tx.output[1];
        assert_eq!(change.value.to_sat(), 330);
        assert_eq!(change.value, change.script_pubkey.minimal_non_dust());
    }

    #[test]
    fn create_spend() {
        const COIN_VALUE: u64 = 100_000;
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        // Every coin this test spends is paid by this one real funding transaction:
        // vout 0 (100_000 sats, index 13), vout 10 (400_000, index 42), vout 100
        // (80_000, index 42), vout 110 (20_000, index 43), vout 120 (dust+500, index 56).
        let dummy_tx = funding_tx(
            control,
            &[
                (0, COIN_VALUE, 13, false),
                (10, 400_000, 42, false),
                (100, 80_000, 42, false),
                (110, 20_000, 43, false),
                (120, DUST + 500, 56, false),
            ],
        );
        let dummy_op = bitcoin::OutPoint::new(dummy_tx.compute_txid(), 0);
        db_conn.new_txs(std::slice::from_ref(&dummy_tx));

        // Arguments sanity checking
        let dummy_addr =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let dummy_value = 10_000;
        let mut destinations = <HashMap<bitcoin::Address<address::NetworkUnchecked>, u64>>::new();
        assert_eq!(
            control.create_spend(&destinations, &[], 1, None),
            Err(CommandError::NoOutpointForSelfSend)
        );
        destinations = [(dummy_addr.clone(), dummy_value)]
            .iter()
            .cloned()
            .collect();
        // Insufficient funds for coin selection.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 0, None),
            Err(CommandError::InvalidFeerate(0))
        );

        // The coin doesn't exist. If we create a new unspent one at this outpoint with a much
        // higher value, we'll get a Spend transaction with a change output.
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Err(CommandError::UnknownOutpoint(dummy_op))
        );
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(COIN_VALUE),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        // If we try to use coin selection, the unconfirmed not-from-self coin will not be used
        // as a candidate and so we get a coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        // The input carries the complete, authenticated previous transaction and a
        // witness_utxo equal to its spent output.
        assert_eq!(psbt.inputs[0].non_witness_utxo.as_ref(), Some(&dummy_tx));
        assert_eq!(
            psbt.inputs[0].witness_utxo.as_ref(),
            Some(&dummy_tx.output[0])
        );
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 2);
        // It has change so no warnings expected.
        assert!(warnings.is_empty());
        assert_eq!(
            tx.output[0].script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        assert_eq!(tx.output[0].value.to_sat(), dummy_value);

        // NOTE: if you are wondering about the usefulness of these tests asserting arbitrary fixed
        // values, that's a belt-and-suspenders check to make sure size and fee calculations do not
        // change unexpectedly. For instance this specific test caught how a change in
        // rust-bitcoin's serialization of transactions with no input silently affected our fee
        // calculation.

        // Transaction is 1 in (P2WSH satisfaction), 2 outs. At 1sat/vb, it's 161 sats fees.
        // At 2sats/vb, it's twice that.
        assert_eq!(tx.output[1].value.to_sat(), 89_839);
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations, &[dummy_op], 2, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.output[1].value.to_sat(), 89_678);

        // A feerate of 555 won't trigger the sanity checks (they were previously not taking the
        // satisfaction size into account and overestimating the feerate).
        control
            .create_spend(&destinations, &[dummy_op], 555, None)
            .unwrap();

        // If we ask for a too high feerate, or a too large/too small output, it'll fail.
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 10_000, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        *destinations.get_mut(&dummy_addr).unwrap() = 100_001;
        assert!(matches!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        *destinations.get_mut(&dummy_addr).unwrap() = DUST - 1;
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::InvalidOutputValue(bitcoin::Amount::from_sat(DUST - 1))
            ))
        );

        // If we ask to create an output for an address from another network, it will fail.
        let invalid_addr =
            bitcoin::Address::from_str("tb1qfufcrdyarcg5eph608c6l8vktrc9re6agu4se2").unwrap();
        let invalid_destinations: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(invalid_addr, dummy_value)].iter().cloned().collect();
        assert!(matches!(
            control.create_spend(&invalid_destinations, &[dummy_op], 1, None),
            Err(CommandError::Address(
                address::error::ParseError::NetworkValidation { .. }
            ))
        ));

        // If we ask for a large, but valid, output we won't get a change output. 99_500 because we
        // won't create an output lower than 500 sats.
        *destinations.get_mut(&dummy_addr).unwrap() = COIN_VALUE - DUST;
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, dummy_op);
        assert_eq!(tx.output.len(), 1);
        assert_eq!(
            tx.output[0].script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        assert_eq!(tx.output[0].value.to_sat(), COIN_VALUE - DUST);
        // change = 100_000 - 99_500 - /* fee without change */ 127 - /* extra fee for change output */ 43 = 330
        assert_eq!(
            warnings,
            vec![
                "Dust UTXO. The minimal change output allowed by Coincube is 500 sats. \
                Instead of creating a change of 339 sats, it was added to the \
                transaction fee. Select a larger input to avoid this from happening."
            ]
        );

        // Increase the target value by the change amount and the warning will disappear.
        *destinations.get_mut(&dummy_addr).unwrap() = (COIN_VALUE - DUST) + 339;
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert!(warnings.is_empty());

        // Now increase target also by the extra fee that was paying for change and we can still create the spend.
        *destinations.get_mut(&dummy_addr).unwrap() =
            (COIN_VALUE - DUST) + 330 + /* fee for change output */ 43;
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 1);
        assert!(warnings.is_empty());

        // Now increase the target by 1 more sat and we will have insufficient funds.
        *destinations.get_mut(&dummy_addr).unwrap() =
            (COIN_VALUE - DUST) + 339 + /* fee for change output */ 43 + 1;
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { missing: 1 }),
        );

        // Now decrease the target so that the lost change is just 1 sat.
        *destinations.get_mut(&dummy_addr).unwrap() =
            COIN_VALUE - /* fee without change */ 118 - /* extra fee for change output */ 43 - 1;
        let warnings = if let CreateSpendResult::Success { warnings, .. } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            warnings
        } else {
            panic!("expect successful spend creation")
        };
        // Message uses "sat" instead of "sats" when value is 1.
        assert_eq!(
            warnings,
            vec![
                "Dust UTXO. The minimal change output allowed by Coincube is 500 sats. \
                Instead of creating a change of 1 sat, it was added to the \
                transaction fee. Select a larger input to avoid this from happening."
            ]
        );

        // Now decrease the target value so that we have enough for a change output.
        *destinations.get_mut(&dummy_addr).unwrap() = (COIN_VALUE - DUST) - /* fee without change */ 118 - /* extra fee for change output */ 43;
        let (psbt, warnings) = if let CreateSpendResult::Success { psbt, warnings } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            (psbt, warnings)
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        assert_eq!(tx.output.len(), 2);
        assert_eq!(tx.output[1].value.to_sat(), DUST);
        assert!(warnings.is_empty());

        // Now increase the target by 1 and we'll get a warning again, this time for 1 less than the dust threshold.
        *destinations.get_mut(&dummy_addr).unwrap() = (COIN_VALUE - DUST) - /* fee without change */ 118 - /* extra fee for change output */ 43
            + 1;
        let warnings = if let CreateSpendResult::Success { warnings, .. } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            warnings
        } else {
            panic!("expect successful spend creation")
        };
        assert_eq!(
            warnings,
            vec![
                "Dust UTXO. The minimal change output allowed by Coincube is 500 sats. \
                Instead of creating a change of 499 sats, it was added to the \
                transaction fee. Select a larger input to avoid this from happening."
            ]
        );

        // Now if we mark the coin as spent, we won't create another Spend transaction containing
        // it.
        db_conn.spend_coins(&[(
            dummy_op,
            bitcoin::Txid::from_str(
                "ef78f79ba747813887747cf8582897a48f1a09f1ca04d2cd3d6fcfdcbb5e0797",
            )
            .unwrap(),
        )]);
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op], 1, None),
            Err(CommandError::AlreadySpent(dummy_op))
        );
        // If we try to use coin selection, the spent coin will not be used as a candidate
        // and so we get a coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));

        // We'd bail out if they tried to create a transaction with a too high feerate.
        let dummy_op_dup = bitcoin::OutPoint {
            txid: dummy_op.txid,
            vout: dummy_op.vout + 10,
        };
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op_dup,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(400_000),
            derivation_index: bip32::ChildNumber::from(42),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        assert_eq!(
            control.create_spend(&destinations, &[dummy_op_dup], 1_001, None),
            Err(CommandError::SpendCreation(SpendCreationError::InsaneFees(
                InsaneFeeInfo::TooHighFeerate(1_001)
            )))
        );

        // Add an unconfirmed coin from self to be used for coin selection.
        let confirmed_op_1 = bitcoin::OutPoint {
            txid: dummy_op.txid,
            vout: dummy_op.vout + 100,
        };
        let unconfirmed_coin = Coin {
            outpoint: confirmed_op_1,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(80_000),
            derivation_index: bip32::ChildNumber::from(42),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: true,
        };
        db_conn.new_unspent_coins(&[unconfirmed_coin]);
        // Coin selection error due to insufficient funds.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        // Set destination amount equal to value of confirmed coins.
        *destinations.get_mut(&dummy_addr).unwrap() = 80_000;
        // Coin selection error occurs due to insufficient funds to pay fee.
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        let confirmed_op_2 = bitcoin::OutPoint {
            txid: confirmed_op_1.txid,
            vout: confirmed_op_1.vout + 10,
        };
        // Add new confirmed coin to cover the fee.
        db_conn.new_unspent_coins(&[Coin {
            outpoint: confirmed_op_2,
            is_immature: false,
            block_info: Some(BlockInfo {
                height: 174500,
                time: 174500,
            }),
            amount: bitcoin::Amount::from_sat(20_000),
            derivation_index: bip32::ChildNumber::from(43),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        // First, create a transaction using auto coin selection.
        let psbt = if let CreateSpendResult::Success { psbt, .. } =
            control.create_spend(&destinations, &[], 1, None).unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx_auto = psbt.unsigned_tx;
        let mut tx_prev_outpoints = tx_auto
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<Vec<OutPoint>>();
        tx_prev_outpoints.sort();
        assert_eq!(tx_auto.input.len(), 2);
        assert_eq!(tx_prev_outpoints, vec![confirmed_op_1, confirmed_op_2]);
        // Output includes change.
        assert_eq!(tx_auto.output.len(), 2);
        assert_eq!(
            tx_auto.output[0].script_pubkey,
            dummy_addr.assume_checked().script_pubkey()
        );
        assert_eq!(tx_auto.output[0].value, Amount::from_sat(80_000));

        // Create a second transaction using manual coin selection.
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations, &[confirmed_op_1, confirmed_op_2], 1, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx_manual = psbt.unsigned_tx;
        // Check that manual and auto selection give same outputs (except change address).
        assert_ne!(tx_auto.output, tx_manual.output);
        assert_eq!(tx_auto.output.len(), tx_manual.output.len());
        assert_eq!(tx_auto.output[0], tx_manual.output[0]);
        assert_eq!(tx_auto.output[1].value, tx_manual.output[1].value);
        assert_ne!(
            tx_auto.output[1].script_pubkey,
            tx_manual.output[1].script_pubkey
        );
        // Check inputs are also the same. Need to sort as order is not guaranteed by `create_spend`.
        let mut auto_input = tx_auto.clone().input;
        let mut manual_input = tx_manual.input;
        auto_input.sort();
        manual_input.sort();
        assert_eq!(auto_input, manual_input);

        // Now check that the spend created above using auto-selection only works when the unconfirmed coin
        // is from self, whether or not it is from change.
        // 1. not from self and not change
        db_conn.remove_coins(&[unconfirmed_coin.outpoint]);
        let mut unconfirmed_coin_2 = unconfirmed_coin;
        unconfirmed_coin_2.is_from_self = false;
        unconfirmed_coin_2.is_change = false;
        db_conn.new_unspent_coins(&[unconfirmed_coin_2]);
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));
        // 2. not from self and change
        db_conn.remove_coins(&[unconfirmed_coin_2.outpoint]);
        unconfirmed_coin_2.is_from_self = false;
        unconfirmed_coin_2.is_change = true;
        db_conn.new_unspent_coins(&[unconfirmed_coin_2]);
        assert!(matches!(
            control.create_spend(&destinations, &[], 1, None),
            Ok(CreateSpendResult::InsufficientFunds { .. }),
        ));

        // Re-insert the original unconfirmed coin again.
        db_conn.remove_coins(&[unconfirmed_coin_2.outpoint]);
        db_conn.new_unspent_coins(&[unconfirmed_coin]);

        // Now do the same again, but this time specifying the change address to be the same
        // as for the auto spend.
        let change_address = bitcoin::Address::from_script(
            tx_auto.output[1].script_pubkey.as_script(),
            bitcoin::Network::Bitcoin,
        )
        .unwrap();
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(
                &destinations,
                &[confirmed_op_1, confirmed_op_2],
                1,
                Some(change_address.as_unchecked().clone()),
            )
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx_manual = psbt.unsigned_tx;
        // Now the outputs of each transaction are the same.
        assert_eq!(tx_auto.output, tx_manual.output);
        // Check again that inputs are still the same.
        let mut auto_input = tx_auto.input;
        let mut manual_input = tx_manual.input;
        auto_input.sort();
        manual_input.sort();
        assert_eq!(auto_input, manual_input);

        // Add a confirmed coin with a value near the dust limit and check that
        // `InsufficientFunds` error is returned if feerate is too high.
        let confirmed_op_3 = bitcoin::OutPoint {
            txid: confirmed_op_2.txid,
            vout: confirmed_op_2.vout + 10,
        };
        db_conn.new_unspent_coins(&[Coin {
            outpoint: confirmed_op_3,
            is_immature: false,
            block_info: Some(BlockInfo {
                height: 174500,
                time: 174500,
            }),
            amount: bitcoin::Amount::from_sat(DUST + 500),
            derivation_index: bip32::ChildNumber::from(56),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        let empty_dest = &HashMap::<bitcoin::Address<address::NetworkUnchecked>, u64>::new();
        assert_eq!(
            control.create_spend(empty_dest, &[confirmed_op_3], 5, None),
            Ok(CreateSpendResult::InsufficientFunds { missing: 150 },)
        );
        // If we use a lower fee, the self-send will succeed.
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(empty_dest, &[confirmed_op_3], 1, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let tx = psbt.unsigned_tx;
        let tx_prev_outpoints = tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<Vec<OutPoint>>();
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx_prev_outpoints, vec![confirmed_op_3]);
        assert_eq!(tx.output.len(), 1);

        // Can't create a transaction that spends an immature coinbase deposit.
        let imma_op = bitcoin::OutPoint::from_str(
            "4753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:0",
        )
        .unwrap();
        db_conn.new_unspent_coins(&[Coin {
            outpoint: imma_op,
            is_immature: true,
            block_info: None,
            amount: bitcoin::Amount::from_sat(COIN_VALUE),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        assert_eq!(
            control.create_spend(&destinations, &[imma_op], 1_001, None),
            Err(CommandError::ImmatureCoinbase(imma_op))
        );

        ms.shutdown();
    }

    #[test]
    fn backfill_previous_transactions_before_broadcast() {
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        let dummy_tx = funding_tx(control, &[(0, 100_000, 13, false)]);
        let dummy_op = bitcoin::OutPoint::new(dummy_tx.compute_txid(), 0);
        db_conn.new_txs(std::slice::from_ref(&dummy_tx));
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        let dummy_addr =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let destinations: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(dummy_addr, 50_000)].iter().cloned().collect();
        let psbt = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations, &[dummy_op], 1, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };

        // A stored PSBT that lost its previous transaction (legacy draft or an
        // import) is backfilled from the wallet and then passes re-verification.
        let mut stripped = psbt.clone();
        stripped.inputs[0].non_witness_utxo = None;
        assert!(spend::reverify_spend_before_broadcast(
            control.config.main_descriptor.as_vault().unwrap(),
            &stripped
        )
        .is_err());
        backfill_previous_transactions(&mut stripped, &mut DbTxGetter::new(&control.db())).unwrap();
        assert_eq!(
            stripped.inputs[0].non_witness_utxo.as_ref(),
            Some(&dummy_tx)
        );
        assert_eq!(stripped, psbt);
        spend::reverify_spend_before_broadcast(
            control.config.main_descriptor.as_vault().unwrap(),
            &stripped,
        )
        .unwrap();

        // An input already carrying a previous transaction is left untouched, even
        // one the wallet would not have handed out.
        let mut foreign = psbt.clone();
        let mut other = dummy_tx.clone();
        other.lock_time = bitcoin::absolute::LockTime::from_consensus(1);
        foreign.inputs[0].non_witness_utxo = Some(other.clone());
        backfill_previous_transactions(&mut foreign, &mut DbTxGetter::new(&control.db())).unwrap();
        assert_eq!(foreign.inputs[0].non_witness_utxo.as_ref(), Some(&other));

        // An input whose previous transaction the wallet does not hold is refused
        // with a recovery hint rather than reaching re-verification.
        let mut unknown = psbt;
        let unknown_op = bitcoin::OutPoint::new(other.compute_txid(), 0);
        unknown.unsigned_tx.input[0].previous_output = unknown_op;
        unknown.inputs[0].non_witness_utxo = None;
        assert_eq!(
            backfill_previous_transactions(&mut unknown, &mut DbTxGetter::new(&control.db())),
            Err(CommandError::SpendMissingPreviousTransaction(unknown_op))
        );

        ms.shutdown();
    }

    #[test]
    fn update_spend() {
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        // Two real funding transactions: coin a is vout 0 of the first, coin b is
        // vout 1 of the second, so spend creation can authenticate both inputs.
        let dummy_tx_a = funding_tx(control, &[(0, 100_000, 13, false)]);
        let dummy_tx_b = funding_tx(control, &[(1, 115_680, 34, false)]);
        let dummy_op_a = bitcoin::OutPoint::new(dummy_tx_a.compute_txid(), 0);
        let dummy_op_b = bitcoin::OutPoint::new(dummy_tx_b.compute_txid(), 1);
        db_conn.new_txs(&[dummy_tx_a, dummy_tx_b]);

        // Add two (unconfirmed) coins in DB
        db_conn.new_unspent_coins(&[
            Coin {
                outpoint: dummy_op_a,
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(100_000),
                derivation_index: bip32::ChildNumber::from(13),
                is_change: false,
                spend_txid: None,
                spend_block: None,
                is_from_self: false,
            },
            Coin {
                outpoint: dummy_op_b,
                is_immature: false,
                block_info: None,
                amount: bitcoin::Amount::from_sat(115_680),
                derivation_index: bip32::ChildNumber::from(34),
                is_change: false,
                spend_txid: None,
                spend_block: None,
                is_from_self: false,
            },
        ]);

        // Now create three transactions spending those coins differently
        let dummy_addr_a =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let dummy_addr_b =
            bitcoin::Address::from_str("bc1q39srgatmkp6k2ne3l52yhkjprdvunvspqydmkx").unwrap();
        let dummy_value_a = 50_000;
        let dummy_value_b = 60_000;
        let destinations_a: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(dummy_addr_a.clone(), dummy_value_a)]
                .iter()
                .cloned()
                .collect();
        let destinations_b: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(dummy_addr_b.clone(), dummy_value_b)]
                .iter()
                .cloned()
                .collect();
        let destinations_c: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
            [(dummy_addr_a, dummy_value_a), (dummy_addr_b, dummy_value_b)]
                .iter()
                .cloned()
                .collect();
        let mut psbt_a = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations_a, &[dummy_op_a], 1, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let txid_a = psbt_a.unsigned_tx.compute_txid();
        let psbt_b = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations_b, &[dummy_op_b], 10, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let txid_b = psbt_b.unsigned_tx.compute_txid();
        let psbt_c = if let CreateSpendResult::Success { psbt, .. } = control
            .create_spend(&destinations_c, &[dummy_op_a, dummy_op_b], 100, None)
            .unwrap()
        {
            psbt
        } else {
            panic!("expect successful spend creation")
        };
        let txid_c = psbt_c.unsigned_tx.compute_txid();

        // We can store and query them all
        control.update_spend(psbt_a.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_a).unwrap(), psbt_a);
        control.update_spend(psbt_b.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_b).unwrap(), psbt_b);
        control.update_spend(psbt_c.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_c).unwrap(), psbt_c);

        // As well as update them, with or without new signatures
        let sig = bitcoin::ecdsa::Signature::from_str("304402204004fcdbb9c0d0cbf585f58cee34dccb012efbd8fc2b0d5e97760045ae35803802201a0bd7ec2383e0b93748abc9946c8e17a8312e314dab85982aeba650e738cbf401").unwrap();
        psbt_a.inputs[0].partial_sigs.insert(
            bitcoin::PublicKey::from_str(
                "023a664c5617412f0b292665b1fd9d766456a7a3b1614c7e7c5f411200ff1958ef",
            )
            .unwrap(),
            sig,
        );
        control.update_spend(psbt_a.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_a).unwrap(), psbt_a);
        control.update_spend(psbt_b.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_b).unwrap(), psbt_b);
        control.update_spend(psbt_c.clone()).unwrap();
        assert_eq!(db_conn.spend_tx(&txid_c).unwrap(), psbt_c);

        // We can't store a PSBT spending an external coin
        let external_op = bitcoin::OutPoint::from_str(
            "8753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:2",
        )
        .unwrap();
        psbt_a.unsigned_tx.input[0].previous_output = external_op;
        assert_eq!(
            control.update_spend(psbt_a),
            Err(CommandError::UnknownOutpoint(external_op))
        );

        ms.shutdown();
    }

    #[test]
    fn rbf_psbt() {
        let dummy_op_a = bitcoin::OutPoint::from_str(
            "3753a1d74c0af8dd0a0f3b763c14faf3bd9ed03cbdf33337a074fb0e9f6c7810:0",
        )
        .unwrap();
        let mut dummy_bitcoind = DummyBitcoind::new();
        // Transaction spends outpoint a.
        let dummy_tx_a = bitcoin::Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::Blocks(absolute::Height::ZERO),
            input: vec![bitcoin::TxIn {
                previous_output: dummy_op_a,
                sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
                ..bitcoin::TxIn::default()
            }],
            output: vec![],
        };
        // PSBT corresponding to the above transaction.
        let dummy_psbt_a = Psbt {
            unsigned_tx: dummy_tx_a.clone(),
            version: 0,
            xpub: BTreeMap::new(),
            proprietary: BTreeMap::new(),
            unknown: BTreeMap::new(),
            inputs: vec![],
            outputs: vec![],
        };
        let dummy_txid_a = dummy_psbt_a.unsigned_tx.compute_txid();
        dummy_bitcoind.txs.insert(dummy_txid_a, (dummy_tx_a, None));
        let ms = DummyCoincube::new(dummy_bitcoind, DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        // The spend needs to be in DB before using RBF.
        assert_eq!(
            control.rbf_psbt(&dummy_txid_a, true, None),
            Err(CommandError::UnknownSpend(dummy_txid_a))
        );
        // Store the spend.
        db_conn.store_spend(&dummy_psbt_a);
        // Now add the coin to DB, but as spent.
        db_conn.new_unspent_coins(&[Coin {
            outpoint: dummy_op_a,
            is_immature: false,
            block_info: Some(BlockInfo {
                height: 174500,
                time: 174500,
            }),
            amount: bitcoin::Amount::from_sat(300_000),
            derivation_index: bip32::ChildNumber::from(11),
            is_change: false,
            spend_txid: Some(dummy_txid_a),
            spend_block: Some(BlockInfo {
                height: 184500,
                time: 184500,
            }),
            is_from_self: false,
        }]);
        // The coin is spent so we cannot RBF.
        assert_eq!(
            control.rbf_psbt(&dummy_txid_a, true, None),
            Err(CommandError::AlreadySpent(dummy_op_a))
        );
        db_conn.unspend_coins(&[dummy_op_a]);
        // Now remove the coin.
        db_conn.remove_coins(&[dummy_op_a]);
        assert_eq!(
            control.rbf_psbt(&dummy_txid_a, true, None),
            Err(CommandError::UnknownOutpoint(dummy_op_a))
        );
        // A target feerate not higher than the previous should return an error. This is tested in
        // the functional tests.

        ms.shutdown();
    }

    #[test]
    fn failed_rbf_without_previous_change_does_not_consume_index() {
        const COIN_VALUE: u64 = 100_000;
        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let funding = funding_tx(control, &[(0, COIN_VALUE, 13, false)]);
        let outpoint = bitcoin::OutPoint::new(funding.compute_txid(), 0);
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_txs(std::slice::from_ref(&funding));
        db_conn.new_unspent_coins(&[Coin {
            outpoint,
            is_immature: false,
            block_info: Some(BlockInfo { height: 1, time: 1 }),
            amount: bitcoin::Amount::from_sat(COIN_VALUE),
            derivation_index: 13.into(),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        }]);
        let destination =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let destinations = HashMap::from([(destination, 99_500)]);
        let psbt = match control
            .create_spend(&destinations, &[outpoint], 1, None)
            .unwrap()
        {
            CreateSpendResult::Success { psbt, .. } => psbt,
            other => panic!("expected changeless spend, got {:?}", other),
        };
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(db_conn.change_index(), 0.into());
        let txid = psbt.unsigned_tx.compute_txid();
        db_conn.store_spend(&psbt);
        db_conn.spend_coins(&[(outpoint, txid)]);

        assert!(matches!(
            control.rbf_psbt(&txid, false, Some(50_000)),
            Ok(CreateSpendResult::InsufficientFunds { .. })
        ));
        assert_eq!(db_conn.change_index(), 0.into());
        assert_eq!(control.reserve_change().unwrap().index(), 1.into());

        ms.shutdown();
    }

    #[test]
    fn rbf_change_conflicts_stop_at_the_budget() {
        let (daemon, conflicts, outpoint, destination) = funded_conflict_control(0);
        let control = daemon.control();
        let psbt = match control
            .create_spend(
                &HashMap::from([(destination, 99_500)]),
                &[outpoint],
                1,
                None,
            )
            .unwrap()
        {
            CreateSpendResult::Success { psbt, .. } => psbt,
            other => panic!("expected changeless spend, got {:?}", other),
        };
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        let txid = psbt.unsigned_tx.compute_txid();
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.store_spend(&psbt);
        db_conn.spend_coins(&[(outpoint, txid)]);
        conflicts.store(
            CHANGE_COMMIT_CONFLICT_LIMIT,
            std::sync::atomic::Ordering::SeqCst,
        );

        assert_eq!(
            control.rbf_psbt(&txid, true, None),
            Err(CommandError::ChangeReservationContended)
        );
        assert_eq!(conflicts.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            db_conn.change_index(),
            bip32::ChildNumber::from(CHANGE_COMMIT_CONFLICT_LIMIT as u32)
        );
        daemon.shutdown();
    }

    /// The amount [`spend_in_mempool`] pays out of the wallet.
    const PAYMENT: u64 = 100_000;

    /// A wallet with two confirmed coins, the first spent at `feerate_vb` to pay [`PAYMENT`] out
    /// with change. Our records show that spend as broadcast and unconfirmed: its PSBT is stored
    /// and the coin is marked as spent by it. Its signed transaction is returned rather than
    /// stored, the caller deciding whether the poller has seen it yet.
    fn spend_in_mempool(
        bitcoind: DummyBitcoind,
        feerate_vb: u64,
    ) -> (DummyCoincube, Psbt, bitcoin::Transaction) {
        let ms = DummyCoincube::new(bitcoind, DummyDatabase::new());
        let control = ms.control();
        let funding = funding_tx(control, &[(0, 300_000, 13, false), (1, 200_000, 14, false)]);
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_txs(std::slice::from_ref(&funding));
        let coins: Vec<Coin> = [(0u32, 300_000u64, 13u32), (1, 200_000, 14)]
            .iter()
            .map(|&(vout, amount, index)| Coin {
                outpoint: bitcoin::OutPoint::new(funding.compute_txid(), vout),
                is_immature: false,
                block_info: Some(BlockInfo { height: 1, time: 1 }),
                amount: bitcoin::Amount::from_sat(amount),
                derivation_index: index.into(),
                is_change: false,
                spend_txid: None,
                spend_block: None,
                is_from_self: false,
            })
            .collect();
        db_conn.new_unspent_coins(&coins);
        let destination =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        let psbt = replacement(control.create_spend(
            &HashMap::from([(destination, PAYMENT)]),
            &[coins[0].outpoint],
            feerate_vb,
            None,
        ));
        db_conn.store_spend(&psbt);
        db_conn.spend_coins(&[(coins[0].outpoint, psbt.unsigned_tx.compute_txid())]);
        let signed = with_witnesses(psbt.unsigned_tx.clone());
        (ms, psbt, signed)
    }

    /// `tx` with witnesses standing in for its signatures, giving it a signed transaction's size.
    fn with_witnesses(mut tx: bitcoin::Transaction) -> bitcoin::Transaction {
        for txin in &mut tx.input {
            txin.witness = Witness::from_slice(&[vec![0; 72], vec![0; 70]]);
        }
        tx
    }

    /// The PSBT of a spend or replacement that was successfully created.
    fn replacement(result: Result<CreateSpendResult, CommandError>) -> Psbt {
        match result {
            Ok(CreateSpendResult::Success { psbt, .. }) => psbt,
            other => panic!("expected a transaction, got {:?}", other),
        }
    }

    fn vsize(tx: &bitcoin::Transaction) -> u64 {
        tx.vsize().try_into().unwrap()
    }

    /// Esplora cannot see the mempool and always reads it as empty, so `rbfpsbt` priced every
    /// cancel at 1 sat/vb, under the transaction being replaced, and the network refused it. On
    /// such a backend the replacement is priced from our own records instead.
    #[test]
    fn rbf_without_a_mempool_view_outbids_the_replaced_transaction() {
        let mut esplora = DummyBitcoind::new();
        esplora.sees_mempool = false;
        let (ms, psbt, signed) = spend_in_mempool(esplora, 20);
        let control = ms.control();
        let txid = psbt.unsigned_tx.compute_txid();
        let fee = psbt.fee().unwrap().to_sat();
        let floor = fee / vsize(&signed) + 1;

        // Before the poller stores the signed transaction, the unsigned one stands in for it. It
        // lacks witnesses, so it overstates the feerate: the replacement pays more, never less.
        let unsigned_floor = fee / vsize(&psbt.unsigned_tx) + 1;
        assert!(unsigned_floor > floor);
        assert_eq!(
            control.rbf_psbt(&txid, false, Some(unsigned_floor - 1)),
            Err(CommandError::RbfError(RbfErrorInfo::TooLowFeerate(
                unsigned_floor - 1,
                unsigned_floor
            )))
        );

        // Once stored, the signed transaction's own feerate is the one to beat (rule #6).
        control
            .db()
            .lock()
            .unwrap()
            .connection()
            .new_txs(std::slice::from_ref(&signed));
        assert_eq!(
            control.rbf_psbt(&txid, false, Some(floor - 1)),
            Err(CommandError::RbfError(RbfErrorInfo::TooLowFeerate(
                floor - 1,
                floor
            )))
        );
        let bump = replacement(control.rbf_psbt(&txid, false, Some(floor)));
        assert!(bump.fee().unwrap().to_sat() >= fee + vsize(&bump.unsigned_tx));

        // A cancel pays more than the transaction it replaces, plus its own relay (rules #3, #4).
        // Being smaller than that transaction, it then pays a higher feerate too (rule #6).
        let cancel = replacement(control.rbf_psbt(&txid, true, None));
        assert_eq!(
            cancel.unsigned_tx.output.len(),
            1,
            "a cancel pays us back only"
        );
        assert_eq!(
            cancel.unsigned_tx.input[0].previous_output,
            psbt.unsigned_tx.input[0].previous_output
        );
        assert!(vsize(&cancel.unsigned_tx) < vsize(&psbt.unsigned_tx));
        assert!(cancel.fee().unwrap().to_sat() >= fee + vsize(&cancel.unsigned_tx));
        ms.shutdown();

        // A backend that sees the mempool is trusted when it reads empty: the transaction left
        // the mempool, nothing is left to outbid. That is how every cancel was priced on Esplora.
        let (ms, psbt, signed) = spend_in_mempool(DummyBitcoind::new(), 20);
        let control = ms.control();
        let txid = psbt.unsigned_tx.compute_txid();
        control
            .db()
            .lock()
            .unwrap()
            .connection()
            .new_txs(std::slice::from_ref(&signed));
        let cancel = replacement(control.rbf_psbt(&txid, true, None));
        assert!(cancel.fee().unwrap() < psbt.fee().unwrap());
        ms.shutdown();
    }

    /// Our own transactions spending from the replaced one are evicted with it, so the
    /// replacement must pay their fees as well (rule #3). Without a mempool view, they are found
    /// in our records.
    #[test]
    fn rbf_without_a_mempool_view_pays_for_our_descendants() {
        const CHILD_FEE: u64 = 5_000;
        let mut esplora = DummyBitcoind::new();
        esplora.sees_mempool = false;
        let (ms, psbt, signed) = spend_in_mempool(esplora, 20);
        let control = ms.control();
        let txid = psbt.unsigned_tx.compute_txid();
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_txs(std::slice::from_ref(&signed));

        // The change was spent before the transaction creating it confirmed.
        let (vout, change) = psbt
            .unsigned_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, txo)| txo.value.to_sat() != PAYMENT)
            .unwrap();
        let change_outpoint = bitcoin::OutPoint::new(txid, vout.try_into().unwrap());
        db_conn.new_unspent_coins(&[Coin {
            outpoint: change_outpoint,
            is_immature: false,
            block_info: None,
            amount: change.value,
            derivation_index: 0.into(),
            is_change: true,
            spend_txid: None,
            spend_block: None,
            is_from_self: true,
        }]);
        let child = with_witnesses(bitcoin::Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: change_outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: change.value - Amount::from_sat(CHILD_FEE),
                script_pubkey: ScriptBuf::from_bytes([vec![0x51, 0x20], vec![0xcd; 32]].concat()),
            }],
        });
        db_conn.new_txs(std::slice::from_ref(&child));
        db_conn.spend_coins(&[(change_outpoint, child.compute_txid())]);

        let cancel = replacement(control.rbf_psbt(&txid, true, None));
        assert!(
            cancel.fee().unwrap().to_sat()
                >= psbt.fee().unwrap().to_sat() + CHILD_FEE + vsize(&cancel.unsigned_tx)
        );
        ms.shutdown();
    }

    /// Our records may show the coins spent by another transaction than the one asked to be
    /// replaced: a fee bump that replaced it already. That one is in the mempool and is the one to
    /// outbid, while the transaction it replaced is not paid for a second time.
    #[test]
    fn rbf_without_a_mempool_view_outbids_what_our_records_show_spending_the_coins() {
        let mut esplora = DummyBitcoind::new();
        esplora.sees_mempool = false;
        let (ms, psbt, signed) = spend_in_mempool(esplora, 20);
        let control = ms.control();
        let txid = psbt.unsigned_tx.compute_txid();
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_txs(std::slice::from_ref(&signed));

        let bump = replacement(control.rbf_psbt(&txid, false, Some(40)));
        let bump_txid = bump.unsigned_tx.compute_txid();
        let signed_bump = with_witnesses(bump.unsigned_tx.clone());
        db_conn.store_spend(&bump);
        db_conn.new_txs(std::slice::from_ref(&signed_bump));
        db_conn.unspend_coins(&[psbt.unsigned_tx.input[0].previous_output]);
        db_conn.spend_coins(
            &bump
                .unsigned_tx
                .input
                .iter()
                .map(|txin| (txin.previous_output, bump_txid))
                .collect::<Vec<_>>(),
        );

        let bump_fee = bump.fee().unwrap().to_sat();
        let floor = bump_fee / vsize(&signed_bump) + 1;
        assert_eq!(
            control.rbf_psbt(&txid, false, Some(floor - 1)),
            Err(CommandError::RbfError(RbfErrorInfo::TooLowFeerate(
                floor - 1,
                floor
            )))
        );
        let cancel = replacement(control.rbf_psbt(&txid, true, None));
        let cancel_fee = cancel.fee().unwrap().to_sat();
        assert!(cancel_fee >= bump_fee + vsize(&cancel.unsigned_tx));
        assert!(cancel_fee < bump_fee + psbt.fee().unwrap().to_sat());
        ms.shutdown();
    }

    /// A conflict our records show, but which also spends a coin they have no value for, cannot
    /// be priced. Leaving it out would build a replacement it may outpay, so there is none.
    #[test]
    fn rbf_without_a_mempool_view_refuses_a_conflict_it_cannot_price() {
        let mut esplora = DummyBitcoind::new();
        esplora.sees_mempool = false;
        let (ms, psbt, signed) = spend_in_mempool(esplora, 20);
        let control = ms.control();
        let txid = psbt.unsigned_tx.compute_txid();
        let mut db_conn = control.db().lock().unwrap().connection();
        db_conn.new_txs(std::slice::from_ref(&signed));

        let our_coin = psbt.unsigned_tx.input[0].previous_output;
        let not_ours = bitcoin::OutPoint::new(
            Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                .unwrap(),
            0,
        );
        let conflict = with_witnesses(bitcoin::Transaction {
            version: TxVersion::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: [our_coin, not_ours]
                .iter()
                .map(|previous_output| TxIn {
                    previous_output: *previous_output,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::new(),
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(250_000),
                script_pubkey: ScriptBuf::from_bytes([vec![0x51, 0x20], vec![0xcd; 32]].concat()),
            }],
        });
        db_conn.new_txs(std::slice::from_ref(&conflict));
        db_conn.unspend_coins(&[our_coin]);
        db_conn.spend_coins(&[(our_coin, conflict.compute_txid())]);

        assert!(matches!(
            control.rbf_psbt(&txid, true, None),
            Err(CommandError::MempoolUnavailable(_))
        ));
        assert!(matches!(
            control.rbf_psbt(&txid, false, Some(50)),
            Err(CommandError::MempoolUnavailable(_))
        ));
        ms.shutdown();
    }

    #[test]
    fn list_confirmed_transactions() {
        let outpoint = OutPoint::new(
            Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                .unwrap(),
            0,
        );

        let deposit1: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(100_000_000),
            }],
        };

        let deposit2: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(2000),
            }],
        };

        let deposit3: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(3000),
            }],
        };

        let spend_tx: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: OutPoint {
                    txid: deposit1.compute_txid(),
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![
                TxOut {
                    script_pubkey: ScriptBuf::new(),
                    value: Amount::from_sat(4000),
                },
                TxOut {
                    script_pubkey: ScriptBuf::new(),
                    value: Amount::from_sat(100_000_000 - 4000 - 1000),
                },
            ],
        };

        let mut db = DummyDatabase::new();
        db.insert_coins(vec![
            // Deposit 1
            Coin {
                is_change: false,
                is_immature: false,
                outpoint: OutPoint {
                    txid: deposit1.compute_txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 1, time: 1 }),
                spend_block: Some(BlockInfo { height: 3, time: 3 }),
                derivation_index: ChildNumber::from(0),
                amount: bitcoin::Amount::from_sat(100_000_000),
                spend_txid: Some(spend_tx.compute_txid()),
                is_from_self: false,
            },
            // Deposit 2
            Coin {
                is_change: false,
                is_immature: false,
                outpoint: OutPoint {
                    txid: deposit2.compute_txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 2, time: 2 }),
                spend_block: None,
                derivation_index: ChildNumber::from(1),
                amount: bitcoin::Amount::from_sat(2000),
                spend_txid: None,
                is_from_self: false,
            },
            // This coin is a change output.
            Coin {
                is_change: true,
                is_immature: false,
                outpoint: OutPoint::new(spend_tx.compute_txid(), 1),
                block_info: Some(BlockInfo { height: 3, time: 3 }),
                spend_block: None,
                derivation_index: ChildNumber::from(2),
                amount: bitcoin::Amount::from_sat(100_000_000 - 4000 - 1000),
                spend_txid: None,
                is_from_self: false,
            },
            // Deposit 3
            Coin {
                is_change: false,
                is_immature: false,
                outpoint: OutPoint {
                    txid: deposit3.compute_txid(),
                    vout: 0,
                },
                block_info: Some(BlockInfo { height: 4, time: 4 }),
                spend_block: None,
                derivation_index: ChildNumber::from(3),
                amount: bitcoin::Amount::from_sat(3000),
                spend_txid: None,
                is_from_self: false,
            },
        ]);

        let mut txs_map = HashMap::new();
        txs_map.insert(
            deposit1.compute_txid(),
            (
                deposit1.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 1,
                    height: 1,
                }),
            ),
        );
        txs_map.insert(
            deposit2.compute_txid(),
            (
                deposit2.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 2,
                    height: 2,
                }),
            ),
        );
        txs_map.insert(
            spend_tx.compute_txid(),
            (
                spend_tx.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 3,
                    height: 3,
                }),
            ),
        );
        txs_map.insert(
            deposit3.compute_txid(),
            (
                deposit3.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 4,
                    height: 4,
                }),
            ),
        );

        let ms = DummyCoincube::new(DummyBitcoind::new(), db);

        let control = &ms.control();
        let mut db_conn = control.db.connection();
        let txs: Vec<_> = txs_map.values().map(|(tx, _)| tx.clone()).collect();
        db_conn.new_txs(&txs);

        let mut transactions = control.list_confirmed_transactions(0, 4, 10).transactions;
        transactions.sort_by_key(|tx| std::cmp::Reverse(tx.height));
        assert_eq!(transactions.len(), 4);

        assert_eq!(transactions[0].time, Some(4));
        assert_eq!(transactions[0].tx, deposit3);

        assert_eq!(transactions[1].time, Some(3));
        assert_eq!(transactions[1].tx, spend_tx);

        assert_eq!(transactions[2].time, Some(2));
        assert_eq!(transactions[2].tx, deposit2);

        assert_eq!(transactions[3].time, Some(1));
        assert_eq!(transactions[3].tx, deposit1);

        let mut transactions = control.list_confirmed_transactions(2, 3, 10).transactions;
        transactions.sort_by_key(|tx| std::cmp::Reverse(tx.height));
        assert_eq!(transactions.len(), 2);

        assert_eq!(transactions[0].time, Some(3));
        assert_eq!(transactions[1].time, Some(2));
        assert_eq!(transactions[1].tx, deposit2);

        let transactions = control.list_confirmed_transactions(2, 3, 1).transactions;
        assert_eq!(transactions.len(), 1);

        assert_eq!(transactions[0].time, Some(3));
        assert_eq!(transactions[0].tx, spend_tx);

        ms.shutdown();
    }

    #[test]
    fn list_transactions() {
        let outpoint = OutPoint::new(
            Txid::from_str("617eab1fc0b03ee7f82ba70166725291783461f1a0e7975eaf8b5f8f674234f3")
                .unwrap(),
            0,
        );

        let tx1: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(100_000_000),
            }],
        };

        let tx2: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(2000),
            }],
        };

        let tx3: Transaction = Transaction {
            version: TxVersion::ONE,
            lock_time: absolute::LockTime::Blocks(absolute::Height::from_consensus(1).unwrap()),
            input: vec![TxIn {
                witness: Witness::new(),
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence(0),
            }],
            output: vec![TxOut {
                script_pubkey: ScriptBuf::new(),
                value: Amount::from_sat(3000),
            }],
        };

        let mut txs_map = HashMap::new();
        txs_map.insert(
            tx1.compute_txid(),
            (
                tx1.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 1,
                    height: 1,
                }),
            ),
        );
        txs_map.insert(
            tx2.compute_txid(),
            (
                tx2.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 2,
                    height: 2,
                }),
            ),
        );
        txs_map.insert(
            tx3.compute_txid(),
            (
                tx3.clone(),
                Some(Block {
                    hash: bitcoin::BlockHash::from_str(
                        "0000000000000000000326b8fca8d3f820647c97ea33ef722096b3c7b2c8ee94",
                    )
                    .unwrap(),
                    time: 4,
                    height: 4,
                }),
            ),
        );

        let ms = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let control = &ms.control();
        let mut db_conn = control.db.connection();
        let txs: Vec<_> = txs_map.values().map(|(tx, _)| tx.clone()).collect();
        db_conn.new_txs(&txs);
        // We need coins in the DB in order to get the block info for the transactions.
        for (txid, (_tx, block)) in txs_map {
            // Insert more than one coin per transaction to check that the command does not
            // return duplicate transactions.
            for vout in 0..4 {
                db_conn.new_unspent_coins(&[Coin {
                    outpoint: bitcoin::OutPoint::new(txid, vout),
                    is_immature: false,
                    block_info: block.map(|b| BlockInfo {
                        height: b.height,
                        time: b.time,
                    }),
                    amount: bitcoin::Amount::from_sat(100_000),
                    derivation_index: bip32::ChildNumber::from(13),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            }
        }

        let transactions = control
            .list_transactions(&[tx1.compute_txid()])
            .transactions;
        assert_eq!(transactions.len(), 1);
        assert_eq!(transactions[0].tx, tx1);

        let transactions = control
            .list_transactions(&[tx1.compute_txid(), tx2.compute_txid(), tx3.compute_txid()])
            .transactions;
        assert_eq!(transactions.len(), 3);

        let txs: Vec<Transaction> = transactions
            .iter()
            .map(|transaction| transaction.tx.clone())
            .collect();

        assert!(txs.contains(&tx1));
        assert!(txs.contains(&tx2));
        assert!(txs.contains(&tx3));

        ms.shutdown();
    }

    #[test]
    fn create_recovery() {
        let ms = DummyCoincube::new_timelock(DummyBitcoind::new(), DummyDatabase::new(), 10);
        let control = &ms.control();
        let mut db_conn = control.db().lock().unwrap().connection();
        // Real funding transaction: vout 0 (100_000 sats, index 13), vout 1 (10_000,
        // index 1378) and vout 2 (dust + 126, index 13) for the too-small case.
        let dummy_tx = funding_tx(
            control,
            &[
                (0, 100_000, 13, false),
                (1, 10_000, 1378, false),
                (2, DUST + 126, 13, false),
            ],
        );
        let dummy_txid = dummy_tx.compute_txid();
        let dummy_op = bitcoin::OutPoint::new(dummy_txid, 0);
        db_conn.new_txs(&[dummy_tx]);

        // Arguments sanity checking
        let dummy_addr =
            bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
        // Feerate cannot be less than 1.
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[], 0, None),
            Err(CommandError::InvalidFeerate(0))
        );
        // If we ask to sweep to an address from another network, it will fail.
        let invalid_addr =
            bitcoin::Address::from_str("tb1qfufcrdyarcg5eph608c6l8vktrc9re6agu4se2").unwrap();
        assert!(matches!(
            control.create_recovery(invalid_addr, &[], 1, None),
            Err(CommandError::Address(
                address::error::ParseError::NetworkValidation { .. }
            ))
        ));

        // We have no coins to create recovery.
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::RecoveryNotAvailable),
        ));
        // Coin is unknown.
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::UnknownOutpoint(dummy_op)),
        );

        // Add unconfirmed coin.
        let dummy_coin = Coin {
            outpoint: dummy_op,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(100_000),
            derivation_index: bip32::ChildNumber::from(13),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        };
        db_conn.new_unspent_coins(&[dummy_coin]);
        // Recovery not available for unconfirmed coins.
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::RecoveryNotAvailable),
        ));
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::OutpointNotRecoverable(dummy_op, 10)),
        );

        // Confirm coin such that timelock (10) has not expired at next block (101).
        db_conn.confirm_coins(&[(dummy_op, 92, 100_000)]);
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::RecoveryNotAvailable),
        ));
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::OutpointNotRecoverable(dummy_op, 10)),
        );

        // If we use a smaller timelock value it works, even though we don't have any such
        // recovery timelock (see https://github.com/wizardsardine/coincube/issues/1089).
        assert!(control
            .create_recovery(dummy_addr.clone(), &[], 1, Some(9))
            .is_ok());
        assert!(control
            .create_recovery(dummy_addr.clone(), &[dummy_op], 1, Some(9))
            .is_ok());

        // Remove coin, re-add and confirm such that recovery available at next block.
        db_conn.remove_coins(&[dummy_op]);
        db_conn.new_unspent_coins(&[dummy_coin]);
        db_conn.confirm_coins(&[(dummy_op, 91, 100_000)]);
        let res = control.create_recovery(dummy_addr.clone(), &[], 1, None);
        assert!(res.is_ok());
        let psbt = res.unwrap().psbt;
        assert_eq!(psbt.outputs.len(), 1);
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        // Amount is coin value minus fee.
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().value,
            Amount::from_sat(100_000 - 127)
        );

        // If we pass a larger timelock, it no longer works:
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, Some(11)),
            Err(CommandError::RecoveryNotAvailable),
        ));
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, Some(11)),
            Err(CommandError::OutpointNotRecoverable(dummy_op, 11)),
        );

        // If the coin is spending, it is no longer recoverable.
        db_conn.spend_coins(&[(
            dummy_op,
            Txid::from_str("84f09bddfe0f036d0390edf655636ad6092c3ab8f09b2bb1503caa393463f241")
                .unwrap(),
        )]);
        assert!(matches!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::RecoveryNotAvailable),
        ));
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::AlreadySpent(dummy_op)),
        );

        // Now remove the coin and add another one, this time with an amount that is too small
        // to create an output. It is paid by vout 2 of the same funding transaction, so its
        // previous output still authenticates the amount.
        // This will give a coin selection error due to insufficient funds.
        db_conn.remove_coins(&[dummy_op]);
        let dummy_op = bitcoin::OutPoint::new(dummy_txid, 2);
        let mut dummy_coin = dummy_coin;
        dummy_coin.outpoint = dummy_op;
        dummy_coin.amount = Amount::from_sat(DUST + 126);
        db_conn.new_unspent_coins(&[dummy_coin]);
        db_conn.confirm_coins(&[(dummy_op, 91, 100_000)]);
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(InsufficientFunds { missing: 1 })
            )),
        );
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(InsufficientFunds { missing: 1 })
            )),
        );

        // Add a new coin so that we have enough funds for the recovery.
        let dummy_op_2 = bitcoin::OutPoint::new(dummy_txid, 1);
        let dummy_coin_2 = Coin {
            outpoint: dummy_op_2,
            is_immature: false,
            block_info: None,
            amount: bitcoin::Amount::from_sat(10_000),
            derivation_index: bip32::ChildNumber::from(1378),
            is_change: false,
            spend_txid: None,
            spend_block: None,
            is_from_self: false,
        };
        db_conn.new_unspent_coins(&[dummy_coin_2]);
        db_conn.confirm_coins(&[(dummy_op_2, 92, 200_000)]);
        // Coin cannot be used as the timelock will still be in place at the next block.
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[], 1, None),
            Err(CommandError::SpendCreation(
                SpendCreationError::CoinSelection(InsufficientFunds { missing: 1 })
            )),
        );
        // If we try to specify the new coin, we'll get an error that the coin is not recoverable.
        assert_eq!(
            control.create_recovery(dummy_addr.clone(), &[dummy_op, dummy_op_2], 1, None),
            Err(CommandError::OutpointNotRecoverable(dummy_op_2, 10)),
        );
        // Using a shorter timelock parameter works:
        assert!(control
            .create_recovery(dummy_addr.clone(), &[], 1, Some(9))
            .is_ok());
        assert!(control
            .create_recovery(dummy_addr.clone(), &[dummy_op, dummy_op_2], 1, Some(9))
            .is_ok());

        // Now re-add the coin with a confirmation one block earlier.
        db_conn.remove_coins(&[dummy_op_2]);
        db_conn.new_unspent_coins(&[dummy_coin_2]);
        db_conn.confirm_coins(&[(dummy_op_2, 91, 200_000)]);

        // Now both coins are used in the recovery and we have enough funds.
        let res = control.create_recovery(dummy_addr.clone(), &[], 1, None);
        assert!(res.is_ok());
        let psbt = res.unwrap().psbt;
        assert_eq!(psbt.outputs.len(), 1);
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        // Amount is coin value minus fee.
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().value,
            Amount::from_sat(/* coin 1 */ DUST + 126 + /* coin 2 */10_000 - /* fee */ 211)
        );

        // Do the same again, now specifying the outpoints explicitly.
        let res = control.create_recovery(dummy_addr.clone(), &[dummy_op, dummy_op_2], 1, None);
        assert!(res.is_ok());
        let psbt = res.unwrap().psbt;
        assert_eq!(psbt.outputs.len(), 1);
        assert_eq!(psbt.unsigned_tx.output.len(), 1);
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().script_pubkey,
            dummy_addr.assume_checked_ref().script_pubkey()
        );
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().value,
            Amount::from_sat(/* coin 1 */ DUST + 126 + /* coin 2 */10_000 - /* fee */ 211)
        );

        // Now check that increasing the feerate increases the fee.
        let res = control.create_recovery(dummy_addr.clone(), &[], 2, None);
        assert!(res.is_ok());
        let psbt = res.unwrap().psbt;
        assert_eq!(
            psbt.unsigned_tx.output.first().unwrap().value,
            Amount::from_sat(/* coin 1 */ DUST + 126 + /* coin 2 */10_000 - /* fee */ 2 * 211)
        );

        ms.shutdown();
    }

    // ── Chain-keyed signature merge and finalisation (Lane B1.2) ────────────────
    //
    // A 2-of-3 native P2WSH Vault with a timelocked recovery leaf, signed with
    // real keys, so the Bitcoin arm can be checked byte-for-byte against
    // rust-miniscript's finaliser and the Blake2b arm against the core one. The
    // daemon itself still refuses to start on a Blake2b chain (dormant), which is
    // why the two helpers are exercised directly with both chain identities.
    /// A full scan holds the backend lock until it finishes, which over a slow Esplora
    /// server took most of an hour. `get_info` used to read a pending rescan's progress
    /// through that lock, so the app's rescan card sat at "0.00%" while the call waited.
    /// It now answers from the lock-free record, with the scan's real progress.
    #[test]
    fn get_info_reports_a_running_full_scan_without_waiting_for_the_backend() {
        let dummy = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let config = dummy.control().config.clone();
        let backend: std::sync::Arc<std::sync::Mutex<dyn BitcoinInterface>> =
            std::sync::Arc::new(std::sync::Mutex::new(DummyBitcoind::new()));
        let db = DummyDatabase::new();
        db.connection().set_rescan(1_000);
        let history = std::sync::Arc::new(crate::bitcoin::HistorySyncCache::default());
        history.begin_full_scan(400);
        for _ in 0..100 {
            history.address_checked();
        }
        let (poller_sender, _poller_receiver) = std::sync::mpsc::sync_channel(1);
        let control = DaemonControl::new(
            config,
            backend.clone(),
            poller_sender,
            std::sync::Arc::new(std::sync::Mutex::new(db)),
            bitcoin::secp256k1::Secp256k1::verification_only(),
            Default::default(),
            Default::default(),
            history,
        );

        // The poller, mid-scan.
        let _scanning = backend.lock().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(control.get_info()).unwrap());
        let info = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("get_info must not wait for the backend lock");
        assert_eq!(info.rescan_progress, Some(0.25));
        assert!(info.history_sync.full_scan_in_progress);
        assert_eq!(info.history_sync.addresses_checked, 100);
        assert_eq!(info.history_sync.addresses_expected, 400);
        assert_eq!(info.history_sync.full_scan_completed_at, None);
    }

    #[test]
    fn eager_retry_returns_while_backend_is_scanning_and_queue_is_full() {
        let dummy = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let config = dummy.control().config.clone();
        let backend: std::sync::Arc<std::sync::Mutex<dyn BitcoinInterface>> =
            std::sync::Arc::new(std::sync::Mutex::new(DummyBitcoind::new()));
        let history = std::sync::Arc::new(crate::bitcoin::HistorySyncCache::default());
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        sender.try_send(PollerMessage::PollNowNoAck).unwrap();
        let control = DaemonControl::new(
            config,
            backend.clone(),
            sender,
            std::sync::Arc::new(std::sync::Mutex::new(DummyDatabase::new())),
            bitcoin::secp256k1::Secp256k1::verification_only(),
            Default::default(),
            Default::default(),
            history.clone(),
        );

        let _scanning = backend.lock().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            control.request_sync();
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("retry must not wait for the full scan's backend mutex");
        assert!(history.take_eager_sync_request());
        assert!(matches!(
            receiver.try_recv(),
            Ok(PollerMessage::PollNowNoAck)
        ));
    }

    /// The daemon must hand its backend the very record `get_info` reports from:
    /// otherwise a scan's progress is counted somewhere nobody reads.
    #[test]
    fn the_daemon_installs_the_history_record_get_info_reports() {
        let bitcoind = DummyBitcoind::new();
        let installed = bitcoind.history_sync.clone();
        let dummy = DummyCoincube::new(bitcoind, DummyDatabase::new());
        let cache = installed
            .lock()
            .unwrap()
            .clone()
            .expect("the daemon installs a history record in its backend");
        assert!(
            !dummy
                .control()
                .get_info()
                .history_sync
                .full_scan_in_progress
        );
        cache.begin_full_scan(10);
        cache.address_checked();
        let reported = dummy.control().get_info().history_sync;
        assert!(reported.full_scan_in_progress);
        assert_eq!(reported.addresses_checked, 1);
        assert_eq!(reported.addresses_expected, 10);
    }

    /// Older daemons send neither `history_sync` nor an explicit chain ID.
    #[test]
    fn get_info_result_without_history_sync_still_parses() {
        let dummy = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let mut value = serde_json::to_value(dummy.control().get_info()).unwrap();
        value.as_object_mut().unwrap().remove("history_sync");
        value.as_object_mut().unwrap().remove("chain_id");
        let info: GetInfoResult = serde_json::from_value(value).unwrap();
        assert_eq!(info.history_sync, crate::bitcoin::HistorySync::default());
        assert_eq!(info.chain_id, None);
    }

    #[test]
    fn legacy_get_info_descriptor_json_remains_the_struct_object() {
        let dummy = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
        let info = serde_json::to_value(dummy.control().get_info()).unwrap();
        let main = &info["descriptors"]["main"];
        let object = main.as_object().expect("legacy main is a JSON object");
        assert!(object.contains_key("multi_desc"));
        assert!(object.contains_key("receive_desc"));
        assert!(object.contains_key("change_desc"));
        assert!(!object.contains_key("kind"));
        assert!(info.get("imported_wallet").is_none());
    }

    mod chain_keyed_spend {
        use super::*;
        use coincube_core::{
            chain::ChainId,
            descriptors::{CoincubeDescriptor, CoincubePolicy, PathInfo},
            miniscript::{
                bitcoin::{
                    self as btc, absolute, bip32::DerivationPath, ecdsa, secp256k1,
                    sighash::EcdsaSighashType, transaction, Amount, OutPoint, ScriptBuf, Sequence,
                    Transaction, TxIn, TxOut,
                },
                descriptor::{DerivPaths, DescriptorMultiXKey, DescriptorPublicKey, Wildcard},
            },
            psbt_unified::{unified_signatures, UnifiedPsbt},
            signer::MasterSigner,
            unified_signing::sign_p2wsh_all_unified,
        };
        use std::str::FromStr;

        fn signer(byte: u8) -> MasterSigner {
            let mnemonic = coincube_core::bip39::Mnemonic::from_entropy(&[byte; 16]).unwrap();
            MasterSigner::from_mnemonic(btc::Network::Bitcoin, mnemonic).unwrap()
        }

        fn descriptor_key(
            signer: &MasterSigner,
            branch: u32,
            secp: &secp256k1::Secp256k1<secp256k1::All>,
        ) -> DescriptorPublicKey {
            let origin = DerivationPath::from(vec![
                bip32::ChildNumber::from_hardened_idx(48).unwrap(),
                bip32::ChildNumber::from_hardened_idx(branch).unwrap(),
            ]);
            DescriptorPublicKey::MultiXPub(DescriptorMultiXKey {
                origin: Some((signer.fingerprint(secp), origin.clone())),
                xkey: signer.xpub_at(&origin, secp),
                derivation_paths: DerivPaths::new(vec![
                    DerivationPath::from_str("m/0").unwrap(),
                    DerivationPath::from_str("m/1").unwrap(),
                ])
                .unwrap(),
                wildcard: Wildcard::Unhardened,
            })
        }

        /// (signers, unsigned PSBT with authenticated prevout and witness script).
        fn vault_psbt() -> (Vec<MasterSigner>, Psbt) {
            let (signers, _, psbt) = vault_fixture();
            (signers, psbt)
        }

        /// A `DaemonControl` on Bitcoin Blake2b over the dummy backend and
        /// database, built directly: `DaemonHandle::start` refuses the chain
        /// while it is dormant, but the control's own chain-keyed paths are
        /// what these tests are about.
        fn blake2b_control(descriptor: CoincubeDescriptor) -> DaemonControl {
            let tmp = tmp_dir();
            std::fs::create_dir_all(&tmp).unwrap();
            let config = crate::config::Config::new(
                crate::config::BitcoinConfig::new(
                    ChainId::BitcoinBlake2b,
                    std::time::Duration::from_secs(2),
                ),
                None,
                log::LevelFilter::Debug,
                descriptor,
                crate::datadir::DataDirectory::new(tmp.join("d")),
            );
            let (poller_sender, _poller_receiver) = std::sync::mpsc::sync_channel(1);
            DaemonControl::new(
                config,
                std::sync::Arc::new(std::sync::Mutex::new(DummyBitcoind::new())),
                poller_sender,
                std::sync::Arc::new(std::sync::Mutex::new(DummyDatabase::new())),
                secp256k1::Secp256k1::verification_only(),
                Default::default(),
                Default::default(),
                Default::default(),
            )
        }

        /// (signers, descriptor, unsigned PSBT with authenticated prevout and
        /// witness script).
        fn vault_fixture() -> (Vec<MasterSigner>, CoincubeDescriptor, Psbt) {
            let secp = secp256k1::Secp256k1::new();
            let signers = vec![signer(11), signer(12), signer(13)];
            let primary = PathInfo::Multi(
                2,
                vec![
                    descriptor_key(&signers[0], 0, &secp),
                    descriptor_key(&signers[1], 0, &secp),
                    descriptor_key(&signers[2], 0, &secp),
                ],
            );
            let recovery = PathInfo::Single(descriptor_key(&signers[2], 1, &secp));
            let descriptor = CoincubeDescriptor::new(
                CoincubePolicy::new_legacy(primary, [(46, recovery)].iter().cloned().collect())
                    .unwrap(),
            );
            let derived = descriptor.receive_descriptor().derive(3.into(), &secp);
            let output = TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: derived.script_pubkey(),
            };
            let previous_tx = Transaction {
                version: transaction::Version::TWO,
                lock_time: absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::null(),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence(7),
                    witness: btc::Witness::new(),
                }],
                output: vec![output.clone()],
            };
            let unsigned_tx = Transaction {
                version: transaction::Version::TWO,
                lock_time: absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint {
                        txid: previous_tx.compute_txid(),
                        vout: 0,
                    },
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: btc::Witness::new(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(40_000),
                    script_pubkey: ScriptBuf::new_p2wsh(&ScriptBuf::new().wscript_hash()),
                }],
            };
            let mut psbt = Psbt::from_unsigned_tx(unsigned_tx).unwrap();
            derived.update_psbt_in(&mut psbt.inputs[0]);
            psbt.inputs[0].non_witness_utxo = Some(previous_tx);
            psbt.inputs[0].witness_utxo = Some(output);
            (signers, descriptor, psbt)
        }

        #[test]
        fn imported_blake2b_ordinary_spend_commands_refuse_before_side_effects() {
            let (signers, descriptor, psbt) = vault_fixture();
            let secp = secp256k1::Secp256k1::new();
            let key = descriptor_key(&signers[0], 0, &secp);
            let imported = coincube_core::wallet_descriptor::ImportedWalletDescriptor::parse(
                &format!("wpkh({key})"),
                None,
                ChainId::Bitcoin,
            )
            .unwrap()
            .paired_blake2b()
            .unwrap();
            let mut control = blake2b_control(descriptor);
            control.config.main_descriptor = WalletDescriptor::Imported(imported);
            let backend = std::sync::Arc::new(std::sync::Mutex::new(DummyBitcoind::new()));
            control.bitcoin = backend.clone();
            let txid = psbt.unsigned_tx.compute_txid();
            let refused = |result: Result<(), CommandError>| {
                assert!(
                    matches!(result, Err(CommandError::ImportedSpendValidation(message)) if message.contains("not available yet"))
                )
            };
            // No coins, no stored spend and an unsigned PSBT still reach the
            // policy refusal first, before ordinary wallet state is consulted.
            assert!(matches!(
                control.create_spend(&HashMap::new(), &[], 1, None),
                Err(CommandError::ImportedSpendValidation(_))
            ));
            refused(control.update_spend(psbt.clone()));
            refused(control.broadcast_spend(&txid));
            assert!(matches!(
                control.rbf_psbt(&txid, false, Some(1)),
                Err(CommandError::ImportedSpendValidation(_))
            ));
            assert!(control
                .db()
                .lock()
                .unwrap()
                .connection()
                .spend_tx(&txid)
                .is_none());
            assert!(backend
                .lock()
                .unwrap()
                .broadcasted
                .lock()
                .unwrap()
                .is_empty());
            assert!(psbt
                .inputs
                .iter()
                .all(|input| input.partial_sigs.is_empty()));
        }

        fn unified(psbt: &Psbt, signer: &MasterSigner) -> Psbt {
            let secp = secp256k1::Secp256k1::new();
            let u = UnifiedPsbt::from_psbt(psbt.clone()).unwrap();
            sign_p2wsh_all_unified(signer, &u, &secp)
                .unwrap()
                .psbt()
                .clone()
        }

        fn legacy(psbt: &Psbt, signer: &MasterSigner) -> Psbt {
            let secp = secp256k1::Secp256k1::new();
            signer.sign_psbt(psbt.clone(), &secp).unwrap()
        }

        fn verify_only() -> secp256k1::Secp256k1<secp256k1::VerifyOnly> {
            secp256k1::Secp256k1::verification_only()
        }

        fn sig_elements(witness: &btc::Witness) -> (usize, usize) {
            let (mut u, mut l) = (0, 0);
            for e in witness.iter() {
                if e.len() > 8 && e[0] == 0x30 {
                    match e.last() {
                        Some(0x21) => u += 1,
                        Some(0x01) => l += 1,
                        other => panic!("unexpected sighash byte {:?}", other),
                    }
                }
            }
            (u, l)
        }

        /// The boundary check must not tighten what the daemon accepts for
        /// its **own** spends: a PSBT `create_spend` produces on a Blake2b
        /// control — zero signatures, then a legacy signature, then a unified
        /// one from the Vault's own keys — is accepted and stored at every
        /// step (`validate_inputs` needs an authenticated prevout, a P2WSH
        /// prevout and a committing witness script on every input, and a
        /// daemon-created spend carries all three).
        #[test]
        fn blake2b_update_spend_accepts_the_daemons_own_unsigned_and_partial_spends() {
            let (signers, descriptor, _) = vault_fixture();
            let control = blake2b_control(descriptor);
            let secp = secp256k1::Secp256k1::new();
            let funding = funding_tx(&control, &[(0, 100_000, 3, false)]);
            let outpoint = bitcoin::OutPoint::new(funding.compute_txid(), 0);
            {
                let mut db = control.db().lock().unwrap().connection();
                db.new_txs(std::slice::from_ref(&funding));
                db.new_unspent_coins(&[Coin {
                    outpoint,
                    is_immature: false,
                    block_info: Some(BlockInfo { height: 1, time: 1 }),
                    amount: bitcoin::Amount::from_sat(100_000),
                    derivation_index: bip32::ChildNumber::from(3),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            }
            let destination =
                bitcoin::Address::from_str("bc1qnsexk3gnuyayu92fc3tczvc7k62u22a22ua2kv").unwrap();
            let destinations: HashMap<bitcoin::Address<address::NetworkUnchecked>, u64> =
                HashMap::from([(destination, 10_000u64)]);
            let created = match control.create_spend(&destinations, &[outpoint], 1, None) {
                Ok(CreateSpendResult::Success { psbt, .. }) => psbt,
                other => panic!("expected a created spend, got {:?}", other),
            };
            let txid = created.unsigned_tx.compute_txid();
            let stored =
                |control: &DaemonControl| control.db().lock().unwrap().connection().spend_tx(&txid);

            // Zero signatures: accepted and stored byte for byte.
            control.update_spend(created.clone()).unwrap();
            assert_eq!(stored(&control).unwrap().serialize(), created.serialize());
            // A legacy signature from one Vault key: merged.
            let legacy_signed = signers[1].sign_psbt(created.clone(), &secp).unwrap();
            assert!(!legacy_signed.inputs[0].partial_sigs.is_empty());
            control.update_spend(legacy_signed).unwrap();
            assert_eq!(stored(&control).unwrap().inputs[0].partial_sigs.len(), 1);
            // A unified signature from another Vault key: merged alongside.
            let unified_signed = coincube_core::unified_signing::sign_p2wsh_all_unified(
                &signers[0],
                &UnifiedPsbt::from_psbt(created.clone()).unwrap(),
                &secp,
            )
            .unwrap();
            control.update_spend(unified_signed.psbt().clone()).unwrap();
            let row = stored(&control).unwrap();
            assert_eq!(row.inputs[0].partial_sigs.len(), 1);
            assert_eq!(
                unified_signatures(&UnifiedPsbt::from_psbt(row.clone()).unwrap())
                    .unwrap()
                    .len(),
                1
            );
            // And the stored spend finalises: the boundary accepted exactly
            // what the finaliser accepts.
            assert!(finalize_spend_for_chain(ChainId::BitcoinBlake2b, row, &verify_only()).is_ok());
        }

        /// Everything `updatespend` accepted at 7d35edac is still accepted:
        /// the public contract never required `non_witness_utxo`, and the
        /// verifier does. `update_spend` backfills it from the wallet before
        /// verifying — first insert and merge — and stores the backfilled,
        /// verified PSBT. A `witness_utxo`-only PSBT with a committing
        /// `witness_script` is accepted unsigned, as a legacy delta, as a
        /// unified delta, and the stored row finalises. A `witness_script`
        /// stays required (the backfill cannot provide it). An input whose
        /// previous transaction the wallet does not hold is refused with a
        /// typed error — the one shape that changed.
        #[test]
        fn blake2b_update_spend_accepts_witness_utxo_only_psbts_by_backfilling() {
            let (signers, descriptor, full) = vault_fixture();
            let control = blake2b_control(descriptor);
            let txid = full.unsigned_tx.compute_txid();
            let outpoint = full.unsigned_tx.input[0].previous_output;
            let funding = full.inputs[0].non_witness_utxo.clone().unwrap();
            let strip = |psbt: &Psbt| {
                let mut stripped = psbt.clone();
                stripped.inputs[0].non_witness_utxo = None;
                assert!(stripped.inputs[0].witness_utxo.is_some());
                assert!(stripped.inputs[0].witness_script.is_some());
                stripped
            };
            let stored =
                |control: &DaemonControl| control.db().lock().unwrap().connection().spend_tx(&txid);

            // The wallet does not hold the funding transaction yet: refused,
            // typed, nothing stored.
            {
                let mut db = control.db().lock().unwrap().connection();
                db.new_unspent_coins(&[Coin {
                    outpoint,
                    is_immature: false,
                    block_info: None,
                    amount: bitcoin::Amount::from_sat(50_000),
                    derivation_index: bip32::ChildNumber::from(3),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            }
            assert!(matches!(
                control.update_spend(strip(&full)),
                Err(CommandError::SpendMissingPreviousTransaction(op)) if op == outpoint
            ));
            assert!(stored(&control).is_none());

            // With the funding transaction in the wallet, as the poller
            // stores it for every wallet coin: accepted, and the row is the
            // backfilled PSBT — the value verified is the value stored.
            control
                .db()
                .lock()
                .unwrap()
                .connection()
                .new_txs(std::slice::from_ref(&funding));
            control.update_spend(strip(&full)).unwrap();
            let row = stored(&control).unwrap();
            assert_eq!(row.inputs[0].non_witness_utxo.as_ref(), Some(&funding));
            assert_eq!(row.serialize(), full.serialize());

            // A witness_utxo-only legacy delta, then a witness_utxo-only
            // unified delta: both merge, and the stored row finalises.
            control
                .update_spend(strip(&legacy(&full, &signers[1])))
                .unwrap();
            assert_eq!(stored(&control).unwrap().inputs[0].partial_sigs.len(), 1);
            control
                .update_spend(strip(&unified(&full, &signers[0])))
                .unwrap();
            let row = stored(&control).unwrap();
            assert_eq!(
                unified_signatures(&UnifiedPsbt::from_psbt(row.clone()).unwrap())
                    .unwrap()
                    .len(),
                1
            );
            assert!(finalize_spend_for_chain(ChainId::BitcoinBlake2b, row, &verify_only()).is_ok());

            // The witness script stays required: the backfill does not
            // provide it.
            let mut no_script = strip(&full);
            no_script.inputs[0].witness_script = None;
            let other = blake2b_control(vault_fixture().1);
            {
                let mut db = other.db().lock().unwrap().connection();
                db.new_txs(std::slice::from_ref(&funding));
                db.new_unspent_coins(&[Coin {
                    outpoint,
                    is_immature: false,
                    block_info: None,
                    amount: bitcoin::Amount::from_sat(50_000),
                    derivation_index: bip32::ChildNumber::from(3),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            }
            assert!(matches!(
                other.update_spend(no_script),
                Err(CommandError::UnifiedSpendValidation(_))
            ));
        }

        /// Gandalf's probes from the review of 1f0c4e07 (WORK_LOGS/LAUNCH_GA/B1/
        /// B1.2/GANDALF_1F0C4E07/PROBES.patch), kept verbatim as regressions:
        /// they assert the fixed behaviour and failed on that head.
        #[test]
        fn gandalf_probe_merged_psbt_is_verified_before_storage() {
            let (signers, descriptor, psbt) = vault_fixture();
            let control = blake2b_control(descriptor);
            let txid = psbt.unsigned_tx.compute_txid();
            let outpoint = psbt.unsigned_tx.input[0].previous_output;
            control
                .db()
                .lock()
                .unwrap()
                .connection()
                .new_unspent_coins(&[Coin {
                    outpoint,
                    is_immature: false,
                    block_info: None,
                    amount: Amount::from_sat(50_000),
                    derivation_index: bip32::ChildNumber::from(3),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            let mut initial = psbt.clone();
            initial.inputs[0].sighash_type = Some(EcdsaSighashType::All.into());
            let verify = |raw: &Psbt| {
                coincube_core::unified_finalize::verify_all_signatures(
                    &UnifiedPsbt::from_psbt(raw.clone()).unwrap(),
                    &verify_only(),
                )
            };
            assert!(
                verify(&initial).is_ok(),
                "the saved unsigned request is supported"
            );
            control.update_spend(initial.clone()).unwrap();
            let incoming = unified(&psbt, &signers[0]);
            assert_eq!(incoming.unsigned_tx, initial.unsigned_tx);
            assert!(
                verify(&incoming).is_ok(),
                "the incoming signed PSBT is valid on its own"
            );
            let result = control.update_spend(incoming.clone());
            let row = control
                .db()
                .lock()
                .unwrap()
                .connection()
                .spend_tx(&txid)
                .unwrap();
            let merged_check = verify(&row);
            let retry = control.update_spend(incoming);
            let row_after_retry = control
                .db()
                .lock()
                .unwrap()
                .connection()
                .spend_tx(&txid)
                .unwrap();
            eprintln!(
                "GANDALF merged request: accepted={} mutated={} merged_check={:?} retry={:?} retry_valid={}",
                result.is_ok(),
                row.serialize() != initial.serialize(),
                merged_check,
                retry,
                verify(&row_after_retry).is_ok()
            );
            assert!(
                result.is_err() || merged_check.is_ok(),
                "update_spend must refuse atomically or store a merged PSBT its own verifier accepts"
            );
            if result.is_err() {
                assert_eq!(
                    row.serialize(),
                    initial.serialize(),
                    "refusal must preserve the stored row"
                );
            }
        }

        #[test]
        fn gandalf_probe_daemon_refuses_taproot_signature_data() {
            let mut outcomes = Vec::new();
            for script_path in [false, true] {
                let (signers, descriptor, psbt) = vault_fixture();
                let control = blake2b_control(descriptor);
                let txid = psbt.unsigned_tx.compute_txid();
                let outpoint = psbt.unsigned_tx.input[0].previous_output;
                control
                    .db()
                    .lock()
                    .unwrap()
                    .connection()
                    .new_unspent_coins(&[Coin {
                        outpoint,
                        is_immature: false,
                        block_info: None,
                        amount: Amount::from_sat(50_000),
                        derivation_index: bip32::ChildNumber::from(3),
                        is_change: false,
                        spend_txid: None,
                        spend_block: None,
                        is_from_self: false,
                    }]);
                let good = legacy(&legacy(&psbt, &signers[0]), &signers[1]);
                assert!(finalize_spend_for_chain(
                    ChainId::BitcoinBlake2b,
                    good.clone(),
                    &verify_only()
                )
                .is_ok());
                let mut bad = good;
                let secp = secp256k1::Secp256k1::new();
                let secret = secp256k1::SecretKey::from_slice(&[5u8; 32]).unwrap();
                let keypair = secp256k1::Keypair::from_secret_key(&secp, &secret);
                let (xonly, _) = btc::XOnlyPublicKey::from_keypair(&keypair);
                let signature = btc::taproot::Signature {
                    signature: secp.sign_schnorr_no_aux_rand(
                        &secp256k1::Message::from_digest([4u8; 32]),
                        &keypair,
                    ),
                    sighash_type: btc::TapSighashType::AllPlusAnyoneCanPay,
                };
                if script_path {
                    bad.inputs[0].tap_script_sigs.insert(
                        (
                            xonly,
                            btc::TapLeafHash::from_script(
                                &ScriptBuf::new(),
                                btc::taproot::LeafVersion::TapScript,
                            ),
                        ),
                        signature,
                    );
                } else {
                    bad.inputs[0].tap_key_sig = Some(signature);
                }
                let store = control.update_spend(bad.clone());
                let row = control.db().lock().unwrap().connection().spend_tx(&txid);
                let finalize =
                    finalize_spend_for_chain(ChainId::BitcoinBlake2b, bad, &verify_only());
                eprintln!(
                    "GANDALF Taproot field: script_path={} stored={} row_exists={} finalized={}",
                    script_path,
                    store.is_ok(),
                    row.is_some(),
                    finalize.is_ok()
                );
                outcomes.push((store.is_err(), row.is_none(), finalize.is_err()));
            }
            assert_eq!(
                outcomes,
                vec![(true, true, true); 2],
                "BTCB2 P2WSH boundaries must reject unsupported Taproot signature data, including ACP"
            );
        }

        /// Gandalf's probes from the review of 7d35edac (WORK_LOGS/LAUNCH_GA/B1/
        /// B1.2/GANDALF_7D35EDAC/PROBES.patch), kept verbatim as regressions:
        /// they assert the fixed behaviour and failed on that head.
        fn gandalf_probe_poisoned_signature(kind: u8) {
            let (signers, descriptor, psbt) = vault_fixture();
            let control = blake2b_control(descriptor);
            let txid = psbt.unsigned_tx.compute_txid();
            let outpoint = psbt.unsigned_tx.input[0].previous_output;
            {
                let mut db = control.db().lock().unwrap().connection();
                db.new_unspent_coins(&[Coin {
                    outpoint,
                    is_immature: false,
                    block_info: None,
                    amount: bitcoin::Amount::from_sat(50_000),
                    derivation_index: bip32::ChildNumber::from(3),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            }
            let stored =
                |control: &DaemonControl| control.db().lock().unwrap().connection().spend_tx(&txid);

            let good = if kind == 2 {
                unified(&psbt, &signers[0])
            } else {
                legacy(&psbt, &signers[0])
            };
            let mut bad = good.clone();
            if kind == 0 {
                for sig in bad.inputs[0].partial_sigs.values_mut() {
                    sig.sighash_type = btc::sighash::EcdsaSighashType::AllPlusAnyoneCanPay;
                }
            } else {
                let mut other_tx = psbt.clone();
                other_tx.unsigned_tx.output[0].value = btc::Amount::from_sat(40_001);
                assert_ne!(other_tx.unsigned_tx, psbt.unsigned_tx);
                if kind == 1 {
                    bad.inputs[0].partial_sigs = legacy(&other_tx, &signers[0]).inputs[0]
                        .partial_sigs
                        .clone();
                } else {
                    bad.inputs[0].proprietary = unified(&other_tx, &signers[0]).inputs[0]
                        .proprietary
                        .clone();
                }
            }
            // Premise, adapted for kind 0 only: since fold 1 of this repair
            // the adapter refuses a legacy ANYONECANPAY *flag* on its own
            // (`UnsupportedLegacySighash`), so "representation accepts it"
            // is no longer true for that kind — the wrong-digest kinds still
            // pass the representation and only the signature check catches
            // them. The acceptance assertion below is unchanged.
            use coincube_core::psbt_unified::UnifiedPsbtError as A;
            use coincube_core::unified_finalize::UnifiedFinalizeError as F;
            use coincube_core::unified_signing::UnifiedSigningError as S;
            if kind == 0 {
                assert!(
                    matches!(
                        UnifiedPsbt::from_psbt(bad.clone()),
                        Err(A::UnsupportedLegacySighash { .. })
                    ),
                    "the representation refuses the flag since fold 1"
                );
            } else {
                assert!(
                    UnifiedPsbt::from_psbt(bad.clone()).is_ok(),
                    "representation accepts it"
                );
                let err = coincube_core::unified_finalize::finalize_p2wsh_all_unified(
                    &UnifiedPsbt::from_psbt(bad.clone()).unwrap(),
                    &verify_only(),
                )
                .unwrap_err();
                match kind {
                    1 => assert!(matches!(err, F::InvalidLegacySignature { .. }), "{:?}", err),
                    _ => assert!(
                        matches!(err, F::Signing(S::InvalidUnifiedSignature { .. })),
                        "{:?}",
                        err
                    ),
                }
            }
            let first = control.update_spend(bad);
            let row = stored(&control);
            let correction = control.update_spend(good);
            assert_eq!(
                (first.is_err(), row.is_none(), correction.is_ok()),
                (true, true, true),
                "must refuse invalid first insert, keep DB empty and accept later valid signature; first={:?}, correction={:?}",
                first,
                correction
            );
        }

        #[test]
        fn gandalf_probe_first_insert_refuses_legacy_acp_signature() {
            gandalf_probe_poisoned_signature(0);
        }
        #[test]
        fn gandalf_probe_first_insert_refuses_wrong_digest_legacy_signature() {
            gandalf_probe_poisoned_signature(1);
        }
        #[test]
        fn gandalf_probe_first_insert_refuses_wrong_digest_unified_signature() {
            gandalf_probe_poisoned_signature(2);
        }

        /// `update_spend` on Bitcoin Blake2b, through the real control and the
        /// dummy database: a PSBT the chain's representation refuses — a
        /// malformed reserved record, a key with both encodings, an
        /// `ANYONECANPAY` request — is refused on the **first insert** with
        /// nothing stored, not only on a merge; a valid one is stored; and a
        /// later conflicting merge is refused with the stored row byte-unchanged.
        #[test]
        fn blake2b_update_spend_validates_before_the_first_insert_and_stores_nothing_on_refusal() {
            let (signers, descriptor, psbt) = vault_fixture();
            let control = blake2b_control(descriptor);
            let txid = psbt.unsigned_tx.compute_txid();
            let outpoint = psbt.unsigned_tx.input[0].previous_output;
            {
                let mut db = control.db().lock().unwrap().connection();
                db.new_unspent_coins(&[Coin {
                    outpoint,
                    is_immature: false,
                    block_info: None,
                    amount: bitcoin::Amount::from_sat(50_000),
                    derivation_index: bip32::ChildNumber::from(3),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            }
            let stored =
                |control: &DaemonControl| control.db().lock().unwrap().connection().spend_tx(&txid);

            // (a) A reserved record ending 0xa1.
            let mut malformed = unified(&psbt, &signers[0]);
            for value in malformed.inputs[0].proprietary.values_mut() {
                *value.last_mut().unwrap() = 0xa1;
            }
            assert!(matches!(
                control.update_spend(malformed),
                Err(CommandError::UnifiedSpendValidation(_))
            ));
            assert!(stored(&control).is_none(), "nothing stored on refusal");

            // (b) One key with both encodings.
            let mut both = unified(&psbt, &signers[0]);
            let legacy_same_key = legacy(&psbt, &signers[0]);
            both.inputs[0]
                .partial_sigs
                .extend(legacy_same_key.inputs[0].partial_sigs.clone());
            assert!(matches!(
                control.update_spend(both),
                Err(CommandError::UnifiedSpendValidation(_))
            ));
            assert!(stored(&control).is_none());

            // (c) A request for ANYONECANPAY next to valid legacy signatures.
            let mut asked = legacy(&legacy(&psbt, &signers[0]), &signers[1]);
            asked.inputs[0].sighash_type = Some(btc::psbt::PsbtSighashType::from_u32(0x81));
            match control.update_spend(asked) {
                Err(CommandError::UnifiedSpendValidation(reason)) => {
                    assert!(reason.contains("0x81"), "{}", reason)
                }
                other => panic!("expected the request to be refused, got {:?}", other),
            }
            assert!(stored(&control).is_none());

            // (d) Signatures that pass the adapter's *representation* checks
            //     but do not verify: an ANYONECANPAY legacy signature, a legacy
            //     signature for another transaction, a unified signature for
            //     another transaction. Each refused, nothing stored.
            let mut other_tx = psbt.clone();
            other_tx.unsigned_tx.output[0].value = btc::Amount::from_sat(40_001);
            let mut acp = legacy(&psbt, &signers[0]);
            for sig in acp.inputs[0].partial_sigs.values_mut() {
                sig.sighash_type = EcdsaSighashType::AllPlusAnyoneCanPay;
            }
            let mut wrong_legacy = psbt.clone();
            wrong_legacy.inputs[0].partial_sigs = legacy(&other_tx, &signers[0]).inputs[0]
                .partial_sigs
                .clone();
            let mut wrong_unified = psbt.clone();
            wrong_unified.inputs[0].proprietary = unified(&other_tx, &signers[0]).inputs[0]
                .proprietary
                .clone();
            for (name, bad, needle, representation_accepts) in [
                // The ANYONECANPAY flag is refused by the representation
                // itself since fold 1; the wrong-digest ones pass it and are
                // caught only by signature verification.
                ("legacy ANYONECANPAY", acp, "0x81", false),
                (
                    "legacy for another tx",
                    wrong_legacy,
                    "does not verify",
                    true,
                ),
                ("unified for another tx", wrong_unified, "unified", true),
            ] {
                assert_eq!(
                    UnifiedPsbt::from_psbt(bad.clone()).is_ok(),
                    representation_accepts,
                    "{}",
                    name
                );
                match control.update_spend(bad) {
                    Err(CommandError::UnifiedSpendValidation(reason)) => {
                        assert!(reason.contains(needle), "{}: {}", name, reason)
                    }
                    other => panic!("{}: expected refusal, got {:?}", name, other),
                }
                assert!(stored(&control).is_none(), "{}: nothing stored", name);
            }

            // Control: an unsigned spend is stored on first insert, byte for
            // byte…
            control.update_spend(psbt.clone()).unwrap();
            assert_eq!(stored(&control).unwrap().serialize(), psbt.serialize());
            // …a valid unified signature then merges into it (the adapter
            // merge carries the record, not the signer's 0x21 request)…
            let valid = unified(&psbt, &signers[0]);
            control.update_spend(valid.clone()).unwrap();
            let row = stored(&control).expect("stored");
            assert_eq!(
                unified_signatures(&UnifiedPsbt::from_psbt(row.clone()).unwrap()).unwrap(),
                unified_signatures(&UnifiedPsbt::from_psbt(valid.clone()).unwrap()).unwrap()
            );
            // …an invalid *update* leaves that row byte-identical…
            let mut wrong_update = psbt.clone();
            wrong_update.inputs[0].partial_sigs = legacy(&other_tx, &signers[1]).inputs[0]
                .partial_sigs
                .clone();
            assert!(matches!(
                control.update_spend(wrong_update),
                Err(CommandError::UnifiedSpendValidation(_))
            ));
            assert_eq!(stored(&control).unwrap().serialize(), row.serialize());
            // …a second signer merges into it (the correct signature for the
            // key whose invalid one was just refused)…
            control.update_spend(unified(&psbt, &signers[1])).unwrap();
            let merged = stored(&control).unwrap();
            assert_eq!(
                unified_signatures(&UnifiedPsbt::from_psbt(merged.clone()).unwrap())
                    .unwrap()
                    .len(),
                2
            );
            // …and a conflicting legacy signature for a key already held as
            // unified (both encodings) is refused with the row byte-unchanged.
            let mut conflicting = psbt.clone();
            conflicting.inputs[0]
                .partial_sigs
                .extend(legacy(&psbt, &signers[0]).inputs[0].partial_sigs.clone());
            assert!(matches!(
                control.update_spend(conflicting),
                Err(CommandError::UnifiedSignatureMerge(_))
            ));
            assert_eq!(stored(&control).unwrap().serialize(), merged.serialize());
        }

        #[test]
        fn blake2b_persistence_refuses_an_independent_legacy_alternative_atomically() {
            let (signers, descriptor, psbt) = vault_fixture();
            let control = blake2b_control(descriptor);
            let txid = psbt.unsigned_tx.compute_txid();
            let outpoint = psbt.unsigned_tx.input[0].previous_output;
            {
                let mut db = control.db().lock().unwrap().connection();
                db.new_unspent_coins(&[Coin {
                    outpoint,
                    is_immature: false,
                    block_info: None,
                    amount: bitcoin::Amount::from_sat(50_000),
                    derivation_index: bip32::ChildNumber::from(3),
                    is_change: false,
                    spend_txid: None,
                    spend_block: None,
                    is_from_self: false,
                }]);
            }
            let stored =
                |control: &DaemonControl| control.db().lock().unwrap().connection().spend_tx(&txid);

            let safe_mixed = legacy(&unified(&psbt, &signers[0]), &signers[1]);
            let unsafe_first = legacy(&safe_mixed, &signers[2]);
            assert!(matches!(
                control.update_spend(unsafe_first.clone()),
                Err(CommandError::UnsafeLegacyAlternative { input: 0, .. })
            ));
            assert!(
                stored(&control).is_none(),
                "unsafe first insert stored nothing"
            );

            control.update_spend(safe_mixed.clone()).unwrap();
            let before = stored(&control).expect("safe mixed witness stored");
            assert_eq!(before.serialize(), safe_mixed.serialize());
            assert!(matches!(
                control.update_spend(legacy(&psbt, &signers[2])),
                Err(CommandError::UnsafeLegacyAlternative { input: 0, .. })
            ));
            assert_eq!(
                stored(&control).unwrap().serialize(),
                before.serialize(),
                "unsafe merge leaves the row byte-identical"
            );

            assert!(matches!(
                finalize_spend_for_chain(ChainId::BitcoinBlake2b, unsafe_first, &verify_only()),
                Err(CommandError::UnsafeLegacyAlternative { input: 0, .. })
            ));
            let mixed =
                finalize_spend_for_chain(ChainId::BitcoinBlake2b, safe_mixed, &verify_only())
                    .expect("one legacy signature needed by a mixed witness remains valid");
            assert_eq!(sig_elements(&mixed.input[0].witness), (1, 1));
        }

        /// Gandalf's probe from the review of 15a26267 (WORK_LOGS/LAUNCH_GA/B1/
        /// B1.2/GANDALF_15a26267/PROBES.patch), kept as a regression: it
        /// asserts the fixed behaviour and failed on that head.
        #[test]
        fn gandalf_probe_legacy_conflict_is_atomic() {
            let (signers, psbt) = vault_psbt();
            let stored = legacy(&psbt, &signers[0]);
            let other = legacy(&psbt, &signers[1]);
            let key = *stored.inputs[0].partial_sigs.keys().next().unwrap();
            let bad_sig = *other.inputs[0].partial_sigs.values().next().unwrap();
            let mut incoming = stored.clone();
            incoming.inputs[0].partial_sigs.insert(key, bad_sig);
            let mut direct = UnifiedPsbt::from_psbt(stored.clone()).unwrap();
            let delta = UnifiedPsbt::from_psbt(incoming.clone()).unwrap();
            assert!(coincube_core::psbt_unified::merge_signatures(&mut direct, &delta).is_err());
            let merged =
                merge_spend_signatures(ChainId::BitcoinBlake2b, stored, &incoming, &verify_only());
            println!("GANDALF daemon conflict accepted: {}", merged.is_ok());
            assert!(
                merged.is_err(),
                "BTCB2 legacy conflict must be rejected before overwrite"
            );
        }

        #[test]
        fn bitcoin_merge_is_the_historical_copy_and_ignores_unified_records() {
            let (signers, psbt) = vault_psbt();
            let stored = legacy(&psbt, &signers[0]);
            let mut incoming = legacy(&psbt, &signers[1]);
            // A unified record riding along on a Bitcoin spend is not carried:
            // the Bitcoin arm copies partial/Taproot signatures and nothing else,
            // exactly as before this change.
            incoming = unified(&incoming, &signers[2]);
            assert!(!incoming.inputs[0].proprietary.is_empty());
            let merged =
                merge_spend_signatures(ChainId::Bitcoin, stored.clone(), &incoming, &verify_only())
                    .unwrap();
            assert_eq!(merged.inputs[0].partial_sigs.len(), 2);
            assert!(merged.inputs[0].proprietary.is_empty());
            assert_eq!(merged.unsigned_tx, stored.unsigned_tx);
        }

        #[test]
        fn blake2b_merge_carries_unified_records_and_refuses_conflicts() {
            let (signers, psbt) = vault_psbt();
            let stored = unified(&psbt, &signers[0]);
            let incoming = unified(&psbt, &signers[1]);
            let merged = merge_spend_signatures(
                ChainId::BitcoinBlake2b,
                stored.clone(),
                &incoming,
                &verify_only(),
            )
            .unwrap();
            let records =
                unified_signatures(&UnifiedPsbt::from_psbt(merged.clone()).unwrap()).unwrap();
            assert_eq!(records.len(), 2, "both signers' records are held");

            // A conflicting record for a key already stored is refused whole.
            let mut conflicting = stored.clone();
            let (key, value) = conflicting.inputs[0]
                .proprietary
                .iter()
                .next()
                .map(|(k, v)| (k.clone(), v.clone()))
                .unwrap();
            let mut other = value.clone();
            other[12] ^= 0x01;
            conflicting.inputs[0].proprietary.insert(key, other);
            assert!(matches!(
                merge_spend_signatures(
                    ChainId::BitcoinBlake2b,
                    stored.clone(),
                    &conflicting,
                    &verify_only()
                ),
                Err(CommandError::UnifiedSignatureMerge(_))
            ));
            // Legacy partial signatures still merge on the Blake2b arm too (signer
            // 2 holds a primary key and the recovery key, hence two of them).
            let incoming_legacy = legacy(&psbt, &signers[2]);
            let merged = merge_spend_signatures(
                ChainId::BitcoinBlake2b,
                stored,
                &incoming_legacy,
                &verify_only(),
            )
            .unwrap();
            assert_eq!(merged.inputs[0].partial_sigs.len(), 2);
            assert_eq!(merged.inputs[0].proprietary.len(), 1);
        }

        /// A conflicting **legacy** signature — same key, a different parseable
        /// signature — is refused on the Blake2b arm and the stored PSBT is
        /// byte-unchanged. The historical copy is last-write-wins, so running it
        /// before the adapter would have replaced the valid stored signature and
        /// left the adapter comparing the incoming value with itself.
        #[test]
        fn blake2b_merge_refuses_a_conflicting_legacy_signature_without_touching_the_stored_psbt() {
            let (signers, psbt) = vault_psbt();
            let stored = legacy(&psbt, &signers[0]);
            let stored_bytes = stored.serialize();
            let (key_a, _) = stored.inputs[0]
                .partial_sigs
                .iter()
                .next()
                .map(|(k, s)| (*k, *s))
                .unwrap();
            // Key B's signature, assigned to key A: well formed, wrong.
            let other = legacy(&psbt, &signers[1]);
            let sig_b = *other.inputs[0].partial_sigs.values().next().unwrap();
            let mut conflicting = psbt.clone();
            conflicting.inputs[0].partial_sigs.insert(key_a, sig_b);

            let refused = merge_spend_signatures(
                ChainId::BitcoinBlake2b,
                stored.clone(),
                &conflicting,
                &verify_only(),
            );
            match refused {
                Err(CommandError::UnifiedSignatureMerge(reason)) => {
                    assert!(reason.to_lowercase().contains("conflict"), "{}", reason)
                }
                other => panic!("expected the conflict to be refused, got {:?}", other),
            }
            assert_eq!(
                stored.serialize(),
                stored_bytes,
                "the stored PSBT is untouched"
            );

            // The Bitcoin arm keeps its historical last-write-wins behaviour.
            let merged = merge_spend_signatures(
                ChainId::Bitcoin,
                stored.clone(),
                &conflicting,
                &verify_only(),
            )
            .unwrap();
            assert_eq!(merged.inputs[0].partial_sigs[&key_a], sig_b);

            // And through `update_spend` on the daemon: refusal stores nothing.
            // (Chain-keyed on the control's config, exercised on the pure
            // function above; the store happens only after `?` succeeds.)
            let same_stored = merge_spend_signatures(
                ChainId::BitcoinBlake2b,
                stored.clone(),
                &stored,
                &verify_only(),
            )
            .unwrap();
            assert_eq!(same_stored.serialize(), stored_bytes, "idempotent re-merge");
        }

        #[test]
        fn bitcoin_finalisation_is_miniscripts_and_cannot_see_unified_records() {
            let (signers, psbt) = vault_psbt();
            let signed = legacy(&legacy(&psbt, &signers[0]), &signers[1]);
            let secp = verify_only();
            let tx = finalize_spend_for_chain(ChainId::Bitcoin, signed.clone(), &secp).unwrap();
            // Same bytes rust-miniscript's finaliser produces on its own.
            let mut expected = signed.clone();
            expected.finalize_mut(&secp).unwrap();
            assert_eq!(tx, expected.extract_tx_unchecked_fee_rate());
            assert_eq!(sig_elements(&tx.input[0].witness), (0, 2));

            // Unified-only on the Bitcoin arm: miniscript's finaliser does not
            // see the records, so this fails as it always did — the Bitcoin path
            // gains no Blake2b behaviour.
            let unified_only = unified(&unified(&psbt, &signers[0]), &signers[1]);
            assert!(matches!(
                finalize_spend_for_chain(ChainId::Bitcoin, unified_only, &secp),
                Err(CommandError::SpendFinalization(_))
            ));
        }

        #[test]
        fn blake2b_finalisation_assembles_unified_witnesses_and_refuses_anyonecanpay() {
            let (signers, psbt) = vault_psbt();
            let secp = verify_only();
            let signed = unified(&unified(&psbt, &signers[0]), &signers[1]);
            let tx =
                finalize_spend_for_chain(ChainId::BitcoinBlake2b, signed.clone(), &secp).unwrap();
            assert_eq!(sig_elements(&tx.input[0].witness), (2, 0));
            let mut stripped = tx.clone();
            stripped.input[0].witness = btc::Witness::new();
            assert_eq!(stripped, signed.unsigned_tx);

            // Legacy-only still finalises (replayable; the GUI gates that), and
            // the unified + legacy mix keeps the unified signature.
            let legacy_only = legacy(&legacy(&psbt, &signers[0]), &signers[1]);
            let tx = finalize_spend_for_chain(ChainId::BitcoinBlake2b, legacy_only, &secp).unwrap();
            assert_eq!(sig_elements(&tx.input[0].witness), (0, 2));
            let mixed = legacy(&unified(&psbt, &signers[0]), &signers[1]);
            let tx = finalize_spend_for_chain(ChainId::BitcoinBlake2b, mixed, &secp).unwrap();
            assert_eq!(sig_elements(&tx.input[0].witness), (1, 1));

            // ANYONECANPAY is refused, not warned about.
            let mut acp = legacy(&unified(&psbt, &signers[0]), &signers[1]);
            let (key, sig) = acp.inputs[0]
                .partial_sigs
                .iter()
                .next()
                .map(|(k, s)| (*k, *s))
                .unwrap();
            acp.inputs[0].partial_sigs.insert(
                key,
                ecdsa::Signature {
                    signature: sig.signature,
                    sighash_type: EcdsaSighashType::AllPlusAnyoneCanPay,
                },
            );
            match finalize_spend_for_chain(ChainId::BitcoinBlake2b, acp, &secp) {
                Err(CommandError::UnifiedSpendFinalization(msg)) => {
                    assert!(msg.contains("0x81"), "{}", msg)
                }
                other => panic!("expected the ANYONECANPAY refusal, got {:?}", other),
            }
            // …and so is an input that merely *asks* for it, with valid
            // SIGHASH_ALL signatures on board (an RPC or imported PSBT is
            // this boundary's concern, not the GUI's dispatch check).
            let mut asked = legacy(&legacy(&psbt, &signers[0]), &signers[1]);
            asked.inputs[0].sighash_type = Some(btc::psbt::PsbtSighashType::from_u32(0x81));
            match finalize_spend_for_chain(ChainId::BitcoinBlake2b, asked, &secp) {
                Err(CommandError::UnifiedSpendFinalization(msg)) => {
                    assert!(msg.contains("0x81") && msg.contains("asks"), "{}", msg)
                }
                other => panic!("expected the requested-sighash refusal, got {:?}", other),
            }
        }
    }
}
