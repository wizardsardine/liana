use uuid::Uuid;

/// Conflict resolution modal state
/// Shown when a resource being edited is modified or deleted by another user
#[derive(Debug, Clone)]
pub struct ConflictModalState {
    pub conflict_type: ConflictType,
    pub title: String,
    pub message: String,
}

/// Type of conflict that occurred
#[derive(Debug, Clone)]
pub enum ConflictType {
    /// Key was modified by another user while modal was open
    KeyModified { key_id: u8, wallet_id: Uuid },
    /// Key was deleted by another user while modal was open
    KeyDeleted { key_id: u8, wallet_id: Uuid },
    /// Path was modified by another user while modal was open
    PathModified {
        is_primary: bool,
        path_index: Option<usize>,
        wallet_id: Uuid,
    },
    /// Path was deleted by another user while modal was open
    PathDeleted {
        is_primary: bool,
        path_index: usize,
        wallet_id: Uuid,
    },
    /// Key used in currently edited path was deleted
    KeyInPathDeleted { key_id: u8, key_alias: String },
}

impl ConflictModalState {
    /// Returns true if this is a choice-based conflict (reload/keep options)
    /// vs info-only conflict (just OK to dismiss)
    pub fn is_choice(&self) -> bool {
        matches!(
            self.conflict_type,
            ConflictType::KeyModified { .. } | ConflictType::PathModified { .. }
        )
    }
}
