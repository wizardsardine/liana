use liana_connect::ws_business::{Key, PolicyTemplate, SecondaryPath, SpendingPath, UserRole};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// Core application data
#[derive(Debug, Clone)]
pub struct AppState {
    pub keys: BTreeMap<u8, Key>,
    pub primary_path: SpendingPath,
    pub secondary_paths: Vec<SecondaryPath>,
    pub keys_ready: bool,
    pub next_key_id: u8,
    // Backend-related state
    pub selected_org: Option<Uuid>,
    pub selected_wallet: Option<Uuid>,
    pub current_wallet_template: Option<PolicyTemplate>,
    /// Current user's role for the selected wallet (set when wallet is selected)
    pub current_user_role: Option<UserRole>,
    /// Logged-in user's global role from their User record
    pub global_user_role: Option<UserRole>,
    /// Flag to track intentional reconnection (don't show error on disconnect)
    pub reconnecting: bool,
    /// Flag to signal exit
    pub exit: bool,
    /// Server time offset in seconds (server_time - client_time)
    pub server_time_offset: i64,
}

impl AppState {
    /// Sort secondary paths by timelock (ascending order)
    pub fn sort_secondary_paths(&mut self) {
        self.secondary_paths
            .sort_by(|a, b| a.timelock.blocks.cmp(&b.timelock.blocks));
    }

    /// Get current time in seconds, adjusted for server time offset
    fn now_adjusted(&self) -> u64 {
        let client_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        (client_ts as i64 + self.server_time_offset) as u64
    }

    /// Format a server timestamp as relative time (e.g., "5 minutes ago")
    pub fn format_relative_time(&self, server_timestamp: u64) -> String {
        let now = self.now_adjusted();
        if server_timestamp > now {
            return "just now".to_string();
        }
        let diff = now - server_timestamp;

        const MINUTE: u64 = 60;
        const HOUR: u64 = 60 * MINUTE;
        const DAY: u64 = 24 * HOUR;
        const WEEK: u64 = 7 * DAY;
        const MONTH: u64 = 30 * DAY;

        if diff < MINUTE {
            "just now".to_string()
        } else if diff < HOUR {
            let mins = diff / MINUTE;
            if mins == 1 {
                "1 minute ago".to_string()
            } else {
                format!("{mins} minutes ago")
            }
        } else if diff < DAY {
            let hours = diff / HOUR;
            if hours == 1 {
                "1 hour ago".to_string()
            } else {
                format!("{hours} hours ago")
            }
        } else if diff < WEEK {
            let days = diff / DAY;
            if days == 1 {
                "1 day ago".to_string()
            } else {
                format!("{days} days ago")
            }
        } else if diff < MONTH {
            let weeks = diff / WEEK;
            if weeks == 1 {
                "1 week ago".to_string()
            } else {
                format!("{weeks} weeks ago")
            }
        } else {
            let months = diff / MONTH;
            if months == 1 {
                "1 month ago".to_string()
            } else {
                format!("{months} months ago")
            }
        }
    }

    /// Empty initial state. Debug overlays seed test data via the
    /// `seed_test_data` helper in `crate::debug`.
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
            primary_path: SpendingPath::new(true, 0, Vec::new()),
            secondary_paths: Vec::new(),
            keys_ready: false,
            next_key_id: 0,
            selected_org: None,
            selected_wallet: None,
            current_wallet_template: None,
            current_user_role: None,
            global_user_role: None,
            reconnecting: false,
            exit: false,
            server_time_offset: 0,
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
            keys_ready: app_state.keys_ready,
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
            keys_ready: template.keys_ready,
            next_key_id,
            selected_org: None,
            selected_wallet: None,
            current_wallet_template: None,
            current_user_role: None,
            global_user_role: None,
            reconnecting: false,
            exit: false,
            server_time_offset: 0,
        }
    }
}
