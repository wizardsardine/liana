use serde::{Deserialize, Serialize};

pub use coincube_ui::component::amount::BitcoinDisplayUnit;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct UnitSetting {
    #[serde(default)]
    pub display_unit: BitcoinDisplayUnit,
}
