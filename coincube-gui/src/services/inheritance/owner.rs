//! Owner-side escrow orchestration (ECIES pivot PR 2).
//!
//! Glue between the Recovery-Alerts settings card and the Connect client: turn
//! escrow on (build the per-keyholder envelope set, upload it, and switch the
//! server-blind heartbeat gate on) or off (true-delete the set and the
//! monitoring record). The card supplies the already-built blob JSON so this
//! layer is network-only.
//!
//! Both the escrow set and the monitoring/heartbeat gate are keyed on the
//! **cube** id (`…/cubes/{cubeId}/vault/escrow` and `…/cubes/{cubeId}/vault/
//! monitoring`) — the Vault is a 1:1 sub-resource of the Cube and the
//! monitoring record itself is stored keyed on CubeID, not VaultID, so there
//! is no separate vault-scoped surface to address.

use zeroize::Zeroizing;

use super::escrow::{build_escrow_set, keyholders_from_vault, EscrowError};
use crate::services::coincube::{
    CoincubeClient, CoincubeError, SetVaultMonitoringRequest, VaultMonitoringLevel,
    VaultMonitoringStatus,
};

/// Errors from the owner escrow orchestration.
#[derive(Debug)]
pub enum OwnerEscrowError {
    /// Building the envelope set failed (no keyholders, bad xpub, seal error).
    Escrow(EscrowError),
    /// A Connect call failed (fetch vault / upload / monitoring).
    Connect(CoincubeError),
}

impl std::fmt::Display for OwnerEscrowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Escrow(e) => write!(f, "{}", e),
            Self::Connect(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for OwnerEscrowError {}

impl From<EscrowError> for OwnerEscrowError {
    fn from(e: EscrowError) -> Self {
        Self::Escrow(e)
    }
}

impl From<CoincubeError> for OwnerEscrowError {
    fn from(e: CoincubeError) -> Self {
        Self::Connect(e)
    }
}

/// Turns inheritance escrow on for a Vault. Fetches the current keyholder set,
/// seals the descriptor (always) and the seed (`seed_json.is_some()` for the
/// Full-Cube tier) to each keyholder's xpub, uploads the whole set, then
/// switches the server-blind heartbeat gate on. Returns the new monitoring
/// status.
///
/// Idempotent: the server replaces the stored set, so re-running for the same
/// Vault is safe. No plaintext descriptor or seed ever reaches the server.
///
/// **The keyholder set is fixed for the life of a Vault.** Once a Vault is
/// `active` its signing quorum is sealed: the server rejects adding a keyholder
/// with 409 `VAULT_KEYHOLDER_LOCKED` and the role chooser hides the Keyholder
/// role (`allowed_vault_member_roles`), and there is no key-rotation path
/// (rotating a cosigner means a new descriptor, i.e. a new Vault). So there is
/// no "re-encrypt on keyholder change" while a Vault is active, and the
/// Recovery-Alerts card's same-tier early-return is correct — re-running this
/// over an unchanged keyholder set would be a no-op. Re-encryption is only
/// relevant when an *expired* Vault is rebuilt with a different keyholder set,
/// which is a fresh enrolment on the rebuilt Vault, not a re-tap on the active
/// one.
pub async fn enroll_escrow(
    client: &CoincubeClient,
    server_cube_id: u64,
    descriptor_json: Vec<u8>,
    seed_json: Option<Zeroizing<Vec<u8>>>,
) -> Result<VaultMonitoringStatus, OwnerEscrowError> {
    // 1. Resolve the current keyholders + xpubs, then build the envelope set
    //    locally (the server never sees plaintext). The `Zeroizing` seed buffer
    //    is owned here so it's wiped when this fn returns; `build_escrow_set`
    //    only borrows the bytes to seal them, so it needs no `Zeroizing` itself.
    let vault = client.get_connect_vault(server_cube_id).await?;
    let keyholders = keyholders_from_vault(&vault)?;
    let set = build_escrow_set(
        &keyholders,
        server_cube_id,
        &descriptor_json,
        seed_json.as_ref().map(|s| s.as_slice()),
    )?;

    // 2. Upload the opaque ciphertext set (cube-scoped).
    client.put_vault_escrow(server_cube_id, set).await?;

    // 3. Switch the server-blind heartbeat gate on. Monitoring is keyed on
    //    the same cube id as the escrow upload above — no vault id involved.
    //    No descriptor — under ECIES the server stores none.
    let mut status = client
        .set_vault_monitoring(
            server_cube_id,
            SetVaultMonitoringRequest {
                level: VaultMonitoringLevel::Heartbeat,
                descriptor: None,
                gap_limit: None,
            },
        )
        .await?;
    // The desktop knows exactly which artifact kinds it just sealed, so stamp
    // them onto the returned status. The settings card derives the escrow tier
    // straight from `escrowed_artifacts` (server-derived model, no session-tracked
    // guess), so this makes the card reflect the enrolment immediately and stays
    // correct even against an API that doesn't yet echo the field on this
    // response. The next `LoadStatus` reconciles against the server's own report.
    status.escrowed_artifacts = Some(if seed_json.is_some() {
        vec!["descriptor".to_string(), "seed".to_string()]
    } else {
        vec!["descriptor".to_string()]
    });
    Ok(status)
}

/// Turns **alerts** (monitoring) on without escrowing anything — the standalone
/// "Recovery alerts" opt-in (PR 2). Posts the monitoring record so the server
/// watches the timelock heartbeat and emails keyholders when the recovery window
/// opens; no descriptor or seed is uploaded. Returns the new (alerts-on,
/// nothing-escrowed) status.
///
/// Only ever called when alerts were off — and by the split-model invariant
/// (*escrow present ⟹ monitoring on*) escrow was therefore off too — so nothing
/// is escrowed on return. We stamp `escrowed_artifacts = []` to say so.
pub async fn enable_alerts(
    client: &CoincubeClient,
    server_cube_id: u64,
) -> Result<VaultMonitoringStatus, OwnerEscrowError> {
    let mut status = client
        .set_vault_monitoring(
            server_cube_id,
            SetVaultMonitoringRequest {
                level: VaultMonitoringLevel::Heartbeat,
                descriptor: None,
                gap_limit: None,
            },
        )
        .await?;
    status.escrowed_artifacts = Some(Vec::new());
    Ok(status)
}

/// Turns **alerts** (monitoring) off — the only path that deletes the monitoring
/// record. When `also_delete_escrow` is set (an escrow set is currently stored),
/// the envelope set is deleted **first**, then monitoring, so a partial failure
/// can never strand the forbidden state *escrow present + monitoring off* — a
/// recovery window that could open with a stored kit no keyholder is being
/// watched for. This reverses the old fused order deliberately: under the split
/// model the safe-to-linger leftover is a *monitored* Vault (alerts-only), not a
/// stored-kit-without-monitoring one. Both deletes are idempotent (404 =
/// success). Returns the fully-off status.
pub async fn disable_alerts(
    client: &CoincubeClient,
    server_cube_id: u64,
    also_delete_escrow: bool,
) -> Result<VaultMonitoringStatus, OwnerEscrowError> {
    if also_delete_escrow {
        // Escrow first: once it's gone we're in the valid alerts-only state, so
        // if the monitoring delete then fails a retry finishes the job and we
        // were never in the forbidden escrow-without-monitoring state.
        client.delete_vault_escrow(server_cube_id).await?;
    }
    client.delete_vault_monitoring(server_cube_id).await?;
    Ok(VaultMonitoringStatus {
        level: VaultMonitoringLevel::Off,
        escrowed_artifacts: Some(Vec::new()),
        ..VaultMonitoringStatus::default()
    })
}

/// Turns inheritance **escrow** off while leaving alerts (the monitoring record)
/// in place — the "Nothing — alerts only" escrow selection (PR 2). True-deletes
/// the stored envelope set; monitoring keeps running so keyholders are still
/// alerted when the recovery window opens (they simply have no escrowed kit to
/// recover with). Idempotent (a 404 on the escrow delete is success).
///
/// This is the escrow half of the old fused teardown, split out so the card's
/// two controls map to two operations. It preserves the split-model invariant
/// *escrow present ⟹ monitoring on*: after this returns escrow is gone and
/// monitoring survives — a valid state. The monitoring record is deleted **only**
/// by [`disable_alerts`].
///
/// Escrow is **cube-scoped**, so a rebuilt Vault (new vault id, same cube)
/// doesn't strand the delete.
pub async fn disable_escrow(
    client: &CoincubeClient,
    server_cube_id: u64,
) -> Result<VaultMonitoringStatus, OwnerEscrowError> {
    client.delete_vault_escrow(server_cube_id).await?;
    // Monitoring stays on; report alerts-on with nothing escrowed. The exact
    // on-level (heartbeat vs full) is immaterial to the desktop — any non-Off
    // level reads as "alerts on" — and the next LoadStatus reconciles it.
    Ok(VaultMonitoringStatus {
        level: VaultMonitoringLevel::Heartbeat,
        escrowed_artifacts: Some(Vec::new()),
        ..VaultMonitoringStatus::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::bip32::{DerivationPath, Xpriv, Xpub};
    use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
    use coincube_core::miniscript::bitcoin::Network;
    use httpmock::{Method, MockServer};
    use serde_json::json;
    use std::str::FromStr;

    fn test_xpub(seed: &[u8]) -> Xpub {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, seed).unwrap();
        let path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
        Xpub::from_priv(&secp, &master.derive_priv(&secp, &path).unwrap())
    }

    #[tokio::test]
    async fn enroll_vault_only_uploads_descriptor_set_and_enables_heartbeat() {
        let xpub = test_xpub(b"owner-enroll-keyholder-seed-vector-00000000");
        let server = MockServer::start();

        // get_connect_vault → one keyholder with a key.
        let vault_mock = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/42/vault");
            then.status(200).json_body(json!({
                "success": true,
                "data": {
                    "id": 9,
                    "cubeId": 42,
                    "timelockDays": 365,
                    "timelockExpiresAt": "2027-06-22T00:00:00Z",
                    "lastResetAt": "2026-06-22T00:00:00Z",
                    "status": "active",
                    "members": [{
                        "id": 1,
                        "keyId": 10,
                        "role": "keyholder",
                        "key": {
                            "id": 10,
                            "name": "Heir",
                            "xpub": xpub.to_string(),
                            "derivationPath": "m/48'/0'/0'/2'"
                        },
                        "createdAt": "2026-06-22T00:00:00Z"
                    }],
                    "createdAt": "2026-06-22T00:00:00Z",
                    "updatedAt": "2026-06-22T00:00:00Z"
                }
            }));
        });

        // PUT escrow — assert it carries exactly one (descriptor) envelope.
        let escrow_mock = server.mock(|when, then| {
            when.method(Method::PUT)
                .path("/api/v1/connect/cubes/42/vault/escrow")
                .json_body_partial(r#"{ "envelopes": [ { "artifactKind": "descriptor", "keyholderKeyId": 10 } ] }"#);
            then.status(200).json_body(json!({ "success": true, "data": {} }));
        });

        // set monitoring → heartbeat, no descriptor. Cube-scoped (42), not
        // vault-scoped: monitoring is keyed on CubeID server-side.
        let monitoring_mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault/monitoring")
                .json_body_partial(r#"{ "level": "heartbeat" }"#);
            // Real API response shape: the level field is named
            // `monitoringLevel`, not `level` (see VaultMonitoringStatus's
            // explicit `rename`).
            then.status(200).json_body(json!({
                "success": true,
                "data": { "monitoringLevel": "heartbeat" }
            }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = enroll_escrow(&client, 42, b"wsh(desc)#ck".to_vec(), None)
            .await
            .expect("enroll should succeed");

        vault_mock.assert();
        escrow_mock.assert();
        monitoring_mock.assert();
        assert_eq!(status.level, VaultMonitoringLevel::Heartbeat);
    }

    #[tokio::test]
    async fn disable_escrow_deletes_escrow_only_keeping_alerts() {
        // The "Nothing — alerts only" selection deletes the envelope set but
        // NOT the monitoring record: keyholders still get the recovery-window
        // alert. The monitoring DELETE mock is registered and must NOT be hit.
        let server = MockServer::start();
        let escrow_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault/escrow");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });
        let monitoring_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault/monitoring");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = disable_escrow(&client, 42).await.expect("disable ok");
        escrow_del.assert();
        monitoring_del.assert_hits(0);
        // Alerts stay on, nothing escrowed.
        assert_ne!(status.level, VaultMonitoringLevel::Off);
        assert_eq!(status.escrowed_artifacts.as_deref(), Some(&[][..]));
    }

    #[tokio::test]
    async fn enable_alerts_posts_monitoring_without_escrow() {
        // The standalone "Recovery alerts" opt-in posts the monitoring record
        // and uploads nothing — no PUT escrow, no descriptor in the body.
        let server = MockServer::start();
        let monitoring_mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/cubes/42/vault/monitoring");
            then.status(200).json_body(json!({
                "success": true,
                "data": { "monitoringLevel": "heartbeat" }
            }));
        });
        // Escrow PUT must NOT be called.
        let escrow_put = server.mock(|when, then| {
            when.method(Method::PUT)
                .path("/api/v1/connect/cubes/42/vault/escrow");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = enable_alerts(&client, 42).await.expect("enable ok");
        monitoring_mock.assert();
        escrow_put.assert_hits(0);
        assert_ne!(status.level, VaultMonitoringLevel::Off);
        assert_eq!(status.escrowed_artifacts.as_deref(), Some(&[][..]));
    }

    #[tokio::test]
    async fn disable_alerts_without_escrow_deletes_monitoring_only() {
        let server = MockServer::start();
        let monitoring_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault/monitoring");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });
        let escrow_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault/escrow");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = disable_alerts(&client, 42, false)
            .await
            .expect("disable ok");
        monitoring_del.assert();
        escrow_del.assert_hits(0);
        assert_eq!(status.level, VaultMonitoringLevel::Off);
    }

    #[tokio::test]
    async fn disable_alerts_removes_escrow_before_monitoring() {
        // Safety ordering under the split model: when both go, escrow is
        // deleted FIRST. If that fails we must bail *before* deleting the
        // monitoring record, so a partial failure never leaves the forbidden
        // state escrow-present + monitoring-off (a recovery window could open
        // with a stored kit no keyholder is being watched for). The monitoring
        // DELETE mock is registered but must NOT be hit.
        let server = MockServer::start();
        let escrow_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault/escrow");
            then.status(500).json_body(json!({ "success": false }));
        });
        let monitoring_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault/monitoring");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = disable_alerts(&client, 42, true)
            .await
            .expect_err("escrow delete failure should propagate");
        assert!(matches!(err, OwnerEscrowError::Connect(_)));
        escrow_del.assert();
        // Monitoring record is left intact (alerts stay on) for a retry to clear.
        monitoring_del.assert_hits(0);
    }

    #[tokio::test]
    async fn get_monitoring_maps_real_api_response_shape() {
        // Regression guard: the API names these fields `monitoringLevel` and
        // `state` (MonitoringStatusResponse), not `level` and
        // `lastNotifiedState`. A struct-level `camelCase` rename alone would
        // silently deserialize both to their defaults (Off / None) on every
        // real response — the bug that made a *successful* enable still show
        // "Off" on the settings card.
        let server = MockServer::start();
        let monitoring_mock = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/42/vault/monitoring");
            then.status(200).json_body(json!({
                "success": true,
                "data": {
                    "enabled": true,
                    "gapLimit": 20,
                    "optedInAt": "2026-07-26T00:00:00Z",
                    "state": "none",
                    "monitoringLevel": "full"
                }
            }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = client
            .get_vault_monitoring(42)
            .await
            .expect("get monitoring ok");
        monitoring_mock.assert();
        assert_eq!(status.level, VaultMonitoringLevel::Full);
        assert_eq!(status.last_notified_state.as_deref(), Some("none"));
    }

    #[tokio::test]
    async fn enroll_with_no_keyholders_errors_before_upload() {
        let server = MockServer::start();
        let vault_mock = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/42/vault");
            then.status(200).json_body(json!({
                "success": true,
                "data": {
                    "id": 9, "cubeId": 42, "timelockDays": 365,
                    "timelockExpiresAt": "2027-06-22T00:00:00Z",
                    "lastResetAt": "2026-06-22T00:00:00Z", "status": "active",
                    "members": [],
                    "createdAt": "2026-06-22T00:00:00Z",
                    "updatedAt": "2026-06-22T00:00:00Z"
                }
            }));
        });
        // No escrow PUT mock — it must NOT be called.
        let client = CoincubeClient::for_test(server.base_url());
        let err = enroll_escrow(&client, 42, b"d".to_vec(), None)
            .await
            .expect_err("no keyholders should error");
        vault_mock.assert();
        assert!(matches!(
            err,
            OwnerEscrowError::Escrow(EscrowError::NoKeyholders)
        ));
    }
}
