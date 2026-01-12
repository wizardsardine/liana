# Liana Business Server

A standalone WebSocket server for the Liana Business protocol. This server handles multiple concurrent client connections, manages organizations, wallets, and users, and broadcasts real-time notifications to connected clients.

## Features

- **Multi-client support**: Handle multiple concurrent WebSocket connections
- **Real-time notifications**: Broadcast state changes to all connected clients
- **In-memory storage**: Fast, lightweight storage for development and testing
- **Simple authentication**: Token-based authentication for easy integration
- **Protocol compliant**: Implements the full Liana Business WSS protocol

## Building

Build the server from the workspace root:

```bash
cargo build --release -p liana-business-server
```

The binary will be located at `target/release/liana-business-server`.

## Running

### Basic Usage

Start the server with default settings (listens on `0.0.0.0:8080`):

```bash
./target/release/liana-business-server
```

### Configuration Options

```bash
liana-business-server [OPTIONS]

Options:
  --host <HOST>          Bind address [default: 0.0.0.0]
  --port <PORT>          Port to listen on [default: 8080]
  --log-level <LEVEL>    Log verbosity (error, warn, info, debug, trace) [default: info]
  -h, --help             Print help
  -V, --version          Print version information
```

### Example

Run the server on a custom port with debug logging:

```bash
./target/release/liana-business-server --port 9000 --log-level debug
```

## Authentication

The server uses simple token-based authentication. On startup, the server prints available authentication tokens:

```
=== Authentication Tokens ===
Use these tokens to connect to the server:

  alice-token -> alice@example.com
  bob-token -> bob@example.com
  owner-token -> owner@example.com
  participant-token -> user@example.com
  shared-owner-token -> shared-owner@example.com
  ws-manager-token -> ws@example.com

============================
```

Clients must include the token in the `token` field of every request message.

### User Roles

Different tokens correspond to different user roles:

- **ws@example.com** (WSManager): Platform administrator with full access
- **owner@example.com** (Owner): Wallet owner with management rights
- **user@example.com** (Participant): Limited access participant
- **shared-owner@example.com** (Owner): Owner of the shared wallet

## Test Data

The server initializes with pre-populated test data including:

- 2 organizations: "Acme Corp" and "Empty Org"
- 4 wallets with different statuses (Draft, Validated, Finalized, Shared)
- Multiple users with different roles
- Sample policy templates with keys and spending paths

This provides a realistic environment for testing multi-user scenarios.

## Deployment

### VPS Deployment with systemd

1. **Build the binary**:
   ```bash
   cargo build --release -p liana-business-server
   ```

2. **Copy binary to VPS**:
   ```bash
   scp target/release/liana-business-server user@your-vps:/usr/local/bin/
   ```

3. **Create liana user** (on VPS):
   ```bash
   sudo useradd -r -s /bin/false liana
   ```

4. **Copy systemd service file**:
   ```bash
   scp contrib/liana-business-server.service user@your-vps:/tmp/
   sudo mv /tmp/liana-business-server.service /etc/systemd/system/
   ```

5. **Enable and start service**:
   ```bash
   sudo systemctl daemon-reload
   sudo systemctl enable liana-business-server
   sudo systemctl start liana-business-server
   ```

6. **Configure firewall**:
   ```bash
   sudo ufw allow 8080/tcp
   ```

7. **Check status**:
   ```bash
   sudo systemctl status liana-business-server
   sudo journalctl -u liana-business-server -f
   ```

### Updating the Server

To update the server on a running VPS:

```bash
# Build new version
cargo build --release -p liana-business-server

# Copy to VPS
scp target/release/liana-business-server user@your-vps:/usr/local/bin/

# Restart service
ssh user@your-vps 'sudo systemctl restart liana-business-server'
```

## Connecting Clients

Update the `BACKEND_URL` in your client application to point to the server:

```rust
// In business-installer/src/client.rs
pub const BACKEND_URL: &str = "ws://your-vps-ip:8080";
```

For local development, use `"debug"` to spawn a local server instance.

## Protocol

The server implements the Liana Business WebSocket protocol as specified in `liana-connect/WSS_BUSINESS.md`.

### Connection Flow

1. Client connects to `ws://host:port`
2. Client sends `connect` message with token and protocol version
3. Server validates token and responds with `connected`
4. Client sends requests, server responds
5. Server broadcasts notifications on state changes

### Message Types

**Requests:**
- `connect` - Initial connection handshake
- `ping` - Heartbeat
- `fetch_org` - Get organization details
- `fetch_wallet` - Get wallet details
- `fetch_user` - Get user details
- `create_wallet` - Create new wallet
- `edit_wallet` - Update wallet
- `edit_xpub` - Update key xpub
- `remove_wallet_from_org` - Remove wallet from organization
- `close` - Close connection

**Responses:**
- `connected` - Connection established
- `pong` - Heartbeat response
- `org` - Organization data (response or notification)
- `wallet` - Wallet data (response or notification)
- `user` - User data (response or notification)
- `error` - Error response

### Notifications

When a client modifies state (e.g., edits a wallet), the server broadcasts an unsolicited notification to all other connected clients. Notifications are identical to response messages but do not include a `request_id` field.

## Troubleshooting

### Server won't start

Check if port is already in use:
```bash
sudo netstat -tulpn | grep 8080
```

Try a different port:
```bash
./liana-business-server --port 9000
```

### Connection refused

Check firewall settings:
```bash
sudo ufw status
```

Verify server is listening:
```bash
sudo netstat -tulpn | grep liana
```

### Authentication failures

Ensure client is using a valid token from the startup output. Tokens are case-sensitive.

### Missing notifications

Check logs for broadcast errors:
```bash
sudo journalctl -u liana-business-server -f
```

Ensure client is properly handling messages without `request_id`.

## Development

### Running locally

For development, you can run the server directly with cargo:

```bash
cd liana-business-server
cargo run -- --log-level debug
```

### Testing with multiple clients

Start the server and connect multiple instances of `liana-business` GUI with different authentication tokens to test multi-client scenarios.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    Server (main.rs)                      │
│  ┌──────────────────────────────────────────────────┐   │
│  │            ServerState (Arc<Mutex<...>>)         │   │
│  │  - Organizations                                  │   │
│  │  - Wallets                                        │   │
│  │  - Users                                          │   │
│  └──────────────────────────────────────────────────┘   │
│                          │                               │
│  ┌──────────────────────┴──────────────────────────┐   │
│  │           AuthManager                             │   │
│  │  - Token validation                               │   │
│  └──────────────────────────────────────────────────┘   │
│                          │                               │
│  ┌──────────────────────┴──────────────────────────┐   │
│  │        ConnectionManager (Registry)              │   │
│  │  - Track active clients                          │   │
│  │  - Broadcast notifications                        │   │
│  └──────────────────────────────────────────────────┘   │
│         │              │              │                  │
│    ┌────┴────┐    ┌────┴────┐   ┌────┴────┐            │
│    │ Client1 │    │ Client2 │   │ ClientN │            │
│    │  Thread │    │  Thread │   │  Thread │            │
│    └─────────┘    └─────────┘   └─────────┘            │
└─────────────────────────────────────────────────────────┘
          │              │              │
     ┌────┴────┐    ┌────┴────┐   ┌────┴────┐
     │ Client1 │    │ Client2 │   │ ClientN │
     │   App   │    │   App   │   │   App   │
     └─────────┘    └─────────┘   └─────────┘
```

### Components

- **main.rs**: Entry point, CLI parsing, server initialization
- **server.rs**: Main server loop, connection acceptance, broadcast coordination
- **connection.rs**: Per-client connection handling, message processing
- **handler.rs**: Request handlers for each protocol message type
- **state.rs**: Shared state management, test data initialization
- **auth.rs**: Token-based authentication

### Threading Model

- **Main thread**: Accepts incoming connections
- **Per-client threads**: Handle WebSocket I/O for each client
- **Broadcast thread**: Coordinates notifications to all clients

All threads share state via `Arc<Mutex<...>>` for thread-safe access.

## Limitations

- **In-memory storage**: All data is lost on server restart
- **No persistence**: Changes are not saved to disk
- **Simple authentication**: No password hashing or JWT tokens
- **No TLS**: WebSocket connections are unencrypted (ws://, not wss://)
- **No access control**: Token validation only, no fine-grained permissions

These limitations make this server suitable for development and testing, but not for production use without additional hardening.

## Future Enhancements

- Database persistence (PostgreSQL, SQLite)
- Real authentication with OAuth2/OIDC
- TLS/SSL support for encrypted connections
- Fine-grained access control and audit logging
- Horizontal scaling with Redis pub/sub
- Metrics and monitoring endpoints
- API versioning and compatibility checks

## License

See the main project LICENSE file.

