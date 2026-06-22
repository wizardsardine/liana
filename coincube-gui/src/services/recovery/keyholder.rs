//! Heir/keyholder descriptor release (COIN-377, PR 2).
//!
//! Unlike the owner restore path in [`super::restore`], this path carries **no
//! password** and does **no client-side decryption**: the server decrypts the
//! escrowed descriptor under its KEK and returns plaintext to an authorised
//! keyholder (`coincube-api .../monitoring/handler.go::GetRecoveryDescriptor`).
//! So this module is just the fetch plus a typed mapping of the server's gate
//! matrix into UI-actionable variants.
//!
//! The duress case (`423`) is deliberately collapsed to a neutral
//! "unavailable, try later" — the heir UI must never explain *why* (invariant
//! I3); the owner being under duress is not disclosed.

use crate::services::coincube::{ApiErrorResponse, CoincubeClient, CoincubeError};

/// Typed outcome of a keyholder descriptor fetch. Each variant maps a server
/// gate response to a distinct UI state; see
/// [`CoincubeClient::get_recovery_descriptor`](crate::services::coincube::CoincubeClient::get_recovery_descriptor)
/// for which status/body produces which.
///
/// `Clone` + `Debug` mirror [`super::RestoreError`] (Iced clones messages
/// between update/task/view); no variant carries secrets.
#[derive(Debug, Clone)]
pub enum KeyholderRecoveryError {
    /// `403 RECOVERY_ACCESS_DENIED` — the signed-in account is not a keyholder
    /// of this vault. Also the fail-closed default for any unrecognised `403`.
    NotKeyholder,
    /// `403 RECOVERY_NOT_AVAILABLE` — the recovery path is not open on-chain
    /// yet. A race from an `open` discovery row; shouldn't normally occur.
    NotOpen,
    /// `423 DURESS_LOCKED` — the **owner's** account is under a duress lock.
    /// Render the same neutral copy the duress flow uses; never explain duress.
    /// `retry_at` is the optional `data.duress_unlock_at` hint, surfaced only
    /// as a soft "try again later", never labelled as duress.
    Unavailable {
        retry_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    /// `404 RECOVERY_NOT_MONITORED` — the vault has no monitoring/escrow record
    /// (the owner never opted into Full monitoring).
    NotMonitored,
    /// `503` — the recovery surface is off (`ALERTS_RECOVERY_ENABLED=false`) or
    /// descriptor escrow isn't configured server-side.
    Unsupported,
    /// `429` — rate limited (the route caps at ~10/min per IP).
    RateLimited { retry_after: std::time::Duration },
    /// Anything else — auth, network, 5xx, parse.
    Api(String),
}

impl std::fmt::Display for KeyholderRecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotKeyholder => write!(
                f,
                "You don't have permission to recover this vault."
            ),
            Self::NotOpen => write!(
                f,
                "This vault's recovery window isn't open yet."
            ),
            // Neutral, duress-safe copy — matches the recovery-kit download
            // flow's wording so the two paths are indistinguishable.
            Self::Unavailable { .. } => write!(
                f,
                "Recovery is unavailable right now. Please try again later."
            ),
            Self::NotMonitored => write!(
                f,
                "This vault isn't set up for assisted recovery."
            ),
            Self::Unsupported => write!(
                f,
                "Assisted recovery isn't available on this server."
            ),
            Self::RateLimited { retry_after } => {
                write!(f, "Too many attempts — try again in {}s.", retry_after.as_secs())
            }
            Self::Api(msg) => write!(f, "Recovery error: {}", msg),
        }
    }
}

impl std::error::Error for KeyholderRecoveryError {}

impl From<CoincubeError> for KeyholderRecoveryError {
    fn from(e: CoincubeError) -> Self {
        match e {
            // The client maps 404 → NotFound (RECOVERY_NOT_MONITORED) and
            // 429 → RateLimited before we ever see them.
            CoincubeError::NotFound => Self::NotMonitored,
            CoincubeError::RateLimited { retry_after } => Self::RateLimited { retry_after },
            CoincubeError::Unsuccessful(ref info) => match info.status_code {
                403 => match error_code(&info.text).as_deref() {
                    Some("RECOVERY_NOT_AVAILABLE") => Self::NotOpen,
                    // RECOVERY_ACCESS_DENIED or any unrecognised 403 fails
                    // closed to "you can't recover this".
                    _ => Self::NotKeyholder,
                },
                423 => Self::Unavailable {
                    retry_at: duress_unlock_at(&info.text),
                },
                503 => Self::Unsupported,
                _ => Self::Api(e.to_string()),
            },
            other => Self::Api(other.to_string()),
        }
    }
}

/// Extracts `error.code` from a JSON error envelope, if present.
fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<ApiErrorResponse>(body)
        .ok()
        .map(|r| r.error.code)
}

/// Body of a `423 DURESS_LOCKED` response. The handler attaches the optional
/// unlock hint via `ErrorWithData`, so the timestamp rides on `data`
/// (snake-case `duress_unlock_at`) alongside the `error` envelope — a
/// different shape from the recovery-kit download's `423` body, so we parse it
/// here rather than reusing `DownloadError::from_locked_body`.
#[derive(serde::Deserialize)]
struct DuressLockEnvelope {
    #[serde(default)]
    data: Option<DuressLockData>,
}

#[derive(serde::Deserialize)]
struct DuressLockData {
    #[serde(default)]
    duress_unlock_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Parses the optional `data.duress_unlock_at` hint from a `423` body. Returns
/// `None` for an opaque/unparseable body — the variant is the same either way;
/// we just lose the soft retry hint.
fn duress_unlock_at(body: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    serde_json::from_str::<DuressLockEnvelope>(body)
        .ok()
        .and_then(|env| env.data)
        .and_then(|d| d.duress_unlock_at)
}

/// Fetches the plaintext recovery descriptor for a vault the caller is a
/// keyholder of, mapping the server's gate matrix to [`KeyholderRecoveryError`].
/// No password, no decryption — the returned string is a ready-to-import
/// watch-only descriptor.
pub async fn fetch_recovery_descriptor(
    client: &CoincubeClient,
    cube_id: u64,
) -> Result<String, KeyholderRecoveryError> {
    client
        .get_recovery_descriptor(cube_id)
        .await
        .map_err(KeyholderRecoveryError::from)
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::services::http::NotSuccessResponseInfo;

    fn unsuccessful(status: u16, text: &str) -> CoincubeError {
        CoincubeError::Unsuccessful(NotSuccessResponseInfo {
            status_code: status,
            text: text.to_string(),
        })
    }

    fn err_body(code: &str) -> String {
        format!(r#"{{"success":false,"error":{{"code":"{}","message":"x"}}}}"#, code)
    }

    #[test]
    fn forbidden_not_keyholder_maps_to_not_keyholder() {
        let err: KeyholderRecoveryError =
            unsuccessful(403, &err_body("RECOVERY_ACCESS_DENIED")).into();
        assert!(matches!(err, KeyholderRecoveryError::NotKeyholder));
    }

    #[test]
    fn forbidden_not_available_maps_to_not_open() {
        let err: KeyholderRecoveryError =
            unsuccessful(403, &err_body("RECOVERY_NOT_AVAILABLE")).into();
        assert!(matches!(err, KeyholderRecoveryError::NotOpen));
    }

    #[test]
    fn unknown_403_fails_closed_to_not_keyholder() {
        // An unrecognised (or unparseable) 403 must NOT leak as "open" or a
        // generic error — it stays a hard deny.
        let err: KeyholderRecoveryError = unsuccessful(403, "garbage").into();
        assert!(matches!(err, KeyholderRecoveryError::NotKeyholder));
    }

    #[test]
    fn locked_with_unlock_hint_maps_to_unavailable_with_retry() {
        let body = r#"{"success":false,"data":{"duress_unlock_at":"2026-07-01T00:00:00Z"},"error":{"code":"DURESS_LOCKED","message":"x"}}"#;
        let err: KeyholderRecoveryError = unsuccessful(423, body).into();
        match err {
            KeyholderRecoveryError::Unavailable { retry_at } => {
                assert_eq!(
                    retry_at.unwrap().to_rfc3339(),
                    "2026-07-01T00:00:00+00:00"
                );
            }
            other => panic!("expected Unavailable, got {:?}", other),
        }
    }

    #[test]
    fn opaque_423_maps_to_unavailable_without_hint() {
        let err: KeyholderRecoveryError = unsuccessful(423, "nope").into();
        assert!(matches!(
            err,
            KeyholderRecoveryError::Unavailable { retry_at: None }
        ));
    }

    #[test]
    fn not_found_maps_to_not_monitored() {
        let err: KeyholderRecoveryError = CoincubeError::NotFound.into();
        assert!(matches!(err, KeyholderRecoveryError::NotMonitored));
    }

    #[test]
    fn service_unavailable_maps_to_unsupported() {
        let err: KeyholderRecoveryError =
            unsuccessful(503, &err_body("ESCROW_NOT_CONFIGURED")).into();
        assert!(matches!(err, KeyholderRecoveryError::Unsupported));
    }

    #[test]
    fn rate_limited_preserves_retry_after() {
        let err: KeyholderRecoveryError = CoincubeError::RateLimited {
            retry_after: std::time::Duration::from_secs(31),
        }
        .into();
        match err {
            KeyholderRecoveryError::RateLimited { retry_after } => {
                assert_eq!(retry_after, std::time::Duration::from_secs(31));
            }
            other => panic!("expected RateLimited, got {:?}", other),
        }
    }

    #[test]
    fn server_5xx_stays_api_error() {
        let err: KeyholderRecoveryError = unsuccessful(500, "boom").into();
        assert!(matches!(err, KeyholderRecoveryError::Api(_)));
    }
}

#[cfg(test)]
mod integration_tests {
    //! End-to-end against a mocked Connect API, exercising the client's URL +
    //! status handling together with this module's gate mapping.
    use super::*;
    use crate::services::coincube::{RecoveryState, VaultMonitoringLevel};
    use httpmock::{Method as MockMethod, MockServer};
    use serde_json::json;

    const DESC_PATH: &str = "/api/v1/connect/cubes/42/vault/recovery-descriptor";

    #[tokio::test]
    async fn fetch_descriptor_200_returns_plaintext() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET).path(DESC_PATH);
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": { "descriptor": "wsh(multi(2,xpubA,xpubB))" },
                    "error": null
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let desc = fetch_recovery_descriptor(&client, 42)
            .await
            .expect("200 should yield a descriptor");
        mock.assert();
        assert_eq!(desc, "wsh(multi(2,xpubA,xpubB))");
    }

    #[tokio::test]
    async fn fetch_descriptor_403_not_available_maps_to_not_open() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET).path(DESC_PATH);
            then.status(403).json_body(json!({
                "success": false,
                "error": { "code": "RECOVERY_NOT_AVAILABLE", "message": "not open" }
            }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_recovery_descriptor(&client, 42)
            .await
            .expect_err("expected NotOpen");
        mock.assert();
        assert!(matches!(err, KeyholderRecoveryError::NotOpen));
    }

    #[tokio::test]
    async fn fetch_descriptor_403_access_denied_maps_to_not_keyholder() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET).path(DESC_PATH);
            then.status(403).json_body(json!({
                "success": false,
                "error": { "code": "RECOVERY_ACCESS_DENIED", "message": "nope" }
            }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_recovery_descriptor(&client, 42)
            .await
            .expect_err("expected NotKeyholder");
        mock.assert();
        assert!(matches!(err, KeyholderRecoveryError::NotKeyholder));
    }

    #[tokio::test]
    async fn fetch_descriptor_423_maps_to_neutral_unavailable() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET).path(DESC_PATH);
            then.status(423).json_body(json!({
                "success": false,
                "data": { "duress_unlock_at": "2026-07-01T00:00:00Z" },
                "error": { "code": "DURESS_LOCKED", "message": "locked" }
            }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_recovery_descriptor(&client, 42)
            .await
            .expect_err("expected Unavailable");
        mock.assert();
        // The duress reason must never reach the heir — only the neutral copy.
        assert_eq!(
            err.to_string(),
            "Recovery is unavailable right now. Please try again later."
        );
        assert!(matches!(
            err,
            KeyholderRecoveryError::Unavailable { retry_at: Some(_) }
        ));
    }

    #[tokio::test]
    async fn fetch_descriptor_404_maps_to_not_monitored() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET).path(DESC_PATH);
            then.status(404);
        });

        let client = CoincubeClient::for_test(server.base_url());
        let err = fetch_recovery_descriptor(&client, 42)
            .await
            .expect_err("expected NotMonitored");
        mock.assert();
        assert!(matches!(err, KeyholderRecoveryError::NotMonitored));
    }

    #[tokio::test]
    async fn list_recoverable_parses_rows_and_actionability() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/recoverable");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(json!({
                    "success": true,
                    "data": [
                        {
                            "cubeId": 7,
                            "ownerLabel": "Dad's Vault",
                            "monitoringLevel": "full",
                            "state": "available",
                            "requiresRecoveryPassword": false
                        },
                        {
                            "cubeId": 8,
                            "ownerLabel": "Mum's Vault",
                            "monitoringLevel": "heartbeat",
                            "state": "approaching",
                            "requiresRecoveryPassword": true
                        },
                        {
                            "cubeId": 9,
                            "ownerLabel": "Open but pw-gated",
                            "monitoringLevel": "heartbeat",
                            "state": "reminding",
                            "requiresRecoveryPassword": true
                        }
                    ],
                    "error": null
                }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let rows = client
            .list_recoverable_vaults()
            .await
            .expect("list should parse");
        mock.assert();
        assert_eq!(rows.len(), 3);

        // Row 0: Full + available → open + actionable now.
        assert_eq!(rows[0].cube_id, 7);
        assert_eq!(rows[0].monitoring_level, VaultMonitoringLevel::Full);
        assert_eq!(rows[0].recovery_state(), RecoveryState::Open);
        assert!(rows[0].is_recoverable_now());

        // Row 1: Heartbeat + approaching → not open, not actionable.
        assert_eq!(rows[1].recovery_state(), RecoveryState::Approaching);
        assert!(!rows[1].is_recoverable_now());
        assert!(rows[1].requires_recovery_password);

        // Row 2: open (`reminding`) but password-required → NOT actionable in
        // v1 (deferred to COIN-375), even though the window is open.
        assert_eq!(rows[2].recovery_state(), RecoveryState::Open);
        assert!(!rows[2].is_recoverable_now());
    }
}
