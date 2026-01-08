use std::{
    collections::BTreeMap,
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::Waker,
    thread,
    time::{Duration, Instant},
};

use crossbeam::channel;
use liana_gui::{
    dir::NetworkDirectory,
    services::connect::client::{
        auth::AuthClient,
        cache::{filter_connect_cache, update_connect_cache, Account, ConnectCache},
        ServiceConfig,
    },
};
use miniscript::bitcoin::Network;
use serde_json::json;
use tracing::error;
use tungstenite::{accept, Message as WsMessage};
use uuid::Uuid;

use crate::{
    backend::{Backend, Error, Notification, Org, OrgData, User, Wallet},
    Message,
};
use liana_connect::{ConnectedPayload, OrgJson, Request, Response, UserJson, WalletJson, XpubJson};

/// HTTP URL for liana-business-server REST API (auth endpoints)
// pub const AUTH_API_URL: &str = "http://127.0.0.1:8099";
pub const AUTH_API_URL: &str = "http://54.37.41.47:8099";

/// WebSocket URL for liana-business-server
// pub const WS_URL: &str = "ws://127.0.0.1:8100";
pub const WS_URL: &str = "ws://54.37.41.47:8100";

/// Protocol version for WebSocket communication
pub const PROTOCOL_VERSION: u8 = 1;

/// Get service configuration for the local business server (blocking)
/// This replaces the production get_service_config that fetches from Liana servers
fn get_service_config_blocking(_network: Network) -> Result<ServiceConfig, reqwest::Error> {
    use tracing::debug;

    // Fetch config from our local server's /v1/desktop endpoint
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/v1/desktop", AUTH_API_URL);

    debug!("get_service_config_blocking: fetching from {}", url);
    let response = client.get(&url).send()?;
    debug!(
        "get_service_config_blocking: response status={}",
        response.status()
    );

    let res: ServiceConfig = response.json()?;
    debug!(
        "get_service_config_blocking: parsed config auth_api_url={}",
        res.auth_api_url
    );

    Ok(ServiceConfig {
        auth_api_url: res.auth_api_url,
        auth_api_public_key: res.auth_api_public_key,
        backend_api_url: AUTH_API_URL.to_string(),
    })
}

/// Shared waker type for waking the notification stream
pub type SharedWaker = Arc<Mutex<Option<Waker>>>;

/// WSS Backend implementation
#[derive(Debug)]
pub struct Client {
    pub(crate) orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>,
    pub(crate) wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    pub(crate) users: Arc<Mutex<BTreeMap<Uuid, User>>>,
    token: Arc<Mutex<Option<String>>>,
    request_sender: Option<channel::Sender<Request>>,
    notif_sender: channel::Sender<Message>,
    notif_waker: SharedWaker,
    wss_thread_handle: Option<thread::JoinHandle<()>>,
    connected: Arc<AtomicBool>,
    // Auth client integration
    auth_client: Arc<Mutex<Option<AuthClient>>>,
    network: Option<Network>,
    network_dir: Option<NetworkDirectory>,
    email: Option<String>,
}

impl Client {
    pub fn new(notif_sender: channel::Sender<Message>, notif_waker: SharedWaker) -> Self {
        Self {
            orgs: Arc::new(Mutex::new(BTreeMap::new())),
            wallets: Arc::new(Mutex::new(BTreeMap::new())),
            users: Arc::new(Mutex::new(BTreeMap::new())),
            token: Arc::new(Mutex::new(None)),
            request_sender: None,
            notif_sender,
            notif_waker,
            wss_thread_handle: None,
            connected: Arc::new(AtomicBool::new(false)),
            auth_client: Arc::new(Mutex::new(None)),
            network: None,
            network_dir: None,
            email: None,
        }
    }

    pub fn set_token(&mut self, token: String) {
        if let Ok(mut token_guard) = self.token.lock() {
            *token_guard = Some(token);
        }
    }

    pub fn set_network_dir(&mut self, network_dir: NetworkDirectory) {
        self.network_dir = Some(network_dir);
    }

    pub fn set_network(&mut self, network: Network) {
        self.network = Some(network);
    }

    /// Send a notification and wake the stream so it gets polled
    fn send_notif(
        notif_sender: &channel::Sender<Message>,
        notif_waker: &SharedWaker,
        msg: Message,
    ) {
        if notif_sender.send(msg).is_ok() {
            if let Ok(guard) = notif_waker.lock() {
                if let Some(waker) = guard.as_ref() {
                    waker.wake_by_ref();
                }
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }

    /// Try to get a cached token, refreshing it if expired
    fn try_get_cached_token(&self) -> Option<String> {
        // Check if we have network_dir and email
        let (network_dir, email) = match (self.network_dir.clone(), self.email.clone()) {
            (Some(nd), Some(e)) => (nd, e),
            _ => return None,
        };

        let network = self.network.unwrap_or(Network::Signet);
        let token = self.token.clone();

        // Get existing auth client BEFORE entering async context
        let existing_auth_client = {
            let client_guard = self.auth_client.lock().unwrap();
            client_guard.clone()
        };

        // If no existing auth client, fetch config BEFORE entering async context
        // (reqwest::blocking cannot be used inside tokio runtime)
        let fallback_auth_client = if existing_auth_client.is_none() {
            match get_service_config_blocking(network) {
                Ok(cfg) => Some(AuthClient::new(
                    cfg.auth_api_url,
                    cfg.auth_api_public_key,
                    email.clone(),
                )),
                Err(_) => None,
            }
        } else {
            None
        };

        let auth_client_for_refresh = existing_auth_client.or(fallback_auth_client);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async {
            // Try to get cached account
            match Account::from_cache(&network_dir, &email) {
                Ok(Some(account)) => {
                    let tokens = &account.tokens;
                    let now = chrono::Utc::now().timestamp();

                    // Check if token is expired (with some buffer time)
                    if tokens.expires_at > now + 60 {
                        // Token is still valid
                        Ok(tokens.access_token.clone())
                    } else {
                        // Token expired, try to refresh
                        if let Some(client) = auth_client_for_refresh {
                            match client.refresh_token(&tokens.refresh_token).await {
                                Ok(new_tokens) => {
                                    // Update cache
                                    match update_connect_cache(
                                        &network_dir,
                                        &new_tokens,
                                        &client,
                                        false,
                                    )
                                    .await
                                    {
                                        Ok(updated) => Ok(updated.access_token),
                                        Err(_) => Ok(new_tokens.access_token),
                                    }
                                }
                                Err(_) => Err(()),
                            }
                        } else {
                            // No auth client, can't refresh
                            Err(())
                        }
                    }
                }
                Ok(None) => Err(()), // No cached account
                Err(_) => Err(()),   // Error reading cache
            }
        });

        match result {
            Ok(access_token) => {
                // Store token
                if let Ok(mut token_guard) = token.lock() {
                    *token_guard = Some(access_token.clone());
                }
                Some(access_token)
            }
            Err(_) => None,
        }
    }

    /// Validate all cached tokens, returning valid accounts and emails to remove from cache.
    /// For each account in connect.json:
    /// - If token is still valid (not expired), add to valid list
    /// - If token is expired, try to refresh it
    /// - If refresh succeeds, add refreshed account to valid list
    /// - If refresh fails, add email to remove list
    pub fn validate_all_cached_tokens(
        &self,
    ) -> (Vec<crate::state::views::login::CachedAccount>, Vec<String>) {
        use crate::state::views::login::CachedAccount;

        let network_dir = match &self.network_dir {
            Some(nd) => nd.clone(),
            None => return (vec![], vec![]),
        };

        let cache = match ConnectCache::from_file(&network_dir) {
            Ok(c) => c,
            Err(_) => return (vec![], vec![]),
        };

        if cache.accounts.is_empty() {
            return (vec![], vec![]);
        }

        let network = self.network.unwrap_or(Network::Signet);
        let mut valid = vec![];
        let mut to_remove = vec![];

        // Fetch config BEFORE entering async context
        // (reqwest::blocking cannot be used inside tokio runtime)
        let config = match get_service_config_blocking(network) {
            Ok(cfg) => Some(cfg),
            Err(_) => None,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            for account in cache.accounts {
                let now = chrono::Utc::now().timestamp();

                if account.tokens.expires_at > now + 60 {
                    // Token still valid
                    valid.push(CachedAccount {
                        email: account.email,
                        tokens: account.tokens,
                    });
                } else {
                    // Token expired, try to refresh
                    let config = match &config {
                        Some(cfg) => cfg,
                        None => {
                            to_remove.push(account.email);
                            continue;
                        }
                    };

                    let auth_client = AuthClient::new(
                        config.auth_api_url.clone(),
                        config.auth_api_public_key.clone(),
                        account.email.clone(),
                    );

                    match auth_client
                        .refresh_token(&account.tokens.refresh_token)
                        .await
                    {
                        Ok(new_tokens) => {
                            // Update cache with refreshed tokens
                            let updated = match update_connect_cache(
                                &network_dir,
                                &new_tokens,
                                &auth_client,
                                false,
                            )
                            .await
                            {
                                Ok(t) => t,
                                Err(_) => new_tokens,
                            };
                            valid.push(CachedAccount {
                                email: account.email,
                                tokens: updated,
                            });
                        }
                        Err(_) => {
                            to_remove.push(account.email);
                        }
                    }
                }
            }
        });

        (valid, to_remove)
    }

    /// Remove invalid accounts from the connect cache
    pub fn clear_invalid_tokens(&self, emails_to_remove: &[String]) {
        if emails_to_remove.is_empty() {
            return;
        }

        let network_dir = match &self.network_dir {
            Some(nd) => nd.clone(),
            None => return,
        };

        // Get current valid accounts and compute emails to keep
        let valid_emails: std::collections::HashSet<String> = {
            match ConnectCache::from_file(&network_dir) {
                Ok(cache) => cache
                    .accounts
                    .into_iter()
                    .map(|a| a.email)
                    .filter(|e| !emails_to_remove.contains(e))
                    .collect(),
                Err(_) => return,
            }
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _ = filter_connect_cache(&network_dir, &valid_emails).await;
        });
    }

    /// Logout: clear token, close connection, remove auth cache, and clear data caches
    pub fn logout(&mut self) {
        // Clear token from memory
        if let Ok(mut token_guard) = self.token.lock() {
            *token_guard = None;
        }

        // Clear auth client
        if let Ok(mut auth_client_guard) = self.auth_client.lock() {
            *auth_client_guard = None;
        }

        // Close WebSocket connection
        self.close();

        // Clear org/wallet/user caches
        if let Ok(mut orgs) = self.orgs.lock() {
            orgs.clear();
        }
        if let Ok(mut wallets) = self.wallets.lock() {
            wallets.clear();
        }
        if let Ok(mut users) = self.users.lock() {
            users.clear();
        }

        // Remove auth cache from disk if network_dir and email are available
        if let (Some(network_dir), Some(email)) = (self.network_dir.clone(), self.email.clone()) {
            let network_dir_clone = network_dir.clone();
            let email_clone = email.clone();

            // Spawn thread to handle async cache removal
            thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let result = rt.block_on(async {
                    use std::collections::HashSet;

                    // Read current cache to get all account emails
                    let cache = match ConnectCache::from_file(&network_dir_clone) {
                        Ok(cache) => cache,
                        Err(_) => {
                            // Cache doesn't exist or can't be read, nothing to do
                            return Ok(());
                        }
                    };

                    // Get all emails except the current one
                    let remaining_emails: HashSet<String> = cache
                        .accounts
                        .iter()
                        .map(|a| a.email.clone())
                        .filter(|e| e != &email_clone)
                        .collect();

                    // Update cache to exclude current email
                    filter_connect_cache(&network_dir_clone, &remaining_emails).await
                });

                let _ = result;
            });
        }

        // Clear email
        self.email = None;
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
    request_receiver: channel::Receiver<Request>,
    request_sender: channel::Sender<Request>,
    notif_sender: channel::Sender<Message>,
    notif_waker: SharedWaker,
    connected: Arc<AtomicBool>,
) {
    let url = if url.starts_with("ws://") || url.starts_with("wss://") {
        url
    } else {
        format!("wss://{}", url)
    };

    let (mut ws_stream, _) = match tungstenite::connect(&url) {
        Ok(stream) => stream,
        Err(_) => {
            Client::send_notif(
                &notif_sender,
                &notif_waker,
                Notification::Error(Error::WsConnection).into(),
            );
            return;
        }
    };

    // Send connect message
    let connect_request = Request::Connect { version };
    let request_id = Uuid::new_v4().to_string();
    let msg = connect_request.to_ws_message(&token, &request_id);

    if ws_stream.send(msg).is_err() {
        Client::send_notif(
            &notif_sender,
            &notif_waker,
            Notification::Error(Error::WsConnection).into(),
        );
        return;
    }

    // we expect the server to ACK the connection
    // NOTE: .read() is still blocking at this point
    if let Ok(msg) = ws_stream.read() {
        if let Ok((Response::Connected { .. }, _)) = Response::from_ws_message(msg)
            .map_err(|e| format!("Failed to convert WSS message: {}", e))
        {
            connected.store(true, Ordering::Relaxed);
            Client::send_notif(&notif_sender, &notif_waker, Notification::Connected.into());
        } else {
            Client::send_notif(
                &notif_sender,
                &notif_waker,
                Notification::Error(Error::WsConnection).into(),
            );
            return;
        }
    } else {
        Client::send_notif(
            &notif_sender,
            &notif_waker,
            Notification::Error(Error::WsConnection).into(),
        );
        return;
    }

    // We need to enable non-blocking read now
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

    // Cache for sent requests to validate response types
    let sent_requests: Arc<Mutex<BTreeMap<String, Request>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let sent_requests2 = sent_requests.clone();
    let sent_requests3 = sent_requests.clone();

    // Ping/pong tracking state
    let last_ping = Arc::new(Mutex::new(None::<Instant>));
    let last_ping_1 = last_ping.clone();
    let last_ping_2 = last_ping.clone();
    let request_sender_for_ping = request_sender.clone();
    let connected2 = connected.clone();
    let connected3 = connected.clone();
    let notif_sender_for_timeout = notif_sender.clone();

    // Spawn ping timer thread: send ping every minute
    thread::spawn(move || {
        // Send first ping immediately after connection
        let _ = request_sender_for_ping.send(Request::Ping);
        {
            let mut ping_time = last_ping_1.lock().unwrap();
            *ping_time = Some(Instant::now());
        }

        loop {
            thread::sleep(Duration::from_secs(60));
            if !connected2.load(Ordering::Relaxed) {
                break;
            }
            // Send ping
            let _ = request_sender_for_ping.send(Request::Ping);
            // Record ping time
            {
                let mut ping_time = last_ping_1.lock().unwrap();
                *ping_time = Some(Instant::now());
            }
        }
    });

    // Spawn timeout checker thread: check if 30 seconds passed without pong
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            if !connected3.load(Ordering::Relaxed) {
                break;
            }
            let should_disconnect = {
                let ping_time = last_ping_2.lock().unwrap();
                if let Some(time) = *ping_time {
                    // If we sent a ping and 30 seconds have passed without pong, disconnect
                    time.elapsed() > Duration::from_secs(30)
                } else {
                    false
                }
            };
            if should_disconnect {
                connected3.store(false, Ordering::Relaxed);
                let _ = notif_sender_for_timeout.send(Notification::Disconnected.into());
                break;
            }
        }
    });

    loop {
        channel::select! {
            // Send to WS, we just forward the message trough the WS stream
            recv(request_receiver) -> msg => {
                match msg {
                    Ok(request) => {
                        // Handle close request specially
                        if matches!(request, Request::Close) {
                            let _ = ws_stream.close(None);
                            connected.store(false, Ordering::Relaxed);
                            break;
                        }

                        let request_id = Uuid::new_v4().to_string();
                        // Cache sent request for response validation
                        {
                            let mut requests = sent_requests2.lock().unwrap();
                            requests.insert(request_id.clone(), request.clone());
                        }
                        let ws_msg = request.to_ws_message(&token, &request_id);
                        if ws_stream.send(ws_msg).is_err() {
                            // Remove from cache on send failure
                            let mut requests = sent_requests2.lock().unwrap();
                            requests.remove(&request_id);
                            Client::send_notif(&notif_sender, &notif_waker, Notification::Error(Error::WsConnection).into());
                            error!("Failed to send WebSocket request");
                            break;
                        }
                    }
                    Err(_) => {
                        // Channel closed, exit loop
                        break;
                    }
                }
            }
            // Receive from WS
            default => {
                // NOTE: .read() is non-blocking here, as the tcp stream has be
                // configured with .setnonblocking(true)
                match ws_stream.read() {
                    Ok(WsMessage::Text(text)) => {
                        // Pass the message directly to the handler
                        let msg = WsMessage::Text(text);
                        if let Err(e) = handle_wss_message(msg, &orgs, &wallets, &users, &request_sender, &sent_requests3, &last_ping, &notif_sender, &notif_waker) {
                            // Send error notification to show warning modal
                            Client::send_notif(&notif_sender, &notif_waker, Notification::Error(Error::WsMessageHandling(e)).into());
                        }
                    }
                    // TODO: we should log errors here
                    Ok(WsMessage::Close(_)) => {
                        Client::send_notif(&notif_sender, &notif_waker, Notification::Disconnected.into());
                        break;
                    }
                    Err(tungstenite::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // Non-blocking read would block, sleep briefly to avoid spin loop
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => {
                        Client::send_notif(&notif_sender, &notif_waker, Notification::Disconnected.into());
                        break;
                    }
                    // TODO: we should log these messages at trace level
                    Ok(_) => {} // Ignore other message types
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_wss_message(
    msg: WsMessage,
    orgs: &Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_sender: &channel::Sender<Request>,
    sent_requests: &Arc<Mutex<BTreeMap<String, Request>>>,
    last_ping_time: &Arc<Mutex<Option<Instant>>>,
    n_sender: &channel::Sender<Message>,
    n_waker: &SharedWaker,
) -> Result<(), String> {
    let (response, request_id) = Response::from_ws_message(msg)
        .map_err(|e| format!("Failed to convert WSS message: {}", e))?;

    // Handle error responses first - they're always valid and we remove from cache
    if let Response::Error { error } = &response {
        if let Some(req_id) = &request_id {
            let mut requests = sent_requests.lock().unwrap();
            requests.remove(req_id);
        }
        return Err(format!("WSS error: {} - {}", error.code, error.message));
    }

    // Validate response type matches request type if request_id is present
    if let Some(req_id) = &request_id {
        let expected_response_type = {
            let requests = sent_requests.lock().unwrap();
            requests.get(req_id).map(get_expected_response_type)
        };

        if let Some(expected) = expected_response_type {
            if !matches_response_type(&response, expected) {
                // Remove from cache on mismatch
                let mut requests = sent_requests.lock().unwrap();
                requests.remove(req_id);
                return Err(format!(
                    "Response type mismatchfor {req_id}: expected {:?}, got {:?}",
                    expected, response
                ));
            }
            // Remove from cache on successful match
            let mut requests = sent_requests.lock().unwrap();
            requests.remove(req_id);
        }
    }

    match response {
        Response::Error { .. } => {
            // Already handled above, but needed for exhaustiveness
            unreachable!()
        }
        Response::Connected { version } => {
            // FIXME: we should never land here
            handle_connected(version, n_sender, n_waker)?;
        }
        Response::Pong => {
            handle_pong(last_ping_time)?;
        }
        Response::Org { org } => {
            handle_org(org, orgs, wallets, users, request_sender, n_sender, n_waker)?;
        }
        Response::Wallet { wallet } => {
            handle_wallet(wallet, wallets, users, request_sender, n_sender, n_waker)?;
        }
        Response::User { user } => {
            handle_user(user, users, n_sender, n_waker)?;
        }
        Response::ServerTime { timestamp } => {
            handle_server_time(timestamp, n_sender, n_waker)?;
        }
    }

    Ok(())
}

/// Get the expected response type for a given request
fn get_expected_response_type(request: &Request) -> ExpectedResponseType {
    match request {
        Request::Connect { .. } => ExpectedResponseType::Connected,
        Request::Ping => ExpectedResponseType::Pong,
        Request::Close => ExpectedResponseType::None, // No response expected
        Request::GetServerTime => ExpectedResponseType::ServerTime,
        Request::FetchOrg { .. } | Request::RemoveWalletFromOrg { .. } => ExpectedResponseType::Org,
        Request::CreateWallet { .. }
        | Request::EditWallet { .. }
        | Request::FetchWallet { .. }
        | Request::EditXpub { .. } => ExpectedResponseType::Wallet,
        Request::FetchUser { .. } => ExpectedResponseType::User,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ExpectedResponseType {
    Connected,
    Pong,
    ServerTime,
    Org,
    Wallet,
    User,
    None,
}

/// Check if a response matches the expected response type
fn matches_response_type(response: &Response, expected: ExpectedResponseType) -> bool {
    match (response, expected) {
        (Response::Connected { .. }, ExpectedResponseType::Connected) => true,
        (Response::Pong, ExpectedResponseType::Pong) => true,
        (Response::ServerTime { .. }, ExpectedResponseType::ServerTime) => true,
        (Response::Org { .. }, ExpectedResponseType::Org) => true,
        (Response::Wallet { .. }, ExpectedResponseType::Wallet) => true,
        (Response::User { .. }, ExpectedResponseType::User) => true,
        (Response::Error { .. }, _) => true, // Error responses are always valid
        _ => false,
    }
}

fn handle_connected(
    _version: u8,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) -> Result<(), String> {
    Client::send_notif(
        notification_sender,
        notification_waker,
        Notification::Connected.into(),
    );
    Ok(())
}

fn handle_pong(last_ping_time: &Arc<Mutex<Option<Instant>>>) -> Result<(), String> {
    // Reset ping tracking on successful pong
    {
        let mut ping_time = last_ping_time.lock().unwrap();
        *ping_time = None;
    }
    Ok(())
}

fn handle_org(
    org_json: OrgJson,
    orgs: &Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_sender: &channel::Sender<Request>,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) -> Result<(), String> {
    let org = Org::try_from(org_json)?;
    let org_id = org.id;

    // Update cache
    {
        let mut orgs_guard = orgs.lock().unwrap();
        orgs_guard.insert(org_id, org.clone());
    }

    // If any users are not cached, send fetch_user requests.
    // The responses will be handled automatically by handle_user().
    {
        let users_guard = users.lock().unwrap();
        for user_id in &org.users {
            if !users_guard.contains_key(user_id) {
                let _ = request_sender.send(Request::FetchUser { id: *user_id });
            }
        }
    }

    // If any wallets are not cached, send fetch_wallet requests.
    // The responses will be handled automatically by handle_wallet().
    {
        let wallets_guard = wallets.lock().unwrap();
        for wallet_id in &org.wallets {
            if !wallets_guard.contains_key(wallet_id) {
                let _ = request_sender.send(Request::FetchWallet { id: *wallet_id });
            }
        }
    }

    // Send response
    Client::send_notif(
        notification_sender,
        notification_waker,
        Notification::Org(org_id).into(),
    );
    Ok(())
}

fn handle_wallet(
    wallet_json: WalletJson,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_sender: &channel::Sender<Request>,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) -> Result<(), String> {
    let wallet = Wallet::try_from(wallet_json)?;
    let wallet_id = wallet.id;
    let owner_id = wallet.owner.uuid;

    // Update cache
    {
        let mut wallets_guard = wallets.lock().unwrap();
        wallets_guard.insert(wallet_id, wallet.clone());
    }

    // If the owner user is not cached, send a fetch_user request.
    // The response will be handled automatically by handle_user().
    {
        let users_guard = users.lock().unwrap();
        if !users_guard.contains_key(&owner_id) {
            let _ = request_sender.send(Request::FetchUser { id: owner_id });
        }
    }

    // Send response
    Client::send_notif(
        notification_sender,
        notification_waker,
        Notification::Wallet(wallet_id).into(),
    );
    Ok(())
}

fn handle_user(
    user_json: UserJson,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) -> Result<(), String> {
    let user = User::try_from(user_json)?;
    let user_id = user.uuid;

    // Update cache
    {
        let mut users_guard = users.lock().unwrap();
        users_guard.insert(user_id, user.clone());
    }

    // Send response
    Client::send_notif(
        notification_sender,
        notification_waker,
        Notification::User(user_id).into(),
    );
    Ok(())
}

fn handle_server_time(
    timestamp: u64,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) -> Result<(), String> {
    Client::send_notif(
        notification_sender,
        notification_waker,
        Notification::ServerTime(timestamp).into(),
    );
    Ok(())
}

macro_rules! check_connection {
    ($s: ident) => {
        if !$s.connected.load(Ordering::Relaxed) {
            Client::send_notif(
                &$s.notif_sender,
                &$s.notif_waker,
                Notification::Disconnected.into(),
            );
            return;
        }
    };
}

impl Backend for Client {
    fn connect_ws(&mut self, url: String, version: u8, notif_sender: channel::Sender<Message>) {
        // Close existing connection if any
        if self.connected.load(Ordering::Relaxed) {
            self.close();
        }

        self.notif_sender = notif_sender.clone();

        // Get token from cache or stored value
        let mut token = {
            let token_guard = self.token.lock().unwrap();
            token_guard.clone()
        };

        // If no token, try to get it from cache
        if token.is_none() {
            token = self.try_get_cached_token();
        }

        // If still no token, return channel but don't attempt WSS connection
        let token = match token {
            Some(t) => t,
            None => {
                return;
            }
        };

        let (request_sender, request_receiver) = channel::unbounded();

        let orgs = Arc::clone(&self.orgs);
        let wallets = Arc::clone(&self.wallets);
        let users = Arc::clone(&self.users);

        self.request_sender = Some(request_sender.clone());
        self.connected = Arc::new(AtomicBool::new(false));
        let connected = self.connected.clone();

        let notif_sender = notif_sender.clone();
        let notif_waker = self.notif_waker.clone();
        let handle = thread::spawn(move || {
            wss_thread(
                url,
                token,
                version,
                orgs,
                wallets,
                users,
                request_receiver,
                request_sender,
                notif_sender,
                notif_waker,
                connected,
            );
        });

        self.wss_thread_handle = Some(handle);
    }

    fn auth_request(&mut self, email: String) {
        use tracing::debug;

        // Store email for later use
        self.email = Some(email.clone());

        let notif_sender = self.notif_sender.clone();
        let notif_waker = self.notif_waker.clone();
        let network = self.network.unwrap_or(Network::Signet); // Default to signet if not set
        let email_clone = email.clone();
        let auth_client_2 = self.auth_client.clone();

        debug!("auth_request: starting for email={}", email);

        thread::spawn(move || {
            // Get service config BEFORE entering async context
            // (reqwest::blocking cannot be used inside tokio runtime)
            debug!(
                "auth_request: fetching service config for network={:?}",
                network
            );
            let config = match get_service_config_blocking(network) {
                Ok(cfg) => {
                    debug!(
                        "auth_request: got config auth_api_url={} backend_api_url={}",
                        cfg.auth_api_url, cfg.backend_api_url
                    );
                    cfg
                }
                Err(e) => {
                    debug!("auth_request: failed to get service config: {:?}", e);
                    Client::send_notif(
                        &notif_sender,
                        &notif_waker,
                        Notification::AuthCodeFail.into(),
                    );
                    return;
                }
            };

            // Create auth client
            debug!(
                "auth_request: creating AuthClient with url={} email={}",
                config.auth_api_url, email_clone
            );
            let auth_client = AuthClient::new(
                config.auth_api_url.clone(),
                config.auth_api_public_key.clone(),
                email_clone.clone(),
            );

            // Send OTP (requires async for the HTTP call)
            debug!("auth_request: sending OTP request");
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                match auth_client.sign_in_otp().await {
                    Ok(()) => {
                        debug!("auth_request: OTP sent successfully");
                        Ok(auth_client)
                    }
                    Err(e) => {
                        debug!(
                            "auth_request: OTP request failed: http_status={:?} error={}",
                            e.http_status, e.error
                        );
                        // Check if it's an invalid email error
                        if let Some(status) = e.http_status {
                            if status == 400 || status == 422 {
                                Client::send_notif(
                                    &notif_sender,
                                    &notif_waker,
                                    Notification::InvalidEmail.into(),
                                );
                            } else {
                                Client::send_notif(
                                    &notif_sender,
                                    &notif_waker,
                                    Notification::AuthCodeFail.into(),
                                );
                            }
                        } else {
                            Client::send_notif(
                                &notif_sender,
                                &notif_waker,
                                Notification::AuthCodeFail.into(),
                            );
                        }
                        Err(())
                    }
                }
            });

            // Handle result after async block
            match result {
                Ok(client) => {
                    // Store auth client
                    if let Ok(mut client_guard) = auth_client_2.lock() {
                        *client_guard = Some(client);
                    }
                    debug!("auth_request: sending AuthCodeSent notification");
                    Client::send_notif(
                        &notif_sender,
                        &notif_waker,
                        Notification::AuthCodeSent.into(),
                    );
                }
                Err(()) => {
                    // Error notification already sent in async block
                }
            }
        });
    }

    fn auth_code(&mut self, code: String) {
        let notif_sender = self.notif_sender.clone();
        let notif_waker = self.notif_waker.clone();
        let auth_client_shared = Arc::clone(&self.auth_client);
        let network_dir = self.network_dir.clone();
        let _email = self.email.clone();
        let token_shared = Arc::clone(&self.token);

        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                // Get auth client
                let auth_client = {
                    let client_guard = auth_client_shared.lock().unwrap();
                    client_guard.clone()
                };

                let auth_client = match auth_client {
                    Some(client) => client,
                    None => {
                        Client::send_notif(
                            &notif_sender,
                            &notif_waker,
                            Notification::LoginFail.into(),
                        );
                        return Err(());
                    }
                };

                // Verify OTP
                let tokens = match auth_client.verify_otp(code.trim()).await {
                    Ok(tokens) => tokens,
                    Err(_) => {
                        Client::send_notif(
                            &notif_sender,
                            &notif_waker,
                            Notification::LoginFail.into(),
                        );
                        return Err(());
                    }
                };

                // Update cache if network_dir is available
                let access_token = if let Some(ref network_dir) = network_dir {
                    match update_connect_cache(network_dir, &tokens, &auth_client, false).await {
                        Ok(updated_tokens) => updated_tokens.access_token,
                        Err(_) => {
                            // Cache update failed, but we still have tokens
                            tokens.access_token
                        }
                    }
                } else {
                    // No network_dir, just use the token
                    tokens.access_token
                };

                if let Ok(mut token_guard) = token_shared.lock() {
                    *token_guard = Some(access_token.clone());
                }

                Ok(access_token)
            });

            match result {
                Ok(_) => {
                    Client::send_notif(
                        &notif_sender,
                        &notif_waker,
                        Notification::LoginSuccess.into(),
                    );
                }
                Err(_) => {
                    // Error already handled above
                }
            }
        });
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

    fn ping(&mut self) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::Ping);
        }
    }

    fn close(&mut self) {
        if !self.connected.load(Ordering::Relaxed) {
            return;
        }

        // Send close message if possible
        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::Close);
        }

        // Wait for thread to finish
        if let Some(handle) = self.wss_thread_handle.take() {
            let _ = handle.join();
        }

        self.connected.store(false, Ordering::Relaxed);
        self.request_sender = None;
    }

    fn fetch_org(&mut self, id: Uuid) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::FetchOrg { id });
        }
    }

    fn remove_wallet_from_org(&mut self, wallet_id: Uuid, org_id: Uuid) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::RemoveWalletFromOrg { wallet_id, org_id });
        }
    }

    fn create_wallet(&mut self, name: String, org: Uuid, owner: Uuid) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::CreateWallet {
                name,
                org_id: org,
                owner_id: owner,
            });
        }
    }

    fn edit_wallet(&mut self, wallet: Wallet) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::EditWallet { wallet });
        }
    }

    fn fetch_wallet(&mut self, id: Uuid) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::FetchWallet { id });
        }
    }

    fn edit_xpub(&mut self, wallet_id: Uuid, xpub: Option<XpubJson>, key_id: u8) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::EditXpub {
                wallet_id,
                key_id,
                xpub,
            });
        }
    }

    fn fetch_user(&mut self, id: Uuid) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::FetchUser { id });
        }
    }

    fn fetch_server_time(&mut self) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::GetServerTime);
        }
    }
}

// Helper function to serialize Response to WsMessage for DummyServer
fn response_to_ws_message(response: &Response, request_id: Option<String>) -> WsMessage {
    let (msg_type, payload, error) = match response {
        Response::Connected { version } => (
            "connected".to_string(),
            Some(serde_json::to_value(ConnectedPayload { version: *version }).unwrap()),
            None,
        ),
        Response::Pong => ("pong".to_string(), None, None),
        Response::ServerTime { timestamp } => (
            "server_time".to_string(),
            Some(serde_json::json!({ "timestamp": timestamp })),
            None,
        ),
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

/// DummyServer is a WebSocket server that can handle Client connections
/// and manage Request/Response messages for development/testing
#[derive(Debug)]
pub struct DummyServer {
    port: u16,
    handle: Option<thread::JoinHandle<()>>,
    shutdown_sender: Option<channel::Sender<()>>,
    request_receiver: Option<channel::Receiver<(Request, String)>>, // Request with request_id
    response_sender: Option<channel::Sender<(Response, Option<String>)>>, // Response with optional request_id
}

impl DummyServer {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            handle: None,
            shutdown_sender: None,
            request_receiver: None,
            response_sender: None,
        }
    }

    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    pub fn start(&mut self, handler: Box<dyn Fn(Request) -> Response + Send + Sync + 'static>) {
        let port = self.port;
        let (shutdown_sender, shutdown_receiver) = channel::bounded(1);
        let (request_sender, request_receiver) = channel::unbounded();
        let (response_sender, response_receiver) = channel::unbounded();

        self.shutdown_sender = Some(shutdown_sender);
        self.request_receiver = Some(request_receiver);
        self.response_sender = Some(response_sender);

        let handle = thread::spawn(move || {
            let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
                .expect("Failed to bind to address");
            listener.set_nonblocking(false).unwrap();

            let shutdown_receiver = shutdown_receiver;
            let response_receiver = response_receiver;

            // Accept one connection
            let (stream, _) = match listener.accept() {
                Ok(conn) => conn,
                Err(_) => return,
            };

            let mut ws_stream = match accept(stream) {
                Ok(ws) => ws,
                Err(_) => return,
            };

            // Read connect request in blocking mode first
            let connect_msg = match ws_stream.read() {
                Ok(WsMessage::Text(text)) => text,
                _ => return,
            };

            // Parse connect request and respond
            let protocol_request: serde_json::Value = match serde_json::from_str(&connect_msg) {
                Ok(req) => req,
                Err(_) => return,
            };

            let request_id = protocol_request["request_id"]
                .as_str()
                .map(|s| s.to_string());

            let msg_type = protocol_request["type"].as_str().unwrap_or("");
            if msg_type == "connect" {
                // Respond with connected
                let connected = Response::Connected { version: 1 };
                let ws_msg = response_to_ws_message(&connected, request_id);
                if ws_stream.send(ws_msg).is_err() {
                    return;
                }
            } else {
                return;
            }

            // Enable non-blocking reads after initial handshake
            let tcp_stream = ws_stream.get_ref();
            tcp_stream.set_nonblocking(true).expect("must not fail");

            // Now handle subsequent messages in non-blocking mode
            loop {
                channel::select! {
                    recv(shutdown_receiver) -> _ => {
                        break;
                    }
                    recv(response_receiver) -> msg => {
                        if let Ok((response, request_id)) = msg {
                            let ws_msg = response_to_ws_message(&response, request_id);
                            if ws_stream.send(ws_msg).is_err() {
                                break;
                            }
                        }
                    }
                    default => {
                        match ws_stream.read() {
                            Ok(WsMessage::Text(text)) => {
                                // Parse request
                                let protocol_request: serde_json::Value = match serde_json::from_str(&text) {
                                    Ok(req) => req,
                                    Err(_) => continue,
                                };

                                let request_id = protocol_request["request_id"]
                                    .as_str()
                                    .map(|s| s.to_string());

                                let msg_type = protocol_request["type"]
                                    .as_str()
                                    .unwrap_or("");

                                if msg_type == "connect" {
                                    // Respond with connected
                                    let connected = Response::Connected { version: 1 };
                                    let ws_msg = response_to_ws_message(&connected, request_id);
                                    if ws_stream.send(ws_msg).is_err() {
                                        break;
                                    }
                                } else if msg_type == "ping" {
                                    // Respond with pong
                                    let pong = Response::Pong;
                                    let ws_msg = response_to_ws_message(&pong, request_id);
                                    if ws_stream.send(ws_msg).is_err() {
                                        break;
                                    }
                                } else if msg_type == "close" {
                                    let _ = ws_stream.close(None);
                                    break;
                                } else {
                                    // Convert to Request (simplified - extract type and payload)
                                    // For testing, we'll use a simple mapping for common request types
                                    // Note: protocol_request was already parsed above
                                    let request = match msg_type {
                                        "fetch_org" => {
                                            let id_str = protocol_request["payload"]["id"]
                                                .as_str()
                                                .unwrap_or("");
                                            Request::FetchOrg {
                                                id: Uuid::parse_str(id_str).unwrap_or_else(|_| Uuid::new_v4()),
                                            }
                                        }
                                        "fetch_wallet" => {
                                            let id_str = protocol_request["payload"]["id"]
                                                .as_str()
                                                .unwrap_or("");
                                            Request::FetchWallet {
                                                id: Uuid::parse_str(id_str).unwrap_or_else(|_| Uuid::new_v4()),
                                            }
                                        }
                                        "fetch_user" => {
                                            let id_str = protocol_request["payload"]["id"]
                                                .as_str()
                                                .unwrap_or("");
                                            Request::FetchUser {
                                                id: Uuid::parse_str(id_str).unwrap_or_else(|_| Uuid::new_v4()),
                                            }
                                        }
                                        "create_wallet" => {
                                            let name = protocol_request["payload"]["name"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string();
                                            let org_id_str = protocol_request["payload"]["org_id"]
                                                .as_str()
                                                .unwrap_or("");
                                            let owner_id_str = protocol_request["payload"]["owner_id"]
                                                .as_str()
                                                .unwrap_or("");
                                            Request::CreateWallet {
                                                name,
                                                org_id: Uuid::parse_str(org_id_str).unwrap_or_else(|_| Uuid::new_v4()),
                                                owner_id: Uuid::parse_str(owner_id_str).unwrap_or_else(|_| Uuid::new_v4()),
                                            }
                                        }
                                        "remove_wallet_from_org" => {
                                            let wallet_id_str = protocol_request["payload"]["wallet_id"]
                                                .as_str()
                                                .unwrap_or("");
                                            let org_id_str = protocol_request["payload"]["org_id"]
                                                .as_str()
                                                .unwrap_or("");
                                            Request::RemoveWalletFromOrg {
                                                wallet_id: Uuid::parse_str(wallet_id_str).unwrap_or_else(|_| Uuid::new_v4()),
                                                org_id: Uuid::parse_str(org_id_str).unwrap_or_else(|_| Uuid::new_v4()),
                                            }
                                        }
                                        _ => {
                                            // For unknown request types, skip (could log or send error)
                                            continue;
                                        }
                                    };

                                    let _ = request_sender.send((request.clone(), request_id.clone().unwrap_or_default()));

                                    // Get response from handler
                                    let response = handler(request);
                                    let ws_msg = response_to_ws_message(&response, request_id);
                                    if ws_stream.send(ws_msg).is_err() {
                                        break;
                                    }
                                }
                            }
                            Ok(WsMessage::Close(_)) => {
                                break;
                            }
                            Err(tungstenite::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                                // Non-blocking read would block, continue loop
                                thread::sleep(Duration::from_millis(10));
                            }
                            Err(_) => {
                                // Other errors, break
                                break;
                            }
                            Ok(_) => {}
                        }
                    }
                }
            }
        });

        self.handle = Some(handle);
    }

    pub fn send_response(&self, response: Response, request_id: Option<String>) {
        if let Some(sender) = &self.response_sender {
            let _ = sender.send((response, request_id));
        }
    }

    pub fn close(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for DummyServer {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use liana_connect::{OrgJson, Response, UserJson, WalletJson, WssConversionError, WssError};
    mod parsing_tests {
        use super::*;

        #[test]
        fn test_parse_connected_response() {
            let json = r#"{
                "type": "connected",
                "request_id": "550e8400-e29b-41d4-a716-446655440000",
                "payload": {"version": 1}
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Connected { version } => assert_eq!(version, 1),
                _ => panic!("Expected Connected response"),
            }
            assert_eq!(
                request_id,
                Some("550e8400-e29b-41d4-a716-446655440000".to_string())
            );
        }

        #[test]
        fn test_parse_connected_response_no_request_id() {
            let json = r#"{
                "type": "connected",
                "payload": {"version": 2}
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Connected { version } => assert_eq!(version, 2),
                _ => panic!("Expected Connected response"),
            }
            assert_eq!(request_id, None);
        }

        #[test]
        fn test_parse_pong_response() {
            let json = r#"{
                "type": "pong",
                "request_id": "550e8400-e29b-41d4-a716-446655440001"
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Pong => {}
                _ => panic!("Expected Pong response"),
            }
            assert_eq!(
                request_id,
                Some("550e8400-e29b-41d4-a716-446655440001".to_string())
            );
        }

        #[test]
        fn test_parse_pong_response_no_request_id() {
            let json = r#"{
                "type": "pong"
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Pong => {}
                _ => panic!("Expected Pong response"),
            }
            assert_eq!(request_id, None);
        }

        #[test]
        fn test_parse_org_response() {
            let json = r#"{
                "type": "org",
                "request_id": "550e8400-e29b-41d4-a716-446655440002",
                "payload": {
                    "name": "Acme Corp",
                    "id": "550e8400-e29b-41d4-a716-446655440010",
                    "wallets": [
                        "550e8400-e29b-41d4-a716-446655440020",
                        "550e8400-e29b-41d4-a716-446655440021"
                    ],
                    "users": [
                        "550e8400-e29b-41d4-a716-446655440030",
                        "550e8400-e29b-41d4-a716-446655440031"
                    ],
                    "owners": [
                        "550e8400-e29b-41d4-a716-446655440030"
                    ]
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Org { org } => {
                    assert_eq!(org.name, "Acme Corp");
                    assert_eq!(org.id, "550e8400-e29b-41d4-a716-446655440010");
                    assert_eq!(org.wallets.len(), 2);
                    assert_eq!(org.users.len(), 2);
                    assert_eq!(org.owners.len(), 1);
                }
                _ => panic!("Expected Org response"),
            }
            assert_eq!(
                request_id,
                Some("550e8400-e29b-41d4-a716-446655440002".to_string())
            );
        }

        #[test]
        fn test_parse_org_response_empty_arrays() {
            let json = r#"{
                "type": "org",
                "payload": {
                    "name": "Empty Org",
                    "id": "550e8400-e29b-41d4-a716-446655440011",
                    "wallets": [],
                    "users": [],
                    "owners": []
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, _) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Org { org } => {
                    assert_eq!(org.name, "Empty Org");
                    assert!(org.wallets.is_empty());
                    assert!(org.users.is_empty());
                    assert!(org.owners.is_empty());
                }
                _ => panic!("Expected Org response"),
            }
        }

        #[test]
        fn test_parse_wallet_response_with_template() {
            let json = r#"{
                "type": "wallet",
                "request_id": "550e8400-e29b-41d4-a716-446655440003",
                "payload": {
                    "id": "550e8400-e29b-41d4-a716-446655440020",
                    "alias": "Main Wallet",
                    "org": "550e8400-e29b-41d4-a716-446655440010",
                    "owner": "550e8400-e29b-41d4-a716-446655440030",
                    "owner_email": "owner@example.com",
                    "status": "Created",
                    "template": {
                        "keys": {
                            "0": {
                                "id": 0,
                                "alias": "Main Key",
                                "description": "Primary signing key",
                                "email": "key1@example.com",
                                "key_type": "Internal",
                                "xpub": null
                            },
                            "1": {
                                "id": 1,
                                "alias": "Backup Key",
                                "description": "Backup signing key",
                                "email": "key2@example.com",
                                "key_type": "External",
                                "xpub": "xpub6CniX6aWJq5LxKi"
                            }
                        },
                        "primary_path": {
                            "is_primary": true,
                            "threshold_n": 2,
                            "key_ids": [0, 1]
                        },
                        "secondary_paths": [
                            {
                                "path": {
                                    "is_primary": false,
                                    "threshold_n": 1,
                                    "key_ids": [0]
                                },
                                "timelock": {
                                    "blocks": 144
                                }
                            }
                        ]
                    }
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Wallet { wallet } => {
                    assert_eq!(wallet.id, "550e8400-e29b-41d4-a716-446655440020");
                    assert_eq!(wallet.alias, "Main Wallet");
                    assert_eq!(wallet.status_str, "Created");
                    assert!(wallet.template.is_some());
                    let template = wallet.template.as_ref().unwrap();
                    assert_eq!(template.keys.len(), 2);
                    assert_eq!(template.primary_path.threshold_n, 2);
                    assert_eq!(template.secondary_paths.len(), 1);
                }
                _ => panic!("Expected Wallet response"),
            }
            assert_eq!(
                request_id,
                Some("550e8400-e29b-41d4-a716-446655440003".to_string())
            );
        }

        #[test]
        fn test_parse_wallet_response_without_template() {
            let json = r#"{
                "type": "wallet",
                "payload": {
                    "id": "550e8400-e29b-41d4-a716-446655440021",
                    "alias": "Simple Wallet",
                    "org": "550e8400-e29b-41d4-a716-446655440010",
                    "owner": "550e8400-e29b-41d4-a716-446655440030",
                    "owner_email": "owner@example.com",
                    "status": "Drafted"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, _) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Wallet { wallet } => {
                    assert_eq!(wallet.alias, "Simple Wallet");
                    assert_eq!(wallet.status_str, "Drafted");
                    assert!(wallet.template.is_none());
                }
                _ => panic!("Expected Wallet response"),
            }
        }

        #[test]
        fn test_parse_user_response() {
            let json = r#"{
                "type": "user",
                "request_id": "550e8400-e29b-41d4-a716-446655440004",
                "payload": {
                    "name": "John Doe",
                    "uuid": "550e8400-e29b-41d4-a716-446655440030",
                    "email": "john@example.com",
                    "orgs": [
                        "550e8400-e29b-41d4-a716-446655440010",
                        "550e8400-e29b-41d4-a716-446655440011"
                    ],
                    "role": "Owner"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::User { user } => {
                    assert_eq!(user.name, "John Doe");
                    assert_eq!(user.uuid, "550e8400-e29b-41d4-a716-446655440030");
                    assert_eq!(user.email, "john@example.com");
                    assert_eq!(user.orgs.len(), 2);
                    assert_eq!(user.role_str, "Owner");
                }
                _ => panic!("Expected User response"),
            }
            assert_eq!(
                request_id,
                Some("550e8400-e29b-41d4-a716-446655440004".to_string())
            );
        }

        #[test]
        fn test_parse_user_response_empty_orgs() {
            let json = r#"{
                "type": "user",
                "payload": {
                    "name": "New User",
                    "uuid": "550e8400-e29b-41d4-a716-446655440031",
                    "email": "new@example.com",
                    "orgs": [],
                    "role": "Participant"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, _) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::User { user } => {
                    assert_eq!(user.name, "New User");
                    assert!(user.orgs.is_empty());
                    assert_eq!(user.role_str, "Participant");
                }
                _ => panic!("Expected User response"),
            }
        }

        #[test]
        fn test_parse_error_response() {
            let json = r#"{
                "type": "error",
                "request_id": "550e8400-e29b-41d4-a716-446655440005",
                "error": {
                    "code": "INVALID_REQUEST",
                    "message": "Invalid request format",
                    "request_id": "550e8400-e29b-41d4-a716-446655440005"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Error { error } => {
                    assert_eq!(error.code, "INVALID_REQUEST");
                    assert_eq!(error.message, "Invalid request format");
                    assert_eq!(
                        error.request_id,
                        Some("550e8400-e29b-41d4-a716-446655440005".to_string())
                    );
                }
                _ => panic!("Expected Error response"),
            }
            assert_eq!(
                request_id,
                Some("550e8400-e29b-41d4-a716-446655440005".to_string())
            );
        }

        #[test]
        fn test_parse_error_response_without_error_object_request_id() {
            // Test error response where request_id is at protocol level but not in error object
            // According to spec, request_id should be in error object when error is related to a request
            let json = r#"{
                "type": "error",
                "request_id": "550e8400-e29b-41d4-a716-446655440006",
                "error": {
                    "code": "SERVER_ERROR",
                    "message": "Internal server error"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Error { error } => {
                    assert_eq!(error.code, "SERVER_ERROR");
                    assert_eq!(error.message, "Internal server error");
                    // request_id may not be in error object, but should be at protocol level
                    assert_eq!(
                        request_id,
                        Some("550e8400-e29b-41d4-a716-446655440006".to_string())
                    );
                }
                _ => panic!("Expected Error response"),
            }
        }

        // Edge cases

        #[test]
        fn test_parse_invalid_message_type_binary() {
            let msg = WsMessage::Binary(vec![1, 2, 3]);
            let result = Response::from_ws_message(msg);

            assert!(matches!(
                result,
                Err(WssConversionError::InvalidMessageType)
            ));
        }

        #[test]
        fn test_parse_invalid_json() {
            let msg = WsMessage::Text("not json".to_string());
            let result = Response::from_ws_message(msg);

            assert!(matches!(
                result,
                Err(WssConversionError::DeserializationFailed(_))
            ));
        }

        #[test]
        fn test_parse_missing_type() {
            let json = r#"{
                "payload": {"version": 1}
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let result = Response::from_ws_message(msg);

            assert!(matches!(
                result,
                Err(WssConversionError::DeserializationFailed(_))
            ));
        }

        #[test]
        fn test_parse_unknown_type() {
            let json = r#"{
                "type": "unknown_type",
                "payload": {}
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let result = Response::from_ws_message(msg);

            assert!(matches!(
                result,
                Err(WssConversionError::DeserializationFailed(_))
            ));
        }

        #[test]
        fn test_parse_connected_missing_payload() {
            let json = r#"{
                "type": "connected"
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let result = Response::from_ws_message(msg);

            assert!(matches!(
                result,
                Err(WssConversionError::DeserializationFailed(_))
            ));
        }

        #[test]
        fn test_parse_org_missing_payload() {
            let json = r#"{
                "type": "org"
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let result = Response::from_ws_message(msg);

            assert!(matches!(
                result,
                Err(WssConversionError::DeserializationFailed(_))
            ));
        }

        #[test]
        fn test_parse_org_invalid_uuid() {
            let json = r#"{
                "type": "org",
                "payload": {
                    "name": "Test Org",
                    "id": "invalid-uuid",
                    "wallets": [],
                    "users": [],
                    "owners": []
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            // This should parse successfully (parsing only validates JSON structure, not UUID validity)
            // UUID validation happens in TryFrom<OrgJson> for Org
            let (response, _) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Org { org } => {
                    assert_eq!(org.id, "invalid-uuid"); // Parsing succeeds, conversion will fail
                }
                _ => panic!("Expected Org response"),
            }
        }

        #[test]
        fn test_parse_wallet_invalid_status() {
            let json = r#"{
                "type": "wallet",
                "payload": {
                    "id": "550e8400-e29b-41d4-a716-446655440020",
                    "alias": "Test Wallet",
                    "org": "550e8400-e29b-41d4-a716-446655440010",
                    "owner": "550e8400-e29b-41d4-a716-446655440030",
                    "owner_email": "test@example.com",
                    "status": "InvalidStatus"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            // Parsing succeeds, but status conversion will fail in TryFrom
            let (response, _) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Wallet { wallet } => {
                    assert_eq!(wallet.status_str, "InvalidStatus"); // Parsing succeeds
                }
                _ => panic!("Expected Wallet response"),
            }
        }

        #[test]
        fn test_parse_user_invalid_role() {
            let json = r#"{
                "type": "user",
                "payload": {
                    "name": "Test User",
                    "uuid": "550e8400-e29b-41d4-a716-446655440030",
                    "email": "test@example.com",
                    "orgs": [],
                    "role": "InvalidRole"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            // Parsing succeeds, but role conversion will fail in TryFrom
            let (response, _) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::User { user } => {
                    assert_eq!(user.role_str, "InvalidRole"); // Parsing succeeds
                }
                _ => panic!("Expected User response"),
            }
        }

        #[test]
        fn test_parse_wallet_invalid_key_type() {
            let json = r#"{
                "type": "wallet",
                "payload": {
                    "id": "550e8400-e29b-41d4-a716-446655440020",
                    "alias": "Test Wallet",
                    "org": "550e8400-e29b-41d4-a716-446655440010",
                    "owner": "550e8400-e29b-41d4-a716-446655440030",
                    "owner_email": "test@example.com",
                    "status": "Created",
                    "template": {
                        "keys": {
                            "0": {
                                "id": 0,
                                "alias": "Test Key",
                                "description": "Test",
                                "email": "test@example.com",
                                "key_type": "InvalidKeyType",
                                "xpub": null
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
            let msg = WsMessage::Text(json.to_string());
            // Parsing succeeds, but key_type conversion will fail in TryFrom
            let (response, _) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Wallet { wallet } => {
                    assert!(wallet.template.is_some());
                    let template = wallet.template.as_ref().unwrap();
                    assert_eq!(
                        template.keys.get("0").unwrap().key_type_str,
                        "InvalidKeyType"
                    ); // Parsing succeeds
                }
                _ => panic!("Expected Wallet response"),
            }
        }
    }

    mod integration_tests {
        use super::*;
        use std::time::Duration;

        fn create_test_org_json() -> OrgJson {
            OrgJson {
                name: "Test Org".to_string(),
                id: "550e8400-e29b-41d4-a716-446655440010".to_string(),
                wallets: vec!["550e8400-e29b-41d4-a716-446655440020".to_string()],
                users: vec!["550e8400-e29b-41d4-a716-446655440030".to_string()],
                owners: vec!["550e8400-e29b-41d4-a716-446655440030".to_string()],
            }
        }

        fn create_test_wallet_json() -> WalletJson {
            WalletJson {
                id: "550e8400-e29b-41d4-a716-446655440020".to_string(),
                alias: "Test Wallet".to_string(),
                org: "550e8400-e29b-41d4-a716-446655440010".to_string(),
                owner: "550e8400-e29b-41d4-a716-446655440030".to_string(),
                owner_email: "test@example.com".to_string(),
                status_str: "Created".to_string(),
                template: None,
            }
        }

        fn create_test_user_json() -> UserJson {
            UserJson {
                name: "Test User".to_string(),
                uuid: "550e8400-e29b-41d4-a716-446655440030".to_string(),
                email: "test@example.com".to_string(),
                orgs: vec!["550e8400-e29b-41d4-a716-446655440010".to_string()],
                role_str: "Owner".to_string(),
            }
        }

        #[test]
        fn test_client_connection_with_dummy_server() {
            let port = 30108;
            let mut server = DummyServer::new(port);

            let handler: Box<dyn Fn(Request) -> Response + Send + Sync> =
                Box::new(|_req| Response::Pong);

            server.start(handler);

            // Give server time to start and bind to port (server thread needs time to spawn and bind)
            thread::sleep(Duration::from_millis(300));

            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            client.set_token("test-token".to_string());
            let url = format!("ws://127.0.0.1:{}", port);
            let (sender, receiver) = channel::unbounded();
            client.connect_ws(url, 1, sender);

            // Wait for connection notification (give more time for handshake)
            for _ in 0..10 {
                thread::sleep(Duration::from_millis(100));
                let mut connected = false;
                while let Ok(notif) = receiver.try_recv() {
                    if let Message::BackendNotif(Notification::Connected) = notif {
                        connected = true
                    }
                }
                if connected && client.connected.load(Ordering::Relaxed) {
                    break;
                }
            }

            // Check for Connected notification one more time
            let mut connected_notified = false;
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::Connected) = notif {
                    connected_notified = true
                }
            }

            // Check if client is actually connected (either via notification or state)
            let is_connected = client.connected.load(Ordering::Relaxed);

            assert!(
                connected_notified || is_connected,
                "Client should have connected (notification: {}, state: {})",
                connected_notified,
                is_connected
            );

            client.close();
            server.close();
        }

        #[test]
        fn test_client_fetch_org() {
            let port = 30101;
            let mut server = DummyServer::new(port);

            let test_org = create_test_org_json();
            let handler: Box<dyn Fn(Request) -> Response + Send + Sync> =
                Box::new(move |req| match req {
                    Request::FetchOrg { .. } => Response::Org {
                        org: test_org.clone(),
                    },
                    _ => Response::Pong,
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            client.set_token("test-token".to_string());
            let url = format!("ws://127.0.0.1:{}", port);
            let (sender, receiver) = channel::unbounded();
            client.connect_ws(url, 1, sender);

            // Wait for connection
            thread::sleep(Duration::from_millis(500));
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::Connected) = notif {
                    break;
                }
            }

            // Fetch org
            let org_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440010").unwrap();
            client.fetch_org(org_id);

            // Wait for org response (give time for request/response round trip)
            thread::sleep(Duration::from_millis(1000));

            // Check cache
            let orgs = client.get_orgs();
            assert!(orgs.contains_key(&org_id), "Org should be cached");

            // Check for Org notification
            let mut org_notified = false;
            while let Ok(notif) = receiver.try_recv() {
                match notif {
                    Message::BackendNotif(Notification::Org(id)) if id == org_id => {
                        org_notified = true
                    }
                    _ => {}
                }
            }
            assert!(org_notified, "Should have received Org notification");

            client.close();
            server.close();
        }

        #[test]
        fn test_client_fetch_wallet() {
            let port = 30102;
            let mut server = DummyServer::new(port);

            let test_wallet = create_test_wallet_json();
            let handler: Box<dyn Fn(Request) -> Response + Send + Sync> =
                Box::new(move |req| match req {
                    Request::FetchWallet { .. } => Response::Wallet {
                        wallet: test_wallet.clone(),
                    },
                    _ => Response::Pong,
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            client.set_token("test-token".to_string());
            let url = format!("ws://127.0.0.1:{}", port);
            let (sender, receiver) = channel::unbounded();
            client.connect_ws(url, 1, sender);

            // Wait for connection
            thread::sleep(Duration::from_millis(500));
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::Connected) = notif {
                    break;
                }
            }

            // Fetch wallet
            let wallet_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440020").unwrap();
            client.fetch_wallet(wallet_id);

            // Wait for wallet response (give time for request/response round trip)
            thread::sleep(Duration::from_millis(1000));

            // Check cache
            let wallet = client.get_wallet(wallet_id);
            assert!(wallet.is_some(), "Wallet should be cached");
            assert_eq!(wallet.unwrap().alias, "Test Wallet");

            // Check for Wallet notification
            let mut wallet_notified = false;
            while let Ok(notif) = receiver.try_recv() {
                match notif {
                    Message::BackendNotif(Notification::Wallet(id)) if id == wallet_id => {
                        wallet_notified = true
                    }
                    _ => {}
                }
            }
            assert!(wallet_notified, "Should have received Wallet notification");

            client.close();
            server.close();
        }

        #[test]
        fn test_client_fetch_user() {
            let port = 30103;
            let mut server = DummyServer::new(port);

            let test_user = create_test_user_json();
            let handler: Box<dyn Fn(Request) -> Response + Send + Sync> =
                Box::new(move |req| match req {
                    Request::FetchUser { .. } => Response::User {
                        user: test_user.clone(),
                    },
                    _ => Response::Pong,
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            client.set_token("test-token".to_string());
            let url = format!("ws://127.0.0.1:{}", port);
            let (sender, receiver) = channel::unbounded();
            client.connect_ws(url, 1, sender);

            // Wait for connection
            thread::sleep(Duration::from_millis(500));
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::Connected) = notif {
                    break;
                }
            }

            // Fetch user
            let user_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440030").unwrap();
            client.fetch_user(user_id);

            // Wait for user response (give time for request/response round trip)
            thread::sleep(Duration::from_millis(1000));

            // Check cache
            let user = client.get_user(user_id);
            assert!(user.is_some(), "User should be cached");
            assert_eq!(user.unwrap().name, "Test User");

            // Check for User notification
            let mut user_notified = false;
            while let Ok(notif) = receiver.try_recv() {
                match notif {
                    Message::BackendNotif(Notification::User(id)) if id == user_id => {
                        user_notified = true
                    }
                    _ => {}
                }
            }
            assert!(user_notified, "Should have received User notification");

            client.close();
            server.close();
        }

        #[test]
        fn test_client_error_response() {
            let port = 30104;
            let mut server = DummyServer::new(port);

            let handler: Box<dyn Fn(Request) -> Response + Send + Sync> =
                Box::new(|_req| Response::Error {
                    error: WssError {
                        code: "TEST_ERROR".to_string(),
                        message: "Test error message".to_string(),
                        request_id: None,
                    },
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            client.set_token("test-token".to_string());
            let url = format!("ws://127.0.0.1:{}", port);
            let (sender, receiver) = channel::unbounded();
            client.connect_ws(url, 1, sender);

            // Wait for connection
            thread::sleep(Duration::from_millis(500));
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::Connected) = notif {
                    break;
                }
            }

            // Fetch org (will get error)
            let org_id = Uuid::new_v4();
            client.fetch_org(org_id);

            // Wait for error response
            thread::sleep(Duration::from_millis(500));

            // Error responses are logged but don't trigger notifications
            // The connection should still be alive
            assert!(
                client.connected.load(Ordering::Relaxed),
                "Connection should still be alive"
            );

            client.close();
            server.close();
        }

        #[test]
        fn test_client_ping_pong() {
            let port = 30105;
            let mut server = DummyServer::new(port);

            let handler: Box<dyn Fn(Request) -> Response + Send + Sync> =
                Box::new(|_req| Response::Pong);

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            client.set_token("test-token".to_string());
            let url = format!("ws://127.0.0.1:{}", port);
            let (sender, receiver) = channel::unbounded();
            client.connect_ws(url, 1, sender);

            // Wait for connection
            thread::sleep(Duration::from_millis(500));
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::Connected) = notif {
                    break;
                }
            }

            // Send ping
            client.ping();

            // Wait a bit for pong (ping is sent immediately after connection, so we wait for that)
            thread::sleep(Duration::from_millis(250));

            // Connection should still be alive (ping/pong keeps it alive)
            assert!(
                client.connected.load(Ordering::Relaxed),
                "Connection should be alive"
            );

            client.close();
            server.close();
        }

        #[test]
        fn test_client_connection_without_token() {
            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            let (sender, receiver) = channel::unbounded();
            // Don't set token
            client.connect_ws("ws://127.0.0.1:9999".to_string(), 1, sender);

            // No connection attempt, so no notifications expected
            thread::sleep(Duration::from_millis(50));

            // Client should NOT be connected (no WSS attempt was made)
            assert!(
                !client.connected.load(Ordering::Relaxed),
                "Client should not be connected without token"
            );

            // Auth methods should still work with the channel
            client.auth_request("test@example.com".to_string());

            thread::sleep(Duration::from_millis(50));

            let mut auth_code_sent = false;
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::AuthCodeSent) = notif {
                    auth_code_sent = true
                }
            }

            assert!(
                auth_code_sent,
                "Auth notifications should work without WSS connection"
            );
        }

        #[test]
        fn test_client_close() {
            let port = 30106;
            let mut server = DummyServer::new(port);

            let handler: Box<dyn Fn(Request) -> Response + Send + Sync> =
                Box::new(|_req| Response::Pong);

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            client.set_token("test-token".to_string());
            let url = format!("ws://127.0.0.1:{}", port);
            let (sender, receiver) = channel::unbounded();
            client.connect_ws(url, 1, sender);

            // Wait for connection
            thread::sleep(Duration::from_millis(500));
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::Connected) = notif {
                    break;
                }
            }

            assert!(
                client.connected.load(Ordering::Relaxed),
                "Should be connected"
            );

            // Close connection
            client.close();

            // Connection should be closed
            assert!(
                !client.connected.load(Ordering::Relaxed),
                "Should not be connected"
            );

            server.close();
        }

        #[test]
        fn test_client_get_org_data() {
            let port = 30107;
            let mut server = DummyServer::new(port);

            let test_org = create_test_org_json();
            let test_wallet = create_test_wallet_json();
            let test_user = create_test_user_json();

            let org_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440010").unwrap();
            let wallet_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440020").unwrap();
            let _user_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440030").unwrap();

            let handler: Box<dyn Fn(Request) -> Response + Send + Sync + 'static> = Box::new({
                let test_org = test_org.clone();
                let test_wallet = test_wallet.clone();
                let test_user = test_user.clone();
                move |req| match req {
                    Request::FetchOrg { .. } => Response::Org {
                        org: test_org.clone(),
                    },
                    Request::FetchWallet { .. } => Response::Wallet {
                        wallet: test_wallet.clone(),
                    },
                    Request::FetchUser { .. } => Response::User {
                        user: test_user.clone(),
                    },
                    _ => Response::Pong,
                }
            });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let mut client = Client::new(sender);
            client.set_token("test-token".to_string());
            let url = format!("ws://127.0.0.1:{}", port);
            let (sender, receiver) = channel::unbounded();
            client.connect_ws(url, 1, sender);

            // Wait for connection
            thread::sleep(Duration::from_millis(500));
            while let Ok(notif) = receiver.try_recv() {
                if let Message::BackendNotif(Notification::Connected) = notif {
                    break;
                }
            }

            // Fetch org (will trigger wallet and user fetches)
            client.fetch_org(org_id);

            // Wait for all responses (org, wallet, user)
            thread::sleep(Duration::from_millis(2000));

            // Get org data
            let org_data = client.get_org(org_id);
            assert!(org_data.is_some(), "Org data should be available");
            let org_data = org_data.unwrap();
            assert_eq!(org_data.name, "Test Org");
            assert!(
                org_data.wallets.contains_key(&wallet_id),
                "Wallet should be in org data"
            );

            client.close();
            server.close();
        }
    }
}
