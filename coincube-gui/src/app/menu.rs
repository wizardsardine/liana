use coincube_core::miniscript::bitcoin::{OutPoint, Txid};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Home,
    Liquid(LiquidSubMenu),
    Usdt(UsdtSubMenu),
    Vault(VaultSubMenu),
    Marketplace(MarketplaceSubMenu),
    Connect(ConnectSubMenu),
    Settings(SettingsSubMenu),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketplaceSubMenu {
    BuySell,
    P2P(P2PSubMenu),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectSubMenu {
    Overview,
    LightningAddress,
    Avatar,
    PlanBilling,
    Security,
    Duress,
    Invites,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2PSubMenu {
    Overview,
    MyTrades,
    CreateOrder,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsdtSubMenu {
    Overview,
    Send,
    Receive,
    Transactions(Option<Txid>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiquidSubMenu {
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
