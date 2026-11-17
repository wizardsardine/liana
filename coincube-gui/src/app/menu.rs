use coincube_core::miniscript::bitcoin::{OutPoint, Txid};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Menu {
    /// "Cube" section — identity / dashboard / cube-level settings.
    Cube(CubeSubMenu),
    /// Spark wallet — default for everyday Lightning UX.
    Spark(SparkSubMenu),
    Vault(VaultSubMenu),
    Marketplace(MarketplaceSubMenu),
}

/// Sub-pages of the Cube section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CubeSubMenu {
    /// Cube landing / dashboard.
    Overview,
    /// Activity across the wallets in this open Cube.
    Activity,
    /// Cube-level settings. The inner enum drives the third rail —
    /// General / Recovery / Payment checks / About / Stats / Avatar / Members.
    Settings(CubeSettingsOption),
}

/// Third-rail options under Cube → Settings. Consolidates what used
/// to live at `Menu::Settings(SettingsSubMenu)` (now deleted) plus
/// Avatar and Members, lifted out of the per-Cube Connect surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CubeSettingsOption {
    AiAccess,
    General,
    /// Backup & recovery: local paper backup, the Connect Recovery Kit
    /// (password / Keychain), and Vault Recovery Alerts. Split out of General
    /// so each screen stays focused.
    Recovery,
    /// Recipient verification controls, separate from display preferences.
    PaymentChecks,
    /// Cube-wide entry points for recovery and Vault-specific files.
    DataExports,
    /// The Connect account, this device's registration and realtime
    /// stream, and the Re-register action. Shared by every wallet in the
    /// Cube; the Vault and Spark Connections tabs link here.
    Connections,
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

/// Sub-pages of the account-level Connect surface rendered by the Home
/// tab. The per-Cube Connect section that used to share this enum has
/// been removed — its features were relocated:
/// - `LightningAddress` → Spark → Settings → Lightning Address
/// - `Avatar` and `CubeMembers` → Cube → Settings → Avatar / Members
///
/// Only the variants the Home tab still routes through stay here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectSubMenu {
    Overview,
    PlanBilling,
    Security,
    Duress,
    Contacts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum P2PSubMenu {
    Overview,
    MyTrades,
    Chat,
    CreateOrder,
    Settings,
}

/// Spark wallet sub-panels. Spark Settings has its own tertiary rail
/// driven by [`SparkSettingsOption`] (General / Lightning Address).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparkSubMenu {
    Overview,
    Send,
    Receive,
    /// Swap BTC ⇄ USDB (S3), mainnet only.
    Swap,
    Transactions(Option<Txid>),
    Settings(Option<SparkSettingsOption>),
}

/// Third-rail options under Spark → Settings. Kept distinct from
/// [`SettingsOption`] (Vault-specific) so the two tertiary rails don't
/// form false sibling relationships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparkSettingsOption {
    /// Balances & conversion: the Stable Balance toggle and the Spark
    /// nickname.
    General,
    /// Lightning address claim / manage page, on its own tab.
    LightningAddress,
    /// Spark wallet service health, and a link to Cube › Connections.
    Connections,
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
    /// Tools › Split coins: the checklist of what is still on both chains
    /// ([`crate::app::state::vault::split_hub`]).
    SplitCoins,
    Settings(Option<SettingsOption>),
}

/// Third-rail options under Vault → Settings. Each variant maps to the
/// corresponding `view::SettingsMessage::*` sub-page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsOption {
    Node,
    Wallet,
    ImportExport,
    /// Local LAN signer ("Paired phones") — pair a Keychain phone over
    /// Wi-Fi so it shows up in the signer list without Connect. Lives
    /// under Vault → Settings because the paired phone signs PSBTs for
    /// this vault specifically (the persisted record is scoped to the
    /// vault's id fingerprint).
    LocalSigning,
    /// Node backend health, and a link to Cube › Connections.
    Connections,
}

/// Discriminant for the primary (left-most) nav rail. Derived from `Menu`
/// at render time — never stored as its own state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopLevel {
    Cube,
    Spark,
    Vault,
    Marketplace,
}

impl TopLevel {
    pub const ALL: &'static [TopLevel] = &[
        TopLevel::Cube,
        TopLevel::Spark,
        TopLevel::Vault,
        TopLevel::Marketplace,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TopLevel::Cube => "Cube",
            TopLevel::Spark => "Spark",
            TopLevel::Vault => "Vault",
            TopLevel::Marketplace => "Marketplace",
        }
    }

    /// Landing route for a primary-rail click.
    pub fn default_menu(self) -> Menu {
        match self {
            TopLevel::Cube => Menu::Cube(CubeSubMenu::Overview),
            TopLevel::Spark => Menu::Spark(SparkSubMenu::Overview),
            TopLevel::Vault => Menu::Vault(VaultSubMenu::Overview),
            TopLevel::Marketplace => {
                Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview))
            }
        }
    }
}

impl From<&Menu> for TopLevel {
    fn from(m: &Menu) -> Self {
        match m {
            Menu::Cube(_) => TopLevel::Cube,
            Menu::Spark(_) => TopLevel::Spark,
            Menu::Vault(_) => TopLevel::Vault,
            Menu::Marketplace(_) => TopLevel::Marketplace,
        }
    }
}
