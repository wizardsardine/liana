/// Warning modal state
#[derive(Debug, Clone)]
pub struct WarningModalState {
    pub title: String,
    pub message: String,
}

impl WarningModalState {
    pub fn new(title: String, message: String) -> Self {
        Self { title, message }
    }
}
