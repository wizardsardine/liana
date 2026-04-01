# Rust Desktop App — gRPC Connect Integration Plan

## Context

The COINCUBE desktop app (Rust) is the **session initiator** in the signing flow. It creates PSBT signing sessions, monitors their progress via a persistent gRPC stream, and receives signed results when the Keychain mobile app completes signing.

This plan covers integrating the desktop app with the Connect API's three gRPC services: `SessionService`, `DeviceService`, and `RealtimeService`, as defined in `coincube-api/grpc/connect.proto`.

---

## Desktop App Responsibilities

1. Authenticate to the Connect API using the existing JWT token (shared with REST backend)
2. Register as a desktop device on first launch
3. Create signing sessions when the user initiates a PSBT send
4. Maintain a persistent bidirectional gRPC stream for realtime session events
5. Display session progress (delivered, viewed, approved, signed, completed)
6. Receive signed PSBTs and merge signatures / continue broadcast
7. Allow the user to cancel pending sessions
8. Reconnect gracefully after network interruptions

---

## Dependencies

Add to `coincube-gui/Cargo.toml`:

```toml
[dependencies]
tonic = { version = "0.13", features = ["tls", "tls-roots"] }
prost = "0.13"
prost-types = "0.13"
tokio-stream = "0.1"
```

Update the existing `tokio` entry to add features:

```toml
tokio = { version = "1", features = ["signal", "sync", "time"] }
```

For codegen (build dependency):

```toml
[build-dependencies]
tonic-build = "0.13"
```

> **Note:** `uuid` is already in dependencies with `v4` and `serde` features — no addition needed.
> TLS features on `tonic` ensure production gRPC works with the same native cert store used by `reqwest`/`rustls-tls`.

---

## Proto Codegen

### Extending `build.rs`

The existing `coincube-gui/build.rs` handles `.env` loading and Windows resources. Append proto codegen after the existing logic:

```rust
// --- Existing code above (dotenvy, Windows resources) ---

// gRPC proto codegen
println!("cargo:rerun-if-changed=../grpc/connect.proto");
tonic_build::configure()
    .build_server(false)   // Client only — no server stubs
    .compile_protos(
        &["../grpc/connect.proto"],
        &["../grpc/"],
    )?;
```

> **Path note:** `build.rs` runs from the crate root (`coincube-gui/`). The proto file is copied into the repo at `grpc/connect.proto` so builds work regardless of external repo locations.

### Generated module

Access the generated types via:

```rust
pub mod connect_v1 {
    tonic::include_proto!("connect.v1");
}
```

---

## Architecture

### Module Layout

Place gRPC modules under `services/connect/grpc/` alongside the existing REST client code:

```
src/services/connect/
  client/                (existing — auth, backend REST, cache)
    auth.rs              — AuthClient, AccessTokenResponse
    backend/             — REST BackendClient (Daemon trait impl)
    cache.rs             — ConnectCache, connect.json persistence
    mod.rs               — ServiceConfig, get_service_config()
  grpc/                  (NEW)
    mod.rs               — Proto include, ConnectStreamMessage enum, channel factory
    interceptor.rs       — Auth interceptor (shares existing token cache)
    device.rs            — GrpcDeviceClient wrapper
    session.rs           — GrpcSessionClient wrapper
    stream.rs            — Realtime stream using iced::stream::channel()
  login.rs               (existing)
  mod.rs                 (existing — add `pub mod grpc;`)
```

### Service Clients (split, not monolithic)

Split into focused client structs matching existing patterns:

```rust
/// Session operations: create, get, cancel, list pending
pub struct GrpcSessionClient {
    inner: SessionServiceClient<InterceptedService<Channel, AuthInterceptor>>,
}

/// Device operations: register, update push token
pub struct GrpcDeviceClient {
    inner: DeviceServiceClient<InterceptedService<Channel, AuthInterceptor>>,
}
```

The realtime stream is a **function** returning `impl iced::futures::Stream`, not a client struct.

All clients share the same cloned `Channel` and `AuthInterceptor`. A factory function creates the channel once:

```rust
pub async fn create_channel(grpc_url: &str) -> Result<Channel, tonic::transport::Error> {
    let tls = ClientTlsConfig::new().with_native_roots();
    Channel::from_shared(grpc_url.to_string())?
        .tls_config(tls)?
        .connect()
        .await
}
```

---

## gRPC Endpoint URL

Extend `ServiceConfig` and `ServiceConfigResource` in `services/connect/client/mod.rs`:

```rust
pub struct ServiceConfig {
    pub auth_api_url: String,
    pub auth_api_public_key: String,
    pub backend_api_url: String,
    pub grpc_url: Option<String>,  // NEW
}
```

The `/v1/desktop` API response must include a `grpc_url` field (server-side change required).

---

## Authentication

### JWT Interceptor (shared token cache)

The gRPC interceptor shares the same `Arc<RwLock<AccessTokenResponse>>` used by the REST `BackendClient`, so token refreshes are automatically picked up:

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use super::super::client::auth::AccessTokenResponse;

pub struct AuthInterceptor {
    tokens: Arc<RwLock<AccessTokenResponse>>,
}

impl tonic::service::Interceptor for AuthInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        let token = self.tokens.blocking_read();
        let bearer = format!("Bearer {}", token.access_token);
        req.metadata_mut().insert(
            "authorization",
            bearer.parse().unwrap(),
        );
        Ok(req)
    }
}
```

When a gRPC call returns `UNAUTHENTICATED`, the caller should trigger a token refresh via the existing `update_connect_cache()` flow in `services/connect/client/cache.rs`, then retry the call.

---

## Device Registration

On first launch (or after app data reset):

1. Call `DeviceService.RegisterDevice` with:
   - `device_name`: hostname or user-chosen name
   - `platform`: `DEVICE_PLATFORM_DESKTOP`
   - `app_version`: current app version
   - `os_version`: OS name + version
   - `device_pubkey`: optional device keypair public key (for v2 assertions)
   - `push_token`: empty (desktop doesn't use push)
   - `capabilities`: `["create_session", "cancel_session"]`

2. Store the returned `device_id` in the existing `ConnectCache` by extending the `Account` struct in `services/connect/client/cache.rs`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Account {
    pub email: String,
    pub tokens: AccessTokenResponse,
    #[serde(default)]
    pub device_id: Option<String>,         // NEW — persisted across launches
    #[serde(default)]
    pub last_seen_event_seq: Option<i64>,   // NEW — for stream replay
}
```

3. On subsequent launches, load `device_id` from the cache — do not re-register.

---

## Session Creation Flow

When the user initiates a transaction send:

### 1. Build the PSBT locally

The desktop app already constructs the PSBT using its wallet/descriptor logic.

### 2. Determine the target signer

Look up the Keychain device(s) that hold the required signing key(s). For v1, this is a single target:

```rust
let target = SignerTarget {
    device_id: keychain_device_id.clone(),
    key_fingerprint: key_fingerprint.clone(),
    key_id: key_id.clone(),
};
```

### 3. Call CreateSigningSession

```rust
let request = CreateSigningSessionRequest {
    request_id: Uuid::new_v4().to_string(),  // Client-generated for idempotency
    vault_id: vault_id.clone(),
    descriptor_id: descriptor_id.clone(),
    psbt: psbt_bytes,                         // Raw PSBT bytes
    targets: vec![target],
    note: user_note,
    ttl: Some(prost_types::Duration { seconds: 3600, nanos: 0 }),  // 1 hour
    require_user_presence: true,
};

let response = client.create_signing_session(request).await?;
let session = response.into_inner().session.unwrap();
```

### 4. Store the request_id locally

If the call fails due to a transient error, retry with the **same** `request_id`. The server returns the existing session instead of creating a duplicate.

### 5. Track session state in the UI

Display the session status to the user:

- `PENDING` — waiting for delivery to Keychain
- `DELIVERED` — Keychain received the request
- `VIEWED` — user opened the request on Keychain
- `APPROVED` — user approved, signing in progress
- `PARTIALLY_SIGNED` — signature submitted
- `COMPLETED` — signed PSBT available

---

## Realtime Stream

### Iced Subscription Pattern

The realtime stream follows the same `iced::stream::channel()` pattern used by the mavapay SSE stream in `services/mavapay/stream.rs`.

#### Message Type

```rust
#[derive(Debug, Clone)]
pub enum ConnectStreamMessage {
    Connected,
    SessionEvent(SessionEvent),
    Disconnected(String),
    Error(String),
}
```

#### Stream Function

```rust
pub fn connect_stream(
    data: &ConnectStreamConfig,
) -> impl iced::futures::Stream<Item = ConnectStreamMessage> + 'static {
    let config = data.clone();

    iced::stream::channel(64, |mut channel| async move {
        let mut backoff = Duration::from_secs(1);
        let max_backoff = Duration::from_secs(30);

        loop {
            // Create tonic channel + realtime client
            match create_channel(&config.grpc_url).await {
                Ok(grpc_channel) => {
                    let interceptor = AuthInterceptor::new(config.tokens.clone());
                    let mut client = RealtimeServiceClient::with_interceptor(
                        grpc_channel, interceptor,
                    );

                    // Set up bidirectional stream
                    let (tx, rx) = tokio::sync::mpsc::channel(64);
                    let hello = StreamEnvelope {
                        body: Some(stream_envelope::Body::ClientHello(ClientHello {
                            device_id: config.device_id.clone(),
                            platform: DevicePlatform::Desktop as i32,
                            user_agent: config.user_agent.clone(),
                            subscribe_vault_ids: config.vault_ids.clone(),
                            last_seen_event_seq: config.last_seen_seq,
                        })),
                    };
                    let _ = tx.send(hello).await;

                    let outbound = tokio_stream::wrappers::ReceiverStream::new(rx);
                    match client.connect(outbound).await {
                        Ok(response) => {
                            let _ = channel.send(ConnectStreamMessage::Connected).await;
                            backoff = Duration::from_secs(1); // Reset on success

                            let mut inbound = response.into_inner();
                            while let Ok(Some(envelope)) = inbound.message().await {
                                match envelope.body {
                                    Some(Body::SessionEvent(event)) => {
                                        // Acknowledge receipt
                                        let _ = tx.send(StreamEnvelope {
                                            body: Some(Body::ClientAck(ClientAck {
                                                event_seq: event.event_seq,
                                            })),
                                        }).await;
                                        let _ = channel.send(
                                            ConnectStreamMessage::SessionEvent(event)
                                        ).await;
                                    }
                                    Some(Body::Ping(ping)) => {
                                        let _ = tx.send(StreamEnvelope {
                                            body: Some(Body::Pong(Pong {
                                                ts_unix_ms: chrono::Utc::now()
                                                    .timestamp_millis(),
                                            })),
                                        }).await;
                                    }
                                    Some(Body::Error(err)) => {
                                        let _ = channel.send(
                                            ConnectStreamMessage::Error(
                                                format!("{}: {}", err.code, err.message)
                                            )
                                        ).await;
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(e) => {
                            let _ = channel.send(
                                ConnectStreamMessage::Error(e.to_string())
                            ).await;
                        }
                    }
                }
                Err(e) => {
                    let _ = channel.send(
                        ConnectStreamMessage::Error(e.to_string())
                    ).await;
                }
            }

            // Disconnected — reconnect with backoff
            let _ = channel.send(
                ConnectStreamMessage::Disconnected("Stream disconnected".into())
            ).await;
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(max_backoff);
        }
    })
}
```

#### Subscription Integration

Expose as an `iced::Subscription` from the relevant panel's `State::subscription()`:

```rust
fn subscription(&self) -> Subscription<Message> {
    if let Some(ref stream_config) = self.stream_config {
        iced::Subscription::run_with(stream_config, connect_stream)
            .map(|msg| Message::View(view::Message::ConnectStream(msg)))
    } else {
        Subscription::none()
    }
}
```

### Event Handling

Session events are routed through the Iced `Message` enum to the panel's `update()` method:

```rust
fn update(&mut self, ..., message: Message) -> Task<Message> {
    match message {
        Message::View(view::Message::ConnectStream(stream_msg)) => {
            match stream_msg {
                ConnectStreamMessage::SessionEvent(event) => {
                    // Update last_seen_event_seq in cache
                    // Update session status in local state
                    // On SESSION_COMPLETED: fetch signed PSBT
                }
                ConnectStreamMessage::Connected => { /* Update UI: stream active */ }
                ConnectStreamMessage::Disconnected(_) => { /* Update UI: reconnecting */ }
                ConnectStreamMessage::Error(e) => { /* Log error, update UI */ }
            }
            Task::none()
        }
        // ...
    }
}
```

---

## Cancel Flow

```rust
let response = client.cancel_signing_session(
    CancelSigningSessionRequest {
        session_id: session_id.clone(),
        reason: "User cancelled".to_string(),
    }
).await?;
```

Only allowed from: `PENDING`, `DELIVERED`, `VIEWED`, `APPROVED`. The server rejects cancellation from `PARTIALLY_SIGNED` or terminal states.

---

## Handling Completed Sessions

When `SESSION_COMPLETED` is received:

1. Call `GetSigningSession` to fetch the full session with signature data
2. Verify the `session_id` matches the expected session
3. Decode the signed PSBT from the session/signature data
4. Validate that the signed PSBT structurally matches the original unsigned PSBT
5. Merge signatures if needed (multi-sig scenarios)
6. Finalize and broadcast the transaction via the existing broadcast flow

---

## Polling Fallback

If the realtime stream is unavailable, use `iced::time::every()` as a degraded polling mechanism:

```rust
fn subscription(&self) -> Subscription<Message> {
    if self.stream_connected {
        // Use realtime stream (preferred)
        iced::Subscription::run_with(&self.stream_config, connect_stream)
            .map(...)
    } else if self.has_active_session {
        // Fall back to polling every 5 seconds
        iced::time::every(Duration::from_secs(5))
            .map(|_| Message::PollSessionStatus)
    } else {
        Subscription::none()
    }
}
```

---

## Error Handling

Map gRPC errors to the existing `DaemonError` type:

```rust
impl From<tonic::Status> for DaemonError {
    fn from(status: tonic::Status) -> Self {
        DaemonError::Http(Some(status.code() as u16), status.message().to_string())
    }
}
```

| gRPC Status        | Meaning                                          | Action                     |
| ------------------ | ------------------------------------------------ | -------------------------- |
| `UNAUTHENTICATED`  | JWT expired or revoked                           | Refresh via `update_connect_cache()`, retry |
| `INVALID_ARGUMENT` | Bad request (missing fields, invalid transition) | Show error to user         |
| `NOT_FOUND`        | Session doesn't exist                            | Remove from local tracking |
| `INTERNAL`         | Server error                                     | Retry with backoff         |
| `UNAVAILABLE`      | Server down / network issue                      | Reconnect with backoff     |

---

## Local State Management

Track these in the Iced panel state (integrated with the Elm architecture):

| Key                   | Type                            | Storage             | Purpose                                        |
| --------------------- | ------------------------------- | -------------------- | ---------------------------------------------- |
| `device_id`           | `Option<String>`                | `ConnectCache` (connect.json) | Registered device ID, persisted across launches |
| `last_seen_event_seq` | `Option<i64>`                   | `ConnectCache` (connect.json) | Last acknowledged event seq, for stream replay |
| `active_sessions`     | `HashMap<String, SessionState>` | Panel state (in memory) | In-flight sessions being tracked              |
| `request_id_map`      | `HashMap<String, String>`       | Panel state (in memory) | Maps `request_id` -> `session_id` for idempotency |

---

## Implementation Order

1. **Extend ServiceConfig** — add `grpc_url` field (requires server-side change)
2. **Extend build.rs** — append tonic codegen after existing `.env` logic
3. **Add dependencies** — tonic, prost, prost-types, tokio-stream, tonic-build
4. **Create `services/connect/grpc/mod.rs`** — proto include, `ConnectStreamMessage` enum
5. **Auth interceptor** — `interceptor.rs`, sharing `Arc<RwLock<AccessTokenResponse>>`
6. **Extend ConnectCache** — add `device_id`, `last_seen_event_seq` to `Account`
7. **Device registration** — `device.rs` with `GrpcDeviceClient`
8. **Session creation** — `session.rs` with `GrpcSessionClient`
9. **Realtime stream** — `stream.rs` using `iced::stream::channel()`
10. **State integration** — wire `ConnectStreamMessage` through Message enum
11. **Session completion** — fetch signed PSBT, merge, broadcast
12. **Cancel flow** — UI + `cancel_signing_session()`
13. **Polling fallback** — `iced::time::every()` based
14. **UI integration** — session progress indicators, notifications

---

## Testing

- Unit test proto type conversions
- Unit test reconnection backoff logic
- Unit test `AuthInterceptor` reads from shared `Arc<RwLock<AccessTokenResponse>>`
- Integration test: create session -> verify response fields
- Integration test: `iced::stream::channel` adapter produces correct `ConnectStreamMessage` variants
- Integration test: stream connect -> receive replayed events
- Integration test: cancel session -> verify terminal state
- Consider tonic mock server for gRPC integration tests
- Manual test: full end-to-end with Keychain simulator
