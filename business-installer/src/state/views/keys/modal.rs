/// State for Edit Key modal
#[derive(Debug, Clone)]
pub struct EditKeyModalState {
    pub key_id: u8,
    pub alias: String,
    pub description: String,
    pub email: String,
    pub key_type: crate::models::KeyType,
}

