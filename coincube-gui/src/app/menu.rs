use coincube_core::miniscript::bitcoin::{OutPoint, Txid};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    /// "Cube" section — identity / dashboard / cube-level settings.
    /// Internal name stays `Home` for churn reasons; user-visible label
    /// is `"Cube"` (see [`TopLevel::label`]).
    Home(HomeSubMenu),
    /// Spark wallet — default for everyday Lightning UX (Phase 5 flips
    /// the Lightning Address routing default here). Listed above Liquid
    /// in the sidebar because it's the default wallet post-Phase 5.
    Spark(SparkSubMenu),
    Liquid(LiquidSubMenu),
    Vault(VaultSubMenu),
    Marketplace(MarketplaceSubMenu),
    Connect(ConnectSubMenu),
}

/// Sub-pages of the Cube section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HomeSubMenu {
    /// Cube landing / dashboard.
    Overview,
    /// Cube-level settings. The inner enum drives the third rail —
    /// General / Lightning / About.
    Settings(HomeSettingsOption),
}

/// Third-rail options under Cube → Settings. Consolidates what used
/// to live at `Menu::Settings(SettingsSubMenu)` (now deleted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeSettingsOption {
    General,
    Lightning,
    About,
    Stats,
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

/// Third-rail options under Vault → Settings. Each variant maps to the
/// corresponding `view::SettingsMessage::*` sub-page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
    Wallet,
    ImportExport,
}

/// Discriminant for the primary (left-most) nav rail. Derived from `Menu`
/// at render time — never stored as its own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopLevel {
    Home,
    Spark,
    Liquid,
    Vault,
    Marketplace,
    Connect,
}

impl TopLevel {
    pub const ALL: &'static [TopLevel] = &[
        TopLevel::Home,
        TopLevel::Spark,
        TopLevel::Liquid,
        TopLevel::Vault,
        TopLevel::Marketplace,
        TopLevel::Connect,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TopLevel::Home => "Cube",
            TopLevel::Spark => "Spark",
            TopLevel::Liquid => "Liquid",
            TopLevel::Vault => "Vault",
            TopLevel::Marketplace => "Marketplace",
            TopLevel::Connect => "Connect",
        }
    }

    /// Landing route for a primary-rail click.
    pub fn default_menu(self) -> Menu {
        match self {
            TopLevel::Home => Menu::Home(HomeSubMenu::Overview),
            TopLevel::Spark => Menu::Spark(SparkSubMenu::Overview),
            TopLevel::Liquid => Menu::Liquid(LiquidSubMenu::Overview),
            TopLevel::Vault => Menu::Vault(VaultSubMenu::Overview),
            TopLevel::Marketplace => {
                Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview))
            }
            TopLevel::Connect => Menu::Connect(ConnectSubMenu::Overview),
        }
    }
}

impl From<&Menu> for TopLevel {
    fn from(m: &Menu) -> Self {
        match m {
            Menu::Home(_) => TopLevel::Home,
            Menu::Spark(_) => TopLevel::Spark,
            Menu::Liquid(_) => TopLevel::Liquid,
            Menu::Vault(_) => TopLevel::Vault,
            Menu::Marketplace(_) => TopLevel::Marketplace,
            Menu::Connect(_) => TopLevel::Connect,
        }
    }
}
