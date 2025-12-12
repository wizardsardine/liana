use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam::channel;
use miniscript::DescriptorPublicKey;
use tungstenite::Message as WsMessage;
use uuid::Uuid;

use crate::backend::{Backend, Error, Notification, Org, OrgData, User, Wallet};
use crate::wss::{OrgJson, Request, Response, UserJson, WalletJson};

/// WSS Backend implementation
#[derive(Debug)]
pub struct Client {
    orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: Arc<Mutex<BTreeMap<Uuid, User>>>,
    token: Option<String>,
    request_sender: Option<channel::Sender<Request>>,
    notif_sender: Option<channel::Sender<Notification>>,
    wss_thread_handle: Option<thread::JoinHandle<()>>,
    connected: Arc<AtomicBool>,
}

impl Client {
    pub fn new() -> Self {
        Self {
            orgs: Arc::new(Mutex::new(BTreeMap::new())),
            wallets: Arc::new(Mutex::new(BTreeMap::new())),
            users: Arc::new(Mutex::new(BTreeMap::new())),
            token: None,
            request_sender: None,
            notif_sender: None,
            wss_thread_handle: None,
            connected: Arc::new(AtomicBool::new(false)),
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
    request_receiver: channel::Receiver<Request>,
    request_sender: channel::Sender<Request>,
    notif_sender: channel::Sender<Notification>,
    connected: Arc<AtomicBool>,
) {
    let url = if url.starts_with("ws://") || url.starts_with("wss://") {
        url
    } else {
        format!("wss://{}", url)
    };

    let (mut ws_stream, _) = match tungstenite::connect(&url) {
        Ok(stream) => stream,
        Err(e) => {
            let _ = notif_sender.send(Notification::Error(Error::WsConnection));
            eprintln!("Failed to connect to WSS: {}", e);
            return;
        }
    };

    // Send connect message
    let connect_request = Request::Connect { version };
    let request_id = Uuid::new_v4().to_string();
    let msg = connect_request.to_ws_message(&token, &request_id);

    if ws_stream.send(msg).is_err() {
        let _ = notif_sender.send(Notification::Error(Error::WsConnection));
        return;
    }

    // we expect the server to ACK the connection
    // NOTE: .read() is still blocking at this point
    if let Ok(msg) = ws_stream.read() {
        if let Ok((Response::Connected { .. }, _)) = Response::from_ws_message(msg)
            .map_err(|e| format!("Failed to convert WSS message: {}", e))
        {
            connected.store(true, Ordering::Relaxed);
        } else {
            let _ = notif_sender.send(Notification::Error(Error::WsConnection));
            return;
        }
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
                let _ = notif_sender_for_timeout.send(Notification::Disconnected);
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
                        if let Err(e) = ws_stream.send(ws_msg) {
                            // Remove from cache on send failure
                            let mut requests = sent_requests2.lock().unwrap();
                            requests.remove(&request_id);
                            let _ = notif_sender.send(Notification::Error(Error::WsConnection));
                            eprintln!("Failed to send request: {}", e);
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
                        if let Err(e) = handle_wss_message(msg, &orgs, &wallets, &users, &request_sender, &sent_requests3, &last_ping, &notif_sender) {
                            eprintln!("Error handling WSS message: {}", e);
                        }
                    }
                    // TODO: we should log errors here
                    Ok(WsMessage::Close(_)) | Err(_)
                    => {
                        let _ = notif_sender.send(Notification::Disconnected);
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
    n_sender: &channel::Sender<Notification>,
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
            handle_connected(version, n_sender)?;
        }
        Response::Pong => {
            handle_pong(last_ping_time)?;
        }
        Response::Org { org } => {
            handle_org(org, orgs, wallets, users, request_sender, n_sender)?;
        }
        Response::Wallet { wallet } => {
            handle_wallet(wallet, wallets, users, request_sender, n_sender)?;
        }
        Response::User { user } => {
            handle_user(user, users, n_sender)?;
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
        (Response::Org { .. }, ExpectedResponseType::Org) => true,
        (Response::Wallet { .. }, ExpectedResponseType::Wallet) => true,
        (Response::User { .. }, ExpectedResponseType::User) => true,
        (Response::Error { .. }, _) => true, // Error responses are always valid
        _ => false,
    }
}

fn handle_connected(
    _version: u8,
    notification_sender: &channel::Sender<Notification>,
) -> Result<(), String> {
    let _ = notification_sender.send(Notification::Connected);
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
    notification_sender: &channel::Sender<Notification>,
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
    let _ = notification_sender.send(Notification::Org(org_id));
    Ok(())
}

fn handle_wallet(
    wallet_json: WalletJson,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_sender: &channel::Sender<Request>,
    notification_sender: &channel::Sender<Notification>,
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
    let _ = notification_sender.send(Notification::Wallet(wallet_id));
    Ok(())
}

fn handle_user(
    user_json: UserJson,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    notification_sender: &channel::Sender<Notification>,
) -> Result<(), String> {
    let user = User::try_from(user_json)?;
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

macro_rules! check_connection {
    ($s: ident) => {
        if !$s.connected.load(Ordering::Relaxed) {
            if let Some(sender) = $s.notif_sender.as_mut() {
                let _ = sender.send(Notification::Disconnected);
            }
            return;
        }
    };
}

impl Backend for Client {
    fn connect(&mut self, url: String, version: u8) -> channel::Receiver<Notification> {
        // Close existing connection if any
        if self.connected.load(Ordering::Relaxed) {
            self.close();
        }

        // Get token - it should have been set before connect
        // If not set, create a dummy channel and return error
        let token = match self.token.clone() {
            Some(t) => t,
            None => {
                let (sender, receiver) = channel::unbounded();
                let _ = sender.send(Notification::Error(Error::TokenMissing));
                return receiver;
            }
        };

        let (request_sender, request_receiver) = channel::unbounded();
        let (notif_sender, notif_receiver) = channel::unbounded();

        let orgs = Arc::clone(&self.orgs);
        let wallets = Arc::clone(&self.wallets);
        let users = Arc::clone(&self.users);

        let notif = notif_sender.clone();
        self.request_sender = Some(request_sender.clone());
        self.connected = Arc::new(AtomicBool::new(false));
        let connected = self.connected.clone();

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
                notif,
                connected,
            );
        });

        self.notif_sender = Some(notif_sender);
        self.wss_thread_handle = Some(handle);

        notif_receiver
    }

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

    fn edit_xpub(&mut self, wallet_id: Uuid, xpub: Option<DescriptorPublicKey>, key_id: u8) {
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
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}
