pub use minisafe::commands::{
    GetAddressResult, GetInfoResult, ListCoinsEntry, ListCoinsResult, ListSpendEntry,
    ListSpendResult,
};

pub type Coin = ListCoinsEntry;
pub type SpendTx = ListSpendEntry;
