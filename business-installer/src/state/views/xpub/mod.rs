use miniscript::{bitcoin::bip32::ChildNumber, DescriptorPublicKey};
use std::str::FromStr;

/// Source for xpub entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XpubSource {
    HardwareWallet,
    LoadFromFile,
}

impl XpubSource {
    pub fn all() -> Vec<Self> {
        vec![
            XpubSource::HardwareWallet,
            XpubSource::LoadFromFile,
        ]
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            XpubSource::HardwareWallet => "Hardware Wallet",
            XpubSource::LoadFromFile => "Load from File",
        }
    }
}

impl Default for XpubSource {
    fn default() -> Self {
        XpubSource::HardwareWallet
    }
}

/// State for the Xpub Entry modal
#[derive(Debug, Clone)]
pub struct XpubEntryModalState {
    /// ID of the key being edited
    pub key_id: u8,
    /// Alias of the key (for display)
    pub key_alias: String,
    /// Current xpub value (if already set)
    pub current_xpub: Option<DescriptorPublicKey>,
    /// User input for xpub
    pub xpub_input: String,
    /// Selected source tab
    pub xpub_source: XpubSource,
    /// Validation error message
    pub validation_error: Option<String>,
    /// Processing state (e.g., fetching from HW or saving)
    pub processing: bool,

    // Hardware wallet state
    /// List of detected hardware wallet devices (fingerprint, device_name)
    pub hw_devices: Vec<(miniscript::bitcoin::bip32::Fingerprint, String)>,
    /// Selected hardware wallet device
    pub selected_device: Option<miniscript::bitcoin::bip32::Fingerprint>,
    /// Selected account index for derivation
    pub selected_account: ChildNumber,

    /// Whether the "Other options" section is collapsed
    pub options_collapsed: bool,
}

impl XpubEntryModalState {
    /// Create a new xpub entry modal state for a given key
    pub fn new(
        key_id: u8,
        key_alias: String,
        current_xpub: Option<DescriptorPublicKey>,
    ) -> Self {
        // Pre-fill input with current xpub if available
        let xpub_input = current_xpub
            .as_ref()
            .map(|xpub| xpub.to_string())
            .unwrap_or_default();

        Self {
            key_id,
            key_alias,
            current_xpub,
            xpub_input,
            xpub_source: XpubSource::default(),
            validation_error: None,
            processing: false,
            hw_devices: Vec::new(),
            selected_device: None,
            selected_account: ChildNumber::from_hardened_idx(0)
                .expect("hardcoded valid account index"),
            options_collapsed: true,  // Start with options collapsed
        }
    }

    /// Update the xpub input and clear any validation errors
    pub fn update_input(&mut self, input: String) {
        self.xpub_input = input;
        self.validation_error = None;
    }

    /// Switch to a different xpub source
    pub fn select_source(&mut self, source: XpubSource) {
        self.xpub_source = source;
        self.validation_error = None;
    }

    /// Select a hardware wallet device
    pub fn select_device(&mut self, fingerprint: miniscript::bitcoin::bip32::Fingerprint) {
        self.selected_device = Some(fingerprint);
        self.validation_error = None;
    }

    /// Update the derivation account
    pub fn update_account(&mut self, account: ChildNumber) {
        self.selected_account = account;
    }

    /// Set processing state
    pub fn set_processing(&mut self, processing: bool) {
        self.processing = processing;
    }

    /// Set validation error
    pub fn set_error(&mut self, error: String) {
        self.validation_error = Some(error);
        self.processing = false;
    }

    /// Clear validation error
    pub fn clear_error(&mut self) {
        self.validation_error = None;
    }

    /// Validate and return the parsed xpub if valid
    pub fn validate(&self) -> Result<DescriptorPublicKey, String> {
        validate_xpub_format(&self.xpub_input)
    }

    /// Check if the modal has unsaved changes
    pub fn has_changes(&self) -> bool {
        // Check if input differs from current xpub
        match &self.current_xpub {
            Some(current) => {
                // Parse input and compare
                if let Ok(parsed) = validate_xpub_format(&self.xpub_input) {
                    parsed != *current
                } else {
                    // Invalid input counts as a change
                    !self.xpub_input.is_empty()
                }
            }
            None => {
                // No current xpub - any non-empty input is a change
                !self.xpub_input.is_empty()
            }
        }
    }
}

/// Xpub view state - manages xpub entry for Validated wallets
#[derive(Debug, Clone, Default)]
pub struct XpubViewState {
    /// Optional modal state for xpub entry
    pub modal: Option<XpubEntryModalState>,
}

impl XpubViewState {
    /// Open the xpub entry modal for a key
    pub fn open_modal(
        &mut self,
        key_id: u8,
        key_alias: String,
        current_xpub: Option<DescriptorPublicKey>,
    ) {
        self.modal = Some(XpubEntryModalState::new(key_id, key_alias, current_xpub));
    }

    /// Close the xpub entry modal
    pub fn close_modal(&mut self) {
        self.modal = None;
    }

    /// Get a mutable reference to the modal state
    pub fn modal_mut(&mut self) -> Option<&mut XpubEntryModalState> {
        self.modal.as_mut()
    }
}

/// Validate xpub format using miniscript
///
/// This function is network-agnostic - it only validates that the string
/// represents a valid extended public key format (xpub, ypub, zpub, tpub, etc.)
/// without checking network compatibility.
pub fn validate_xpub_format(xpub_str: &str) -> Result<DescriptorPublicKey, String> {
    let trimmed = xpub_str.trim();

    if trimmed.is_empty() {
        return Err("Extended public key cannot be empty".to_string());
    }

    // Try to parse as DescriptorPublicKey
    DescriptorPublicKey::from_str(trimmed).map_err(|e| {
        format!("Invalid extended public key format: {}", e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_xpub_empty() {
        assert!(validate_xpub_format("").is_err());
        assert!(validate_xpub_format("   ").is_err());
    }

    #[test]
    fn test_validate_xpub_invalid() {
        assert!(validate_xpub_format("not-an-xpub").is_err());
        assert!(validate_xpub_format("xpub123").is_err());
    }

    #[test]
    fn test_validate_xpub_valid() {
        // Valid mainnet xpub
        let valid_xpub = "xpub6CUGRUonZSQ4TWtTMmzXdrXDtypWKiKrhko4egpiMZbpiaQL2jkwSB1icqYh2cfDfVxdx4df189oLKnC5fSwqPfgyP3hooxujYzAu3fDVmz";
        assert!(validate_xpub_format(valid_xpub).is_ok());

        // Valid with whitespace
        let with_whitespace = format!("  {}  ", valid_xpub);
        assert!(validate_xpub_format(&with_whitespace).is_ok());
    }

    #[test]
    fn test_modal_state_new() {
        let state = XpubEntryModalState::new(1, "Test Key".to_string(), None);
        assert_eq!(state.key_id, 1);
        assert_eq!(state.key_alias, "Test Key");
        assert_eq!(state.xpub_input, "");
        assert!(!state.processing);
        assert!(state.validation_error.is_none());
    }

    #[test]
    fn test_modal_state_update_input() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None);
        state.set_error("Previous error".to_string());

        state.update_input("new input".to_string());
        assert_eq!(state.xpub_input, "new input");
        assert!(state.validation_error.is_none());
    }

    #[test]
    fn test_has_changes() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None);

        // No changes initially
        assert!(!state.has_changes());

        // Adding input counts as change
        state.update_input("something".to_string());
        assert!(state.has_changes());
    }
}
