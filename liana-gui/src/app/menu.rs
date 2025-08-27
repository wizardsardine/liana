use liana::miniscript::bitcoin::{OutPoint, Txid};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Home,
    Receive,
    PSBTs,
    Transactions,
    TransactionPreSelected(Txid),
    Settings,
    SettingsPreSelected(SettingsOption),
    Coins,
    CreateSpendTx,
    Recovery,
    RefreshCoins(Vec<OutPoint>),
    PsbtPreSelected(Txid),
    #[cfg(feature = "dev-coincube")]
    BuyAndSell, //(Option<AccountInfo>),
}

/// Pre-selectable settings options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
}
