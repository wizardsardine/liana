use coincube_core::miniscript::bitcoin::{OutPoint, Txid};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    /// "Cube" section — identity / dashboard / cube-level settings.
    Cube(CubeSubMenu),
    /// Spark wallet — default for everyday Lightning UX. Listed above
    /// Liquid in the sidebar because it's the default wallet.
    Spark(SparkSubMenu),
    Liquid(LiquidSubMenu),
    Vault(VaultSubMenu),
    Marketplace(MarketplaceSubMenu),
    Connect(ConnectSubMenu),
}

/// Sub-pages of the Cube section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CubeSubMenu {
    /// Cube landing / dashboard.
    Overview,
    /// Cube-level settings. The inner enum drives the third rail —
    /// General / About / Stats.
    Settings(CubeSettingsOption),
}

/// Third-rail options under Cube → Settings. Consolidates what used
/// to live at `Menu::Settings(SettingsSubMenu)` (now deleted) plus
/// Avatar and Members, lifted out of the per-Cube Connect surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CubeSettingsOption {
    General,
    About,
    Stats,
    /// Avatar questionnaire / generation / reveal / management. Renders
    /// from `ConnectCubePanel` via App-level dispatch — the State
    /// trait's view signature can't reach Connect state.
    Avatar,
    /// Cube members + pending-invites management. Feature-flagged by
    /// `feature_flags::CUBE_MEMBERS_UI_ENABLED`; sidebar entry is hidden
    /// otherwise. Same App-level dispatch as Avatar.
    Members,
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
/// Closely mirrors [`LiquidSubMenu`] — the Phase 4 plan copied the
/// Liquid panels into `state/spark/` and `view/spark/`. Settings
/// diverges: Spark Settings has its own tertiary rail driven by
/// [`SparkSettingsOption`] (General / Lightning Address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparkSubMenu {
    Overview,
    Send,
    Receive,
    Transactions(Option<Txid>),
    Settings(Option<SparkSettingsOption>),
}

/// Third-rail options under Spark → Settings. Kept distinct from
/// [`SettingsOption`] (Vault-specific) so the two tertiary rails don't
/// form false sibling relationships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparkSettingsOption {
    /// Stable Balance toggle + Bridge status diagnostics.
    General,
    /// Lightning Address claim / manage page.
    LightningAddress,
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
    Cube,
    Spark,
    Liquid,
    Vault,
    Marketplace,
    Connect,
}

impl TopLevel {
    pub const ALL: &'static [TopLevel] = &[
        TopLevel::Cube,
        TopLevel::Spark,
        TopLevel::Liquid,
        TopLevel::Vault,
        TopLevel::Marketplace,
        TopLevel::Connect,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TopLevel::Cube => "Cube",
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
            TopLevel::Cube => Menu::Cube(CubeSubMenu::Overview),
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
            Menu::Cube(_) => TopLevel::Cube,
            Menu::Spark(_) => TopLevel::Spark,
            Menu::Liquid(_) => TopLevel::Liquid,
            Menu::Vault(_) => TopLevel::Vault,
            Menu::Marketplace(_) => TopLevel::Marketplace,
            Menu::Connect(_) => TopLevel::Connect,
        }
    }
}
