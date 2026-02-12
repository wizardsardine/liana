use liana_connect::ws_business;
use uuid::Uuid;

/// All application messages
#[derive(Debug, Clone)]
#[rustfmt::skip]
pub enum Msg {
    // Login/Auth
    LoginUpdateEmail(String),  // Update email input field
    LoginUpdateCode(String),   // Update auth code input field
    LoginSendToken,            // Send login token to email
    LoginResendToken,          // Resend login token
    LoginSendAuthCode,         // Submit auth code for verification
    Logout,                    // Log out current user

    // Account selection (cached token login)
    AccountSelectConnect(String), // Connect with cached account by email
    AccountSelectDelete(String),  // Delete cached account by email
    AccountSelectNewEmail,        // Start fresh login with new email

    // Org management
    OrgSelected(Uuid),          // Select an organization
    OrgWalletSelected(Uuid),    // Select a wallet within org

    // Wallet selection
    WalletSelectUpdateSearchFilter(String), // Update wallet search filter

    // Organization selection
    OrgSelectUpdateSearchFilter(String), // Update org search filter

    // Key management
    KeyAdd,                              // Open modal to add new key
    KeyEdit(u8),                         // Open modal to edit key by index
    KeyDelete(u8),                       // Delete key by index
    KeySave,                             // Save key changes
    KeyCancelModal,                      // Close key modal
    KeyUpdateAlias(String),              // Update key alias field
    KeyUpdateDescr(String),              // Update key description field
    KeyUpdateEmail(String),              // Update key email field
    KeyUpdateType(ws_business::KeyType), // Update key type

    // Template management
    TemplateAddKeyToPrimary(u8),                                // Add key to primary path
    TemplateDelKeyFromPrimary(u8),                              // Remove key from primary path
    TemplateAddKeyToSecondary(usize, u8),                       // Add key to secondary path
    TemplateDelKeyFromSecondary(usize, u8),                     // Remove key from secondary path
    TemplateAddSecondaryPath,                                   // Add new secondary path
    TemplateDeleteSecondaryPath(usize),                         // Delete secondary path by index
    TemplateEditPath(
        bool,          /* is_primary */
        Option<usize>, /* secondary_path_index */
    ),                                                          // Open path editor modal
    TemplateNewPathModal,                                       // Open modal to create a new recovery path
    TemplateToggleKeyInPath(u8),                                // Toggle key in/out of the currently edited path
    TemplateSavePath,                                           // Save path changes
    TemplateCancelPathModal,                                    // Close path modal
    TemplateUpdateThreshold(String),                            // Update threshold field
    TemplateUpdateTimelock(String),                             // Update timelock field
    TemplateUpdateTimelockUnit(crate::state::views::path::TimelockUnit), // Update timelock unit
    TemplateLock,                                               // Lock template (Draft → Locked)
    TemplateUnlock,                                             // Unlock template (Locked → Draft)
    TemplateValidate,                                           // Validate template

    // Navigation
    NavigateToHome,         // Navigate to home view
    NavigateToKeys,         // Navigate to keys view
    NavigateToOrgSelect,    // Navigate to org selection
    NavigateToWalletSelect, // Navigate to wallet selection
    NavigateBack,           // Navigate back

    // Backend
    BackendNotif(crate::backend::Notification), // Backend notification received
    BackendDisconnected,                        // Backend connection lost

    // Hardware Wallets
    HardwareWallets(async_hwi::service::SigningDeviceMsg), // Hardware wallet service message

    // Xpub management
    XpubSelectKey(u8),                                          // Open modal for key
    XpubUpdateInput(String),                                    // Update xpub text input
    XpubSelectDevice(miniscript::bitcoin::bip32::Fingerprint),  // Select HW device (opens details step)
    XpubDeviceBack,                                             // Go back from details to device selection
    XpubFetchFromDevice(
        miniscript::bitcoin::bip32::Fingerprint,
        miniscript::bitcoin::bip32::ChildNumber,
    ),                                                          // Fetch xpub from HW device
    XpubRetry,                                                  // Retry fetch after error
    XpubLoadFromFile,                                           // Trigger file picker
    XpubFileLoaded(Result<(String, String), String>),           // (content, filename) or error
    XpubSelectEnterXpub,                                        // Expand paste xpub card
    XpubPaste,                                                  // Trigger paste from clipboard
    XpubPasted(String),                                         // Xpub pasted from clipboard
    XpubUpdateAccount(miniscript::bitcoin::bip32::ChildNumber), // Change account (triggers re-fetch)
    XpubSave,                                                   // Save xpub to backend
    XpubClear,                                                  // Clear xpub (send null to backend)
    XpubCancelModal,                                            // Close modal
    XpubToggleOptions,                                          // Toggle "Other options" section

    // Registration (device descriptor registration)
    RegistrationSelectDevice(miniscript::bitcoin::bip32::Fingerprint), // Click on connected device to register
    RegistrationResult(Result<(miniscript::bitcoin::bip32::Fingerprint, Option<[u8; 32]>, String), String>), // async-hwi result (fp, hmac, alias)
    RegistrationCancelModal,                                           // Close registration modal
    RegistrationRetry,                                                 // Retry after error
    RegistrationConfirmYes,                                            // User confirms Coldcard registration succeeded
    RegistrationConfirmNo,                                             // User says Coldcard registration failed
    RegistrationSkip(miniscript::bitcoin::bip32::Fingerprint),         // Skip device registration
    RegistrationSkipAll,                                                   // Skip all remaining devices

    // Warnings
    WarningShowModal(String, String), // Show warning modal (title, message)
    WarningCloseModal,                // Close warning modal

    // Conflict resolution
    ConflictReload,    // User chose to reload from server
    ConflictKeepLocal, // User chose to keep local changes
    ConflictDismiss,   // Dismiss info-only conflict (e.g., key deleted)

    // Trigger a call on .update() & .view()
    Update, // Force UI refresh
}

/// Type alias for Msg (used in views)
pub type Message = Msg;

/// Required by HwiService<Message> to send notifications through the shared channel
impl From<async_hwi::service::SigningDeviceMsg> for Msg {
    fn from(msg: async_hwi::service::SigningDeviceMsg) -> Self {
        Msg::HardwareWallets(msg)
    }
}
