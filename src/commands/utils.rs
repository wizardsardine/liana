use miniscript::bitcoin::{self, consensus, util::psbt::PartiallySignedTransaction as Psbt};
use serde::{de, Deserialize, Deserializer, Serializer};

/// Serialize an amount as sats
pub fn ser_amount<S: Serializer>(amount: &bitcoin::Amount, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(amount.as_sat())
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
