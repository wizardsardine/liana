use coincube_core::miniscript::bitcoin::{OutPoint, Txid};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    Home,
    /// Spark wallet — default for everyday Lightning UX (Phase 5 flips
    /// the Lightning Address routing default here). Listed above Liquid
    /// in the sidebar because it's the default wallet post-Phase 5.
    Spark(SparkSubMenu),
    Liquid(LiquidSubMenu),
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
    Contacts,
    Invites,
    /// Cube-scoped members + pending-invites management. Feature-flagged by
    /// `feature_flags::CUBE_MEMBERS_UI_ENABLED`; sidebar entry is hidden
    /// otherwise.
    CubeMembers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2PSubMenu {
    Overview,
    MyTrades,
    Chat,
    CreateOrder,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiquidSubMenu {
    Overview,
    Send,
    Receive,
    Transactions(Option<Txid>),
    Settings(Option<SettingsOption>),
}

/// Spark wallet sub-panels.
///
/// Mirrors [`LiquidSubMenu`] on purpose — the Phase 4 plan is to copy
/// the Liquid panels into `state/spark/` and `view/spark/` and strip
/// the Liquid-only flows, so keeping the enum shape identical lets that
/// work land without menu churn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparkSubMenu {
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
    /// App-level Lightning preferences — currently a single card
    /// picking which backend fulfills incoming Lightning Address
    /// invoices for this cube.
    Lightning,
    About,
}

/// Pre-selectable settings options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
}
