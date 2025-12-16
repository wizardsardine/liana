/// State for Edit Path modal (handles both threshold and timelock)
#[derive(Debug, Clone)]
pub struct EditPathModalState {
    pub is_primary: bool,
    pub path_index: Option<usize>, // None for primary, Some(index) for secondary
    pub threshold: String,
    pub timelock: Option<String>, // None for primary paths, Some for secondary paths
}

