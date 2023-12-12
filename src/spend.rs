use crate::descriptors;

use std::{collections::BTreeMap, convert::TryInto, fmt};

pub use bdk_coin_select::InsufficientFunds;
use bdk_coin_select::{
    change_policy, metrics::LowestFee, Candidate, CoinSelector, DrainWeights, FeeRate, Target,
    TXIN_BASE_WEIGHT,
};
use miniscript::bitcoin::{
    self,
    absolute::{Height, LockTime},
    bip32,
    constants::WITNESS_SCALE_FACTOR,
    psbt::{Input as PsbtIn, Output as PsbtOut, Psbt},
    secp256k1,
};

/// We would never create a transaction with an output worth less than this.
/// That's 1$ at 20_000$ per BTC.
pub const DUST_OUTPUT_SATS: u64 = 5_000;

/// Long-term feerate (sats/vb) used for coin selection considerations.
pub const LONG_TERM_FEERATE_VB: f32 = 10.0;

/// Assume that paying more than 1BTC in fee is a bug.
pub const MAX_FEE: u64 = bitcoin::blockdata::constants::COIN_VALUE;

/// Assume that paying more than 1000sat/vb in feerate is a bug.
pub const MAX_FEERATE: u64 = 1_000;

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
}

impl fmt::Display for SpendCreationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::InvalidFeerate(sats_vb) => write!(f, "Invalid feerate: {} sats/vb.", sats_vb),
            Self::InvalidOutputValue(amount) => write!(f, "Invalid output value '{}'.", amount),
            Self::InsaneFees(info) => write!(
                f,
                "We assume transactions with a fee larger than {} sats or a feerate larger than {} sats/vb are a mistake. \
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
        }
    }
}

impl std::error::Error for SpendCreationError {}

// Sanity check the value of a transaction output.
fn check_output_value(value: bitcoin::Amount) -> Result<(), SpendCreationError> {
    // NOTE: the network parameter isn't used upstream
    if value.to_sat() > bitcoin::blockdata::constants::MAX_MONEY
        || value.to_sat() < DUST_OUTPUT_SATS
    {
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
        if psbtin.bip32_derivation.is_empty() {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()));
        }
        value_in += psbtin
            .witness_utxo
            .as_ref()
            .ok_or_else(|| SpendCreationError::SanityCheckFailure(psbt.clone()))?
            .value;
    }

    // Compute the output value and check the absolute fee isn't insane.
    let value_out: u64 = tx.output.iter().map(|o| o.value).sum();
    let abs_fee = value_in
        .checked_sub(value_out)
        .ok_or(SpendCreationError::InsaneFees(InsaneFeeInfo::NegativeFee))?;
    if abs_fee > MAX_FEE {
        return Err(SpendCreationError::InsaneFees(InsaneFeeInfo::TooHighFee(
            abs_fee,
        )));
    }

    // Check the feerate isn't insane.
    // Add weights together before converting to vbytes to avoid rounding up multiple times
    // and increasing the result, which could lead to the feerate in sats/vb falling below 1.
    let tx_wu = tx.weight().to_wu() + (spent_desc.max_sat_weight() * tx.input.len()) as u64;
    let tx_vb = tx_wu
        .checked_add(WITNESS_SCALE_FACTOR as u64 - 1)
        .unwrap()
        .checked_div(WITNESS_SCALE_FACTOR as u64)
        .unwrap();
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
        if txo.value < txo.script_pubkey.dust_value().to_sat() {
            return Err(SpendCreationError::SanityCheckFailure(psbt.clone()));
        }
    }

    Ok(())
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
}

/// Metric based on [`LowestFee`] that aims to minimize transaction fees
/// with the additional option to only find solutions with a change output.
///
/// Using this metric with `must_have_change: false` is equivalent to using
/// [`LowestFee`].
struct LowestFeeChangeCondition<'c, C> {
    /// The underlying [`LowestFee`] metric to use.
    pub lowest_fee: LowestFee<'c, C>,
    /// If `true`, only solutions with change will be found.
    pub must_have_change: bool,
}

impl<'c, C> bdk_coin_select::BnbMetric for LowestFeeChangeCondition<'c, C>
where
    for<'a, 'b> C: Fn(&'b CoinSelector<'a>, Target) -> bdk_coin_select::Drain,
{
    fn score(&mut self, cs: &CoinSelector<'_>) -> Option<bdk_coin_select::float::Ordf32> {
        let drain = (self.lowest_fee.change_policy)(cs, self.lowest_fee.target);
        if drain.is_none() && self.must_have_change {
            None
        } else {
            // This is a temporary partial fix for https://github.com/bitcoindevkit/coin-select/issues/6
            // until it has been fixed upstream.
            // TODO: Revert this change once upstream fix has been made.
            // When calculating the score, the excess should be added to changeless solutions instead of
            // those with change.
            // Given a solution has been found, this fix adds or removes the excess to its incorrectly
            // calculated score as required so that two changeless solutions can be differentiated
            // if one has higher excess (and therefore pays a higher fee).
            // Note that the `bound` function is also affected by this bug, which could mean some branches
            // are not considered when running BnB, but at least this fix will mean the score for those
            // solutions that are found is correct.
            self.lowest_fee.score(cs).map(|score| {
                // See https://github.com/bitcoindevkit/coin-select/blob/29b187f5509a01ba125a0354f6711e317bb5522a/src/metrics/lowest_fee.rs#L35-L45
                assert!(cs.selected_value() >= self.lowest_fee.target.value);
                let excess = (cs.selected_value() - self.lowest_fee.target.value) as f32;
                bdk_coin_select::float::Ordf32(if drain.is_none() {
                    score.0 + excess
                } else {
                    score.0 - excess
                })
            })
        }
    }

    fn bound(&mut self, cs: &CoinSelector<'_>) -> Option<bdk_coin_select::float::Ordf32> {
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
/// `min_fee` is the minimum fee (in sats) that the selection must have.
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
    min_fee: u64,
    max_sat_weight: u32,
    must_have_change: bool,
) -> Result<(Vec<CandidateCoin>, bitcoin::Amount), InsufficientFunds> {
    let out_value_nochange = base_tx.output.iter().map(|o| o.value).sum();

    // Create the coin selector from the given candidates. NOTE: the coin selector keeps track
    // of the original ordering of candidates so we can select any mandatory candidates using their
    // original indices.
    let base_weight: u32 = base_tx
        .weight()
        .to_wu()
        .try_into()
        .expect("Transaction weight must fit in u32");
    let max_input_weight = TXIN_BASE_WEIGHT + max_sat_weight;
    let candidates: Vec<Candidate> = candidate_coins
        .iter()
        .map(|cand| Candidate {
            input_count: 1,
            value: cand.amount.to_sat(),
            weight: max_input_weight,
            is_segwit: true, // We only support receiving on Segwit scripts.
        })
        .collect();
    let mut selector = CoinSelector::new(&candidates, base_weight);
    for (i, cand) in candidate_coins.iter().enumerate() {
        if cand.must_select {
            // It's fine because the index passed to `select` refers to the original candidates ordering
            // (and in any case the ordering of candidates is still the same in the coin selector).
            selector.select(i);
        }
    }

    // Now set the change policy. We use a policy which ensures no change output is created with a
    // lower value than our custom dust limit. NOTE: the change output weight must account for a
    // potential difference in the size of the outputs count varint. This is why we take the whole
    // change txo as argument and compute the weight difference below.
    let long_term_feerate = FeeRate::from_sat_per_vb(LONG_TERM_FEERATE_VB);
    let drain_weights = DrainWeights {
        output_weight: {
            let mut tx_with_change = base_tx;
            tx_with_change.output.push(change_txo);
            tx_with_change
                .weight()
                .to_wu()
                .checked_sub(base_weight.into())
                .expect("base_weight can't be larger")
                .try_into()
                .expect("tx size must always fit in u32")
        },
        spend_weight: max_input_weight,
    };
    let change_policy =
        change_policy::min_value_and_waste(drain_weights, DUST_OUTPUT_SATS, long_term_feerate);

    // Finally, run the coin selection algorithm. We use an opportunistic BnB and if it couldn't
    // find any solution we fall back to selecting coins by descending value.
    let target = Target {
        value: out_value_nochange,
        feerate: FeeRate::from_sat_per_vb(feerate_vb),
        min_fee,
    };
    let lowest_fee = LowestFee {
        target,
        long_term_feerate,
        change_policy: &change_policy,
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
    #[cfg(debug)]
    let bnb_rounds = bnb_rounds / 1_000;
    if let Err(e) = selector.run_bnb(lowest_fee_change_cond, bnb_rounds) {
        log::warn!(
            "Coin selection error: '{}'. Selecting coins by descending value per weight unit...",
            e.to_string()
        );
        selector.sort_candidates_by_descending_value_pwu();
        // Select more coins until target is met and change condition satisfied.
        loop {
            let drain = change_policy(&selector, target);
            if selector.is_target_met(target, drain) && (drain.is_some() || !must_have_change) {
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
    let drain = change_policy(&selector, target);
    let change_amount = bitcoin::Amount::from_sat(drain.value);
    Ok((
        selector
            .selected_indices()
            .iter()
            .map(|i| candidate_coins[*i])
            .collect(),
        change_amount,
    ))
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
    /// The (target feerate, minimum absolute fees) for this transactions. Both in sats.
    Rbf(u64, u64),
}

pub struct CreateSpendRes {
    /// The created PSBT.
    pub psbt: Psbt,
    /// Whether the created PSBT has a change output.
    pub has_change: bool,
}

/// Create a PSBT for a transaction spending some, or all, of `candidate_coins` to `destinations`.
/// Important information for signers will be populated. Will refuse to create outputs worth less
/// than `DUST_OUTPUT_SATS`. Will refuse to create a transaction paying more than `MAX_FEE`
/// satoshis in fees or whose feerate is larger than `MAX_FEERATE` sats/vb.
///
/// More about the parameters:
/// * `main_descriptor`: the multipath Liana descriptor, used to derive the addresses of the
/// candidate coins.
/// * `secp`: necessary to derive data from the descriptor.
/// * `tx_getter`: an interface to get the wallet transaction for the prevouts of the transaction.
/// Wouldn't be necessary if we only spent Taproot coins.
/// * `destinations`: a list of addresses and amounts, one per recipient i.e. per output in the
/// transaction created. If empty all the `candidate_coins` get spent and a single change output
/// is created to the provided `change_addr`. Can be used to sweep all, or some, coins from the
/// wallet.
/// * `candidate_coins`: a list of coins to consider including as input of the transaction. If
/// `destinations` is empty, they will all be included as inputs of the transaction. Otherwise, a
/// coin selection algorithm will be run to spend the most efficient subset of them to meet the
/// `destinations` requirements.
/// * `fees`: the target feerate (in sats/vb) and, if necessary, minimum absolute fee for this tx.
/// * `change_addr`: the address to use for a change output if we need to create one. Can be set to
/// an external address (if combined with an empty list of `destinations` it's useful to sweep some
/// or all coins of a wallet to an external address).
pub fn create_spend(
    main_descriptor: &descriptors::LianaDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    tx_getter: &mut impl TxGetter,
    destinations: &[(SpendOutputAddress, bitcoin::Amount)],
    candidate_coins: &[CandidateCoin],
    fees: SpendTxFees,
    change_addr: SpendOutputAddress,
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

    let (feerate_vb, min_fee) = match fees {
        SpendTxFees::Regular(feerate) => (feerate, 0),
        SpendTxFees::Rbf(feerate, fee) => (feerate, fee),
    };
    let is_self_send = destinations.is_empty();
    if feerate_vb < 1 {
        return Err(SpendCreationError::InvalidFeerate(feerate_vb));
    }

    // Create transaction with no inputs and no outputs.
    let mut tx = bitcoin::Transaction {
        version: 2,
        lock_time: LockTime::Blocks(Height::ZERO), // TODO: randomized anti fee sniping
        input: Vec::with_capacity(candidate_coins.iter().filter(|c| c.must_select).count()),
        output: Vec::with_capacity(destinations.len()),
    };
    // Add the destinations outputs to the transaction and PSBT. At the same time
    // sanity check each output's value.
    let mut psbt_outs = Vec::with_capacity(destinations.len());
    for (address, amount) in destinations {
        check_output_value(*amount)?;

        tx.output.push(bitcoin::TxOut {
            value: amount.to_sat(),
            script_pubkey: address.addr.script_pubkey(),
        });
        // If it's an address of ours, signal it as change to signing devices by adding the
        // BIP32 derivation path to the PSBT output.
        let bip32_derivation = if let Some(AddrInfo { index, is_change }) = address.info {
            let desc = if is_change {
                main_descriptor.change_descriptor()
            } else {
                main_descriptor.receive_descriptor()
            };
            desc.derive(index, secp).bip32_derivations()
        } else {
            Default::default()
        };
        psbt_outs.push(PsbtOut {
            bip32_derivation,
            ..PsbtOut::default()
        });
    }
    assert_eq!(tx.output.is_empty(), is_self_send);

    // Now compute whether we'll need a change output while automatically selecting coins to be
    // used as input if necessary.
    // We need to get the size of a potential change output to select coins / determine whether
    // we should include one, so get the change address and create a dummy txo for this purpose.
    let mut change_txo = bitcoin::TxOut {
        value: std::u64::MAX,
        script_pubkey: change_addr.addr.script_pubkey(),
    };
    // Now select the coins necessary using the provided candidates and determine whether
    // there is any leftover to create a change output.
    let (selected_coins, change_amount) = {
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
        .try_into()
        .expect("u16 must fit in f32");
        let max_sat_wu = main_descriptor
            .max_sat_weight()
            .try_into()
            .expect("Weight must fit in a u32");
        select_coins_for_spend(
            candidate_coins,
            tx.clone(),
            change_txo.clone(),
            feerate_vb,
            min_fee,
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
        let bip32_derivation = if let Some(AddrInfo { index, is_change }) = change_addr.info {
            let desc = if is_change {
                main_descriptor.change_descriptor()
            } else {
                main_descriptor.receive_descriptor()
            };
            desc.derive(index, secp).bip32_derivations()
        } else {
            Default::default()
        };

        // TODO: shuffle once we have Taproot
        change_txo.value = change_amount.to_sat();
        tx.output.push(change_txo);
        psbt_outs.push(PsbtOut {
            bip32_derivation,
            ..PsbtOut::default()
        });
    }

    // Iterate through selected coins and add necessary information to the PSBT inputs.
    let mut psbt_ins = Vec::with_capacity(selected_coins.len());
    for cand in &selected_coins {
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
        let coin_desc = derived_desc(secp, main_descriptor, cand);
        let witness_script = Some(coin_desc.witness_script());
        let witness_utxo = Some(bitcoin::TxOut {
            value: cand.amount.to_sat(),
            script_pubkey: coin_desc.script_pubkey(),
        });
        let non_witness_utxo = tx_getter.get_tx(&cand.outpoint.txid);
        let bip32_derivation = coin_desc.bip32_derivations();
        psbt_ins.push(PsbtIn {
            witness_script,
            witness_utxo,
            bip32_derivation,
            non_witness_utxo,
            ..PsbtIn::default()
        });
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
    sanity_check_psbt(main_descriptor, &psbt)?;
    // TODO: maybe check for common standardness rules (max size, ..)?

    Ok(CreateSpendRes { psbt, has_change })
}
