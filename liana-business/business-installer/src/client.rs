#[cfg(test)]
use std::net::TcpListener;

use crate::{
    backend::{Backend, Error, Notification},
    state::Message,
};
use crossbeam::channel;
use liana_connect::ws_business::{self, Org, Request, Response, User, Wallet};
use liana_gui::{
    dir::{LianaDirectory, NetworkDirectory},
    services::connect::client::{
        auth::AuthClient,
        cache::{filter_connect_cache, update_connect_cache, Account, ConnectCache},
        ServiceConfig, ServiceConfigResource, BUSINESS_MAINNET_API_URL, BUSINESS_SIGNET_API_URL,
    },
};
use miniscript::bitcoin::Network;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::Waker,
    thread,
    time::{Duration, Instant},
};
use tracing::error;
#[cfg(test)]
use tungstenite::accept;
use tungstenite::Message as WsMessage;
use uuid::Uuid;
/// Default WebSocket URL for liana-business backend (mainnet)
const DEFAULT_MAINNET_WS_URL: &str = "wss://business.lianawallet.com/ws/v1/business/wallet/create";
/// Default WebSocket URL for liana-business backend (signet/testnet)
const DEFAULT_SIGNET_WS_URL: &str =
    "wss://business.signet.lianawallet.com/ws/v1/business/wallet/create";

/// Get AUTH API URL for the given network.
/// Environment variables can override the defaults for local testing:
/// - LIANA_BUSINESS_SIGNET_API_URL: overrides only for signet/testnet
pub fn auth_api_url(network: Network) -> String {
    // Then check network-specific override
    if network == Network::Bitcoin {
        BUSINESS_MAINNET_API_URL.to_string()
    } else {
        std::env::var("LIANA_BUSINESS_SIGNET_API_URL")
            .unwrap_or_else(|_| BUSINESS_SIGNET_API_URL.to_string())
    }
}

/// Get WebSocket URL for the given network.
/// Environment variables can override the defaults for local testing:
/// - LIANA_BUSINESS_SIGNET_WS_URL: overrides only for signet/testnet
pub fn ws_url(network: Network) -> String {
    if network == Network::Bitcoin {
        DEFAULT_MAINNET_WS_URL.to_string()
    } else {
        std::env::var("LIANA_BUSINESS_SIGNET_WS_URL")
            .unwrap_or_else(|_| DEFAULT_SIGNET_WS_URL.to_string())
    }
}

/// Protocol version for WebSocket communication
pub const PROTOCOL_VERSION: u8 = 1;

/// Get service configuration for the business server (blocking)
fn get_service_config_blocking(network: Network) -> Result<ServiceConfig, reqwest::Error> {
    use tracing::debug;

    let api_url = auth_api_url(network);
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/v1/desktop", api_url);

    debug!("get_service_config_blocking: fetching from {}", url);
    let response = client.get(&url).send()?;
    debug!(
        "get_service_config_blocking: response status={}",
        response.status()
    );

    let res: ServiceConfigResource = response.json()?;
    debug!(
        "get_service_config_blocking: parsed config auth_api_url={}",
        res.auth_api_url
    );

    Ok(ServiceConfig {
        auth_api_url: res.auth_api_url,
        auth_api_public_key: res.auth_api_public_key,
        backend_api_url: api_url,
    })
}

/// Shared waker type for waking the notification stream
pub type SharedWaker = Arc<Mutex<Option<Waker>>>;

/// WSS Backend implementation
#[derive(Debug)]
pub struct Client {
    // Cached data from backend
    pub(crate) orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>,
    pub(crate) wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    pub(crate) users: Arc<Mutex<BTreeMap<Uuid, User>>>,
    pub(crate) user_id: Arc<Mutex<Option<Uuid>>>,
    token: Arc<Mutex<Option<String>>>,
    /// Sends requests to the WSS thread; None when disconnected.
    request_sender: Option<channel::Sender<Request>>,
    notif_sender: channel::Sender<Message>,
    notif_waker: SharedWaker,
    wss_thread_handle: Option<thread::JoinHandle<()>>,
    connected: Arc<AtomicBool>,
    /// Temporarily holds the AuthClient between `auth_request()` and `auth_code()` calls.
    auth_client: Arc<Mutex<Option<AuthClient>>>,
    network: Option<Network>,
    network_dir: Option<NetworkDirectory>,
    email: Option<String>,
    /// Background thread that periodically refreshes the access token.
    refresh_thread_handle: Option<thread::JoinHandle<()>>,
    /// Signal to stop the refresh thread on disconnect.
    refresh_stop: Arc<AtomicBool>,
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
            refresh_thread_handle: None,
            refresh_stop: Arc::new(AtomicBool::new(false)),
            user_id: Arc::new(Mutex::new(None)),
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

    pub fn set_network(&mut self, network: Network, datadir: LianaDirectory) {
        self.network = Some(network);
        // Set network directory for token caching (same location as liana-gui)
        let network_dir = datadir.network_directory(network);
        self.set_network_dir(network_dir);
    }

    /// Send a notification and wake the stream so it gets polled
    fn send_notif(
        notif_sender: &channel::Sender<Message>,
        notif_waker: &SharedWaker,
        msg: Message,
    ) {
        tracing::debug!("send_notif: sending {:?}", msg);
        match notif_sender.send(msg) {
            Ok(()) => {
                tracing::debug!("send_notif: sent to channel successfully");
                if let Ok(guard) = notif_waker.lock() {
                    if let Some(waker) = guard.as_ref() {
                        tracing::debug!("send_notif: waking stream");
                        waker.wake_by_ref();
                    } else {
                        tracing::debug!("send_notif: no waker available");
                    }
                } else {
                    tracing::debug!("send_notif: failed to lock waker");
                }
            }
            Err(e) => {
                tracing::debug!("send_notif: failed to send: {:?}", e);
            }
        }
    }

    /// Start the background token refresh thread.
    /// The thread checks token expiry every 60 seconds and refreshes if needed.
    pub fn start_token_refresh_thread(&mut self) {
        // Stop any existing refresh thread
        self.stop_token_refresh_thread();

        let (network_dir, email) = match (self.network_dir.clone(), self.email.clone()) {
            (Some(nd), Some(e)) => (nd, e),
            _ => return,
        };

        let network = self.network.unwrap_or(Network::Signet);
        let token_arc = self.token.clone();
        let stop_flag = self.refresh_stop.clone();

        // Get auth client config before spawning thread
        let config = match get_service_config_blocking(network) {
            Ok(cfg) => cfg,
            Err(_) => return,
        };

        // Reset stop flag
        stop_flag.store(false, Ordering::Relaxed);

        let handle = thread::spawn(move || {
            loop {
                // Sleep for 60 seconds, checking stop flag periodically
                for _ in 0..60 {
                    if stop_flag.load(Ordering::Relaxed) {
                        tracing::debug!("Token refresh thread stopping");
                        return;
                    }
                    thread::sleep(std::time::Duration::from_secs(1));
                }

                // Check and refresh token
                let result = futures::executor::block_on(async {
                    let account = match Account::from_cache(&network_dir, &email) {
                        Ok(Some(acc)) => acc,
                        _ => return false,
                    };

                    let tokens = &account.tokens;
                    let now = chrono::Utc::now().timestamp();

                    // Refresh if token expires within 5 minutes (300 seconds)
                    const REFRESH_THRESHOLD_SECS: i64 = 300;

                    if tokens.expires_at > now + REFRESH_THRESHOLD_SECS {
                        return false; // No refresh needed
                    }

                    tracing::debug!(
                        "Token expires in {} seconds, refreshing proactively",
                        tokens.expires_at - now
                    );

                    let auth_client = AuthClient::new(
                        config.auth_api_url.clone(),
                        config.auth_api_public_key.clone(),
                        email.clone(),
                    );

                    match auth_client.refresh_token(&tokens.refresh_token).await {
                        Ok(new_tokens) => {
                            let final_tokens = match update_connect_cache(
                                &network_dir,
                                &new_tokens,
                                &auth_client,
                                false,
                            )
                            .await
                            {
                                Ok(updated) => updated,
                                Err(_) => new_tokens,
                            };

                            if let Ok(mut token_guard) = token_arc.lock() {
                                *token_guard = Some(final_tokens.access_token);
                            }

                            tracing::info!("Token refreshed successfully");
                            true
                        }
                        Err(e) => {
                            tracing::warn!("Failed to refresh token: {:?}", e);
                            false
                        }
                    }
                });

                if !result {
                    tracing::trace!("Token refresh check: no refresh needed");
                }
            }
        });

        self.refresh_thread_handle = Some(handle);
        tracing::debug!("Token refresh thread started");
    }

    /// Stop the background token refresh thread.
    pub fn stop_token_refresh_thread(&mut self) {
        self.refresh_stop.store(true, Ordering::Relaxed);
        // Don't join - let it die on its own to avoid blocking the GUI
        self.refresh_thread_handle.take();
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

        tracing::debug!("validate_all_cached_tokens: starting validation");

        let network_dir = match &self.network_dir {
            Some(nd) => nd.clone(),
            None => {
                tracing::debug!("validate_all_cached_tokens: no network_dir configured");
                return (vec![], vec![]);
            }
        };

        let cache = match ConnectCache::from_file(&network_dir) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("validate_all_cached_tokens: failed to read cache: {:?}", e);
                return (vec![], vec![]);
            }
        };

        if cache.accounts.is_empty() {
            tracing::debug!("validate_all_cached_tokens: no cached accounts");
            return (vec![], vec![]);
        }

        tracing::debug!(
            "validate_all_cached_tokens: validating {} accounts",
            cache.accounts.len()
        );

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
                    tracing::debug!(
                        "validate_all_cached_tokens: token valid for email={}",
                        account.email
                    );
                    valid.push(CachedAccount {
                        email: account.email,
                        tokens: account.tokens,
                    });
                } else {
                    // Token expired, try to refresh
                    tracing::debug!(
                        "validate_all_cached_tokens: token expired for email={}, attempting refresh",
                        account.email
                    );
                    let config = match &config {
                        Some(cfg) => cfg,
                        None => {
                            tracing::warn!(
                                "validate_all_cached_tokens: no config available, removing account email={}",
                                account.email
                            );
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
                            tracing::debug!(
                                "validate_all_cached_tokens: token refreshed for email={}",
                                account.email
                            );
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
                        Err(e) => {
                            tracing::warn!(
                                "validate_all_cached_tokens: token refresh failed for email={}: {:?}",
                                account.email,
                                e
                            );
                            to_remove.push(account.email);
                        }
                    }
                }
            }
        });

        tracing::debug!(
            "validate_all_cached_tokens: completed, valid={} to_remove={}",
            valid.len(),
            to_remove.len()
        );

        (valid, to_remove)
    }

    /// Remove invalid accounts from the connect cache
    pub fn clear_invalid_tokens(&self, emails_to_remove: &[String]) {
        if emails_to_remove.is_empty() {
            return;
        }

        tracing::debug!(
            "clear_invalid_tokens: removing {} invalid tokens",
            emails_to_remove.len()
        );

        let network_dir = match &self.network_dir {
            Some(nd) => nd.clone(),
            None => {
                tracing::debug!("clear_invalid_tokens: no network_dir configured");
                return;
            }
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
                Err(e) => {
                    tracing::debug!("clear_invalid_tokens: failed to read cache: {:?}", e);
                    return;
                }
            }
        };

        tracing::debug!(
            "clear_invalid_tokens: keeping {} valid accounts",
            valid_emails.len()
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let _ = filter_connect_cache(&network_dir, &valid_emails).await;
        });

        tracing::debug!("clear_invalid_tokens: cache updated");
    }

    /// Logout: clear token, close connection, remove auth cache, and clear data caches
    pub fn logout(&mut self) {
        tracing::info!("logout: logging out user");

        // Stop token refresh thread
        tracing::debug!("logout: stopping token refresh thread");
        self.stop_token_refresh_thread();

        // Clear token from memory
        tracing::debug!("logout: clearing token from memory");
        if let Ok(mut token_guard) = self.token.lock() {
            *token_guard = None;
        }

        // Clear auth client
        tracing::debug!("logout: clearing auth client");
        if let Ok(mut auth_client_guard) = self.auth_client.lock() {
            *auth_client_guard = None;
        }

        // Close WebSocket connection
        tracing::debug!("logout: closing WebSocket connection");
        self.close();

        // Clear org/wallet/user caches
        tracing::debug!("logout: clearing data caches");
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
            tracing::debug!("logout: removing auth cache from disk for email={}", email);
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

        tracing::info!("logout: logout complete");
    }
}

/// Data needed to retrieve a cached token in the WSS thread
pub struct TokenRetrievalData {
    pub token: Arc<Mutex<Option<String>>>,
    pub network_dir: Option<NetworkDirectory>,
    pub email: Option<String>,
    pub network: Option<Network>,
    pub auth_client: Arc<Mutex<Option<AuthClient>>>,
}

/// Try to get a cached token, refreshing it if expired.
// NOTE: this function is blocking
fn try_get_cached_token(data: &TokenRetrievalData) -> Option<String> {
    tracing::debug!("try_get_cached_token: checking for existing token");

    // First check if we already have a token
    if let Ok(token_guard) = data.token.lock() {
        if let Some(t) = token_guard.as_ref() {
            tracing::debug!("try_get_cached_token: token already in memory");
            return Some(t.clone());
        }
    }

    // Check if we have network_dir and email
    let (network_dir, email) = match (data.network_dir.clone(), data.email.clone()) {
        (Some(nd), Some(e)) => (nd, e),
        _ => {
            tracing::debug!("try_get_cached_token: missing network_dir or email");
            return None;
        }
    };

    let network = data.network.unwrap_or(Network::Signet);

    // Get existing auth client
    let existing_auth_client = {
        let client_guard = data.auth_client.lock().expect("poisoned");
        client_guard.clone()
    };

    // If no existing auth client, fetch config
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

    tracing::debug!("try_get_cached_token: checking cache for email={}", email);

    let result = futures::executor::block_on(async {
        // Try to get cached account
        match Account::from_cache(&network_dir, &email) {
            Ok(Some(account)) => {
                let tokens = &account.tokens;
                let now = chrono::Utc::now().timestamp();

                // Check if token is expired (with some buffer time)
                if tokens.expires_at > now + 60 {
                    // Token is still valid
                    tracing::debug!(
                        "try_get_cached_token: token valid, expires in {} seconds",
                        tokens.expires_at - now
                    );
                    Ok(tokens.access_token.clone())
                } else {
                    // Token expired, try to refresh
                    tracing::debug!(
                        "try_get_cached_token: token expired {} seconds ago, attempting refresh",
                        now - tokens.expires_at
                    );
                    if let Some(client) = auth_client_for_refresh {
                        match client.refresh_token(&tokens.refresh_token).await {
                            Ok(new_tokens) => {
                                tracing::debug!("try_get_cached_token: token refreshed successfully");
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
                            Err(e) => {
                                tracing::warn!("try_get_cached_token: token refresh failed: {:?}", e);
                                Err(())
                            }
                        }
                    } else {
                        // No auth client, can't refresh
                        tracing::debug!("try_get_cached_token: no auth client available for refresh");
                        Err(())
                    }
                }
            }
            Ok(None) => {
                tracing::debug!("try_get_cached_token: no cached account found");
                Err(())
            }
            Err(e) => {
                tracing::debug!("try_get_cached_token: error reading cache: {:?}", e);
                Err(())
            }
        }
    });

    match result {
        Ok(access_token) => {
            // Store token
            if let Ok(mut token_guard) = data.token.lock() {
                *token_guard = Some(access_token.clone());
            }
            Some(access_token)
        }
        Err(_) => None,
    }
}

/// Stack size for lightweight polling threads (16KB)
const POLLING_THREAD_STACK_SIZE: usize = 16 * 1024;

/// Spawn ping timer thread: send ping every minute
fn ping_thread(
    request_sender: channel::Sender<Request>,
    connected: Arc<AtomicBool>,
    last_ping: Arc<Mutex<Option<Instant>>>,
) {
    thread::Builder::new()
        .name("ping".into())
        .stack_size(POLLING_THREAD_STACK_SIZE)
        .spawn(move || {
            tracing::debug!("ping_thread: started");

            // Send first ping immediately after connection
            tracing::debug!("ping_thread: sending initial ping");
            let _ = request_sender.send(Request::Ping);
            {
                let mut ping_time = last_ping.lock().expect("poisoned");
                *ping_time = Some(Instant::now());
            }

            loop {
                thread::sleep(Duration::from_secs(60));
                if !connected.load(Ordering::Relaxed) {
                    tracing::debug!("ping_thread: connection closed, stopping");
                    break;
                }
                // Send ping
                tracing::debug!("ping_thread: sending ping");
                let _ = request_sender.send(Request::Ping);
                // Record ping time
                {
                    let mut ping_time = last_ping.lock().expect("poisoned");
                    *ping_time = Some(Instant::now());
                }
            }
        })
        .expect("failed to spawn ping thread");
}

/// Spawn timeout checker thread: check if 30 seconds passed without pong
fn ping_timeout_checker_thread(
    notif_sender: channel::Sender<Message>,
    connected: Arc<AtomicBool>,
    last_ping: Arc<Mutex<Option<Instant>>>,
) {
    thread::Builder::new()
        .name("ping-timeout".into())
        .stack_size(POLLING_THREAD_STACK_SIZE)
        .spawn(move || {
            tracing::debug!("ping_timeout_checker_thread: started");

            loop {
                thread::sleep(Duration::from_secs(1));
                if !connected.load(Ordering::Relaxed) {
                    tracing::debug!("ping_timeout_checker_thread: connection closed, stopping");
                    break;
                }
                let should_disconnect = {
                    let ping_time = last_ping.lock().expect("poisoned");
                    if let Some(time) = *ping_time {
                        // If we sent a ping and 30 seconds have passed without pong, disconnect
                        time.elapsed() > Duration::from_secs(30)
                    } else {
                        false
                    }
                };
                if should_disconnect {
                    tracing::warn!(
                        "ping_timeout_checker_thread: ping timeout (30s without pong), disconnecting"
                    );
                    connected.store(false, Ordering::Relaxed);
                    let _ = notif_sender.send(Notification::Disconnected.into());
                    break;
                }
            }
        })
        .expect("failed to spawn ping timeout thread");
}

/// Spawn timeout checker thread: check for stale requests and re-send them.
/// After 3 retries, abort and notify GUI.
#[allow(clippy::type_complexity)]
fn request_timeout_checker_thread(
    request_sender: channel::Sender<Request>,
    connected: Arc<AtomicBool>,
    sent_requests: Arc<Mutex<BTreeMap<Uuid, (Request, Instant, u8)>>>,
    notif_sender: channel::Sender<Message>,
    notif_waker: SharedWaker,
) {
    thread::Builder::new()
        .name("request-timeout".into())
        .stack_size(POLLING_THREAD_STACK_SIZE)
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(5));
                if !connected.load(Ordering::Relaxed) {
                    break;
                }
                let stale: Vec<(Uuid, Request, u8)> = {
                    let requests = sent_requests.lock().expect("poisoned");
                    requests
                        .iter()
                        .filter(|(_, (_, time, _))| time.elapsed() > Duration::from_secs(30))
                        .map(|(id, (req, _, count))| (*id, req.clone(), *count))
                        .collect()
                };
                for (id, request, count) in stale {
                    if count >= 3 {
                        // Max retries exceeded - abort
                        tracing::error!("Request {:?} timed out after 3 retries, aborting", id);
                        {
                            let mut requests = sent_requests.lock().expect("poisoned");
                            requests.remove(&id);
                        }
                        Client::send_notif(
                            &notif_sender,
                            &notif_waker,
                            Notification::Error(Error::RequestTimeout).into(),
                        );
                    } else {
                        // Retry
                        tracing::warn!(
                            "Re-sending timed out request {:?} (attempt {})",
                            id,
                            count + 1
                        );
                        {
                            let mut requests = sent_requests.lock().expect("poisoned");
                            if let Some((_, time, retry_count)) = requests.get_mut(&id) {
                                *time = Instant::now();
                                *retry_count += 1;
                            }
                        }
                        let _ = request_sender.send(request);
                    }
                }
            }
        })
        .expect("failed to spawn request timeout thread");
}

// WSS thread function
#[allow(clippy::too_many_arguments)]
fn wss_thread(
    url: String,
    token_data: TokenRetrievalData,
    version: u8,
    orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: Arc<Mutex<BTreeMap<Uuid, User>>>,
    user_id: Arc<Mutex<Option<Uuid>>>,
    request_receiver: channel::Receiver<Request>,
    request_sender: channel::Sender<Request>,
    notif_sender: channel::Sender<Message>,
    notif_waker: SharedWaker,
    connected: Arc<AtomicBool>,
) {
    tracing::debug!("wss_thread: starting, url={}", url);

    // Get token (this may involve network calls to refresh, but we're in a background thread)
    let token = match try_get_cached_token(&token_data) {
        Some(t) => {
            tracing::debug!("wss_thread: token retrieved successfully");
            t
        }
        None => {
            tracing::error!("wss_thread: failed to get token");
            Client::send_notif(
                &notif_sender,
                &notif_waker,
                Notification::Error(Error::TokenMissing).into(),
            );
            return;
        }
    };

    let url = if url.starts_with("ws://") || url.starts_with("wss://") {
        url
    } else {
        format!("wss://{}", url)
    };

    tracing::debug!("wss_thread: connecting to {}", url);

    let (mut ws_stream, _) = match tungstenite::connect(&url) {
        Ok(stream) => {
            tracing::debug!("wss_thread: WebSocket connection established");
            stream
        }
        Err(e) => {
            tracing::error!("wss_thread: WebSocket connection failed: {:?}", e);
            Client::send_notif(
                &notif_sender,
                &notif_waker,
                Notification::Error(Error::WsConnection).into(),
            );
            return;
        }
    };

    // Send connect message
    tracing::debug!("wss_thread: sending connect message with version={}", version);
    let (msg, _id) = Request::Connect { version }.to_ws_message(&token);
    if ws_stream.send(msg).is_err() {
        tracing::error!("wss_thread: failed to send connect message");
        Client::send_notif(
            &notif_sender,
            &notif_waker,
            Notification::Error(Error::WsConnection).into(),
        );
        return;
    }

    // Set read timeout for the handshake (avoid blocking forever if server doesn't respond)
    match ws_stream.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
        }
        tungstenite::stream::MaybeTlsStream::Rustls(s) => {
            let _ = s.get_ref().set_read_timeout(Some(Duration::from_secs(30)));
        }
        _ => {}
    }

    // we expect the server to ACK the connection w/ a Response::Connected
    tracing::debug!("wss_thread: waiting for Connected response");
    if let Ok(msg) = ws_stream.read() {
        match Response::from_ws_message(msg) {
            Ok((Response::Connected { user, .. }, _)) => {
                tracing::info!("wss_thread: connected successfully, user_id={}", user);
                // Store the authenticated user's ID
                if let Ok(mut id) = user_id.lock() {
                    *id = Some(user);
                }
                connected.store(true, Ordering::Relaxed);
                Client::send_notif(&notif_sender, &notif_waker, Notification::Connected.into());
            }
            Ok((response, _)) => {
                // NOTE: Handshake fails if we receive anything other than Response::Connected
                tracing::error!(
                    "wss_thread: handshake failed, expected Connected, got {:?}",
                    response
                );
                Client::send_notif(
                    &notif_sender,
                    &notif_waker,
                    Notification::Error(Error::WsConnection).into(),
                );
                return;
            }
            Err(e) => {
                tracing::error!("wss_thread: handshake failed, parse error: {:?}", e);
                Client::send_notif(
                    &notif_sender,
                    &notif_waker,
                    Notification::Error(Error::WsConnection).into(),
                );
                return;
            }
        }
    } else {
        tracing::error!("wss_thread: handshake failed, no response from server");
        Client::send_notif(
            &notif_sender,
            &notif_waker,
            Notification::Error(Error::WsConnection).into(),
        );
        return;
    }

    // Clear read timeout and enable non-blocking mode
    match ws_stream.get_ref() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => {
            stream.set_read_timeout(None).expect("must not fail");
            stream.set_nonblocking(true).expect("must not fail");
        }
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            stream
                .get_ref()
                .set_read_timeout(None)
                .expect("must not fail");
            stream
                .get_ref()
                .set_nonblocking(true)
                .expect("must not fail");
        }
        _ => unreachable!("NativeTls not enabled"),
    }

    // Cache for sent requests to validate response types: (Request, sent_time, retry_count)
    #[allow(clippy::type_complexity)]
    let sent_requests: Arc<Mutex<BTreeMap<Uuid, (Request, Instant, u8)>>> =
        Arc::new(Mutex::new(BTreeMap::new()));
    let sent_requests2 = sent_requests.clone();
    let sent_requests3 = sent_requests.clone();

    // Spawn ping thread
    let last_ping = Arc::new(Mutex::new(None::<Instant>));
    ping_thread(request_sender.clone(), connected.clone(), last_ping.clone());
    ping_timeout_checker_thread(notif_sender.clone(), connected.clone(), last_ping.clone());

    // Spawn request timeout checker thread
    request_timeout_checker_thread(
        request_sender.clone(),
        connected.clone(),
        sent_requests.clone(),
        notif_sender.clone(),
        notif_waker.clone(),
    );

    tracing::debug!("wss_thread: entering main message loop");

    loop {
        channel::select! {
            recv(request_receiver) -> rq => {
                match rq {
                    Ok(request) => {
                        // Handle close request specially
                        if matches!(request, Request::Close) {
                            tracing::debug!("wss_thread: received close request");
                            let _ = ws_stream.close(None);
                            connected.store(false, Ordering::Relaxed);
                            break;
                        }

                        tracing::debug!("wss_thread: sending request {:?}", request);
                        let (ws_msg, request_id) = request.to_ws_message(&token);
                        // Cache sent request for response validation
                        {
                            let mut requests = sent_requests2.lock().expect("poisoned");
                            requests.insert(request_id, (request.clone(), Instant::now(), 0));
                        }
                        if ws_stream.send(ws_msg).is_err() {
                            // Remove from cache on send failure
                            let mut requests = sent_requests2.lock().expect("poisoned");
                            requests.remove(&request_id);
                            Client::send_notif(&notif_sender, &notif_waker, Notification::Error(Error::WsConnection).into());
                            error!("wss_thread: failed to send WebSocket request");
                            break;
                        }
                    }
                    Err(_) => {
                        // Channel closed, exit loop
                        tracing::debug!("wss_thread: request channel closed, exiting");
                        break;
                    }
                }
            }
            // Receive from WS
            default => {
                // NOTE: .read() is non-blocking here, as the tcp stream has be
                //      configured with .setnonblocking(true)
                match ws_stream.read() {
                    Ok(WsMessage::Text(text)) => {
                        tracing::debug!("wss_thread: received text message");
                        // Pass the message directly to the handler
                        let msg = WsMessage::Text(text);
                        if let Err(e) = handle_wss_message(
                            msg,
                            &orgs,
                            &wallets,
                            &users,
                            &user_id,
                            &request_sender,
                            &sent_requests3,
                            &last_ping,
                            &notif_sender,
                            &notif_waker
                        ) {
                            // Send error notification to show warning modal
                            Client::send_notif(&notif_sender, &notif_waker, Notification::Error(Error::WsMessageHandling(e)).into());
                        }
                    }
                    Ok(WsMessage::Close(_)) => {
                        tracing::info!("wss_thread: WebSocket connection closed by server");
                        Client::send_notif(&notif_sender, &notif_waker, Notification::Disconnected.into());
                        break;
                    }
                    Err(tungstenite::Error::Io(ref e)) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // Non-blocking read would block, sleep briefly to avoid spin loop
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        tracing::warn!("wss_thread: WebSocket error: {:?}", e);
                        Client::send_notif(&notif_sender, &notif_waker, Notification::Disconnected.into());
                        break;
                    }
                    Ok(m) => {
                        // Ignore other message types
                        tracing::debug!("wss_thread: received unexpected message type: {}", m);
                    }
                }
            }
        }
    }

    tracing::debug!("wss_thread: exiting");
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
fn handle_wss_message(
    msg: WsMessage,
    orgs: &Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    user_id: &Arc<Mutex<Option<Uuid>>>,
    request_sender: &channel::Sender<Request>,
    sent_requests: &Arc<Mutex<BTreeMap<Uuid, (Request, Instant, u8)>>>,
    last_ping_time: &Arc<Mutex<Option<Instant>>>,
    n_sender: &channel::Sender<Message>,
    n_waker: &SharedWaker,
) -> Result<(), String> {
    let (response, request_id) = Response::from_ws_message(msg)
        .map_err(|e| format!("Failed to convert WSS message: {}", e))?;
    let request_id = request_id.and_then(|s| Uuid::try_parse(&s).ok());

    tracing::debug!(
        "handle_wss_message: received response type={:?} request_id={:?}",
        std::mem::discriminant(&response),
        request_id
    );

    // Handle error responses first - they're always valid and we remove from cache
    if let Response::Error { error } = &response {
        if let Some(req_id) = &request_id {
            let mut requests = sent_requests.lock().expect("poisoned");
            requests.remove(req_id);
        }
        let wss_error = format!("WSS error: {} - {}", error.code, error.message);
        tracing::error!("{wss_error}");
        return Err(wss_error);
    }

    // Validate response type matches request type if request_id is present
    if let Some(req_id) = &request_id {
        let request = sent_requests
            .lock()
            .expect("poisoned")
            .get(req_id)
            .map(|(r, _, _)| r.clone());
        let expected_response_type = request.as_ref().map(get_expected_response_type);

        if let Some(expected) = expected_response_type {
            if !matches_response_type(&response, expected) {
                tracing::error!("Response {response:?} do not match request {request:?}");
                return Err(format!(
                    "Response type mismatch for {req_id}: expected {:?}, got {:?}",
                    expected, response
                ));
            }
            // Remove from cache on successful match
            let mut requests = sent_requests.lock().expect("poisoned");
            requests.remove(req_id);
        }
    }

    match response {
        Response::Error { .. } => {
            // Already handled above, but needed for exhaustiveness
            unreachable!()
        }
        Response::Connected { version, .. } => {
            // NOTE: we should never land here
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
            handle_user(user, users, request_sender, n_sender, n_waker)?;
        }
        Response::DeleteUserOrg { user, org } => {
            let user_id = *user_id.lock().expect("poisoned");
            handle_delete_user_org(orgs, user_id, user, org, n_sender, n_waker);
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
        Request::FetchOrg { .. } => ExpectedResponseType::Org,
        Request::EditWallet { .. } | Request::FetchWallet { .. } | Request::EditXpub { .. } => {
            ExpectedResponseType::Wallet
        }
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
    tracing::debug!("handle_pong: received pong, connection healthy");
    // Reset ping tracking on successful pong
    {
        let mut ping_time = last_ping_time.lock().expect("poisoned");
        *ping_time = None;
    }
    Ok(())
}

fn fetch_user_maybe(
    user: Option<Uuid>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_sender: &channel::Sender<Request>,
) {
    if let Some(id) = user {
        if !users.lock().expect("poisoned").contains_key(&id) {
            let _ = request_sender.send(Request::FetchUser { id });
        }
    }
}

fn handle_org(
    org: Org,
    orgs: &Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_sender: &channel::Sender<Request>,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) -> Result<(), String> {
    let org_id = org.id;

    tracing::debug!(
        "handle_org: received org update, org_id={} name={}",
        org_id,
        org.name
    );

    fetch_user_maybe(org.last_editor, users, request_sender);

    // Update cache
    orgs.lock().expect("poisoned").insert(org_id, org.clone());

    // If any users are not cached, send fetch_user requests.
    // The responses will be handled automatically by handle_user().
    let mut missing_users = 0;
    {
        let users_guard = users.lock().expect("poisoned");
        for user_id in &org.users {
            if !users_guard.contains_key(user_id) {
                let _ = request_sender.send(Request::FetchUser { id: *user_id });
                missing_users += 1;
            }
        }
    }

    // If any wallets are not cached, send fetch_wallet requests.
    // The responses will be handled automatically by handle_wallet().
    let mut missing_wallets = 0;
    {
        let wallets_guard = wallets.lock().expect("poisoned");
        for wallet_id in &org.wallets {
            if !wallets_guard.contains_key(wallet_id) {
                let _ = request_sender.send(Request::FetchWallet { id: *wallet_id });
                missing_wallets += 1;
            }
        }
    }

    if missing_users > 0 || missing_wallets > 0 {
        tracing::debug!(
            "handle_org: fetching {} missing users, {} missing wallets",
            missing_users,
            missing_wallets
        );
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
    wallet: Wallet,
    wallets: &Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_sender: &channel::Sender<Request>,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) -> Result<(), String> {
    let wallet_id = wallet.id;
    let owner_id = wallet.owner;

    tracing::debug!(
        "handle_wallet: received wallet update, wallet_id={} alias={} status={:?}",
        wallet_id,
        wallet.alias,
        wallet.status
    );

    // Fetch last editors
    fetch_user_maybe(wallet.last_editor, users, request_sender);
    if let Some(template) = &wallet.template {
        for key in template.keys.values() {
            fetch_user_maybe(key.last_editor, users, request_sender);
        }

        fetch_user_maybe(template.primary_path.last_editor, users, request_sender);
        for path in &template.secondary_paths {
            fetch_user_maybe(path.path.last_editor, users, request_sender);
        }
    }

    // Update cache
    wallets
        .lock()
        .expect("poisoned")
        .insert(wallet_id, wallet.clone());

    // If the owner user is not cached, send a fetch_user request.
    // The response will be handled automatically by handle_user().
    {
        let users_guard = users.lock().expect("poisoned");
        if !users_guard.contains_key(&owner_id) {
            tracing::debug!(
                "handle_wallet: fetching owner user_id={}",
                owner_id
            );
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
    user: User,
    users: &Arc<Mutex<BTreeMap<Uuid, User>>>,
    request_sender: &channel::Sender<Request>,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) -> Result<(), String> {
    let user_id = user.uuid;

    tracing::debug!(
        "handle_user: received user update, user_id={} name={} role={:?}",
        user_id,
        user.name,
        user.role
    );

    fetch_user_maybe(user.last_editor, users, request_sender);

    // Update cache
    {
        let mut users_guard = users.lock().expect("poisoned");
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

pub fn handle_delete_user_org(
    orgs: &Arc<Mutex<BTreeMap<Uuid, Org>>>,
    user: Option<Uuid>,
    target_user: Uuid,
    org: Uuid,
    notification_sender: &channel::Sender<Message>,
    notification_waker: &SharedWaker,
) {
    if let Some(user) = user {
        if target_user != user {
            tracing::debug!(
                "handle_delete_user_org: ignoring deletion for different user, target={} current={}",
                target_user,
                user
            );
            return;
        }

        tracing::info!(
            "handle_delete_user_org: user removed from org, user_id={} org_id={}",
            user,
            org
        );

        tracing::debug!("handle_delete_user_org: removing org from cache");
        orgs.lock().expect("poisoned").remove(&org);

        // Send response
        Client::send_notif(
            notification_sender,
            notification_waker,
            Notification::Update.into(),
        );
    }
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
        tracing::info!("connect_ws: connecting to WebSocket url={}", url);

        // Close existing connection if any
        if self.connected.load(Ordering::Relaxed) {
            tracing::debug!("connect_ws: closing existing connection");
            self.close();
        }

        self.notif_sender = notif_sender.clone();

        // Prepare token retrieval data for the background thread
        let token_data = TokenRetrievalData {
            token: self.token.clone(),
            network_dir: self.network_dir.clone(),
            email: self.email.clone(),
            network: self.network,
            auth_client: self.auth_client.clone(),
        };

        let (request_sender, request_receiver) = channel::unbounded();

        let orgs = self.orgs.clone();
        let wallets = self.wallets.clone();
        let users = self.users.clone();
        let user_id = self.user_id.clone();

        self.request_sender = Some(request_sender.clone());
        self.connected = Arc::new(AtomicBool::new(false));
        let connected = self.connected.clone();

        tracing::debug!("connect_ws: spawning WebSocket thread");

        let notif_sender = notif_sender.clone();
        let notif_waker = self.notif_waker.clone();
        let handle = thread::spawn(move || {
            wss_thread(
                url,
                token_data,
                version,
                orgs,
                wallets,
                users,
                user_id,
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
        // Store email for later use
        self.email = Some(email.clone());

        let notif_sender = self.notif_sender.clone();
        let notif_waker = self.notif_waker.clone();
        let network = self.network.unwrap_or(Network::Bitcoin);
        let email_clone = email.clone();
        let auth_client_2 = self.auth_client.clone();

        tracing::debug!("auth_request: starting for email={}", email);

        let rt = tokio::runtime::Handle::current();
        rt.spawn(async move {
            tracing::debug!(
                "auth_request: fetching service config for network={:?}",
                network
            );
            let config = match get_service_config_blocking(network) {
                Ok(cfg) => {
                    tracing::debug!(
                        "auth_request: got config auth_api_url={} backend_api_url={}",
                        cfg.auth_api_url,
                        cfg.backend_api_url
                    );
                    cfg
                }
                Err(e) => {
                    tracing::debug!("auth_request: failed to get service config: {:?}", e);
                    Client::send_notif(
                        &notif_sender,
                        &notif_waker,
                        Notification::AuthCodeFail.into(),
                    );
                    return;
                }
            };

            // Create auth client
            tracing::debug!(
                "auth_request: creating AuthClient with url={} email={}",
                config.auth_api_url,
                email_clone
            );
            let auth_client = AuthClient::new(
                config.auth_api_url.clone(),
                config.auth_api_public_key.clone(),
                email_clone.clone(),
            );

            // Send OTP (requires async for the HTTP call)
            tracing::debug!("auth_request: sending OTP request");
            let result = match auth_client.sign_in_otp().await {
                Ok(()) => {
                    tracing::debug!("auth_request: OTP sent successfully");
                    Ok(auth_client)
                }
                Err(e) => {
                    tracing::debug!(
                        "auth_request: OTP request failed: http_status={:?} error={}",
                        e.http_status,
                        e.error
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
            };

            // Handle result after async block
            match result {
                Ok(client) => {
                    // Store auth client
                    if let Ok(mut client_guard) = auth_client_2.lock() {
                        *client_guard = Some(client);
                    }
                    tracing::debug!("auth_request: sending AuthCodeSent notification");
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
        tracing::debug!("auth_code: starting OTP verification");

        let notif_sender = self.notif_sender.clone();
        let notif_waker = self.notif_waker.clone();
        let auth_client_shared = Arc::clone(&self.auth_client);
        let network_dir = self.network_dir.clone();
        let token_shared = Arc::clone(&self.token);

        let rt = tokio::runtime::Handle::current();
        rt.spawn(async move {
            // Get auth client
            let auth_client = {
                let client_guard = auth_client_shared.lock().expect("poisoned");
                client_guard.clone()
            };
            let auth_client = match auth_client {
                Some(client) => client,
                None => {
                    tracing::error!("auth_code: no auth client available");
                    Client::send_notif(&notif_sender, &notif_waker, Notification::LoginFail.into());
                    return;
                }
            };

            // Verify OTP
            tracing::debug!("auth_code: verifying OTP");
            let tokens = match auth_client.verify_otp(code.trim()).await {
                Ok(tokens) => {
                    tracing::debug!("auth_code: OTP verified successfully");
                    tokens
                }
                Err(e) => {
                    tracing::warn!("auth_code: OTP verification failed: {:?}", e);
                    Client::send_notif(&notif_sender, &notif_waker, Notification::LoginFail.into());
                    return;
                }
            };

            // Update cache if network_dir is available
            let access_token = if let Some(ref network_dir) = network_dir {
                tracing::debug!("auth_code: updating token cache");
                match update_connect_cache(network_dir, &tokens, &auth_client, false).await {
                    Ok(updated_tokens) => updated_tokens.access_token,
                    Err(e) => {
                        // Cache update failed, but we still have tokens
                        tracing::error!("auth_code: failed to cache token on disk: {e}");
                        tokens.access_token
                    }
                }
            } else {
                // No network_dir, just use the token
                tracing::debug!("auth_code: no network_dir, skipping cache update");
                tokens.access_token
            };

            if let Ok(mut token_guard) = token_shared.lock() {
                *token_guard = Some(access_token.clone());
            }

            tracing::info!("auth_code: login successful");

            Client::send_notif(
                &notif_sender,
                &notif_waker,
                Notification::LoginSuccess.into(),
            );
        });
    }

    fn get_orgs(&self) -> BTreeMap<Uuid, Org> {
        self.orgs.lock().expect("poisoned").clone()
    }

    fn get_org(&self, id: Uuid) -> Option<Org> {
        let org = self.orgs.lock().expect("poisoned").get(&id)?.clone();
        let mut wallets = BTreeMap::new();
        {
            let wallets_guard = self.wallets.lock().expect("poisoned");
            for w_id in &org.wallets {
                if let Some(wallet) = wallets_guard.get(w_id) {
                    wallets.insert(*w_id, wallet.clone());
                }
            }
        }

        let wallets = wallets.keys().copied().collect();
        Some(Org {
            name: org.name,
            id: org.id,
            wallets,
            users: org.users,
            owners: org.owners,
            last_edited: None,
            last_editor: None,
        })
    }

    fn get_user(&self, id: Uuid) -> Option<User> {
        self.users.lock().expect("poisoned").get(&id).cloned()
    }

    fn get_wallet(&self, id: Uuid) -> Option<Wallet> {
        self.wallets.lock().expect("poisoned").get(&id).cloned()
    }

    fn close(&mut self) {
        if !self.connected.load(Ordering::Relaxed) {
            tracing::debug!("close: already disconnected");
            return;
        }

        tracing::info!("close: closing WebSocket connection");

        // Send close message if possible
        if let Some(sender) = &self.request_sender {
            tracing::debug!("close: sending close request");
            let _ = sender.send(Request::Close);
        }

        let _ = self.wss_thread_handle.take();

        self.connected.store(false, Ordering::Relaxed);
        self.request_sender = None;

        tracing::debug!("close: connection closed");
    }

    #[cfg(test)]
    fn fetch_org(&mut self, id: Uuid) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::FetchOrg { id });
        }
    }

    fn edit_wallet(&mut self, wallet: Wallet) {
        check_connection!(self);

        tracing::debug!(
            "edit_wallet: sending edit request for wallet_id={}",
            wallet.id
        );

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::EditWallet { wallet });
        }
    }

    #[cfg(test)]
    fn fetch_wallet(&mut self, id: Uuid) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::FetchWallet { id });
        }
    }

    fn edit_xpub(&mut self, wallet_id: Uuid, xpub: Option<ws_business::Xpub>, key_id: u8) {
        check_connection!(self);

        tracing::debug!(
            "edit_xpub: sending edit request for wallet_id={} key_id={} xpub={}",
            wallet_id,
            key_id,
            if xpub.is_some() { "present" } else { "none" }
        );

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::EditXpub {
                wallet_id,
                key_id,
                xpub,
            });
        }
    }

    #[cfg(test)]
    fn fetch_user(&mut self, id: Uuid) {
        check_connection!(self);

        if let Some(sender) = &self.request_sender {
            let _ = sender.send(Request::FetchUser { id });
        }
    }
}

/// DummyServer is a WebSocket server that can handle Client connections
/// and manage Request/Response messages for development/testing
#[derive(Debug)]
#[cfg(test)]
pub struct DummyServer {
    port: u16,
    handle: Option<thread::JoinHandle<()>>,
    shutdown_sender: Option<channel::Sender<()>>,
}

#[cfg(test)]
impl DummyServer {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            handle: None,
            shutdown_sender: None,
        }
    }

    pub fn start(&mut self, handler: Box<dyn Fn(WsMessage) -> WsMessage + Send + Sync + 'static>) {
        let port = self.port;
        let (shutdown_sender, shutdown_receiver) = channel::bounded(1);
        self.shutdown_sender = Some(shutdown_sender);

        let handle = thread::spawn(move || {
            let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
                .expect("Failed to bind to address");
            listener.set_nonblocking(false).unwrap();

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
                Ok(ws_msg) => ws_msg,
                _ => return,
            };

            // Parse connect request and respond
            let (request, _token, id) = match ws_business::Request::from_ws_message(connect_msg) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!("{e}");
                    return;
                }
            };

            if let Request::Connect { version } = request {
                if version == 1 {
                    // Respond with connected
                    let connected = Response::Connected {
                        version: 1,
                        user: uuid::Uuid::nil(),
                    };
                    let resp = connected.to_ws_message(Some(&id));
                    if ws_stream.send(resp).is_err() {
                        return;
                    }
                } else {
                    return;
                }
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
                    default => {
                        match ws_stream.read() {
                            Ok(WsMessage::Text(t)) => {
                                let ws_msg = handler(WsMessage::Text(t));
                                if ws_stream.send(ws_msg).is_err() {
                                    break;
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

    pub fn close(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
impl Drop for DummyServer {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod parsing_tests {
        use crate::client::WsMessage;
        use liana_connect::ws_business::{Response, UserRole, WalletStatus, WssConversionError};

        #[test]
        fn test_parse_connected_response() {
            let json = r#"{
                "type": "connected",
                "request_id": "550e8400-e29b-41d4-a716-446655440000",
                "payload": {"version": 1, "user": "550e8400-e29b-41d4-a716-446655440001"}
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Connected { version, .. } => assert_eq!(version, 1),
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
                "payload": {"version": 2, "user": "550e8400-e29b-41d4-a716-446655440001"}
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Connected { version, .. } => assert_eq!(version, 2),
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
                    assert_eq!(org.id.to_string(), "550e8400-e29b-41d4-a716-446655440010");
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
                                "xpub": "[aabbccdd/48'/0'/0'/2']xpub661MyMwAqRbcFtXgS5sYJABqqG9YLmC4Q1Rdap9gSE8NqtwybGhePY2gZ29ESFjqJoCu1Rupje8YtGqsefD265TMg7usUDFdp6W1EGMcet8/<0;1>/*"
                            }
                        },
                        "primary_path": {
                            "is_primary": true,
                            "threshold_n": 2,
                            "key_ids": [0, 1]
                        },
                        "secondary_paths": [
                            {
                                "is_primary": false,
                                "threshold_n": 1,
                                "key_ids": [0],
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
                    assert_eq!(
                        wallet.id.to_string(),
                        "550e8400-e29b-41d4-a716-446655440020"
                    );
                    assert_eq!(wallet.alias, "Main Wallet");
                    assert_eq!(wallet.status, WalletStatus::Created);
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
                    "status": "Drafted"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, _) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::Wallet { wallet } => {
                    assert_eq!(wallet.alias, "Simple Wallet");
                    assert_eq!(wallet.status, WalletStatus::Drafted);
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
                    "role": "WalletManager"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            let (response, request_id) = Response::from_ws_message(msg).unwrap();

            match response {
                Response::User { user } => {
                    assert_eq!(user.name, "John Doe");
                    assert_eq!(
                        user.uuid.to_string(),
                        "550e8400-e29b-41d4-a716-446655440030"
                    );
                    assert_eq!(user.email, "john@example.com");
                    assert_eq!(user.role, UserRole::WalletManager);
                }
                _ => panic!("Expected User response"),
            }
            assert_eq!(
                request_id,
                Some("550e8400-e29b-41d4-a716-446655440004".to_string())
            );
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
            // With direct deserialization to Org, invalid UUID should fail parsing
            let result = Response::from_ws_message(msg);
            assert!(result.is_err(), "Invalid UUID should fail to parse");
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
            // Parsing now fails for invalid enum variants
            let result = Response::from_ws_message(msg);
            assert!(result.is_err(), "Parsing should fail for invalid status");
        }

        #[test]
        fn test_parse_user_invalid_role() {
            let json = r#"{
                "type": "user",
                "payload": {
                    "name": "Test User",
                    "uuid": "550e8400-e29b-41d4-a716-446655440030",
                    "email": "test@example.com",
                    "role": "InvalidRole"
                }
            }"#;
            let msg = WsMessage::Text(json.to_string());
            // Parsing now fails for invalid enum variants
            let result = Response::from_ws_message(msg);
            assert!(result.is_err(), "Parsing should fail for invalid role");
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
                    "status": "created",
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
            // Parsing now fails for invalid enum variants
            let result = Response::from_ws_message(msg);
            assert!(result.is_err(), "Parsing should fail for invalid key_type");
        }
    }

    mod integration_tests {
        use super::*;
        use liana_connect::ws_business::{
            models::{UserRole, WalletStatus},
            WssError,
        };
        use std::time::Duration;

        fn create_test_org() -> Org {
            use std::collections::BTreeSet;
            Org {
                name: "Test Org".to_string(),
                id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440010").unwrap(),
                wallets: BTreeSet::from_iter([Uuid::parse_str(
                    "550e8400-e29b-41d4-a716-446655440020",
                )
                .unwrap()]),
                users: BTreeSet::from_iter([Uuid::parse_str(
                    "550e8400-e29b-41d4-a716-446655440030",
                )
                .unwrap()]),
                owners: vec![Uuid::parse_str("550e8400-e29b-41d4-a716-446655440030").unwrap()],
                last_edited: None,
                last_editor: None,
            }
        }

        fn create_test_wallet() -> Wallet {
            Wallet {
                id: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440020").unwrap(),
                alias: "Test Wallet".to_string(),
                org: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440010").unwrap(),
                owner: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440030").unwrap(),
                status: WalletStatus::Created,
                template: None,
                last_edited: None,
                last_editor: None,
            }
        }

        fn create_test_user() -> User {
            User {
                name: "Test User".to_string(),
                uuid: Uuid::parse_str("550e8400-e29b-41d4-a716-446655440030").unwrap(),
                email: "test@example.com".to_string(),
                role: UserRole::WalletManager,
                last_edited: None,
                last_editor: None,
            }
        }

        #[test]
        fn test_client_connection_with_dummy_server() {
            let port = 30108;
            let mut server = DummyServer::new(port);

            let handler: Box<dyn Fn(WsMessage) -> WsMessage + Send + Sync + 'static> =
                Box::new(|msg| {
                    // Parse request, respond with Pong
                    if let Ok((_req, _token, id)) = Request::from_ws_message(msg) {
                        Response::Pong.to_ws_message(Some(&id))
                    } else {
                        Response::Pong.to_ws_message(None)
                    }
                });

            server.start(handler);

            // Give server time to start and bind to port (server thread needs time to spawn and bind)
            thread::sleep(Duration::from_millis(300));

            let (sender, _receiver) = channel::unbounded();
            let notif_waker: SharedWaker = Arc::new(Mutex::new(None));
            let mut client = Client::new(sender, notif_waker);
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

            let test_org = create_test_org();
            let handler: Box<dyn Fn(WsMessage) -> WsMessage + Send + Sync + 'static> =
                Box::new(move |msg| {
                    if let Ok((req, _token, id)) = Request::from_ws_message(msg) {
                        let response = match req {
                            Request::FetchOrg { .. } => Response::Org {
                                org: test_org.clone(),
                            },
                            _ => Response::Pong,
                        };
                        response.to_ws_message(Some(&id))
                    } else {
                        Response::Pong.to_ws_message(None)
                    }
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let notif_waker: SharedWaker = Arc::new(Mutex::new(None));
            let mut client = Client::new(sender, notif_waker);
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

            let test_wallet = create_test_wallet();
            let handler: Box<dyn Fn(WsMessage) -> WsMessage + Send + Sync + 'static> =
                Box::new(move |msg| {
                    if let Ok((req, _token, id)) = Request::from_ws_message(msg) {
                        let response = match req {
                            Request::FetchWallet { .. } => Response::Wallet {
                                wallet: test_wallet.clone(),
                            },
                            _ => Response::Pong,
                        };
                        response.to_ws_message(Some(&id))
                    } else {
                        Response::Pong.to_ws_message(None)
                    }
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let notif_waker: SharedWaker = Arc::new(Mutex::new(None));
            let mut client = Client::new(sender, notif_waker);
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

            let test_user = create_test_user();
            let handler: Box<dyn Fn(WsMessage) -> WsMessage + Send + Sync + 'static> =
                Box::new(move |msg| {
                    if let Ok((req, _token, id)) = Request::from_ws_message(msg) {
                        let response = match req {
                            Request::FetchUser { .. } => Response::User {
                                user: test_user.clone(),
                            },
                            _ => Response::Pong,
                        };
                        response.to_ws_message(Some(&id))
                    } else {
                        Response::Pong.to_ws_message(None)
                    }
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let notif_waker: SharedWaker = Arc::new(Mutex::new(None));
            let mut client = Client::new(sender, notif_waker);
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

            let handler: Box<dyn Fn(WsMessage) -> WsMessage + Send + Sync + 'static> =
                Box::new(|msg| {
                    if let Ok((_req, _token, id)) = Request::from_ws_message(msg) {
                        let response = Response::Error {
                            error: WssError {
                                code: "TEST_ERROR".to_string(),
                                message: "Test error message".to_string(),
                                request_id: None,
                            },
                        };
                        response.to_ws_message(Some(&id))
                    } else {
                        Response::Pong.to_ws_message(None)
                    }
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let notif_waker: SharedWaker = Arc::new(Mutex::new(None));
            let mut client = Client::new(sender, notif_waker);
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
        fn test_client_close() {
            let port = 30106;
            let mut server = DummyServer::new(port);

            let handler: Box<dyn Fn(WsMessage) -> WsMessage + Send + Sync + 'static> =
                Box::new(|msg| {
                    if let Ok((_req, _token, id)) = Request::from_ws_message(msg) {
                        Response::Pong.to_ws_message(Some(&id))
                    } else {
                        Response::Pong.to_ws_message(None)
                    }
                });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let notif_waker: SharedWaker = Arc::new(Mutex::new(None));
            let mut client = Client::new(sender, notif_waker);
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

            let test_org = create_test_org();
            let test_wallet = create_test_wallet();
            let test_user = create_test_user();

            let org_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440010").unwrap();
            let wallet_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440020").unwrap();
            let _user_id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440030").unwrap();

            let handler: Box<dyn Fn(WsMessage) -> WsMessage + Send + Sync + 'static> = Box::new({
                let test_org = test_org.clone();
                let test_wallet = test_wallet.clone();
                let test_user = test_user.clone();
                move |msg| {
                    if let Ok((req, _token, id)) = Request::from_ws_message(msg) {
                        let response = match req {
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
                        };
                        response.to_ws_message(Some(&id))
                    } else {
                        Response::Pong.to_ws_message(None)
                    }
                }
            });

            server.start(handler);

            thread::sleep(Duration::from_millis(200));

            let (sender, _receiver) = channel::unbounded();
            let notif_waker: SharedWaker = Arc::new(Mutex::new(None));
            let mut client = Client::new(sender, notif_waker);
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
                org_data.wallets.contains(&wallet_id),
                "Wallet should be in org data"
            );

            client.close();
            server.close();
        }
    }
}
