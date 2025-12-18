use liana_connect::{Key, KeyType, PolicyTemplate, SpendingPath, Timelock};
use liana_connect::models::UserRole;
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
    /// Current user's role for the selected wallet
    pub current_user_role: Option<UserRole>,
    /// Flag to track intentional reconnection (don't show error on disconnect)
    pub reconnecting: bool,
    /// Flag to signal exit to Liana Lite login
    pub exit_to_liana_lite: bool,
}

impl AppState {
    /// Sort secondary paths by timelock (ascending order)
    pub fn sort_secondary_paths(&mut self) {
        self.secondary_paths.sort_by(|a, b| a.1.blocks.cmp(&b.1.blocks));
    }

    pub fn new() -> Self {
        // Test data keys
        let mut keys = BTreeMap::new();
        keys.insert(
            0,
            Key {
                id: 0,
                alias: "Owner".to_string(),
                description: "Owner key".to_string(),
                email: "owner@example.com".to_string(),
                key_type: KeyType::Internal,
                xpub: None,
            },
        );
        keys.insert(
            1,
            Key {
                id: 1,
                alias: "Bob".to_string(),
                description: "Bob's key".to_string(),
                email: "bob@example.com".to_string(),
                key_type: KeyType::External,
                xpub: None,
            },
        );
        keys.insert(
            2,
            Key {
                id: 2,
                alias: "Alice".to_string(),
                description: "Alice's key".to_string(),
                email: "alice@example.com".to_string(),
                key_type: KeyType::External,
                xpub: None,
            },
        );

        // Primary path: All of Owner, Bob (threshold 2 of 2)
        let primary_path = SpendingPath::new(true, 2, vec![0, 1]);

        // Secondary paths
        let mut secondary_paths = Vec::new();

        // Secondary path 1: 1 of Alice, Bob - After 2 months (8760 blocks)
        let secondary1 = SpendingPath::new(false, 1, vec![2, 1]);
        let timelock1 = Timelock::new(8760); // 2 months
        secondary_paths.push((secondary1, timelock1));

        // Secondary path 2: All of Owner - After 5 months (21900 blocks)
        let secondary2 = SpendingPath::new(false, 1, vec![0]);
        let timelock2 = Timelock::new(21900); // 5 months
        secondary_paths.push((secondary2, timelock2));

        Self {
            keys,
            primary_path,
            secondary_paths,
            next_key_id: 3,
            selected_org: None,
            selected_wallet: None,
            current_wallet_template: None,
            current_user_role: None,
            reconnecting: false,
            exit_to_liana_lite: false,
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
            current_user_role: None,
            reconnecting: false,
            exit_to_liana_lite: false,
        }
    }
}
