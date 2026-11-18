//! Imported-key checkpoint transport. No legacy Recovery Kit fallback or
//! automatic mutation retry: a timeout can occur after the server commits.
#![allow(dead_code)] // Staged backend integration; no UI consumers yet.

use std::{fmt, time::Duration};

use reqwest::Method;
use serde::{de::DeserializeOwned, Deserialize};

use super::{client::parse_retry_after, CoincubeClient};

pub(crate) const MAX_CIPHERTEXT_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Checkpoint {
    pub generation: u64,
    pub commitment: String,
    pub active: bool,
}

impl Checkpoint {
    pub fn validate(&self) -> Result<(), CheckpointError> {
        let digest = self.commitment.len() == 64
            && self
                .commitment
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if (self.generation == 0 && (self.active || !self.commitment.is_empty()))
            || (self.generation > 0 && !digest && (self.active || !self.commitment.is_empty()))
        {
            return Err(CheckpointError::InvalidResponse);
        }
        Ok(())
    }
}

// Intentionally non-Clone, with redacted Debug even though it holds ciphertext.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckpointCiphertext {
    pub generation: u64,
    pub commitment: String,
    pub active: bool,
    pub ciphertext: String,
}

impl fmt::Debug for CheckpointCiphertext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckpointCiphertext")
            .finish_non_exhaustive()
    }
}

/// These errors never retain server bodies, ciphertext, passwords, or URLs.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CheckpointError {
    SignInRequired,
    DeviceSignInRequired,
    Forbidden,
    NotFound,
    Conflict,
    Locked,
    TrustedDeviceDelay {
        available_at: Option<chrono::DateTime<chrono::Utc>>,
    },
    RateLimited {
        retry_after: Duration,
    },
    Unavailable,
    InvalidResponse,
    InvalidRequest,
}

impl fmt::Display for CheckpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DeviceSignInRequired => "Sign in on this device to download the Recovery Kit.",
            Self::SignInRequired => "Sign in to Connect to access this Recovery Kit.",
            Self::Forbidden => "This session cannot access the Recovery Kit.",
            Self::NotFound => "The imported Recovery Kit is unavailable.",
            Self::Conflict => "The Recovery Kit checkpoint changed; reload its status.",
            Self::Locked => "Recovery Kit access is locked.",
            Self::TrustedDeviceDelay { .. } => "Recovery Kit download is delayed on this device.",
            Self::RateLimited { .. } => "Recovery Kit access is rate limited; try again later.",
            Self::Unavailable => "Cannot verify the current Recovery Kit checkpoint.",
            Self::InvalidResponse => "The Recovery Kit checkpoint response is invalid.",
            Self::InvalidRequest => "The Recovery Kit checkpoint request is invalid.",
        })
    }
}

impl std::error::Error for CheckpointError {}

#[derive(Deserialize)]
struct Envelope<T> {
    success: bool,
    data: Option<T>,
}

async fn bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>, CheckpointError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CheckpointError::Unavailable)?
    {
        if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
            return Err(CheckpointError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn locked_checkpoint_error(response: reqwest::Response) -> CheckpointError {
    // The server uses 423 for both duress and a new device's wait.
    // Retain the legacy envelope's dated delay, but never retain its
    // message/body. Opaque, truncated, or oversized locks stay neutral.
    let locked = match bounded_body(response).await {
        Ok(bytes) => std::str::from_utf8(&bytes)
            .ok()
            .map(super::DownloadError::from_locked_body),
        Err(_) => None,
    };
    match locked {
        Some(super::DownloadError::TrustedDeviceDelay { available_at }) => {
            CheckpointError::TrustedDeviceDelay { available_at }
        }
        _ => CheckpointError::Locked,
    }
}

impl CoincubeClient {
    async fn imported_checkpoint_request<T: DeserializeOwned>(
        &self,
        cube_id: u64,
        method: Method,
        resource: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, CheckpointError> {
        if self.token().is_none() {
            return Err(CheckpointError::SignInRequired);
        }
        if cube_id == 0 {
            return Err(CheckpointError::InvalidRequest);
        }
        let url = format!("{}/api/v1/connect/cubes/{cube_id}{resource}", self.base_url);
        // An anchor response must originate at the configured API. Following
        // redirects can disclose the opaque PUT body/device identity and make
        // another origin's status appear authenticated.
        let transport = self
            .anchor_transport()
            .map_err(|_| CheckpointError::Unavailable)?;
        let mut request = transport.request(method, url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| CheckpointError::Unavailable)?;
        let status = response.status().as_u16();
        // Read headers before consuming the response.
        if status == 429 {
            return Err(CheckpointError::RateLimited {
                retry_after: parse_retry_after(response.headers()),
            });
        }
        if status == 423 {
            return Err(locked_checkpoint_error(response).await);
        }
        match status {
            300..=399 => return Err(CheckpointError::InvalidResponse),
            401 => return Err(CheckpointError::SignInRequired),
            404 => return Err(CheckpointError::NotFound),
            409 => return Err(CheckpointError::Conflict),
            408 | 500..=599 => return Err(CheckpointError::Unavailable),
            _ => {}
        }
        let bytes = bounded_body(response).await?;
        if status == 403 {
            let device_required = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| v.get("error")?.get("code")?.as_str().map(str::to_owned))
                .as_deref()
                == Some("DEVICE_BOUND_SESSION_REQUIRED");
            return Err(if device_required {
                CheckpointError::DeviceSignInRequired
            } else {
                CheckpointError::Forbidden
            });
        }
        if !(200..300).contains(&status) {
            return Err(CheckpointError::InvalidRequest);
        }
        let envelope: Envelope<T> =
            serde_json::from_slice(&bytes).map_err(|_| CheckpointError::InvalidResponse)?;
        if !envelope.success {
            return Err(CheckpointError::InvalidResponse);
        }
        envelope.data.ok_or(CheckpointError::InvalidResponse)
    }

    /// Authenticated same-origin scope read for checkpoint transactions.
    pub(crate) async fn get_imported_checkpoint_cube(
        &self,
        cube_id: u64,
    ) -> Result<super::CubeResponse, CheckpointError> {
        let cube: super::CubeResponse = self
            .imported_checkpoint_request(cube_id, Method::GET, "", None)
            .await?;
        if cube.id != cube_id || cube.uuid.is_empty() {
            return Err(CheckpointError::InvalidResponse);
        }
        Ok(cube)
    }

    pub(crate) async fn get_imported_checkpoint(
        &self,
        cube_id: u64,
    ) -> Result<Checkpoint, CheckpointError> {
        let result: Checkpoint = self
            .imported_checkpoint_request(
                cube_id,
                Method::GET,
                "/recovery-kit/imported-checkpoint",
                None,
            )
            .await?;
        result.validate()?;
        Ok(result)
    }

    pub(crate) async fn get_imported_checkpoint_ciphertext(
        &self,
        cube_id: u64,
    ) -> Result<CheckpointCiphertext, CheckpointError> {
        self.imported_checkpoint_request(
            cube_id,
            Method::GET,
            "/recovery-kit/imported-checkpoint/ciphertext",
            None,
        )
        .await
    }

    pub(crate) async fn put_imported_checkpoint(
        &self,
        cube_id: u64,
        expected_generation: u64,
        ciphertext: &str,
    ) -> Result<Checkpoint, CheckpointError> {
        if ciphertext.is_empty()
            || ciphertext.len() > MAX_CIPHERTEXT_BYTES
            || expected_generation == u64::MAX
        {
            return Err(CheckpointError::InvalidRequest);
        }
        let result: Checkpoint = self.imported_checkpoint_request(cube_id, Method::PUT, "/recovery-kit/imported-checkpoint", Some(serde_json::json!({"expectedGeneration": expected_generation, "ciphertext": ciphertext}))).await?;
        result.validate()?;
        Ok(result)
    }

    pub(crate) async fn delete_imported_checkpoint(
        &self,
        cube_id: u64,
        expected_generation: u64,
    ) -> Result<Checkpoint, CheckpointError> {
        if expected_generation == u64::MAX {
            return Err(CheckpointError::InvalidRequest);
        }
        let result: Checkpoint = self
            .imported_checkpoint_request(
                cube_id,
                Method::DELETE,
                "/recovery-kit/imported-checkpoint",
                Some(serde_json::json!({"expectedGeneration": expected_generation})),
            )
            .await?;
        result.validate()?;
        if result.active || result.generation != expected_generation + 1 {
            return Err(CheckpointError::InvalidResponse);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method as MockMethod, MockServer};
    use serde_json::json;

    fn client(server: &MockServer) -> CoincubeClient {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("X-Device-Fingerprint", "synthetic-device".parse().unwrap());
        let mut client = CoincubeClient::for_test_with_identity(server.base_url(), headers);
        client.set_token("synthetic-test-token");
        client
    }

    #[tokio::test]
    async fn checkpoint_transport_uses_authenticated_cas_without_legacy_fallback() {
        let server = MockServer::start();
        let publish = server.mock(|when, then| {
            when.method(MockMethod::PUT)
                .path("/api/v1/connect/cubes/42/recovery-kit/imported-checkpoint")
                .header("authorization", "Bearer synthetic-test-token")
                .header("X-Device-Fingerprint", "synthetic-device")
                .json_body(json!({"expectedGeneration": 0, "ciphertext": "opaque"}));
            then.status(409);
        });
        assert_eq!(
            client(&server)
                .put_imported_checkpoint(42, 0, "opaque")
                .await
                .unwrap_err(),
            CheckpointError::Conflict
        );
        publish.assert_hits(1);
        let revoke = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit/imported-checkpoint")
                .json_body(json!({"expectedGeneration": 7}));
            then.status(200).json_body(json!({"success": true, "data": {"generation": 8, "commitment": "a".repeat(64), "active": false}}));
        });
        assert!(
            !client(&server)
                .delete_imported_checkpoint(42, 7)
                .await
                .unwrap()
                .active
        );
        revoke.assert_hits(1);
    }

    #[tokio::test]
    async fn checkpoint_initial_tombstone_fences_a_first_publication() {
        let server = MockServer::start();
        let revoke = server.mock(|when, then| {
            when.method(MockMethod::DELETE)
                .path("/api/v1/connect/cubes/42/recovery-kit/imported-checkpoint")
                .json_body(json!({"expectedGeneration":0}));
            then.status(200).json_body(
                json!({"success":true,"data":{"generation":1,"commitment":"","active":false}}),
            );
        });
        let status = server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/imported-checkpoint");
            then.status(200).json_body(
                json!({"success":true,"data":{"generation":1,"commitment":"","active":false}}),
            );
        });
        server.mock(|when, then| {
            when.method(MockMethod::PUT)
                .json_body(json!({"expectedGeneration":0,"ciphertext":"late"}));
            then.status(409);
        });
        server.mock(|when, then| {
            when.method(MockMethod::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/imported-checkpoint/ciphertext");
            then.status(404);
        });
        let client = client(&server);
        assert_eq!(
            client
                .get_imported_checkpoint_ciphertext(42)
                .await
                .unwrap_err(),
            CheckpointError::NotFound
        );
        assert_eq!(
            client.delete_imported_checkpoint(42, 0).await.unwrap(),
            client.get_imported_checkpoint(42).await.unwrap()
        );
        assert_eq!(
            client
                .put_imported_checkpoint(42, 0, "late")
                .await
                .unwrap_err(),
            CheckpointError::Conflict
        );
        revoke.assert_hits(1);
        status.assert_hits(1);
        assert!(Checkpoint {
            generation: 1,
            commitment: String::new(),
            active: true
        }
        .validate()
        .is_err());
    }

    #[tokio::test]
    async fn checkpoint_device_sign_in_is_distinct_from_invalid_session() {
        for (status, code, expected) in [
            (
                403,
                "DEVICE_BOUND_SESSION_REQUIRED",
                CheckpointError::DeviceSignInRequired,
            ),
            (403, "FORBIDDEN", CheckpointError::Forbidden),
            (401, "UNAUTHORIZED", CheckpointError::SignInRequired),
            (423, "DURESS_LOCKED", CheckpointError::Locked),
            (404, "NOT_FOUND", CheckpointError::NotFound),
        ] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(MockMethod::GET).path("/api/v1/connect/cubes/42/recovery-kit/imported-checkpoint/ciphertext");
                then.status(status).json_body(json!({"success": false, "error": {"code": code, "message": "private server text"}}));
            });
            let error = client(&server)
                .get_imported_checkpoint_ciphertext(42)
                .await
                .unwrap_err();
            assert_eq!(error, expected);
            assert!(!format!("{error:?} {error}").contains("private server text"));
        }
    }

    #[tokio::test]
    async fn checkpoint_locked_response_preserves_trusted_device_wait_and_neutral_fallback() {
        let available_at = "2026-11-03T14:00:00Z";
        let dated = CheckpointError::TrustedDeviceDelay {
            available_at: Some(available_at.parse().unwrap()),
        };
        for (body, expected) in [
            (json!({"success": false, "error": {"code": "TRUSTED_DEVICE_DELAY", "message": "private server text"}, "data": {"available_at": available_at}}).to_string(), dated),
            (json!({"error": {"code": "TRUSTED_DEVICE_DELAY"}}).to_string(), CheckpointError::TrustedDeviceDelay { available_at: None }),
            (json!({"error": {"code": "DURESS_LOCKED"}, "data": {"available_at": available_at}}).to_string(), CheckpointError::Locked),
            (json!({"error": {"code": "TRUSTED_DEVICE_DELAY"}, "data": {"available_at": "malformed"}}).to_string(), CheckpointError::Locked),
            ("not JSON private server text".into(), CheckpointError::Locked),
            (json!({"error": {"code": "TRUSTED_DEVICE_DELAY", "message": "x".repeat(MAX_RESPONSE_BYTES)}, "data": {"available_at": available_at}}).to_string(), CheckpointError::Locked),
        ] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(MockMethod::GET);
                then.status(423).body(body);
            });
            let error = client(&server).get_imported_checkpoint_ciphertext(42).await.unwrap_err();
            assert_eq!(error, expected);
            assert!(!format!("{error:?} {error}").contains("private server text"));
        }
    }

    #[tokio::test]
    async fn checkpoint_response_is_bounded_and_shape_checked() {
        for data in [
            json!({"generation": 0, "commitment": "", "active": true}),
            json!({"generation": 1, "commitment": "A".repeat(64), "active": true}),
            json!({"generation": 1, "commitment": "a".repeat(64)}),
        ] {
            let server = MockServer::start();
            server.mock(|when, then| {
                when.method(MockMethod::GET);
                then.status(200)
                    .json_body(json!({"success": true, "data": data}));
            });
            assert_eq!(
                client(&server)
                    .get_imported_checkpoint(42)
                    .await
                    .unwrap_err(),
                CheckpointError::InvalidResponse
            );
        }
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(MockMethod::GET);
            then.status(200).body("x".repeat(MAX_RESPONSE_BYTES + 1));
        });
        assert_eq!(
            client(&server)
                .get_imported_checkpoint(42)
                .await
                .unwrap_err(),
            CheckpointError::InvalidResponse
        );
    }

    #[tokio::test]
    async fn checkpoint_redirects_cannot_forward_ciphertext_or_supply_authority() {
        let sink = MockServer::start();
        let forwarded = sink.mock(|_when, then| {
            then.status(200).json_body(json!({"success": true, "data": {"generation": 1, "commitment": "a".repeat(64), "active": true}}));
        });
        for status in [301, 302, 303, 307, 308] {
            let server = MockServer::start();
            let redirected = server.mock(|_when, then| {
                then.status(status)
                    .header("Location", sink.url("/untrusted"));
            });
            let client = client(&server);
            assert_eq!(
                client
                    .put_imported_checkpoint(42, 0, "private opaque ciphertext")
                    .await
                    .unwrap_err(),
                CheckpointError::InvalidResponse
            );
            assert_eq!(
                client.get_imported_checkpoint(42).await.unwrap_err(),
                CheckpointError::InvalidResponse
            );
            assert_eq!(
                client
                    .get_imported_checkpoint_ciphertext(42)
                    .await
                    .unwrap_err(),
                CheckpointError::InvalidResponse
            );
            assert_eq!(
                client.delete_imported_checkpoint(42, 1).await.unwrap_err(),
                CheckpointError::InvalidResponse
            );
            assert_eq!(
                client.get_imported_checkpoint_cube(42).await.unwrap_err(),
                CheckpointError::InvalidResponse
            );
            redirected.assert_hits(5);
        }
        forwarded.assert_hits(0);
    }

    #[tokio::test]
    async fn checkpoint_cube_scope_requires_same_authenticated_id_and_uuid() {
        for (id, uuid, expected_ok) in [
            (42, "cube-uuid", true),
            (43, "cube-uuid", false),
            (42, "", false),
        ] {
            let server = MockServer::start();
            let mock = server.mock(|when, then| {
                when.method(MockMethod::GET).path("/api/v1/connect/cubes/42")
                    .header("authorization", "Bearer synthetic-test-token");
                then.status(200).json_body(json!({"success":true,"data":{"id":id,"uuid":uuid,"network":"regtest","name":"Synthetic","status":"active"}}));
            });
            assert_eq!(
                client(&server)
                    .get_imported_checkpoint_cube(42)
                    .await
                    .is_ok(),
                expected_ok
            );
            mock.assert_hits(1);
        }
    }

    #[tokio::test]
    async fn checkpoint_rate_limit_preserves_retry_after() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(MockMethod::GET);
            then.status(429).header("Retry-After", "75");
        });
        assert_eq!(
            client(&server)
                .get_imported_checkpoint(42)
                .await
                .unwrap_err(),
            CheckpointError::RateLimited {
                retry_after: Duration::from_secs(75)
            }
        );
        let no_token = CoincubeClient::for_test(server.base_url());
        assert_eq!(
            no_token.get_imported_checkpoint(42).await.unwrap_err(),
            CheckpointError::SignInRequired
        );
    }
}
