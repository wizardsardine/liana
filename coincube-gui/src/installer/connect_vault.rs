//! Orchestrates creation of the backend `ConnectVault` shell and its
//! `ConnectVaultMember` rows after the local wallet install completes.
//!
//! Design decisions (2026-04-18, `PLAN-cube-membership-desktop.md`):
//! - **Timelock days** = `ceil(max_recovery_blocks / 144)` (≈ blocks per
//!   day), clamped to a minimum of 1. Carried through in
//!   `Context::connect_vault_timelock_days`. Inherently approximate —
//!   surfaced as such in the Final step's outcome caption.
//! - **Member mapping** is restricted to `KeySource::KeychainKey`. HW,
//!   xpub, master-signer, token, and border-wallet keys are skipped
//!   (with a `tracing::info!` log). Rationale: only keychain keys have
//!   backend `keys.id` rows, and W9's "used in another vault" guard
//!   only matters for those.
//! - **Role** defaults to `Keyholder` for every member. Refinement into
//!   Beneficiary/Observer is a follow-up.
//! - **Failure UX**: the W9 409 (`KEY_ALREADY_USED_IN_VAULT`) rolls
//!   back the just-created vault so the user can restart with a clean
//!   slate; other errors leave the partial vault in place and surface a
//!   retry-able warning.

use crate::services::coincube::{
    AddVaultMemberRequest, CoincubeClient, ConnectVaultResponse, CreateConnectVaultRequest,
    RegisterCubeRequest, VaultMemberRole,
};

use super::context::ConnectVaultMemberPayload;

/// Successful outcome of the vault-create fan-out.
#[derive(Debug, Clone)]
pub struct ConnectVaultOutcome {
    pub vault_id: u64,
    pub cube_server_id: u64,
    pub timelock_days: i32,
    pub members_added: usize,
    pub members_skipped_non_keychain: usize,
}

/// Error kinds surfaced to the Final step so it can pick the right UX.
#[derive(Debug, Clone)]
pub enum ConnectVaultError {
    /// The inputs don't support backend vault creation — no authenticated
    /// client, no cube id, or no members to attach. Treated as
    /// "silently skipped" by the Final step (user sees nothing).
    NotApplicable,
    /// W9 409 `KEY_ALREADY_USED_IN_VAULT`. The vault shell was rolled
    /// back before the error surfaced, so the user can restart. Carries
    /// the offending `key_id` for the dialog.
    KeyAlreadyUsedInVault { key_id: u64 },
    /// Any other failure (network, backend 5xx, partial success). The
    /// caller gets a message suitable for display.
    Other(String),
}

impl std::fmt::Display for ConnectVaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotApplicable => write!(f, "Not applicable"),
            Self::KeyAlreadyUsedInVault { key_id } => {
                write!(
                    f,
                    "Key #{} is already used in another Vault. A key can \
                     only participate in one Vault. Remove it from this \
                     configuration and pick a different key.",
                    key_id
                )
            }
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

/// Run the full vault-create + member-attach flow. Safe to call when
/// Connect isn't authenticated — returns `NotApplicable` in that case.
///
/// Cube registration is idempotent on `(user_id, uuid)` server-side so
/// calling `register_cube` every time is safe — it just reaches into
/// the existing row.
pub async fn create_connect_vault(
    client: Option<CoincubeClient>,
    cube_uuid: Option<String>,
    cube_name: Option<String>,
    network: String,
    members: Vec<ConnectVaultMemberPayload>,
    timelock_days: Option<i32>,
) -> Result<ConnectVaultOutcome, ConnectVaultError> {
    let (Some(client), Some(cube_uuid), Some(cube_name)) = (client, cube_uuid, cube_name) else {
        return Err(ConnectVaultError::NotApplicable);
    };
    if members.is_empty() {
        // No keychain-sourced members means nothing for the backend to
        // track. Skip silently — the Final step translates this into a
        // no-op.
        return Err(ConnectVaultError::NotApplicable);
    }
    // `div_ceil` upstream already clamps to ≥ 1; defensive default here
    // in case we're called with `None` (shouldn't happen when members
    // is non-empty because a recovery path always exists in a valid
    // descriptor, but cheap insurance).
    let timelock_days = timelock_days.unwrap_or(1).max(1);

    // 1. Register cube (idempotent — returns the existing row if the
    //    uuid + owner already match).
    let cube = client
        .register_cube(RegisterCubeRequest {
            uuid: cube_uuid,
            name: cube_name,
            network,
        })
        .await
        .map_err(|e| ConnectVaultError::Other(format!("Failed to register cube: {}", e)))?;

    // 2. Create the vault shell.
    let vault: ConnectVaultResponse = client
        .create_connect_vault(cube.id, CreateConnectVaultRequest { timelock_days })
        .await
        .map_err(|e| ConnectVaultError::Other(format!("Failed to create Connect vault: {}", e)))?;

    // 3. Fan out member rows. On W9 409, roll back and bail.
    let mut members_added = 0usize;
    for payload in &members {
        let req = AddVaultMemberRequest {
            contact_id: payload.contact_id,
            key_id: Some(payload.key_id),
            role: VaultMemberRole::Keyholder,
        };
        match client.add_vault_member(cube.id, req).await {
            Ok(_) => {
                members_added += 1;
            }
            Err(e) if e.is_key_already_used_in_vault() => {
                // Roll back the vault we just created so the user can
                // restart the Vault Builder with a clean slate. The
                // delete is best-effort — a failure to roll back just
                // means the user will see a "vault already exists" on
                // their next attempt and the backend's `delete_connect_vault`
                // can be retried.
                if let Err(rollback_err) = client.delete_connect_vault(cube.id).await {
                    tracing::warn!(
                        "W9 rollback failed to delete vault {}: {}",
                        vault.id,
                        rollback_err
                    );
                }
                return Err(ConnectVaultError::KeyAlreadyUsedInVault {
                    key_id: payload.key_id,
                });
            }
            Err(e) => {
                return Err(ConnectVaultError::Other(format!(
                    "Failed to add vault member (key {}): {}",
                    payload.key_id, e
                )));
            }
        }
    }

    Ok(ConnectVaultOutcome {
        vault_id: vault.id,
        cube_server_id: cube.id,
        timelock_days: vault.timelock_days,
        members_added,
        members_skipped_non_keychain: 0, // already filtered upstream
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::descriptor::PathKind;
    use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
    use httpmock::{Method, MockServer};
    use serde_json::json;
    use std::str::FromStr;

    fn sample_member(fp: &str, key_id: u64, contact_id: Option<u64>) -> ConnectVaultMemberPayload {
        ConnectVaultMemberPayload {
            fingerprint: Fingerprint::from_str(fp).expect("valid fp"),
            key_id,
            contact_id,
            path_kind: PathKind::Primary,
        }
    }

    #[tokio::test]
    async fn not_applicable_when_client_missing() {
        let err = create_connect_vault(
            None,
            Some("uuid".to_string()),
            Some("My Cube".to_string()),
            "mainnet".to_string(),
            vec![sample_member("deadbeef", 1, None)],
            Some(180),
        )
        .await
        .expect_err("should short-circuit");
        assert!(matches!(err, ConnectVaultError::NotApplicable));
    }

    #[tokio::test]
    async fn not_applicable_when_members_empty() {
        let server = MockServer::start();
        let client = CoincubeClient::for_test(server.base_url());
        let err = create_connect_vault(
            Some(client),
            Some("uuid".to_string()),
            Some("My Cube".to_string()),
            "mainnet".to_string(),
            vec![],
            Some(180),
        )
        .await
        .expect_err("should short-circuit");
        assert!(matches!(err, ConnectVaultError::NotApplicable));
    }

    #[tokio::test]
    async fn happy_path_registers_creates_and_adds_members() {
        let server = MockServer::start();

        let register = server.mock(|when, then| {
            when.method(Method::POST).path("/api/v1/connect/cubes");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 42,
                        "uuid": "abc-uuid",
                        "name": "My Cube",
                        "network": "mainnet",
                        "lightningAddress": null,
                        "bolt12Offer": null,
                        "status": "active"
                    }
                }));
        });

        let create_vault = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault")
                .json_body(json!({ "timelockDays": 180 }));
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 5,
                        "cubeId": 42,
                        "timelockDays": 180,
                        "timelockExpiresAt": "2026-10-15T00:00:00Z",
                        "lastResetAt": "2026-04-18T00:00:00Z",
                        "status": "active",
                        "members": [],
                        "createdAt": "2026-04-18T00:00:00Z",
                        "updatedAt": "2026-04-18T00:00:00Z"
                    }
                }));
        });

        let add_member = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault/members");
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 7,
                        "keyId": 99,
                        "role": "keyholder",
                        "createdAt": "2026-04-18T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let outcome = create_connect_vault(
            Some(client),
            Some("abc-uuid".to_string()),
            Some("My Cube".to_string()),
            "mainnet".to_string(),
            vec![sample_member("deadbeef", 99, None)],
            Some(180),
        )
        .await
        .expect("happy path");

        register.assert();
        create_vault.assert();
        add_member.assert();
        assert_eq!(outcome.vault_id, 5);
        assert_eq!(outcome.cube_server_id, 42);
        assert_eq!(outcome.timelock_days, 180);
        assert_eq!(outcome.members_added, 1);
    }

    #[tokio::test]
    async fn w9_409_rolls_back_vault_and_surfaces_key_id() {
        let server = MockServer::start();

        let register = server.mock(|when, then| {
            when.method(Method::POST).path("/api/v1/connect/cubes");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 42,
                        "uuid": "abc-uuid",
                        "name": "My Cube",
                        "network": "mainnet",
                        "lightningAddress": null,
                        "bolt12Offer": null,
                        "status": "active"
                    }
                }));
        });

        let create_vault = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault");
            then.status(201)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 5,
                        "cubeId": 42,
                        "timelockDays": 180,
                        "timelockExpiresAt": "2026-10-15T00:00:00Z",
                        "lastResetAt": "2026-04-18T00:00:00Z",
                        "status": "active",
                        "members": [],
                        "createdAt": "2026-04-18T00:00:00Z",
                        "updatedAt": "2026-04-18T00:00:00Z"
                    }
                }));
        });

        let add_member = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault/members");
            then.status(409)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": false,
                    "error": {
                        "code": "KEY_ALREADY_USED_IN_VAULT",
                        "message": "Key has already been used in another vault"
                    }
                }));
        });

        let rollback = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": { "deleted": true }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = create_connect_vault(
            Some(client),
            Some("abc-uuid".to_string()),
            Some("My Cube".to_string()),
            "mainnet".to_string(),
            vec![sample_member("deadbeef", 99, None)],
            Some(180),
        )
        .await
        .expect_err("expected W9 409 error");

        register.assert();
        create_vault.assert();
        add_member.assert();
        rollback.assert();
        assert!(
            matches!(err, ConnectVaultError::KeyAlreadyUsedInVault { key_id: 99 }),
            "expected W9 error with key_id=99, got: {:?}",
            err
        );
    }
}
