pub mod conflict;
pub mod warning;

pub use conflict::{ConflictModalState, ConflictType};
pub use warning::WarningModalState;

/// Modal states (kept for compatibility, but modals are now in view states)
#[derive(Debug, Clone, Default)]
pub struct ModalsState {
    pub warning: Option<WarningModalState>,
    pub conflict: Option<ConflictModalState>,
}
