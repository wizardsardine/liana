//! Integration tests against the live `coincube-api` dev gRPC endpoint.
//!
//! These tests are gated behind the `integration-tests` feature so they
//! never run on a hermetic PR CI. Enable with:
//!
//! ```sh
//! COINCUBE_API_URL=https://dev-api.example.com \
//!   COINCUBE_INTEGRATION_TOKEN=<jwt> \
//!   cargo test -p coincube-gui --features integration-tests --test connect_grpc
//! ```
//!
//! What's covered today:
//! - Stream connect/disconnect lifecycle.
//! - `RegisterDevice` idempotency.
//! - `ResolveSigners` against a known fixture vault — currently a skeleton
//!   that asserts the call succeeds; richer "expected signer set" checks
//!   are blocked on API-side fixture support (see notes below).
//!
//! What's NOT covered:
//! - End-to-end signing. Requires a phone-side mock or CLI that can submit
//!   partial signatures. Defer until the test infrastructure exists; the
//!   `signing_flow` test file alongside this one documents the test plan.

#![cfg(feature = "integration-tests")]

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio_stream::StreamExt;

use coincube_gui::services::connect::client::auth::AccessTokenResponse;
use coincube_gui::services::connect::grpc::{
    self, connect_v1,
    interceptor::AuthInterceptor,
    session::GrpcSessionClient,
    stream::{connect_stream, ConnectStreamConfig},
    ConnectStreamMessage,
};

/// Env-driven test config. We don't panic in `setup()` itself — let
/// `#[test]` cases call `.expect("...")` so a missing variable surfaces
/// as a clear failure rather than a panic at collection time.
struct TestEnv {
    grpc_url: String,
    token: String,
    /// `vault_id` for `ResolveSigners` smoke tests. Optional because not
    /// every dev account has a fixture vault provisioned.
    vault_id: Option<String>,
}

impl TestEnv {
    fn from_env() -> Result<Self, String> {
        Ok(Self {
            grpc_url: std::env::var("COINCUBE_API_URL")
                .map_err(|_| "COINCUBE_API_URL must be set".to_string())?,
            token: std::env::var("COINCUBE_INTEGRATION_TOKEN")
                .map_err(|_| "COINCUBE_INTEGRATION_TOKEN must be set".to_string())?,
            vault_id: std::env::var("COINCUBE_INTEGRATION_VAULT_ID").ok(),
        })
    }

    fn tokens(&self) -> Arc<RwLock<AccessTokenResponse>> {
        Arc::new(RwLock::new(AccessTokenResponse {
            access_token: self.token.clone(),
            expires_at: i64::MAX, // skip the local refresh path
            refresh_token: String::new(),
        }))
    }
}

#[tokio::test]
async fn stream_connects_and_emits_connected_message() {
    let env = TestEnv::from_env().expect("integration env");
    let config = ConnectStreamConfig {
        grpc_url: env.grpc_url.clone(),
        tokens: env.tokens(),
        device_id: "integration-test-device".to_string(),
        user_agent: "coincube-gui-integration/0.0".to_string(),
        vault_ids: Vec::new(),
        last_seen_seq: 0,
    };

    let stream = connect_stream(&config);
    tokio::pin!(stream);

    // Allow a few seconds for the gRPC handshake. If the dev API is
    // slow this may need bumping; integration tests run on nightly CI
    // so a 10s budget is acceptable.
    let timeout = Duration::from_secs(10);
    let evt = tokio::time::timeout(timeout, stream.next())
        .await
        .expect("stream produced an event within 10s")
        .expect("stream not empty");
    match evt {
        ConnectStreamMessage::Connected => {}
        ConnectStreamMessage::Error(e) => {
            panic!("stream emitted Error instead of Connected: {}", e)
        }
        other => panic!("first event should be Connected, got {:?}", other),
    }
}

#[tokio::test]
async fn register_device_is_idempotent() {
    let env = TestEnv::from_env().expect("integration env");
    let channel = grpc::create_channel(&env.grpc_url)
        .await
        .expect("create_channel");
    let mut client = coincube_gui::services::connect::grpc::device::GrpcDeviceClient::new(
        channel.clone(),
        AuthInterceptor::new(&env.token),
    );

    // Two register calls with the same device_name should yield the same
    // device_id. The server's idempotency key is (user_id, device_name).
    let first = client
        .register_device(
            "integration-test-device".to_string(),
            "0.0.0".to_string(),
            std::env::consts::OS.to_string(),
        )
        .await
        .expect("first RegisterDevice");
    let second = client
        .register_device(
            "integration-test-device".to_string(),
            "0.0.0".to_string(),
            std::env::consts::OS.to_string(),
        )
        .await
        .expect("second RegisterDevice");
    assert_eq!(
        first.device_id, second.device_id,
        "RegisterDevice should return the same device_id for repeated calls",
    );
}

#[tokio::test]
async fn resolve_signers_against_fixture_vault() {
    let env = TestEnv::from_env().expect("integration env");
    let Some(vault_id) = env.vault_id.clone() else {
        eprintln!(
            "Skipping resolve_signers_against_fixture_vault — set \
             COINCUBE_INTEGRATION_VAULT_ID to enable."
        );
        return;
    };

    let channel = grpc::create_channel(&env.grpc_url)
        .await
        .expect("create_channel");
    let mut client = GrpcSessionClient::new(channel, AuthInterceptor::new(&env.token));
    let resp: connect_v1::ResolveSignersResponse = client
        .resolve_signers(vault_id)
        .await
        .expect("ResolveSigners");
    // Soft assertion: at least one target or one unresolved entry must
    // come back; an empty response indicates a misconfigured fixture
    // (no Keychain members on the vault). A stronger "expected signer
    // set" assertion is blocked on API-side fixture support — the
    // test account needs a stable, known set of keys.
    assert!(
        !resp.targets.is_empty() || !resp.unresolved.is_empty(),
        "ResolveSigners returned no targets and no unresolved — fixture vault likely empty"
    );
}
