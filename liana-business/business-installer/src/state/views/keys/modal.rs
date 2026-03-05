use liana_connect::ws_business;

/// State for Edit Key modal
#[derive(Debug, Clone)]
pub struct EditKeyModalState {
    pub key_id: u8,
    pub alias: String,
    pub description: String,
    pub key_type: ws_business::KeyType,
    pub is_new: bool,
    // Identity fields - only one is active based on key_type
    pub email: String,
    pub token: String,
    // Token validation state
    pub token_warning: Option<&'static str>,
}
