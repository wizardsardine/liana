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
