use liana_connect::{Request, Response, User, UserRole, Wallet, WalletStatus, WssError};
use uuid::Uuid;

use crate::state::ServerState;

/// Process a request and return a response
pub fn handle_request(request: Request, state: &ServerState) -> Response {
    match request {
        Request::FetchOrg { id } => handle_fetch_org(state, id),
        Request::FetchWallet { id } => handle_fetch_wallet(state, id),
        Request::FetchUser { id } => handle_fetch_user(state, id),
        Request::CreateWallet {
            name,
            org_id,
            owner_id,
        } => handle_create_wallet(state, name, org_id, owner_id),
        Request::EditWallet { wallet } => handle_edit_wallet(state, wallet),
        Request::RemoveWalletFromOrg { org_id, wallet_id } => {
            handle_remove_wallet_from_org(state, org_id, wallet_id)
        }
        Request::EditXpub {
            wallet_id,
            key_id,
            xpub,
        } => handle_edit_xpub(state, wallet_id, key_id, xpub),
        _ => handle_unknown_request(),
    }
}

fn handle_fetch_org(state: &ServerState, id: Uuid) -> Response {
    let orgs = state.orgs.lock().unwrap();
    if let Some(org) = orgs.get(&id) {
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

fn handle_fetch_wallet(state: &ServerState, id: Uuid) -> Response {
    let wallets = state.wallets.lock().unwrap();
    if let Some(wallet) = wallets.get(&id) {
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

fn handle_fetch_user(state: &ServerState, id: Uuid) -> Response {
    let users = state.users.lock().unwrap();
    if let Some(user) = users.get(&id) {
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
    state: &ServerState,
    name: String,
    org_id: Uuid,
    owner_id: Uuid,
) -> Response {
    let users = state.users.lock().unwrap();
    let owner = users.get(&owner_id).cloned().unwrap_or_else(|| User {
        name: "Unknown User".to_string(),
        uuid: owner_id,
        email: "unknown@example.com".to_string(),
        orgs: Vec::new(),
        role: UserRole::Owner,
    });
    drop(users);

    let wallet_id = Uuid::new_v4();
    let wallet = Wallet {
        alias: name.clone(),
        org: org_id,
        owner: owner.clone(),
        id: wallet_id,
        template: None,
        status: WalletStatus::Created,
    };

    // Store wallet in state
    let mut wallets = state.wallets.lock().unwrap();
    wallets.insert(wallet_id, wallet.clone());
    drop(wallets);

    // Add wallet to org
    let mut orgs = state.orgs.lock().unwrap();
    if let Some(org) = orgs.get_mut(&org_id) {
        org.wallets.insert(wallet_id);
    }
    drop(orgs);

    Response::Wallet {
        wallet: (&wallet).into(),
    }
}

fn handle_edit_wallet(state: &ServerState, wallet: Wallet) -> Response {
    // Update wallet in state
    let mut wallets = state.wallets.lock().unwrap();
    wallets.insert(wallet.id, wallet.clone());
    drop(wallets);

    Response::Wallet {
        wallet: (&wallet).into(),
    }
}

fn handle_remove_wallet_from_org(state: &ServerState, org_id: Uuid, wallet_id: Uuid) -> Response {
    let mut orgs = state.orgs.lock().unwrap();
    if let Some(org) = orgs.get_mut(&org_id) {
        org.wallets.remove(&wallet_id);
        let response = Response::Org {
            org: (&*org).into(),
        };
        drop(orgs);
        response
    } else {
        drop(orgs);
        Response::Error {
            error: WssError {
                code: "NOT_FOUND".to_string(),
                message: format!("Org {} not found", org_id),
                request_id: None,
            },
        }
    }
}

fn handle_edit_xpub(
    state: &ServerState,
    wallet_id: Uuid,
    key_id: u8,
    xpub: Option<miniscript::DescriptorPublicKey>,
) -> Response {
    let mut wallets = state.wallets.lock().unwrap();
    if let Some(wallet) = wallets.get_mut(&wallet_id) {
        // Update the xpub for the specified key
        if let Some(template) = &mut wallet.template {
            if let Some(key) = template.keys.get_mut(&key_id) {
                key.xpub = xpub;
            }
        }
        let response = Response::Wallet {
            wallet: (&*wallet).into(),
        };
        drop(wallets);
        response
    } else {
        drop(wallets);
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

