/// Code input state for login
#[derive(Debug, Clone, Default)]
pub struct CodeState {
    pub form: liana_ui::component::form::Value<String>,
    pub processing: bool,
    pub can_resend_token: bool,
}

impl CodeState {
    pub fn new() -> Self {
        Self {
            form: liana_ui::component::form::Value {
                value: String::new(),
                warning: None,
                valid: true,
            },
            processing: false,
            can_resend_token: true,
        }
    }

    pub fn can_send(&self) -> bool {
        self.form.valid && !self.processing && self.form.value.chars().count() == 6
    }
}
