//! WSS Protocol Types
//!
//! This module contains all the JSON structures used for communication
//! between Liana Connect clients and servers.

use crate::ws_business::models::{Org, User, Wallet, Xpub};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::Display;
use tungstenite::Message as WsMessage;
use uuid::Uuid;

use super::RegistrationInfos;

/// Protocol-level request structure for serialization
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolRequest {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub token: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

/// Protocol-level response structure for deserialization
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Create a WebSocket message from protocol request data
fn create_ws_request(
    msg_type: &str,
    token: &str,
    request_id: &str,
    payload: Option<Value>,
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
fn parse_ws_response(msg: WsMessage) -> Result<ProtocolResponse, WssConversionError> {
    let text = match msg {
        WsMessage::Text(text) => text,
        _ => return Err(WssConversionError::InvalidMessageType),
    };
    serde_json::from_str(&text)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))
}

/// Parse a WebSocket message into protocol request data
fn parse_ws_request(msg: WsMessage) -> Result<ProtocolRequest, WssConversionError> {
    let text = match msg {
        WsMessage::Text(text) => text,
        _ => return Err(WssConversionError::InvalidMessageType),
    };
    serde_json::from_str(&text)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))
}

fn connect_payload(version: u8) -> Value {
    serde_json::json!({ "version": version })
}

fn fetch_by_id_payload(id: &Uuid) -> Value {
    serde_json::json!({ "id": id.to_string() })
}

fn edit_wallet_payload(wallet: &Wallet) -> Value {
    serde_json::json!({ "wallet": wallet })
}

fn edit_xpub_payload(wallet_id: &Uuid, key_id: u8, xpub: &Option<Xpub>) -> Value {
    let mut payload = serde_json::json!({
        "wallet_id": wallet_id.to_string(),
        "key_id": key_id,
    });
    if let Some(x) = xpub {
        payload["xpub"] = serde_json::to_value(x).expect("serialization must not fail");
    }
    payload
}

fn device_registered_payload(wallet_id: &Uuid, infos: &RegistrationInfos) -> Value {
    serde_json::json!({
        "wallet_id": wallet_id.to_string(),
        "infos": infos,
    })
}

fn parse_connected(payload: Option<Value>) -> Result<Response, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let version = payload["version"]
        .as_u64()
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing version".to_string()))?
        as u8;
    let user = payload["user"]
        .as_str()
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing user".to_string()))?;
    let user = Uuid::parse_str(user)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
    Ok(Response::Connected { version, user })
}

fn parse_org(payload: Option<Value>) -> Result<Response, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let org: Org = serde_json::from_value(payload)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
    Ok(Response::Org { org })
}

fn parse_wallet(payload: Option<Value>) -> Result<Response, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let wallet: Wallet = serde_json::from_value(payload)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
    Ok(Response::Wallet { wallet })
}

fn parse_user(payload: Option<Value>) -> Result<Response, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let user: User = serde_json::from_value(payload)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
    Ok(Response::User { user })
}

fn parse_delete_user_org(payload: Option<Value>) -> Result<Response, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let user = payload["user"]
        .as_str()
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing user".to_string()))?;
    let user = Uuid::parse_str(user)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
    let org = payload["org"]
        .as_str()
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing org".to_string()))?;
    let org = Uuid::parse_str(org)
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
    Ok(Response::DeleteUserOrg { user, org })
}

fn parse_connect_request(payload: Option<Value>) -> Result<Request, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let version = payload["version"]
        .as_u64()
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing version".to_string()))?
        as u8;
    Ok(Request::Connect { version })
}

fn parse_fetch_request(payload: Option<Value>) -> Result<Uuid, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let id_str = payload["id"]
        .as_str()
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing id".to_string()))?;
    Uuid::parse_str(id_str)
        .map_err(|e| WssConversionError::DeserializationFailed(format!("Invalid UUID: {}", e)))
}

fn parse_edit_wallet_request(payload: Option<Value>) -> Result<Request, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let wallet_value = payload
        .get("wallet")
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing wallet".to_string()))?;
    let wallet: Wallet = serde_json::from_value(wallet_value.clone())
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
    Ok(Request::EditWallet { wallet })
}

fn parse_edit_xpub_request(payload: Option<Value>) -> Result<Request, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let wallet_id_str = payload["wallet_id"].as_str().ok_or_else(|| {
        WssConversionError::DeserializationFailed("Missing wallet_id".to_string())
    })?;
    let wallet_id = Uuid::parse_str(wallet_id_str)
        .map_err(|e| WssConversionError::DeserializationFailed(format!("Invalid UUID: {}", e)))?;
    let key_id = payload["key_id"]
        .as_u64()
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing key_id".to_string()))?
        as u8;
    let xpub = if let Some(xpub_value) = payload.get("xpub") {
        Some(
            serde_json::from_value(xpub_value.clone())
                .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?,
        )
    } else {
        None
    };
    Ok(Request::EditXpub {
        wallet_id,
        key_id,
        xpub,
    })
}

fn parse_device_registered_request(payload: Option<Value>) -> Result<Request, WssConversionError> {
    let payload = payload
        .ok_or_else(|| WssConversionError::DeserializationFailed("Missing payload".to_string()))?;
    let wallet_id_str = payload["wallet_id"].as_str().ok_or_else(|| {
        WssConversionError::DeserializationFailed("Missing wallet_id".to_string())
    })?;
    let wallet_id = Uuid::parse_str(wallet_id_str)
        .map_err(|e| WssConversionError::DeserializationFailed(format!("Invalid UUID: {}", e)))?;
    let infos: RegistrationInfos = serde_json::from_value(payload["infos"].clone())
        .map_err(|e| WssConversionError::DeserializationFailed(e.to_string()))?;
    Ok(Request::DeviceRegistered { wallet_id, infos })
}

/// Application-level request enum for WSS protocol operations
#[derive(Debug, Clone)]
pub enum Request {
    Connect {
        version: u8,
    },
    Ping,
    Close,
    FetchOrg {
        id: Uuid,
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
        xpub: Option<Xpub>,
    },
    FetchUser {
        id: Uuid,
    },
    DeviceRegistered {
        wallet_id: Uuid,
        infos: RegistrationInfos,
    },
}

/// Application-level response enum for WSS protocol operations
#[derive(Debug, Clone)]
pub enum Response {
    Connected { version: u8, user: Uuid },
    Pong,
    Org { org: Org },
    Wallet { wallet: Wallet },
    User { user: User },
    Error { error: WssError },
    DeleteUserOrg { user: Uuid, org: Uuid },
}

impl Request {
    pub const METHOD_CONNECT: &'static str = "connect";
    pub const METHOD_PING: &'static str = "ping";
    pub const METHOD_CLOSE: &'static str = "close";
    pub const METHOD_FETCH_ORG: &'static str = "fetch_org";
    pub const METHOD_FETCH_WALLET: &'static str = "fetch_wallet";
    pub const METHOD_FETCH_USER: &'static str = "fetch_user";
    pub const METHOD_EDIT_WALLET: &'static str = "edit_wallet";
    pub const METHOD_EDIT_XPUB: &'static str = "edit_xpub";
    pub const METHOD_DEVICE_REGISTERED: &'static str = "device_registered";

    /// Returns the protocol message type for this request.
    pub fn method(&self) -> &'static str {
        match self {
            Request::Connect { .. } => Self::METHOD_CONNECT,
            Request::Ping => Self::METHOD_PING,
            Request::Close => Self::METHOD_CLOSE,
            Request::FetchOrg { .. } => Self::METHOD_FETCH_ORG,
            Request::EditWallet { .. } => Self::METHOD_EDIT_WALLET,
            Request::FetchWallet { .. } => Self::METHOD_FETCH_WALLET,
            Request::EditXpub { .. } => Self::METHOD_EDIT_XPUB,
            Request::FetchUser { .. } => Self::METHOD_FETCH_USER,
            Request::DeviceRegistered { .. } => Self::METHOD_DEVICE_REGISTERED,
        }
    }

    /// Returns the payload as a JSON value, if any.
    pub fn payload(&self) -> Option<Value> {
        match self {
            Request::Connect { version } => Some(connect_payload(*version)),
            Request::Ping => None,
            Request::Close => None,
            Request::FetchOrg { id } => Some(fetch_by_id_payload(id)),
            Request::FetchWallet { id } => Some(fetch_by_id_payload(id)),
            Request::FetchUser { id } => Some(fetch_by_id_payload(id)),
            Request::EditWallet { wallet } => Some(edit_wallet_payload(wallet)),
            Request::EditXpub {
                wallet_id,
                key_id,
                xpub,
            } => Some(edit_xpub_payload(wallet_id, *key_id, xpub)),
            Request::DeviceRegistered { wallet_id, infos } => {
                Some(device_registered_payload(wallet_id, infos))
            }
        }
    }

    /// Convert WebSocket message to application-level request, extracting protocol details.
    pub fn from_ws_message(
        msg: WsMessage,
    ) -> Result<(Self, String /* token */, String /* request_id */), WssConversionError> {
        let protocol_request = parse_ws_request(msg)?;
        let token = protocol_request.token;
        let request_id = protocol_request.request_id;

        let request = match protocol_request.msg_type.as_str() {
            Self::METHOD_CONNECT => parse_connect_request(protocol_request.payload)?,
            Self::METHOD_PING => Request::Ping,
            Self::METHOD_CLOSE => Request::Close,
            Self::METHOD_FETCH_ORG => {
                let id = parse_fetch_request(protocol_request.payload)?;
                Request::FetchOrg { id }
            }
            Self::METHOD_FETCH_WALLET => {
                let id = parse_fetch_request(protocol_request.payload)?;
                Request::FetchWallet { id }
            }
            Self::METHOD_FETCH_USER => {
                let id = parse_fetch_request(protocol_request.payload)?;
                Request::FetchUser { id }
            }
            Self::METHOD_EDIT_WALLET => parse_edit_wallet_request(protocol_request.payload)?,
            Self::METHOD_EDIT_XPUB => parse_edit_xpub_request(protocol_request.payload)?,
            Self::METHOD_DEVICE_REGISTERED => {
                parse_device_registered_request(protocol_request.payload)?
            }
            _ => {
                return Err(WssConversionError::DeserializationFailed(format!(
                    "Unknown message type: {}",
                    protocol_request.msg_type
                )))
            }
        };

        Ok((request, token, request_id))
    }

    /// Convert application-level request to WebSocket message with protocol details
    pub fn to_ws_message_with_id(&self, token: &str, request_id: &str) -> WsMessage {
        create_ws_request(self.method(), token, request_id, self.payload())
    }

    /// Convert application-level request to WebSocket message with protocol details
    pub fn to_ws_message(&self, token: &str) -> (WsMessage, Uuid) {
        let id = Uuid::new_v4();
        let request_id = id.to_string();
        (
            create_ws_request(self.method(), token, &request_id, self.payload()),
            id,
        )
    }
}

impl Response {
    pub const METHOD_CONNECTED: &'static str = "connected";
    pub const METHOD_PONG: &'static str = "pong";
    pub const METHOD_ORG: &'static str = "org";
    pub const METHOD_WALLET: &'static str = "wallet";
    pub const METHOD_USER: &'static str = "user";
    pub const METHOD_ERROR: &'static str = "error";
    pub const METHOD_DELETE_USER_ORG: &'static str = "delete_user_org";

    /// Returns the protocol message type for this response.
    pub fn method(&self) -> &'static str {
        match self {
            Response::Connected { .. } => Self::METHOD_CONNECTED,
            Response::Pong => Self::METHOD_PONG,
            Response::Org { .. } => Self::METHOD_ORG,
            Response::Wallet { .. } => Self::METHOD_WALLET,
            Response::User { .. } => Self::METHOD_USER,
            Response::Error { .. } => Self::METHOD_ERROR,
            Response::DeleteUserOrg { .. } => Self::METHOD_DELETE_USER_ORG,
        }
    }

    /// Returns the payload as a JSON value, if any.
    pub fn payload(&self) -> Option<Value> {
        match self {
            Response::Connected { version, user } => {
                Some(serde_json::json!({ "version": version, "user": user }))
            }
            Response::Pong => None,
            Response::Org { org } => {
                Some(serde_json::to_value(org).expect("serialization must not fail"))
            }
            Response::Wallet { wallet } => {
                Some(serde_json::to_value(wallet).expect("serialization must not fail"))
            }
            Response::User { user } => {
                Some(serde_json::to_value(user).expect("serialization must not fail"))
            }
            Response::Error { .. } => None,
            Response::DeleteUserOrg { user, org } => {
                Some(serde_json::json!({ "user": user, "org": org }))
            }
        }
    }

    /// Convert application-level response to WebSocket message with protocol details.
    pub fn to_ws_message(&self, request_id: Option<&str>) -> WsMessage {
        let protocol_response = ProtocolResponse {
            msg_type: self.method().to_string(),
            request_id: request_id.map(String::from),
            payload: self.payload(),
            error: match self {
                Response::Error { error } => Some(error.clone()),
                _ => None,
            },
        };
        let json = serde_json::to_string(&protocol_response).expect("serialization must not fail");
        WsMessage::Text(json)
    }

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
            Self::METHOD_CONNECTED => parse_connected(protocol_response.payload)?,
            Self::METHOD_PONG => Response::Pong,
            Self::METHOD_ORG => parse_org(protocol_response.payload)?,
            Self::METHOD_WALLET => parse_wallet(protocol_response.payload)?,
            Self::METHOD_USER => parse_user(protocol_response.payload)?,
            Self::METHOD_DELETE_USER_ORG => parse_delete_user_org(protocol_response.payload)?,
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

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use crate::ws_business::models::{
        DeviceKind, Key, KeyIdentity, KeyType, Org, PolicyTemplate, SecondaryPath, SpendingPath,
        Timelock, User, UserRole, WalletStatus, Xpub, XpubSource,
    };
    use std::collections::{BTreeMap, BTreeSet};

    // Helper to extract JSON from WsMessage
    fn ws_msg_to_json(msg: WsMessage) -> serde_json::Value {
        match msg {
            WsMessage::Text(text) => serde_json::from_str(&text).unwrap(),
            _ => panic!("Expected Text message"),
        }
    }

    // Helper to roundtrip a ProtocolRequest through JSON
    fn roundtrip_request(json: &str) {
        let parsed: ProtocolRequest = serde_json::from_str(json).unwrap();
        let reserialized = serde_json::to_value(&parsed).unwrap();
        let expected: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            reserialized, expected,
            "Roundtrip failed: serialization changed the JSON"
        );
    }

    // Helper to roundtrip a ProtocolResponse through JSON
    fn roundtrip_response(json: &str) {
        let parsed: ProtocolResponse = serde_json::from_str(json).unwrap();
        let reserialized = serde_json::to_value(&parsed).unwrap();
        let expected: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            reserialized, expected,
            "Roundtrip failed: serialization changed the JSON"
        );
    }

    // Helper to roundtrip a WssError through JSON
    fn roundtrip_wss_error(json: &str) {
        let parsed: WssError = serde_json::from_str(json).unwrap();
        let reserialized = serde_json::to_value(&parsed).unwrap();
        let expected: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            reserialized, expected,
            "Roundtrip failed: serialization changed the JSON"
        );
    }

    // Test UUIDs
    fn test_uuid(n: u8) -> Uuid {
        Uuid::parse_str(&format!("12345678-1234-1234-1234-12345678900{}", n)).unwrap()
    }

    // ==================== REQUEST WIRE FORMAT TESTS ====================
    // These tests document the exact JSON structure of each request type.
    // If any of these fail, it indicates a breaking change in the wire format.

    #[test]
    fn test_request_connect_wire_format() {
        // Documentation: Connect request wire format
        let expected_json = r#"{
            "type": "connect",
            "token": "test-token",
            "request_id": "req-001",
            "payload": {"version": 1}
        }"#;

        let request = Request::Connect { version: 1 };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-001");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_ping_wire_format() {
        // Documentation: Ping request wire format (no payload)
        let expected_json = r#"{
            "type": "ping",
            "token": "test-token",
            "request_id": "req-002"
        }"#;

        let request = Request::Ping;
        let ws_msg = request.to_ws_message_with_id("test-token", "req-002");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_close_wire_format() {
        // Documentation: Close request wire format (no payload)
        let expected_json = r#"{
            "type": "close",
            "token": "test-token",
            "request_id": "req-003"
        }"#;

        let request = Request::Close;
        let ws_msg = request.to_ws_message_with_id("test-token", "req-003");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_fetch_org_wire_format() {
        // Documentation: FetchOrg request wire format
        let expected_json = r#"{
            "type": "fetch_org",
            "token": "test-token",
            "request_id": "req-005",
            "payload": {"id": "12345678-1234-1234-1234-123456789001"}
        }"#;

        let request = Request::FetchOrg { id: test_uuid(1) };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-005");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_fetch_wallet_wire_format() {
        // Documentation: FetchWallet request wire format
        let expected_json = r#"{
            "type": "fetch_wallet",
            "token": "test-token",
            "request_id": "req-006",
            "payload": {"id": "12345678-1234-1234-1234-123456789002"}
        }"#;

        let request = Request::FetchWallet { id: test_uuid(2) };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-006");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_fetch_user_wire_format() {
        // Documentation: FetchUser request wire format
        let expected_json = r#"{
            "type": "fetch_user",
            "token": "test-token",
            "request_id": "req-007",
            "payload": {"id": "12345678-1234-1234-1234-123456789003"}
        }"#;

        let request = Request::FetchUser { id: test_uuid(3) };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-007");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_edit_wallet_wire_format() {
        // Documentation: EditWallet request wire format
        let expected_json = r#"{
            "type": "edit_wallet",
            "token": "test-token",
            "request_id": "req-008",
            "payload": {
                "wallet": {
                    "alias": "Test Wallet",
                    "org": "12345678-1234-1234-1234-123456789001",
                    "owner": "12345678-1234-1234-1234-123456789002",
                    "id": "12345678-1234-1234-1234-123456789003",
                    "status": "Drafted"
                }
            }
        }"#;

        let wallet = Wallet {
            alias: "Test Wallet".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Drafted,
            template: None,
            last_edited: None,
            last_editor: None,
            descriptor: None,
            devices: None,
        };
        let request = Request::EditWallet { wallet };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-008");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_edit_wallet_with_metadata_wire_format() {
        // Documentation: EditWallet with last_edited and last_editor
        let expected_json = r#"{
            "type": "edit_wallet",
            "token": "test-token",
            "request_id": "req-008",
            "payload": {
                "wallet": {
                    "alias": "Test Wallet",
                    "org": "12345678-1234-1234-1234-123456789001",
                    "owner": "12345678-1234-1234-1234-123456789002",
                    "id": "12345678-1234-1234-1234-123456789003",
                    "status": "Locked",
                    "last_edited": 1700000000,
                    "last_editor": "12345678-1234-1234-1234-123456789004"
                }
            }
        }"#;

        let wallet = Wallet {
            alias: "Test Wallet".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Locked,
            template: None,
            last_edited: Some(1700000000),
            last_editor: Some(test_uuid(4)),
            descriptor: None,
            devices: None,
        };
        let request = Request::EditWallet { wallet };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-008");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_edit_wallet_with_template_wire_format() {
        // Documentation: EditWallet with policy template
        let expected_json = r#"{
            "type": "edit_wallet",
            "token": "test-token",
            "request_id": "req-008",
            "payload": {
                "wallet": {
                    "alias": "Multisig Vault",
                    "org": "12345678-1234-1234-1234-123456789001",
                    "owner": "12345678-1234-1234-1234-123456789002",
                    "id": "12345678-1234-1234-1234-123456789003",
                    "status": "Validated",
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
                                "description": "Cosigner key",
                                "email": "bob@example.com",
                                "key_type": "Cosigner"
                            }
                        },
                        "primary_path": {
                            "is_primary": true,
                            "threshold_n": 2,
                            "key_ids": [0, 1]
                        },
                        "secondary_paths": []
                    }
                }
            }
        }"#;

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
                description: "Cosigner key".to_string(),
                identity: KeyIdentity::Email("bob@example.com".to_string()),
                key_type: KeyType::Cosigner,
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
            primary_path: SpendingPath::new(true, 2, vec![0, 1]),
            secondary_paths: vec![],
        };

        let wallet = Wallet {
            alias: "Multisig Vault".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Validated,
            template: Some(template),
            last_edited: None,
            last_editor: None,
            descriptor: None,
            devices: None,
        };
        let request = Request::EditWallet { wallet };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-008");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_edit_wallet_with_secondary_path_wire_format() {
        // Documentation: EditWallet with secondary recovery path
        let expected_json = r#"{
            "type": "edit_wallet",
            "token": "test-token",
            "request_id": "req-008",
            "payload": {
                "wallet": {
                    "alias": "Recovery Vault",
                    "org": "12345678-1234-1234-1234-123456789001",
                    "owner": "12345678-1234-1234-1234-123456789002",
                    "id": "12345678-1234-1234-1234-123456789003",
                    "status": "Finalized",
                    "template": {
                        "keys": {
                            "0": {
                                "id": 0,
                                "alias": "Wallet Manager Key",
                                "description": "Primary owner key",
                                "email": "alice@example.com",
                                "key_type": "Internal"
                            },
                            "1": {
                                "id": 1,
                                "alias": "Recovery Key",
                                "description": "Safety net recovery key",
                                "email": "recovery@example.com",
                                "key_type": "SafetyNet"
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
                                "key_ids": [1],
                                "timelock": {"blocks": 52560}
                            }
                        ]
                    },
                    "last_edited": 1700000000,
                    "last_editor": "12345678-1234-1234-1234-123456789004"
                }
            }
        }"#;

        let mut keys = BTreeMap::new();
        keys.insert(
            0,
            Key {
                id: 0,
                alias: "Wallet Manager Key".to_string(),
                description: "Primary owner key".to_string(),
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
                alias: "Recovery Key".to_string(),
                description: "Safety net recovery key".to_string(),
                identity: KeyIdentity::Email("recovery@example.com".to_string()),
                key_type: KeyType::SafetyNet,
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
            secondary_paths: vec![SecondaryPath {
                path: SpendingPath::new(false, 1, vec![1]),
                timelock: Timelock::new(52560),
            }],
        };

        let wallet = Wallet {
            alias: "Recovery Vault".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Finalized,
            template: Some(template),
            last_edited: Some(1700000000),
            last_editor: Some(test_uuid(4)),
            descriptor: None,
            devices: None,
        };
        let request = Request::EditWallet { wallet };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-008");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_edit_xpub_wire_format() {
        // Documentation: EditXpub request wire format
        let expected_json = r#"{
            "type": "edit_xpub",
            "token": "test-token",
            "request_id": "req-009",
            "payload": {
                "wallet_id": "12345678-1234-1234-1234-123456789001",
                "key_id": 0,
                "xpub": {
                    "value": "xpub661MyMwAqRbcFtest",
                    "source": "Device"
                }
            }
        }"#;

        let xpub = Xpub {
            value: "xpub661MyMwAqRbcFtest".to_string(),
            source: XpubSource::Device,
            device_kind: None,
            device_version: None,
            file_name: None,
        };
        let request = Request::EditXpub {
            wallet_id: test_uuid(1),
            key_id: 0,
            xpub: Some(xpub),
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-009");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_edit_xpub_clear_wire_format() {
        // Documentation: EditXpub request wire format when clearing xpub (None)
        let expected_json = r#"{
            "type": "edit_xpub",
            "token": "test-token",
            "request_id": "req-010",
            "payload": {
                "wallet_id": "12345678-1234-1234-1234-123456789001",
                "key_id": 0
            }
        }"#;

        let request = Request::EditXpub {
            wallet_id: test_uuid(1),
            key_id: 0,
            xpub: None,
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-010");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_edit_xpub_with_device_metadata_wire_format() {
        // Documentation: EditXpub with device metadata (device_kind + device_version)
        let expected_json = r#"{
            "type": "edit_xpub",
            "token": "test-token",
            "request_id": "req-009",
            "payload": {
                "wallet_id": "12345678-1234-1234-1234-123456789001",
                "key_id": 0,
                "xpub": {
                    "value": "xpub661MyMwAqRbcFtest",
                    "source": "Device",
                    "device_kind": "LedgerNanoS",
                    "device_version": "2.1.0"
                }
            }
        }"#;

        let xpub = Xpub {
            value: "xpub661MyMwAqRbcFtest".to_string(),
            source: XpubSource::Device,
            device_kind: Some(DeviceKind::LedgerNanoS),
            device_version: Some("2.1.0".to_string()),
            file_name: None,
        };
        let request = Request::EditXpub {
            wallet_id: test_uuid(1),
            key_id: 0,
            xpub: Some(xpub),
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-009");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_edit_xpub_with_file_metadata_wire_format() {
        // Documentation: EditXpub with file metadata (file_name)
        let expected_json = r#"{
            "type": "edit_xpub",
            "token": "test-token",
            "request_id": "req-009",
            "payload": {
                "wallet_id": "12345678-1234-1234-1234-123456789001",
                "key_id": 0,
                "xpub": {
                    "value": "xpub661MyMwAqRbcFtest",
                    "source": "File",
                    "file_name": "coldcard-export.json"
                }
            }
        }"#;

        let xpub = Xpub {
            value: "xpub661MyMwAqRbcFtest".to_string(),
            source: XpubSource::File,
            device_kind: None,
            device_version: None,
            file_name: Some("coldcard-export.json".to_string()),
        };
        let request = Request::EditXpub {
            wallet_id: test_uuid(1),
            key_id: 0,
            xpub: Some(xpub),
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-009");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_device_registered_wire_format() {
        use crate::ws_business::RegistrationInfos;
        use miniscript::bitcoin::bip32::Fingerprint;

        // Documentation: DeviceRegistered request wire format
        let expected_json = r#"{
            "type": "device_registered",
            "token": "test-token",
            "request_id": "req-011",
            "payload": {
                "wallet_id": "12345678-1234-1234-1234-123456789001",
                "infos": {
                    "user": "12345678-1234-1234-1234-123456789002",
                    "fingerprint": "d34db33f",
                    "registered": true,
                    "registered_alias": null,
                    "proof_of_registration": [161, 178, 195, 212, 229, 246]
                }
            }
        }"#;

        let fingerprint = Fingerprint::from_hex("d34db33f").unwrap();
        let mut infos = RegistrationInfos::new(test_uuid(2), fingerprint);
        infos.registered = true;
        infos.proof_of_registration = Some(vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6]);

        let request = Request::DeviceRegistered {
            wallet_id: test_uuid(1),
            infos,
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-011");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_device_registered_no_proof_wire_format() {
        use crate::ws_business::RegistrationInfos;
        use miniscript::bitcoin::bip32::Fingerprint;

        // Documentation: DeviceRegistered without proof_of_registration (non-Ledger device)
        let expected_json = r#"{
            "type": "device_registered",
            "token": "test-token",
            "request_id": "req-012",
            "payload": {
                "wallet_id": "12345678-1234-1234-1234-123456789001",
                "infos": {
                    "user": "12345678-1234-1234-1234-123456789002",
                    "fingerprint": "cafebabe",
                    "registered": true,
                    "registered_alias": null,
                    "proof_of_registration": null
                }
            }
        }"#;

        let fingerprint = Fingerprint::from_hex("cafebabe").unwrap();
        let mut infos = RegistrationInfos::new(test_uuid(2), fingerprint);
        infos.registered = true;
        // proof_of_registration is None for non-Ledger devices

        let request = Request::DeviceRegistered {
            wallet_id: test_uuid(1),
            infos,
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-012");

        let actual: serde_json::Value = ws_msg_to_json(ws_msg);
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
        roundtrip_request(expected_json);
    }

    #[test]
    fn test_request_from_ws_message_device_registered() {
        use miniscript::bitcoin::bip32::Fingerprint;

        let json = r#"{
            "type": "device_registered",
            "token": "test-token",
            "request_id": "req-011",
            "payload": {
                "wallet_id": "12345678-1234-1234-1234-123456789001",
                "infos": {
                    "user": "12345678-1234-1234-1234-123456789002",
                    "fingerprint": "d34db33f",
                    "registered": true,
                    "registered_alias": null,
                    "proof_of_registration": [161, 178, 195, 212, 229, 246]
                }
            }
        }"#;

        let ws_msg = WsMessage::Text(json.to_string());
        let (request, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-011");

        match request {
            Request::DeviceRegistered { wallet_id, infos } => {
                assert_eq!(wallet_id, test_uuid(1));
                assert_eq!(infos.user, test_uuid(2));
                assert_eq!(
                    infos.fingerprint,
                    Fingerprint::from_hex("d34db33f").unwrap()
                );
                assert!(infos.registered);
                assert_eq!(
                    infos.proof_of_registration,
                    Some(vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6])
                );
            }
            _ => panic!("Expected DeviceRegistered request"),
        }
    }

    // ==================== RESPONSE WIRE FORMAT TESTS ====================
    // These tests parse hardcoded JSON strings representing server responses.
    // They serve as documentation and detect breaking changes.

    #[test]
    fn test_response_connected_wire_format() {
        // Documentation: Connected response format from server
        let expected_json = r#"{
            "type": "connected",
            "request_id": "req-001",
            "payload": {"version": 1, "user": "12345678-1234-1234-1234-123456789001"}
        }"#;

        // Create response and roundtrip through API
        let response = Response::Connected {
            version: 1,
            user: test_uuid(1),
        };
        let ws_msg = response.to_ws_message(Some("req-001"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        assert!(matches!(
            parsed,
            Response::Connected { version: 1, user } if user == test_uuid(1)
        ));
        assert_eq!(request_id, Some("req-001".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-001")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_pong_wire_format() {
        // Documentation: Pong response format from server
        let expected_json = r#"{
            "type": "pong",
            "request_id": "req-002"
        }"#;

        // Create response and roundtrip through API
        let response = Response::Pong;
        let ws_msg = response.to_ws_message(Some("req-002"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        assert!(matches!(parsed, Response::Pong));
        assert_eq!(request_id, Some("req-002".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-002")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_org_wire_format() {
        // Documentation: Org response format from server
        let expected_json = r#"{
            "type": "org",
            "request_id": "req-004",
            "payload": {
                "name": "Acme Corp",
                "id": "12345678-1234-1234-1234-123456789001",
                "wallets": ["aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"],
                "users": ["bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"],
                "owners": ["cccccccc-cccc-cccc-cccc-cccccccccccc"]
            }
        }"#;

        // Create response and roundtrip through API
        let org = Org {
            name: "Acme Corp".to_string(),
            id: Uuid::parse_str("12345678-1234-1234-1234-123456789001").unwrap(),
            wallets: BTreeSet::from([
                Uuid::parse_str("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa").unwrap()
            ]),
            users: BTreeSet::from([
                Uuid::parse_str("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").unwrap()
            ]),
            owners: vec![Uuid::parse_str("cccccccc-cccc-cccc-cccc-cccccccccccc").unwrap()],
            last_edited: None,
            last_editor: None,
        };
        let response = Response::Org { org: org.clone() };
        let ws_msg = response.to_ws_message(Some("req-004"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        match parsed {
            Response::Org { org: parsed_org } => assert_eq!(parsed_org, org),
            _ => panic!("Expected Org response"),
        }
        assert_eq!(request_id, Some("req-004".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-004")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_org_empty_collections_wire_format() {
        // Documentation: Org response with empty collections
        let expected_json = r#"{
            "type": "org",
            "request_id": "req-004",
            "payload": {
                "name": "Empty Org",
                "id": "12345678-1234-1234-1234-123456789001",
                "wallets": [],
                "users": [],
                "owners": []
            }
        }"#;

        // Create response and roundtrip through API
        let org = Org {
            name: "Empty Org".to_string(),
            id: test_uuid(1),
            wallets: BTreeSet::new(),
            users: BTreeSet::new(),
            owners: vec![],
            last_edited: None,
            last_editor: None,
        };
        let response = Response::Org { org: org.clone() };
        let ws_msg = response.to_ws_message(Some("req-004"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        match parsed {
            Response::Org { org: parsed_org } => assert_eq!(parsed_org, org),
            _ => panic!("Expected Org response"),
        }
        assert_eq!(request_id, Some("req-004".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-004")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_wallet_wire_format() {
        // Documentation: Wallet response format from server
        let expected_json = r#"{
            "type": "wallet",
            "request_id": "req-005",
            "payload": {
                "alias": "Main Vault",
                "org": "12345678-1234-1234-1234-123456789001",
                "owner": "12345678-1234-1234-1234-123456789002",
                "id": "12345678-1234-1234-1234-123456789003",
                "status": "Drafted"
            }
        }"#;

        // Create response and roundtrip through API
        let wallet = Wallet {
            alias: "Main Vault".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Drafted,
            template: None,
            last_edited: None,
            last_editor: None,
            descriptor: None,
            devices: None,
        };
        let response = Response::Wallet {
            wallet: wallet.clone(),
        };
        let ws_msg = response.to_ws_message(Some("req-005"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        match parsed {
            Response::Wallet {
                wallet: parsed_wallet,
            } => assert_eq!(parsed_wallet, wallet),
            _ => panic!("Expected Wallet response"),
        }
        assert_eq!(request_id, Some("req-005".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-005")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_wallet_with_template_wire_format() {
        // Documentation: Wallet response with full policy template
        let expected_json = r#"{
            "type": "wallet",
            "request_id": "req-005",
            "payload": {
                "alias": "Multisig Vault",
                "org": "12345678-1234-1234-1234-123456789001",
                "owner": "12345678-1234-1234-1234-123456789002",
                "id": "12345678-1234-1234-1234-123456789003",
                "status": "Validated",
                "template": {
                    "keys": {
                        "0": {
                            "id": 0,
                            "alias": "Wallet Manager Key",
                            "description": "Primary owner key",
                            "email": "owner@example.com",
                            "key_type": "Internal"
                        }
                    },
                    "primary_path": {
                        "is_primary": true,
                        "threshold_n": 1,
                        "key_ids": [0]
                    },
                    "secondary_paths": []
                }
            }
        }"#;

        // Create response and roundtrip through API
        let mut keys = BTreeMap::new();
        keys.insert(
            0,
            Key {
                id: 0,
                alias: "Wallet Manager Key".to_string(),
                description: "Primary owner key".to_string(),
                identity: KeyIdentity::Email("owner@example.com".to_string()),
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
        let wallet = Wallet {
            alias: "Multisig Vault".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Validated,
            template: Some(PolicyTemplate {
                keys,
                primary_path: SpendingPath::new(true, 1, vec![0]),
                secondary_paths: vec![],
            }),
            last_edited: None,
            last_editor: None,
            descriptor: None,
            devices: None,
        };
        let response = Response::Wallet {
            wallet: wallet.clone(),
        };
        let ws_msg = response.to_ws_message(Some("req-005"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        match parsed {
            Response::Wallet {
                wallet: parsed_wallet,
            } => assert_eq!(parsed_wallet, wallet),
            _ => panic!("Expected Wallet response"),
        }
        assert_eq!(request_id, Some("req-005".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-005")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_user_wire_format() {
        // Documentation: User response format from server
        let expected_json = r#"{
            "type": "user",
            "request_id": "req-006",
            "payload": {
                "name": "Alice Smith",
                "uuid": "12345678-1234-1234-1234-123456789001",
                "email": "alice@example.com",
                "role": "WalletManager"
            }
        }"#;

        // Create response and roundtrip through API
        let user = User {
            name: "Alice Smith".to_string(),
            uuid: test_uuid(1),
            email: "alice@example.com".to_string(),
            role: UserRole::WalletManager,
            last_edited: None,
            last_editor: None,
        };
        let response = Response::User { user: user.clone() };
        let ws_msg = response.to_ws_message(Some("req-006"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        match parsed {
            Response::User { user: parsed_user } => assert_eq!(parsed_user, user),
            _ => panic!("Expected User response"),
        }
        assert_eq!(request_id, Some("req-006".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-006")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_delete_user_org_wire_format() {
        // Documentation: DeleteUserOrg response format from server
        let expected_json = r#"{
            "type": "delete_user_org",
            "request_id": "req-007",
            "payload": {
                "user": "12345678-1234-1234-1234-123456789001",
                "org": "12345678-1234-1234-1234-123456789002"
            }
        }"#;

        // Create response and roundtrip through API
        let response = Response::DeleteUserOrg {
            user: test_uuid(1),
            org: test_uuid(2),
        };
        let ws_msg = response.to_ws_message(Some("req-007"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        match parsed {
            Response::DeleteUserOrg { user, org } => {
                assert_eq!(user, test_uuid(1));
                assert_eq!(org, test_uuid(2));
            }
            _ => panic!("Expected DeleteUserOrg response"),
        }
        assert_eq!(request_id, Some("req-007".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-007")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_response_error_wire_format() {
        // Documentation: Error response format from server
        let expected_json = r#"{
            "type": "error",
            "request_id": "req-007",
            "error": {
                "code": "NOT_FOUND",
                "message": "Wallet not found"
            }
        }"#;

        // Create response and roundtrip through API
        let error = WssError {
            code: "NOT_FOUND".to_string(),
            message: "Wallet not found".to_string(),
            request_id: None,
        };
        let response = Response::Error {
            error: error.clone(),
        };
        let ws_msg = response.to_ws_message(Some("req-007"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        match parsed {
            Response::Error {
                error: parsed_error,
            } => assert_eq!(parsed_error, error),
            _ => panic!("Expected Error response"),
        }
        assert_eq!(request_id, Some("req-007".to_string()));

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(Some("req-007")));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    // ==================== PROTOCOL STRUCTURE TESTS ====================
    // These tests verify the exact structure of protocol-level types.

    #[test]
    fn test_protocol_request_wire_format() {
        // Documentation: ProtocolRequest JSON structure with payload
        let json = r#"{
            "type": "fetch_wallet",
            "token": "auth-token-xyz",
            "request_id": "req-123",
            "payload": {"id": "12345678-1234-1234-1234-123456789abc"}
        }"#;
        let parsed: ProtocolRequest = serde_json::from_str(json).unwrap();

        let expected = ProtocolRequest {
            msg_type: "fetch_wallet".to_string(),
            token: "auth-token-xyz".to_string(),
            request_id: "req-123".to_string(),
            payload: Some(serde_json::json!({"id": "12345678-1234-1234-1234-123456789abc"})),
        };

        assert_eq!(parsed, expected);
        roundtrip_request(json);
    }

    #[test]
    fn test_protocol_request_without_payload_wire_format() {
        // Documentation: ProtocolRequest JSON structure without payload
        let json = r#"{
            "type": "ping",
            "token": "auth-token-xyz",
            "request_id": "req-123"
        }"#;
        let parsed: ProtocolRequest = serde_json::from_str(json).unwrap();

        let expected = ProtocolRequest {
            msg_type: "ping".to_string(),
            token: "auth-token-xyz".to_string(),
            request_id: "req-123".to_string(),
            payload: None,
        };

        assert_eq!(parsed, expected);
        roundtrip_request(json);
    }

    #[test]
    fn test_protocol_response_wire_format() {
        // Documentation: ProtocolResponse JSON structure
        let json = r#"{
            "type": "wallet",
            "request_id": "req-123",
            "payload": {
                "alias": "Vault",
                "org": "12345678-1234-1234-1234-123456789001",
                "owner": "12345678-1234-1234-1234-123456789002",
                "id": "12345678-1234-1234-1234-123456789003",
                "status": "Created"
            }
        }"#;
        let parsed: ProtocolResponse = serde_json::from_str(json).unwrap();

        let expected = ProtocolResponse {
            msg_type: "wallet".to_string(),
            request_id: Some("req-123".to_string()),
            payload: Some(serde_json::json!({
                "alias": "Vault",
                "org": "12345678-1234-1234-1234-123456789001",
                "owner": "12345678-1234-1234-1234-123456789002",
                "id": "12345678-1234-1234-1234-123456789003",
                "status": "Created"
            })),
            error: None,
        };

        assert_eq!(parsed, expected);
        roundtrip_response(json);
    }

    #[test]
    fn test_wss_error_wire_format() {
        // Documentation: WssError JSON structure
        let json = r#"{
            "code": "UNAUTHORIZED",
            "message": "Invalid token",
            "request_id": "req-123"
        }"#;
        let parsed: WssError = serde_json::from_str(json).unwrap();

        let expected = WssError {
            code: "UNAUTHORIZED".to_string(),
            message: "Invalid token".to_string(),
            request_id: Some("req-123".to_string()),
        };

        assert_eq!(parsed, expected);
        roundtrip_wss_error(json);
    }

    #[test]
    fn test_wss_error_without_request_id_wire_format() {
        // Documentation: WssError without optional request_id
        let json = r#"{
            "code": "INTERNAL_ERROR",
            "message": "Server error"
        }"#;
        let parsed: WssError = serde_json::from_str(json).unwrap();

        let expected = WssError {
            code: "INTERNAL_ERROR".to_string(),
            message: "Server error".to_string(),
            request_id: None,
        };

        assert_eq!(parsed, expected);
        roundtrip_wss_error(json);
    }

    // ==================== ERROR CASE TESTS ====================
    // These tests verify error handling for malformed/invalid messages.

    #[test]
    fn test_error_binary_message() {
        // Binary WebSocket messages should be rejected
        let ws_msg = WsMessage::Binary(vec![0, 1, 2, 3]);
        let result = Response::from_ws_message(ws_msg);

        assert!(matches!(
            result,
            Err(WssConversionError::InvalidMessageType)
        ));
    }

    #[test]
    fn test_error_malformed_json() {
        // Malformed JSON should produce DeserializationFailed
        let ws_msg = WsMessage::Text("not valid json".to_string());
        let result = Response::from_ws_message(ws_msg);

        assert!(matches!(
            result,
            Err(WssConversionError::DeserializationFailed(_))
        ));
    }

    #[test]
    fn test_error_missing_type_field() {
        // Missing "type" field should fail
        let json = r#"{"request_id": "req-001", "payload": {}}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        assert!(matches!(
            result,
            Err(WssConversionError::DeserializationFailed(_))
        ));
    }

    #[test]
    fn test_error_unknown_message_type() {
        // Unknown message type should fail
        let json = r#"{"type": "unknown_type", "request_id": "req-001"}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Unknown message type"));
            }
            _ => panic!("Expected DeserializationFailed with unknown message type"),
        }
    }

    #[test]
    fn test_error_connected_missing_payload() {
        // "connected" response without payload should fail
        let json = r#"{"type": "connected", "request_id": "req-001"}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing payload"));
            }
            _ => panic!("Expected DeserializationFailed with missing payload"),
        }
    }

    #[test]
    fn test_error_connected_missing_version() {
        // "connected" response with payload but missing version should fail
        let json = r#"{"type": "connected", "request_id": "req-001", "payload": {}}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing version"));
            }
            _ => panic!("Expected DeserializationFailed with missing version"),
        }
    }

    #[test]
    fn test_error_org_missing_payload() {
        // "org" response without payload should fail
        let json = r#"{"type": "org", "request_id": "req-001"}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing payload"));
            }
            _ => panic!("Expected DeserializationFailed with missing payload"),
        }
    }

    #[test]
    fn test_error_org_invalid_payload() {
        // "org" response with invalid org structure should fail
        let json = r#"{"type": "org", "request_id": "req-001", "payload": {"invalid": true}}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        assert!(matches!(
            result,
            Err(WssConversionError::DeserializationFailed(_))
        ));
    }

    #[test]
    fn test_error_wallet_missing_payload() {
        // "wallet" response without payload should fail
        let json = r#"{"type": "wallet", "request_id": "req-001"}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing payload"));
            }
            _ => panic!("Expected DeserializationFailed with missing payload"),
        }
    }

    #[test]
    fn test_error_wallet_invalid_payload() {
        // "wallet" response with invalid wallet structure should fail
        let json = r#"{"type": "wallet", "request_id": "req-001", "payload": {"invalid": true}}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        assert!(matches!(
            result,
            Err(WssConversionError::DeserializationFailed(_))
        ));
    }

    #[test]
    fn test_error_user_missing_payload() {
        // "user" response without payload should fail
        let json = r#"{"type": "user", "request_id": "req-001"}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing payload"));
            }
            _ => panic!("Expected DeserializationFailed with missing payload"),
        }
    }

    #[test]
    fn test_error_user_invalid_payload() {
        // "user" response with invalid user structure should fail
        let json = r#"{"type": "user", "request_id": "req-001", "payload": {"invalid": true}}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Response::from_ws_message(ws_msg);

        assert!(matches!(
            result,
            Err(WssConversionError::DeserializationFailed(_))
        ));
    }

    #[test]
    fn test_response_without_request_id() {
        // Responses without request_id should still parse
        let expected_json = r#"{"type": "pong"}"#;

        // Create response and roundtrip through API (no request_id)
        let response = Response::Pong;
        let ws_msg = response.to_ws_message(None);
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        // Verify roundtrip
        assert!(matches!(parsed, Response::Pong));
        assert!(request_id.is_none());

        // Verify wire format against hardcoded JSON
        let actual: serde_json::Value = ws_msg_to_json(response.to_ws_message(None));
        let expected: serde_json::Value = serde_json::from_str(expected_json).unwrap();
        assert_eq!(actual, expected);
    }

    // ==================== REQUEST METHOD TESTS ====================

    #[test]
    fn test_request_method() {
        assert_eq!(Request::Connect { version: 1 }.method(), "connect");
        assert_eq!(Request::Ping.method(), "ping");
        assert_eq!(Request::Close.method(), "close");
        assert_eq!(Request::FetchOrg { id: test_uuid(1) }.method(), "fetch_org");
        assert_eq!(
            Request::FetchWallet { id: test_uuid(1) }.method(),
            "fetch_wallet"
        );
        assert_eq!(
            Request::FetchUser { id: test_uuid(1) }.method(),
            "fetch_user"
        );
        assert_eq!(
            Request::EditWallet {
                wallet: Wallet {
                    alias: "Test".to_string(),
                    org: test_uuid(1),
                    owner: test_uuid(2),
                    id: test_uuid(3),
                    status: WalletStatus::Drafted,
                    template: None,
                    last_edited: None,
                    last_editor: None,
                    descriptor: None,
                    devices: None,
                }
            }
            .method(),
            "edit_wallet"
        );
        assert_eq!(
            Request::EditXpub {
                wallet_id: test_uuid(1),
                key_id: 0,
                xpub: None
            }
            .method(),
            "edit_xpub"
        );
    }

    // ==================== RESPONSE METHOD TESTS ====================

    #[test]
    fn test_response_method() {
        assert_eq!(
            Response::Connected {
                version: 1,
                user: test_uuid(1)
            }
            .method(),
            "connected"
        );
        assert_eq!(Response::Pong.method(), "pong");
        assert_eq!(
            Response::Org {
                org: Org {
                    name: "Test".to_string(),
                    id: test_uuid(1),
                    wallets: BTreeSet::new(),
                    users: BTreeSet::new(),
                    owners: vec![],
                    last_edited: None,
                    last_editor: None,
                }
            }
            .method(),
            "org"
        );
        assert_eq!(
            Response::Wallet {
                wallet: Wallet {
                    alias: "Test".to_string(),
                    org: test_uuid(1),
                    owner: test_uuid(2),
                    id: test_uuid(3),
                    status: WalletStatus::Drafted,
                    template: None,
                    last_edited: None,
                    last_editor: None,
                    descriptor: None,
                    devices: None,
                }
            }
            .method(),
            "wallet"
        );
        assert_eq!(
            Response::User {
                user: User {
                    name: "Test".to_string(),
                    uuid: test_uuid(1),
                    email: "test@example.com".to_string(),
                    role: UserRole::Participant,
                    last_edited: None,
                    last_editor: None,
                }
            }
            .method(),
            "user"
        );
        assert_eq!(
            Response::Error {
                error: WssError {
                    code: "ERR".to_string(),
                    message: "Error".to_string(),
                    request_id: None,
                }
            }
            .method(),
            "error"
        );
        assert_eq!(
            Response::DeleteUserOrg {
                user: test_uuid(1),
                org: test_uuid(2),
            }
            .method(),
            "delete_user_org"
        );
    }

    // ==================== REQUEST ROUNDTRIP TESTS ====================

    #[test]
    fn test_request_from_ws_message_connect() {
        let request = Request::Connect { version: 1 };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-001");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        assert!(matches!(parsed, Request::Connect { version: 1 }));
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-001");
    }

    #[test]
    fn test_request_from_ws_message_ping() {
        let request = Request::Ping;
        let ws_msg = request.to_ws_message_with_id("test-token", "req-002");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        assert!(matches!(parsed, Request::Ping));
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-002");
    }

    #[test]
    fn test_request_from_ws_message_close() {
        let request = Request::Close;
        let ws_msg = request.to_ws_message_with_id("test-token", "req-003");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        assert!(matches!(parsed, Request::Close));
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-003");
    }

    #[test]
    fn test_request_from_ws_message_fetch_org() {
        let request = Request::FetchOrg { id: test_uuid(1) };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-004");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        match parsed {
            Request::FetchOrg { id } => assert_eq!(id, test_uuid(1)),
            _ => panic!("Expected FetchOrg"),
        }
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-004");
    }

    #[test]
    fn test_request_from_ws_message_fetch_wallet() {
        let request = Request::FetchWallet { id: test_uuid(2) };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-005");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        match parsed {
            Request::FetchWallet { id } => assert_eq!(id, test_uuid(2)),
            _ => panic!("Expected FetchWallet"),
        }
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-005");
    }

    #[test]
    fn test_request_from_ws_message_fetch_user() {
        let request = Request::FetchUser { id: test_uuid(3) };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-006");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        match parsed {
            Request::FetchUser { id } => assert_eq!(id, test_uuid(3)),
            _ => panic!("Expected FetchUser"),
        }
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-006");
    }

    #[test]
    fn test_request_from_ws_message_edit_wallet() {
        let wallet = Wallet {
            alias: "Test Wallet".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Drafted,
            template: None,
            last_edited: None,
            last_editor: None,
            descriptor: None,
            devices: None,
        };
        let request = Request::EditWallet {
            wallet: wallet.clone(),
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-007");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        match parsed {
            Request::EditWallet { wallet: w } => assert_eq!(w, wallet),
            _ => panic!("Expected EditWallet"),
        }
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-007");
    }

    #[test]
    fn test_request_from_ws_message_edit_xpub() {
        let xpub = Xpub {
            value: "xpub661MyMwAqRbcFtest".to_string(),
            source: XpubSource::Device,
            device_kind: Some(DeviceKind::LedgerNanoS),
            device_version: Some("2.1.0".to_string()),
            file_name: None,
        };
        let request = Request::EditXpub {
            wallet_id: test_uuid(1),
            key_id: 0,
            xpub: Some(xpub.clone()),
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-008");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        match parsed {
            Request::EditXpub {
                wallet_id,
                key_id,
                xpub: x,
            } => {
                assert_eq!(wallet_id, test_uuid(1));
                assert_eq!(key_id, 0);
                assert_eq!(x, Some(xpub));
            }
            _ => panic!("Expected EditXpub"),
        }
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-008");
    }

    #[test]
    fn test_request_from_ws_message_edit_xpub_clear() {
        let request = Request::EditXpub {
            wallet_id: test_uuid(1),
            key_id: 0,
            xpub: None,
        };
        let ws_msg = request.to_ws_message_with_id("test-token", "req-009");
        let (parsed, token, request_id) = Request::from_ws_message(ws_msg).unwrap();

        match parsed {
            Request::EditXpub {
                wallet_id,
                key_id,
                xpub,
            } => {
                assert_eq!(wallet_id, test_uuid(1));
                assert_eq!(key_id, 0);
                assert!(xpub.is_none());
            }
            _ => panic!("Expected EditXpub"),
        }
        assert_eq!(token, "test-token");
        assert_eq!(request_id, "req-009");
    }

    // ==================== RESPONSE ROUNDTRIP TESTS ====================

    #[test]
    fn test_response_to_ws_message_connected() {
        let response = Response::Connected {
            version: 1,
            user: test_uuid(1),
        };
        let ws_msg = response.to_ws_message(Some("req-001"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        assert!(matches!(
            parsed,
            Response::Connected { version: 1, user } if user == test_uuid(1)
        ));
        assert_eq!(request_id, Some("req-001".to_string()));
    }

    #[test]
    fn test_response_to_ws_message_pong() {
        let response = Response::Pong;
        let ws_msg = response.to_ws_message(Some("req-002"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        assert!(matches!(parsed, Response::Pong));
        assert_eq!(request_id, Some("req-002".to_string()));
    }

    #[test]
    fn test_response_to_ws_message_org() {
        let org = Org {
            name: "Test Org".to_string(),
            id: test_uuid(1),
            wallets: BTreeSet::from([test_uuid(2)]),
            users: BTreeSet::from([test_uuid(3)]),
            owners: vec![test_uuid(4)],
            last_edited: Some(1700000000),
            last_editor: Some(test_uuid(5)),
        };
        let response = Response::Org { org: org.clone() };
        let ws_msg = response.to_ws_message(Some("req-003"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        match parsed {
            Response::Org { org: o } => assert_eq!(o, org),
            _ => panic!("Expected Org response"),
        }
        assert_eq!(request_id, Some("req-003".to_string()));
    }

    #[test]
    fn test_response_to_ws_message_wallet() {
        let wallet = Wallet {
            alias: "Test Wallet".to_string(),
            org: test_uuid(1),
            owner: test_uuid(2),
            id: test_uuid(3),
            status: WalletStatus::Validated,
            template: None,
            last_edited: Some(1700000000),
            last_editor: Some(test_uuid(4)),
            descriptor: None,
            devices: None,
        };
        let response = Response::Wallet {
            wallet: wallet.clone(),
        };
        let ws_msg = response.to_ws_message(Some("req-004"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        match parsed {
            Response::Wallet { wallet: w } => assert_eq!(w, wallet),
            _ => panic!("Expected Wallet response"),
        }
        assert_eq!(request_id, Some("req-004".to_string()));
    }

    #[test]
    fn test_response_to_ws_message_user() {
        let user = User {
            name: "Alice".to_string(),
            uuid: test_uuid(1),
            email: "alice@example.com".to_string(),
            role: UserRole::WalletManager,
            last_edited: None,
            last_editor: None,
        };
        let response = Response::User { user: user.clone() };
        let ws_msg = response.to_ws_message(Some("req-005"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        match parsed {
            Response::User { user: u } => assert_eq!(u, user),
            _ => panic!("Expected User response"),
        }
        assert_eq!(request_id, Some("req-005".to_string()));
    }

    #[test]
    fn test_response_to_ws_message_error() {
        let error = WssError {
            code: "NOT_FOUND".to_string(),
            message: "Resource not found".to_string(),
            request_id: None,
        };
        let response = Response::Error {
            error: error.clone(),
        };
        let ws_msg = response.to_ws_message(Some("req-006"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        match parsed {
            Response::Error { error: e } => assert_eq!(e, error),
            _ => panic!("Expected Error response"),
        }
        assert_eq!(request_id, Some("req-006".to_string()));
    }

    #[test]
    fn test_response_to_ws_message_delete_user_org() {
        let response = Response::DeleteUserOrg {
            user: test_uuid(1),
            org: test_uuid(2),
        };
        let ws_msg = response.to_ws_message(Some("req-007"));
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        match parsed {
            Response::DeleteUserOrg { user, org } => {
                assert_eq!(user, test_uuid(1));
                assert_eq!(org, test_uuid(2));
            }
            _ => panic!("Expected DeleteUserOrg response"),
        }
        assert_eq!(request_id, Some("req-007".to_string()));
    }

    #[test]
    fn test_response_to_ws_message_no_request_id() {
        let response = Response::Pong;
        let ws_msg = response.to_ws_message(None);
        let (parsed, request_id) = Response::from_ws_message(ws_msg).unwrap();

        assert!(matches!(parsed, Response::Pong));
        assert!(request_id.is_none());
    }

    // ==================== REQUEST ERROR CASE TESTS ====================

    #[test]
    fn test_request_from_ws_message_binary() {
        let ws_msg = WsMessage::Binary(vec![0, 1, 2, 3]);
        let result = Request::from_ws_message(ws_msg);

        assert!(matches!(
            result,
            Err(WssConversionError::InvalidMessageType)
        ));
    }

    #[test]
    fn test_request_from_ws_message_malformed_json() {
        let ws_msg = WsMessage::Text("not valid json".to_string());
        let result = Request::from_ws_message(ws_msg);

        assert!(matches!(
            result,
            Err(WssConversionError::DeserializationFailed(_))
        ));
    }

    #[test]
    fn test_request_from_ws_message_unknown_type() {
        let json = r#"{"type": "unknown", "token": "t", "request_id": "r"}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Request::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Unknown message type"));
            }
            _ => panic!("Expected DeserializationFailed with unknown message type"),
        }
    }

    #[test]
    fn test_request_from_ws_message_connect_missing_payload() {
        let json = r#"{"type": "connect", "token": "t", "request_id": "r"}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Request::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing payload"));
            }
            _ => panic!("Expected DeserializationFailed with missing payload"),
        }
    }

    #[test]
    fn test_request_from_ws_message_fetch_missing_id() {
        let json = r#"{"type": "fetch_org", "token": "t", "request_id": "r", "payload": {}}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Request::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing id"));
            }
            _ => panic!("Expected DeserializationFailed with missing id"),
        }
    }

    #[test]
    fn test_request_from_ws_message_edit_wallet_missing_wallet() {
        let json = r#"{"type": "edit_wallet", "token": "t", "request_id": "r", "payload": {}}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Request::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing wallet"));
            }
            _ => panic!("Expected DeserializationFailed with missing wallet"),
        }
    }

    #[test]
    fn test_request_from_ws_message_edit_xpub_missing_wallet_id() {
        let json =
            r#"{"type": "edit_xpub", "token": "t", "request_id": "r", "payload": {"key_id": 0}}"#;
        let ws_msg = WsMessage::Text(json.to_string());
        let result = Request::from_ws_message(ws_msg);

        match result {
            Err(WssConversionError::DeserializationFailed(msg)) => {
                assert!(msg.contains("Missing wallet_id"));
            }
            _ => panic!("Expected DeserializationFailed with missing wallet_id"),
        }
    }

    // ==================== PAYLOAD TESTS ====================

    #[test]
    fn test_request_payload() {
        assert!(Request::Connect { version: 1 }.payload().is_some());
        assert!(Request::Ping.payload().is_none());
        assert!(Request::Close.payload().is_none());
        assert!(Request::FetchOrg { id: test_uuid(1) }.payload().is_some());
        assert!(Request::FetchWallet { id: test_uuid(1) }
            .payload()
            .is_some());
        assert!(Request::FetchUser { id: test_uuid(1) }.payload().is_some());
    }

    #[test]
    fn test_response_payload() {
        assert!(Response::Connected {
            version: 1,
            user: test_uuid(1)
        }
        .payload()
        .is_some());
        assert!(Response::Pong.payload().is_none());
        assert!(Response::Error {
            error: WssError {
                code: "ERR".to_string(),
                message: "Error".to_string(),
                request_id: None,
            }
        }
        .payload()
        .is_none());
    }
}
