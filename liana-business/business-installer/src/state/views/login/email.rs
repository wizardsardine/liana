/// Email input state for login
#[derive(Debug, Clone, Default)]
pub struct EmailState {
    pub form: liana_ui::component::form::Value<String>,
    pub processing: bool,
}

impl EmailState {
    pub fn new() -> Self {
        Self {
            form: liana_ui::component::form::Value {
                // Test emails for different roles:
                // - ws@example.com    -> Manager for all wallets
                // - owner@example.com -> Owner (Draft/Validated/Final), Participant (Shared)
                // - user@example.com  -> Participant for all (Draft hidden)
                value: "user@example.com".to_string(),
                warning: None,
                valid: true,
            },
            processing: false,
        }
    }
    pub fn can_send(&self) -> bool {
        self.form.valid && !self.processing
    }
}
