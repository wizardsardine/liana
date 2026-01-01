//! WSS Protocol Types
//!
//! This module contains all the JSON structures used for communication
//! between Liana Connect clients and servers, and conversions to/from domain types.

use std::collections::BTreeMap;
use std::fmt::Display;
use std::str::FromStr;

use miniscript::DescriptorPublicKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::ws_business::models::{
    Key, KeyType, Org, PolicyTemplate, SpendingPath, Timelock, User, UserRole, Wallet, WalletStatus,
};

// ============================================================================
// JSON Payload Structures
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectPayload {
    pub version: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedPayload {
    pub version: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchOrgPayload {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveWalletFromOrgPayload {
    pub wallet_id: String,
    pub org_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateWalletPayload {
    pub name: String,
    pub org_id: String,
    pub owner_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditWalletPayload {
    pub wallet: WalletJson,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchWalletPayload {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditXpubPayload {
    pub wallet_id: String,
    pub key_id: u8,
    /// Xpub data with source info (None to clear)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xpub: Option<XpubJson>,
}

/// Xpub data bundled with source information for audit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XpubJson {
    /// The extended public key string
    pub value: String,
    /// Source type: "device", "file", or "pasted"
    pub source: String,
    /// Device kind (e.g., "Ledger", "Trezor") - only for source="device"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_kind: Option<String>,
    /// Device fingerprint - only for source="device"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_fingerprint: Option<String>,
    /// Device firmware version - only for source="device"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_version: Option<String>,
    /// File name - only for source="file"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchUserPayload {
    pub id: String,
}

// ============================================================================
// JSON Domain Representations
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgJson {
    pub name: String,
    pub id: String,
    pub wallets: Vec<String>,
    pub users: Vec<String>,
    pub owners: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletJson {
    pub id: String,
    pub alias: String,
    pub org: String,
    pub owner: String,
    /// Owner's email address for role derivation (avoids needing to fetch user)
    pub owner_email: String,
    #[serde(rename = "status")]
    pub status_str: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<PolicyTemplateJson>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserJson {
    pub name: String,
    pub uuid: String,
    pub email: String,
    pub orgs: Vec<String>,
    #[serde(rename = "role")]
    pub role_str: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyTemplateJson {
    pub keys: BTreeMap<String, KeyJson>,
    pub primary_path: SpendingPathJson,
    pub secondary_paths: Vec<SecondaryPathJson>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyJson {
    pub id: u8,
    pub alias: String,
    pub description: String,
    pub email: String,
    #[serde(rename = "key_type")]
    pub key_type_str: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xpub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xpub_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xpub_device_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xpub_device_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xpub_device_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xpub_file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_edited: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_editor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendingPathJson {
    pub is_primary: bool,
    pub threshold_n: u8,
    pub key_ids: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecondaryPathJson {
    pub path: SpendingPathJson,
    pub timelock: TimelockJson,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelockJson {
    pub blocks: u64,
}

// ============================================================================
// Protocol Internals
// ============================================================================

/// Protocol-level request structure for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolRequest {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub token: String,
    pub request_id: String,
    pub payload: Value,
}

/// Protocol-level response structure for deserialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WssError>,
}

// ============================================================================
// Error Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WssError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl Display for WssError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string_pretty(&self).expect("serialization must not fail")
        )
    }
}

#[derive(Debug, Clone)]
pub enum WssConversionError {
    DeserializationFailed(String),
    InvalidMessageType,
}

impl std::fmt::Display for WssConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WssConversionError::DeserializationFailed(msg) => {
                write!(f, "Failed to deserialize WSS response: {}", msg)
            }
            WssConversionError::InvalidMessageType => {
                write!(f, "Invalid WebSocket message type (expected Text)")
            }
        }
    }
}

impl std::error::Error for WssConversionError {}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a WebSocket message from protocol request data
pub fn create_ws_request(
    msg_type: &str,
    token: &str,
    request_id: &str,
    payload: Value,
) -> WsMessage {
    let protocol_request = ProtocolRequest {
        msg_type: msg_type.to_string(),
        token: token.to_string(),
        request_id: request_id.to_string(),
        payload,
    };

    let json = serde_json::to_string(&protocol_request).expect("serialization must not fail");
    WsMessage::Text(json)
}

/// Parse a WebSocket message into protocol response data
pub fn parse_ws_response(msg: WsMessage) -> Result<ProtocolResponse, WssConversionError> {
    let text = match msg {
        WsMessage::Text(text) => text,
        _ => return Err(WssConversionError::InvalidMessageType),
    };

    serde_json::from_str(&text)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))
}

// ============================================================================
// Conversions: JSON -> Domain
// ============================================================================

impl TryFrom<OrgJson> for Org {
    type Error = String;

    fn try_from(json: OrgJson) -> Result<Self, Self::Error> {
        let id = Uuid::parse_str(&json.id).map_err(|e| format!("Invalid org UUID: {}", e))?;
        let wallets = json
            .wallets
            .into_iter()
            .map(|w| Uuid::parse_str(&w))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|e| format!("Invalid wallet UUID: {}", e))?;
        let users = json
            .users
            .into_iter()
            .map(|u| Uuid::parse_str(&u))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|e| format!("Invalid user UUID: {}", e))?;
        let owners = json
            .owners
            .into_iter()
            .map(|o| Uuid::parse_str(&o))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Invalid owner UUID: {}", e))?;

        Ok(Org {
            name: json.name,
            id,
            wallets,
            users,
            owners,
            last_edited: None,
            last_editor: None,
        })
    }
}

impl TryFrom<UserJson> for User {
    type Error = String;

    fn try_from(json: UserJson) -> Result<Self, Self::Error> {
        let uuid = Uuid::parse_str(&json.uuid).map_err(|e| format!("Invalid user UUID: {}", e))?;
        let orgs = json
            .orgs
            .into_iter()
            .map(|o| Uuid::parse_str(&o))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Invalid org UUID: {}", e))?;
        let role = UserRole::from_str(&json.role_str)
            .ok_or_else(|| format!("Invalid user role: {}", json.role_str))?;

        Ok(User {
            name: json.name,
            uuid,
            email: json.email,
            orgs,
            role,
            last_edited: None,
            last_editor: None,
        })
    }
}

impl TryFrom<WalletJson> for Wallet {
    type Error = String;

    fn try_from(json: WalletJson) -> Result<Self, Self::Error> {
        let id = Uuid::parse_str(&json.id).map_err(|e| format!("Invalid wallet UUID: {}", e))?;
        let org = Uuid::parse_str(&json.org).map_err(|e| format!("Invalid org UUID: {}", e))?;
        let owner_id =
            Uuid::parse_str(&json.owner).map_err(|e| format!("Invalid owner UUID: {}", e))?;
        let status = WalletStatus::from_str(&json.status_str)
            .ok_or_else(|| format!("Invalid wallet status: {}", json.status_str))?;

        // Create owner User with email from WalletJson for role derivation
        let owner = User {
            name: String::new(),
            uuid: owner_id,
            email: json.owner_email,
            orgs: Vec::new(),
            role: UserRole::Owner,
            last_edited: None,
            last_editor: None,
        };

        let template = json.template.map(|t| t.try_into()).transpose()?;

        Ok(Wallet {
            alias: json.alias,
            org,
            owner,
            id,
            status,
            template,
            last_edited: None,
            last_editor: None,
        })
    }
}

impl TryFrom<PolicyTemplateJson> for PolicyTemplate {
    type Error = String;

    fn try_from(json: PolicyTemplateJson) -> Result<Self, Self::Error> {
        let mut keys = BTreeMap::new();
        for (k, v) in json.keys {
            let key_id = k
                .parse::<u8>()
                .map_err(|_| format!("Invalid key ID: {}", k))?;
            let key = v.try_into()?;
            keys.insert(key_id, key);
        }

        let primary_path = json.primary_path.try_into()?;
        let secondary_paths = json
            .secondary_paths
            .into_iter()
            .map(|sp| Ok((sp.path.try_into()?, sp.timelock.try_into()?)))
            .collect::<Result<Vec<_>, String>>()?;

        Ok(PolicyTemplate {
            keys,
            primary_path,
            secondary_paths,
        })
    }
}

impl TryFrom<KeyJson> for Key {
    type Error = String;

    fn try_from(json: KeyJson) -> Result<Self, Self::Error> {
        let key_type = KeyType::from_str(&json.key_type_str)
            .ok_or_else(|| format!("Invalid key type: {}", json.key_type_str))?;
        let xpub = json
            .xpub
            .map(|x| DescriptorPublicKey::from_str(&x).map_err(|e| format!("Invalid xpub: {}", e)))
            .transpose()?;
        let last_editor = json
            .last_editor
            .map(|s| Uuid::parse_str(&s).map_err(|e| format!("Invalid last_editor UUID: {}", e)))
            .transpose()?;

        Ok(Key {
            id: json.id,
            alias: json.alias,
            description: json.description,
            email: json.email,
            key_type,
            xpub,
            xpub_source: json.xpub_source,
            xpub_device_kind: json.xpub_device_kind,
            xpub_device_fingerprint: json.xpub_device_fingerprint,
            xpub_device_version: json.xpub_device_version,
            xpub_file_name: json.xpub_file_name,
            last_edited: json.last_edited,
            last_editor,
        })
    }
}

impl TryFrom<SpendingPathJson> for SpendingPath {
    type Error = String;

    fn try_from(json: SpendingPathJson) -> Result<Self, Self::Error> {
        Ok(SpendingPath {
            is_primary: json.is_primary,
            threshold_n: json.threshold_n,
            key_ids: json.key_ids,
            last_edited: None,
            last_editor: None,
        })
    }
}

impl TryFrom<TimelockJson> for Timelock {
    type Error = String;

    fn try_from(json: TimelockJson) -> Result<Self, Self::Error> {
        Ok(Timelock {
            blocks: json.blocks,
        })
    }
}

// ============================================================================
// Conversions: Domain -> JSON
// ============================================================================

impl From<&Org> for OrgJson {
    fn from(org: &Org) -> Self {
        OrgJson {
            name: org.name.clone(),
            id: org.id.to_string(),
            wallets: org.wallets.iter().map(|w| w.to_string()).collect(),
            users: org.users.iter().map(|u| u.to_string()).collect(),
            owners: org.owners.iter().map(|o| o.to_string()).collect(),
        }
    }
}

impl From<&User> for UserJson {
    fn from(user: &User) -> Self {
        UserJson {
            name: user.name.clone(),
            uuid: user.uuid.to_string(),
            email: user.email.clone(),
            orgs: user.orgs.iter().map(|o| o.to_string()).collect(),
            role_str: user.role.as_str().to_string(),
        }
    }
}

impl From<&Wallet> for WalletJson {
    fn from(wallet: &Wallet) -> Self {
        WalletJson {
            id: wallet.id.to_string(),
            alias: wallet.alias.clone(),
            org: wallet.org.to_string(),
            owner: wallet.owner.uuid.to_string(),
            owner_email: wallet.owner.email.clone(),
            status_str: wallet.status.as_str().to_string(),
            template: wallet.template.as_ref().map(|t| t.into()),
        }
    }
}

impl From<&PolicyTemplate> for PolicyTemplateJson {
    fn from(template: &PolicyTemplate) -> Self {
        let mut keys_json = BTreeMap::new();
        for (k, v) in &template.keys {
            keys_json.insert(k.to_string(), v.into());
        }

        PolicyTemplateJson {
            keys: keys_json,
            primary_path: (&template.primary_path).into(),
            secondary_paths: template
                .secondary_paths
                .iter()
                .map(|(path, timelock)| SecondaryPathJson {
                    path: path.into(),
                    timelock: timelock.into(),
                })
                .collect(),
        }
    }
}

impl From<&Key> for KeyJson {
    fn from(key: &Key) -> Self {
        KeyJson {
            id: key.id,
            alias: key.alias.clone(),
            description: key.description.clone(),
            email: key.email.clone(),
            key_type_str: key.key_type.as_str().to_string(),
            xpub: key.xpub.as_ref().map(|x| x.to_string()),
            xpub_source: key.xpub_source.clone(),
            xpub_device_kind: key.xpub_device_kind.clone(),
            xpub_device_fingerprint: key.xpub_device_fingerprint.clone(),
            xpub_device_version: key.xpub_device_version.clone(),
            xpub_file_name: key.xpub_file_name.clone(),
            last_edited: key.last_edited,
            last_editor: key.last_editor.map(|u| u.to_string()),
        }
    }
}

impl From<&SpendingPath> for SpendingPathJson {
    fn from(path: &SpendingPath) -> Self {
        SpendingPathJson {
            is_primary: path.is_primary,
            threshold_n: path.threshold_n,
            key_ids: path.key_ids.clone(),
        }
    }
}

impl From<&Timelock> for TimelockJson {
    fn from(timelock: &Timelock) -> Self {
        TimelockJson {
            blocks: timelock.blocks,
        }
    }
}

// ============================================================================
// Application-level Request and Response enums
// ============================================================================

/// Application-level request enum for WSS protocol operations
#[derive(Debug, Clone)]
pub enum Request {
    Connect {
        version: u8,
    },
    Ping,
    Close,
    GetServerTime,
    FetchOrg {
        id: Uuid,
    },
    RemoveWalletFromOrg {
        wallet_id: Uuid,
        org_id: Uuid,
    },
    CreateWallet {
        name: String,
        org_id: Uuid,
        owner_id: Uuid,
    },
    EditWallet {
        wallet: Wallet,
    },
    FetchWallet {
        id: Uuid,
    },
    EditXpub {
        wallet_id: Uuid,
        key_id: u8,
        /// Xpub with source info (None to clear)
        xpub: Option<XpubJson>,
    },
    FetchUser {
        id: Uuid,
    },
}

/// Application-level response enum for WSS protocol operations
#[derive(Debug, Clone)]
pub enum Response {
    Connected { version: u8 },
    Pong,
    ServerTime { timestamp: u64 },
    Org { org: OrgJson },
    Wallet { wallet: WalletJson },
    User { user: UserJson },
    Error { error: WssError },
}

impl Request {
    /// Convert application-level request to WebSocket message with protocol details
    pub fn to_ws_message(&self, token: &str, request_id: &str) -> WsMessage {
        let (msg_type, payload) = match self {
            Request::Connect { version } => (
                "connect",
                serde_json::to_value(ConnectPayload { version: *version })
                    .expect("serialization must not fail"),
            ),
            Request::Ping => ("ping", serde_json::json!({})),
            Request::Close => ("close", serde_json::json!({})),
            Request::GetServerTime => ("get_server_time", serde_json::json!({})),
            Request::FetchOrg { id } => (
                "fetch_org",
                serde_json::to_value(FetchOrgPayload { id: id.to_string() })
                    .expect("serialization must not fail"),
            ),
            Request::RemoveWalletFromOrg { wallet_id, org_id } => (
                "remove_wallet_from_org",
                serde_json::to_value(RemoveWalletFromOrgPayload {
                    wallet_id: wallet_id.to_string(),
                    org_id: org_id.to_string(),
                })
                .expect("serialization must not fail"),
            ),
            Request::CreateWallet {
                name,
                org_id,
                owner_id,
            } => (
                "create_wallet",
                serde_json::to_value(CreateWalletPayload {
                    name: name.clone(),
                    org_id: org_id.to_string(),
                    owner_id: owner_id.to_string(),
                })
                .expect("serialization must not fail"),
            ),
            Request::EditWallet { wallet } => {
                let wallet_json: WalletJson = wallet.into();
                (
                    "edit_wallet",
                    serde_json::to_value(EditWalletPayload {
                        wallet: wallet_json,
                    })
                    .expect("serialization must not fail"),
                )
            }
            Request::FetchWallet { id } => (
                "fetch_wallet",
                serde_json::to_value(FetchWalletPayload { id: id.to_string() })
                    .expect("serialization must not fail"),
            ),
            Request::EditXpub {
                wallet_id,
                key_id,
                xpub,
            } => (
                "edit_xpub",
                serde_json::to_value(EditXpubPayload {
                    wallet_id: wallet_id.to_string(),
                    key_id: *key_id,
                    xpub: xpub.clone(),
                })
                .expect("serialization must not fail"),
            ),
            Request::FetchUser { id } => (
                "fetch_user",
                serde_json::to_value(FetchUserPayload { id: id.to_string() })
                    .expect("serialization must not fail"),
            ),
        };

        create_ws_request(msg_type, token, request_id, payload)
    }
}

impl Response {
    /// Convert WebSocket message to application-level response, extracting protocol details
    pub fn from_ws_message(
        msg: WsMessage,
    ) -> Result<(Self, Option<String> /* request_id */), WssConversionError> {
        let protocol_response = parse_ws_response(msg)?;
        let request_id = protocol_response.request_id;

        // Handle error responses
        if let Some(error) = protocol_response.error {
            return Ok((Response::Error { error }, request_id));
        }

        let response = match protocol_response.msg_type.as_str() {
            "connected" => {
                let payload: ConnectedPayload =
                    serde_json::from_value(protocol_response.payload.ok_or_else(|| {
                        WssConversionError::DeserializationFailed("Missing payload".to_string())
                    })?)
                    .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
                Response::Connected {
                    version: payload.version,
                }
            }
            "pong" => Response::Pong,
            "server_time" => {
                let payload = protocol_response.payload.ok_or_else(|| {
                    WssConversionError::DeserializationFailed("Missing payload".to_string())
                })?;
                let timestamp = payload["timestamp"].as_u64().ok_or_else(|| {
                    WssConversionError::DeserializationFailed("Missing timestamp".to_string())
                })?;
                Response::ServerTime { timestamp }
            }
            "org" => {
                let payload: OrgJson =
                    serde_json::from_value(protocol_response.payload.ok_or_else(|| {
                        WssConversionError::DeserializationFailed("Missing payload".to_string())
                    })?)
                    .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
                Response::Org { org: payload }
            }
            "wallet" => {
                let payload: WalletJson =
                    serde_json::from_value(protocol_response.payload.ok_or_else(|| {
                        WssConversionError::DeserializationFailed("Missing payload".to_string())
                    })?)
                    .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
                Response::Wallet { wallet: payload }
            }
            "user" => {
                let payload: UserJson =
                    serde_json::from_value(protocol_response.payload.ok_or_else(|| {
                        WssConversionError::DeserializationFailed("Missing payload".to_string())
                    })?)
                    .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
                Response::User { user: payload }
            }
            _ => {
                return Err(WssConversionError::DeserializationFailed(format!(
                    "Unknown message type: {}",
                    protocol_response.msg_type
                )))
            }
        };

        Ok((response, request_id))
    }
}
