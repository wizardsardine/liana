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
    /// Devices the user has already registered in this session
    pub completed_devices: Vec<Fingerprint>,
    /// Currently open modal (if any)
    pub modal: Option<RegistrationModalState>,
}

impl RegistrationViewState {
    pub fn load_wallet(&mut self, descriptor: Option<String>, user_devices: Vec<Fingerprint>) {
        self.descriptor = descriptor;
        self.user_devices = user_devices;
        self.completed_devices.clear();
        self.modal = None;
    }

    pub fn sync_wallet(&mut self, descriptor: Option<String>, user_devices: Vec<Fingerprint>) {
        // The backend owns registration. A device that was pending and is no longer pending has
        // been registered there; keep it visible (rendered as Registered) via completed_devices.
        // A device that comes back pending is dropped from completed by the retain below.
        for fingerprint in &self.user_devices {
            if !user_devices.contains(fingerprint) && !self.completed_devices.contains(fingerprint)
            {
                self.completed_devices.push(*fingerprint);
            }
        }
        self.descriptor = descriptor;
        self.user_devices = user_devices;
        self.completed_devices
            .retain(|fingerprint| !self.user_devices.contains(fingerprint));
    }

    pub fn is_registered(&self, fingerprint: Fingerprint) -> bool {
        self.completed_devices.contains(&fingerprint)
    }

    pub fn visible_devices(&self) -> Vec<Fingerprint> {
        let mut devices = self.user_devices.clone();
        for fingerprint in &self.completed_devices {
            if !devices.contains(fingerprint) {
                devices.push(*fingerprint);
            }
        }
        devices
    }

    pub fn has_visible_devices(&self) -> bool {
        !self.user_devices.is_empty() || !self.completed_devices.is_empty()
    }

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
            let error = if error.contains("status: Unknown,") {
                "Device disconnected".into()
            } else {
                error
            };
            modal.step = RegistrationModalStep::Error;
            modal.error = Some(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RegistrationViewState;
    use miniscript::bitcoin::bip32::Fingerprint;

    #[test]
    fn visible_devices_keeps_completed_entries() {
        let mut state = RegistrationViewState {
            user_devices: vec![
                Fingerprint::from([0, 0, 0, 1]),
                Fingerprint::from([0, 0, 0, 2]),
            ],
            ..Default::default()
        };

        // Backend reports fp2 no longer pending => registered, fp1 still pending.
        state.sync_wallet(
            Some("desc".to_string()),
            vec![Fingerprint::from([0, 0, 0, 1])],
        );
        assert_eq!(state.user_devices, vec![Fingerprint::from([0, 0, 0, 1])]);

        assert_eq!(
            state.visible_devices(),
            vec![
                Fingerprint::from([0, 0, 0, 1]),
                Fingerprint::from([0, 0, 0, 2])
            ]
        );
        assert!(state.is_registered(Fingerprint::from([0, 0, 0, 2])));
        assert!(state.has_visible_devices());
    }

    #[test]
    fn load_wallet_clears_completed_devices() {
        let mut state = RegistrationViewState {
            user_devices: vec![Fingerprint::from([0, 0, 0, 1])],
            ..Default::default()
        };
        state.sync_wallet(Some("desc".to_string()), vec![]);
        assert!(state.is_registered(Fingerprint::from([0, 0, 0, 1])));

        state.load_wallet(
            Some("desc".to_string()),
            vec![Fingerprint::from([0, 0, 0, 2])],
        );

        assert_eq!(
            state.visible_devices(),
            vec![Fingerprint::from([0, 0, 0, 2])]
        );
        assert!(!state.is_registered(Fingerprint::from([0, 0, 0, 1])));
    }

    #[test]
    fn sync_wallet_clears_completed_devices_still_pending() {
        let fingerprint = Fingerprint::from([0, 0, 0, 1]);
        let mut state = RegistrationViewState {
            user_devices: vec![fingerprint],
            ..Default::default()
        };
        state.sync_wallet(Some("desc".to_string()), vec![]);
        assert!(state.is_registered(fingerprint));

        // Backend reports it pending again => no longer registered.
        state.sync_wallet(Some("desc".to_string()), vec![fingerprint]);

        assert_eq!(state.visible_devices(), vec![fingerprint]);
        assert!(!state.is_registered(fingerprint));
    }

    #[test]
    fn has_visible_devices_includes_completed_rows() {
        let mut state = RegistrationViewState {
            user_devices: vec![Fingerprint::from([0, 0, 0, 1])],
            ..Default::default()
        };
        state.sync_wallet(Some("desc".to_string()), vec![]);

        assert!(state.has_visible_devices());
    }
}
