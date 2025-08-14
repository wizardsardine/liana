use crate::descriptors;

use std::{
    collections::{BTreeMap, HashMap},
    convert::TryInto,
    fmt,
    time::Duration,
};

pub use bdk_coin_select::InsufficientFunds;
use bdk_coin_select::{
    metrics::LowestFee, Candidate, ChangePolicy, CoinSelector, DrainWeights, FeeRate, Replace,
    Target, TargetFee, TargetOutputs, TXIN_BASE_WEIGHT,
};
use miniscript::bitcoin::{
    self,
    absolute::{Height, LockTime},
    bip32,
    constants::WITNESS_SCALE_FACTOR,
    psbt::{Input as PsbtIn, Output as PsbtOut, Psbt},
    secp256k1,
};
use serde::{Deserialize, Serialize};

/// We would never create a transaction with an output worth less than this.
/// That's 1$ at 20_000$ per BTC.
pub const DUST_OUTPUT_SATS: u64 = 5_000;

/// Long-term feerate (sats/vb) used for coin selection considerations.
pub const LONG_TERM_FEERATE_VB: f32 = 10.0;

/// Assume that paying more than 1BTC in fee is a bug.
pub const MAX_FEE: bitcoin::Amount = bitcoin::Amount::ONE_BTC;

/// Assume that paying more than 1000sat/vb in feerate is a bug.
pub const MAX_FEERATE: u64 = 1_000;

/// Do not set locktime if tip age in seconds is older than this.
// See also https://github.com/bitcoin/bitcoin/blob/ecd23656db174adef61d3bd753d02698c3528192/src/wallet/spend.cpp#L906.
pub const MAX_ANTI_FEE_SNIPING_TIP_AGE_SECS: u64 = 8 * 60 * 60; // 8 hours

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsaneFeeInfo {
    NegativeFee,
    InvalidFeerate,
    TooHighFee(u64),
    TooHighFeerate(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpendCreationError {
    InvalidFeerate(/* sats/vb */ u64),
    InvalidOutputValue(bitcoin::Amount),
    InsaneFees(InsaneFeeInfo),
    SanityCheckFailure(Psbt),
    FetchingTransaction(bitcoin::OutPoint),
    CoinSelection(InsufficientFunds),
    // TODO: wrap more specific error
    InvalidBip21,
}

impl fmt::Display for SpendCreationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InvalidFeerate(sats_vb) => write!(f, "Invalid feerate: {} sats/vb.", sats_vb),
            Self::InvalidOutputValue(amount) => write!(f, "Invalid output value '{}'.", amount),
            Self::InsaneFees(info) => write!(
                f,
                "We assume transactions with a fee larger than {} or a feerate larger than {} sats/vb are a mistake. \
                The created transaction {}.",
                MAX_FEE,
                MAX_FEERATE,
                match info {
                    InsaneFeeInfo::NegativeFee => "would have a negative fee".to_string(),
                    InsaneFeeInfo::TooHighFee(f) => format!("{} sats in fees", f),
                    InsaneFeeInfo::InvalidFeerate => "would have an invalid feerate".to_string(),
                    InsaneFeeInfo::TooHighFeerate(r) => format!("has a feerate of {} sats/vb", r),
                },
            ),
            Self::FetchingTransaction(op) => {
                write!(f, "Could not fetch transaction for coin {}", op)
            }
            Self::CoinSelection(e) => write!(f, "Coin selection error: '{}'", e),
            Self::SanityCheckFailure(psbt) => write!(
                f,
                "BUG! Please report this. Failed sanity checks for PSBT '{}'.",
                psbt
            ),
            Self::InvalidBip21 => write!(f, "Invalid BIP21"),
        }
    }
}

impl std::error::Error for SpendCreationError {}

// Sanity check the value of a transaction output.
fn check_output_value(value: bitcoin::Amount) -> Result<(), SpendCreationError> {
    if value > bitcoin::Amount::MAX_MONEY || value.to_sat() < DUST_OUTPUT_SATS {
        Err(SpendCreationError::InvalidOutputValue(value))
    } else {
        Ok(())
    }
}

// Apply some sanity checks on a created transaction's PSBT.
// TODO: add more sanity checks from revault_tx
fn sanity_check_psbt(
    spent_desc: &descriptors::LianaDescriptor,
    psbt: &Psbt,
    use_primary_path: bool,
) -> Result<(), SpendCreationError> {
    let tx = &psbt.unsigned_tx;

    // Must have as many in/out in the PSBT and Bitcoin tx.
    if psbt.inputs.len() != tx.input.len()
        || psbt.outputs.len() != tx.output.len()
        || tx.output.is_empty()
    {
        return Err(SpendCreationError::SanityCheckFailure(psbt.clone()));
    }

    // Compute the transaction input value, checking all PSBT inputs have the derivation
    // index set for signing devices to recognize them as ours.
    let mut value_in = 0;
    for psbtin in psbt.inputs.iter() {
        if psbtin.bip32_derivation.is_empty() && psbtin.tap_key_origins.is_empty() {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()));
        }
        value_in += psbtin
            .witness_utxo
            .as_ref()
            .ok_or_else(|| SpendCreationError::SanityCheckFailure(psbt.clone()))?
            .value
            .to_sat();
    }

    // Compute the output value and check the absolute fee isn't insane.
    let value_out: u64 = tx.output.iter().map(|o| o.value.to_sat()).sum();
    let abs_fee = value_in
        .checked_sub(value_out)
        .ok_or(SpendCreationError::InsaneFees(InsaneFeeInfo::NegativeFee))?;
    if abs_fee > MAX_FEE.to_sat() {
        return Err(SpendCreationError::InsaneFees(InsaneFeeInfo::TooHighFee(
            abs_fee,
        )));
    }

    // Check the feerate isn't insane.
    let tx_vb = spent_desc.unsigned_tx_max_vbytes(tx, use_primary_path);
    let feerate_sats_vb = abs_fee
        .checked_div(tx_vb)
        .ok_or(SpendCreationError::InsaneFees(
            InsaneFeeInfo::InvalidFeerate,
        ))?;
    if !(1..=MAX_FEERATE).contains(&feerate_sats_vb) {
        return Err(SpendCreationError::InsaneFees(
            InsaneFeeInfo::TooHighFeerate(feerate_sats_vb),
        ));
    }

    // Check for dust outputs
    for txo in psbt.unsigned_tx.output.iter() {
        if txo.value < txo.script_pubkey.minimal_non_dust() {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()));
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AncestorInfo {
    pub vsize: u64,
    pub fee: u32,
}

/// A candidate for coin selection when creating a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CandidateCoin {
    /// Unique identifier of this coin.
    pub outpoint: bitcoin::OutPoint,
    /// The value of this coin.
    pub amount: bitcoin::Amount,
    /// The derivation index used to generate the scriptpubkey of this coin.
    pub deriv_index: bip32::ChildNumber,
    /// Whether this coin pays to a scriptpubkey derived from the internal keychain.
    pub is_change: bool,
    /// Whether or not this coin must be selected by the coin selection algorithm.
    pub must_select: bool,
    /// The nSequence field to set for an input spending this coin.
    pub sequence: Option<bitcoin::Sequence>,
    /// Information about in-mempool ancestors of the coin.
    pub ancestor_info: Option<AncestorInfo>,
}

/// A coin selection result.
///
/// A change output should only be added if `change_amount > 0`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoinSelectionRes {
    /// Selected candidates.
    pub selected: Vec<CandidateCoin>,
    /// Change amount that should be included according to the change policy used
    /// for selection.
    pub change_amount: bitcoin::Amount,
    /// Maximum change amount possible with the selection irrespective of any change
    /// policy.
    pub max_change_amount: bitcoin::Amount,
    /// Fee added to pay for ancestors at the target feerate.
    pub fee_for_ancestors: bitcoin::Amount,
}

/// Metric based on [`LowestFee`] that aims to minimize transaction fees
/// with the additional option to only find solutions with a change output.
///
/// Using this metric with `must_have_change: false` is equivalent to using
/// [`LowestFee`].
struct LowestFeeChangeCondition {
    /// The underlying [`LowestFee`] metric to use.
    pub lowest_fee: LowestFee,
    /// If `true`, only solutions with change will be found.
    pub must_have_change: bool,
}

impl bdk_coin_select::BnbMetric for LowestFeeChangeCondition {
    fn score(&mut self, cs: &CoinSelector) -> Option<bdk_coin_select::float::Ordf32> {
        let drain = cs.drain(self.lowest_fee.target, self.lowest_fee.change_policy);
        if drain.is_none() && self.must_have_change {
            None
        } else {
            self.lowest_fee.score(cs)
        }
    }

    fn bound(&mut self, cs: &CoinSelector) -> Option<bdk_coin_select::float::Ordf32> {
        self.lowest_fee.bound(cs)
    }

    fn requires_ordering_by_descending_value_pwu(&self) -> bool {
        self.lowest_fee.requires_ordering_by_descending_value_pwu()
    }
}

/// Select coins for spend.
///
/// Returns the selected coins and the change amount, which could be zero.
///
/// `candidate_coins` are the coins to consider for selection.
///
/// `base_tx` is the transaction to select coins for. It should be without any inputs
/// and without a change output, but with all non-change outputs added.
///
/// `change_txo` is the change output to add if needed (with any value).
///
/// `feerate_vb` is the minimum feerate (in sats/vb). Note that the selected coins
/// and change may result in a slightly lower feerate than this as the underlying
/// function instead uses a minimum feerate of `feerate_vb / 4.0` sats/wu.
///
/// If this is a replacement spend using RBF, then `replaced_fee` should be set to
/// the total fees (in sats) of the transaction(s) being replaced, including any
/// descendants, which will ensure that RBF rule 4 is satisfied.
/// Otherwise, it should be `None`.
///
/// `max_sat_weight` is the maximum weight difference of an input in the
/// transaction before and after satisfaction.
///
/// `must_have_change` indicates whether the transaction must have a change output.
/// If `true`, the returned change amount will be positive.
fn select_coins_for_spend(
    candidate_coins: &[CandidateCoin],
    base_tx: bitcoin::Transaction,
    change_txo: bitcoin::TxOut,
    feerate_vb: f32,
    replaced_fee: Option<u64>,
    max_sat_weight: u64,
    must_have_change: bool,
) -> Result<CoinSelectionRes, InsufficientFunds> {
    let out_value_nochange = base_tx.output.iter().map(|o| o.value.to_sat()).sum();
    let out_weight_nochange = {
        let mut total: u64 = 0;
        for output in &base_tx.output {
            let weight = output.weight().to_wu();
            total = total
                .checked_add(weight)
                .expect("sum of transaction outputs' weights must fit in u64");
        }
        total
    };
    let n_outputs_nochange = base_tx.output.len();
    let max_input_weight = TXIN_BASE_WEIGHT + max_sat_weight;
    // Get feerate as u64 for calculation relating to ancestor below.
    // We expect `feerate_vb` to be a positive integer, but take ceil()
    // just in case to be sure we pay enough for ancestors.
    let feerate_vb_u64 = feerate_vb.ceil() as u64;
    let witness_factor: u64 = WITNESS_SCALE_FACTOR
        .try_into()
        .expect("scale factor must fit in u64");
    // This will be used to store any extra weight added to candidates.
    let mut added_weights = HashMap::<bitcoin::OutPoint, u64>::with_capacity(candidate_coins.len());
    let candidates: Vec<Candidate> = candidate_coins
        .iter()
        .map(|cand| Candidate {
            input_count: 1,
            value: cand.amount.to_sat(),
            weight: {
                let extra = cand
                    .ancestor_info
                    .map(|info| {
                        // The implied ancestor vsize if the fee had been paid at our target feerate.
                        let ancestor_vsize_at_feerate = <u32 as Into<u64>>::into(info.fee)
                            .checked_div(feerate_vb_u64)
                            .expect("feerate is greater than zero");
                        // If the actual ancestor vsize is bigger than the implied vsize, we will need to
                        // pay the difference in order for the combined feerate to be at the target value.
                        // We multiply the vsize by 4 to get the ancestor weight, which is an upper bound
                        // on its true weight (vsize*4 - 3 <= weight <= vsize*4), to ensure we pay enough.
                        // Note that if candidates share ancestors, we may add this difference more than
                        // once in the resulting transaction.
                        info.vsize
                            .saturating_sub(ancestor_vsize_at_feerate)
                            .checked_mul(witness_factor)
                            .expect("weight difference must fit in u64")
                    })
                    .unwrap_or(0);
                // Store the extra weight for this candidate for use later on.
                // At the same time, make sure there are no duplicate outpoints.
                assert!(added_weights.insert(cand.outpoint, extra).is_none());
                max_input_weight
                    .checked_add(extra)
                    .expect("effective weight must fit in u64")
            },
            is_segwit: true, // We only support receiving on Segwit scripts.
        })
        .collect();
    let mut selector = CoinSelector::new(&candidates);
    for (i, cand) in candidate_coins.iter().enumerate() {
        if cand.must_select {
            // It's fine because the index passed to `select` refers to the original candidates ordering
            // (and in any case the ordering of candidates is still the same in the coin selector).
            selector.select(i);
        }
    }

    // Now set the change policy. We use a policy which ensures no change output is created with a
    // lower value than our custom dust limit. NOTE: the change output weight must not account for
    // a potential difference in the size of the outputs count varint.
    let feerate = FeeRate::from_sat_per_vb(feerate_vb);
    let long_term_feerate = FeeRate::from_sat_per_vb(LONG_TERM_FEERATE_VB);
    let change_output_weight = change_txo.weight().to_wu();
    let drain_weights = DrainWeights {
        output_weight: change_output_weight,
        spend_weight: max_input_weight,
        n_outputs: 1, // we only want a single change output
    };
    // As of bdk_coin_select v0.3.0, the min change value is exclusive so we must subtract 1.
    let change_min_value = DUST_OUTPUT_SATS.saturating_sub(1);
    let change_policy = ChangePolicy::min_value_and_waste(
        drain_weights,
        change_min_value,
        feerate,
        long_term_feerate,
    );

    // Finally, run the coin selection algorithm. We use an opportunistic BnB and if it couldn't
    // find any solution we fall back to selecting coins by descending value.
    let replace = replaced_fee.map(Replace::new);
    let target_fee = TargetFee {
        rate: feerate,
        replace,
    };
    let target_outputs = TargetOutputs {
        value_sum: out_value_nochange,
        weight_sum: out_weight_nochange,
        n_outputs: n_outputs_nochange,
    };
    let target = Target {
        fee: target_fee,
        outputs: target_outputs,
    };
    let lowest_fee = LowestFee {
        target,
        long_term_feerate,
        change_policy,
    };
    let lowest_fee_change_cond = LowestFeeChangeCondition {
        lowest_fee,
        must_have_change,
    };
    // Scale down the number of rounds to perform if there is too many candidates. If the binary
    // isn't optimized, scale it down further to avoid lags in hot loops.
    let bnb_rounds = match candidate_coins.len() {
        i if i >= 500 => 1_000,
        i if i >= 100 => 10_000,
        _ => 100_000,
    };
    #[cfg(debug_assertions)]
    let bnb_rounds = bnb_rounds / 1_000;
    if let Err(e) = selector.run_bnb(lowest_fee_change_cond, bnb_rounds) {
        log::debug!(
            "Coin selection error: '{}'. Selecting coins by descending value per weight unit...",
            e.to_string()
        );
        selector.sort_candidates_by_descending_value_pwu();
        // Select more coins until target is met and change condition satisfied.
        loop {
            let drain = selector.drain(target, change_policy);
            if selector.is_target_met_with_drain(target, drain)
                && (drain.is_some() || !must_have_change)
            {
                break;
            }
            if !selector.select_next() {
                // If the solution must have change, we calculate how much is missing from the current
                // selection in order for there to be a change output with the smallest possible value.
                let drain = if must_have_change {
                    bdk_coin_select::Drain {
                        weights: drain_weights,
                        value: DUST_OUTPUT_SATS,
                    }
                } else {
                    drain
                };
                let missing = selector.excess(target, drain).unsigned_abs();
                return Err(InsufficientFunds { missing });
            }
        }
    }
    // By now, selection is complete and we can check how much change to give according to our policy.
    let drain = selector.drain(target, change_policy);
    let change_amount = bitcoin::Amount::from_sat(drain.value);
    // Max available change is given by the excess when adding a change output with zero value.
    let drain_novalue = bdk_coin_select::Drain {
        weights: drain_weights,
        value: 0,
    };
    let max_change_amount = bitcoin::Amount::from_sat(
        selector
            .excess(target, drain_novalue)
            .max(0) // negative excess would mean insufficient funds to pay for change output
            .try_into()
            .expect("value is non-negative"),
    );
    let mut total_added_weight: u64 = 0;
    let selected = selector
        .selected_indices()
        .iter()
        .map(|i| candidate_coins[*i])
        .inspect(|cand| {
            total_added_weight = total_added_weight
                .checked_add(
                    *added_weights
                        .get(&cand.outpoint)
                        .expect("contains added weight for all candidates"),
                )
                .expect("should fit in u64")
        })
        .collect();
    // Calculate added fee based on the feerate in sats/wu, which is the feerate used for coin selection.
    let fee_for_ancestors =
        bitcoin::Amount::from_sat(((total_added_weight as f32) * feerate.spwu()).ceil() as u64);
    Ok(CoinSelectionRes {
        selected,
        change_amount,
        max_change_amount,
        fee_for_ancestors,
    })
}

// Get the derived descriptor for this coin
fn derived_desc(
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    desc: &descriptors::LianaDescriptor,
    coin: &CandidateCoin,
) -> descriptors::DerivedSinglePathLianaDesc {
    let desc = if coin.is_change {
        desc.change_descriptor()
    } else {
        desc.receive_descriptor()
    };
    desc.derive(coin.deriv_index, secp)
}

/// Get value to use for transaction nLockTime in order to
/// discourage fee sniping.
///
/// The approach follows that taken by Bitcoin Core:
/// - most of the time, the value returned will be the current
///   block height, but will randomly be up to 100 blocks earlier.
/// - if the current tip is more than [`MAX_ANTI_FEE_SNIPING_TIP_AGE_SECS`]
///   seconds old, a locktime value of 0 will be returned.
pub fn anti_fee_sniping_locktime(
    now: Duration,
    tip_height: u32,
    tip_time_secs: Option<u32>,
) -> LockTime {
    tip_time_secs
        .map(|tip_time| now.as_secs().saturating_sub(tip_time.into()))
        .filter(|tip_age| *tip_age <= MAX_ANTI_FEE_SNIPING_TIP_AGE_SECS)
        .map(|_| {
            // Randomly (approx 10% of cases) set locktime further back
            // using current time as source of randomness.
            let nanos = now.subsec_nanos();
            // Note this condition will fail if nano precision is not available
            // and so nothing will be subtracted.
            let delta = if nanos % 10 == 1 {
                (nanos % 1000) / 10 + 1 // a number in [1, 100]
            } else {
                0
            };
            let height = tip_height.saturating_sub(delta);
            LockTime::from_height(height)
                .expect("height is valid block height as it cannot be bigger than tip height")
        })
        .unwrap_or(LockTime::Blocks(Height::ZERO))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddrInfo {
    pub index: bip32::ChildNumber,
    pub is_change: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendOutputAddress {
    pub addr: bitcoin::Address,
    pub info: Option<AddrInfo>,
}

/// A trait for getting a wallet transaction by its txid.
pub trait TxGetter {
    /// Get a wallet transaction. Allows for a cache by making the access mutable.
    fn get_tx(&mut self, txid: &bitcoin::Txid) -> Option<bitcoin::Transaction>;
}

/// Specify the fee requirements for a transaction. In both cases set a target feerate in satoshi
/// per virtual byte. For RBF also set a minimum fee in satoshis for this transaction. See
/// https://github.com/bitcoin/bitcoin/blob/master/doc/policy/mempool-replacements.md for more
/// information about how it should be set.
pub enum SpendTxFees {
    /// The target feerate in sats/vb for this transaction.
    Regular(u64),
    /// The (target feerate in sats/vb, total fees in sats of transaction(s) to be replaced
    /// including descendants) for this transaction.
    Rbf(u64, u64),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CreateSpendWarning {
    ChangeAddedToFee(u64),
    AdditionalFeeForAncestors(u64),
}

impl fmt::Display for CreateSpendWarning {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CreateSpendWarning::ChangeAddedToFee(amt) => write!(
                f,
                "Dust UTXO. The minimal change output allowed by Liana is {} sats. \
                Instead of creating a change of {} sat{}, it was added to the \
                transaction fee. Select a larger input to avoid this from happening.",
                DUST_OUTPUT_SATS,
                amt,
                if *amt > 1 { "s" } else { "" },
            ),
            CreateSpendWarning::AdditionalFeeForAncestors(amt) => write!(
                f,
                "CPFP: an unconfirmed input was selected. The current transaction fee \
                was increased by {} sat{} to make the average feerate of both the input \
                and current transaction equal to the selected feerate.",
                amt,
                if *amt > 1 { "s" } else { "" },
            ),
        }
    }
}

pub struct CreateSpendRes {
    /// The created PSBT.
    pub psbt: Psbt,
    /// Whether the created PSBT has a change output.
    pub has_change: bool,
    /// Warnings relating to the PSBT.
    pub warnings: Vec<CreateSpendWarning>,
}

/// Create a PSBT for a transaction spending some, or all, of `candidate_coins` to `destinations`.
/// Important information for signers will be populated. Will refuse to create outputs worth less
/// than `DUST_OUTPUT_SATS`. Will refuse to create a transaction paying more than `MAX_FEE`
/// satoshis in fees or whose feerate is larger than `MAX_FEERATE` sats/vb.
///
/// More about the parameters:
/// * `main_descriptor`: the multipath Liana descriptor, used to derive the addresses of the
///   candidate coins.
/// * `secp`: necessary to derive data from the descriptor.
/// * `tx_getter`: an interface to get the wallet transaction for the prevouts of the transaction.
///   Wouldn't be necessary if we only spent Taproot coins.
/// * `destinations`: a list of addresses and amounts, one per recipient i.e. per output in the
///   transaction created. If empty all the `candidate_coins` get spent and a single change output
///   is created to the provided `change_addr`. Can be used to sweep all, or some, coins from the
///   wallet.
/// * `candidate_coins`: a list of coins to consider including as input of the transaction. If
///   `destinations` is empty, they will all be included as inputs of the transaction. Otherwise, a
///   coin selection algorithm will be run to spend the most efficient subset of them to meet the
///   `destinations` requirements.
/// * `fees`: the target feerate (in sats/vb) and, if necessary, minimum absolute fee for this tx.
/// * `change_addr`: the address to use for a change output if we need to create one. Can be set to
///   an external address (if combined with an empty list of `destinations` it's useful to sweep some
///   or all coins of a wallet to an external address).
/// * `locktime`: the locktime to use for the transaction.
#[allow(clippy::too_many_arguments)]
pub fn create_spend(
    main_descriptor: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    tx_getter: &mut impl TxGetter,
    destinations: &[(SpendOutputAddress, bitcoin::Amount)],
    candidate_coins: &[CandidateCoin],
    fees: SpendTxFees,
    change_addr: SpendOutputAddress,
    locktime: LockTime,
) -> Result<CreateSpendRes, SpendCreationError> {
    // This method does quite a few things. In addition, we support different modes (coin control
    // vs automated coin selection, self-spend, sweep, etc..) which make the logic a bit more
    // intricate. Here is a brief overview of what we're doing here:
    // 1. Create a transaction with all the target outputs (if this is a self-send, none are added
    //    at this step the only output will be added as a change output).
    // 2. Automatically select the coins if necessary and determine whether a change output will be
    //    necessary for this transaction from the set of (automatically or manually) selected
    //    coins. The output for a self-send is added there.  The change output is also (ab)used to
    //    implement a "sweep" functionality. We allow to set it to an external address to send all
    //    the inputs' value minus the fee and the
    //    other output's value to a specific, external, address.
    // 3. Add the selected coins as inputs to the transaction.
    // 4. Finalize the PSBT and sanity check it before returning it.

    let mut warnings = Vec::new();
    let (feerate_vb, replaced_fee) = match fees {
        SpendTxFees::Regular(feerate) => (feerate, None),
        SpendTxFees::Rbf(feerate, fee) => (feerate, Some(fee)),
    };
    let is_self_send = destinations.is_empty();
    if feerate_vb < 1 {
        return Err(SpendCreationError::InvalidFeerate(feerate_vb));
    }

    // Create transaction with no inputs and no outputs.
    let mut tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: locktime,
        input: Vec::with_capacity(candidate_coins.iter().filter(|c| c.must_select).count()),
        output: Vec::with_capacity(destinations.len()),
    };
    // Add the destinations outputs to the transaction and PSBT. At the same time
    // sanity check each output's value.
    let mut psbt_outs = Vec::with_capacity(destinations.len());
    for (address, amount) in destinations {
        check_output_value(*amount)?;

        tx.output.push(bitcoin::TxOut {
            value: *amount,
            script_pubkey: address.addr.script_pubkey(),
        });
        // If it's an address of ours, signal it as change to signing devices by adding the
        // BIP32 derivation path to the PSBT output.
        let mut psbt_out = PsbtOut::default();
        if let Some(AddrInfo { index, is_change }) = address.info {
            let desc = if is_change {
                main_descriptor.change_descriptor()
            } else {
                main_descriptor.receive_descriptor()
            };
            desc.derive(index, secp)
                .update_change_psbt_out(&mut psbt_out)
        }
        psbt_outs.push(psbt_out);
    }
    assert_eq!(tx.output.is_empty(), is_self_send);

    // Now compute whether we'll need a change output while automatically selecting coins to be
    // used as input if necessary.
    // We need to get the size of a potential change output to select coins / determine whether
    // we should include one, so get the change address and create a dummy txo for this purpose.
    let mut change_txo = bitcoin::TxOut {
        value: bitcoin::Amount::MAX,
        script_pubkey: change_addr.addr.script_pubkey(),
    };
    // If no candidates have relative locktime, then we should use the primary spending path.
    // Note we set this value before actually selecting the coins, but we expect either all
    // candidates or none to have relative locktime sequence so this is fine.
    let use_primary_path = !candidate_coins
        .iter()
        .filter_map(|cand| cand.sequence)
        .any(|seq| seq.is_relative_lock_time());
    // Now select the coins necessary using the provided candidates and determine whether
    // there is any leftover to create a change output.
    let CoinSelectionRes {
        selected,
        change_amount,
        max_change_amount,
        fee_for_ancestors,
    } = {
        // At this point the transaction still has no input and no change output, as expected
        // by the coins selection helper function.
        assert!(tx.input.is_empty());
        assert_eq!(tx.output.len(), destinations.len());
        // TODO: Introduce general conversion error type.
        let feerate_vb: f32 = {
            let fr: u16 = feerate_vb.try_into().map_err(|_| {
                SpendCreationError::InsaneFees(InsaneFeeInfo::TooHighFeerate(feerate_vb))
            })?;
            fr
        }
        .into();
        let max_sat_wu = main_descriptor
            .max_sat_weight(use_primary_path)
            .try_into()
            .expect("Weight must fit in a u64");
        select_coins_for_spend(
            candidate_coins,
            tx.clone(),
            change_txo.clone(),
            feerate_vb,
            replaced_fee,
            max_sat_wu,
            is_self_send,
        )
        .map_err(SpendCreationError::CoinSelection)?
    };
    // If necessary, add a change output.
    // For a self-send, coin selection will only find solutions with change and will otherwise
    // return an error. In any case, the PSBT sanity check will catch a transaction with no outputs.
    let has_change = change_amount.to_sat() > 0;
    if has_change {
        check_output_value(change_amount)?;

        // If the change address is ours, tell the signers by setting the BIP32 derivations in the
        // PSBT output.
        let mut psbt_out = PsbtOut::default();
        if let Some(AddrInfo { index, is_change }) = change_addr.info {
            let desc = if is_change {
                main_descriptor.change_descriptor()
            } else {
                main_descriptor.receive_descriptor()
            };
            desc.derive(index, secp)
                .update_change_psbt_out(&mut psbt_out);
        }

        // TODO: shuffle once we have Taproot
        change_txo.value = change_amount;
        tx.output.push(change_txo);
        psbt_outs.push(psbt_out);
    } else if max_change_amount.to_sat() > 0 {
        warnings.push(CreateSpendWarning::ChangeAddedToFee(
            max_change_amount.to_sat(),
        ));
    }

    if fee_for_ancestors.to_sat() > 0 {
        warnings.push(CreateSpendWarning::AdditionalFeeForAncestors(
            fee_for_ancestors.to_sat(),
        ));
    }

    // Iterate through selected coins and add necessary information to the PSBT inputs.
    let mut psbt_ins = Vec::with_capacity(selected.len());
    for cand in &selected {
        let sequence = cand
            .sequence
            .unwrap_or(bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME);
        tx.input.push(bitcoin::TxIn {
            previous_output: cand.outpoint,
            sequence,
            // TODO: once we move to Taproot, anti-fee-sniping using nSequence
            ..bitcoin::TxIn::default()
        });

        // Populate the PSBT input with the information needed by signers.
        let mut psbt_in = PsbtIn::default();
        let coin_desc = derived_desc(secp, main_descriptor, cand);
        coin_desc.update_psbt_in(&mut psbt_in);
        psbt_in.witness_utxo = Some(bitcoin::TxOut {
            value: cand.amount,
            script_pubkey: coin_desc.script_pubkey(),
        });
        if !main_descriptor.is_taproot() {
            psbt_in.non_witness_utxo = tx_getter.get_tx(&cand.outpoint.txid);
        }
        psbt_ins.push(psbt_in);
    }

    // Finally, create the PSBT with all inputs and outputs, sanity check it and return it.
    let psbt = Psbt {
        unsigned_tx: tx,
        version: 0,
        xpub: BTreeMap::new(),
        proprietary: BTreeMap::new(),
        unknown: BTreeMap::new(),
        inputs: psbt_ins,
        outputs: psbt_outs,
    };
    sanity_check_psbt(main_descriptor, &psbt, use_primary_path)?;
    // TODO: maybe check for common standardness rules (max size, ..)?

    Ok(CreateSpendRes {
        psbt,
        has_change,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use miniscript::bitcoin::absolute::{Height, LockTime};

    #[test]
    fn test_anti_fee_sniping_locktime() {
        // If we have no tip time, locktime is 0.
        assert_eq!(
            anti_fee_sniping_locktime(Duration::from_secs(100), 123_456, None),
            LockTime::Blocks(Height::ZERO)
        );

        // If tip time is too old, locktime is 0.
        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(100_000),
                123_456,
                Some(100_000 - (8 * 60 * 60) - 1)
            ),
            LockTime::Blocks(Height::ZERO)
        );

        // If tip age is exactly the max threshold, we set locktime.
        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(100_000),
                123_456,
                Some(100_000 - (8 * 60 * 60))
            ),
            LockTime::from_height(123_456).unwrap()
        );

        // If tip time is later than now, we set locktime depending on nanos.
        // If nanos are 0, set to current height.
        assert_eq!(
            anti_fee_sniping_locktime(Duration::from_secs(50_000), 123_456, Some(100_000)),
            LockTime::from_height(123_456).unwrap()
        );

        // We might set locktime earlier than current height.
        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(50_000) + Duration::from_nanos(1),
                123_456,
                Some(100_000)
            ),
            LockTime::from_height(123_455).unwrap() // subtract 1
        );

        // If tip time is older than now, we also vary the locktime depending on current nanos.
        // If nanos are truncated or 0, set locktime to current height.
        assert_eq!(
            anti_fee_sniping_locktime(Duration::from_secs(100), 123_456, Some(100)),
            LockTime::from_height(123_456).unwrap() // subtract 1
        );

        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(100) + Duration::from_nanos(1),
                123_456,
                Some(100)
            ),
            LockTime::from_height(123_455).unwrap() // subtract 1
        );

        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(100) + Duration::from_nanos(10_000_041),
                123_456,
                Some(100)
            ),
            LockTime::from_height(123_451).unwrap() // subtract 5
        );

        // If nanos % 10 != 1, don't subtract anything.
        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(100) + Duration::from_nanos(10_000_040),
                123_456,
                Some(100)
            ),
            LockTime::from_height(123_456).unwrap() // subtract 0
        );

        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(100) + Duration::from_nanos(100_000_891),
                123_456,
                Some(100)
            ),
            LockTime::from_height(123_366).unwrap() // subtract 90
        );

        // We would subtract 90, but current height is 56, so return locktime of 0.
        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(100) + Duration::from_nanos(100_000_891),
                56,
                Some(100)
            ),
            LockTime::Blocks(Height::ZERO)
        );

        // If block height is 91, we can now subtract 90.
        assert_eq!(
            anti_fee_sniping_locktime(
                Duration::from_secs(100) + Duration::from_nanos(100_000_891),
                91,
                Some(100)
            ),
            LockTime::from_height(1).unwrap() // subtract 90
        );
    }
}
