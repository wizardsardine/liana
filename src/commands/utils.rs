use bdk_coin_select::{
    change_policy, metrics::LowestFee, Candidate, CoinSelector, DrainWeights, FeeRate,
    InsufficientFunds, Target, TXIN_BASE_WEIGHT,
};
use log::warn;
use std::{convert::TryInto, str::FromStr};

use miniscript::bitcoin::{self, consensus, hashes::hex::FromHex};
use serde::{de, Deserialize, Deserializer, Serializer};

use crate::database::Coin;

use super::{CandidateCoin, DUST_OUTPUT_SATS, LONG_TERM_FEERATE_VB};

pub fn deser_fromstr<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    let string = String::deserialize(deserializer)?;
    T::from_str(&string).map_err(de::Error::custom)
}

pub fn ser_to_string<T: std::fmt::Display, S: Serializer>(
    field: T,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&field.to_string())
}

/// Deserialize an address from string, assuming the network was checked.
pub fn deser_addr_assume_checked<'de, D>(deserializer: D) -> Result<bitcoin::Address, D::Error>
where
    D: Deserializer<'de>,
{
    let string = String::deserialize(deserializer)?;
    bitcoin::Address::from_str(&string)
        .map(|addr| addr.assume_checked())
        .map_err(de::Error::custom)
}

/// Serialize an amount as sats
pub fn ser_amount<S: Serializer>(amount: &bitcoin::Amount, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(amount.to_sat())
}

/// Deserialize an amount from sats
pub fn deser_amount_from_sats<'de, D>(deserializer: D) -> Result<bitcoin::Amount, D::Error>
where
    D: Deserializer<'de>,
{
    let a = u64::deserialize(deserializer)?;
    Ok(bitcoin::Amount::from_sat(a))
}

pub fn ser_hex<S, T>(t: T, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: consensus::Encodable,
{
    s.serialize_str(&consensus::encode::serialize_hex(&t))
}

pub fn deser_hex<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: consensus::Decodable,
{
    let s = String::deserialize(d)?;
    let s = Vec::from_hex(&s).map_err(de::Error::custom)?;
    consensus::deserialize(&s).map_err(de::Error::custom)
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
/// `max_sat_weight` is the maximum size difference (in vb) of
/// an input in the transaction before and after satisfaction.
pub fn select_coins_for_spend(
    candidate_coins: &[CandidateCoin],
    base_tx: bitcoin::Transaction,
    change_txo: bitcoin::TxOut,
    feerate_vb: f32,
    min_fee: u64,
    max_sat_weight: u32,
) -> Result<(Vec<Coin>, bitcoin::Amount), InsufficientFunds> {
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
            value: cand.coin.amount.to_sat(),
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

    // Finally, run the coin selection algorithm. We use a BnB with 100k iterations and if it
    // couldn't find any solution we fall back to selecting coins by descending value.
    let target = Target {
        value: out_value_nochange,
        feerate: FeeRate::from_sat_per_vb(feerate_vb),
        min_fee,
    };
    if let Err(e) = selector.run_bnb(
        LowestFee {
            target,
            long_term_feerate,
            change_policy: &change_policy,
        },
        100_000,
    ) {
        warn!(
            "Coin selection error: '{}'. Selecting coins by descending value per weight unit...",
            e.to_string()
        );
        selector.sort_candidates_by_descending_value_pwu();
        // If more coins still need to be selected to meet target, then `change_policy(&selector, target)`
        // will give `Drain::none()`, i.e. no change, and this will simply select more coins until
        // they cover the target.
        selector.select_until_target_met(target, change_policy(&selector, target))?;
    }
    // By now, selection is complete and we can check how much change to give according to our policy.
    let drain = change_policy(&selector, target);
    let change_amount = bitcoin::Amount::from_sat(drain.value);
    Ok((
        selector
            .selected_indices()
            .iter()
            .map(|i| candidate_coins[*i].coin)
            .collect(),
        change_amount,
    ))
}
