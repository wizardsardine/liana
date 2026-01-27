#[cfg(test)]
use liana_connect::ws_business::KeyIdentity;
use liana_connect::ws_business::{
    RegistrationInfos, Request, Response, SpendingPath, User, UserRole, Wallet,
    WalletStatus, WssError, Xpub,
};
use std::time::{SystemTime, UNIX_EPOCH};
use log::{debug, error};
use uuid::Uuid;

use crate::state::ServerState;

/// Check if a user can access a wallet (has any role)
/// Participants cannot access Draft/Locked wallets
pub fn can_user_access_wallet(user: &User, wallet: &Wallet) -> bool {
    match user.role(wallet) {
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
        Request::FetchOrg { id } => handle_fetch_org(state, id, editor_id),
        Request::FetchWallet { id } => handle_fetch_wallet(state, id, editor_id),
        Request::FetchUser { id } => handle_fetch_user(state, id),
        Request::EditWallet { wallet } => handle_edit_wallet(state, wallet, editor_id),
        Request::EditXpub {
            wallet_id,
            key_id,
            xpub,
        } => handle_edit_xpub(state, wallet_id, key_id, xpub, editor_id),
        Request::DeviceRegistered { wallet_id, infos } => {
            handle_device_registered(state, wallet_id, infos, editor_id)
        }
        _ => handle_unknown_request(),
    }
}

fn handle_fetch_org(state: &ServerState, id: Uuid, editor_id: Uuid) -> Response {
    let orgs = state.orgs.lock().unwrap();
    if let Some(org) = orgs.get(&id) {
        // Get editor user
        let editor = {
            let users = state.users.lock().unwrap();
            users.get(&editor_id).cloned()
        };

        // Filter wallet IDs based on user's access
        let wallets = state.wallets.lock().unwrap();
        let filtered_wallet_ids: std::collections::BTreeSet<Uuid> = org
            .wallets
            .iter()
            .filter(|wallet_id| {
                if let Some(wallet) = wallets.get(wallet_id) {
                    // Check if user has access to this wallet
                    let role = editor.as_ref().and_then(|u| u.role(wallet));

                    match role {
                        None => false, // No access
                        Some(UserRole::Participant) => {
                            // Participants cannot see Draft or Locked wallets
                            !matches!(
                                wallet.status,
                                WalletStatus::Created
                                    | WalletStatus::Drafted
                                    | WalletStatus::Locked
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
        let filtered_org = liana_connect::ws_business::Org {
            name: org.name.clone(),
            id: org.id,
            wallets: filtered_wallet_ids,
            users: org.users.clone(),
            owners: org.owners.clone(),
            last_edited: org.last_edited,
            last_editor: org.last_editor,
        };

        Response::Org { org: filtered_org }
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
        // Get editor's derived role for this wallet
        let role = {
            let users = state.users.lock().unwrap();
            users.get(&editor_id).and_then(|u| u.role(wallet))
        };

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
                wallet: wallet.clone(),
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
        Response::User { user: user.clone() }
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

    // Get editor's derived role for this specific wallet
    let editor_role = {
        let users = state.users.lock().unwrap();
        users.get(&editor_id).and_then(|u| u.role(&existing))
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

    // Helper: check if only key metadata changed (not paths)
    let only_keys_changed = || -> bool {
        match (&existing.template, &wallet.template) {
            (Some(old), Some(new)) => {
                // Paths must be unchanged
                old.primary_path == new.primary_path && old.secondary_paths == new.secondary_paths
            }
            (None, None) => true,
            _ => false, // Template added or removed
        }
    };

    // Check if wallet is in registration state (descriptor set but not finalized)
    if existing.descriptor.is_some() && existing.status != WalletStatus::Finalized {
        return Response::Error {
            error: WssError {
                code: "ACCESS_DENIED".to_string(),
                message: "Wallets in registration cannot be modified".to_string(),
                request_id: None,
            },
        };
    }

    // Role-based permission checks based on current wallet status
    match existing.status {
        WalletStatus::Created | WalletStatus::Drafted => {
            match editor_role {
                Some(UserRole::WizardSardineAdmin) => {
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
                Some(UserRole::WalletManager) => {
                    // Owner can only edit key metadata and create keys, not paths
                    if !only_keys_changed() {
                        return Response::Error {
                            error: WssError {
                                code: "ACCESS_DENIED".to_string(),
                                message: "Owner can only edit key metadata, not paths".to_string(),
                                request_id: None,
                            },
                        };
                    }
                    // Owner cannot change wallet status
                    if wallet.status != existing.status {
                        return Response::Error {
                            error: WssError {
                                code: "ACCESS_DENIED".to_string(),
                                message: "Owner cannot change wallet status".to_string(),
                                request_id: None,
                            },
                        };
                    }
                }
                _ => {
                    return Response::Error {
                        error: WssError {
                            code: "ACCESS_DENIED".to_string(),
                            message: "Only WSManager or Owner can edit draft wallets".to_string(),
                            request_id: None,
                        },
                    };
                }
            }
        }
        WalletStatus::Locked => {
            // WSManager can: unlock (→Drafted) without template changes
            // Owner can: validate (→Validated) without template changes
            match editor_role {
                Some(UserRole::WizardSardineAdmin) => {
                    // WSManager can only unlock (change status to Drafted)
                    if wallet.status != WalletStatus::Drafted {
                        return Response::Error {
                            error: WssError {
                                code: "ACCESS_DENIED".to_string(),
                                message:
                                    "WSManager can only unlock (revert to Draft) a locked wallet"
                                        .to_string(),
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
                Some(UserRole::WalletManager) => {
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
                                message: "Template cannot be modified during validation"
                                    .to_string(),
                                request_id: None,
                            },
                        };
                    }
                }
                _ => {
                    return Response::Error {
                        error: WssError {
                            code: "ACCESS_DENIED".to_string(),
                            message: "Only WSManager or Owner can modify a locked wallet"
                                .to_string(),
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
        for (i, sp) in template.secondary_paths.iter().enumerate() {
            if let Err(e) = validate_spending_path(&sp.path) {
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
        for (i, new_sp) in new_template.secondary_paths.iter_mut().enumerate() {
            let path_changed = match old_template {
                Some(old) => {
                    // Try to find a matching old path by index
                    old.secondary_paths.get(i).is_none_or(|old_sp| {
                        old_sp.path.key_ids != new_sp.path.key_ids
                            || old_sp.path.threshold_n != new_sp.path.threshold_n
                            || old_sp.timelock.blocks != new_sp.timelock.blocks
                    })
                }
                None => true, // New template, path is new
            };
            if path_changed {
                new_sp.path.last_edited = Some(timestamp);
                new_sp.path.last_editor = Some(editor_id);
            } else if let Some(old) = old_template {
                // Preserve existing last_edited info
                if let Some(old_sp) = old.secondary_paths.get(i) {
                    new_sp.path.last_edited = old_sp.path.last_edited;
                    new_sp.path.last_editor = old_sp.path.last_editor;
                }
            }
        }

        // Check keys - compare ignoring last_edited/last_editor and xpub fields
        for (key_id, new_key) in new_template.keys.iter_mut() {
            let key_changed = match old_template {
                Some(old) => old.keys.get(key_id).is_none_or(|old_key| {
                    old_key.alias != new_key.alias
                        || old_key.description != new_key.description
                        || old_key.identity != new_key.identity
                        || old_key.key_type != new_key.key_type
                }),
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

    Response::Wallet { wallet }
}

fn validate_spending_path(path: &SpendingPath) -> Result<(), String> {
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

fn handle_edit_xpub(
    state: &ServerState,
    wallet_id: Uuid,
    key_id: u8,
    xpub: Option<Xpub>,
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

        // Parse xpub value from Xpub struct if provided
        let parsed_xpub: Option<miniscript::DescriptorPublicKey> = match &xpub {
            Some(xpub_data) => match xpub_data.value.parse() {
                Ok(parsed) => Some(parsed),
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
                key.xpub = parsed_xpub;
                // Store xpub source info for audit (already strongly typed)
                if let Some(ref xpub_data) = xpub {
                    key.xpub_source = Some(xpub_data.source.clone());
                    key.xpub_device_kind = xpub_data.device_kind.clone();
                    key.xpub_device_version = xpub_data.device_version.clone();
                    key.xpub_file_name = xpub_data.file_name.clone();
                } else {
                    // Clear source info when xpub is cleared
                    key.xpub_source = None;
                    key.xpub_device_kind = None;
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
            wallet: wallet.clone(),
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

fn handle_device_registered(
    state: &ServerState,
    wallet_id: Uuid,
    infos: RegistrationInfos,
    editor_id: Uuid,
) -> Response {
    debug!(
        "handle_device_registered: wallet_id={}, fingerprint={}, editor_id={}, infos.user={}",
        wallet_id, infos.fingerprint, editor_id, infos.user
    );

    let timestamp = now_timestamp();
    let mut wallets = state.wallets.lock().unwrap();
    let mut registration_infos = state.registration_infos.lock().unwrap();

    let wallet = match wallets.get_mut(&wallet_id) {
        Some(w) => {
            debug!(
                "handle_device_registered: found wallet '{}', status={:?}",
                w.alias, w.status
            );
            w
        }
        None => {
            error!("handle_device_registered: wallet {} not found", wallet_id);
            return Response::Error {
                error: WssError {
                    code: "NOT_FOUND".to_string(),
                    message: format!("Wallet {} not found", wallet_id),
                    request_id: None,
                },
            };
        }
    };

    // Verify user is registering their own device
    if infos.user != editor_id {
        error!(
            "handle_device_registered: user mismatch - infos.user={} != editor_id={}",
            infos.user, editor_id
        );
        return Response::Error {
            error: WssError {
                code: "ACCESS_DENIED".to_string(),
                message: "You cannot register a device for another user".to_string(),
                request_id: None,
            },
        };
    }

    // Must be in Registration state (descriptor set but not finalized)
    let devices = match &wallet.devices {
        Some(devices) if wallet.descriptor.is_some() && wallet.status != WalletStatus::Finalized => {
            debug!(
                "handle_device_registered: wallet in Registration state with {} devices",
                devices.len()
            );
            devices.clone()
        }
        _ => {
            error!(
                "handle_device_registered: invalid wallet state {:?}, expected Registration",
                wallet.status
            );
            return Response::Error {
                error: WssError {
                    code: "INVALID_STATUS".to_string(),
                    message: "Wallet is not in registration status".to_string(),
                    request_id: None,
                },
            };
        }
    };

    // Check if this fingerprint exists in devices list
    if !devices.contains(&infos.fingerprint) {
        error!(
            "handle_device_registered: fingerprint {} not found in devices list",
            infos.fingerprint
        );
        debug!("  available fingerprints: {:?}", devices);
        return Response::Error {
            error: WssError {
                code: "NOT_FOUND".to_string(),
                message: format!(
                    "Fingerprint {} not found in wallet devices",
                    infos.fingerprint
                ),
                request_id: None,
            },
        };
    }

    // Store registration info
    let registered_fingerprint = infos.fingerprint;
    debug!(
        "handle_device_registered: storing registration - registered={}, proof_len={:?}",
        infos.registered,
        infos.proof_of_registration.as_ref().map(|p| p.len())
    );
    registration_infos.insert((wallet_id, infos.fingerprint), infos);

    // Check if all devices now have registration info (registered or skipped)
    let all_have_info = devices.iter().all(|fp| {
        registration_infos.contains_key(&(wallet_id, *fp))
    });
    debug!(
        "handle_device_registered: all_have_info={}, transitioning to Finalized={}",
        all_have_info, all_have_info
    );

    if all_have_info {
        wallet.status = WalletStatus::Finalized;
        wallet.devices = None; // Clear devices list when finalized
    } else {
        // Remove the registered fingerprint from devices list so client doesn't show it
        if let Some(devices) = &mut wallet.devices {
            devices.retain(|fp| *fp != registered_fingerprint);
            debug!(
                "handle_device_registered: removed fingerprint from devices, {} remaining",
                devices.len()
            );
        }
    }

    // Update wallet timestamps
    wallet.last_edited = Some(timestamp);
    wallet.last_editor = Some(editor_id);

    debug!(
        "handle_device_registered: success, returning wallet with status={:?}",
        wallet.status
    );
    Response::Wallet {
        wallet: wallet.clone(),
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
    use liana_connect::ws_business::{Key, KeyType, PolicyTemplate};

    fn make_user(email: &str, role: UserRole) -> User {
        User {
            name: email.split('@').next().unwrap_or("User").to_string(),
            uuid: uuid::Uuid::new_v4(),
            email: email.to_string(),
            role,
            last_edited: None,
            last_editor: None,
        }
    }

    fn make_test_wallet(owner: &User, key_emails: &[&str], status: WalletStatus) -> Wallet {
        let mut template = PolicyTemplate::new();
        for (i, email) in key_emails.iter().enumerate() {
            template.keys.insert(
                i as u8,
                Key {
                    id: i as u8,
                    alias: format!("Key {}", i),
                    description: String::new(),
                    identity: KeyIdentity::Email(email.to_string()),
                    key_type: KeyType::External,
                    xpub: None,
                    xpub_source: None,
                    xpub_device_kind: None,
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
            owner: owner.uuid,
            id: uuid::Uuid::new_v4(),
            template: Some(template),
            status,
            last_edited: None,
            last_editor: None,
        }
    }

    #[test]
    fn test_wsmanager_can_access_all_wallets() {
        let owner = make_user("owner@example.com", UserRole::Participant);
        let ws_manager = make_user("ws@example.com", UserRole::WizardSardineAdmin);

        let statuses = [
            WalletStatus::Created,
            WalletStatus::Drafted,
            WalletStatus::Locked,
            WalletStatus::Validated,
            WalletStatus::Finalized,
        ];

        for status in statuses {
            let wallet = make_test_wallet(&owner, &["alice@example.com"], status.clone());

            // WSManager should have access to all wallets
            let role = ws_manager.role(&wallet);
            assert_eq!(
                role,
                Some(UserRole::WizardSardineAdmin),
                "WSManager should get WizardSardineAdmin role for {:?}",
                status
            );

            assert!(
                can_user_access_wallet(&ws_manager, &wallet),
                "WSManager should access {:?} wallet",
                status
            );
        }
    }

    #[test]
    fn test_participant_cannot_access_draft_locked() {
        let owner = make_user("owner@example.com", UserRole::Participant);
        let participant = make_user("participant@example.com", UserRole::Participant);

        let draft_wallet =
            make_test_wallet(&owner, &["participant@example.com"], WalletStatus::Drafted);
        let locked_wallet =
            make_test_wallet(&owner, &["participant@example.com"], WalletStatus::Locked);
        let created_wallet =
            make_test_wallet(&owner, &["participant@example.com"], WalletStatus::Created);

        // Participant has key but cannot access draft/locked/created
        assert!(!can_user_access_wallet(&participant, &draft_wallet));
        assert!(!can_user_access_wallet(&participant, &locked_wallet));
        assert!(!can_user_access_wallet(&participant, &created_wallet));
    }

    #[test]
    fn test_participant_can_access_validated_finalized() {
        let owner = make_user("owner@example.com", UserRole::Participant);
        let participant = make_user("participant@example.com", UserRole::Participant);

        let validated_wallet = make_test_wallet(
            &owner,
            &["participant@example.com"],
            WalletStatus::Validated,
        );
        let finalized_wallet = make_test_wallet(
            &owner,
            &["participant@example.com"],
            WalletStatus::Finalized,
        );

        assert!(can_user_access_wallet(&participant, &validated_wallet));
        assert!(can_user_access_wallet(&participant, &finalized_wallet));
    }

    #[test]
    fn test_user_without_access_cannot_see_wallet() {
        let owner = make_user("owner@example.com", UserRole::Participant);
        let random_user = make_user("random@example.com", UserRole::Participant);

        let wallet = make_test_wallet(&owner, &["alice@example.com"], WalletStatus::Validated);

        // User without keys should have no role
        let role = random_user.role(&wallet);
        assert_eq!(role, None, "User without keys should have no role");

        assert!(!can_user_access_wallet(&random_user, &wallet));
    }

    #[test]
    fn test_owner_can_access_all_statuses() {
        // Owner stored as Participant, but derives WalletManager for owned wallets
        let owner = make_user("owner@example.com", UserRole::Participant);

        let statuses = [
            WalletStatus::Created,
            WalletStatus::Drafted,
            WalletStatus::Locked,
            WalletStatus::Validated,
            WalletStatus::Finalized,
        ];

        for status in statuses {
            let wallet = make_test_wallet(&owner, &["alice@example.com"], status.clone());

            // Owner derives WalletManager role
            let role = owner.role(&wallet);
            assert_eq!(
                role,
                Some(UserRole::WalletManager),
                "Owner should get WalletManager role for {:?}",
                status
            );

            assert!(
                can_user_access_wallet(&owner, &wallet),
                "Owner should access {:?} wallet",
                status
            );
        }
    }
}
