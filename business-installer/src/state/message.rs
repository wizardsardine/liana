use uuid::Uuid;

/// All application messages
#[derive(Debug, Clone)]
pub enum Msg {
    // Login/Auth
    LoginUpdateEmail(String),
    LoginUpdateCode(String),
    LoginSendToken,
    LoginResendToken,
    LoginSendAuthCode,
    Logout,

    // Org management
    OrgSelected(Uuid),
    OrgWalletSelected(Uuid),
    OrgCreateNewWallet,

    // Wallet selection
    WalletSelectToggleHideFinalized(bool),
    WalletSelectUpdateSearchFilter(String),

    // Organization selection
    OrgSelectUpdateSearchFilter(String),

    // Key management
    KeyAdd,
    KeyEdit(u8),
    KeyDelete(u8),
    KeySave,
    KeyCancelModal,
    KeyUpdateAlias(String),
    KeyUpdateDescr(String),
    KeyUpdateEmail(String),
    KeyUpdateType(liana_connect::KeyType),

    // Template management
    TemplateAddKeyToPrimary(u8),
    TemplateDelKeyFromPrimary(u8),
    TemplateAddKeyToSecondary(usize, u8),
    TemplateDelKeyFromSecondary(usize, u8),
    TemplateAddSecondaryPath,
    TemplateDeleteSecondaryPath(usize),
    TemplateEditPath(
        bool,          /* is_primary */
        Option<usize>, /* secondary_path_index */
    ),
    TemplateNewPathModal, // Open modal to create a new recovery path
    TemplateToggleKeyInPath(u8), // Toggle key in/out of the currently edited path
    TemplateSavePath,
    TemplateCancelPathModal,
    TemplateUpdateThreshold(String),
    TemplateUpdateTimelock(String),
    TemplateUpdateTimelockUnit(crate::state::views::path::TimelockUnit),
    TemplateValidate,

    // Navigation
    NavigateToHome,
    NavigateToKeys,
    NavigateToOrgSelect,
    NavigateToWalletSelect,
    NavigateBack,

    // Backend
    BackendNotif(crate::backend::Notification),
    BackendDisconnected,

    // Warnings
    WarningShowModal(String, String), // title, message
    WarningCloseModal,

    // Conflict resolution
    ConflictReload,    // User chose to reload from server
    ConflictKeepLocal, // User chose to keep local changes
    ConflictDismiss,   // Dismiss info-only conflict (e.g., key deleted)
}

/// Type alias for Msg (used in views)
pub type Message = Msg;
