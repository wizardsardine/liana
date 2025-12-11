use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpStream};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use miniscript::DescriptorPublicKey;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::mpsc;
use tungstenite::client::{connect_with_config, IntoClientRequest};
use tungstenite::{connect, Message as WsMessage};
use uuid::Uuid;

use crate::backend::{
    Backend, Error, Notification, Org, OrgData, User, UserRole, Wallet, WalletStatus,
};
use crate::models::{Key, KeyType, PolicyTemplate, SpendingPath, Timelock};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WssRequestType {
    Connect,
    Ping,
    Close,
    FetchOrg,
    RemoveWalletFromOrg,
    CreateWallet,
    EditWallet,
    FetchWallet,
    EditXpub,
    FetchUser,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WssResponseType {
    Connected,
    Pong,
    Org,
    Wallet,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WssRequest {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub token: String,
    pub request_id: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WssResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WssError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WssError {
    pub code: String,
    pub message: String,
}

// JSON structures for protocol messages
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xpub: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchUserPayload {
    pub id: String,
}

// JSON representations of domain objects
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

// Conversion helpers
impl WalletStatus {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "Created" => Some(WalletStatus::Created),
            "Drafted" => Some(WalletStatus::Drafted),
            "Validated" => Some(WalletStatus::Validated),
            "Finalized" => Some(WalletStatus::Finalized),
            _ => None,
        }
    }
}

impl UserRole {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "WSManager" => Some(UserRole::WSManager),
            "Owner" => Some(UserRole::Owner),
            "Participant" => Some(UserRole::Participant),
            _ => None,
        }
    }

    #[allow(dead_code)]
    fn as_str(&self) -> &'static str {
        match self {
            UserRole::WSManager => "WSManager",
            UserRole::Owner => "Owner",
            UserRole::Participant => "Participant",
        }
    }
}

impl KeyType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "Internal" => Some(KeyType::Internal),
            "External" => Some(KeyType::External),
            "Cosigner" => Some(KeyType::Cosigner),
            "SafetyNet" => Some(KeyType::SafetyNet),
            _ => None,
        }
    }
}

// Conversion from JSON to domain objects
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

        // Note: owner User will need to be fetched separately or provided
        // For now, we'll create a placeholder - the actual implementation should fetch it
        let owner = User {
            name: String::new(),
            uuid: owner_id,
            email: String::new(),
            orgs: Vec::new(),
            role: UserRole::Owner,
        };

        let template = json.template.map(|t| t.try_into()).transpose()?;

        Ok(Wallet {
            alias: json.alias,
            org,
            owner,
            id,
            status,
            template,
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

        Ok(Key {
            id: json.id,
            alias: json.alias,
            description: json.description,
            email: json.email,
            key_type,
            xpub,
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

#[derive(Debug)]
pub struct WssRequestMessage {
    pub msg_type: String,
    pub payload: Value,
}

/// WSS Backend implementation
#[derive(Debug)]
pub struct WssClient {
    orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: Arc<Mutex<BTreeMap<Uuid, User>>>,
    token: Option<String>,
    request_sender: Option<mpsc::Sender<WssRequestMessage>>,
    wss_thread_handle: Option<thread::JoinHandle<()>>,
    connected: bool,
}

impl WssClient {
    pub fn new() -> Self {
        Self {
            orgs: Arc::new(Mutex::new(BTreeMap::new())),
            wallets: Arc::new(Mutex::new(BTreeMap::new())),
            users: Arc::new(Mutex::new(BTreeMap::new())),
            token: None,
            request_sender: None,
            wss_thread_handle: None,
            connected: false,
        }
    }

    pub fn set_token(&mut self, token: String) {
        self.token = Some(token);
    }
}

// WSS thread function
#[allow(clippy::too_many_arguments)]
fn wss_thread(
    url: String,
    token: String,
    version: u8,
    orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_receiver: mpsc::Receiver<WssRequestMessage>,
    notif_sender: mpsc::Sender<Notification>,
) {
    // Ensure URL has proper scheme
    let url = if url.starts_with("ws://") || url.starts_with("wss://") {
        url
    } else {
        format!("wss://{}", url)
    };

    // Connect to WSS
    let (mut ws_stream, _) = match tungstenite::connect(&url) {
        Ok(stream) => stream,
        Err(e) => {
            let _ = notif_sender.send(Notification::Error(Error::SubscriptionFailed));
            eprintln!("Failed to connect to WSS: {}", e);
            return;
        }
    };

    // We need to enable non-blocking read
    match ws_stream.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => {
            stream.set_nonblocking(true).expect("must not fail");
        }
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            stream
                .get_ref()
                .set_nonblocking(true)
                .expect("must not fail");
        }
        _ => unreachable!(),
    }

    // Send connect message
    let connect_request = WssRequest {
        msg_type: "connect".to_string(),
        token: token.clone(),
        request_id: Uuid::new_v4().to_string(),
        payload: serde_json::to_value(ConnectPayload { version }).unwrap(),
    };

    if let Err(e) = ws_stream.send(WsMessage::Text(
        serde_json::to_string(&connect_request).unwrap(),
    )) {
        let _ = notif_sender.send(Notification::Error(Error::SubscriptionFailed));
        eprintln!("Failed to send connect message: {}", e);
        return;
    }

    // Main message loop
    loop {
        // Check for outgoing requests (non-blocking)
        while let Ok(request) = request_receiver.try_recv() {
            // Handle close request specially
            if request.msg_type == "close" {
                let _ = ws_stream.close(None);
                break;
            }

            let wss_request = WssRequest {
                msg_type: request.msg_type,
                token: token.clone(),
                request_id: Uuid::new_v4().to_string(),
                payload: request.payload,
            };

            if let Err(e) = ws_stream.send(WsMessage::Text(
                serde_json::to_string(&wss_request).unwrap(),
            )) {
                eprintln!("Failed to send request: {}", e);
                break;
            }
        }

        // Check for incoming messages (blocking, but that's OK in a thread)
        match ws_stream.read() {
            Ok(WsMessage::Text(text)) => {
                if let Err(e) = handle_wss_message(&text, &orgs, &wallets, &users, &notif_sender) {
                    eprintln!("Error handling WSS message: {}", e);
                }
            }
            Ok(WsMessage::Close(_)) => {
                let _ = notif_sender.send(Notification::Error(Error::SubscriptionFailed));
                break;
            }
            Ok(_) => {} // Ignore other message types
            Err(tungstenite::Error::ConnectionClosed) => {
                let _ = notif_sender.send(Notification::Error(Error::SubscriptionFailed));
                break;
            }
            Err(tungstenite::Error::AlreadyClosed) => {
                break;
            }
            Err(e) => {
                eprintln!("WSS error: {}", e);
                let _ = notif_sender.send(Notification::Error(Error::SubscriptionFailed));
                break;
            }
        }
    }
}

fn handle_wss_message(
    text: &str,
    orgs: &Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    n_sender: &mpsc::Sender<Notification>,
) -> Result<(), String> {
    let response: WssResponse =
        serde_json::from_str(text).map_err(|e| format!("Failed to parse WSS response: {}", e))?;

    // Handle errors
    if let Some(error) = response.error {
        let _ = n_sender.send(Notification::Error(Error::SubscriptionFailed));
        return Err(format!("WSS error: {} - {}", error.code, error.message));
    }

    match response.msg_type.as_str() {
        "connected" => handle_connected(response.payload, n_sender)?,
        "pong" => handle_pong()?,
        "org" => handle_org(response.payload, orgs, n_sender)?,
        "wallet" => handle_wallet(response.payload, wallets, n_sender)?,
        "user" => handle_user(response.payload, users, n_sender)?,
        _ => {
            let error_msg = format!("Unknown message type: {}", response.msg_type);
            let _ = n_sender.send(Notification::Error(Error::SubscriptionFailed));
            return Err(error_msg);
        }
    }

    Ok(())
}

fn handle_connected(
    _payload: Option<Value>,
    notification_sender: &mpsc::Sender<Notification>,
) -> Result<(), String> {
    let _ = notification_sender.send(Notification::Connected);
    Ok(())
}

fn handle_pong() -> Result<(), String> {
    // TODO:
    Ok(())
}

fn handle_org(
    payload: Option<Value>,
    orgs: &Arc<Mutex<BTreeMap<Uuid, Org>>>,
    notification_sender: &mpsc::Sender<Notification>,
) -> Result<(), String> {
    let payload: OrgJson =
        serde_json::from_value(payload.ok_or_else(|| "Missing payload".to_string())?)
            .map_err(|e| format!("Failed to parse org payload: {}", e))?;

    let org = Org::try_from(payload)?;
    let org_id = org.id;

    // Update cache
    {
        let mut orgs_guard = orgs.lock().unwrap();
        orgs_guard.insert(org_id, org.clone());
    }

    // Send response
    let _ = notification_sender.send(Notification::Org(org_id));
    Ok(())
}

fn handle_wallet(
    payload: Option<Value>,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    notification_sender: &mpsc::Sender<Notification>,
) -> Result<(), String> {
    let payload: WalletJson =
        serde_json::from_value(payload.ok_or_else(|| "Missing payload".to_string())?)
            .map_err(|e| format!("Failed to parse wallet payload: {}", e))?;

    let wallet = Wallet::try_from(payload)?;
    let wallet_id = wallet.id;

    // Check if owner user is cached, if not we need to fetch it
    // For now, we'll just update the wallet cache
    // The owner User should be fetched separately if needed
    {
        let mut wallets_guard = wallets.lock().unwrap();
        wallets_guard.insert(wallet_id, wallet.clone());
    }

    // Send response
    let _ = notification_sender.send(Notification::Wallet(wallet_id));
    Ok(())
}

fn handle_user(
    payload: Option<Value>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    notification_sender: &mpsc::Sender<Notification>,
) -> Result<(), String> {
    let payload: UserJson =
        serde_json::from_value(payload.ok_or_else(|| "Missing payload".to_string())?)
            .map_err(|e| format!("Failed to parse user payload: {}", e))?;

    let user = User::try_from(payload)?;
    let user_id = user.uuid;

    // Update cache
    {
        let mut users_guard = users.lock().unwrap();
        users_guard.insert(user_id, user.clone());
    }

    // Send response
    let _ = notification_sender.send(Notification::User(user_id));
    Ok(())
}

impl Backend for WssClient {
    fn auth_request(&mut self, _email: String) {
        // Skip implementation for now
    }

    fn auth_code(&mut self, _code: String) {
        // Skip implementation for now
    }

    fn get_orgs(&self) -> BTreeMap<Uuid, Org> {
        let orgs_guard = self.orgs.lock().unwrap();
        orgs_guard.clone()
    }

    fn get_org(&self, id: Uuid) -> Option<OrgData> {
        let orgs_guard = self.orgs.lock().unwrap();
        let wallets_guard = self.wallets.lock().unwrap();
        let org = orgs_guard.get(&id)?.clone();
        let mut wallets = BTreeMap::new();
        for w_id in &org.wallets {
            if let Some(wallet) = wallets_guard.get(w_id) {
                wallets.insert(*w_id, wallet.clone());
            }
        }
        Some(OrgData {
            name: org.name,
            id: org.id,
            wallets,
            users: org.users,
            owners: org.owners,
        })
    }

    fn get_user(&self, id: Uuid) -> Option<User> {
        let users_guard = self.users.lock().unwrap();
        users_guard.get(&id).cloned()
    }

    fn get_wallet(&self, id: Uuid) -> Option<Wallet> {
        let wallets_guard = self.wallets.lock().unwrap();
        wallets_guard.get(&id).cloned()
    }

    fn connect(&mut self, url: String, version: u8) -> mpsc::Receiver<Notification> {
        // Close existing connection if any
        if self.connected {
            self.close();
        }

        // Get token - it should have been set before connect
        // If not set, create a dummy channel and return error
        let token = match self.token.clone() {
            Some(t) => t,
            None => {
                let (sender, receiver) = mpsc::channel();
                let _ = sender.send(Notification::Error(Error::SubscriptionFailed));
                return receiver;
            }
        };

        let (request_sender, request_receiver) = mpsc::channel();
        let (notif_sender, notif_receiver) = mpsc::channel();

        let orgs = Arc::clone(&self.orgs);
        let wallets = Arc::clone(&self.wallets);
        let users = Arc::clone(&self.users);

        let handle = thread::spawn(move || {
            wss_thread(
                url,
                token,
                version,
                orgs,
                wallets,
                users,
                request_receiver,
                notif_sender,
            );
        });

        self.request_sender = Some(request_sender);
        self.wss_thread_handle = Some(handle);
        self.connected = true;

        notif_receiver
    }

    fn ping(&mut self) {
        if !self.connected {
            return;
        }

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(WssRequestMessage {
                msg_type: "ping".to_string(),
                payload: serde_json::json!({}),
            });
        }
    }

    fn close(&mut self) {
        if !self.connected {
            return;
        }

        // Send close message if possible
        if let Some(sender) = &self.request_sender {
            let _ = sender.send(WssRequestMessage {
                msg_type: "close".to_string(),
                payload: serde_json::json!({}),
            });
        }

        // Wait for thread to finish
        if let Some(handle) = self.wss_thread_handle.take() {
            let _ = handle.join();
        }

        self.connected = false;
        self.request_sender = None;
    }

    fn fetch_org(&mut self, id: Uuid) {
        if !self.connected {
            return;
        }

        if let Some(sender) = &self.request_sender {
            let payload = serde_json::to_value(FetchOrgPayload { id: id.to_string() }).unwrap();
            let _ = sender.send(WssRequestMessage {
                msg_type: "fetch_org".to_string(),
                payload,
            });
        }
    }

    fn remove_wallet_from_org(&mut self, wallet_id: Uuid, org_id: Uuid) {
        if !self.connected {
            return;
        }

        if let Some(sender) = &self.request_sender {
            let payload = serde_json::to_value(RemoveWalletFromOrgPayload {
                wallet_id: wallet_id.to_string(),
                org_id: org_id.to_string(),
            })
            .unwrap();
            let _ = sender.send(WssRequestMessage {
                msg_type: "remove_wallet_from_org".to_string(),
                payload,
            });
        }
    }

    fn create_wallet(&mut self, name: String, org: Uuid, owner: Uuid) {
        if !self.connected {
            return;
        }

        if let Some(sender) = &self.request_sender {
            let payload = serde_json::to_value(CreateWalletPayload {
                name,
                org_id: org.to_string(),
                owner_id: owner.to_string(),
            })
            .unwrap();
            let _ = sender.send(WssRequestMessage {
                msg_type: "create_wallet".to_string(),
                payload,
            });
        }
    }

    fn edit_wallet(&mut self, wallet: Wallet) {
        if !self.connected {
            return;
        }

        if let Some(sender) = &self.request_sender {
            // Convert Wallet to WalletJson
            let wallet_json = WalletJson {
                id: wallet.id.to_string(),
                alias: wallet.alias,
                org: wallet.org.to_string(),
                owner: wallet.owner.uuid.to_string(),
                status_str: wallet.status.as_str().to_string(),
                template: wallet.template.map(|t| {
                    // Convert PolicyTemplate to PolicyTemplateJson
                    let mut keys_json = BTreeMap::new();
                    for (k, v) in &t.keys {
                        keys_json.insert(
                            k.to_string(),
                            KeyJson {
                                id: v.id,
                                alias: v.alias.clone(),
                                description: v.description.clone(),
                                email: v.email.clone(),
                                key_type_str: crate::models::KeyType::as_str(&v.key_type)
                                    .to_string(),
                                xpub: v.xpub.as_ref().map(|x| x.to_string()),
                            },
                        );
                    }

                    let primary_path_json = SpendingPathJson {
                        is_primary: t.primary_path.is_primary,
                        threshold_n: t.primary_path.threshold_n,
                        key_ids: t.primary_path.key_ids.clone(),
                    };

                    let secondary_paths_json = t
                        .secondary_paths
                        .iter()
                        .map(|(path, timelock)| SecondaryPathJson {
                            path: SpendingPathJson {
                                is_primary: path.is_primary,
                                threshold_n: path.threshold_n,
                                key_ids: path.key_ids.clone(),
                            },
                            timelock: TimelockJson {
                                blocks: timelock.blocks,
                            },
                        })
                        .collect();

                    PolicyTemplateJson {
                        keys: keys_json,
                        primary_path: primary_path_json,
                        secondary_paths: secondary_paths_json,
                    }
                }),
            };

            let payload = serde_json::to_value(EditWalletPayload {
                wallet: wallet_json,
            })
            .unwrap();
            let _ = sender.send(WssRequestMessage {
                msg_type: "edit_wallet".to_string(),
                payload,
            });
        }
    }

    fn fetch_wallet(&mut self, id: Uuid) {
        if !self.connected {
            return;
        }

        if let Some(sender) = &self.request_sender {
            let payload = serde_json::to_value(FetchWalletPayload { id: id.to_string() }).unwrap();
            let _ = sender.send(WssRequestMessage {
                msg_type: "fetch_wallet".to_string(),
                payload,
            });
        }
    }

    fn edit_xpub(&mut self, wallet_id: Uuid, xpub: Option<DescriptorPublicKey>, key_id: u8) {
        if !self.connected {
            return;
        }

        if let Some(sender) = &self.request_sender {
            let payload = serde_json::to_value(EditXpubPayload {
                wallet_id: wallet_id.to_string(),
                key_id,
                xpub: xpub.map(|x| x.to_string()),
            })
            .unwrap();
            let _ = sender.send(WssRequestMessage {
                msg_type: "edit_xpub".to_string(),
                payload,
            });
        }
    }

    fn fetch_user(&mut self, id: Uuid) {
        if !self.connected {
            return;
        }

        if let Some(sender) = &self.request_sender {
            let payload = serde_json::to_value(FetchUserPayload { id: id.to_string() }).unwrap();
            let _ = sender.send(WssRequestMessage {
                msg_type: "fetch_user".to_string(),
                payload,
            });
        }
    }
}

impl Default for WssClient {
    fn default() -> Self {
        Self::new()
    }
}
