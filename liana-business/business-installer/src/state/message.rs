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

    // Account selection (cached token login)
    AccountSelectConnect(String), // Connect with cached account by email
    AccountSelectNewEmail,        // Start fresh login with new email

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
    TemplateLock,   // WSManager locks template (Draft → Locked)
    TemplateUnlock, // WSManager unlocks template (Locked → Draft)
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

    // Hardware Wallets
    HardwareWallets(async_hwi::service::SigningDeviceMsg),

    // Xpub management
    XpubSelectKey(u8),                     // Open modal for key
    XpubUpdateInput(String),               // Update xpub text input
    XpubSelectSource(crate::state::views::XpubSource), // Switch source tab
    XpubSelectDevice(miniscript::bitcoin::bip32::Fingerprint), // Select HW device (opens details step)
    XpubDeviceBack,                        // Go back from details to device selection
    XpubFetchFromDevice(
        miniscript::bitcoin::bip32::Fingerprint,
        miniscript::bitcoin::bip32::ChildNumber,
    ), // Fetch xpub from HW device
    XpubRetry,                             // Retry fetch after error
    XpubLoadFromFile,                      // Trigger file picker
    XpubFileLoaded(Result<(String, String), String>), // (content, filename) or error
    XpubPaste,                             // Trigger paste from clipboard
    XpubPasted(String),                    // Xpub pasted (with source tracking)
    XpubUpdateAccount(miniscript::bitcoin::bip32::ChildNumber), // Change account (triggers re-fetch)
    XpubSave,                              // Save xpub to backend
    XpubClear,                             // Clear xpub (send null to backend)
    XpubCancelModal,                       // Close modal
    XpubToggleOptions,                     // Toggle "Other options" section

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

/// Required by HwiService<Message> to send notifications through the shared channel
impl From<async_hwi::service::SigningDeviceMsg> for Msg {
    fn from(msg: async_hwi::service::SigningDeviceMsg) -> Self {
        Msg::HardwareWallets(msg)
    }
}
