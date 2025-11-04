use liana::miniscript::bitcoin::{OutPoint, Txid};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Home,
    
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
    
    #[cfg(feature = "buysell")]
    BuySell, //(Option<AccountInfo>),
}

/// Pre-selectable settings options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
}
