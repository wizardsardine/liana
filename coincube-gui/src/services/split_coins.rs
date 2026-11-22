//! One-sided splits from Coin control (`decisions/2026-10-29-coincube-desktop-
//! btcb2-split-protection-in-coin-control.md`).
//!
//! A split is an ordinary self-send of the selected both-chain coins back to
//! a fresh change address in the same Vault, made so the twin chain rejects
//! it:
//!
//! - [`Method::SplitKey`] adds one coin proven to exist only on this chain
//!   (the split key). The twin chain has no such coin, so the transaction is
//!   invalid there. The key's proof is re-qualified against both canonical
//!   chains immediately before the transaction is built.
//! - [`Method::Signature`] (Bitcoin Blake2b only) relies on the Vault's
//!   replay-protected signature, which Bitcoin rejects. The signing screen's
//!   replay check still has to pass before broadcast. A txid commits to no
//!   witness, and the Vault could have signed the same transaction the
//!   legacy way too, so the twin Cube counts it only from the witness its
//!   confirming block commits to: [`twin_confirmed`] fetches that block,
//!   authenticates it ([`coincube_core::btcb2_block`]) and checks every
//!   signature is a valid `ALL|UNIFIED` one
//!   ([`coincube_core::confirmed_unified`]; decision open item 1). Its own
//!   outputs past the threshold still show *Split · unconfirmed*: the Vault
//!   could yet confirm a legacy copy on Bitcoin, so they are not proven
//!   one-chain coins.
//!
//! Either way one side is enough: once the split has depth, neither copy can
//! be replayed onto the other chain. The transaction is handed to the Vault's
//! normal PSBT screen for signing and broadcast, and recorded in a small
//! journal ([`journal`]) from which the Chains column derives *Splitting*
//! and candidate proofs for the split outputs, which a fresh ancestry check
//! must still verify. The twin Cube's journal only nominates its splits: a
//! coin counts as separated by one only once Connect shows that split
//! [`SPLIT_CONFIRMATIONS`] deep on the twin chain now and its confirmed
//! transaction spends the coin (and, for a signature split, carries only
//! unified signatures; [`twin_confirmed`]).

pub mod journal;

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use coincube_core::{
    claim_ancestry::{retained::RetainedPath, search::OwnedLink},
    descriptors::CoincubeDescriptor,
    miniscript::bitcoin::{
        consensus::serialize, psbt::Psbt, BlockHash, OutPoint, Transaction, Txid,
    },
    wallet_descriptor::{SpendAnalysis, WalletDescriptor},
};
use coincubed::commands::CoinStatus;
use serde::{Deserialize, Serialize};

use crate::{
    app::wallet::Wallet,
    chain::ChainId,
    daemon::{
        model::{Coin, CreateSpendResult},
        Daemon,
    },
    services::{
        chain_status::{ProofEntry, SPLIT_CONFIRMATIONS},
        claim_observation::{ObservationSource, TransactionObservation},
    },
};

/// How the split is made rejectable on the twin chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    SplitKey,
    Signature,
}

/// One split this Cube built. The Chains column is derived from it; nothing
/// here authorizes signing or broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitRecord {
    pub txid: Txid,
    pub chain: ChainId,
    pub method: Method,
    /// The both-chain coins being split, without the key.
    pub inputs: Vec<OutPoint>,
    /// The split key, for [`Method::SplitKey`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<OutPoint>,
    /// The key's [`RetainedPath::encode`], hex, so the split's outputs can be
    /// proven by extending it by one link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    /// Unix seconds.
    pub created_at: u64,
    /// The split's confirmations when this Cube last looked: 0 while it is
    /// unconfirmed or after a reorg drops it. Display state only; the twin
    /// Cube never trusts it (see [`twin_confirmed`]).
    #[serde(default)]
    pub confirmations: u32,
}

/// A record whose transaction never showed up is dropped after this long.
pub const UNSEEN_RECORD_LIFETIME_SECONDS: u64 = 14 * 24 * 60 * 60;

/// The split key a split is built with.
#[derive(Debug, Clone)]
pub struct KeyChoice {
    pub outpoint: OutPoint,
    pub path: RetainedPath,
}

/// A built, stored, unsigned split ready for the PSBT screen.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub psbt: Psbt,
    pub record: SplitRecord,
}

/// What the Chains column learns from the journals.
#[derive(Debug, Default)]
pub struct Derived {
    /// Outputs of splits still short of [`SPLIT_CONFIRMATIONS`].
    pub splitting: HashMap<OutPoint, u32>,
    /// Outputs of signature splits past [`SPLIT_CONFIRMATIONS`]. Never
    /// proven: a legacy copy of the split could still confirm on Bitcoin.
    pub unconfirmed: HashSet<OutPoint>,
    /// Outputs of confirmed split-key splits with the key's extended path,
    /// as [`ProofEntry::Saved`]: the path is only structurally checked, so a
    /// fresh ancestry check must qualify it before it is shown as verified or
    /// used as a split key. Merge with [`merge_proofs`].
    pub proofs: Vec<(OutPoint, ProofEntry)>,
    /// Whether any record changed (its confirmations moved or it was pruned).
    pub changed: bool,
}

/// Derive the Chains column's split facts from this Cube's records, updating
/// each record's confirmation count and pruning records whose transaction
/// never appeared. `coins` are the Vault's coins, `tip` the daemon's tip and
/// `transactions` the raw split transactions the daemon knows about.
pub fn derive(
    records: &mut Vec<SplitRecord>,
    coins: &[Coin],
    tip: i32,
    transactions: &HashMap<Txid, Transaction>,
    now: SystemTime,
) -> Derived {
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut derived = Derived::default();
    let before = records.len();
    records.retain(|record| {
        coins.iter().any(|coin| coin.outpoint.txid == record.txid)
            || now.saturating_sub(record.created_at) < UNSEEN_RECORD_LIFETIME_SECONDS
    });
    derived.changed |= records.len() != before;
    for record in records.iter_mut() {
        let outputs: Vec<&Coin> = coins
            .iter()
            .filter(|coin| coin.outpoint.txid == record.txid)
            .collect();
        let confirmations = split_confirmations(&outputs, tip);
        // Follow the chain both ways: a reorg that drops the split takes its
        // confirmations with it.
        if confirmations != record.confirmations {
            record.confirmations = confirmations;
            derived.changed = true;
        }
        if confirmations == 0 {
            for coin in &outputs {
                derived.splitting.insert(coin.outpoint, 0);
            }
            continue;
        }
        if confirmations < SPLIT_CONFIRMATIONS {
            for coin in &outputs {
                derived.splitting.insert(coin.outpoint, confirmations);
            }
            continue;
        }
        record_confirmed_split_outputs(&mut derived, record, &outputs, transactions);
    }
    derived
}

fn split_confirmations(outputs: &[&Coin], tip: i32) -> u32 {
    match outputs.iter().find_map(|coin| coin.block_height) {
        Some(height) if height >= 0 && height <= tip => (tip - height + 1) as u32,
        _ => 0,
    }
}

fn record_confirmed_split_outputs(
    derived: &mut Derived,
    record: &SplitRecord,
    outputs: &[&Coin],
    transactions: &HashMap<Txid, Transaction>,
) {
    match record.method {
        // Not a proof for these outputs: the Vault holds the inputs' keys
        // on Bitcoin too and could still confirm a legacy-signed copy of
        // this transaction there, which would put the outputs on both
        // chains. They are shown as split but unconfirmed, never proven.
        // The twin Cube's inputs are another matter: see `twin_confirmed`.
        Method::Signature => {
            derived
                .unconfirmed
                .extend(outputs.iter().map(|coin| coin.outpoint));
        }
        Method::SplitKey => {
            let Some(tx) = transactions.get(&record.txid) else {
                return;
            };
            for coin in outputs {
                if let Some(path) = extended_path(record, tx, coin.outpoint) {
                    derived
                        .proofs
                        .push((coin.outpoint, ProofEntry::Saved(path)));
                }
            }
        }
    }
}

/// The proof of a split-key split's output: the split transaction, linked
/// through the key's input, followed by the key's own path. Structurally
/// verified by [`RetainedPath::new`]; `None` if anything does not line up.
/// The key's path comes from the journal, so nothing here re-qualifies it
/// against the chains: [`derive`] offers it as [`ProofEntry::Saved`].
pub fn extended_path(
    record: &SplitRecord,
    tx: &Transaction,
    output: OutPoint,
) -> Option<RetainedPath> {
    let key = record.key?;
    if tx.compute_txid() != record.txid || output.txid != record.txid {
        return None;
    }
    let key_path = RetainedPath::decode(key, &hex::decode(record.key_path.as_ref()?).ok()?).ok()?;
    let parent_input = tx
        .input
        .iter()
        .position(|input| input.previous_output == key)?;
    let mut links = vec![OwnedLink {
        transaction: serialize(tx),
        parent_input: Some(parent_input),
    }];
    links.extend(key_path.links().iter().cloned());
    RetainedPath::new(output, links).ok()
}

/// Add [`Derived::proofs`] to the cache's ancestry results. A derived path
/// only fills a gap: it never replaces a check's own answer, verified or not,
/// so a coin a check found no path for keeps that answer until it is due
/// again. A check that could not finish is not an answer, so the path fills
/// in beside it ([`ProofEntry::SavedRetry`]). Returns whether anything was
/// added.
pub fn merge_proofs(
    chain_proofs: &mut HashMap<OutPoint, ProofEntry>,
    derived: Vec<(OutPoint, ProofEntry)>,
) -> bool {
    let mut added = false;
    for (outpoint, entry) in derived {
        match chain_proofs.entry(outpoint) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(entry);
                added = true;
            }
            // A check that couldn't finish is no answer: the derived path
            // fills in and keeps the failure's retry time.
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if let (ProofEntry::Failed { failed_at }, Some(path)) = (slot.get(), entry.path()) {
                    let failed_at = *failed_at;
                    slot.insert(ProofEntry::SavedRetry {
                        path: path.clone(),
                        failed_at,
                    });
                    added = true;
                }
            }
        }
    }
    added
}

/// At most this many of the twin Cube's splits of each method are looked up
/// per pass, so signature splits never crowd out split-key ones.
pub const MAX_TWIN_LOOKUPS: usize = 8;

/// A twin split's last fresh check stops counting after this long; by then
/// the round-robin in [`twin_candidates`] has normally checked it again.
pub const TWIN_CHECK_LIFETIME: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The last fresh check of one twin split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwinCheck {
    /// The inputs of the transaction Connect showed at least
    /// [`SPLIT_CONFIRMATIONS`] deep on the twin chain, read from its
    /// txid-authenticated bytes (for a signature split, from its block's,
    /// once its witness passed). `None` when it was shallower or absent, a
    /// signature split's witness did not pass, a read failed, or the lookup
    /// did not run.
    pub spent: Option<HashSet<OutPoint>>,
    /// When the pass that made the check started, before any of its reads,
    /// so a slow pass never stretches [`TWIN_CHECK_LIFETIME`].
    pub at: std::time::Instant,
}

/// Record the twin lookups of one pass, which started at `at`. A lookup
/// replaces a split's previous check only if that check is older, so a pass
/// that lands after a newer one still counts for the splits it looked up,
/// and a pass that did not look a split up leaves its check alone. Returns
/// whether any check changed.
pub fn record_twin_checks(
    checks: &mut HashMap<Txid, TwinCheck>,
    checked: Vec<(Txid, Option<HashSet<OutPoint>>)>,
    at: std::time::Instant,
) -> bool {
    let mut changed = false;
    for (txid, spent) in checked {
        if checks.get(&txid).is_some_and(|check| check.at >= at) {
            continue;
        }
        checks.insert(txid, TwinCheck { spent, at });
        changed = true;
    }
    changed
}

/// The twin Cube's splits to ask Connect about this pass, with how each was
/// made: never checked first, then the longest unchecked, newest first among
/// equals, up to [`MAX_TWIN_LOOKUPS`] of each method. Every record is reached
/// in turn however many there are. The twin journal's own confirmation count
/// is not consulted.
pub fn twin_candidates(
    twin_records: &[SplitRecord],
    checks: &HashMap<Txid, TwinCheck>,
) -> Vec<(Txid, Method)> {
    let mut records: Vec<&SplitRecord> = twin_records.iter().collect();
    records.sort_by_key(|record| {
        (
            checks.get(&record.txid).map(|check| check.at),
            std::cmp::Reverse(record.created_at),
        )
    });
    let mut seen = HashSet::new();
    let mut taken: HashMap<Method, usize> = HashMap::new();
    records
        .into_iter()
        .filter(|record| seen.insert(record.txid))
        .filter(|record| {
            let count = taken.entry(record.method).or_default();
            *count += 1;
            *count <= MAX_TWIN_LOOKUPS
        })
        .map(|record| (record.txid, record.method))
        .collect()
}

/// Coins of this Cube whose twin copy a split on the twin Cube has separated:
/// the inputs of the twin's splits whose last fresh check, within
/// [`TWIN_CHECK_LIFETIME`] of `now`, showed them [`SPLIT_CONFIRMATIONS`]
/// deep on the twin chain. Only inputs the journal lists *and* the confirmed
/// transaction spends count: the journal nominates, the chain decides. A
/// signature split has a check with inputs only once its confirmed witness
/// passed ([`twin_confirmed`]).
///
/// Each coin maps to the instant its check stops counting, so a reader can
/// drop it then even if no later pass lands ([`is_separated`]).
pub fn twin_separated(
    twin_records: &[SplitRecord],
    checks: &HashMap<Txid, TwinCheck>,
    now: std::time::Instant,
) -> HashMap<OutPoint, std::time::Instant> {
    let mut separated = HashMap::new();
    for record in twin_records {
        let Some(check) = checks.get(&record.txid) else {
            continue;
        };
        let Some(spent) = check.spent.as_ref() else {
            continue;
        };
        let until = check.at + TWIN_CHECK_LIFETIME;
        if now > until {
            continue;
        }
        for input in record.inputs.iter().filter(|input| spent.contains(input)) {
            let entry = separated.entry(*input).or_insert(until);
            *entry = (*entry).max(until);
        }
    }
    separated
}

/// Whether `outpoint` counts as separated at `now` by a [`twin_separated`]
/// result: its check has not expired yet.
pub fn is_separated(
    separated: &HashMap<OutPoint, std::time::Instant>,
    outpoint: &OutPoint,
    now: std::time::Instant,
) -> bool {
    separated.get(outpoint).is_some_and(|until| now <= *until)
}

/// Upper bound, in vbytes, of a split spending `shared_count` both-chain
/// coins plus one more input (the split key, or for a signature split an
/// overestimate by one input) to one change output of `descriptor`'s wallet:
/// a Vault, or an imported wallet of any supported shape. A funding estimate
/// only, not authorization to construct, sign or broadcast anything. `None`
/// for a Taproot Vault, no coins, or more inputs than the estimate covers.
/// Moved from the removed Claim engine
/// (`coincube_core::claim_spend::estimate_ancestry_self_transfer_vbytes`).
pub fn estimate_split_vbytes(descriptor: &WalletDescriptor, shared_count: usize) -> Option<u64> {
    use coincube_core::miniscript::bitcoin::{
        absolute::LockTime, transaction, Amount, TxIn, TxOut,
    };
    if descriptor
        .as_vault()
        .is_some_and(CoincubeDescriptor::is_taproot)
        || shared_count == 0
    {
        return None;
    }
    let input_count = shared_count
        .checked_add(1)
        .filter(|count| *count <= 20_001)?;
    let change_script = descriptor.change_descriptor().script_pubkey(0).ok()?;
    let tx = Transaction {
        version: transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn::default(); input_count],
        output: vec![TxOut {
            value: Amount::ZERO,
            script_pubkey: change_script,
        }],
    };
    Some(descriptor.spend_max_vbytes(&tx, true))
}

/// Whether this Vault can split with a replay-protected signature: Bitcoin
/// Blake2b, and every primary-path signer either signs in process or is a
/// recorded Keychain key. Any other signer is treated as a hardware device,
/// which can't make the signature (decision item 3). Never for an imported
/// Vault: its signers (hardware, the original wallet, words stored in the
/// Cube) make ordinary signatures only, so it splits with a split key.
pub fn signature_available(wallet: &Wallet) -> bool {
    if !wallet.chain.is_blake2b() {
        return false;
    }
    let Some(descriptor) = wallet.inheritance_descriptor() else {
        return false;
    };
    let mut in_process: HashSet<_> = wallet.border_wallet_fingerprints.iter().copied().collect();
    if let Some(signer) = &wallet.signer {
        in_process.insert(signer.fingerprint());
    }
    descriptor
        .policy()
        .primary_path()
        .thresh_origins()
        .1
        .keys()
        .all(|fingerprint| {
            in_process.contains(fingerprint)
                || (wallet.keychain_keys_recorded
                    && wallet.keychain_key_ids.contains_key(fingerprint))
        })
}

/// The split's fee rate: the six-block estimate of Connect's Esplora for
/// this chain only, failing closed (`services::split_fees`). Never another
/// chain's estimate and never a local guess.
pub async fn fee_rate(
    client: Option<crate::services::coincube::CoincubeClient>,
    chain: ChainId,
) -> Option<u64> {
    use crate::services::split_fees::{self, SweepFeeSource};
    let client = client?;
    let rate = match chain {
        ChainId::Bitcoin => {
            split_fees::ConnectBitcoinFees::new(&client)?
                .mid_priority_sat_vb()
                .await
        }
        ChainId::BitcoinBlake2b => {
            split_fees::ConnectBtcb2Fees::new(client)?
                .mid_priority_sat_vb()
                .await
        }
        _ => None,
    }?;
    (1..=coincube_core::spend::MAX_FEERATE)
        .contains(&rate)
        .then_some(rate)
}

/// Whether a refresh pass numbered `seq` may be applied after the pass
/// numbered `applied`: only a newer one. A slow pass that lands after a newer
/// one would otherwise put back facts the newer one had cleared.
pub fn newer_pass(applied: &mut u64, seq: u64) -> bool {
    if seq <= *applied {
        return false;
    }
    *applied = seq;
    true
}

/// How deep `chain`'s best chain holds `txid` now, from fresh reads: the tip,
/// the transaction's block, the hash at that height, which must be the same
/// block, and the tip again, which must not have moved. `None` when it is
/// unconfirmed, absent, off the best chain, the chain moved during the reads
/// or any read fails; never a guess.
pub async fn depth_on(source: &dyn ObservationSource, chain: ChainId, txid: Txid) -> Option<u32> {
    confirmed_on(source, chain, txid)
        .await
        .map(|(depth, _)| depth)
}

/// [`depth_on`], with the hash of the block that holds `txid`.
async fn confirmed_on(
    source: &dyn ObservationSource,
    chain: ChainId,
    txid: Txid,
) -> Option<(u32, BlockHash)> {
    let tip = source
        .tip(chain)
        .await
        .ok()
        .filter(|read| read.chain() == chain)?;
    let found = source
        .transaction(chain, txid)
        .await
        .ok()
        .filter(|read| read.chain() == chain)?;
    let TransactionObservation::Confirmed { txid: seen, block } = *found.value() else {
        return None;
    };
    if seen != txid || block.height > tip.value().height {
        return None;
    }
    let at = source
        .hash_at_height(chain, block.height)
        .await
        .ok()
        .filter(|read| read.chain() == chain)?;
    if *at.value() != block.hash {
        return None;
    }
    let after = source
        .tip(chain)
        .await
        .ok()
        .filter(|read| read.chain() == chain)?;
    if after.value() != tip.value() {
        return None;
    }
    let depth = std::convert::TryFrom::try_from(tip.value().height - block.height + 1).ok()?;
    Some((depth, block.hash))
}

/// Check the twin Cube's splits (`candidates`) on the twin of `here` now:
/// for each that Connect shows at least [`SPLIT_CONFIRMATIONS`] deep, the
/// inputs of that transaction (see [`TwinCheck::spent`]). A signature split
/// answers only if its confirmed witness is replay protected
/// ([`signature_split_inputs`]). Every txid gets an answer; a shallow or
/// absent split, a failed read or check, or a source that cannot be built
/// answers `None`.
pub async fn twin_confirmed(
    client: crate::services::coincube::CoincubeClient,
    here: ChainId,
    candidates: Vec<(Txid, Method)>,
    generation: u64,
    live: tokio::sync::watch::Receiver<u64>,
) -> Vec<(Txid, Option<HashSet<OutPoint>>)> {
    use crate::services::claim_observation::{http::HttpObservationSource, CollectionContext};

    let unchecked = |candidates: Vec<(Txid, Method)>| {
        candidates
            .into_iter()
            .map(|(txid, _)| (txid, None))
            .collect()
    };
    let Some(twin) = crate::services::entangled::twin_chain(here) else {
        return unchecked(candidates);
    };
    let Ok(source) = HttpObservationSource::new(
        client,
        ChainId::Bitcoin,
        ChainId::BitcoinBlake2b,
        CollectionContext {
            expected_generation: generation,
            generation: live,
        },
    ) else {
        return unchecked(candidates);
    };
    let mut checked = Vec::with_capacity(candidates.len());
    for (txid, method) in candidates {
        let block = confirmed_on(&source, twin, txid)
            .await
            .filter(|(depth, _)| *depth >= SPLIT_CONFIRMATIONS)
            .map(|(_, block)| block);
        let spent = match (block, method) {
            (None, _) => None,
            (Some(_), Method::SplitKey) => source
                .ancestry_transaction(twin, txid)
                .await
                .ok()
                .and_then(|raw| spent_by(&raw, txid)),
            // Only Bitcoin rejects a unified signature.
            (Some(block), Method::Signature) if twin.is_blake2b() => {
                signature_split_on(&source, twin, block, txid).await
            }
            (Some(_), Method::Signature) => None,
        };
        checked.push((txid, spent));
    }
    checked
}

/// A signature split holds at most this many inputs for the twin check to
/// look up their parents; a larger one is not checked.
pub const MAX_SIGNATURE_SPLIT_INPUTS: usize = 64;

/// What a signature split's confirmed witness showed, by (chain, block,
/// txid). The block's bytes never change, so a verdict, either way, holds
/// for as long as that block does; only the depth is read again each pass.
/// Failed reads are not verdicts and are never kept.
type Verdicts = HashMap<(ChainId, BlockHash, Txid), Option<HashSet<OutPoint>>>;

/// Verdicts kept at most; past this the cache starts over.
const MAX_VERDICTS: usize = 256;

fn verdicts() -> &'static std::sync::Mutex<Verdicts> {
    static VERDICTS: std::sync::OnceLock<std::sync::Mutex<Verdicts>> = std::sync::OnceLock::new();
    VERDICTS.get_or_init(Default::default)
}

/// The inputs of signature split `txid`, confirmed in `block` on `chain`,
/// if the witness that block commits to is replay protected. A block and
/// its parents are fetched once per (block, txid) ([`Verdicts`]).
async fn signature_split_on(
    source: &crate::services::claim_observation::http::HttpObservationSource,
    chain: ChainId,
    block: BlockHash,
    txid: Txid,
) -> Option<HashSet<OutPoint>> {
    let key = (chain, block, txid);
    if let Some(verdict) = verdicts().lock().ok()?.get(&key) {
        return verdict.clone();
    }
    let verdict = signature_split_verdict(source, chain, block, txid).await?;
    if let Ok(mut verdicts) = verdicts().lock() {
        if verdicts.len() >= MAX_VERDICTS {
            verdicts.clear();
        }
        verdicts.insert(key, verdict.clone());
    }
    verdict
}

/// [`signature_split_on`] without the cache: `None` when a read failed or
/// the block did not authenticate (no verdict), `Some` with the verdict
/// otherwise.
async fn signature_split_verdict(
    source: &crate::services::claim_observation::http::HttpObservationSource,
    chain: ChainId,
    block: BlockHash,
    txid: Txid,
) -> Option<Option<HashSet<OutPoint>>> {
    let raw = source.raw_block(chain, block).await.ok()?;
    let tx = coincube_core::btcb2_block::confirmed_transaction(&raw, block, txid).ok()?;
    drop(raw);
    if tx.input.len() > MAX_SIGNATURE_SPLIT_INPUTS {
        return Some(None);
    }
    let mut parents = HashMap::new();
    for input in &tx.input {
        let parent = input.previous_output.txid;
        if parents.contains_key(&parent) {
            continue;
        }
        let raw = source.ancestry_transaction(chain, parent).await.ok()?;
        let parent_tx: Transaction =
            coincube_core::miniscript::bitcoin::consensus::deserialize(&raw).ok()?;
        parents.insert(parent, parent_tx);
    }
    Some(signature_split_inputs(&tx, &parents))
}

/// The inputs of `tx`, a confirmed signature split as its block commits to
/// it, if every input is signed only with valid `ALL|UNIFIED` signatures
/// ([`coincube_core::confirmed_unified`]), so Bitcoin rejects it. `parents`
/// holds the transactions it spends, by txid.
fn signature_split_inputs(
    tx: &Transaction,
    parents: &HashMap<Txid, Transaction>,
) -> Option<HashSet<OutPoint>> {
    let spent_outputs = tx
        .input
        .iter()
        .map(|input| {
            let parent = parents.get(&input.previous_output.txid)?;
            if parent.compute_txid() != input.previous_output.txid {
                return None;
            }
            parent
                .output
                .get(input.previous_output.vout as usize)
                .cloned()
        })
        .collect::<Option<Vec<_>>>()?;
    let secp = coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::verification_only();
    coincube_core::confirmed_unified::verify_replay_protected(tx, &spent_outputs, &secp).ok()?;
    Some(tx.input.iter().map(|input| input.previous_output).collect())
}

/// The inputs of `raw` if it is the transaction `txid`. A txid commits to
/// every input, so these are the inputs of the transaction the chain holds.
fn spent_by(raw: &[u8], txid: Txid) -> Option<HashSet<OutPoint>> {
    let tx: Transaction = coincube_core::miniscript::bitcoin::consensus::deserialize(raw).ok()?;
    (tx.compute_txid() == txid)
        .then(|| tx.input.iter().map(|input| input.previous_output).collect())
}

/// Where a split build is, for the review's status line while it runs.
/// Checking the split key asks Connect about both chains and can take a
/// while; the user should see which part is slow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStage {
    CheckingKey,
    CheckingCoins,
    Building,
}

impl BuildStage {
    pub fn label(self) -> &'static str {
        match self {
            BuildStage::CheckingKey => "Checking the split key on both chains",
            BuildStage::CheckingCoins => "Checking the coins",
            BuildStage::Building => "Building the transaction",
        }
    }
}

/// Build the split, store it with the daemon, and return it for the PSBT
/// screen. The caller records it in the journal. For [`Method::SplitKey`]
/// the key is re-qualified against both chains first; nothing is built if
/// that fails.
#[allow(clippy::too_many_arguments)]
pub async fn prepare(
    daemon: Arc<dyn Daemon + Send + Sync>,
    wallet: Arc<Wallet>,
    method: Method,
    coins: Vec<OutPoint>,
    key: Option<KeyChoice>,
    feerate_vb: u64,
    client: crate::services::coincube::CoincubeClient,
    generation: u64,
    live: tokio::sync::watch::Receiver<u64>,
    mut progress: iced_runtime::task::Sender<BuildStage>,
) -> Result<Prepared, String> {
    let chain = wallet.chain;
    if !crate::services::chain_status::checks_enabled(chain) {
        return Err("Splitting coins isn't available on this network.".into());
    }
    if coins.is_empty() {
        return Err("Select at least one coin to split.".into());
    }
    let key = match (method, key) {
        (Method::SplitKey, Some(key)) => Some(key),
        (Method::SplitKey, None) => return Err("This split needs a split key.".into()),
        (Method::Signature, _) if !chain.is_blake2b() => {
            return Err("Bitcoin can't split with a replay-protected signature.".into())
        }
        (Method::Signature, _) => None,
    };
    if let Some(key) = &key {
        progress.send(BuildStage::CheckingKey).await;
        requalify(&client, chain, key, generation, live.clone()).await?;
    }
    let mut requested: BTreeSet<OutPoint> = coins.iter().copied().collect();
    if requested.len() != coins.len() {
        return Err("The split lists a coin twice.".into());
    }
    if let Some(key) = &key {
        if !requested.insert(key.outpoint) {
            return Err("The split key can't also be a coin to split.".into());
        }
    }
    let requested: Vec<OutPoint> = requested.into_iter().collect();
    progress.send(BuildStage::CheckingCoins).await;
    let mut fresh = daemon
        .list_coins(&[CoinStatus::Confirmed], &requested)
        .await
        .map_err(|e| e.to_string())?
        .coins;
    wallet.apply_coin_overrides(&mut fresh);
    let found: BTreeSet<OutPoint> = fresh.iter().map(|coin| coin.outpoint).collect();
    if found.len() != requested.len()
        || requested.iter().any(|outpoint| !found.contains(outpoint))
        || fresh.iter().any(|coin| {
            coin.spend_info.is_some() || coin.is_immature || coin.block_height.is_none()
        })
    {
        return Err(
            "Some of these coins changed, are spent or aren't confirmed yet. Read the Vault again."
                .into(),
        );
    }
    if !crate::services::claim_observation::session_current(&live, generation) {
        return Err("The session changed. Try again.".into());
    }
    progress.send(BuildStage::Building).await;
    let psbt = match daemon
        .create_spend_tx(&requested, &HashMap::new(), feerate_vb, None)
        .await
        .map_err(|e| e.to_string())?
    {
        CreateSpendResult::Success { psbt, .. } => psbt,
        CreateSpendResult::InsufficientFunds { missing } => {
            return Err(format!(
                "These coins don't cover the fee. {} sats short.",
                missing
            ))
        }
    };
    check_construction(&psbt, &requested)?;
    daemon
        .update_spend_tx(&psbt)
        .await
        .map_err(|e| e.to_string())?;
    let record = SplitRecord {
        txid: psbt.unsigned_tx.compute_txid(),
        chain,
        method,
        inputs: coins,
        key: key.as_ref().map(|key| key.outpoint),
        key_path: key.as_ref().map(|key| hex::encode(key.path.encode())),
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        confirmations: 0,
    };
    Ok(Prepared { psbt, record })
}

/// The daemon's self-send must spend exactly the requested coins into one
/// output back into this Vault.
fn check_construction(psbt: &Psbt, requested: &[OutPoint]) -> Result<(), String> {
    let spent: BTreeSet<OutPoint> = psbt
        .unsigned_tx
        .input
        .iter()
        .map(|input| input.previous_output)
        .collect();
    let wanted: BTreeSet<OutPoint> = requested.iter().copied().collect();
    let owned = |i: usize| {
        psbt.outputs.get(i).is_some_and(|output| {
            !output.bip32_derivation.is_empty() || !output.tap_key_origins.is_empty()
        })
    };
    if spent != wanted || psbt.unsigned_tx.input.len() != requested.len() {
        return Err("The split doesn't spend exactly the selected coins.".into());
    }
    if psbt.unsigned_tx.output.len() != 1 || !owned(0) {
        return Err("The split must pay back into this Vault only.".into());
    }
    Ok(())
}

/// Re-qualify the key's proof against both canonical chains now. Run before
/// a split is built, and again by the PSBT screen before the split may be
/// signed or broadcast, so a reorg in between can't leave a stale proof
/// standing in for the key.
pub async fn requalify(
    client: &crate::services::coincube::CoincubeClient,
    chain: ChainId,
    key: &KeyChoice,
    generation: u64,
    live: tokio::sync::watch::Receiver<u64>,
) -> Result<(), String> {
    use crate::services::claim_observation::{http::HttpObservationSource, CollectionContext};
    if key.path.selected() != key.outpoint {
        return Err("The split key's proof doesn't match it.".into());
    }
    let source = HttpObservationSource::new(
        client.clone(),
        ChainId::Bitcoin,
        ChainId::BitcoinBlake2b,
        CollectionContext {
            expected_generation: generation,
            generation: live,
        },
    )
    .map_err(|e| format!("Couldn't check the split key: {:?}", e))?;
    let policy = coincube_core::claim::Policy {
        max_observation_age_seconds: 60,
    };
    let refreshed = if chain.is_blake2b() {
        source.requalify_fork_ancestry(&key.path, policy).await
    } else {
        source.requalify_ancestry(&key.path, policy).await
    }
    .map_err(|e| format!("The split key couldn't be verified right now: {:?}", e))?;
    let path = refreshed
        .retained_path()
        .map_err(|e| format!("The split key's proof is invalid: {:?}", e))?;
    if path.selected() != key.outpoint {
        return Err("The split key's proof doesn't match it.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::{
        absolute, bip32::ChildNumber, script::Builder, transaction, Address, Amount, ScriptBuf,
        Sequence, TxIn, TxOut, Witness,
    };
    use std::str::FromStr;

    /// The split size estimate: refused for no coins or too many, and one
    /// more shared coin costs at least one more input.
    #[test]
    fn split_vbytes_estimate_bounds_and_refusals() {
        let wsh = CoincubeDescriptor::from_str("wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr").unwrap();
        let wsh = WalletDescriptor::Vault(wsh);
        assert_eq!(estimate_split_vbytes(&wsh, 0), None);
        let one = estimate_split_vbytes(&wsh, 1).unwrap();
        let two = estimate_split_vbytes(&wsh, 2).unwrap();
        // Two P2WSH inputs and one P2WSH output: well above a bare skeleton.
        assert!(one > 2 * 41 + 43, "{}", one);
        // Each input adds at least its outpoint, sequence and a signature.
        assert!(two >= one + 41 + 18, "{} {}", one, two);
        assert_eq!(estimate_split_vbytes(&wsh, 20_001), None);
    }

    /// An imported Vault's split is sized from its own shape: each extra
    /// input costs what that shape's input costs once signed, and the one
    /// output is a change address of the wallet. The same bounds hold on
    /// Bitcoin Blake2b, where the wallet's scripts are identical.
    #[test]
    fn split_vbytes_estimate_covers_every_imported_shape() {
        use crate::services::split_test_wallets::{
            imported_fork_vault, imported_vault, Shape, SHAPES,
        };
        for shape in SHAPES {
            let bitcoin = imported_vault(shape).main_descriptor;
            let fork = imported_fork_vault(shape).main_descriptor;
            for descriptor in [&bitcoin, &fork] {
                assert_eq!(estimate_split_vbytes(descriptor, 0), None, "{shape:?}");
                assert_eq!(estimate_split_vbytes(descriptor, 20_001), None, "{shape:?}");
                let one = estimate_split_vbytes(descriptor, 1).unwrap();
                let two = estimate_split_vbytes(descriptor, 2).unwrap();
                let three = estimate_split_vbytes(descriptor, 3).unwrap();
                let input = two - one;
                // A signed input of this shape, in vbytes, rounded either way.
                let expected = match shape {
                    Shape::Wpkh => 68..=69,
                    Shape::ShWpkh => 91..=92,
                    Shape::Pkh => 148..=149,
                    Shape::WshSortedMulti | Shape::WshMulti => 104..=105,
                };
                for step in [input, three - two] {
                    assert!(expected.contains(&step), "{:?}: {}", shape, step);
                }
                // Version, locktime, counts, one change output and the
                // segwit marker around the two inputs.
                let output = match shape {
                    Shape::Wpkh => 31,
                    Shape::ShWpkh => 32,
                    Shape::Pkh => 34,
                    Shape::WshSortedMulti | Shape::WshMulti => 43,
                };
                let skeleton = one - 2 * input;
                assert!(
                    (9 + output..=13 + output).contains(&skeleton),
                    "{:?}: {}",
                    shape,
                    skeleton
                );
            }
        }
    }

    /// An imported Vault never offers the replay-protected signature, on
    /// either chain, whatever its signers: it splits with a split key.
    #[test]
    fn an_imported_vault_splits_only_with_a_key() {
        use crate::services::split_test_wallets::{imported_fork_vault, imported_vault, SHAPES};
        for shape in SHAPES {
            for mut wallet in [imported_vault(shape), imported_fork_vault(shape)] {
                wallet.keychain_keys_recorded = true;
                if let Some(policy) = wallet.policy() {
                    wallet
                        .border_wallet_fingerprints
                        .extend(policy.primary_path().thresh_origins().1.keys().copied());
                }
                assert!(
                    !signature_available(&wallet),
                    "{shape:?} {:?}",
                    wallet.chain
                );
            }
        }
    }

    /// The daemon's imported self-send is what a split builds: for every
    /// imported shape, on both chains, sweeping the coins and the key into one
    /// change output passes the split's construction check, and the overlay's
    /// estimate covers the fee the daemon charged at the same rate.
    #[test]
    fn an_imported_sweep_is_a_valid_split_for_every_shape() {
        use crate::services::split_test_wallets::{imported_fork_vault, imported_vault, SHAPES};
        use coincube_core::spend::{
            AddrInfo, CandidateCoin, SpendOutputAddress, SpendTxFees, TxGetter,
        };
        struct Txs(Vec<Transaction>);
        impl TxGetter for Txs {
            fn get_tx(&mut self, txid: &Txid) -> Option<Transaction> {
                self.0.iter().find(|tx| tx.compute_txid() == *txid).cloned()
            }
        }
        const FEERATE: u64 = 5;
        for shape in SHAPES {
            for wallet in [imported_vault(shape), imported_fork_vault(shape)] {
                let imported = wallet.main_descriptor.as_imported().unwrap();
                let network = wallet.chain.bitcoin_network();
                // A both-chain coin on the receive branch and a split key on
                // the change branch.
                let funding = |change: bool, index: u32, sats: u64| {
                    let descriptor = if change {
                        imported.change_descriptor()
                    } else {
                        imported.receive_descriptor()
                    };
                    let previous = Transaction {
                        version: transaction::Version::TWO,
                        lock_time: absolute::LockTime::ZERO,
                        input: vec![TxIn {
                            previous_output: OutPoint::new(
                                Txid::from_str(&format!("{:064x}", index + 1)).unwrap(),
                                0,
                            ),
                            ..TxIn::default()
                        }],
                        output: vec![TxOut {
                            value: Amount::from_sat(sats),
                            script_pubkey: descriptor
                                .at_derivation_index(index)
                                .unwrap()
                                .script_pubkey(),
                        }],
                    };
                    let candidate = CandidateCoin {
                        outpoint: OutPoint::new(previous.compute_txid(), 0),
                        amount: Amount::from_sat(sats),
                        deriv_index: ChildNumber::Normal { index },
                        is_change: change,
                        must_select: true,
                        sequence: None,
                        ancestor_info: None,
                    };
                    (previous, candidate)
                };
                let (coin_tx, coin) = funding(false, 0, 100_000);
                let (key_tx, key) = funding(true, 3, 20_000);
                let change = SpendOutputAddress {
                    addr: Address::from_script(
                        &wallet
                            .main_descriptor
                            .change_descriptor()
                            .script_pubkey(4)
                            .unwrap(),
                        network,
                    )
                    .unwrap(),
                    info: Some(AddrInfo {
                        index: ChildNumber::Normal { index: 4 },
                        is_change: true,
                    }),
                };
                let secp =
                    coincube_core::miniscript::bitcoin::secp256k1::Secp256k1::verification_only();
                let created = coincube_core::imported_spend::create_imported_spend(
                    imported,
                    &secp,
                    &mut Txs(vec![coin_tx, key_tx]),
                    &[],
                    &[coin, key],
                    SpendTxFees::Regular(FEERATE),
                    change,
                    absolute::LockTime::ZERO,
                )
                .unwrap();
                let requested = [coin.outpoint, key.outpoint];
                check_construction(&created.psbt, &requested)
                    .unwrap_or_else(|e| panic!("{shape:?} {:?}: {e}", wallet.chain));
                let out: u64 = created
                    .psbt
                    .unsigned_tx
                    .output
                    .iter()
                    .map(|output| output.value.to_sat())
                    .sum();
                let fee = 120_000 - out;
                let estimate = estimate_split_vbytes(&wallet.main_descriptor, 1).unwrap();
                assert!(
                    fee <= FEERATE * estimate && fee + 2 * FEERATE >= FEERATE * estimate,
                    "{shape:?} {:?}: fee {fee} for an estimate of {estimate} vbytes",
                    wallet.chain
                );
                // The split must not reach past the two coins.
                assert!(check_construction(&created.psbt, &requested[..1]).is_err());
            }
        }
    }

    fn tx(inputs: Vec<OutPoint>, coinbase_height: Option<i64>) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: match coinbase_height {
                Some(height) => vec![TxIn {
                    previous_output: OutPoint::null(),
                    script_sig: Builder::new().push_int(height).push_int(1).into_script(),
                    sequence: Sequence::MAX,
                    witness: Witness::new(),
                }],
                None => inputs
                    .into_iter()
                    .map(|previous_output| TxIn {
                        previous_output,
                        script_sig: ScriptBuf::new(),
                        sequence: Sequence::MAX,
                        witness: Witness::new(),
                    })
                    .collect(),
            },
            output: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    fn coin(outpoint: OutPoint, height: Option<i32>) -> Coin {
        Coin {
            amount: Amount::from_sat(50_000),
            outpoint,
            address: Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
                .unwrap()
                .assume_checked(),
            block_height: height,
            derivation_index: ChildNumber::from_normal_idx(0).unwrap(),
            spend_info: None,
            is_immature: false,
            is_change: true,
            is_from_self: true,
        }
    }

    /// A key with a one-link path to a coinbase, a shared coin, and the split
    /// spending both.
    fn split_fixture() -> (SplitRecord, Transaction) {
        let coinbase = tx(vec![], Some(962_000));
        let key = OutPoint::new(coinbase.compute_txid(), 0);
        let key_path = RetainedPath::new(
            key,
            vec![OwnedLink {
                transaction: serialize(&coinbase),
                parent_input: None,
            }],
        )
        .unwrap();
        let shared = OutPoint::new(Txid::from_str(&"11".repeat(32)).unwrap(), 3);
        let split = tx(vec![shared, key], None);
        let record = SplitRecord {
            txid: split.compute_txid(),
            chain: ChainId::Bitcoin,
            method: Method::SplitKey,
            inputs: vec![shared],
            key: Some(key),
            key_path: Some(hex::encode(key_path.encode())),
            created_at: 0,
            confirmations: 0,
        };
        (record, split)
    }

    #[test]
    fn split_output_is_splitting_until_six_confirmations_then_saved() {
        let (record, split) = split_fixture();
        let output = OutPoint::new(record.txid, 0);
        let transactions: HashMap<_, _> = [(record.txid, split)].into();
        let mut records = vec![record];

        let derived = derive(
            &mut records,
            &[coin(output, Some(100))],
            102,
            &transactions,
            UNIX_EPOCH,
        );
        assert_eq!(derived.splitting.get(&output), Some(&3));
        assert!(derived.proofs.is_empty());
        assert_eq!(records[0].confirmations, 3);
        assert!(derived.changed);

        let derived = derive(
            &mut records,
            &[coin(output, Some(100))],
            105,
            &transactions,
            UNIX_EPOCH,
        );
        assert!(derived.splitting.is_empty());
        assert_eq!(derived.proofs.len(), 1);
        assert!(
            matches!(&derived.proofs[0], (o, ProofEntry::Saved(p)) if *o == output && p.selected() == output),
            "the journal's key path is not re-qualified, so a fresh check must verify it"
        );
        assert_eq!(derived.proofs[0].1.outcome(), None, "not shown as verified");
        assert_eq!(records[0].confirmations, 6);
    }

    #[test]
    fn a_derived_path_only_fills_a_gap() {
        let (record, split) = split_fixture();
        let output = OutPoint::new(record.txid, 0);
        let path = extended_path(&record, &split, output).unwrap();
        let derived = || vec![(output, ProofEntry::Saved(path.clone()))];

        let mut proofs = HashMap::new();
        assert!(merge_proofs(&mut proofs, derived()));
        assert!(matches!(proofs.get(&output), Some(ProofEntry::Saved(_))));
        assert!(!merge_proofs(&mut proofs, derived()), "already there");

        for answer in [
            ProofEntry::Verified(path.clone()),
            ProofEntry::NoQualifyingAncestry {
                checked_at: UNIX_EPOCH,
            },
        ] {
            let mut proofs: HashMap<_, _> = [(output, answer.clone())].into();
            assert!(!merge_proofs(&mut proofs, derived()));
            assert_eq!(
                std::mem::discriminant(&proofs[&output]),
                std::mem::discriminant(&answer),
                "a check's own answer stays"
            );
        }

        // A check that couldn't finish is no answer: the path fills in and
        // the failure's retry time stays.
        let mut proofs: HashMap<_, _> = [(
            output,
            ProofEntry::Failed {
                failed_at: UNIX_EPOCH,
            },
        )]
        .into();
        assert!(merge_proofs(&mut proofs, derived()));
        assert!(matches!(
            proofs.get(&output),
            Some(ProofEntry::SavedRetry { failed_at, .. }) if *failed_at == UNIX_EPOCH
        ));
        assert!(!merge_proofs(&mut proofs, derived()), "already there");
    }

    #[test]
    fn unconfirmed_split_output_is_splitting_at_zero() {
        let (record, _) = split_fixture();
        let output = OutPoint::new(record.txid, 0);
        let mut records = vec![record];
        let derived = derive(
            &mut records,
            &[coin(output, None)],
            100,
            &HashMap::new(),
            UNIX_EPOCH,
        );
        assert_eq!(derived.splitting.get(&output), Some(&0));
    }

    #[test]
    fn a_signature_split_separates_only_on_a_checked_witness() {
        let (mut record, _) = split_fixture();
        record.method = Method::Signature;
        record.key = None;
        record.key_path = None;
        let output = OutPoint::new(record.txid, 0);
        let mut records = vec![record];
        let derived = derive(
            &mut records,
            &[coin(output, Some(10))],
            20,
            &HashMap::new(),
            UNIX_EPOCH,
        );
        assert!(derived.proofs.is_empty(), "no proof for its own outputs");
        assert!(
            derived.splitting.is_empty(),
            "past the threshold, so not splitting"
        );
        assert_eq!(
            derived.unconfirmed,
            vec![output].into_iter().collect::<HashSet<_>>(),
            "shown as split but unconfirmed instead"
        );
        let now = std::time::Instant::now();
        assert!(
            twin_separated(&records, &HashMap::new(), now).is_empty(),
            "nothing on the twin side from the journal's word"
        );
        assert_eq!(
            twin_candidates(&records, &HashMap::new()),
            vec![(records[0].txid, Method::Signature)],
            "so the twin Cube looks it up"
        );
        // A check only carries inputs once the witness passed.
        let check = |spent| {
            vec![(records[0].txid, TwinCheck { spent, at: now })]
                .into_iter()
                .collect::<HashMap<_, _>>()
        };
        assert!(twin_separated(&records, &check(None), now).is_empty());
        let separated = twin_separated(
            &records,
            &check(Some(records[0].inputs.iter().copied().collect())),
            now,
        );
        assert_eq!(
            separated.keys().copied().collect::<HashSet<_>>(),
            records[0].inputs.iter().copied().collect::<HashSet<_>>()
        );
    }

    /// A Vault-style 2-of-2 output and a signature split of two of them,
    /// signed `ALL|UNIFIED` by both keys.
    fn signature_split() -> (Transaction, HashMap<Txid, Transaction>, ScriptBuf) {
        signed_split(false)
    }

    /// [`signature_split`], or with `legacy` the same transaction (same
    /// txid) signed BIP 143 `SIGHASH_ALL`, as Bitcoin accepts it.
    fn signed_split(legacy: bool) -> (Transaction, HashMap<Txid, Transaction>, ScriptBuf) {
        use coincube_core::{
            miniscript::{
                bitcoin::{secp256k1, PublicKey},
                Miniscript, Segwitv0,
            },
            unified_sighash::{UnifiedSighashCache, SCRIPT_TYPE_WITNESS_V0},
        };
        let secp = secp256k1::Secp256k1::new();
        let keys: Vec<secp256k1::SecretKey> = (1..=2u8)
            .map(|n| secp256k1::SecretKey::from_slice(&[n; 32]).unwrap())
            .collect();
        let public: Vec<String> = keys
            .iter()
            .map(|key| {
                PublicKey::new(secp256k1::PublicKey::from_secret_key(&secp, key)).to_string()
            })
            .collect();
        let script = Miniscript::<PublicKey, Segwitv0>::from_str(&format!(
            "multi(2,{},{})",
            public[0], public[1]
        ))
        .unwrap()
        .encode();
        let mut parent = tx(
            vec![OutPoint::new(Txid::from_str(&"aa".repeat(32)).unwrap(), 0)],
            None,
        );
        parent.output = vec![
            TxOut {
                value: Amount::from_sat(40_000),
                script_pubkey: script.to_p2wsh(),
            };
            2
        ];
        let parent_txid = parent.compute_txid();
        let mut split = tx(
            vec![OutPoint::new(parent_txid, 0), OutPoint::new(parent_txid, 1)],
            None,
        );
        split.output[0].script_pubkey = script.to_p2wsh();
        let spent = parent.output.clone();
        let cache = UnifiedSighashCache::new(&split, &spent).unwrap();
        let unsigned = split.clone();
        let witnesses: Vec<Witness> = (0..2)
            .map(|input| {
                let (digest, sighash) = if legacy {
                    let digest =
                        coincube_core::miniscript::bitcoin::sighash::SighashCache::new(&unsigned)
                            .p2wsh_signature_hash(
                                input,
                                &script,
                                spent[input].value,
                                coincube_core::miniscript::bitcoin::sighash::EcdsaSighashType::All,
                            )
                            .unwrap();
                    use coincube_core::miniscript::bitcoin::hashes::Hash;
                    (digest.to_byte_array(), 0x01)
                } else {
                    let digest = cache
                        .signature_hash(input, 0x21, SCRIPT_TYPE_WITNESS_V0, &script)
                        .unwrap();
                    (digest, 0x21)
                };
                let message = secp256k1::Message::from_digest(digest);
                let mut stack = vec![vec![]];
                for key in &keys {
                    let mut sig = secp.sign_ecdsa(&message, key).serialize_der().to_vec();
                    sig.push(sighash);
                    stack.push(sig);
                }
                stack.push(script.to_bytes());
                Witness::from_slice(&stack)
            })
            .collect();
        for (input, witness) in split.input.iter_mut().zip(witnesses) {
            input.witness = witness;
        }
        (
            split,
            vec![(parent_txid, parent)].into_iter().collect(),
            script,
        )
    }

    #[test]
    fn a_unified_witness_yields_the_split_inputs() {
        let (split, parents, _) = signature_split();
        assert_eq!(
            signature_split_inputs(&split, &parents),
            Some(
                split
                    .input
                    .iter()
                    .map(|input| input.previous_output)
                    .collect()
            )
        );
    }

    #[test]
    fn a_legacy_or_unverifiable_witness_yields_nothing() {
        let (split, parents, _) = signature_split();
        // A second, legacy signing of the same split: Bitcoin-valid, same txid.
        let (legacy, _, _) = signed_split(true);
        assert_eq!(legacy.compute_txid(), split.compute_txid());
        assert_ne!(legacy.compute_wtxid(), split.compute_wtxid());
        assert_eq!(signature_split_inputs(&legacy, &parents), None);
        // One legacy input is enough to refuse.
        let mut mixed = split.clone();
        mixed.input[1].witness = legacy.input[1].witness.clone();
        assert_eq!(signature_split_inputs(&mixed, &parents), None);
        // A parent missing, or served under the wrong txid.
        assert_eq!(signature_split_inputs(&split, &HashMap::new()), None);
        let (txid, mut parent) = parents.iter().next().map(|(k, v)| (*k, v.clone())).unwrap();
        parent.output[1].value = Amount::from_sat(40_001);
        let wrong: HashMap<Txid, Transaction> = vec![(txid, parent.clone())].into_iter().collect();
        assert_eq!(signature_split_inputs(&split, &wrong), None);
        // An output index the parent does not have.
        let mut beyond = split.clone();
        beyond.input[1].previous_output.vout = 2;
        assert_eq!(signature_split_inputs(&beyond, &parents), None);
    }

    /// A legacy-header block holding a coinbase with a witness commitment
    /// and `split`.
    fn block_with(split: &Transaction) -> coincube_core::miniscript::bitcoin::Block {
        use coincube_core::miniscript::bitcoin::{
            block::{Header, Version},
            hashes::Hash,
            Block, CompactTarget, TxMerkleNode, WitnessMerkleNode,
        };
        let mut coinbase = tx(vec![], Some(1));
        coinbase.input[0].witness = Witness::from_slice(&[[0u8; 32]]);
        let mut block = Block {
            header: Header {
                version: Version::TWO,
                prev_blockhash: BlockHash::all_zeros(),
                merkle_root: TxMerkleNode::all_zeros(),
                time: 0,
                bits: CompactTarget::from_consensus(0x207fffff),
                nonce: 0,
            },
            txdata: vec![coinbase, split.clone()],
        };
        let root: WitnessMerkleNode = block.witness_root().unwrap();
        let commitment = Block::compute_witness_commitment(&root, &[0u8; 32]);
        let mut script = vec![0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed];
        script.extend_from_slice(commitment.as_byte_array());
        block.txdata[0].output.push(TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::from(script),
        });
        block.header.merkle_root = block.compute_merkle_root().unwrap();
        block
    }

    #[test]
    fn a_block_commits_the_witness_the_check_reads() {
        use coincube_core::{
            btcb2_block::confirmed_transaction, miniscript::bitcoin::consensus::serialize,
        };
        let (split, parents, _) = signature_split();
        let block = block_with(&split);
        let confirmed =
            confirmed_transaction(&serialize(&block), block.block_hash(), split.compute_txid())
                .unwrap();
        assert!(signature_split_inputs(&confirmed, &parents).is_some());

        // Served the unified copy while the block holds a legacy one: the
        // commitment catches the swap before the witness is ever judged.
        let (legacy, _, _) = signed_split(true);
        let held = block_with(&legacy);
        let mut served = held.clone();
        served.txdata[1] = split.clone();
        assert!(confirmed_transaction(
            &serialize(&served),
            held.block_hash(),
            split.compute_txid()
        )
        .is_err());
        let confirmed =
            confirmed_transaction(&serialize(&held), held.block_hash(), split.compute_txid())
                .unwrap();
        assert_eq!(signature_split_inputs(&confirmed, &parents), None);
    }

    /// Connect's BTCB2 routes for one split confirmed 11 deep in `block`,
    /// with its parents. Returns the raw-block and parent mocks.
    fn connect_holding<'a>(
        server: &'a httpmock::MockServer,
        block: &coincube_core::miniscript::bitcoin::Block,
        parents: &HashMap<Txid, Transaction>,
        raw_status: u16,
    ) -> (httpmock::Mock<'a>, Vec<httpmock::Mock<'a>>) {
        use coincube_core::miniscript::bitcoin::consensus::serialize;
        use httpmock::prelude::*;
        let base = "/api/v1/esplora/bitcoin-blake2b/mainnet";
        let txid = block.txdata[1].compute_txid();
        let hash = block.block_hash();
        let tip = BlockHash::from_str(&"11".repeat(32)).unwrap();
        let fresh = |path: String, body: String| {
            server.mock(move |when, then| {
                when.method(GET)
                    .path(path)
                    .header("x-coincube-observation", "fresh");
                then.status(200)
                    .header("X-Coincube-Observation", "fresh")
                    .header("X-Cache", "BYPASS")
                    .header("Cache-Control", "no-store")
                    .body(body);
            })
        };
        fresh(format!("{base}/blocks/tip/hash"), tip.to_string());
        fresh(
            format!("{base}/block/{tip}/status"),
            r#"{"in_best_chain":true,"height":110}"#.into(),
        );
        fresh(format!("{base}/block-height/110"), tip.to_string());
        fresh(
            format!("{base}/tx/{txid}"),
            format!(
                r#"{{"txid":"{txid}","status":{{"confirmed":true,"block_height":100,"block_hash":"{hash}"}}}}"#
            ),
        );
        fresh(format!("{base}/block-height/100"), hash.to_string());
        let raw = serialize(block);
        let raw_mock = server.mock(move |when, then| {
            when.method(GET).path(format!("{base}/block/{hash}/raw"));
            then.status(raw_status).body(raw);
        });
        let parent_mocks = parents
            .iter()
            .map(|(id, parent)| {
                let hex = hex::encode(serialize(parent));
                server.mock(move |when, then| {
                    when.method(GET).path(format!("{base}/tx/{id}/hex"));
                    then.status(200).body(hex);
                })
            })
            .collect();
        (raw_mock, parent_mocks)
    }

    async fn check_twin(
        server: &httpmock::MockServer,
        txid: Txid,
    ) -> Vec<(Txid, Option<HashSet<OutPoint>>)> {
        let mut client = crate::services::coincube::CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-observation-token");
        let (_sender, live) = tokio::sync::watch::channel(4);
        twin_confirmed(
            client,
            ChainId::Bitcoin,
            vec![(txid, Method::Signature)],
            4,
            live,
        )
        .await
    }

    #[tokio::test]
    async fn the_twin_check_counts_a_confirmed_unified_split_from_one_block_read() {
        let (split, parents, _) = signature_split();
        let txid = split.compute_txid();
        let server = httpmock::MockServer::start();
        let refusal = crate::services::split_test_connect::strict(&server);
        let (raw, parent_mocks) = connect_holding(&server, &block_with(&split), &parents, 200);
        let inputs: HashSet<OutPoint> = split.input.iter().map(|i| i.previous_output).collect();
        assert_eq!(
            check_twin(&server, txid).await,
            vec![(txid, Some(inputs.clone()))]
        );
        // The next pass reads the depth again but not the block or parents.
        assert_eq!(check_twin(&server, txid).await, vec![(txid, Some(inputs))]);
        raw.assert_hits(1);
        for parent in parent_mocks {
            parent.assert_hits(1);
        }
        refusal.assert_hits(0);
    }

    #[tokio::test]
    async fn the_twin_check_refuses_a_legacy_split_and_retries_a_failed_read() {
        let (legacy, parents, _) = signed_split(true);
        let txid = legacy.compute_txid();
        let server = httpmock::MockServer::start();
        let refusal = crate::services::split_test_connect::strict(&server);
        let (raw, _) = connect_holding(&server, &block_with(&legacy), &parents, 200);
        assert_eq!(check_twin(&server, txid).await, vec![(txid, None)]);
        assert_eq!(check_twin(&server, txid).await, vec![(txid, None)]);
        raw.assert_hits(1);
        refusal.assert_hits(0);

        // A block Connect can't serve is no verdict: the next pass asks again.
        // (A unified split in a block of its own, so no earlier verdict.)
        let (mut split, parents, _) = signature_split();
        split.lock_time =
            coincube_core::miniscript::bitcoin::absolute::LockTime::from_height(7).unwrap();
        let server = httpmock::MockServer::start();
        let (raw, _) = connect_holding(&server, &block_with(&split), &parents, 503);
        let txid = split.compute_txid();
        assert_eq!(check_twin(&server, txid).await, vec![(txid, None)]);
        assert_eq!(check_twin(&server, txid).await, vec![(txid, None)]);
        raw.assert_hits(2);
    }

    #[test]
    fn unseen_records_expire_and_seen_ones_stay() {
        let (record, _) = split_fixture();
        let later = UNIX_EPOCH + std::time::Duration::from_secs(UNSEEN_RECORD_LIFETIME_SECONDS);
        let mut records = vec![record.clone()];
        assert!(derive(&mut records, &[], 1, &HashMap::new(), later).changed);
        assert!(records.is_empty());
        let mut records = vec![record.clone()];
        derive(
            &mut records,
            &[coin(OutPoint::new(record.txid, 0), None)],
            1,
            &HashMap::new(),
            later,
        );
        assert_eq!(records.len(), 1);
    }

    #[test]
    fn a_path_through_a_different_key_is_refused() {
        let (mut record, split) = split_fixture();
        let output = OutPoint::new(record.txid, 0);
        assert!(extended_path(&record, &split, output).is_some());
        record.key = Some(OutPoint::new(record.txid, 9));
        assert!(extended_path(&record, &split, output).is_none());
    }

    #[test]
    fn a_reorg_takes_the_confirmations_back() {
        let (record, split) = split_fixture();
        let output = OutPoint::new(record.txid, 0);
        let transactions: HashMap<_, _> = [(record.txid, split)].into();
        let mut records = vec![record];
        derive(
            &mut records,
            &[coin(output, Some(100))],
            107,
            &transactions,
            UNIX_EPOCH,
        );
        assert_eq!(records[0].confirmations, 8);
        // The block is gone: the output is unconfirmed again.
        let derived = derive(
            &mut records,
            &[coin(output, None)],
            107,
            &transactions,
            UNIX_EPOCH,
        );
        assert_eq!(records[0].confirmations, 0);
        assert!(derived.changed);
        assert_eq!(derived.splitting.get(&output), Some(&0));
        assert!(derived.proofs.is_empty());
        // Or the transaction left the wallet entirely.
        records[0].confirmations = 8;
        assert!(derive(&mut records, &[], 107, &transactions, UNIX_EPOCH).changed);
        assert_eq!(records[0].confirmations, 0);
    }

    #[test]
    fn twin_coins_separate_only_on_a_recent_fresh_confirmation() {
        let (mut record, _) = split_fixture();
        let now = std::time::Instant::now();
        record.confirmations = 100;
        assert!(
            twin_separated(&[record.clone()], &HashMap::new(), now).is_empty(),
            "the twin journal's count is never trusted"
        );
        let (_, split) = split_fixture();
        let confirmed = spent_by(&serialize(&split), record.txid);
        let check = |spent, at| -> HashMap<Txid, TwinCheck> {
            vec![(record.txid, TwinCheck { spent, at })]
                .into_iter()
                .collect()
        };
        assert_eq!(
            twin_separated(&[record.clone()], &check(confirmed.clone(), now), now)
                .into_keys()
                .collect::<HashSet<_>>(),
            record.inputs.iter().copied().collect()
        );
        assert!(
            twin_separated(&[record.clone()], &check(None, now), now).is_empty(),
            "a failed or shallow read separates nothing"
        );
        let later = now + TWIN_CHECK_LIFETIME + std::time::Duration::from_secs(1);
        assert!(
            twin_separated(&[record.clone()], &check(confirmed, now), later).is_empty(),
            "an old check stops counting"
        );
    }

    #[test]
    fn a_separation_expires_without_another_pass() {
        let (record, split) = split_fixture();
        let at = std::time::Instant::now();
        let checks: HashMap<Txid, TwinCheck> = [(
            record.txid,
            TwinCheck {
                spent: spent_by(&serialize(&split), record.txid),
                at,
            },
        )]
        .into();
        // Computed once, by the last pass that landed; no pass follows.
        let separated = twin_separated(std::slice::from_ref(&record), &checks, at);
        let coin = record.inputs[0];
        assert!(is_separated(&separated, &coin, at));
        assert!(is_separated(&separated, &coin, at + TWIN_CHECK_LIFETIME));
        assert!(
            !is_separated(
                &separated,
                &coin,
                at + TWIN_CHECK_LIFETIME + std::time::Duration::from_secs(1)
            ),
            "the stored result stops counting when its check does"
        );
    }

    #[test]
    fn only_inputs_the_confirmed_transaction_spends_are_separated() {
        let (mut record, split) = split_fixture();
        let shared = record.inputs[0];
        let extra = OutPoint::new(Txid::from_str(&"22".repeat(32)).unwrap(), 1);
        record.inputs.push(extra);
        let now = std::time::Instant::now();
        let checks: HashMap<Txid, TwinCheck> = [(
            record.txid,
            TwinCheck {
                spent: spent_by(&serialize(&split), record.txid),
                at: now,
            },
        )]
        .into();
        assert_eq!(
            twin_separated(&[record], &checks, now)
                .into_keys()
                .collect::<HashSet<_>>(),
            [shared].into(),
            "a journal input the confirmed transaction does not spend stays"
        );
    }

    #[test]
    fn spent_inputs_come_only_from_the_named_transaction() {
        let (record, split) = split_fixture();
        assert_eq!(
            spent_by(&serialize(&split), record.txid),
            Some(
                split
                    .input
                    .iter()
                    .map(|input| input.previous_output)
                    .collect()
            )
        );
        let other = Txid::from_str(&"33".repeat(32)).unwrap();
        assert_eq!(spent_by(&serialize(&split), other), None);
        assert_eq!(spent_by(&[0, 1, 2], record.txid), None);
    }

    #[test]
    fn a_late_pass_keeps_its_checks_and_an_unchecked_split_clears() {
        let (record, split) = split_fixture();
        let spent = spent_by(&serialize(&split), record.txid);
        let other = Txid::from_str(&"44".repeat(32)).unwrap();
        let early = std::time::Instant::now();
        let late = early + std::time::Duration::from_secs(1);
        let mut checks = HashMap::new();

        // The newer pass finished first and did not look `record` up.
        assert!(record_twin_checks(&mut checks, vec![(other, None)], late));
        // The older pass that did look it up still counts for it.
        assert!(record_twin_checks(
            &mut checks,
            vec![(record.txid, spent.clone()), (other, spent.clone())],
            early
        ));
        assert_eq!(checks[&record.txid].spent, spent);
        assert_eq!(
            checks[&other].spent, None,
            "an older check never replaces a newer one"
        );
        assert_eq!(
            twin_separated(std::slice::from_ref(&record), &checks, late)
                .into_keys()
                .collect::<HashSet<_>>(),
            record.inputs.iter().copied().collect()
        );

        // A later lookup that did not run or came back negative clears it.
        let later = late + std::time::Duration::from_secs(1);
        assert!(record_twin_checks(
            &mut checks,
            vec![(record.txid, None)],
            later
        ));
        assert!(twin_separated(&[record], &checks, later).is_empty());
    }

    #[test]
    fn twin_candidates_reach_every_record_in_turn() {
        let (record, _) = split_fixture();
        let records: Vec<SplitRecord> = (0..(MAX_TWIN_LOOKUPS as u8 + 2))
            .map(|n| SplitRecord {
                txid: Txid::from_str(&format!("{:02x}", n).repeat(32)).unwrap(),
                created_at: u64::from(n),
                ..record.clone()
            })
            .collect();
        let mut checks = HashMap::new();
        let first = twin_candidates(&records, &checks);
        assert_eq!(first.len(), MAX_TWIN_LOOKUPS);
        assert_eq!(first[0].0, records.last().unwrap().txid, "newest first");
        let at = std::time::Instant::now();
        for (txid, _) in &first {
            checks.insert(*txid, TwinCheck { spent: None, at });
        }
        let second = twin_candidates(&records, &checks);
        assert_eq!(
            &second[..2],
            &[
                (records[1].txid, records[1].method),
                (records[0].txid, records[0].method)
            ],
            "the two never checked come next"
        );
    }

    #[test]
    fn each_method_has_its_own_lookup_budget() {
        let (record, _) = split_fixture();
        let records: Vec<SplitRecord> = (0..(2 * MAX_TWIN_LOOKUPS as u8 + 4))
            .map(|n| SplitRecord {
                txid: Txid::from_str(&format!("{:02x}", n).repeat(32)).unwrap(),
                created_at: u64::from(n),
                // Signature splits are the newest, so they would come first.
                method: if usize::from(n) > MAX_TWIN_LOOKUPS + 1 {
                    Method::Signature
                } else {
                    Method::SplitKey
                },
                ..record.clone()
            })
            .collect();
        let candidates = twin_candidates(&records, &HashMap::new());
        for method in [Method::SplitKey, Method::Signature] {
            assert_eq!(
                candidates.iter().filter(|(_, m)| *m == method).count(),
                MAX_TWIN_LOOKUPS,
                "{:?}",
                method
            );
        }
    }

    #[test]
    fn only_a_newer_pass_applies() {
        let mut applied = 0;
        assert!(newer_pass(&mut applied, 2));
        assert!(!newer_pass(&mut applied, 1), "a slow older pass is dropped");
        assert!(!newer_pass(&mut applied, 2));
        assert!(newer_pass(&mut applied, 3));
        assert_eq!(applied, 3);
    }

    mod depth {
        use super::super::*;
        use crate::services::claim_observation::{FailureKind, FreshRead};
        use coincube_core::claim::BlockRef;
        use coincube_core::miniscript::bitcoin::{hashes::Hash, BlockHash};
        use reqwest::header::{HeaderMap, CACHE_CONTROL};
        use std::sync::Mutex;

        fn fresh<T>(chain: ChainId, value: T) -> FreshRead<T> {
            let mut headers = HeaderMap::new();
            headers.insert("x-cache", "BYPASS".parse().unwrap());
            headers.insert(CACHE_CONTROL, "private, no-store".parse().unwrap());
            FreshRead::from_response(chain, value, 1, &headers).unwrap()
        }

        /// Answers from one chain's view; records every request so tests
        /// can pin which chain, transaction and height were asked for.
        struct Source {
            /// Tip heights, one per `tip` call; the last repeats.
            tips: Vec<u64>,
            tx: TransactionObservation,
            at_height: BlockHash,
            /// Stamp replies with this chain instead of the requested one.
            answer_as: Option<ChainId>,
            asked: Mutex<Vec<String>>,
        }

        impl Source {
            fn new(tip: u64, tx: TransactionObservation) -> Self {
                Self {
                    tips: vec![tip],
                    tx,
                    at_height: block_hash(),
                    answer_as: None,
                    asked: Mutex::new(Vec::new()),
                }
            }
            fn chain(&self, asked: ChainId) -> ChainId {
                self.answer_as.unwrap_or(asked)
            }
        }

        #[async_trait::async_trait]
        impl ObservationSource for Source {
            fn now(&self) -> i64 {
                1
            }
            async fn anchor(
                &self,
                _: ChainId,
            ) -> Result<crate::services::coincube::network_anchor::NetworkAnchorStatus, FailureKind>
            {
                Err(FailureKind::Unavailable)
            }
            async fn tip(&self, chain: ChainId) -> Result<FreshRead<BlockRef>, FailureKind> {
                let mut asked = self.asked.lock().unwrap();
                let n = asked.iter().filter(|a| a.starts_with("tip")).count();
                asked.push(format!("tip {:?}", chain));
                let height = *self.tips.get(n).or(self.tips.last()).unwrap();
                Ok(fresh(
                    self.chain(chain),
                    BlockRef {
                        height,
                        hash: BlockHash::from_byte_array([height as u8; 32]),
                    },
                ))
            }
            async fn transaction(
                &self,
                chain: ChainId,
                txid: Txid,
            ) -> Result<FreshRead<TransactionObservation>, FailureKind> {
                self.asked
                    .lock()
                    .unwrap()
                    .push(format!("tx {:?} {}", chain, txid));
                Ok(fresh(self.chain(chain), self.tx))
            }
            async fn hash_at_height(
                &self,
                chain: ChainId,
                height: u64,
            ) -> Result<FreshRead<BlockHash>, FailureKind> {
                self.asked
                    .lock()
                    .unwrap()
                    .push(format!("hash {:?} {}", chain, height));
                Ok(fresh(self.chain(chain), self.at_height))
            }
        }

        fn txid() -> Txid {
            Txid::from_byte_array([4; 32])
        }

        fn block_hash() -> BlockHash {
            BlockHash::from_byte_array([1; 32])
        }

        fn confirmed_at(height: u64) -> TransactionObservation {
            TransactionObservation::Confirmed {
                txid: txid(),
                block: BlockRef {
                    height,
                    hash: block_hash(),
                },
            }
        }

        #[tokio::test]
        async fn depth_counts_from_the_fresh_tip_on_the_asked_chain() {
            let source = Source::new(105, confirmed_at(100));
            assert_eq!(
                depth_on(&source, ChainId::BitcoinBlake2b, txid()).await,
                Some(6)
            );
            assert_eq!(
                *source.asked.lock().unwrap(),
                vec![
                    "tip BitcoinBlake2b".to_string(),
                    format!("tx BitcoinBlake2b {}", txid()),
                    "hash BitcoinBlake2b 100".to_string(),
                    "tip BitcoinBlake2b".to_string(),
                ]
            );
        }

        #[tokio::test]
        async fn a_block_off_the_best_chain_has_no_depth() {
            let mut source = Source::new(105, confirmed_at(100));
            source.at_height = BlockHash::from_byte_array([2; 32]);
            assert_eq!(
                depth_on(&source, ChainId::BitcoinBlake2b, txid()).await,
                None
            );
        }

        #[tokio::test]
        async fn a_tip_that_moves_during_the_reads_has_no_depth() {
            let mut source = Source::new(105, confirmed_at(100));
            source.tips = vec![105, 103];
            assert_eq!(
                depth_on(&source, ChainId::BitcoinBlake2b, txid()).await,
                None
            );
        }

        #[tokio::test]
        async fn a_reply_from_another_chain_has_no_depth() {
            let mut source = Source::new(105, confirmed_at(100));
            source.answer_as = Some(ChainId::Bitcoin);
            assert_eq!(
                depth_on(&source, ChainId::BitcoinBlake2b, txid()).await,
                None
            );
        }

        #[tokio::test]
        async fn another_transaction_or_a_block_above_the_tip_has_no_depth() {
            let other = TransactionObservation::Confirmed {
                txid: Txid::from_byte_array([5; 32]),
                block: BlockRef {
                    height: 100,
                    hash: block_hash(),
                },
            };
            for (tip, tx) in [(105, other), (99, confirmed_at(100))].iter().copied() {
                let source = Source::new(tip, tx);
                assert_eq!(
                    depth_on(&source, ChainId::BitcoinBlake2b, txid()).await,
                    None
                );
            }
        }

        #[tokio::test]
        async fn unconfirmed_or_absent_has_no_depth() {
            for tx in [
                TransactionObservation::Absent,
                TransactionObservation::Unconfirmed { txid: txid() },
            ]
            .iter()
            .copied()
            {
                let source = Source::new(105, tx);
                assert_eq!(
                    depth_on(&source, ChainId::BitcoinBlake2b, txid()).await,
                    None
                );
            }
        }
    }

    #[test]
    fn construction_must_spend_exactly_the_request_into_one_owned_output() {
        let (_, split) = split_fixture();
        let requested: Vec<_> = split.input.iter().map(|i| i.previous_output).collect();
        let mut psbt = Psbt::from_unsigned_tx(split).unwrap();
        assert!(
            check_construction(&psbt, &requested).is_err(),
            "output not owned"
        );
        psbt.outputs[0].bip32_derivation.insert(
            coincube_core::miniscript::bitcoin::secp256k1::PublicKey::from_str(
                "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5",
            )
            .unwrap(),
            (Default::default(), Default::default()),
        );
        assert!(check_construction(&psbt, &requested).is_ok());
        assert!(check_construction(&psbt, &requested[..1]).is_err());
    }
}
