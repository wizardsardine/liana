//! Owner-side escrow orchestration (ECIES pivot PR 2).
//!
//! Glue between the Recovery-Alerts settings card and the Connect client: turn
//! escrow on (build the per-keyholder envelope set, upload it, and switch the
//! server-blind heartbeat gate on) or off (true-delete the set and the
//! monitoring record). The card supplies the already-built blob JSON so this
//! layer is network-only.
//!
//! The escrow set is keyed on the **cube** id (`PUT …/cubes/{cubeId}/vault/
//! escrow`); the heartbeat gate is keyed on the **vault** id
//! (`…/vaults/{vaultId}/monitoring`).

use zeroize::Zeroizing;

use super::escrow::{build_escrow_set, keyholders_from_vault, EscrowError};
use crate::services::coincube::{
    CoincubeClient, CoincubeError, KeyholderDownloadPolicy, SetVaultMonitoringRequest,
    VaultMonitoringLevel, VaultMonitoringStatus,
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
    download_policy: KeyholderDownloadPolicy,
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

    // 3. Switch the server-blind heartbeat gate on, on the *freshly fetched*
    //    vault rather than a caller-cached id that could be stale — so
    //    monitoring lands on the same vault we just built the escrow set
    //    against. No descriptor — under ECIES the server stores none.
    //    `download_policy` is the vault's *current* keyholder download policy,
    //    forwarded explicitly so a re-enrol / tier switch (Vault-only ↔
    //    Full-Cube) preserves the owner's choice instead of letting an omitted
    //    field reset it to the server default.
    let status = client
        .set_vault_monitoring(
            vault.id,
            SetVaultMonitoringRequest {
                level: VaultMonitoringLevel::Heartbeat,
                descriptor: None,
                gap_limit: None,
                crk_keyholder_download: Some(download_policy),
            },
        )
        .await?;
    Ok(status)
}

/// Turns inheritance escrow off: true-delete the monitoring record and the
/// stored envelope set. Idempotent (both deletes treat 404 as success).
///
/// Order mirrors `enroll_escrow` in reverse to preserve its invariant — escrow
/// is uploaded before monitoring is switched on, so "monitoring on" always
/// implies "ciphertext present." Tearing down, we therefore switch monitoring
/// **off first**, then delete the escrow set. If the second call fails the Vault
/// is left un-monitored with a harmless leftover (opaque, server-blind)
/// envelope set that a retry removes — never the inverse, a monitored Vault with
/// no ciphertext, where a recovery window could open with nothing for heirs.
///
/// Monitoring is **vault-scoped** and escrow is **cube-scoped** — two different
/// resources keyed on two different ids (not the same vault in two forms). The
/// monitoring delete resolves the *current* vault id from the server rather than
/// trusting the caller's cached `vault_id`: if the vault was rebuilt with a new
/// id, a stale cached id would 404 (no-op) and leave the live vault monitored
/// after escrow is gone — the dangerous state above. Best-effort: if the lookup
/// fails (vault gone / offline) we fall back to the caller's id, and we always
/// attempt the cube-scoped escrow delete so it's never stranded.
pub async fn disable_escrow(
    client: &CoincubeClient,
    server_cube_id: u64,
    vault_id: u64,
) -> Result<VaultMonitoringStatus, OwnerEscrowError> {
    let monitoring_vault_id = client
        .get_connect_vault(server_cube_id)
        .await
        .ok()
        .map(|v| v.id)
        .unwrap_or(vault_id);
    client.delete_vault_monitoring(monitoring_vault_id).await?;
    client.delete_vault_escrow(server_cube_id).await?;
    Ok(VaultMonitoringStatus {
        level: VaultMonitoringLevel::Off,
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

        // set monitoring → heartbeat, no descriptor.
        let monitoring_mock = server.mock(|when, then| {
            when.method(Method::POST)
                .path("/api/v1/connect/vaults/9/monitoring")
                // The current download policy must be forwarded (camelCase
                // field, snake_case value) so a re-enrol doesn't reset it.
                .json_body_partial(
                    r#"{ "level": "heartbeat", "crkKeyholderDownload": "anytime" }"#,
                );
            then.status(200).json_body(json!({
                "success": true,
                "data": { "level": "heartbeat" }
            }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = enroll_escrow(
            &client,
            42,
            b"wsh(desc)#ck".to_vec(),
            None,
            KeyholderDownloadPolicy::Anytime,
        )
        .await
        .expect("enroll should succeed");

        vault_mock.assert();
        escrow_mock.assert();
        monitoring_mock.assert();
        assert_eq!(status.level, VaultMonitoringLevel::Heartbeat);
    }

    #[tokio::test]
    async fn disable_deletes_escrow_and_monitoring() {
        let server = MockServer::start();
        let escrow_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault/escrow");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });
        let monitoring_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/vaults/9/monitoring");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = disable_escrow(&client, 42, 9).await.expect("disable ok");
        escrow_del.assert();
        monitoring_del.assert();
        assert_eq!(status.level, VaultMonitoringLevel::Off);
    }

    #[tokio::test]
    async fn disable_removes_monitoring_before_escrow() {
        // Safety ordering: monitoring is switched off first. If that fails we
        // must bail *before* deleting the escrow set, so a partial failure never
        // leaves a monitored Vault with no ciphertext (a recovery window could
        // open with nothing for heirs). The escrow DELETE mock is registered but
        // must NOT be hit.
        let server = MockServer::start();
        let monitoring_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/vaults/9/monitoring");
            then.status(500).json_body(json!({ "success": false }));
        });
        let escrow_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/cubes/42/vault/escrow");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = disable_escrow(&client, 42, 9)
            .await
            .expect_err("monitoring delete failure should propagate");
        assert!(matches!(err, OwnerEscrowError::Connect(_)));
        monitoring_del.assert();
        // Escrow set is left intact (opaque, server-blind) for a retry to clear.
        escrow_del.assert_hits(0);
    }

    #[tokio::test]
    async fn disable_resolves_fresh_vault_id_for_monitoring() {
        // The server's current vault id (99) differs from the caller's stale
        // cached id (9). Monitoring (vault-scoped) must be deleted on the FRESH
        // id, not the stale one — otherwise a rebuilt vault stays monitored
        // after escrow is gone. Escrow stays cube-scoped (42).
        let server = MockServer::start();
        let vault_mock = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/42/vault");
            then.status(200).json_body(json!({
                "success": true,
                "data": {
                    "id": 99,
                    "cubeId": 42,
                    "timelockDays": 365,
                    "timelockExpiresAt": "2027-06-22T00:00:00Z",
                    "lastResetAt": "2026-06-22T00:00:00Z",
                    "status": "active",
                    "members": [],
                    "createdAt": "2026-06-22T00:00:00Z",
                    "updatedAt": "2026-06-22T00:00:00Z"
                }
            }));
        });
        // Monitoring on the FRESH id 99 (a /9 delete would be the stale bug).
        let monitoring_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/vaults/99/monitoring");
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
        let status = disable_escrow(&client, 42, 9).await.expect("disable ok");
        vault_mock.assert();
        monitoring_del.assert(); // hit on /99, not the stale /9
        escrow_del.assert();
        assert_eq!(status.level, VaultMonitoringLevel::Off);
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
        let err = enroll_escrow(
            &client,
            42,
            b"d".to_vec(),
            None,
            KeyholderDownloadPolicy::AtApproaching,
        )
        .await
        .expect_err("no keyholders should error");
        vault_mock.assert();
        assert!(matches!(
            err,
            OwnerEscrowError::Escrow(EscrowError::NoKeyholders)
        ));
    }
}
