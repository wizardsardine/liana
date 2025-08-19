use serde::{Deserialize, Serialize};

use crate::services::fiat::{Currency, PriceSource};
use crate::utils::serde::{deser_fromstr, serialize_display};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PriceSetting {
    #[serde(
        deserialize_with = "deser_fromstr",
        serialize_with = "serialize_display"
    )]
    pub source: PriceSource,
    #[serde(
        deserialize_with = "deser_fromstr",
        serialize_with = "serialize_display"
    )]
    pub currency: Currency,
    pub is_enabled: bool,
}
