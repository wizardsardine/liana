use liana_connect::UserRole;
use rand::Rng;
use std::collections::HashMap;
use uuid::Uuid;

/// Test user configuration
#[derive(Debug, Clone)]
pub struct TestUser {
    pub email: String,
    pub role: UserRole,
    pub otp_code: String,
}

/// Simple token-based authentication with per-user OTP codes
pub struct AuthManager {
    /// Maps email -> TestUser (with OTP code)
    users: HashMap<String, TestUser>,
}

impl AuthManager {
    pub fn new() -> Self {
        let mut users = HashMap::new();
        let mut rng = rand::thread_rng();

        // Generate 6-digit OTP codes for each test user
        let test_users = vec![
            ("ws@example.com", "WS Manager", UserRole::WSManager),
            ("owner@example.com", "Wallet Owner", UserRole::Owner),
            (
                "user@example.com",
                "Participant User",
                UserRole::Participant,
            ),
            (
                "shared-owner@example.com",
                "Shared Wallet Owner",
                UserRole::Owner,
            ),
            ("bob@example.com", "Bob", UserRole::Participant),
            ("alice@example.com", "Alice", UserRole::Participant),
        ];

        for (email, _name, role) in test_users {
            let otp_code = format!("{:06}", rng.gen_range(100000..999999));
            users.insert(
                email.to_string(),
                TestUser {
                    email: email.to_string(),
                    role,
                    otp_code,
                },
            );
        }

        Self { users }
    }

    /// Validate OTP code for a given email
    /// Returns the user if valid, None otherwise
    pub fn validate_otp(&self, email: &str, code: &str) -> Option<&TestUser> {
        self.users.get(email).filter(|user| user.otp_code == code)
    }

    /// Validate an access token (JWT-like)
    /// Token format: "access-token-{uuid}"
    /// Returns the user UUID if valid
    pub fn validate_token(&self, token: &str) -> Option<Uuid> {
        let uuid_str = token.strip_prefix("access-token-")?;
        Uuid::parse_str(uuid_str).ok()
    }

    /// Check if an email is a registered test user
    pub fn is_registered(&self, email: &str) -> bool {
        self.users.contains_key(email)
    }

    /// Get all test users for display
    pub fn get_all_users(&self) -> &HashMap<String, TestUser> {
        &self.users
    }
}
