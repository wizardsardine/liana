use miniscript::bitcoin::{self, consensus, util::psbt::PartiallySignedTransaction as Psbt};
use serde::{de, Deserialize, Deserializer, Serializer};

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

pub fn ser_base64<S, T>(t: T, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: consensus::Encodable,
{
    s.serialize_str(&base64::encode(consensus::serialize(&t)))
}

pub fn deser_psbt_base64<'de, D>(d: D) -> Result<Psbt, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(d)?;
    let s = base64::decode(&s).map_err(de::Error::custom)?;
    let psbt = consensus::deserialize(&s).map_err(de::Error::custom)?;
    Ok(psbt)
}

// Utility to gather the index of a change output in a Psbt, if there is one.
// FIXME: this is temporary! This is based on create_spend's behaviour that reuses the
// first coin address and doesn't shuffle the outputs!
pub fn change_index(psbt: &Psbt) -> Option<usize> {
    // We always set the witness UTxO in the PSBTs we create.
    let first_coin_spk = match psbt.inputs[0]
        .witness_utxo
        .as_ref()
        .map(|o| &o.script_pubkey)
    {
        Some(spk) => spk,
        None => return None,
    };

    let tx = &psbt.unsigned_tx;
    (0..tx.output.len())
        .rev()
        .find(|&i| &tx.output[i].script_pubkey == first_coin_spk)
}
