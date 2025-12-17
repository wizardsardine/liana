/// State for the organization selection view
#[derive(Debug, Clone)]
pub struct OrgSelectState {
    /// Search filter text for organization names
    pub search_filter: String,
}

impl Default for OrgSelectState {
    fn default() -> Self {
        Self {
            search_filter: String::new(),
        }
    }
}

