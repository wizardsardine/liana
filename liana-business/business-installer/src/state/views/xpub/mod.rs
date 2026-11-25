use liana_gui::{
    airgap::device::{AirgappedDevice, Signer},
    utils::derivation_path,
};
use liana_i18n::t;
use miniscript::{
    bitcoin::{
        bip32::{ChildNumber, DerivationPath, Fingerprint},
        Network,
    },
    descriptor::{DescriptorXKey, Wildcard},
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
    /// Scanned from a QR signing device
    Airgapped(AirgappedDevice),
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
    /// Whether the "Paste extended public key" input is expanded
    pub paste_expanded: bool,

    /// Current modal step (Select device or Details)
    pub step: ModalStep,
    /// Error during fetch (shown in details view)
    pub fetch_error: Option<String>,

    /// Network for validation (mainnet vs testnet)
    pub network: Network,

    /// Actual source of the current xpub input (for audit)
    pub input_source: Option<XpubInputSource>,
    /// QR signing device whose keys were scanned
    pub airgapped_signer: Option<Signer>,
    /// Error of the QR signing device scan (shown on its entry)
    pub airgap_error: Option<String>,
    active_request_id: Option<u64>,
    next_request_id: u64,
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
            selected_account: default_account(),
            options_collapsed: true, // Start with options collapsed
            paste_expanded: false,
            step: ModalStep::default(),
            fetch_error: None,
            network,
            input_source: None,
            airgapped_signer: None,
            airgap_error: None,
            active_request_id: None,
            next_request_id: 0,
        }
    }

    /// Update the xpub input
    pub fn update_input(&mut self, input: String) {
        self.xpub_input = input;
    }

    /// Select a hardware wallet device and transition to Details step
    pub fn select_device(&mut self, fingerprint: Fingerprint) {
        self.selected_device = Some(fingerprint);
        self.airgapped_signer = None;
        self.fetch_error = None;
        self.input_source = None;
        self.step = ModalStep::Details;
        self.processing = true;
    }

    /// Go back to Select step
    pub fn go_back(&mut self) {
        self.step = ModalStep::Select;
        self.selected_device = None;
        self.airgapped_signer = None;
        self.fetch_error = None;
        self.processing = false;
        self.active_request_id = None;
    }

    /// Update the derivation account
    pub fn update_account(&mut self, account: ChildNumber) {
        self.selected_account = account;
        self.input_source = None;
        self.active_request_id = None;
    }

    /// Use the keys scanned from a QR signing device and transition to Details step
    pub fn select_airgapped_signer(&mut self, signer: Signer) {
        self.selected_device = Some(signer.fingerprint);
        self.airgapped_signer = Some(signer);
        self.airgap_error = None;
        self.fetch_error = None;
        self.processing = false;
        self.active_request_id = None;
        self.step = ModalStep::Details;
        self.select_airgapped_account(self.selected_account);
    }

    /// Take the scanned key of an account, no device round trip needed
    pub fn select_airgapped_account(&mut self, account: ChildNumber) {
        let Some(signer) = self.airgapped_signer.as_ref() else {
            return;
        };
        let path = derivation_path(self.network, account);
        let Some((_, xpub)) = signer.keys.iter().find(|(candidate, _)| *candidate == path) else {
            tracing::warn!("account {account} is not among the scanned signing device's keys");
            return;
        };
        let key = DescriptorPublicKey::XPub(DescriptorXKey {
            origin: Some((signer.fingerprint, path)),
            derivation_path: DerivationPath::master(),
            wildcard: Wildcard::None,
            xkey: *xpub,
        });
        self.selected_account = account;
        self.xpub_input = key.to_string();
        self.input_source = Some(XpubInputSource::Airgapped(AirgappedDevice::new(
            signer, None,
        )));
    }

    pub fn start_fetch(&mut self) -> u64 {
        self.processing = true;
        self.fetch_error = None;
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.active_request_id = Some(request_id);
        request_id
    }

    pub fn matches_fetch_request(
        &self,
        request_id: u64,
        fingerprint: Fingerprint,
        path: &DerivationPath,
    ) -> bool {
        let requested_account = path.as_ref().get(2).copied();
        self.step == ModalStep::Details
            && self.selected_device == Some(fingerprint)
            && requested_account == Some(self.selected_account)
            && self.active_request_id == Some(request_id)
    }

    pub fn matches_fetch_error_request(&self, request_id: u64) -> bool {
        self.step == ModalStep::Details && self.active_request_id == Some(request_id)
    }

    /// Set processing state
    pub fn set_processing(&mut self, processing: bool) {
        self.processing = processing;
        if !processing {
            self.active_request_id = None;
        }
    }

    /// Set fetch error (shown in details view)
    pub fn set_fetch_error(&mut self, error: String) {
        self.fetch_error = Some(error);
        self.processing = false;
        self.active_request_id = None;
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

fn default_account() -> ChildNumber {
    ChildNumber::from_hardened_idx(0).expect("hardcoded valid account index")
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
        return Err(t!("business-xpub-empty"));
    }

    // Try to parse as DescriptorPublicKey
    DescriptorPublicKey::from_str(trimmed)
        .map_err(|e| t!("business-xpub-invalid-format", error = e.to_string()))
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
        return Err(t!("business-xpub-invalid-network", network = expected));
    }

    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bip377::ll::response::{Capabilities, FirmwareVersion, ReleaseFlag};

    const ACCOUNT_KEYS: [&str; 3] = [
        "[9f141cf0/48'/1'/0'/2']tpubDFnReAwXvYd6RA46X55HuFpmvZsLanDrwHAUsdYEGEpNGTRnCdbDRXJGLTwDeqKURCPZUDgdkuuu9dYkuBNQHmSNBUu7V2CdLKwpJjx2JuC",
        "[9f141cf0/48'/1'/1'/2']tpubDDwKEc4i4k8rBgVLGxytHrP13VVYucUGmL2cadux7AfMwMnRHKcw1YZKt9SMB4fWut7ZAiZqPefzm3BBCNXLZMxDrWJ4Q6VA1AFB6b8GzbT",
        "[9f141cf0/48'/1'/2'/2']tpubDEnYysximqdZkZnW5W9gYc7N3sxizKyqfdJfZ2qRfwNvSv6E11yDgyLTAnWQqDmVJ7oQ3h3ui59RQm1qmGxMm4jinq5wvSzyueKgrLJj5Cy",
    ];

    fn scanned_signer() -> Signer {
        Signer {
            fingerprint: Fingerprint::from_str("9f141cf0").unwrap(),
            vendor: "test vendor".to_owned(),
            model: "test signer".to_owned(),
            version: FirmwareVersion {
                major: 1,
                minor: 0,
                patch: 0,
                flag: ReleaseFlag::Stable,
            },
            capabilities: Capabilities(1),
            keys: ACCOUNT_KEYS
                .iter()
                .map(|key| match DescriptorPublicKey::from_str(key).unwrap() {
                    DescriptorPublicKey::XPub(key) => (key.origin.unwrap().1, key.xkey),
                    _ => unreachable!("the fixtures are xpubs"),
                })
                .collect(),
        }
    }

    fn account_key(index: usize) -> String {
        DescriptorPublicKey::from_str(ACCOUNT_KEYS[index])
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_qr_scan_takes_the_first_account_and_records_the_device() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Testnet);
        state.select_airgapped_signer(scanned_signer());

        assert_eq!(state.step, ModalStep::Details);
        assert_eq!(
            state.selected_device,
            Some(Fingerprint::from_str("9f141cf0").unwrap())
        );
        assert!(!state.processing);
        assert_eq!(state.xpub_input, account_key(0));
        assert!(state.validate().is_ok());
        match &state.input_source {
            Some(XpubInputSource::Airgapped(device)) => {
                assert_eq!(device.fingerprint.to_string(), "9f141cf0");
                assert_eq!(device.vendor, "test vendor");
                assert_eq!(device.model, "test signer");
            }
            other => panic!("unexpected source {other:?}"),
        }
    }

    #[test]
    fn a_qr_account_change_takes_the_scanned_key() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Testnet);
        state.select_airgapped_signer(scanned_signer());
        let account = ChildNumber::from_hardened_idx(2).unwrap();
        state.select_airgapped_account(account);

        assert_eq!(state.selected_account, account);
        assert_eq!(state.xpub_input, account_key(2));
        assert!(!state.processing);
    }

    #[test]
    fn going_back_drops_the_scanned_device() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Testnet);
        state.select_airgapped_signer(scanned_signer());
        state.go_back();

        assert!(state.airgapped_signer.is_none());
        assert_eq!(state.step, ModalStep::Select);
    }

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
        let with_whitespace = format!("  {valid_xpub}  ");
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
    fn test_start_fetch_marks_processing() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Bitcoin);
        state.select_device(Fingerprint::from_str("abcdef01").expect("hardcoded fingerprint"));
        let request_id = state.start_fetch();

        assert_eq!(request_id, 0);
        assert!(state.processing);
        assert!(state.matches_fetch_error_request(request_id));
    }

    #[test]
    fn test_matches_fetch_request_checks_request_id() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Bitcoin);
        let fingerprint = Fingerprint::from_str("abcdef01").expect("hardcoded fingerprint");
        state.select_device(fingerprint);
        let request_id = state.start_fetch();
        let path = DerivationPath::from(vec![
            ChildNumber::Hardened { index: 48 },
            ChildNumber::Hardened { index: 0 },
            ChildNumber::Hardened { index: 0 },
            ChildNumber::Hardened { index: 2 },
        ]);

        assert!(state.matches_fetch_request(request_id, fingerprint, &path));
        assert!(!state.matches_fetch_request(request_id.wrapping_add(1), fingerprint, &path));
    }

    #[test]
    fn test_select_device_clears_previous_input_source() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Bitcoin);
        state.input_source = Some(XpubInputSource::Pasted);

        state.select_device(
            miniscript::bitcoin::bip32::Fingerprint::from_str("abcdef01")
                .expect("hardcoded fingerprint"),
        );

        assert_eq!(state.input_source, None);
        assert_eq!(state.step, ModalStep::Details);
        assert!(state.processing);
    }

    #[test]
    fn test_update_account_clears_previous_input_source() {
        let mut state = XpubEntryModalState::new(1, "Test".to_string(), None, Network::Bitcoin);
        state.input_source = Some(XpubInputSource::Pasted);

        state.update_account(ChildNumber::from_hardened_idx(1).expect("hardcoded account index"));

        assert_eq!(state.input_source, None);
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
