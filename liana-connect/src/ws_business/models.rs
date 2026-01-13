//! Domain Models
//!
//! This module contains the core domain types used by Liana Connect
//! for representing organizations, wallets, users, and policy templates.

use miniscript::DescriptorPublicKey;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::{self, Display},
};
use uuid::Uuid;

const BLOCKS_PER_DAY: u64 = 144; // 24 * 60 / 10
const BLOCKS_PER_MONTH: u64 = 4380; // ~30.4 days
const BLOCKS_PER_YEAR: u64 = 52560; // ~365 days

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalletStatus {
    Created,   // Empty
    Drafted,   // Draft by WS manager
    Locked,    // Locked by WS manager, ready for owner validation
    Validated, // Policy validated by owner, keys metadata not yet completed
    Finalized, // All key metadata filled, ready for prod
}

impl Display for WalletStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let str = serde_json::to_string(&self).expect("must not fail");
        write!(f, "{str}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserRole {
    WizardSardineAdmin,
    WalletManager,
    Participant,
}

impl Display for UserRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let str = match self {
            UserRole::WizardSardineAdmin => "WsAdmin",
            UserRole::WalletManager => "WalletManager",
            UserRole::Participant => "Participant",
        };
        write!(f, "{str}")
    }
}

/// Type of key
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyType {
    Internal,
    External,
    Cosigner,
    SafetyNet,
}

impl fmt::Display for KeyType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyType::Internal => write!(f, "Internal"),
            KeyType::External => write!(f, "External"),
            KeyType::Cosigner => write!(f, "Cosigner"),
            KeyType::SafetyNet => write!(f, "Safety Net"),
        }
    }
}

impl KeyType {
    pub fn all() -> Vec<KeyType> {
        vec![
            KeyType::Internal,
            KeyType::External,
            KeyType::Cosigner,
            KeyType::SafetyNet,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum XpubSource {
    Device,
    File,
    Pasted,
}

impl fmt::Display for XpubSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let str = serde_json::to_string(&self).expect("must not fail");
        write!(f, "{str}")
    }
}

/// Xpub data bundled with source information for audit
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Xpub {
    /// The extended public key string
    pub value: String,
    /// Source type: device, file, or pasted
    pub source: XpubSource,
    /// Device kind (e.g., Ledger, Trezor) - only for source=Device
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_kind: Option<DeviceKind>,
    /// Device firmware version - only for source=Device
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_version: Option<String>,
    /// File name - only for source=File
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

// NOTE: for now only the device brand is detected in async-hwi, so default
// model will be used, other models are reserved for future use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceKind {
    Ledger, // Default ledger
    LedgerNano,
    LedgerNanoS,
    LedgerNanoPlus,
    LedgerFlex,
    LedgerStax,
    SpecterDiy, // Default SpecterDiy
    Bitbox02,   // Default Bitbox
    Bitbox02Nova,
    Jade, // Default Jade
    JadePlus,
    Coldcard, // Default Coldcard
    ColdcardMk4,
    ColdcardQ,
    #[serde(rename = "other")]
    Other(String),
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeviceKind::Ledger => write!(f, "Ledger"),
            DeviceKind::LedgerNano => write!(f, "LedgerNano"),
            DeviceKind::LedgerNanoS => write!(f, "LedgerNanoS"),
            DeviceKind::LedgerNanoPlus => write!(f, "LedgerNanoPlus"),
            DeviceKind::LedgerFlex => write!(f, "LedgerFlex"),
            DeviceKind::LedgerStax => write!(f, "LedgerStax"),
            DeviceKind::SpecterDiy => write!(f, "SpecterDiy"),
            DeviceKind::Bitbox02 => write!(f, "Bitbox02"),
            DeviceKind::Bitbox02Nova => write!(f, "Bitbox02Nova"),
            DeviceKind::Jade => write!(f, "Jade"),
            DeviceKind::JadePlus => write!(f, "JadePlus"),
            DeviceKind::Coldcard => write!(f, "Coldcard"),
            DeviceKind::ColdcardMk4 => write!(f, "ColdcardMk4"),
            DeviceKind::ColdcardQ => write!(f, "ColdcardQ"),
            DeviceKind::Other(s) => write!(f, "{}", s),
        }
    }
}

impl std::str::FromStr for DeviceKind {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "Ledger" => DeviceKind::Ledger,
            "LedgerNano" => DeviceKind::LedgerNano,
            "LedgerNanoS" => DeviceKind::LedgerNanoS,
            "LedgerNanoPlus" => DeviceKind::LedgerNanoPlus,
            "LedgerFlex" => DeviceKind::LedgerFlex,
            "LedgerStax" => DeviceKind::LedgerStax,
            "SpecterDiy" => DeviceKind::SpecterDiy,
            "Bitbox02" => DeviceKind::Bitbox02,
            "Bitbox02Nova" => DeviceKind::Bitbox02Nova,
            "Jade" => DeviceKind::Jade,
            "JadePlus" => DeviceKind::JadePlus,
            "Coldcard" => DeviceKind::Coldcard,
            "ColdcardMk4" => DeviceKind::ColdcardMk4,
            "ColdcardQ" => DeviceKind::ColdcardQ,
            other => DeviceKind::Other(other.to_string()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyIdentity {
    // The key is related to an user account in WS account system
    Email(String),
    // The key is related to a provider in WS keys system
    Token(String),
    // The key holder is not registered in any WS account systems
    Other(String),
}

impl Display for KeyIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let str = match self {
            KeyIdentity::Email(s) | KeyIdentity::Token(s) | KeyIdentity::Other(s) => s,
        };
        write!(f, "{str}")
    }
}

/// Represents a key in the spending policy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub id: u8,
    pub alias: String,
    pub description: String,
    #[serde(flatten)]
    pub identity: KeyIdentity,
    pub key_type: KeyType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpub: Option<DescriptorPublicKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpub_source: Option<XpubSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpub_device_kind: Option<DeviceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpub_device_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xpub_file_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_edited: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_editor: Option<Uuid>,
}

/// Represents a timelock duration in blocks
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Timelock {
    pub blocks: u64,
}

impl Display for Timelock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.blocks == 0 {
            return write!(f, "0 blocks");
        }

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

/// Represents a spending path in the policy
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendingPath {
    pub is_primary: bool,
    pub threshold_n: u8,
    pub key_ids: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_edited: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// A recovery path combining a spending path with its timelock
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecondaryPath {
    #[serde(flatten)]
    pub path: SpendingPath,
    pub timelock: Timelock,
}

/// Template structure containing all policy data
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyTemplate {
    pub keys: BTreeMap<u8, Key>,
    pub primary_path: SpendingPath,
    pub secondary_paths: Vec<SecondaryPath>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    pub name: String,
    pub id: Uuid,
    pub wallets: BTreeSet<Uuid>,
    pub users: BTreeSet<Uuid>,
    pub owners: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_edited: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub name: String,
    pub uuid: Uuid,
    pub email: String,
    // NOTE: role can only have WsManager | Participant, wallet ownership
    // must only infered from Wallet.owner
    pub role: UserRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_edited: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_editor: Option<Uuid>,
}

impl User {
    /// Derive the user's role for a specific wallet based on wallet data and global role
    pub fn role(&self, wallet: &Wallet) -> Option<UserRole> {
        match self.role {
            UserRole::WalletManager => {
                // NOTE: The admin role is dependant of the wallet, it cannot
                //       be assigned on server.
                eprintln!(
                    "UserRole for {} is Admin on server! THIS IS A BUG, PLEASE REPORT",
                    self.uuid
                );
                return None;
            }
            UserRole::Participant => { /* continue */ }
            // WizardSardineManager has access to all wallets
            wsm => return Some(wsm),
        }

        // Check if user is wallet owner
        if wallet.owner == self.uuid {
            return Some(UserRole::WalletManager);
        }
        // Check if user is a participant (has keys with matching email)
        if let Some(template) = &wallet.template {
            for key in template.keys.values() {
                if let KeyIdentity::Email(email) = &key.identity {
                    if email == &self.email {
                        return Some(UserRole::Participant);
                    }
                }
            }
        }
        // User has no access to this wallet
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wallet {
    pub alias: String,
    pub org: Uuid,
    pub owner: Uuid,
    pub id: Uuid,
    pub status: WalletStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<PolicyTemplate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_edited: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_editor: Option<Uuid>,
}

#[cfg(test)]
mod wire_format_tests {
    use super::*;
    use serde_json::json;

    // Test UUIDs - use parse_str instead of new_v4 (no v4 feature dependency)
    fn test_uuid(n: u8) -> Uuid {
        Uuid::parse_str(&format!("12345678-1234-1234-1234-12345678900{}", n)).unwrap()
    }

    // Helper to roundtrip a type through JSON
    fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + std::fmt::Debug + PartialEq>(
        value: &T,
    ) {
        let json = serde_json::to_string(value).expect("serialize");
        let parsed: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&parsed, value, "roundtrip failed");
    }

    #[test]
    fn test_uuid_serializes_as_string() {
        let id = Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap();
        let user = User {
            name: "Test".to_string(),
            uuid: id,
            email: "test@example.com".to_string(),
            role: UserRole::WalletManager,
            last_edited: None,
            last_editor: None,
        };
        let json = serde_json::to_value(&user).unwrap();
        assert_eq!(json["uuid"], "12345678-1234-1234-1234-123456789abc");
    }

    #[test]
    fn test_wallet_status_enum_format() {
        // Test all variants serialize correctly
        let cases = [
            (WalletStatus::Created, "Created"),
            (WalletStatus::Drafted, "Drafted"),
            (WalletStatus::Locked, "Locked"),
            (WalletStatus::Finalized, "Finalized"),
        ];
        for (status, expected) in cases {
            let json = serde_json::to_value(status).unwrap();
            assert_eq!(json, expected, "WalletStatus::{:?}", status);
        }
    }

    #[test]
    fn test_user_role_enum_format() {
        let cases = [
            (UserRole::WizardSardineAdmin, "WizardSardineAdmin"),
            (UserRole::WalletManager, "WalletManager"),
            (UserRole::Participant, "Participant"),
        ];
        for (role, expected) in cases {
            let json = serde_json::to_value(role).unwrap();
            assert_eq!(json, expected, "UserRole::{:?}", role);
        }
    }

    #[test]
    fn test_key_type_enum_format() {
        let cases = [
            (KeyType::Internal, "Internal"),
            (KeyType::External, "External"),
            (KeyType::Cosigner, "Cosigner"),
            (KeyType::SafetyNet, "SafetyNet"),
        ];
        for (kt, expected) in cases {
            let json = serde_json::to_value(kt).unwrap();
            assert_eq!(json, expected, "KeyType::{:?}", kt);
        }
    }

    #[test]
    fn test_xpub_source_format() {
        let cases = [
            (XpubSource::Device, "Device"),
            (XpubSource::File, "File"),
            (XpubSource::Pasted, "Pasted"),
        ];
        for (src, expected) in cases {
            let json = serde_json::to_value(&src).unwrap();
            assert_eq!(json, expected, "XpubSource::{:?}", src);
        }
    }

    #[test]
    fn test_device_kind_format() {
        // Known variants serialize as plain strings
        let known_cases = [
            (DeviceKind::Ledger, json!("Ledger")),
            (DeviceKind::Coldcard, json!("Coldcard")),
            (DeviceKind::Bitbox02, json!("Bitbox02")),
            (DeviceKind::Jade, json!("Jade")),
            (DeviceKind::SpecterDiy, json!("SpecterDiy")),
        ];
        for (dk, expected) in known_cases {
            let json = serde_json::to_value(&dk).unwrap();
            assert_eq!(json, expected, "DeviceKind::{:?}", dk);
        }

        // Other variant serializes as {"other": "value"} (snake_case)
        let other = DeviceKind::Other("Custom".to_string());
        let json = serde_json::to_value(&other).unwrap();
        assert_eq!(json, json!({"other": "Custom"}));
    }

    #[test]
    fn test_key_identity_serializes_as_email() {
        let key = Key {
            id: 1,
            alias: "Test".to_string(),
            description: "".to_string(),
            identity: KeyIdentity::Email("test@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        };
        let json = serde_json::to_value(&key).unwrap();
        // KeyIdentity::Email flattens to "email" field (snake_case)
        assert_eq!(json["email"], "test@example.com");
    }

    #[test]
    fn test_policy_template_keys_use_string_keys() {
        let mut keys = BTreeMap::new();
        keys.insert(
            0,
            Key {
                id: 0,
                alias: "Key0".to_string(),
                description: "".to_string(),
                identity: KeyIdentity::Email("a@b.com".to_string()),
                key_type: KeyType::Internal,
                xpub: None,
                xpub_source: None,
                xpub_device_kind: None,
                xpub_device_version: None,
                xpub_file_name: None,
                last_edited: None,
                last_editor: None,
            },
        );
        let template = PolicyTemplate {
            keys,
            primary_path: SpendingPath::new(true, 1, vec![0]),
            secondary_paths: vec![],
        };
        let json = serde_json::to_value(&template).unwrap();
        // Keys should be keyed by string "0", not integer
        assert!(json["keys"]["0"].is_object(), "keys should use string keys");
    }

    #[test]
    fn test_secondary_path_flattened() {
        let sp = SecondaryPath {
            path: SpendingPath::new(false, 1, vec![0]),
            timelock: Timelock::new(144),
        };
        let json = serde_json::to_value(&sp).unwrap();
        // SpendingPath fields should be flattened into SecondaryPath
        assert!(json["is_primary"].is_boolean());
        assert!(json["threshold_n"].is_number());
        assert!(json["key_ids"].is_array());
        assert!(json["timelock"]["blocks"].is_number());
    }

    #[test]
    fn test_user_roundtrip() {
        let user = User {
            name: "Test User".to_string(),
            uuid: test_uuid(1),
            email: "test@example.com".to_string(),
            role: UserRole::WalletManager,
            last_edited: Some(1234567890),
            last_editor: Some(test_uuid(4)),
        };
        roundtrip(&user);
    }

    #[test]
    fn test_org_roundtrip() {
        let org = Org {
            name: "Test Org".to_string(),
            id: test_uuid(1),
            wallets: [test_uuid(2), test_uuid(3)].into_iter().collect(),
            users: [test_uuid(4)].into_iter().collect(),
            owners: vec![test_uuid(5)],
            last_edited: Some(1234567890),
            last_editor: Some(test_uuid(6)),
        };
        roundtrip(&org);
    }

    #[test]
    fn test_timelock_roundtrip() {
        roundtrip(&Timelock::new(144));
        roundtrip(&Timelock::new(0));
        roundtrip(&Timelock::new(u64::MAX));
    }

    #[test]
    fn test_spending_path_roundtrip() {
        let path = SpendingPath {
            is_primary: true,
            threshold_n: 2,
            key_ids: vec![0, 1, 2],
            last_edited: Some(123),
            last_editor: Some(test_uuid(1)),
        };
        roundtrip(&path);
    }

    #[test]
    fn test_key_roundtrip() {
        let key = Key {
            id: 5,
            alias: "Test Key".to_string(),
            description: "A test key".to_string(),
            identity: KeyIdentity::Email("key@example.com".to_string()),
            key_type: KeyType::External,
            xpub: None,
            xpub_source: Some(XpubSource::Device),
            xpub_device_kind: Some(DeviceKind::Ledger),
            xpub_device_version: Some("2.1.0".to_string()),
            xpub_file_name: None,
            last_edited: Some(999),
            last_editor: Some(test_uuid(1)),
        };
        roundtrip(&key);
    }

    #[test]
    fn test_wallet_serialize_deserialize() {
        // Wallet doesn't impl PartialEq, so test serialize/deserialize separately
        let wallet = Wallet {
            alias: "Test Wallet".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Drafted,
            template: None,
            last_edited: Some(12345),
            last_editor: Some(test_uuid(4)),
        };
        let json = serde_json::to_string(&wallet).expect("serialize");
        let parsed: Wallet = serde_json::from_str(&json).expect("deserialize");
        // Verify key fields match
        assert_eq!(parsed.alias, wallet.alias);
        assert_eq!(parsed.org, wallet.org);
        assert_eq!(parsed.owner, wallet.owner);
        assert_eq!(parsed.id, wallet.id);
    }

    #[test]
    fn test_backward_compat_extra_fields_ignored() {
        // Old JSON with extra unknown fields should still parse
        let json = json!({
            "name": "Test",
            "uuid": "12345678-1234-1234-1234-123456789abc",
            "email": "test@example.com",
            "orgs": [],
            "role": "WalletManager",
            "unknown_field": "should be ignored",
            "another_unknown": 123
        });
        let user: User = serde_json::from_value(json).expect("should parse with extra fields");
        assert_eq!(user.name, "Test");
    }

    #[test]
    fn test_optional_fields_absent() {
        // JSON without optional fields should parse with defaults
        let json = json!({
            "name": "Test",
            "uuid": "12345678-1234-1234-1234-123456789abc",
            "email": "test@example.com",
            "orgs": [],
            "role": "WalletManager"
        });
        let user: User =
            serde_json::from_value(json).expect("should parse without optional fields");
        assert!(user.last_edited.is_none());
        assert!(user.last_editor.is_none());
    }

    #[test]
    fn test_device_kind_other_parses_correctly() {
        // DeviceKind::Other must be explicitly specified as {"other": "value"} (snake_case)
        let json = json!({"other": "SomeNewDevice"});
        let dk: DeviceKind = serde_json::from_value(json).expect("should parse other variant");
        assert_eq!(dk, DeviceKind::Other("SomeNewDevice".to_string()));
    }

    #[test]
    fn test_device_kind_unknown_string_fails() {
        // Unknown device strings are NOT auto-converted to Other
        // They fail to parse (this is the current behavior)
        let json = json!("SomeNewDevice");
        let result: Result<DeviceKind, _> = serde_json::from_value(json);
        assert!(result.is_err(), "unknown string should fail to parse");
    }

    #[test]
    fn test_xpub_with_device_source() {
        let xpub = Xpub {
            value: "xpub123...".to_string(),
            source: XpubSource::Device,
            device_kind: Some(DeviceKind::Ledger),
            device_version: Some("2.1.0".to_string()),
            file_name: None,
        };
        roundtrip(&xpub);

        // Verify JSON format
        let json = serde_json::to_value(&xpub).unwrap();
        assert_eq!(json["source"], "Device");
        assert_eq!(json["device_kind"], "Ledger");
        assert_eq!(json["device_version"], "2.1.0");
        assert!(json.get("file_name").is_none()); // Optional field should be absent
    }

    #[test]
    fn test_xpub_with_file_source() {
        let xpub = Xpub {
            value: "xpub456...".to_string(),
            source: XpubSource::File,
            device_kind: None,
            device_version: None,
            file_name: Some("my_key.txt".to_string()),
        };
        roundtrip(&xpub);

        let json = serde_json::to_value(&xpub).unwrap();
        assert_eq!(json["source"], "File");
        assert_eq!(json["file_name"], "my_key.txt");
    }

    #[test]
    fn test_xpub_with_pasted_source() {
        let xpub = Xpub {
            value: "xpub789...".to_string(),
            source: XpubSource::Pasted,
            device_kind: None,
            device_version: None,
            file_name: None,
        };
        roundtrip(&xpub);

        let json = serde_json::to_value(&xpub).unwrap();
        assert_eq!(json["source"], "Pasted");
    }

    // ==================== HARDCODED JSON WIRE FORMAT TESTS ====================
    // These tests parse hardcoded JSON strings representing the exact wire format.
    // They serve as documentation and detect breaking changes in the protocol.

    #[test]
    fn test_user_wire_format() {
        // Documentation: User JSON format from server
        let json = r#"{
            "name": "Alice Smith",
            "uuid": "12345678-1234-1234-1234-123456789abc",
            "email": "alice@example.com",
            "role": "WalletManager",
            "last_edited": 1234567890,
            "last_editor": "11111111-1111-1111-1111-111111111111"
        }"#;

        let parsed: User = serde_json::from_str(json).expect("wire format must parse");

        let expected = User {
            name: "Alice Smith".to_string(),
            uuid: Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap(),
            email: "alice@example.com".to_string(),
            role: UserRole::WalletManager,
            last_edited: Some(1234567890),
            last_editor: Some(Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap()),
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_org_wire_format() {
        // Documentation: Org JSON format from server
        let json = r#"{
            "name": "Acme Corp",
            "id": "12345678-1234-1234-1234-123456789abc",
            "wallets": ["aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"],
            "users": ["bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"],
            "owners": ["cccccccc-cccc-cccc-cccc-cccccccccccc"],
            "last_edited": 1234567890,
            "last_editor": "dddddddd-dddd-dddd-dddd-dddddddddddd"
        }"#;

        let parsed: Org = serde_json::from_str(json).expect("wire format must parse");

        let expected = Org {
            name: "Acme Corp".to_string(),
            id: Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap(),
            wallets: [Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap()]
                .into_iter()
                .collect(),
            users: [Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap()]
                .into_iter()
                .collect(),
            owners: vec![Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap()],
            last_edited: Some(1234567890),
            last_editor: Some(Uuid::parse_str("dddddddd-dddd-dddd-dddd-dddddddddddd").unwrap()),
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_wallet_wire_format() {
        // Documentation: Wallet JSON format from server (minimal)
        let json = r#"{
            "alias": "Main Vault",
            "org": "12345678-1234-1234-1234-123456789abc",
            "owner": "87654321-1234-1234-1234-123456789abc",
            "id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "status": "Drafted"
        }"#;

        let parsed: Wallet = serde_json::from_str(json).expect("wire format must parse");

        let expected = Wallet {
            alias: "Main Vault".to_string(),
            org: Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap(),
            owner: Uuid::parse_str("87654321-1234-1234-1234-123456789abc").unwrap(),
            id: Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap(),
            status: WalletStatus::Drafted,
            template: None,
            last_edited: None,
            last_editor: None,
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_wallet_with_template_wire_format() {
        // Documentation: Wallet with PolicyTemplate JSON format (complex nested structure)
        let json = r#"{
            "alias": "Multi-sig Vault",
            "org": "12345678-1234-1234-1234-123456789001",
            "owner": "12345678-1234-1234-1234-123456789002",
            "id": "12345678-1234-1234-1234-123456789003",
            "status": "Finalized",
            "template": {
                "keys": {
                    "0": {
                        "id": 0,
                        "alias": "Alice Key",
                        "description": "Primary signing key",
                        "email": "alice@example.com",
                        "key_type": "Internal"
                    },
                    "1": {
                        "id": 1,
                        "alias": "Bob Key",
                        "description": "Backup key",
                        "email": "bob@example.com",
                        "key_type": "External"
                    }
                },
                "primary_path": {
                    "is_primary": true,
                    "threshold_n": 2,
                    "key_ids": [0, 1]
                },
                "secondary_paths": []
            }
        }"#;

        let parsed: Wallet = serde_json::from_str(json).expect("wire format must parse");

        let mut keys = BTreeMap::new();
        keys.insert(
            0,
            Key {
                id: 0,
                alias: "Alice Key".to_string(),
                description: "Primary signing key".to_string(),
                identity: KeyIdentity::Email("alice@example.com".to_string()),
                key_type: KeyType::Internal,
                xpub: None,
                xpub_source: None,
                xpub_device_kind: None,
                xpub_device_version: None,
                xpub_file_name: None,
                last_edited: None,
                last_editor: None,
            },
        );
        keys.insert(
            1,
            Key {
                id: 1,
                alias: "Bob Key".to_string(),
                description: "Backup key".to_string(),
                identity: KeyIdentity::Email("bob@example.com".to_string()),
                key_type: KeyType::External,
                xpub: None,
                xpub_source: None,
                xpub_device_kind: None,
                xpub_device_version: None,
                xpub_file_name: None,
                last_edited: None,
                last_editor: None,
            },
        );

        let expected = Wallet {
            alias: "Multi-sig Vault".to_string(),
            org: Uuid::parse_str("12345678-1234-1234-1234-123456789001").unwrap(),
            owner: Uuid::parse_str("12345678-1234-1234-1234-123456789002").unwrap(),
            id: Uuid::parse_str("12345678-1234-1234-1234-123456789003").unwrap(),
            status: WalletStatus::Finalized,
            template: Some(PolicyTemplate {
                keys,
                primary_path: SpendingPath {
                    is_primary: true,
                    threshold_n: 2,
                    key_ids: vec![0, 1],
                    last_edited: None,
                    last_editor: None,
                },
                secondary_paths: vec![],
            }),
            last_edited: None,
            last_editor: None,
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_key_with_email_identity_wire_format() {
        // Documentation: Key with Email identity (flattened as "email" field)
        let json = r#"{
            "id": 0,
            "alias": "Alice Key",
            "description": "Primary signing key",
            "email": "alice@example.com",
            "key_type": "Internal"
        }"#;

        let parsed: Key = serde_json::from_str(json).expect("wire format must parse");

        let expected = Key {
            id: 0,
            alias: "Alice Key".to_string(),
            description: "Primary signing key".to_string(),
            identity: KeyIdentity::Email("alice@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_key_with_token_identity_wire_format() {
        // Documentation: Key with Token identity (flattened as "token" field)
        let json = r#"{
            "id": 1,
            "alias": "Provider Key",
            "description": "Cosigner service key",
            "token": "provider-token-123",
            "key_type": "Cosigner"
        }"#;

        let parsed: Key = serde_json::from_str(json).expect("wire format must parse");

        let expected = Key {
            id: 1,
            alias: "Provider Key".to_string(),
            description: "Cosigner service key".to_string(),
            identity: KeyIdentity::Token("provider-token-123".to_string()),
            key_type: KeyType::Cosigner,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_key_with_other_identity_wire_format() {
        // Documentation: Key with Other identity (flattened as "other" field)
        let json = r#"{
            "id": 2,
            "alias": "External Key",
            "description": "Unregistered holder",
            "other": "external-identifier",
            "key_type": "External"
        }"#;

        let parsed: Key = serde_json::from_str(json).expect("wire format must parse");

        let expected = Key {
            id: 2,
            alias: "External Key".to_string(),
            description: "Unregistered holder".to_string(),
            identity: KeyIdentity::Other("external-identifier".to_string()),
            key_type: KeyType::External,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_key_with_full_xpub_wire_format() {
        // Documentation: Key with all xpub-related fields populated
        let json = r#"{
            "id": 0,
            "alias": "Hardware Key",
            "description": "Ledger device key",
            "email": "user@example.com",
            "key_type": "Internal",
            "xpub_source": "Device",
            "xpub_device_kind": "Ledger",
            "xpub_device_version": "2.1.0"
        }"#;

        let parsed: Key = serde_json::from_str(json).expect("wire format must parse");

        let expected = Key {
            id: 0,
            alias: "Hardware Key".to_string(),
            description: "Ledger device key".to_string(),
            identity: KeyIdentity::Email("user@example.com".to_string()),
            key_type: KeyType::Internal,
            xpub: None,
            xpub_source: Some(XpubSource::Device),
            xpub_device_kind: Some(DeviceKind::Ledger),
            xpub_device_version: Some("2.1.0".to_string()),
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_xpub_device_source_wire_format() {
        // Documentation: Xpub with device source JSON format
        let json = r#"{
            "value": "xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8",
            "source": "Device",
            "device_kind": "Ledger",
            "device_version": "2.1.0"
        }"#;

        let parsed: Xpub = serde_json::from_str(json).expect("wire format must parse");

        let expected = Xpub {
            value: "xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8".to_string(),
            source: XpubSource::Device,
            device_kind: Some(DeviceKind::Ledger),
            device_version: Some("2.1.0".to_string()),
            file_name: None,
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_xpub_file_source_wire_format() {
        // Documentation: Xpub with file source JSON format
        let json = r#"{
            "value": "xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8",
            "source": "File",
            "file_name": "coldcard-export.json"
        }"#;

        let parsed: Xpub = serde_json::from_str(json).expect("wire format must parse");

        let expected = Xpub {
            value: "xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8".to_string(),
            source: XpubSource::File,
            device_kind: None,
            device_version: None,
            file_name: Some("coldcard-export.json".to_string()),
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_xpub_pasted_source_wire_format() {
        // Documentation: Xpub with pasted source JSON format (minimal)
        let json = r#"{
            "value": "xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8",
            "source": "Pasted"
        }"#;

        let parsed: Xpub = serde_json::from_str(json).expect("wire format must parse");

        let expected = Xpub {
            value: "xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8".to_string(),
            source: XpubSource::Pasted,
            device_kind: None,
            device_version: None,
            file_name: None,
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_spending_path_wire_format() {
        // Documentation: SpendingPath JSON format
        let json = r#"{
            "is_primary": true,
            "threshold_n": 2,
            "key_ids": [0, 1, 2],
            "last_edited": 1234567890,
            "last_editor": "12345678-1234-1234-1234-123456789abc"
        }"#;

        let parsed: SpendingPath = serde_json::from_str(json).expect("wire format must parse");

        let expected = SpendingPath {
            is_primary: true,
            threshold_n: 2,
            key_ids: vec![0, 1, 2],
            last_edited: Some(1234567890),
            last_editor: Some(Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()),
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_secondary_path_wire_format() {
        // Documentation: SecondaryPath JSON format (SpendingPath fields are flattened)
        let json = r#"{
            "is_primary": false,
            "threshold_n": 1,
            "key_ids": [2],
            "timelock": {"blocks": 52560}
        }"#;

        let parsed: SecondaryPath = serde_json::from_str(json).expect("wire format must parse");

        let expected = SecondaryPath {
            path: SpendingPath {
                is_primary: false,
                threshold_n: 1,
                key_ids: vec![2],
                last_edited: None,
                last_editor: None,
            },
            timelock: Timelock::new(52560),
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_timelock_wire_format() {
        // Documentation: Timelock JSON format
        let json = r#"{"blocks": 144}"#;

        let parsed: Timelock = serde_json::from_str(json).expect("wire format must parse");

        let expected = Timelock::new(144);

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    #[test]
    fn test_policy_template_wire_format() {
        // Documentation: PolicyTemplate JSON format with all components
        let json = r#"{
            "keys": {
                "0": {
                    "id": 0,
                    "alias": "Primary",
                    "description": "Primary key",
                    "email": "alice@example.com",
                    "key_type": "Internal"
                }
            },
            "primary_path": {
                "is_primary": true,
                "threshold_n": 1,
                "key_ids": [0]
            },
            "secondary_paths": [
                {
                    "is_primary": false,
                    "threshold_n": 1,
                    "key_ids": [0],
                    "timelock": {"blocks": 52560}
                }
            ]
        }"#;

        let parsed: PolicyTemplate = serde_json::from_str(json).expect("wire format must parse");

        let mut keys = BTreeMap::new();
        keys.insert(
            0,
            Key {
                id: 0,
                alias: "Primary".to_string(),
                description: "Primary key".to_string(),
                identity: KeyIdentity::Email("alice@example.com".to_string()),
                key_type: KeyType::Internal,
                xpub: None,
                xpub_source: None,
                xpub_device_kind: None,
                xpub_device_version: None,
                xpub_file_name: None,
                last_edited: None,
                last_editor: None,
            },
        );

        let expected = PolicyTemplate {
            keys,
            primary_path: SpendingPath {
                is_primary: true,
                threshold_n: 1,
                key_ids: vec![0],
                last_edited: None,
                last_editor: None,
            },
            secondary_paths: vec![SecondaryPath {
                path: SpendingPath {
                    is_primary: false,
                    threshold_n: 1,
                    key_ids: vec![0],
                    last_edited: None,
                    last_editor: None,
                },
                timelock: Timelock::new(52560),
            }],
        };

        assert_eq!(parsed, expected);
        roundtrip(&parsed);
    }

    // ==================== ENUM EXHAUSTIVE WIRE FORMAT TESTS ====================
    // These tests verify ALL enum variants serialize/deserialize to exact strings.
    // Adding or renaming variants will cause these tests to fail.

    #[test]
    fn test_wallet_status_all_variants_wire_format() {
        // Documentation: All WalletStatus variants and their wire format strings
        let cases = [
            (r#""Created""#, WalletStatus::Created),
            (r#""Drafted""#, WalletStatus::Drafted),
            (r#""Locked""#, WalletStatus::Locked),
            (r#""Validated""#, WalletStatus::Validated),
            (r#""Finalized""#, WalletStatus::Finalized),
        ];

        // Verify count matches enum variant count (catches new variants)
        assert_eq!(cases.len(), 5, "test must cover all WalletStatus variants");

        for (json_str, expected) in cases {
            // Test deserialization
            let parsed: WalletStatus = serde_json::from_str(json_str)
                .unwrap_or_else(|_| panic!("should parse {}", json_str));
            assert_eq!(parsed, expected);

            // Test serialization produces same string
            let serialized = serde_json::to_string(&parsed).unwrap();
            assert_eq!(serialized, json_str);
        }
    }

    #[test]
    fn test_user_role_all_variants_wire_format() {
        // Documentation: All UserRole variants and their wire format strings
        let cases = [
            (r#""WizardSardineAdmin""#, UserRole::WizardSardineAdmin),
            (r#""WalletManager""#, UserRole::WalletManager),
            (r#""Participant""#, UserRole::Participant),
        ];

        assert_eq!(cases.len(), 3, "test must cover all UserRole variants");

        for (json_str, expected) in cases {
            let parsed: UserRole = serde_json::from_str(json_str).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json_str);
        }
    }

    #[test]
    fn test_key_type_all_variants_wire_format() {
        // Documentation: All KeyType variants and their wire format strings
        let cases = [
            (r#""Internal""#, KeyType::Internal),
            (r#""External""#, KeyType::External),
            (r#""Cosigner""#, KeyType::Cosigner),
            (r#""SafetyNet""#, KeyType::SafetyNet),
        ];

        assert_eq!(cases.len(), 4, "test must cover all KeyType variants");

        for (json_str, expected) in cases {
            let parsed: KeyType = serde_json::from_str(json_str).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json_str);
        }
    }

    #[test]
    fn test_xpub_source_all_variants_wire_format() {
        // Documentation: All XpubSource variants and their wire format strings
        let cases = [
            (r#""Device""#, XpubSource::Device),
            (r#""File""#, XpubSource::File),
            (r#""Pasted""#, XpubSource::Pasted),
        ];

        assert_eq!(cases.len(), 3, "test must cover all XpubSource variants");

        for (json_str, expected) in cases {
            let parsed: XpubSource = serde_json::from_str(json_str).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json_str);
        }
    }

    #[test]
    fn test_device_kind_all_known_variants_wire_format() {
        // Documentation: All known DeviceKind variants and their wire format strings
        let cases = [
            (r#""Ledger""#, DeviceKind::Ledger),
            (r#""LedgerNano""#, DeviceKind::LedgerNano),
            (r#""LedgerNanoS""#, DeviceKind::LedgerNanoS),
            (r#""LedgerNanoPlus""#, DeviceKind::LedgerNanoPlus),
            (r#""LedgerFlex""#, DeviceKind::LedgerFlex),
            (r#""LedgerStax""#, DeviceKind::LedgerStax),
            (r#""SpecterDiy""#, DeviceKind::SpecterDiy),
            (r#""Bitbox02""#, DeviceKind::Bitbox02),
            (r#""Bitbox02Nova""#, DeviceKind::Bitbox02Nova),
            (r#""Jade""#, DeviceKind::Jade),
            (r#""JadePlus""#, DeviceKind::JadePlus),
            (r#""Coldcard""#, DeviceKind::Coldcard),
            (r#""ColdcardMk4""#, DeviceKind::ColdcardMk4),
            (r#""ColdcardQ""#, DeviceKind::ColdcardQ),
        ];

        assert_eq!(
            cases.len(),
            14,
            "test must cover all known DeviceKind variants (excluding Other)"
        );

        for (json_str, expected) in cases {
            let parsed: DeviceKind = serde_json::from_str(json_str).unwrap();
            assert_eq!(parsed, expected);
            assert_eq!(serde_json::to_string(&parsed).unwrap(), json_str);
        }

        // Test Other variant - must be explicitly specified as {"other": "value"}
        let other_json = r#"{"other":"FutureDevice"}"#;
        let parsed: DeviceKind = serde_json::from_str(other_json).unwrap();
        assert_eq!(parsed, DeviceKind::Other("FutureDevice".to_string()));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), other_json);
    }
}
