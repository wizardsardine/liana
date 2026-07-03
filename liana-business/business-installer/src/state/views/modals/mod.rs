pub mod conflict;
pub mod template_help;
pub mod warning;

pub use conflict::{ConflictModalState, ConflictType};
pub use template_help::TemplateHelpModalState;
pub use warning::WarningModalState;

/// Modal states (kept for compatibility, but modals are now in view states)
#[derive(Debug, Clone, Default)]
pub struct ModalsState {
    pub warning: Option<WarningModalState>,
    pub template_help: Option<TemplateHelpModalState>,
    pub conflict: Option<ConflictModalState>,
}
