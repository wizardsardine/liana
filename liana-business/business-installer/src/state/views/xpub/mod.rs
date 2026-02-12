use miniscript::{
    bitcoin::{bip32::ChildNumber, Network},
    DescriptorPublicKey,
};
use std::str::FromStr;

/// Modal step - Select device or Details (account selection + alias)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModalStep {
    #[default]
    Select,
    Details,
}

/// Actual source of the xpub for audit (not the UI tab selection)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XpubInputSource {
    /// Fetched from a hardware wallet device
    Device {
        kind: String,
        fingerprint: String,
        version: Option<String>,
    },
    /// Loaded from a file
    File { name: String },
    /// Pasted manually
    Pasted,
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
    /// Processing state (e.g., fetching from HW or saving)
    pub processing: bool,

    // Hardware wallet state
    /// Selected hardware wallet device
    pub selected_device: Option<miniscript::bitcoin::bip32::Fingerprint>,
    /// Selected account index for derivation
    pub selected_account: ChildNumber,

    /// Whether the "Other options" section is collapsed
    pub options_collapsed: bool,

    /// Current modal step (Select device or Details)
    pub step: ModalStep,
    /// Error during fetch (shown in details view)
    pub fetch_error: Option<String>,

    /// Network for validation (mainnet vs testnet)
    pub network: Network,

    /// Actual source of the current xpub input (for audit)
    pub input_source: Option<XpubInputSource>,

    /// Whether the paste xpub card is expanded (showing input + paste button)
    pub paste_expanded: bool,
    /// Form value for the paste xpub input field (with validation state)
    pub form_xpub: liana_ui::component::form::Value<String>,
}

impl XpubEntryModalState {
    /// Create a new xpub entry modal state for a given key
    pub fn new(
        key_id: u8,
        key_alias: String,
        current_xpub: Option<DescriptorPublicKey>,
        network: Network,
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
            processing: false,
            selected_device: None,
            selected_account: ChildNumber::from_hardened_idx(0)
                .expect("hardcoded valid account index"),
            options_collapsed: true, // Start with options collapsed
            step: ModalStep::default(),
            fetch_error: None,
            network,
            input_source: None,
            paste_expanded: false,
            form_xpub: liana_ui::component::form::Value::default(),
        }
    }

    /// Update the xpub input
    pub fn update_input(&mut self, input: String) {
        self.xpub_input = input;
    }

    /// Select a hardware wallet device and transition to Details step
    pub fn select_device(&mut self, fingerprint: miniscript::bitcoin::bip32::Fingerprint) {
        self.selected_device = Some(fingerprint);
        self.fetch_error = None;
        self.step = ModalStep::Details;
        self.processing = true;
    }

    /// Go back to Select step
    pub fn go_back(&mut self) {
        self.step = ModalStep::Select;
        self.selected_device = None;
        self.fetch_error = None;
        self.processing = false;
    }

    /// Update the derivation account
    pub fn update_account(&mut self, account: ChildNumber) {
        self.selected_account = account;
    }

    /// Set processing state
    pub fn set_processing(&mut self, processing: bool) {
        self.processing = processing;
    }

    /// Set fetch error (shown in details view)
    pub fn set_fetch_error(&mut self, error: String) {
        self.fetch_error = Some(error);
        self.processing = false;
    }

    /// Clear fetch error
    pub fn clear_fetch_error(&mut self) {
        self.fetch_error = None;
    }

    /// Validate and return the parsed xpub if valid (includes network check)
    pub fn validate(&self) -> Result<DescriptorPublicKey, String> {
        validate_xpub(&self.xpub_input, self.network)
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
        network: Network,
    ) {
        self.modal = Some(XpubEntryModalState::new(
            key_id,
            key_alias,
            current_xpub,
            network,
        ));
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
    DescriptorPublicKey::from_str(trimmed)
        .map_err(|e| format!("Invalid extended public key format: {}", e))
}

/// Check if a descriptor public key matches the expected network
///
/// Returns true if the key's network matches the expected network.
/// For mainnet, expects Bitcoin network. For testnet/signet/regtest, expects Testnet network.
pub fn check_key_network(key: &DescriptorPublicKey, network: Network) -> bool {
    match key {
        DescriptorPublicKey::XPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin.into()
            } else {
                key.xkey.network == Network::Testnet.into()
            }
        }
        DescriptorPublicKey::MultiXPub(key) => {
            if network == Network::Bitcoin {
                key.xkey.network == Network::Bitcoin.into()
            } else {
                key.xkey.network == Network::Testnet.into()
            }
        }
        // Single keys don't have network information
        _ => true,
    }
}

/// Validate xpub format and network compatibility
///
/// Validates both the format of the xpub string and that it matches the expected network.
pub fn validate_xpub(xpub_str: &str, network: Network) -> Result<DescriptorPublicKey, String> {
    let key = validate_xpub_format(xpub_str)?;

    if !check_key_network(&key, network) {
        let expected = network.to_string();
        return Err(format!("Extended public key is not valid for {}", expected));
    }

    Ok(key)
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
        let state = XpubEntryModalState::new(1, "Test Key".to_string(), None, Network::Bitcoin);
        assert_eq!(state.key_id, 1);
        assert_eq!(state.key_alias, "Test Key");
        assert_eq!(state.xpub_input, "");
        assert!(!state.processing);
    }

    #[test]
    fn test_modal_state_update_input() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Bitcoin);
        state.update_input("new input".to_string());
        assert_eq!(state.xpub_input, "new input");
    }

    #[test]
    fn test_has_changes() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Bitcoin);

        // No changes initially
        assert!(!state.has_changes());

        // Adding input counts as change
        state.update_input("something".to_string());
        assert!(state.has_changes());
    }

    #[test]
    fn test_check_key_network_mainnet() {
        // Mainnet xpub
        let mainnet_xpub = "[abcdef01/48'/0'/0'/2']xpub6CUGRUonZSQ4TWtTMmzXdrXDtypWKiKrhko4egpiMZbpiaQL2jkwSB1icqYh2cfDfVxdx4df189oLKnC5fSwqPfgyP3hooxujYzAu3fDVmz";
        let key = DescriptorPublicKey::from_str(mainnet_xpub).unwrap();

        // Should pass for mainnet
        assert!(check_key_network(&key, Network::Bitcoin));
        // Should fail for testnet/signet
        assert!(!check_key_network(&key, Network::Testnet));
        assert!(!check_key_network(&key, Network::Signet));
    }

    #[test]
    fn test_check_key_network_testnet() {
        // Testnet tpub (valid format)
        let testnet_xpub = "[abcdef01/48'/1'/0'/2']tpubDC8msFGeGuwnKG9Upg7DM2b4DaRqg3CUZa5g8v2SRQ6K4NSkxUgd7HsL2XVWbVm39yBA4LAxysQAm397zwQSQoQgewGiYZqrA9DsP4zbQ1M";
        let key = DescriptorPublicKey::from_str(testnet_xpub).unwrap();

        // Should fail for mainnet
        assert!(!check_key_network(&key, Network::Bitcoin));
        // Should pass for testnet/signet
        assert!(check_key_network(&key, Network::Testnet));
        assert!(check_key_network(&key, Network::Signet));
    }

    #[test]
    fn test_validate_xpub_network() {
        let mainnet_xpub = "[abcdef01/48'/0'/0'/2']xpub6CUGRUonZSQ4TWtTMmzXdrXDtypWKiKrhko4egpiMZbpiaQL2jkwSB1icqYh2cfDfVxdx4df189oLKnC5fSwqPfgyP3hooxujYzAu3fDVmz";

        // Should pass for mainnet
        assert!(validate_xpub(mainnet_xpub, Network::Bitcoin).is_ok());
        // Should fail for signet (non-mainnet)
        assert!(validate_xpub(mainnet_xpub, Network::Signet).is_err());
    }
}
