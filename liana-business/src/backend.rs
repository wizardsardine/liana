use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::task::{Context, Poll};

use iced::futures::Stream;
use miniscript::DescriptorPublicKey;
use std::sync::mpsc;
use thiserror::Error;
use uuid::Uuid;

use crate::models::PolicyTemplate;

#[derive(Debug, Clone)]
pub enum WalletStatus {
    Created,   // Empty
    Drafted,   // Draft by WS manager
    Validated, // Policy validated by owner, keys metadata not yet completed
    Finalized, // All key metadata filled, ready for prod
}

#[derive(Debug, Clone)]
pub struct Org {
    pub name: String,
    pub id: Uuid,
    pub wallets: BTreeSet<Uuid>,
    pub users: BTreeSet<Uuid>,
    pub owners: Vec<Uuid>,
}

#[allow(unused)]
#[derive(Debug, Clone)]
pub struct OrgData {
    pub name: String,
    pub id: Uuid,
    pub wallets: BTreeMap<Uuid, Wallet>,
    pub users: BTreeSet<Uuid>,
    pub owners: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct User {
    pub name: String,
    pub uuid: Uuid,
    pub email: String,
    pub orgs: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct Wallet {
    pub alias: String,
    pub org: Uuid,
    pub owner: User,
    pub id: Uuid,
    pub status: WalletStatus,
    pub template: Option<PolicyTemplate>,
}

#[derive(Debug, Clone, Error)]
pub enum Error {
    #[error("")]
    None,
    #[error("Iced subscription failed!")]
    SubscriptionFailed,
}

impl Error {
    pub fn show_warning(&self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone)]
pub enum Response {
    Connected { version: u8 },
    Pong,
    // email login step
    AuthCodeSent, // auth code has been send to email
    InvalidEmail, // the backend refuse you to connect with this email
    AuthCodeFail, // fail

    // code login step
    LoginSuccess,
    LoginFail,

    // Notifications
    Org(Org),
    Wallet(Wallet),
    User(User),

    // Orgs management responses
    Orgs(BTreeMap<Uuid, Org>),

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

    // Connection (WSS)
    fn connect(&mut self, url: String, version: u8) -> mpsc::Receiver<Response>; // -> Response::Connected
    fn ping(&mut self); // -> Response::Pong
    fn close(&mut self);    // Connection closed

    // Org management (WSS)
    fn create_org(&mut self, name: String);                                 // -> Response::Org
    fn fetch_org(&mut self, id: Uuid);                                      // -> Response::Org
    fn add_user_to_org(&mut self, user_id: Uuid, org_id: Uuid);             // -> Response::Org
    fn remove_user_from_org(&mut self, user_id: Uuid, org_id: Uuid);        // -> Response::Org
    fn remove_wallet_from_org(&mut self, wallet_id: Uuid, org_id: Uuid);    // -> Response::Org

    fn create_wallet(&mut self, name: String, org: Uuid, owner: Uuid);      // -> Response::Wallet
    fn edit_wallet(&mut self, wallet: Wallet);                              // -> Response::Wallet
    fn fetch_wallet(&mut self, id: Uuid);                                   // -> Response::Wallet
    fn edit_xpub(
        &mut self,
        wallet_id: Uuid,
        xpub: Option<DescriptorPublicKey>,
        key_id: u8);                                                     // -> Response::Wallet

    fn create_user(&mut self, name: String);    // Response::User
    fn edit_user(&mut self, user: User);        // -> Response::User
    fn fetch_user(&mut self, id: Uuid);         // -> Response::User

}

/// Stream wrapper for Backend responses
pub struct BackendStream {
    pub receiver: mpsc::Receiver<Response>,
}

impl Stream for BackendStream {
    type Item = Response;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // Use try_recv for non-blocking check
        match self.receiver.try_recv() {
            Ok(item) => Poll::Ready(Some(item)),
            Err(mpsc::TryRecvError::Empty) => Poll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => Poll::Ready(None),
        }
    }
}

/// Mock backend implementation for testing
#[derive(Debug)]
pub struct MockBackend {
    pub sender: Option<mpsc::Sender<Response>>,
    orgs: BTreeMap<Uuid, Org>,
    wallets: BTreeMap<Uuid, Wallet>,
    users: BTreeMap<Uuid, User>,
    connected: bool,
    auth_code: String, // Simple mock: always accepts "123456"
}

impl MockBackend {
    pub fn new() -> Self {
        let mut backend = Self {
            sender: None,
            orgs: BTreeMap::new(),
            wallets: BTreeMap::new(),
            users: BTreeMap::new(),
            connected: false,
            auth_code: "123456".to_string(),
        };

        // Initialize with predefined test data
        backend.init_test_data();
        backend
    }

    fn init_test_data(&mut self) {
        use crate::models::{Key, KeyType};

        // Create test user
        let user1 = User {
            name: "Test User".to_string(),
            uuid: Uuid::new_v4(),
            email: "test@example.com".to_string(),
            orgs: Vec::new(),
        };
        self.users.insert(user1.uuid, user1.clone());

        // Create Org 1: "Acme Corp"
        let org1_id = Uuid::new_v4();
        let mut org1_wallets = BTreeSet::new();

        // Wallet 1 for Org 1
        let wallet1_id = Uuid::new_v4();
        let mut wallet1_template = PolicyTemplate::new();
        wallet1_template.keys.insert(
            0,
            Key {
                id: 0,
                alias: "Main Key".to_string(),
                description: "Primary signing key".to_string(),
                email: "key1@example.com".to_string(),
                key_type: KeyType::Internal,
                xpub: None,
            },
        );
        wallet1_template.primary_path.key_ids.push(0);
        wallet1_template.primary_path.threshold_n = 1;

        let wallet1 = Wallet {
            alias: "Main Wallet".to_string(),
            org: org1_id,
            owner: user1.clone(),
            id: wallet1_id,
            template: Some(wallet1_template),
            status: WalletStatus::Created,
        };
        org1_wallets.insert(wallet1_id);
        self.wallets.insert(wallet1_id, wallet1);

        let org1 = Org {
            name: "Acme Corp".to_string(),
            id: org1_id,
            wallets: org1_wallets,
            users: Default::default(),
            owners: Default::default(),
        };
        self.orgs.insert(org1_id, org1);

        // Create Org 2: "Tech Solutions"
        let org2_id = Uuid::new_v4();
        let mut org2_wallets = BTreeSet::new();

        // Wallet 1 for Org 2
        let wallet2_id = Uuid::new_v4();
        let mut wallet2_template = PolicyTemplate::new();
        wallet2_template.keys.insert(
            0,
            Key {
                id: 0,
                alias: "Admin Key".to_string(),
                description: "Administrative key".to_string(),
                email: "admin@example.com".to_string(),
                key_type: KeyType::Internal,
                xpub: None,
            },
        );
        wallet2_template.keys.insert(
            1,
            Key {
                id: 1,
                alias: "Backup Key".to_string(),
                description: "Backup signing key".to_string(),
                email: "backup@example.com".to_string(),
                key_type: KeyType::External,
                xpub: None,
            },
        );
        wallet2_template.primary_path.key_ids.push(0);
        wallet2_template.primary_path.key_ids.push(1);
        wallet2_template.primary_path.threshold_n = 2;

        let wallet2 = Wallet {
            alias: "Company Wallet".to_string(),
            org: org2_id,
            owner: user1.clone(),
            id: wallet2_id,
            template: Some(wallet2_template),
            status: WalletStatus::Created,
        };
        org2_wallets.insert(wallet2_id);
        self.wallets.insert(wallet2_id, wallet2);

        let org2 = Org {
            name: "Tech Solutions".to_string(),
            id: org2_id,
            wallets: org2_wallets,
            users: Default::default(),
            owners: Default::default(),
        };
        self.orgs.insert(org2_id, org2);

        // Create Org 3: "Startup Inc"
        let org3_id = Uuid::new_v4();
        let org3 = Org {
            name: "Startup Inc".to_string(),
            id: org3_id,
            wallets: BTreeSet::new(),
            users: Default::default(),
            owners: Default::default(),
        };
        self.orgs.insert(org3_id, org3);
    }
}

impl Backend for MockBackend {
    fn connect(&mut self, _url: String, version: u8) -> mpsc::Receiver<Response> {
        self.connected = true;
        let (sender, receiver) = mpsc::channel();
        let _ = sender.send(Response::Connected { version });
        self.sender = Some(sender);
        receiver
    }

    fn ping(&mut self) {
        if self.connected {
            if let Some(sender) = &self.sender {
                let _ = sender.send(Response::Pong);
            }
        }
    }

    fn close(&mut self) {
        self.connected = false;
    }

    fn auth_request(&mut self, _email: String) {
        if let Some(sender) = &self.sender {
            sender.send(Response::AuthCodeSent).unwrap();
        } else {
            panic!("auth_token()");
        }
    }

    fn auth_code(&mut self, code: String) {
        // Simulate code verification
        if let Some(sender) = &self.sender {
            if code == self.auth_code {
                // Send orgs on successful auth
                let _ = sender.send(Response::LoginSuccess);
                let _ = sender.send(Response::Orgs(self.orgs.clone()));
            } else {
                let _ = sender.send(Response::LoginFail);
            }
        }
    }

    fn create_org(&mut self, name: String) {
        let org_id = Uuid::new_v4();
        let org = Org {
            name: name.clone(),
            id: org_id,
            wallets: BTreeSet::new(),
            users: Default::default(),
            owners: Default::default(),
        };
        self.orgs.insert(org_id, org.clone());
        if let Some(sender) = &self.sender {
            let _ = sender.send(Response::Org(org));
        }
    }

    fn edit_wallet(&mut self, _wallet: Wallet) {
        todo!()
    }

    fn get_org(&self, id: Uuid) -> Option<OrgData> {
        let org = self.orgs.get(&id).cloned()?;
        let mut wallets = BTreeMap::new();
        for w in org.wallets {
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

    fn get_orgs(&self) -> BTreeMap<Uuid, Org> {
        self.orgs.clone()
    }

    fn fetch_org(&mut self, _id: Uuid) {
        todo!()
    }

    fn add_user_to_org(&mut self, _user_id: Uuid, _org_id: Uuid) {
        todo!()
    }

    fn remove_user_from_org(&mut self, _user_id: Uuid, _org_id: Uuid) {
        todo!()
    }

    fn remove_wallet_from_org(&mut self, _wallet_id: Uuid, _org_id: Uuid) {
        todo!()
    }

    fn create_wallet(&mut self, _name: String, _org: Uuid, _owner: Uuid) {
        todo!()
    }

    fn fetch_wallet(&mut self, _id: Uuid) {
        todo!()
    }

    fn edit_xpub(&mut self, _wallet_id: Uuid, _xpub: Option<DescriptorPublicKey>, _key_id: u8) {
        todo!()
    }

    fn create_user(&mut self, _name: String) {
        todo!()
    }

    fn edit_user(&mut self, _user: User) {
        todo!()
    }

    fn fetch_user(&mut self, _id: Uuid) {
        todo!()
    }
}
