use crate::models::{Key, PolicyTemplate, SpendingPath, Timelock};
use std::collections::BTreeMap;
use uuid::Uuid;

/// Core application data
#[derive(Debug, Clone)]
pub struct AppState {
    pub keys: BTreeMap<u8, Key>,
    pub primary_path: SpendingPath,
    pub secondary_paths: Vec<(SpendingPath, Timelock)>,
    pub next_key_id: u8,
    // Backend-related state
    pub selected_org: Option<Uuid>,
    pub selected_wallet: Option<Uuid>,
    pub current_wallet_template: Option<PolicyTemplate>,
    /// Flag to track intentional reconnection (don't show error on disconnect)
    pub reconnecting: bool,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
            primary_path: SpendingPath::new(true, 1, Vec::new()),
            secondary_paths: Vec::new(),
            next_key_id: 0,
            selected_org: None,
            selected_wallet: None,
            current_wallet_template: None,
            reconnecting: false,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl From<AppState> for PolicyTemplate {
    fn from(app_state: AppState) -> Self {
        PolicyTemplate {
            keys: app_state.keys,
            primary_path: app_state.primary_path,
            secondary_paths: app_state.secondary_paths,
        }
    }
}

impl From<PolicyTemplate> for AppState {
    fn from(template: PolicyTemplate) -> Self {
        // Calculate next_key_id from the maximum key ID in the template
        let next_key_id = template
            .keys
            .keys()
            .max()
            .map(|&id| id.wrapping_add(1))
            .unwrap_or(0);

        AppState {
            keys: template.keys,
            primary_path: template.primary_path,
            secondary_paths: template.secondary_paths,
            next_key_id,
            selected_org: None,
            selected_wallet: None,
            current_wallet_template: None,
            reconnecting: false,
        }
    }
}
