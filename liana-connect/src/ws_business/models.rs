//! Domain Models
//!
//! This module contains the core domain types used by Liana Connect
//! for representing organizations, wallets, users, and policy templates.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display};

use miniscript::DescriptorPublicKey;
use uuid::Uuid;

// ============================================================================
// Wallet Status
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletStatus {
    Created,   // Empty
    Drafted,   // Draft by WS manager
    Validated, // Policy validated by owner, keys metadata not yet completed
    Finalized, // All key metadata filled, ready for prod
}

impl WalletStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            WalletStatus::Created => "Created",
            WalletStatus::Drafted => "Drafted",
            WalletStatus::Validated => "Validated",
            WalletStatus::Finalized => "Finalized",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Created" => Some(WalletStatus::Created),
            "Drafted" => Some(WalletStatus::Drafted),
            "Validated" => Some(WalletStatus::Validated),
            "Finalized" => Some(WalletStatus::Finalized),
            _ => None,
        }
    }
}

// ============================================================================
// User Role
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserRole {
    WSManager,
    Owner,
    Participant,
}

impl UserRole {
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "WSManager" => Some(UserRole::WSManager),
            "Owner" => Some(UserRole::Owner),
            "Participant" => Some(UserRole::Participant),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            UserRole::WSManager => "WSManager",
            UserRole::Owner => "Owner",
            UserRole::Participant => "Participant",
        }
    }
}

// ============================================================================
// Key Type
// ============================================================================

/// Type of key
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyType {
    Internal,
    External,
    Cosigner,
    SafetyNet,
}

impl fmt::Display for KeyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl KeyType {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyType::Internal => "Internal",
            KeyType::External => "External",
            KeyType::Cosigner => "Cosigner",
            KeyType::SafetyNet => "Safety Net",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Internal" => Some(KeyType::Internal),
            "External" => Some(KeyType::External),
            "Cosigner" => Some(KeyType::Cosigner),
            "SafetyNet" => Some(KeyType::SafetyNet),
            _ => None,
        }
    }

    pub fn all() -> Vec<KeyType> {
        vec![
            KeyType::Internal,
            KeyType::External,
            KeyType::Cosigner,
            KeyType::SafetyNet,
        ]
    }
}

// ============================================================================
// Key
// ============================================================================

/// Represents a key in the spending policy
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    pub id: u8,
    pub alias: String,
    pub description: String,
    pub email: String,
    pub key_type: KeyType,
    pub xpub: Option<DescriptorPublicKey>,
    pub last_edited: Option<u64>,
    pub last_editor: Option<Uuid>,
}

// ============================================================================
// Timelock
// ============================================================================

/// Represents a timelock duration in blocks
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Timelock {
    pub blocks: u64,
}

impl Display for Timelock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.blocks == 0 {
            return write!(f, "0 blocks");
        }

        // Approximate conversions (1 block ≈ 10 minutes)
        const BLOCKS_PER_DAY: u64 = 144; // 24 * 60 / 10
        const BLOCKS_PER_MONTH: u64 = 4380; // ~30.4 days
        const BLOCKS_PER_YEAR: u64 = 52560; // ~365 days

        let mut remaining = self.blocks;
        let mut parts = Vec::new();

        // Years
        if remaining >= BLOCKS_PER_YEAR {
            let years = remaining / BLOCKS_PER_YEAR;
            parts.push(format!("{}y", years));
            remaining %= BLOCKS_PER_YEAR;
        }

        // Months
        if remaining >= BLOCKS_PER_MONTH {
            let months = remaining / BLOCKS_PER_MONTH;
            parts.push(format!("{}m", months));
            remaining %= BLOCKS_PER_MONTH;
        }

        // Days
        if remaining >= BLOCKS_PER_DAY {
            let days = remaining / BLOCKS_PER_DAY;
            parts.push(format!("{}d", days));
            remaining %= BLOCKS_PER_DAY;
        }

        // Blocks (only show if there are no larger units)
        if parts.is_empty() {
            parts.push(format!("{} blocks", remaining));
        }

        write!(f, "{}", parts.join(" "))
    }
}

impl Timelock {
    pub fn new(blocks: u64) -> Self {
        Self { blocks }
    }

    pub fn is_zero(&self) -> bool {
        self.blocks == 0
    }
}

// ============================================================================
// Spending Path
// ============================================================================

/// Represents a spending path in the policy
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpendingPath {
    pub is_primary: bool,
    pub threshold_n: u8,
    pub key_ids: Vec<u8>,
    pub last_edited: Option<u64>,
    pub last_editor: Option<Uuid>,
}

impl SpendingPath {
    pub fn new(is_primary: bool, threshold_n: u8, key_ids: Vec<u8>) -> Self {
        Self {
            is_primary,
            threshold_n,
            key_ids,
            last_edited: None,
            last_editor: None,
        }
    }

    /// Get threshold_m which is always key_ids.len()
    pub fn threshold_m(&self) -> usize {
        self.key_ids.len()
    }

    /// Validate that threshold_n is valid for the current key_ids
    pub fn is_valid(&self) -> bool {
        let m = self.key_ids.len();
        self.threshold_n > 0 && (self.threshold_n as usize) <= m && m > 0
    }

    /// Check if a key_id is already in this path
    pub fn contains_key(&self, key_id: u8) -> bool {
        self.key_ids.contains(&key_id)
    }
}

// ============================================================================
// Policy Template
// ============================================================================

/// Template structure containing all policy data
#[derive(Debug, Clone)]
pub struct PolicyTemplate {
    pub keys: BTreeMap<u8, Key>,
    pub primary_path: SpendingPath,
    pub secondary_paths: Vec<(SpendingPath, Timelock)>,
}

impl PolicyTemplate {
    pub fn new() -> Self {
        Self {
            keys: BTreeMap::new(),
            primary_path: SpendingPath::new(true, 1, Vec::new()),
            secondary_paths: Vec::new(),
        }
    }
}

impl Default for PolicyTemplate {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Organization
// ============================================================================

#[derive(Debug, Clone)]
pub struct Org {
    pub name: String,
    pub id: Uuid,
    pub wallets: BTreeSet<Uuid>,
    pub users: BTreeSet<Uuid>,
    pub owners: Vec<Uuid>,
    pub last_edited: Option<u64>,
    pub last_editor: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct OrgData {
    pub name: String,
    pub id: Uuid,
    pub wallets: BTreeMap<Uuid, Wallet>,
    pub users: BTreeSet<Uuid>,
    pub owners: Vec<Uuid>,
}

// ============================================================================
// User
// ============================================================================

#[derive(Debug, Clone)]
pub struct User {
    pub name: String,
    pub uuid: Uuid,
    pub email: String,
    pub orgs: Vec<Uuid>,
    pub role: UserRole,
    pub last_edited: Option<u64>,
    pub last_editor: Option<Uuid>,
}

// ============================================================================
// Wallet
// ============================================================================

#[derive(Debug, Clone)]
pub struct Wallet {
    pub alias: String,
    pub org: Uuid,
    pub owner: User,
    pub id: Uuid,
    pub status: WalletStatus,
    pub template: Option<PolicyTemplate>,
    pub last_edited: Option<u64>,
    pub last_editor: Option<Uuid>,
}
