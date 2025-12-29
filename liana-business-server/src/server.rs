use crossbeam::channel::{self, Receiver, Sender};
use std::collections::HashMap;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::auth::AuthManager;
use crate::connection::{ClientConnection, ClientId, Notification};
use crate::http;
use crate::state::ServerState;
use liana_connect::Response;

/// Main server with separate HTTP and WebSocket ports
pub struct Server {
    host: String,
    auth_port: u16,
    ws_port: u16,
    state: ServerState,
    auth: Arc<AuthManager>,
}

impl Server {
    /// Create a new server
    pub fn new(
        host: &str,
        auth_port: u16,
        ws_port: u16,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            host: host.to_string(),
            auth_port,
            ws_port,
            state: ServerState::new(),
            auth: Arc::new(AuthManager::new()),
        })
    }

    /// Print server info and test user credentials
    pub fn print_tokens(&self) {
        let auth_addr = format!("http://{}:{}", self.host, self.auth_port);
        let ws_addr = format!("ws://{}:{}", self.host, self.ws_port);

        println!();
        println!("╔══════════════════════════════════════════════════════════════╗");
        println!("║            LIANA BUSINESS SERVER (Dev Mode)                  ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║ Auth API: {:<51}║", auth_addr);
        println!("║   POST /auth/v1/otp    - Request OTP                         ║");
        println!("║   POST /auth/v1/verify - Verify OTP                          ║");
        println!("║   POST /auth/v1/token  - Refresh token                       ║");
        println!("║   GET  /v1/desktop     - Service config                      ║");
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║ WebSocket: {:<50}║", ws_addr);
        println!("╠══════════════════════════════════════════════════════════════╣");
        println!("║                    TEST USER CREDENTIALS                     ║");
        println!("╠══════════════════════════════════════════════════════════════╣");

        // Collect and sort users by role for display
        let users = self.auth.get_all_users();
        let mut sorted_users: Vec<_> = users.values().collect();
        sorted_users.sort_by(|a, b| {
            // Sort by role then by email
            let role_order = |r: &liana_connect::UserRole| match r {
                liana_connect::UserRole::WSManager => 0,
                liana_connect::UserRole::Owner => 1,
                liana_connect::UserRole::Participant => 2,
            };
            role_order(&a.role)
                .cmp(&role_order(&b.role))
                .then(a.email.cmp(&b.email))
        });

        for user in sorted_users {
            let role_str = match user.role {
                liana_connect::UserRole::WSManager => "WSManager  ",
                liana_connect::UserRole::Owner => "Owner      ",
                liana_connect::UserRole::Participant => "Participant",
            };
            println!(
                "║  {} │ {:<29} │ OTP: {}   ║",
                role_str, user.email, user.otp_code
            );
        }

        println!("╚══════════════════════════════════════════════════════════════╝");
        println!();
    }

    /// Run the server (blocking)
    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Bind both listeners
        let auth_addr = format!("{}:{}", self.host, self.auth_port);
        let ws_addr = format!("{}:{}", self.host, self.ws_port);

        let auth_listener = TcpListener::bind(&auth_addr)?;
        let ws_listener = TcpListener::bind(&ws_addr)?;

        log::info!("Auth API listening on {}", auth_addr);
        log::info!("WebSocket listening on {}", ws_addr);

        // Server URL for config endpoint (points to auth port)
        // Use 127.0.0.1 instead of 0.0.0.0 for client connections
        let client_host = if self.host == "0.0.0.0" {
            "127.0.0.1"
        } else {
            &self.host
        };
        let server_url = format!("http://{}:{}", client_host, self.auth_port);

        // Channel for broadcast notifications
        #[allow(clippy::type_complexity)]
        let (broadcast_sender, broadcast_receiver): (
            Sender<(ClientId, Notification)>,
            Receiver<(ClientId, Notification)>,
        ) = channel::unbounded();

        // Connection registry
        #[allow(clippy::type_complexity)]
        let connections: Arc<Mutex<HashMap<ClientId, Sender<(Response, Option<String>)>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn broadcast handler thread
        let connections_clone = Arc::clone(&connections);
        let state_clone = ServerState {
            orgs: Arc::clone(&self.state.orgs),
            wallets: Arc::clone(&self.state.wallets),
            users: Arc::clone(&self.state.users),
        };

        thread::spawn(move || {
            handle_broadcasts(broadcast_receiver, connections_clone, state_clone);
        });

        // Spawn HTTP auth server thread
        let auth_clone = Arc::clone(&self.auth);
        let server_url_clone = server_url.clone();
        thread::spawn(move || {
            for stream in auth_listener.incoming() {
                match stream {
                    Ok(mut stream) => {
                        let auth_clone = Arc::clone(&auth_clone);
                        let server_url_clone = server_url_clone.clone();

                        thread::spawn(move || {
                            if let Some(req) = parse_http_request(&mut stream) {
                                http::handle_http_request(
                                    &mut stream,
                                    &req.method,
                                    &req.path,
                                    &req.headers,
                                    &req.body,
                                    &server_url_clone,
                                    &auth_clone,
                                );
                            }
                        });
                    }
                    Err(e) => {
                        log::error!("Error accepting HTTP connection: {}", e);
                    }
                }
            }
        });

        // WebSocket server runs on main thread
        for stream in ws_listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let state_clone = ServerState {
                        orgs: Arc::clone(&self.state.orgs),
                        wallets: Arc::clone(&self.state.wallets),
                        users: Arc::clone(&self.state.users),
                    };
                    let auth_clone = Arc::clone(&self.auth);
                    let broadcast_sender_clone = broadcast_sender.clone();
                    let connections_clone = Arc::clone(&connections);

                    thread::spawn(move || {
                        if let Some(req) = parse_http_request(&mut stream) {
                            if req.is_websocket {
                                if let Some(key) = req.websocket_key {
                                    match perform_websocket_handshake(stream, &key) {
                                        Ok(ws_stream) => {
                                            handle_websocket_connection(
                                                ws_stream,
                                                state_clone,
                                                auth_clone,
                                                broadcast_sender_clone,
                                                connections_clone,
                                            );
                                        }
                                        Err(e) => {
                                            log::error!("WebSocket handshake failed: {}", e);
                                        }
                                    }
                                } else {
                                    log::error!("WebSocket upgrade without Sec-WebSocket-Key");
                                }
                            } else {
                                log::warn!("Non-WebSocket request on WebSocket port");
                            }
                        }
                    });
                }
                Err(e) => {
                    log::error!("Error accepting WebSocket connection: {}", e);
                }
            }
        }

        Ok(())
    }
}

/// Parse HTTP request from stream, returns parsed request info
/// For WebSocket, includes the sec-websocket-key for handshake
fn parse_http_request(stream: &mut TcpStream) -> Option<ParsedRequest> {
    use std::io::Read;

    // Read headers byte by byte until we hit \r\n\r\n
    let mut header_bytes = Vec::new();
    let mut buf = [0u8; 1];

    loop {
        if stream.read_exact(&mut buf).is_err() {
            return None;
        }
        header_bytes.push(buf[0]);

        // Check for end of headers
        if header_bytes.len() >= 4 {
            let len = header_bytes.len();
            if &header_bytes[len - 4..] == b"\r\n\r\n" {
                break;
            }
        }

        // Prevent reading too much
        if header_bytes.len() > 8192 {
            return None;
        }
    }

    let header_str = String::from_utf8_lossy(&header_bytes);
    let mut lines = header_str.lines();

    // Parse request line
    let request_line = lines.next()?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }

    let method = parts[0].to_string();
    let path = parts[1].to_string();

    // Parse headers
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    let mut is_websocket = false;
    let mut websocket_key = None;

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some((name, value)) = line.split_once(':') {
            let name_lower = name.trim().to_lowercase();
            let value_trimmed = value.trim().to_string();

            if name_lower == "content-length" {
                content_length = value_trimmed.parse().unwrap_or(0);
            }
            if name_lower == "upgrade" && value_trimmed.to_lowercase() == "websocket" {
                is_websocket = true;
            }
            if name_lower == "sec-websocket-key" {
                websocket_key = Some(value_trimmed.clone());
            }

            headers.push((name_lower, value_trimmed));
        }
    }

    // Read body if present
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).ok()?;
    }

    Some(ParsedRequest {
        method,
        path,
        headers,
        body,
        is_websocket,
        websocket_key,
    })
}

struct ParsedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    is_websocket: bool,
    websocket_key: Option<String>,
}

/// Perform WebSocket handshake and return the WebSocket
fn perform_websocket_handshake(
    mut stream: TcpStream,
    websocket_key: &str,
) -> Result<tungstenite::WebSocket<TcpStream>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use sha1::{Digest, Sha1};

    // Calculate accept key per RFC 6455
    let mut hasher = Sha1::new();
    hasher.update(websocket_key.as_bytes());
    hasher.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    let accept_key = STANDARD.encode(hasher.finalize());

    // Send handshake response
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\
         \r\n",
        accept_key
    );

    stream
        .write_all(response.as_bytes())
        .map_err(|e| format!("Failed to write handshake: {}", e))?;
    stream
        .flush()
        .map_err(|e| format!("Failed to flush: {}", e))?;

    // Create WebSocket from the stream (already handshaked)
    Ok(tungstenite::WebSocket::from_raw_socket(
        stream,
        tungstenite::protocol::Role::Server,
        None,
    ))
}

/// Handle a WebSocket connection (after handshake)
#[allow(clippy::type_complexity)]
fn handle_websocket_connection(
    ws_stream: tungstenite::WebSocket<TcpStream>,
    state: ServerState,
    auth: Arc<AuthManager>,
    broadcast_sender: Sender<(ClientId, Notification)>,
    connections: Arc<Mutex<HashMap<ClientId, Sender<(Response, Option<String>)>>>>,
) {
    match ClientConnection::from_websocket(ws_stream, &state, auth, broadcast_sender) {
        Ok(mut conn) => {
            let client_id = conn.id;
            let (sender, receiver) = channel::unbounded();

            // Register connection
            {
                let mut conns = connections.lock().unwrap();
                conns.insert(client_id, sender);
            }

            log::info!("Client {} registered", client_id);

            // Wait for messages to send
            while conn.is_connected() {
                if let Ok((response, request_id)) = receiver.try_recv() {
                    conn.send_notification(response, request_id);
                }
                thread::sleep(std::time::Duration::from_millis(10));
            }

            // Unregister connection
            {
                let mut conns = connections.lock().unwrap();
                conns.remove(&client_id);
            }

            log::info!("Client {} unregistered", client_id);
        }
        Err(e) => {
            log::error!("Failed to create client connection: {}", e);
        }
    }
}

/// Handle broadcast notifications to all connected clients
#[allow(clippy::type_complexity)]
fn handle_broadcasts(
    receiver: Receiver<(ClientId, Notification)>,
    connections: Arc<Mutex<HashMap<ClientId, Sender<(Response, Option<String>)>>>>,
    state: ServerState,
) {
    log::debug!("Broadcast handler started");

    loop {
        match receiver.recv() {
            Ok((originating_client, notification)) => {
                log::info!(
                    "[BROADCAST] Received notification from client {}: {:?}",
                    originating_client,
                    notification
                );

                // Build response based on notification type
                let response = match &notification {
                    Notification::Org(org_id) => {
                        let orgs = state.orgs.lock().unwrap();
                        orgs.get(org_id)
                            .map(|org| Response::Org { org: org.into() })
                    }
                    Notification::Wallet(wallet_id) => {
                        let wallets = state.wallets.lock().unwrap();
                        wallets.get(wallet_id).map(|wallet| Response::Wallet {
                            wallet: wallet.into(),
                        })
                    }
                };

                if let Some(response) = response {
                    // Send to all clients except the originating one
                    let conns = connections.lock().unwrap();
                    log::info!(
                        "[BROADCAST] Sending to {} connected clients (excluding originator)",
                        conns.len() - 1
                    );
                    for (client_id, sender) in conns.iter() {
                        if *client_id != originating_client {
                            log::info!("[BROADCAST] Sending to client {}", client_id);
                            // Send without request_id (unsolicited notification)
                            if let Err(e) = sender.send((response.clone(), None)) {
                                log::error!(
                                    "Failed to send broadcast to client {}: {}",
                                    client_id,
                                    e
                                );
                            }
                        }
                    }
                    log::debug!("Broadcast sent to {} clients", conns.len() - 1);
                }
            }
            Err(e) => {
                log::error!("Broadcast receiver error: {}", e);
                break;
            }
        }
    }

    log::debug!("Broadcast handler stopped");
}
