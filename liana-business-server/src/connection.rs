use crossbeam::channel::{self, Receiver, Sender};
use liana_connect::{ConnectedPayload, Request, Response, Wallet, WssError};
use miniscript::DescriptorPublicKey;
use serde_json::json;
use std::net::TcpStream;
use std::sync::Arc;
use tungstenite::{accept, Message as WsMessage, WebSocket};
use uuid::Uuid;

use crate::auth::AuthManager;
use crate::handler::handle_request;
use crate::state::ServerState;

/// Unique identifier for each client connection
pub type ClientId = Uuid;

/// Notification message that can be broadcast to clients
#[derive(Debug, Clone)]
pub enum Notification {
    Org(Uuid),
    Wallet(Uuid),
    User(Uuid),
}

/// Manages a single client connection
pub struct ClientConnection {
    pub id: ClientId,
    notification_sender: Sender<(Response, Option<String>)>,
    connected: Arc<std::sync::atomic::AtomicBool>,
}

impl ClientConnection {
    /// Create a new client connection from a TCP stream
    pub fn new(
        stream: TcpStream,
        state: &ServerState,
        auth: Arc<AuthManager>,
        broadcast_sender: Sender<(ClientId, Notification)>,
    ) -> Result<Self, String> {
        let id = Uuid::new_v4();
        let mut ws_stream = accept(stream).map_err(|e| format!("Failed to accept WebSocket: {}", e))?;

        log::info!("New client connection: {}", id);

        // Read connect request in blocking mode
        let connect_msg = match ws_stream.read() {
            Ok(WsMessage::Text(text)) => text,
            Ok(_) => return Err("Expected text message for connect".to_string()),
            Err(e) => return Err(format!("Failed to read connect message: {}", e)),
        };

        log::info!("[new] Connect request: {}", connect_msg);

        // Parse connect request
        let protocol_request: serde_json::Value = serde_json::from_str(&connect_msg)
            .map_err(|e| format!("Failed to parse connect request: {}", e))?;

        let msg_type = protocol_request["type"].as_str().unwrap_or("");
        if msg_type != "connect" {
            return Err(format!("Expected 'connect' message, got '{}'", msg_type));
        }

        // Validate token
        let token = protocol_request["token"].as_str().unwrap_or("");
        log::info!("[new] Token: '{}'", token);
        if auth.validate_token(token).is_none() {
            log::warn!("[new] Token validation FAILED");
            let error_response = Response::Error {
                error: WssError {
                    code: "INVALID_TOKEN".to_string(),
                    message: "Invalid authentication token".to_string(),
                    request_id: protocol_request["request_id"].as_str().map(|s| s.to_string()),
                },
            };
            let ws_msg = response_to_ws_message(
                &error_response,
                protocol_request["request_id"].as_str().map(|s| s.to_string()),
            );
            let _ = ws_stream.send(ws_msg);
            return Err("Invalid token".to_string());
        }

        let request_id = protocol_request["request_id"]
            .as_str()
            .map(|s| s.to_string());

        // Respond with connected
        let connected = Response::Connected { version: 1 };
        let ws_msg = response_to_ws_message(&connected, request_id);
        ws_stream
            .send(ws_msg)
            .map_err(|e| format!("Failed to send connected response: {}", e))?;

        // Send all accessible orgs to the client
        // For now, send all orgs (WSManager sees all, filtering can be added later)
        {
            let orgs = state.orgs.lock().unwrap();
            for org in orgs.values() {
                let org_response = Response::Org { org: org.into() };
                let ws_msg = response_to_ws_message(&org_response, None);
                if let Err(e) = ws_stream.send(ws_msg) {
                    log::warn!("[new] Failed to send initial org to client: {}", e);
                }
            }
            log::info!("[new] Sent {} orgs to client {}", orgs.len(), id);
        }

        // Enable non-blocking reads after handshake
        ws_stream
            .get_ref()
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set non-blocking: {}", e))?;

        log::info!("Client {} connected successfully", id);

        // Create notification channel for this client
        // Sender is held by ClientConnection to send notifications TO the thread
        // Receiver is held by the thread to receive notifications
        let (notif_sender, notif_receiver) = channel::unbounded();
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Spawn handler thread for this client
        let state_clone = ServerState {
            orgs: Arc::clone(&state.orgs),
            wallets: Arc::clone(&state.wallets),
            users: Arc::clone(&state.users),
        };
        let auth_clone = Arc::clone(&auth);
        let client_id = id;
        let connected_clone = Arc::clone(&connected);

        std::thread::spawn(move || {
            handle_client_messages(
                client_id,
                ws_stream,
                &state_clone,
                auth_clone,
                notif_receiver,
                broadcast_sender,
                connected_clone,
            );
        });

        Ok(Self {
            id,
            notification_sender: notif_sender,
            connected,
        })
    }

    /// Create a new client connection from an already-handshaked WebSocket
    pub fn from_websocket(
        mut ws_stream: WebSocket<TcpStream>,
        state: &ServerState,
        auth: Arc<AuthManager>,
        broadcast_sender: Sender<(ClientId, Notification)>,
    ) -> Result<Self, String> {
        let id = Uuid::new_v4();

        log::info!("New client connection (from WebSocket): {}", id);

        // Read connect request in blocking mode
        let connect_msg = match ws_stream.read() {
            Ok(WsMessage::Text(text)) => text,
            Ok(_) => return Err("Expected text message for connect".to_string()),
            Err(e) => return Err(format!("Failed to read connect message: {}", e)),
        };

        log::info!("[from_ws] Connect request: {}", connect_msg);

        // Parse connect request
        let protocol_request: serde_json::Value = serde_json::from_str(&connect_msg)
            .map_err(|e| format!("Failed to parse connect request: {}", e))?;

        let msg_type = protocol_request["type"].as_str().unwrap_or("");
        if msg_type != "connect" {
            return Err(format!("Expected 'connect' message, got '{}'", msg_type));
        }

        // Validate token
        let token = protocol_request["token"].as_str().unwrap_or("");
        log::info!("[from_ws] Token: '{}'", token);
        if auth.validate_token(token).is_none() {
            log::warn!("[from_ws] Token validation FAILED");
            let error_response = Response::Error {
                error: WssError {
                    code: "INVALID_TOKEN".to_string(),
                    message: "Invalid authentication token".to_string(),
                    request_id: protocol_request["request_id"].as_str().map(|s| s.to_string()),
                },
            };
            let ws_msg = response_to_ws_message(
                &error_response,
                protocol_request["request_id"].as_str().map(|s| s.to_string()),
            );
            let _ = ws_stream.send(ws_msg);
            return Err("Invalid token".to_string());
        }

        let request_id = protocol_request["request_id"]
            .as_str()
            .map(|s| s.to_string());

        // Respond with connected
        let connected = Response::Connected { version: 1 };
        let ws_msg = response_to_ws_message(&connected, request_id);
        ws_stream
            .send(ws_msg)
            .map_err(|e| format!("Failed to send connected response: {}", e))?;

        // Send all accessible orgs to the client
        // For now, send all orgs (WSManager sees all, filtering can be added later)
        {
            let orgs = state.orgs.lock().unwrap();
            for org in orgs.values() {
                let org_response = Response::Org { org: org.into() };
                let ws_msg = response_to_ws_message(&org_response, None);
                if let Err(e) = ws_stream.send(ws_msg) {
                    log::warn!("[from_ws] Failed to send initial org to client: {}", e);
                }
            }
            log::info!("[from_ws] Sent {} orgs to client {}", orgs.len(), id);
        }

        // Enable non-blocking reads after handshake
        ws_stream
            .get_ref()
            .set_nonblocking(true)
            .map_err(|e| format!("Failed to set non-blocking: {}", e))?;

        log::info!("Client {} connected successfully", id);

        // Create notification channel for this client
        let (notif_sender, notif_receiver) = channel::unbounded();
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Spawn handler thread for this client
        let state_clone = ServerState {
            orgs: Arc::clone(&state.orgs),
            wallets: Arc::clone(&state.wallets),
            users: Arc::clone(&state.users),
        };
        let auth_clone = Arc::clone(&auth);
        let client_id = id;
        let connected_clone = Arc::clone(&connected);

        std::thread::spawn(move || {
            handle_client_messages(
                client_id,
                ws_stream,
                &state_clone,
                auth_clone,
                notif_receiver,
                broadcast_sender,
                connected_clone,
            );
        });

        Ok(Self {
            id,
            notification_sender: notif_sender,
            connected,
        })
    }

    /// Send a notification to this client
    pub fn send_notification(&mut self, response: Response, request_id: Option<String>) {
        if let Err(e) = self.notification_sender.send((response, request_id)) {
            log::error!("Failed to send notification to client {}: {}", self.id, e);
        }
    }

    /// Check if client is still connected
    pub fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Handle messages from a client
fn handle_client_messages(
    client_id: ClientId,
    mut ws_stream: WebSocket<TcpStream>,
    state: &ServerState,
    auth: Arc<AuthManager>,
    notification_receiver: Receiver<(Response, Option<String>)>,
    broadcast_sender: Sender<(ClientId, Notification)>,
    connected: Arc<std::sync::atomic::AtomicBool>,
) {
    log::debug!("Starting message handler for client {}", client_id);

    loop {
        // Check for incoming messages
        match ws_stream.read() {
            Ok(WsMessage::Text(text)) => {
                log::debug!("Client {} sent message: {}", client_id, text);

                // Parse request
                let protocol_request: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(req) => req,
                    Err(e) => {
                        log::error!("Failed to parse request from client {}: {}", client_id, e);
                        continue;
                    }
                };

                let request_id = protocol_request["request_id"]
                    .as_str()
                    .map(|s| s.to_string());

                let msg_type = protocol_request["type"].as_str().unwrap_or("");

                // Validate token for each request
                let token = protocol_request["token"].as_str().unwrap_or("");
                if auth.validate_token(token).is_none() && msg_type != "close" {
                    let error_response = Response::Error {
                        error: WssError {
                            code: "INVALID_TOKEN".to_string(),
                            message: "Invalid authentication token".to_string(),
                            request_id: request_id.clone(),
                        },
                    };
                    let ws_msg = response_to_ws_message(&error_response, request_id);
                    let _ = ws_stream.send(ws_msg);
                    continue;
                }

                // Handle special message types
                match msg_type {
                    "ping" => {
                        let pong = Response::Pong;
                        let ws_msg = response_to_ws_message(&pong, request_id);
                        if ws_stream.send(ws_msg).is_err() {
                            break;
                        }
                    }
                    "close" => {
                        log::info!("Client {} requested close", client_id);
                        let _ = ws_stream.close(None);
                        break;
                    }
                    _ => {
                        // Parse into Request enum manually based on type
                        let payload = &protocol_request["payload"];
                        let request: Request = match parse_request(msg_type, payload) {
                            Ok(req) => req,
                            Err(e) => {
                                log::error!("Failed to parse request: {}", e);
                                let error_response = Response::Error {
                                    error: WssError {
                                        code: "PROTOCOL_ERROR".to_string(),
                                        message: format!("Invalid request format: {}", e),
                                        request_id: request_id.clone(),
                                    },
                                };
                                let ws_msg = response_to_ws_message(&error_response, request_id);
                                let _ = ws_stream.send(ws_msg);
                                continue;
                            }
                        };

                        // Handle request
                        let response = handle_request(request.clone(), state);

                        // Send response to client
                        let ws_msg = response_to_ws_message(&response, request_id.clone());
                        if ws_stream.send(ws_msg).is_err() {
                            break;
                        }

                        // Determine if we need to broadcast to other clients
                        log::info!("[REQ] Processing request type: {:?}", request);
                        let notification = match &request {
                            Request::EditWallet { wallet } => {
                                log::info!("[REQ] EditWallet -> broadcasting Wallet({})", wallet.id);
                                Some(Notification::Wallet(wallet.id))
                            }
                            Request::CreateWallet { .. } => {
                                // Extract wallet ID from response
                                if let Response::Wallet { wallet } = &response {
                                    log::info!("[REQ] CreateWallet -> broadcasting Wallet({})", wallet.id);
                                    Some(Notification::Wallet(Uuid::parse_str(&wallet.id).unwrap()))
                                } else {
                                    None
                                }
                            }
                            Request::EditXpub { wallet_id, .. } => {
                                log::info!("[REQ] EditXpub -> broadcasting Wallet({})", wallet_id);
                                Some(Notification::Wallet(*wallet_id))
                            }
                            Request::RemoveWalletFromOrg { org_id, .. } => {
                                log::info!("[REQ] RemoveWalletFromOrg -> broadcasting Org({})", org_id);
                                Some(Notification::Org(*org_id))
                            }
                            _ => {
                                log::info!("[REQ] No broadcast needed for this request type");
                                None
                            }
                        };

                        if let Some(notif) = notification {
                            log::info!("[REQ] Sending broadcast notification: {:?}", notif);
                            let _ = broadcast_sender.send((client_id, notif));
                        }
                    }
                }
            }
            Ok(WsMessage::Close(_)) => {
                log::info!("Client {} disconnected", client_id);
                break;
            }
            Ok(_) => {
                // Ignore other message types (binary, ping, pong)
            }
            Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data available, sleep a bit
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => {
                log::error!("Error reading from client {}: {}", client_id, e);
                break;
            }
        }

        // Check for notifications to send
        while let Ok((response, request_id)) = notification_receiver.try_recv() {
            let ws_msg = response_to_ws_message(&response, request_id);
            if ws_stream.send(ws_msg).is_err() {
                break;
            }
        }
    }

    // Mark as disconnected
    connected.store(false, std::sync::atomic::Ordering::Relaxed);
    log::info!("Client {} handler thread exiting", client_id);
}

/// Convert Response to WebSocket message
fn response_to_ws_message(response: &Response, request_id: Option<String>) -> WsMessage {
    let (msg_type, payload, error) = match response {
        Response::Connected { version } => (
            "connected".to_string(),
            Some(serde_json::to_value(ConnectedPayload { version: *version }).unwrap()),
            None,
        ),
        Response::Pong => ("pong".to_string(), None, None),
        Response::Org { org } => (
            "org".to_string(),
            Some(serde_json::to_value(org).unwrap()),
            None,
        ),
        Response::Wallet { wallet } => (
            "wallet".to_string(),
            Some(serde_json::to_value(wallet).unwrap()),
            None,
        ),
        Response::User { user } => (
            "user".to_string(),
            Some(serde_json::to_value(user).unwrap()),
            None,
        ),
        Response::Error { error } => {
            let mut error = error.clone();
            error.request_id = request_id.clone();
            ("error".to_string(), None, Some(error))
        }
    };

    let protocol_response = json!({
        "type": msg_type,
        "request_id": request_id,
        "payload": payload,
        "error": error,
    });

    WsMessage::Text(serde_json::to_string(&protocol_response).unwrap())
}

/// Parse a Request from protocol message type and payload
fn parse_request(msg_type: &str, payload: &serde_json::Value) -> Result<Request, String> {
    match msg_type {
        "connect" => {
            let version = payload["version"].as_u64().unwrap_or(1) as u8;
            Ok(Request::Connect { version })
        }
        "ping" => Ok(Request::Ping),
        "close" => Ok(Request::Close),
        "fetch_org" => {
            let id = payload["id"]
                .as_str()
                .ok_or("Missing 'id' field")?
                .parse()
                .map_err(|e| format!("Invalid UUID: {}", e))?;
            Ok(Request::FetchOrg { id })
        }
        "fetch_wallet" => {
            let id = payload["id"]
                .as_str()
                .ok_or("Missing 'id' field")?
                .parse()
                .map_err(|e| format!("Invalid UUID: {}", e))?;
            Ok(Request::FetchWallet { id })
        }
        "fetch_user" => {
            let id = payload["id"]
                .as_str()
                .ok_or("Missing 'id' field")?
                .parse()
                .map_err(|e| format!("Invalid UUID: {}", e))?;
            Ok(Request::FetchUser { id })
        }
        "create_wallet" => {
            let name = payload["name"]
                .as_str()
                .ok_or("Missing 'name' field")?
                .to_string();
            let org_id = payload["org_id"]
                .as_str()
                .ok_or("Missing 'org_id' field")?
                .parse()
                .map_err(|e| format!("Invalid org UUID: {}", e))?;
            let owner_id = payload["owner_id"]
                .as_str()
                .ok_or("Missing 'owner_id' field")?
                .parse()
                .map_err(|e| format!("Invalid owner UUID: {}", e))?;
            Ok(Request::CreateWallet {
                name,
                org_id,
                owner_id,
            })
        }
        "edit_wallet" => {
            let wallet_json: liana_connect::WalletJson = serde_json::from_value(payload["wallet"].clone())
                .map_err(|e| format!("Invalid wallet: {}", e))?;
            let wallet: Wallet = wallet_json
                .try_into()
                .map_err(|e| format!("Failed to convert wallet: {}", e))?;
            Ok(Request::EditWallet { wallet })
        }
        "edit_xpub" => {
            let wallet_id = payload["wallet_id"]
                .as_str()
                .ok_or("Missing 'wallet_id' field")?
                .parse()
                .map_err(|e| format!("Invalid wallet UUID: {}", e))?;
            let key_id = payload["key_id"]
                .as_u64()
                .ok_or("Missing 'key_id' field")? as u8;
            let xpub: Option<DescriptorPublicKey> = if payload["xpub"].is_null() {
                None
            } else {
                let xpub_str = payload["xpub"]
                    .as_str()
                    .ok_or("Invalid 'xpub' field")?;
                Some(
                    xpub_str
                        .parse()
                        .map_err(|e| format!("Invalid xpub: {}", e))?,
                )
            };
            Ok(Request::EditXpub {
                wallet_id,
                key_id,
                xpub,
            })
        }
        "remove_wallet_from_org" => {
            let org_id = payload["org_id"]
                .as_str()
                .ok_or("Missing 'org_id' field")?
                .parse()
                .map_err(|e| format!("Invalid org UUID: {}", e))?;
            let wallet_id = payload["wallet_id"]
                .as_str()
                .ok_or("Missing 'wallet_id' field")?
                .parse()
                .map_err(|e| format!("Invalid wallet UUID: {}", e))?;
            Ok(Request::RemoveWalletFromOrg { org_id, wallet_id })
        }
        _ => Err(format!("Unknown message type: {}", msg_type)),
    }
}
