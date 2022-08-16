use miniscript::bitcoin;
use serde::{Deserialize, Deserializer, Serializer};

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
