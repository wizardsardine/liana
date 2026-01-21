use miniscript::bitcoin::bip32::Fingerprint;

/// Modal step for registration process
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RegistrationModalStep {
    #[default]
    Registering, // "Confirm on device..."
    /// Coldcard confirmation - device doesn't block, need user to confirm success
    ConfirmColdcard {
        /// Registration result to send if user confirms
        hmac: Option<[u8; 32]>,
        wallet_name: String,
    },
    Error, // Show error with retry
}

/// State for a single device registration modal
#[derive(Debug, Clone)]
pub struct RegistrationModalState {
    pub fingerprint: Fingerprint,
    pub device_kind: Option<async_hwi::DeviceKind>,
    pub step: RegistrationModalStep,
    pub error: Option<String>,
}

/// State for the registration view
#[derive(Debug, Clone, Default)]
pub struct RegistrationViewState {
    /// Descriptor string to register
    pub descriptor: Option<String>,
    /// Devices assigned to current user that need registration
    pub user_devices: Vec<Fingerprint>,
    /// Currently open modal (if any)
    pub modal: Option<RegistrationModalState>,
}

impl RegistrationViewState {
    /// Open registration modal for a device
    pub fn open_modal(
        &mut self,
        fingerprint: Fingerprint,
        device_kind: Option<async_hwi::DeviceKind>,
    ) {
        self.modal = Some(RegistrationModalState {
            fingerprint,
            device_kind,
            step: RegistrationModalStep::Registering,
            error: None,
        });
    }

    /// Close the modal
    pub fn close_modal(&mut self) {
        self.modal = None;
    }

    /// Set error on modal
    pub fn set_modal_error(&mut self, error: String) {
        if let Some(modal) = &mut self.modal {
            modal.step = RegistrationModalStep::Error;
            modal.error = Some(error);
        }
    }
}
