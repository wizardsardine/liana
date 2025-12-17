# Debug Mode

This document describes how to enable and use debug mode for local development and testing.

## Enabling Debug Mode

Debug mode is enabled by setting `BACKEND_URL` to `"debug"` in `business-installer/src/client.rs`:

```rust
pub const BACKEND_URL: &str = "debug";
```

When debug mode is active:
1. A local `DummyServer` is spawned automatically on a random port
2. Test data is pre-populated via `init_client_with_test_data()`
3. Authentication bypasses network calls
4. Useful for UI development without a running backend server

## Authentication

### OTP Code

In debug mode, the OTP code is **always** `123456`. Any other code will result in a login failure.

**Note:** The code must be entered exactly as `123456` (whitespace is trimmed, so `" 123456 "` also works).

### Test Emails

The following test emails are available with different user roles:

| Email | Role | Description |
|-------|------|-------------|
| `ws@example.com` | WSManager | Platform-side administrator with full access to all wallets |
| `owner@example.com` | Owner | Consumer-side wallet manager. Owner of Draft/Validated/Final wallets, Participant of Shared wallet |
| `user@example.com` | Participant | Limited access user. Participant for all wallets (Draft wallets are hidden) |
| `shared-owner@example.com` | Owner | Owner of the Shared wallet |

### Login Flow in Debug Mode

1. Enter any of the test emails above
2. Click "Send Code" - the `AuthCodeSent` notification is sent immediately (no network call)
3. Enter OTP code: `123456`
4. Login succeeds and you're authenticated

## Test Data

When debug mode is enabled, the following test data is automatically populated:

- **Organizations**: "Acme Corp" (with multiple wallets) and "Empty Org" (no wallets)
- **Wallets**: Multiple wallets in different statuses (Draft, Validated, Final, Shared)
- **Users**: Pre-configured users with different roles and permissions

See `business-installer/src/backend.rs` function `init_test_data()` for the complete test data structure.

## Implementation Details

### Client Behavior

When `BACKEND_URL == "debug"`:

- `Client::connect_ws()` spawns a local `DummyServer` on a random available port
- `Client::auth_request()` immediately sends `AuthCodeSent` notification without network calls
- `Client::auth_code()` accepts only `"123456"` and sets a dummy token

### Dummy Server

The dummy server:
- Runs on `127.0.0.1` with an auto-assigned port
- Handles WebSocket requests using pre-populated test data
- Shuts down automatically when the client connection closes

## Switching to Production Mode

To use a real backend server, change `BACKEND_URL` to the actual WebSocket URL:

```rust
pub const BACKEND_URL: &str = "wss://your-backend-url.com/ws";
```

In production mode:
- Authentication uses the real `AuthClient` from `liana-gui`
- OTP codes are sent via email and must be verified
- WebSocket connections are made to the specified backend URL
- Token caching is enabled for persistent sessions

