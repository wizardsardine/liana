use coincube_core::miniscript::bitcoin::{OutPoint, Txid};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Home,
    Active(ActiveSubMenu),
    Vault(VaultSubMenu),
    Settings(SettingsSubMenu),

    #[cfg(feature = "buysell")]
    BuySell,

    // Legacy menu items (kept for backward compatibility during transition)
    Receive,
    PSBTs,
    Transactions,
    TransactionPreSelected(Txid),
    SettingsPreSelected(SettingsOption),
    Coins,
    CreateSpendTx,
    Recovery,
    RefreshCoins(Vec<OutPoint>),
    PsbtPreSelected(Txid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveSubMenu {
    Overview,
    Send,
    Receive,
    Transactions(Option<Txid>),
    Settings(Option<SettingsOption>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultSubMenu {
    Overview,
    Send,
    Receive,
    Coins(Option<Vec<OutPoint>>),
    Transactions(Option<Txid>),
    PSBTs(Option<Txid>),
    Recovery,
    Settings(Option<SettingsOption>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsSubMenu {
    General,
    About,
}

/// Pre-selectable settings options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
}
