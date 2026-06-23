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
/// Idempotent: the server replaces the stored set, so re-running on a keyholder
/// change is safe. No plaintext descriptor or seed ever reaches the server.
///
/// **Re-encryption on keyholder change** is this same call. It is *not*
/// auto-fired from the membership-change paths because the Full-Cube tier
/// re-seals the seed, which requires unlocking it behind the owner's PIN — a
/// silent re-encrypt could only refresh the descriptor and would leave a newly
/// added Full-Cube keyholder with a descriptor-only (silently degraded)
/// envelope. Instead the owner re-confirms the tier in the Recovery-Alerts card
/// (one tap for Vault-only; PIN re-entry for Full-Cube), which re-runs this over
/// the current keyholder set.
pub async fn enroll_escrow(
    client: &CoincubeClient,
    server_cube_id: u64,
    vault_id: u64,
    descriptor_json: Vec<u8>,
    seed_json: Option<Vec<u8>>,
) -> Result<VaultMonitoringStatus, OwnerEscrowError> {
    // 1. Resolve the current keyholders + xpubs, then build the envelope set
    //    locally (the server never sees plaintext).
    let vault = client.get_connect_vault(server_cube_id).await?;
    let keyholders = keyholders_from_vault(&vault)?;
    let set = build_escrow_set(&keyholders, &descriptor_json, seed_json.as_deref())?;

    // 2. Upload the opaque ciphertext set (cube-scoped).
    client.put_vault_escrow(server_cube_id, set).await?;

    // 3. Switch the server-blind heartbeat gate on (vault-scoped). No
    //    descriptor — under ECIES the server stores none. Keep the existing
    //    download policy if the caller set one (None lets the server default).
    let status = client
        .set_vault_monitoring(
            vault_id,
            SetVaultMonitoringRequest {
                level: VaultMonitoringLevel::Heartbeat,
                descriptor: None,
                gap_limit: None,
                crk_keyholder_download: None,
            },
        )
        .await?;
    Ok(status)
}

/// Turns inheritance escrow off: true-delete the stored envelope set and the
/// monitoring record. Idempotent (both deletes treat 404 as success).
pub async fn disable_escrow(
    client: &CoincubeClient,
    server_cube_id: u64,
    vault_id: u64,
) -> Result<VaultMonitoringStatus, OwnerEscrowError> {
    client.delete_vault_escrow(server_cube_id).await?;
    client.delete_vault_monitoring(vault_id).await?;
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
            when.method(Method::GET).path("/api/v1/connect/cubes/42/vault");
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
                .json_body_partial(r#"{ "level": "heartbeat" }"#);
            then.status(200).json_body(json!({
                "success": true,
                "data": { "level": "heartbeat" }
            }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = enroll_escrow(&client, 42, 9, b"wsh(desc)#ck".to_vec(), None)
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
            then.status(200).json_body(json!({ "success": true, "data": {} }));
        });
        let monitoring_del = server.mock(|when, then| {
            when.method(Method::DELETE)
                .path("/api/v1/connect/vaults/9/monitoring");
            then.status(200).json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let status = disable_escrow(&client, 42, 9).await.expect("disable ok");
        escrow_del.assert();
        monitoring_del.assert();
        assert_eq!(status.level, VaultMonitoringLevel::Off);
    }

    #[tokio::test]
    async fn enroll_with_no_keyholders_errors_before_upload() {
        let server = MockServer::start();
        let vault_mock = server.mock(|when, then| {
            when.method(Method::GET).path("/api/v1/connect/cubes/42/vault");
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
        let err = enroll_escrow(&client, 42, 9, b"d".to_vec(), None)
            .await
            .expect_err("no keyholders should error");
        vault_mock.assert();
        assert!(matches!(
            err,
            OwnerEscrowError::Escrow(EscrowError::NoKeyholders)
        ));
    }
}
