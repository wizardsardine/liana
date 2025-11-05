use liana::miniscript::bitcoin::{OutPoint, Txid};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Home,

    // Active menu and submenus
    Active,
    ActiveSend,
    ActiveReceive,
    ActiveTransactions,
    ActiveTransactionPreSelected(Txid),
    ActiveSettings,
    ActiveSettingsPreSelected(SettingsOption),

    // Vault menu and submenus
    Vault,
    VaultHome,
    VaultSend,
    VaultReceive,
    VaultCoins,
    VaultTransactions,
    VaultTransactionPreSelected(Txid),
    VaultPSBTs,
    VaultPsbtPreSelected(Txid),
    VaultRecovery,
    VaultRefreshCoins(Vec<OutPoint>),
    VaultSettings,
    VaultSettingsPreSelected(SettingsOption),

    #[cfg(feature = "buysell")]
    BuySell, //(Option<AccountInfo>),

    // Legacy menu items (kept for backward compatibility during transition)
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
}

/// Pre-selectable settings options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
}
