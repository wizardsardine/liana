//! Reusable service helpers for the restore half of Cube Recovery Kit.
//!
//! All three restore entry points (W13 installer restore, W14 post-
//! mnemonic descriptor fetch, W15 vault-wizard restore) share the
//! same crypto + API sequence:
//!
//!   1. Fetch the kit from Connect (`GET /recovery-kit`).
//!   2. Decrypt the ciphertext envelope(s) with the user's recovery
//!      password.
//!   3. Parse JSON into `SeedBlob` / `DescriptorBlob`.
//!
//! Keeping the sequence here means the three UI integrations each
//! become a thin shell around `fetch_and_decrypt_kit` — no duplicated
//! crypto, no duplicated error mapping, and (importantly) one place
//! to audit for zeroization correctness.

use zeroize::Zeroizing;

use super::{decrypt, DescriptorBlob, RecoveryError, SeedBlob};
use crate::services::coincube::{CoincubeClient, CoincubeError};

/// Errors produced by the restore helpers. Collapses coincube-client,
/// envelope-decrypt, and JSON parse errors into a single enum so UI
/// code can pattern-match without touching three sub-types.
#[derive(Debug)]
pub enum RestoreError {
    /// The Cube has no kit on Connect (backend 404). UI should
    /// surface as "No backup found for this Cube".
    NotFound,
    /// Connect returned 429 — the caller should back off. `retry_after`
    /// is the parsed `Retry-After` duration.
    RateLimited { retry_after: std::time::Duration },
    /// Generic Connect error — auth, network, server 5xx.
    Api(String),
    /// The envelope failed to decrypt: wrong password, tampered
    /// ciphertext, or unsupported wire version. Collapsed to a
    /// single variant for the UI (see envelope codec notes) so an
    /// offline bruteforcer can't distinguish the cases.
    BadPasswordOrCorrupt,
    /// Envelope decrypted cleanly but the plaintext JSON failed to
    /// deserialize into the expected blob type. Indicates a client
    /// writing a shape this client doesn't understand (e.g. a newer
    /// blob version we should refuse rather than mis-parse).
    BlobParse(String),
    /// The kit exists on Connect but doesn't contain the half this
    /// caller needs (e.g. W14 wants a descriptor; the server-side
    /// kit is seed-only).
    HalfMissing,
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "No Recovery Kit found on Connect for this Cube."),
            Self::RateLimited { retry_after } => write!(
                f,
                "Rate limited — try again in {}s.",
                retry_after.as_secs()
            ),
            Self::Api(msg) => write!(f, "Connect error: {}", msg),
            Self::BadPasswordOrCorrupt => write!(
                f,
                "Recovery password is incorrect or the backup is corrupted."
            ),
            Self::BlobParse(msg) => write!(f, "Recovery Kit shape not recognised: {}", msg),
            Self::HalfMissing => write!(
                f,
                "This Recovery Kit doesn't include the part you're trying to restore."
            ),
        }
    }
}

impl std::error::Error for RestoreError {}

impl From<CoincubeError> for RestoreError {
    fn from(e: CoincubeError) -> Self {
        match e {
            CoincubeError::NotFound => Self::NotFound,
            CoincubeError::RateLimited { retry_after } => Self::RateLimited { retry_after },
            other => Self::Api(other.to_string()),
        }
    }
}

impl From<RecoveryError> for RestoreError {
    fn from(e: RecoveryError) -> Self {
        match e {
            RecoveryError::BadPasswordOrCorrupt => Self::BadPasswordOrCorrupt,
            other => Self::Api(other.to_string()),
        }
    }
}

/// Outcome of a successful fetch+decrypt. Either half may be absent
/// depending on what's on the server. Passkey cubes never carry a
/// seed half; mnemonic cubes without a Vault never carry a descriptor.
///
/// `SeedBlob` isn't wrapped in `Zeroizing` because the type doesn't
/// impl `Zeroize` at the struct level — the sensitive mnemonic field
/// inside it derives `ZeroizeOnDrop`, so the phrase material is wiped
/// automatically when the blob drops regardless of how callers hold
/// it. Don't clone `SeedBlob` unnecessarily in UI code.
#[derive(Debug)]
pub struct DecryptedKit {
    pub seed: Option<SeedBlob>,
    pub descriptor: Option<DescriptorBlob>,
}

/// Decrypts a single ciphertext envelope into `SeedBlob`.
pub fn decrypt_seed_blob(
    ciphertext_b64: &str,
    password: &Zeroizing<String>,
) -> Result<SeedBlob, RestoreError> {
    let bytes = decrypt(ciphertext_b64, password)?;
    let blob: SeedBlob = serde_json::from_slice(&bytes)
        .map_err(|e| RestoreError::BlobParse(format!("seed blob: {}", e)))?;
    Ok(blob)
}

/// Decrypts a single ciphertext envelope into `DescriptorBlob`.
/// No `Zeroizing` wrap — the descriptor isn't secret; it's the
/// public-facing half of the kit.
pub fn decrypt_descriptor_blob(
    ciphertext_b64: &str,
    password: &Zeroizing<String>,
) -> Result<DescriptorBlob, RestoreError> {
    let bytes = decrypt(ciphertext_b64, password)?;
    let blob: DescriptorBlob = serde_json::from_slice(&bytes)
        .map_err(|e| RestoreError::BlobParse(format!("descriptor blob: {}", e)))?;
    Ok(blob)
}

/// Fetches the kit from Connect and decrypts whichever halves it
/// carries using the supplied password. Either half may come back
/// as `None` when the server-side kit is partial (see plan §5 on
/// partial-field kits).
pub async fn fetch_and_decrypt_kit(
    client: &CoincubeClient,
    cube_id: u64,
    password: &Zeroizing<String>,
) -> Result<DecryptedKit, RestoreError> {
    let kit = client.get_recovery_kit(cube_id).await?;

    // Empty strings on the wire mean "this half wasn't uploaded."
    // Treat them the same as `None` here so the caller can pattern-
    // match cleanly.
    let seed = if kit.encrypted_cube_seed.is_empty() {
        None
    } else {
        Some(decrypt_seed_blob(&kit.encrypted_cube_seed, password)?)
    };
    let descriptor = if kit.encrypted_wallet_descriptor.is_empty() {
        None
    } else {
        Some(decrypt_descriptor_blob(
            &kit.encrypted_wallet_descriptor,
            password,
        )?)
    };

    Ok(DecryptedKit { seed, descriptor })
}

/// Specialisation for W14 and W15 — the restore-descriptor-only
/// flows. Converts "kit exists but has no descriptor half" into a
/// typed `HalfMissing` so the UI copy can be precise.
pub async fn fetch_and_decrypt_descriptor(
    client: &CoincubeClient,
    cube_id: u64,
    password: &Zeroizing<String>,
) -> Result<DescriptorBlob, RestoreError> {
    let DecryptedKit { descriptor, .. } = fetch_and_decrypt_kit(client, cube_id, password).await?;
    descriptor.ok_or(RestoreError::HalfMissing)
}

/// Specialisation for W13 — the full-install restore flow. Returns
/// the seed half paired with optional descriptor; missing seed is
/// `HalfMissing` because a fresh install can't meaningfully proceed
/// without one. The installer flow can still apply the descriptor
/// after the seed is set up.
pub async fn fetch_and_decrypt_for_install(
    client: &CoincubeClient,
    cube_id: u64,
    password: &Zeroizing<String>,
) -> Result<(SeedBlob, Option<DescriptorBlob>), RestoreError> {
    let DecryptedKit { seed, descriptor } =
        fetch_and_decrypt_kit(client, cube_id, password).await?;
    let seed = seed.ok_or(RestoreError::HalfMissing)?;
    Ok((seed, descriptor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::recovery::{
        encrypt, DescriptorBlobCube, DescriptorBlobVault, KdfParams, SeedBlobCube,
        SeedBlobMnemonic, BLOB_VERSION,
    };

    // Minimum-cost params — keeps tests fast.
    const TEST_PARAMS: KdfParams = KdfParams {
        memory_kib: 512,
        t_cost: 1,
        p_cost: 1,
    };

    fn pw(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_string())
    }

    fn sample_seed_blob() -> SeedBlob {
        SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: "cube-uuid".into(),
                name: "My Cube".into(),
                network: "bitcoin".into(),
                created_at: "2026-04-22T00:00:00Z".into(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: "abandon abandon abandon abandon abandon abandon abandon abandon \
                         abandon abandon abandon about"
                    .into(),
                language: "en".into(),
            },
        }
    }

    fn sample_descriptor_blob() -> DescriptorBlob {
        DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "cube-uuid".into(),
                network: "bitcoin".into(),
            },
            vault: DescriptorBlobVault {
                name: "My Vault".into(),
                descriptor: "wsh(...)".into(),
                change_descriptor: None,
                signers: vec![],
            },
        }
    }

    #[test]
    fn decrypt_seed_blob_roundtrips() {
        let password = pw("correct horse battery staple");
        let blob = sample_seed_blob();
        let bytes = serde_json::to_vec(&blob).unwrap();
        let envelope = encrypt(&bytes, &password, TEST_PARAMS).unwrap();

        let got = decrypt_seed_blob(&envelope, &password).unwrap();
        assert_eq!(got.cube.uuid, blob.cube.uuid);
        assert_eq!(got.mnemonic.phrase, blob.mnemonic.phrase);
    }

    #[test]
    fn decrypt_descriptor_blob_roundtrips() {
        let password = pw("correct horse battery staple");
        let blob = sample_descriptor_blob();
        let bytes = serde_json::to_vec(&blob).unwrap();
        let envelope = encrypt(&bytes, &password, TEST_PARAMS).unwrap();

        let got = decrypt_descriptor_blob(&envelope, &password).unwrap();
        assert_eq!(got.vault.name, blob.vault.name);
        assert_eq!(got.vault.descriptor, blob.vault.descriptor);
    }

    #[test]
    fn wrong_password_maps_to_bad_or_corrupt() {
        let bytes = serde_json::to_vec(&sample_seed_blob()).unwrap();
        let envelope = encrypt(&bytes, &pw("right"), TEST_PARAMS).unwrap();
        let err = decrypt_seed_blob(&envelope, &pw("wrong")).unwrap_err();
        assert!(matches!(err, RestoreError::BadPasswordOrCorrupt));
    }

    #[test]
    fn corrupt_plaintext_maps_to_blob_parse() {
        // Encrypt garbage (valid envelope, invalid JSON payload). The
        // decrypt step succeeds but JSON parsing fails — this is a
        // separate error case from a wrong password / tampered tag.
        let password = pw("pw");
        let envelope = encrypt(b"not-json", &password, TEST_PARAMS).unwrap();
        let err = decrypt_seed_blob(&envelope, &password).unwrap_err();
        assert!(
            matches!(err, RestoreError::BlobParse(_)),
            "expected BlobParse, got {:?}",
            err
        );
    }

    #[test]
    fn coincube_not_found_maps_to_restore_not_found() {
        let err: RestoreError = CoincubeError::NotFound.into();
        assert!(matches!(err, RestoreError::NotFound));
    }

    #[test]
    fn coincube_rate_limited_preserves_retry_after() {
        let err: RestoreError = CoincubeError::RateLimited {
            retry_after: std::time::Duration::from_secs(42),
        }
        .into();
        match err {
            RestoreError::RateLimited { retry_after } => {
                assert_eq!(retry_after, std::time::Duration::from_secs(42));
            }
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }
}

#[cfg(test)]
mod integration_tests {
    //! Integration tests against a mocked Connect API. Exercises the
    //! full fetch→decrypt path including the typed error mapping
    //! from the client layer.
    use super::*;
    use crate::services::recovery::{encrypt, KdfParams, BLOB_VERSION};
    use crate::services::recovery::{
        DescriptorBlobCube, DescriptorBlobVault, SeedBlobCube, SeedBlobMnemonic,
    };
    use httpmock::{Method as MockMethod, MockServer};
    use serde_json::json;

    const TEST_PARAMS: KdfParams = KdfParams {
        memory_kib: 512,
        t_cost: 1,
        p_cost: 1,
    };

    fn pw(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_string())
    }

    fn sample_seed_blob() -> SeedBlob {
        SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: "cube-uuid".into(),
                name: "My Cube".into(),
                network: "bitcoin".into(),
                created_at: "2026-04-22T00:00:00Z".into(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: "a".repeat(50),
                language: "en".into(),
            },
        }
    }

    fn sample_descriptor_blob() -> DescriptorBlob {
        DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "cube-uuid".into(),
                network: "bitcoin".into(),
            },
            vault: DescriptorBlobVault {
                name: "My Vault".into(),
                descriptor: "wsh(multi(2,xpub1,xpub2))".into(),
                change_descriptor: None,
                signers: vec![],
            },
        }
    }

    #[tokio::test]
    async fn fetch_and_decrypt_kit_returns_both_halves() {
        let password = pw("correct horse battery staple");
        let seed_ct = encrypt(
            &serde_json::to_vec(&sample_seed_blob()).unwrap(),
            &password,
            TEST_PARAMS,
        )
        .unwrap();
        let desc_ct = encrypt(
            &serde_json::to_vec(&sample_descriptor_blob()).unwrap(),
            &password,
            TEST_PARAMS,
        )
        .unwrap();

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 1,
                        "cubeId": 42,
                        "encryptedCubeSeed": seed_ct,
                        "encryptedWalletDescriptor": desc_ct,
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let kit = fetch_and_decrypt_kit(&client, 42, &password)
            .await
            .expect("fetch+decrypt should succeed");
        mock.assert();
        assert!(kit.seed.is_some());
        assert!(kit.descriptor.is_some());
        assert_eq!(kit.seed.unwrap().cube.uuid, "cube-uuid");
        assert_eq!(kit.descriptor.unwrap().vault.name, "My Vault");
    }

    #[tokio::test]
    async fn fetch_and_decrypt_kit_handles_seed_only() {
        let password = pw("pw");
        let seed_ct = encrypt(
            &serde_json::to_vec(&sample_seed_blob()).unwrap(),
            &password,
            TEST_PARAMS,
        )
        .unwrap();

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 1,
                        "cubeId": 42,
                        "encryptedCubeSeed": seed_ct,
                        "encryptedWalletDescriptor": "",
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let kit = fetch_and_decrypt_kit(&client, 42, &password)
            .await
            .expect("fetch+decrypt should succeed");
        mock.assert();
        assert!(kit.seed.is_some());
        assert!(kit.descriptor.is_none());
    }

    #[tokio::test]
    async fn fetch_descriptor_on_seed_only_kit_reports_half_missing() {
        // W14/W15 case: the user has a seed-only kit and clicks
        // "Restore descriptor from Connect" — we expect the typed
        // HalfMissing error, not a generic parse failure.
        let password = pw("pw");
        let seed_ct = encrypt(
            &serde_json::to_vec(&sample_seed_blob()).unwrap(),
            &password,
            TEST_PARAMS,
        )
        .unwrap();

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 1,
                        "cubeId": 42,
                        "encryptedCubeSeed": seed_ct,
                        "encryptedWalletDescriptor": "",
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_and_decrypt_descriptor(&client, 42, &password)
            .await
            .expect_err("expected HalfMissing");
        mock.assert();
        assert!(matches!(err, RestoreError::HalfMissing));
    }

    #[tokio::test]
    async fn fetch_for_install_on_descriptor_only_kit_reports_half_missing() {
        // W13 case: passkey cube's descriptor-only kit; full install
        // restore needs the seed.
        let password = pw("pw");
        let desc_ct = encrypt(
            &serde_json::to_vec(&sample_descriptor_blob()).unwrap(),
            &password,
            TEST_PARAMS,
        )
        .unwrap();

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 1,
                        "cubeId": 42,
                        "encryptedCubeSeed": "",
                        "encryptedWalletDescriptor": desc_ct,
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_and_decrypt_for_install(&client, 42, &password)
            .await
            .expect_err("expected HalfMissing");
        mock.assert();
        assert!(matches!(err, RestoreError::HalfMissing));
    }

    #[tokio::test]
    async fn fetch_and_decrypt_kit_404_maps_to_not_found() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(404);
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_and_decrypt_kit(&client, 42, &pw("pw"))
            .await
            .expect_err("expected NotFound");
        mock.assert();
        assert!(matches!(err, RestoreError::NotFound));
    }

    #[tokio::test]
    async fn fetch_and_decrypt_kit_429_preserves_retry_after() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(429).header("Retry-After", "17");
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_and_decrypt_kit(&client, 42, &pw("pw"))
            .await
            .expect_err("expected RateLimited");
        mock.assert();
        match err {
            RestoreError::RateLimited { retry_after } => {
                assert_eq!(retry_after, std::time::Duration::from_secs(17));
            }
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn fetch_and_decrypt_kit_wrong_password_maps_to_bad_or_corrupt() {
        let right_pw = pw("right password");
        let wrong_pw = pw("wrong password");
        let seed_ct = encrypt(
            &serde_json::to_vec(&sample_seed_blob()).unwrap(),
            &right_pw,
            TEST_PARAMS,
        )
        .unwrap();

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": {
                        "id": 1,
                        "cubeId": 42,
                        "encryptedCubeSeed": seed_ct,
                        "encryptedWalletDescriptor": "",
                        "encryptionScheme": "aes-256-gcm",
                        "createdAt": "2026-04-22T00:00:00Z",
                        "updatedAt": "2026-04-22T00:00:00Z"
                    }
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_and_decrypt_kit(&client, 42, &wrong_pw)
            .await
            .expect_err("expected BadPasswordOrCorrupt");
        mock.assert();
        assert!(matches!(err, RestoreError::BadPasswordOrCorrupt));
    }
}
