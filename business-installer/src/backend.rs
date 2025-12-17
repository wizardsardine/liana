use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use crossbeam::channel;
use iced::futures::Stream;
use miniscript::DescriptorPublicKey;
use std::sync::mpsc;
use thiserror::Error;
use uuid::Uuid;

use crate::client::{Client, DummyServer};
use liana_connect::PolicyTemplate;
use liana_connect::{Request, Response, WssError};

// Re-export domain types from liana-connect for use by other modules
pub use liana_connect::{Key, KeyType, Org, OrgData, User, UserRole, Wallet, WalletStatus};

/// Global channel for backend communication (used by subscription)
pub static BACKEND_RECV: Mutex<Option<channel::Receiver<Notification>>> = Mutex::new(None);

#[derive(Debug, Clone, Error)]
pub enum Error {
    #[error("")]
    None,
    #[error("Iced subscription failed!")]
    SubscriptionFailed,
    #[error("Missing token for auth on backend!")]
    TokenMissing,
    #[error("Failed to open the websocket connection")]
    WsConnection,
    #[error("Failed to handle a Websocket response: {0}")]
    WsMessageHandling(String),
    #[error("Receive an error from the server: {0}")]
    Wss(WssError),
}

impl Error {
    pub fn show_warning(&self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone)]
pub enum Notification {
    Connected,
    Disconnected,
    AuthCodeSent,
    InvalidEmail,
    AuthCodeFail,
    LoginSuccess,
    LoginFail,
    Org(Uuid),
    Wallet(Uuid),
    User(Uuid),
    Error(Error),
}

#[allow(unused)]
#[rustfmt::skip]
pub trait Backend {
    // Auth, not part of WSS protocol
    fn auth_request(&mut self, email: String);  // -> Response::AuthCodeSent
                                                // -> Response::InvalidEmail
                                                // -> Response::AuthCodeFail
    fn auth_code(&mut self, code: String);  // -> Response::LoginSuccess
                                            // -> Response::LoginFail

    // Cache only, not backend calls
    fn get_orgs(&self) -> BTreeMap<Uuid, Org>;
    fn get_org(&self, id: Uuid) -> Option<OrgData>;
    fn get_user(&self, id: Uuid) -> Option<User>;
    fn get_wallet(&self, id: Uuid) -> Option<Wallet>;

    // Connection (WSS)
    fn connect_ws(&mut self, url: String, version: u8) -> Option<channel::Receiver<Notification>>; // -> Response::Connected
    fn ping(&mut self); // -> Response::Pong
    fn close(&mut self);    // Connection closed

    // Org management (WSS)
    fn fetch_org(&mut self, id: Uuid);                                      // -> Response::Org
    fn remove_wallet_from_org(&mut self, wallet_id: Uuid, org_id: Uuid);    // -> Response::Org

    fn create_wallet(&mut self, name: String, org: Uuid, owner: Uuid);      // -> Response::Wallet
    fn edit_wallet(&mut self, wallet: Wallet);                              // -> Response::Wallet
    fn fetch_wallet(&mut self, id: Uuid);                                   // -> Response::Wallet
    fn edit_xpub(
        &mut self,
        wallet_id: Uuid,
        xpub: Option<DescriptorPublicKey>,
        key_id: u8);                                                     // -> Response::Wallet

    fn fetch_user(&mut self, id: Uuid);         // -> Response::User

}

/// Stream wrapper for Backend responses
pub struct BackendStream {
    pub receiver: mpsc::Receiver<Notification>,
}

impl Stream for BackendStream {
    type Item = Notification;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Use try_recv for non-blocking check
        match self.receiver.try_recv() {
            Ok(item) => Poll::Ready(Some(item)),
            Err(mpsc::TryRecvError::Empty) => Poll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => Poll::Ready(None),
        }
    }
}

/// Development backend that uses DummyServer and Client for local testing
/// This is a temporary test feature that will be removed when the server launches.
/// The dummy server is spawned automatically by Client::connect() when connecting to localhost.
#[derive(Debug)]
pub struct DevBackend {
    server: Option<DummyServer>,
    client: Client,
    orgs: BTreeMap<Uuid, Org>,
    wallets: BTreeMap<Uuid, Wallet>,
    users: BTreeMap<Uuid, User>,
    auth_code: String,
}

/// Initialize a Client with test data (DEBUG feature for local testing)
pub fn init_client_with_test_data() -> Client {
    let client = Client::new();
    let mut orgs = BTreeMap::new();
    let mut wallets = BTreeMap::new();
    let mut users = BTreeMap::new();

    init_test_data(&mut orgs, &mut wallets, &mut users);

    // Populate Client's data structures with test data
    {
        let mut orgs_guard = client.orgs.lock().unwrap();
        *orgs_guard = orgs;
    }
    {
        let mut wallets_guard = client.wallets.lock().unwrap();
        *wallets_guard = wallets;
    }
    {
        let mut users_guard = client.users.lock().unwrap();
        *users_guard = users;
    }

    client
}

fn init_test_data(
    orgs: &mut BTreeMap<Uuid, Org>,
    wallets: &mut BTreeMap<Uuid, Wallet>,
    users: &mut BTreeMap<Uuid, User>,
) {
    // ==========================================================================
    // TEST USERS - Login with these emails to test different roles:
    //
    // ws@example.com    -> WSManager for all wallets (not owner, no keys)
    // owner@example.com -> Owner of Draft/Validated/Final, Participant of Shared
    // user@example.com  -> Participant for all wallets (has keys in all)
    //
    // Note: Participants should NOT see Draft wallets (filtered in view)
    // ==========================================================================

    // Owner user - owns Draft, Validated, Final wallets
    let owner_user = User {
        name: "Wallet Owner".to_string(),
        uuid: Uuid::new_v4(),
        email: "owner@example.com".to_string(),
        orgs: Vec::new(),
        role: UserRole::Owner,
    };
    users.insert(owner_user.uuid, owner_user.clone());

    // Shared wallet owner - owns the Shared wallet
    let shared_owner = User {
        name: "Shared Wallet Owner".to_string(),
        uuid: Uuid::new_v4(),
        email: "shared-owner@example.com".to_string(),
        orgs: Vec::new(),
        role: UserRole::Owner,
    };
    users.insert(shared_owner.uuid, shared_owner.clone());

    // Create Org 1: "Acme Corp" - demonstrates all wallet statuses and roles
    let org1_id = Uuid::new_v4();
    let mut org1_wallets = BTreeSet::new();

    // -------------------------------------------------------------------------
    // Wallet 1: DRAFT - Only visible to WSManager and Owner
    // owner@example.com -> Owner
    // ws@example.com    -> Manager
    // user@example.com  -> (should not see this wallet)
    // -------------------------------------------------------------------------
    let wallet1_id = Uuid::new_v4();
    let mut wallet1_template = PolicyTemplate::new();
    wallet1_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Owner Key".to_string(),
            description: "Key held by wallet owner".to_string(),
            email: "owner@example.com".to_string(),
            key_type: KeyType::Internal,
            xpub: None,
        },
    );
    wallet1_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "Participant Key".to_string(),
            description: "Key for participant user".to_string(),
            email: "user@example.com".to_string(),
            key_type: KeyType::External,
            xpub: None,
        },
    );
    wallet1_template.primary_path.key_ids.push(0);
    wallet1_template.primary_path.key_ids.push(1);
    wallet1_template.primary_path.threshold_n = 2;

    let wallet1 = Wallet {
        alias: "Draft Wallet".to_string(),
        org: org1_id,
        owner: owner_user.clone(),
        id: wallet1_id,
        template: Some(wallet1_template),
        status: WalletStatus::Drafted,
    };
    org1_wallets.insert(wallet1_id);
    wallets.insert(wallet1_id, wallet1);

    // -------------------------------------------------------------------------
    // Wallet 2: VALIDATED - Visible to all, participants can add xpubs
    // owner@example.com -> Owner
    // ws@example.com    -> Manager
    // user@example.com  -> Participant
    // -------------------------------------------------------------------------
    let wallet2_id = Uuid::new_v4();
    let mut wallet2_template = PolicyTemplate::new();
    wallet2_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Owner Key".to_string(),
            description: "Key held by wallet owner".to_string(),
            email: "owner@example.com".to_string(),
            key_type: KeyType::Internal,
            xpub: None,
        },
    );
    wallet2_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "Participant Key".to_string(),
            description: "Key for participant user".to_string(),
            email: "user@example.com".to_string(),
            key_type: KeyType::External,
            xpub: None,
        },
    );
    wallet2_template.primary_path.key_ids.push(0);
    wallet2_template.primary_path.key_ids.push(1);
    wallet2_template.primary_path.threshold_n = 2;

    let wallet2 = Wallet {
        alias: "Validated Wallet".to_string(),
        org: org1_id,
        owner: owner_user.clone(),
        id: wallet2_id,
        template: Some(wallet2_template),
        status: WalletStatus::Validated,
    };
    org1_wallets.insert(wallet2_id);
    wallets.insert(wallet2_id, wallet2);

    // -------------------------------------------------------------------------
    // Wallet 3: FINALIZED - Ready to load
    // owner@example.com -> Owner
    // ws@example.com    -> Manager
    // user@example.com  -> Participant
    // -------------------------------------------------------------------------
    let wallet3_id = Uuid::new_v4();
    let mut wallet3_template = PolicyTemplate::new();
    wallet3_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Owner Key".to_string(),
            description: "Key held by wallet owner".to_string(),
            email: "owner@example.com".to_string(),
            key_type: KeyType::Internal,
            xpub: None,
        },
    );
    wallet3_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "Participant Key".to_string(),
            description: "Key for participant user".to_string(),
            email: "user@example.com".to_string(),
            key_type: KeyType::External,
            xpub: None,
        },
    );
    wallet3_template.primary_path.key_ids.push(0);
    wallet3_template.primary_path.key_ids.push(1);
    wallet3_template.primary_path.threshold_n = 2;

    let wallet3 = Wallet {
        alias: "Final Wallet".to_string(),
        org: org1_id,
        owner: owner_user.clone(),
        id: wallet3_id,
        template: Some(wallet3_template),
        status: WalletStatus::Finalized,
    };
    org1_wallets.insert(wallet3_id);
    wallets.insert(wallet3_id, wallet3);

    // -------------------------------------------------------------------------
    // Wallet 4: SHARED - Owner is different, owner@example.com is participant
    // shared-owner@example.com -> Owner
    // owner@example.com        -> Participant (has a key)
    // ws@example.com           -> Manager
    // user@example.com         -> Participant
    // -------------------------------------------------------------------------
    let wallet4_id = Uuid::new_v4();
    let mut wallet4_template = PolicyTemplate::new();
    wallet4_template.keys.insert(
        0,
        Key {
            id: 0,
            alias: "Shared Owner Key".to_string(),
            description: "Key held by shared wallet owner".to_string(),
            email: "shared-owner@example.com".to_string(),
            key_type: KeyType::Internal,
            xpub: None,
        },
    );
    wallet4_template.keys.insert(
        1,
        Key {
            id: 1,
            alias: "Owner as Participant".to_string(),
            description: "owner@example.com is participant here".to_string(),
            email: "owner@example.com".to_string(),
            key_type: KeyType::External,
            xpub: None,
        },
    );
    wallet4_template.keys.insert(
        2,
        Key {
            id: 2,
            alias: "User Key".to_string(),
            description: "Key for user@example.com".to_string(),
            email: "user@example.com".to_string(),
            key_type: KeyType::External,
            xpub: None,
        },
    );
    wallet4_template.primary_path.key_ids.push(0);
    wallet4_template.primary_path.key_ids.push(1);
    wallet4_template.primary_path.key_ids.push(2);
    wallet4_template.primary_path.threshold_n = 2;

    let wallet4 = Wallet {
        alias: "Shared Wallet".to_string(),
        org: org1_id,
        owner: shared_owner.clone(),
        id: wallet4_id,
        template: Some(wallet4_template),
        status: WalletStatus::Finalized,
    };
    org1_wallets.insert(wallet4_id);
    wallets.insert(wallet4_id, wallet4);

    let org1 = Org {
        name: "Acme Corp".to_string(),
        id: org1_id,
        wallets: org1_wallets,
        users: Default::default(),
        owners: Default::default(),
    };
    orgs.insert(org1_id, org1);

    // Create Org 2: "Empty Org" - no wallets
    let org2_id = Uuid::new_v4();
    let org2 = Org {
        name: "Empty Org".to_string(),
        id: org2_id,
        wallets: BTreeSet::new(),
        users: Default::default(),
        owners: Default::default(),
    };
    orgs.insert(org2_id, org2);
}

impl DevBackend {
    pub fn new() -> Self {
        let mut backend = Self {
            server: None,
            client: init_client_with_test_data(),
            orgs: BTreeMap::new(),
            wallets: BTreeMap::new(),
            users: BTreeMap::new(),
            auth_code: "123456".to_string(),
        };

        // Populate local data from client for get_org, get_orgs, etc.
        {
            let orgs_guard = backend.client.orgs.lock().unwrap();
            backend.orgs = orgs_guard.clone();
        }
        {
            let wallets_guard = backend.client.wallets.lock().unwrap();
            backend.wallets = wallets_guard.clone();
        }
        {
            let users_guard = backend.client.users.lock().unwrap();
            backend.users = users_guard.clone();
        }

        backend
    }
}

// Handler creation for dummy server (DEBUG feature)
pub fn create_dummy_server_handler(
    orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>,
    wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>,
    users: Arc<Mutex<BTreeMap<Uuid, User>>>,
) -> Box<dyn Fn(Request) -> Response + Send + Sync> {
    Box::new(move |request| match request {
        Request::FetchOrg { id } => handle_fetch_org(orgs.clone(), id),
        Request::FetchWallet { id } => handle_fetch_wallet(wallets.clone(), id),
        Request::FetchUser { id } => handle_fetch_user(users.clone(), id),
        Request::CreateWallet {
            name,
            org_id,
            owner_id,
        } => handle_create_wallet(users.clone(), name, org_id, owner_id),
        Request::EditWallet { wallet } => handle_edit_wallet(wallet),
        Request::RemoveWalletFromOrg { org_id, .. } => {
            handle_remove_wallet_from_org(orgs.clone(), org_id)
        }
        Request::EditXpub { wallet_id, .. } => handle_edit_xpub(wallets.clone(), wallet_id),
        _ => handle_unknown_request(),
    })
}

fn handle_fetch_org(orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>, id: Uuid) -> Response {
    let orgs_guard = orgs.lock().unwrap();
    if let Some(org) = orgs_guard.get(&id) {
        Response::Org {
            org: org.into(), // Use From<&Org> for OrgJson
        }
    } else {
        Response::Error {
            error: WssError {
                code: "NOT_FOUND".to_string(),
                message: format!("Org {} not found", id),
                request_id: None,
            },
        }
    }
}

fn handle_fetch_wallet(wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>, id: Uuid) -> Response {
    let wallets_guard = wallets.lock().unwrap();
    if let Some(wallet) = wallets_guard.get(&id) {
        Response::Wallet {
            wallet: wallet.into(), // Use From<&Wallet> for WalletJson
        }
    } else {
        Response::Error {
            error: WssError {
                code: "NOT_FOUND".to_string(),
                message: format!("Wallet {} not found", id),
                request_id: None,
            },
        }
    }
}

fn handle_fetch_user(users: Arc<Mutex<BTreeMap<Uuid, User>>>, id: Uuid) -> Response {
    let users_guard = users.lock().unwrap();
    if let Some(user) = users_guard.get(&id) {
        Response::User {
            user: user.into(), // Use From<&User> for UserJson
        }
    } else {
        Response::Error {
            error: WssError {
                code: "NOT_FOUND".to_string(),
                message: format!("User {} not found", id),
                request_id: None,
            },
        }
    }
}

fn handle_create_wallet(
    users: Arc<Mutex<BTreeMap<Uuid, User>>>,
    name: String,
    org_id: Uuid,
    owner_id: Uuid,
) -> Response {
    let users_guard = users.lock().unwrap();
    let owner = users_guard.get(&owner_id).cloned().unwrap_or_else(|| User {
        name: "Unknown User".to_string(),
        uuid: owner_id,
        email: "unknown@example.com".to_string(),
        orgs: Vec::new(),
        role: UserRole::Owner,
    });
    let wallet_id = Uuid::new_v4();
    let wallet = Wallet {
        alias: name.clone(),
        org: org_id,
        owner: owner.clone(),
        id: wallet_id,
        template: None,
        status: WalletStatus::Created,
    };
    Response::Wallet {
        wallet: (&wallet).into(),
    }
}

fn handle_edit_wallet(wallet: Wallet) -> Response {
    Response::Wallet {
        wallet: (&wallet).into(),
    }
}

fn handle_remove_wallet_from_org(orgs: Arc<Mutex<BTreeMap<Uuid, Org>>>, org_id: Uuid) -> Response {
    let orgs_guard = orgs.lock().unwrap();
    if let Some(org) = orgs_guard.get(&org_id) {
        Response::Org {
            org: org.into(),
        }
    } else {
        Response::Error {
            error: WssError {
                code: "NOT_FOUND".to_string(),
                message: format!("Org {} not found", org_id),
                request_id: None,
            },
        }
    }
}

fn handle_edit_xpub(wallets: Arc<Mutex<BTreeMap<Uuid, Wallet>>>, wallet_id: Uuid) -> Response {
    let wallets_guard = wallets.lock().unwrap();
    if let Some(wallet) = wallets_guard.get(&wallet_id) {
        Response::Wallet {
            wallet: wallet.into(),
        }
    } else {
        Response::Error {
            error: WssError {
                code: "NOT_FOUND".to_string(),
                message: format!("Wallet {} not found", wallet_id),
                request_id: None,
            },
        }
    }
}

fn handle_unknown_request() -> Response {
    Response::Error {
        error: WssError {
            code: "NOT_IMPLEMENTED".to_string(),
            message: "Request type not implemented".to_string(),
            request_id: None,
        },
    }
}

impl Backend for DevBackend {
    fn connect_ws(&mut self, url: String, version: u8) -> Option<channel::Receiver<Notification>> {
        self.client.set_token("dev-token".to_string());
        self.client.connect_ws(url, version)
    }

    fn ping(&mut self) {
        self.client.ping();
    }

    fn close(&mut self) {
        if let Some(_server) = self.server.take() {
            // server will be dropped and close automatically
        }
        self.client.close();
    }

    fn auth_request(&mut self, _email: String) {
        // In dev mode, always send auth code
        todo!()
    }

    fn auth_code(&mut self, code: String) {
        // Simulate code verification
        if code == self.auth_code {
            // Send orgs on successful auth - this would be handled by the Client
            todo!()
        }
    }

    fn get_orgs(&self) -> BTreeMap<Uuid, Org> {
        self.orgs.clone()
    }

    fn get_org(&self, id: Uuid) -> Option<OrgData> {
        let org = self.orgs.get(&id).cloned()?;
        let mut wallets = BTreeMap::new();
        for w in org.wallets.clone() {
            let wallet = self.wallets.get(&w).cloned()?;
            wallets.insert(w, wallet);
        }
        Some(OrgData {
            name: org.name,
            id,
            wallets,
            users: org.users,
            owners: org.owners,
        })
    }

    fn get_user(&self, id: Uuid) -> Option<User> {
        self.users.get(&id).cloned()
    }

    fn get_wallet(&self, id: Uuid) -> Option<Wallet> {
        self.wallets.get(&id).cloned()
    }

    fn fetch_org(&mut self, id: Uuid) {
        self.client.fetch_org(id);
    }

    fn remove_wallet_from_org(&mut self, wallet_id: Uuid, org_id: Uuid) {
        self.client.remove_wallet_from_org(wallet_id, org_id);
    }

    fn create_wallet(&mut self, name: String, org: Uuid, owner: Uuid) {
        self.client.create_wallet(name, org, owner);
    }

    fn edit_wallet(&mut self, wallet: Wallet) {
        self.client.edit_wallet(wallet);
    }

    fn fetch_wallet(&mut self, id: Uuid) {
        self.client.fetch_wallet(id);
    }

    fn edit_xpub(&mut self, wallet_id: Uuid, xpub: Option<DescriptorPublicKey>, key_id: u8) {
        self.client.edit_xpub(wallet_id, xpub, key_id);
    }

    fn fetch_user(&mut self, id: Uuid) {
        self.client.fetch_user(id);
    }
}
