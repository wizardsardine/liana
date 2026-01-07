use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BitcoinDisplayUnit {
    #[default]
    BTC,
    Sats,
}

impl From<BitcoinDisplayUnit> for coincube_ui::component::amount::BitcoinDisplayUnit {
    fn from(unit: BitcoinDisplayUnit) -> Self {
        match unit {
            BitcoinDisplayUnit::BTC => coincube_ui::component::amount::BitcoinDisplayUnit::BTC,
            BitcoinDisplayUnit::Sats => coincube_ui::component::amount::BitcoinDisplayUnit::Sats,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UnitSetting {
    #[serde(default)]
    pub display_unit: BitcoinDisplayUnit,
}
