use liana_connect::{Request, Response, User, UserRole, Wallet, WalletStatus, WssError};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::state::ServerState;

/// Derive the user's role for a specific wallet based on wallet data, user email, and global role
/// Returns None if the user has no access to this wallet
pub fn derive_user_role_for_wallet(
    wallet: &Wallet,
    user_email: &str,
    global_role: UserRole,
) -> Option<UserRole> {
    // WSManager has access to all wallets
    if global_role == UserRole::WSManager {
        return Some(UserRole::WSManager);
    }

    let email_lower = user_email.to_lowercase();
    // Check if user is wallet owner
    if wallet.owner.email.to_lowercase() == email_lower {
        return Some(UserRole::Owner);
    }
    // Check if user is a participant (has keys with matching email)
    if let Some(template) = &wallet.template {
        for key in template.keys.values() {
            if key.email.to_lowercase() == email_lower {
                return Some(UserRole::Participant);
            }
        }
    }
    // User has no access to this wallet
    None
}

/// Check if a user can access a wallet (has any role)
/// Participants cannot access Draft/Locked wallets
pub fn can_user_access_wallet(
    wallet: &Wallet,
    user_email: &str,
    global_role: UserRole,
) -> bool {
    match derive_user_role_for_wallet(wallet, user_email, global_role) {
        None => false, // No access
        Some(UserRole::Participant) => {
            // Participants cannot see Draft or Locked wallets
            !matches!(
                wallet.status,
                WalletStatus::Created | WalletStatus::Drafted | WalletStatus::Locked
            )
        }
        Some(_) => true, // WSManager and Owner can see all
    }
}

/// Get current unix timestamp in seconds
fn now_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Process a request and return a response
pub fn handle_request(request: Request, state: &ServerState, editor_id: Uuid) -> Response {
    match request {
        Request::GetServerTime => Response::ServerTime {
            timestamp: now_timestamp(),
        },
        Request::FetchOrg { id } => handle_fetch_org(state, id, editor_id),
        Request::FetchWallet { id } => handle_fetch_wallet(state, id, editor_id),
        Request::FetchUser { id } => handle_fetch_user(state, id),
        Request::CreateWallet {
            name,
            org_id,
            owner_id,
        } => handle_create_wallet(state, name, org_id, owner_id, editor_id),
        Request::EditWallet { wallet } => handle_edit_wallet(state, wallet, editor_id),
        Request::RemoveWalletFromOrg { org_id, wallet_id } => {
            handle_remove_wallet_from_org(state, org_id, wallet_id)
        }
        Request::EditXpub {
            wallet_id,
            key_id,
            xpub,
        } => handle_edit_xpub(state, wallet_id, key_id, xpub, editor_id),
        _ => handle_unknown_request(),
    }
}

fn handle_fetch_org(state: &ServerState, id: Uuid, editor_id: Uuid) -> Response {
    let orgs = state.orgs.lock().unwrap();
    if let Some(org) = orgs.get(&id) {
        // Get editor's email and global role
        let (editor_email, global_role) = {
            let users = state.users.lock().unwrap();
            users
                .get(&editor_id)
                .map(|u| (u.email.clone(), u.role.clone()))
                .unwrap_or_else(|| (String::new(), UserRole::Participant))
        };

        // Filter wallet IDs based on user's access
        let wallets = state.wallets.lock().unwrap();
        let filtered_wallet_ids: std::collections::BTreeSet<Uuid> = org
            .wallets
            .iter()
            .filter(|wallet_id| {
                if let Some(wallet) = wallets.get(wallet_id) {
                    // Check if user has access to this wallet
                    let role =
                        derive_user_role_for_wallet(wallet, &editor_email, global_role.clone());

                    match role {
                        None => false, // No access
                        Some(UserRole::Participant) => {
                            // Participants cannot see Draft or Locked wallets
                            !matches!(
                                wallet.status,
                                WalletStatus::Created | WalletStatus::Drafted | WalletStatus::Locked
                            )
                        }
                        Some(_) => true, // WSManager and Owner can see all
                    }
                } else {
                    false // Wallet not found, skip it
                }
            })
            .copied()
            .collect();

        // Create a modified org with filtered wallets
        let filtered_org = liana_connect::Org {
            name: org.name.clone(),
            id: org.id,
            wallets: filtered_wallet_ids,
            users: org.users.clone(),
            owners: org.owners.clone(),
            last_edited: org.last_edited,
            last_editor: org.last_editor,
        };

        Response::Org {
            org: (&filtered_org).into(),
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

fn handle_fetch_wallet(state: &ServerState, id: Uuid, editor_id: Uuid) -> Response {
    let wallets = state.wallets.lock().unwrap();
    if let Some(wallet) = wallets.get(&id) {
        // Get editor's email and global role
        let (editor_email, global_role) = {
            let users = state.users.lock().unwrap();
            users
                .get(&editor_id)
                .map(|u| (u.email.clone(), u.role.clone()))
                .unwrap_or_else(|| (String::new(), UserRole::Participant))
        };

        let role = derive_user_role_for_wallet(wallet, &editor_email, global_role);

        match role {
            None => {
                // User has no access to this wallet
                Response::Error {
                    error: WssError {
                        code: "ACCESS_DENIED".to_string(),
                        message: "You do not have access to this wallet".to_string(),
                        request_id: None,
                    },
                }
            }
            Some(UserRole::Participant)
                if matches!(
                    wallet.status,
                    WalletStatus::Created | WalletStatus::Drafted | WalletStatus::Locked
                ) =>
            {
                // Participants cannot access Draft or Locked wallets
                Response::Error {
                    error: WssError {
                        code: "ACCESS_DENIED".to_string(),
                        message: "Participants cannot access Draft or Locked wallets".to_string(),
                        request_id: None,
                    },
                }
            }
            Some(_) => Response::Wallet {
                wallet: wallet.into(),
            },
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
    editor_id: Uuid,
) -> Response {
    let timestamp = now_timestamp();
    let users = state.users.lock().unwrap();
    let owner = users.get(&owner_id).cloned().unwrap_or_else(|| User {
        name: "Unknown User".to_string(),
        uuid: owner_id,
        email: "unknown@example.com".to_string(),
        orgs: Vec::new(),
        role: UserRole::Owner,
        last_edited: Some(timestamp),
        last_editor: Some(editor_id),
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
        last_edited: Some(timestamp),
        last_editor: Some(editor_id),
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

fn handle_edit_wallet(state: &ServerState, mut wallet: Wallet, editor_id: Uuid) -> Response {
    let timestamp = now_timestamp();
    let mut wallets = state.wallets.lock().unwrap();

    // Check if wallet exists and get current state
    let existing = match wallets.get(&wallet.id) {
        Some(w) => w.clone(),
        None => {
            return Response::Error {
                error: WssError {
                    code: "NOT_FOUND".to_string(),
                    message: format!("Wallet {} not found", wallet.id),
                    request_id: None,
                },
            };
        }
    };

    // Get editor's role for permission checks
    let editor_role = {
        let users = state.users.lock().unwrap();
        users.get(&editor_id).map(|u| u.role.clone())
    };

    // Helper: check if template changed
    let template_changed = || -> bool {
        match (&existing.template, &wallet.template) {
            (Some(old), Some(new)) => {
                old.keys != new.keys
                    || old.primary_path != new.primary_path
                    || old.secondary_paths != new.secondary_paths
            }
            (None, Some(_)) | (Some(_), None) => true,
            (None, None) => false,
        }
    };

    // Role-based permission checks based on current wallet status
    match existing.status {
        WalletStatus::Created | WalletStatus::Drafted => {
            // Only WSManager can edit or lock
            if editor_role != Some(UserRole::WSManager) {
                return Response::Error {
                    error: WssError {
                        code: "ACCESS_DENIED".to_string(),
                        message: "Only WSManager can edit draft wallets".to_string(),
                        request_id: None,
                    },
                };
            }
            // WSManager can: edit template, change status to Locked (or keep as Drafted)
            if wallet.status != WalletStatus::Created
                && wallet.status != WalletStatus::Drafted
                && wallet.status != WalletStatus::Locked
            {
                return Response::Error {
                    error: WssError {
                        code: "ACCESS_DENIED".to_string(),
                        message: "Invalid status transition from draft".to_string(),
                        request_id: None,
                    },
                };
            }
        }
        WalletStatus::Locked => {
            // WSManager can: unlock (→Drafted) without template changes
            // Owner can: validate (→Validated) without template changes
            match editor_role {
                Some(UserRole::WSManager) => {
                    // WSManager can only unlock (change status to Drafted)
                    if wallet.status != WalletStatus::Drafted {
                        return Response::Error {
                            error: WssError {
                                code: "ACCESS_DENIED".to_string(),
                                message: "WSManager can only unlock (revert to Draft) a locked wallet".to_string(),
                                request_id: None,
                            },
                        };
                    }
                    if template_changed() {
                        return Response::Error {
                            error: WssError {
                                code: "ACCESS_DENIED".to_string(),
                                message: "Template cannot be modified when unlocking".to_string(),
                                request_id: None,
                            },
                        };
                    }
                }
                Some(UserRole::Owner) => {
                    // Owner can only validate (change status to Validated)
                    if wallet.status != WalletStatus::Validated {
                        return Response::Error {
                            error: WssError {
                                code: "ACCESS_DENIED".to_string(),
                                message: "Owner can only validate a locked wallet".to_string(),
                                request_id: None,
                            },
                        };
                    }
                    if template_changed() {
                        return Response::Error {
                            error: WssError {
                                code: "ACCESS_DENIED".to_string(),
                                message: "Template cannot be modified during validation".to_string(),
                                request_id: None,
                            },
                        };
                    }
                }
                _ => {
                    return Response::Error {
                        error: WssError {
                            code: "ACCESS_DENIED".to_string(),
                            message: "Only WSManager or Owner can modify a locked wallet".to_string(),
                            request_id: None,
                        },
                    };
                }
            }
        }
        WalletStatus::Validated => {
            // Template is locked, only xpub edits allowed (via edit_xpub endpoint)
            if template_changed() {
                return Response::Error {
                    error: WssError {
                        code: "ACCESS_DENIED".to_string(),
                        message: "Template cannot be modified after validation".to_string(),
                        request_id: None,
                    },
                };
            }
        }
        WalletStatus::Finalized => {
            // Finalized wallets are immutable
            return Response::Error {
                error: WssError {
                    code: "ACCESS_DENIED".to_string(),
                    message: "Finalized wallets cannot be modified".to_string(),
                    request_id: None,
                },
            };
        }
    }

    // Validate alias
    if wallet.alias.trim().is_empty() {
        return Response::Error {
            error: WssError {
                code: "VALIDATION_ERROR".to_string(),
                message: "Wallet alias cannot be empty".to_string(),
                request_id: None,
            },
        };
    }

    // Validate template constraints
    if let Some(template) = &wallet.template {
        // Validate primary path
        if let Err(e) = validate_spending_path(&template.primary_path) {
            return Response::Error {
                error: WssError {
                    code: "VALIDATION_ERROR".to_string(),
                    message: format!("Primary path: {}", e),
                    request_id: None,
                },
            };
        }

        // Validate secondary paths
        for (i, (path, _timelock)) in template.secondary_paths.iter().enumerate() {
            if let Err(e) = validate_spending_path(path) {
                return Response::Error {
                    error: WssError {
                        code: "VALIDATION_ERROR".to_string(),
                        message: format!("Secondary path {}: {}", i + 1, e),
                        request_id: None,
                    },
                };
            }
        }
    }

    // Set last_edited and last_editor on wallet
    wallet.last_edited = Some(timestamp);
    wallet.last_editor = Some(editor_id);

    // Set last_edited and last_editor on changed paths
    if let Some(ref mut new_template) = wallet.template {
        let old_template = existing.template.as_ref();

        // Check primary path - compare ignoring last_edited/last_editor
        let primary_changed = match old_template {
            Some(old) => {
                old.primary_path.key_ids != new_template.primary_path.key_ids
                    || old.primary_path.threshold_n != new_template.primary_path.threshold_n
            }
            None => true, // New template, path is new
        };
        if primary_changed {
            new_template.primary_path.last_edited = Some(timestamp);
            new_template.primary_path.last_editor = Some(editor_id);
        } else if let Some(old) = old_template {
            // Preserve existing last_edited info
            new_template.primary_path.last_edited = old.primary_path.last_edited;
            new_template.primary_path.last_editor = old.primary_path.last_editor;
        }

        // Check secondary paths
        for (i, (new_path, new_timelock)) in new_template.secondary_paths.iter_mut().enumerate() {
            let path_changed = match old_template {
                Some(old) => {
                    // Try to find a matching old path by index
                    old.secondary_paths.get(i).map_or(true, |(old_path, old_timelock)| {
                        old_path.key_ids != new_path.key_ids
                            || old_path.threshold_n != new_path.threshold_n
                            || old_timelock.blocks != new_timelock.blocks
                    })
                }
                None => true, // New template, path is new
            };
            if path_changed {
                new_path.last_edited = Some(timestamp);
                new_path.last_editor = Some(editor_id);
            } else if let Some(old) = old_template {
                // Preserve existing last_edited info
                if let Some((old_path, _)) = old.secondary_paths.get(i) {
                    new_path.last_edited = old_path.last_edited;
                    new_path.last_editor = old_path.last_editor;
                }
            }
        }

        // Check keys - compare ignoring last_edited/last_editor and xpub fields
        for (key_id, new_key) in new_template.keys.iter_mut() {
            let key_changed = match old_template {
                Some(old) => {
                    old.keys.get(key_id).map_or(true, |old_key| {
                        old_key.alias != new_key.alias
                            || old_key.description != new_key.description
                            || old_key.email != new_key.email
                            || old_key.key_type != new_key.key_type
                    })
                }
                None => true, // New template, key is new
            };
            if key_changed {
                new_key.last_edited = Some(timestamp);
                new_key.last_editor = Some(editor_id);
            } else if let Some(old) = old_template {
                // Preserve existing last_edited info
                if let Some(old_key) = old.keys.get(key_id) {
                    new_key.last_edited = old_key.last_edited;
                    new_key.last_editor = old_key.last_editor;
                }
            }
        }
    }

    // Update wallet in state
    wallets.insert(wallet.id, wallet.clone());
    drop(wallets);

    Response::Wallet {
        wallet: (&wallet).into(),
    }
}

fn validate_spending_path(path: &liana_connect::SpendingPath) -> Result<(), String> {
    if path.threshold_n == 0 {
        return Err("threshold must be greater than 0".to_string());
    }
    if path.threshold_n as usize > path.key_ids.len() {
        return Err(format!(
            "threshold ({}) cannot exceed number of keys ({})",
            path.threshold_n,
            path.key_ids.len()
        ));
    }
    Ok(())
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
    xpub_json: Option<liana_connect::XpubJson>,
    editor_id: Uuid,
) -> Response {
    let timestamp = now_timestamp();
    let mut wallets = state.wallets.lock().unwrap();
    if let Some(wallet) = wallets.get_mut(&wallet_id) {
        // Locked and Finalized wallets cannot have xpub edits
        if wallet.status == WalletStatus::Locked {
            return Response::Error {
                error: WssError {
                    code: "ACCESS_DENIED".to_string(),
                    message: "Locked wallets cannot be modified".to_string(),
                    request_id: None,
                },
            };
        }
        if wallet.status == WalletStatus::Finalized {
            return Response::Error {
                error: WssError {
                    code: "ACCESS_DENIED".to_string(),
                    message: "Finalized wallets cannot be modified".to_string(),
                    request_id: None,
                },
            };
        }

        // Parse xpub from JSON if provided
        let xpub: Option<miniscript::DescriptorPublicKey> = match &xpub_json {
            Some(json) => match json.value.parse() {
                Ok(xpub) => Some(xpub),
                Err(e) => {
                    return Response::Error {
                        error: WssError {
                            code: "INVALID_XPUB".to_string(),
                            message: format!("Invalid xpub format: {}", e),
                            request_id: None,
                        },
                    };
                }
            },
            None => None,
        };

        // Update the xpub for the specified key
        if let Some(template) = &mut wallet.template {
            if let Some(key) = template.keys.get_mut(&key_id) {
                key.xpub = xpub;
                // Store xpub source info for audit
                if let Some(ref json) = xpub_json {
                    key.xpub_source = Some(json.source.clone());
                    key.xpub_device_kind = json.device_kind.clone();
                    key.xpub_device_fingerprint = json.device_fingerprint.clone();
                    key.xpub_device_version = json.device_version.clone();
                    key.xpub_file_name = json.file_name.clone();
                } else {
                    // Clear source info when xpub is cleared
                    key.xpub_source = None;
                    key.xpub_device_kind = None;
                    key.xpub_device_fingerprint = None;
                    key.xpub_device_version = None;
                    key.xpub_file_name = None;
                }
                key.last_edited = Some(timestamp);
                key.last_editor = Some(editor_id);
            }
        }

        // Update wallet timestamps
        wallet.last_edited = Some(timestamp);
        wallet.last_editor = Some(editor_id);

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

#[cfg(test)]
mod tests {
    use super::*;
    use liana_connect::{Key, KeyType, PolicyTemplate};

    fn make_test_wallet(
        owner_email: &str,
        key_emails: &[&str],
        status: WalletStatus,
    ) -> Wallet {
        let mut template = PolicyTemplate::new();
        for (i, email) in key_emails.iter().enumerate() {
            template.keys.insert(
                i as u8,
                Key {
                    id: i as u8,
                    alias: format!("Key {}", i),
                    description: String::new(),
                    email: email.to_string(),
                    key_type: KeyType::External,
                    xpub: None,
                    xpub_source: None,
                    xpub_device_kind: None,
                    xpub_device_fingerprint: None,
                    xpub_device_version: None,
                    xpub_file_name: None,
                    last_edited: None,
                    last_editor: None,
                },
            );
        }

        Wallet {
            alias: "Test Wallet".to_string(),
            org: uuid::Uuid::new_v4(),
            owner: User {
                name: "Owner".to_string(),
                uuid: uuid::Uuid::new_v4(),
                email: owner_email.to_string(),
                orgs: vec![],
                role: UserRole::Owner,
                last_edited: None,
                last_editor: None,
            },
            id: uuid::Uuid::new_v4(),
            template: Some(template),
            status,
            last_edited: None,
            last_editor: None,
        }
    }

    #[test]
    fn test_wsmanager_can_access_all_wallets() {
        // WSManager should access all wallet statuses
        let statuses = [
            WalletStatus::Created,
            WalletStatus::Drafted,
            WalletStatus::Locked,
            WalletStatus::Validated,
            WalletStatus::Finalized,
        ];

        for status in statuses {
            let wallet = make_test_wallet("owner@example.com", &["alice@example.com"], status.clone());

            // WSManager with global role should have access
            let role = derive_user_role_for_wallet(&wallet, "ws@example.com", UserRole::WSManager);
            assert_eq!(role, Some(UserRole::WSManager), "WSManager should get WSManager role for {:?}", status);

            let can_access = can_user_access_wallet(&wallet, "ws@example.com", UserRole::WSManager);
            assert!(can_access, "WSManager should access {:?} wallet", status);
        }
    }

    #[test]
    fn test_participant_cannot_access_draft_locked() {
        // Participant cannot access Draft/Locked wallets
        let draft_wallet = make_test_wallet("owner@example.com", &["participant@example.com"], WalletStatus::Drafted);
        let locked_wallet = make_test_wallet("owner@example.com", &["participant@example.com"], WalletStatus::Locked);
        let created_wallet = make_test_wallet("owner@example.com", &["participant@example.com"], WalletStatus::Created);

        // Participant has key but cannot access draft/locked
        assert!(!can_user_access_wallet(&draft_wallet, "participant@example.com", UserRole::Participant));
        assert!(!can_user_access_wallet(&locked_wallet, "participant@example.com", UserRole::Participant));
        assert!(!can_user_access_wallet(&created_wallet, "participant@example.com", UserRole::Participant));
    }

    #[test]
    fn test_participant_can_access_validated_finalized() {
        // Participant CAN access Validated/Finalized wallets
        let validated_wallet = make_test_wallet("owner@example.com", &["participant@example.com"], WalletStatus::Validated);
        let finalized_wallet = make_test_wallet("owner@example.com", &["participant@example.com"], WalletStatus::Finalized);

        assert!(can_user_access_wallet(&validated_wallet, "participant@example.com", UserRole::Participant));
        assert!(can_user_access_wallet(&finalized_wallet, "participant@example.com", UserRole::Participant));
    }

    #[test]
    fn test_user_without_access_cannot_see_wallet() {
        // User with no keys and not owner should not access
        let wallet = make_test_wallet("owner@example.com", &["alice@example.com"], WalletStatus::Validated);

        // User without keys, with Participant global role
        let role = derive_user_role_for_wallet(&wallet, "random@example.com", UserRole::Participant);
        assert_eq!(role, None, "User without keys should have no role");

        assert!(!can_user_access_wallet(&wallet, "random@example.com", UserRole::Participant));
    }

    #[test]
    fn test_owner_can_access_all_statuses() {
        let statuses = [
            WalletStatus::Created,
            WalletStatus::Drafted,
            WalletStatus::Locked,
            WalletStatus::Validated,
            WalletStatus::Finalized,
        ];

        for status in statuses {
            let wallet = make_test_wallet("owner@example.com", &["alice@example.com"], status.clone());

            // Owner (global role Owner, is wallet owner)
            let role = derive_user_role_for_wallet(&wallet, "owner@example.com", UserRole::Owner);
            assert_eq!(role, Some(UserRole::Owner), "Owner should get Owner role for {:?}", status);

            assert!(can_user_access_wallet(&wallet, "owner@example.com", UserRole::Owner),
                "Owner should access {:?} wallet", status);
        }
    }
}

