#[derive(Debug, Clone)]
pub struct TemplateHelpModalState {
    pub wallet_name: String,
}

impl TemplateHelpModalState {
    pub fn new(wallet_name: String) -> Self {
        Self { wallet_name }
    }
}
