# ROADMAP

## Priority

- [x] **2. Back**
  - [x] 2.1 Auth Client
  - [x] 2.2 Installer Trait Integration
  - [x] 2.3 WSS Protocol Extraction
- [ ] **3.0 Server Update Notifications** (NEXT MAIN PRIORITY)
- [ ] **3.1 WS Manager Flow**
- [ ] **1. Front**
  - [x] 1.1 Wallet Selection View
  - [x] 1.2 Edit Wallet Template Subflow (needed for completion of WS Manager flow)
  - [ ] 1.3 Add Key Information Subflow
  - [x] 1.4 Filter/Search Bar (WS Manager Only)
  - [x] 1.5 Better Keyboard Navigation in Login
  - [ ] 1.6 Load Wallet Subflow
  - [x] 1.7 Logout Feature
- [ ] **3.2 Owner Flow**
- [ ] **3.3 Participant Flow**
- [ ] **4. Local Storage**

## Concepts

### User Roles

Users can have 3 roles in a wallet:

1. **WS Manager** - Platform-side administrator
2. **Owner** - Consumer-side wallet manager (aka Wallet Owner)
3. **Participant** - Limited access user

Roles are defined in `liana-connect/src/models.rs` as `UserRole` enum.

### Wallet Statuses

Wallets progress through 3 statuses:

1. **Draft** (`Drafted`) - Template can be edited (paths/keys). Only WSManager and Owner
   can edit.
2. **Validated** - Owner has accepted the template. Paths/keys cannot be changed.
   Participants can now populate xpub information for keys linked to their account.
3. **Final** (`Finalized`) - All users have successfully populated their xpub information.
   The descriptor is now known and cannot be changed. Wallet can be loaded.

Statuses are defined in `liana-connect/src/models.rs` as `WalletStatus` enum.

### Subflows at Wallet Selection

When user arrives at wallet selection, three possible subflows based on status:

1. **Edit Wallet Template** - For Draft wallets, WSManager/Owner only
2. **Add Key Information** - For Validated wallets, role-filtered key access
3. **Load Wallet** - For Final wallets, triggers `exit_maybe()` -> `LoginLianaLite`

## 1. Front

### 1.1 Wallet Selection View ✓
- [x] Display wallet status badge (Draft/Validated/Final) for each wallet
- [x] Show user's role for each wallet
- [x] Route to appropriate subflow based on role + status:
  - Draft + (WSManager|Owner) -> Edit Template
  - Draft + Participant -> Access Denied
  - Validated -> Add Key Information
  - Final -> Load Wallet (exit to Liana Lite)
- [x] Differentiate UI styling per status
- [x] Sort wallets by status (Draft first, Finalized last)
- [x] Filter out Draft wallets from Participant view
- [x] "Hide finalized wallets" checkbox for WSManager
- [x] Show "WS Manager" badge in header for platform admins
- [x] Debug mode hints showing test emails/code

### 1.2 Edit Wallet Template Subflow
- [x] Restrict access to WSManager and WalletOwner roles
- [x] Restrict to Draft status wallets only
- [x] Finalize key management panel
  - [x] Complete UI implementation
  - [x] Ensure all key operations are functional
  - [x] Polish user experience
- [x] Finalize path management panel
  - [x] Complete UI implementation
    - [x] Clickable path cards in template visualization
    - [x] Edit Path modal with key selection (checkboxes)
    - [x] Threshold input with validation
    - [x] Timelock input with unit dropdown (blocks/hours/days/months)
    - [x] Create New Path mode
  - [x] Ensure all path operations are functional
    - [x] Role-based edit permissions (WSManager can edit, others read-only)
    - [x] Auto-save for WSManager on path changes
  - [x] Polish user experience
- [x] Add "Validate Template" action for Owner (Draft -> Validated transition)

### 1.3 Add Key Information Subflow
- [ ] Create xpub entry view (reuse `SelectKeySource` pattern from liana-gui)
- [ ] Integrate hardware wallet detection
  - [ ] Add `HardwareWallets` subscription (from `liana-gui/src/hw.rs`)
  - [ ] Support device detection: Ledger, Trezor, BitBox02, Coldcard, Jade, Specter
  - [ ] Fetch xpub from connected devices
- [ ] Support multiple key sources:
  - [ ] Hardware wallet (detected devices)
  - [ ] Manual xpub entry (paste)
  - [ ] Load xpub from file
- [ ] Filter keys by user email for Participant role
  - [ ] WSManager/Owner: can edit any key
  - [ ] Participant: can only edit keys where `key.email == user.email`
- [ ] Validate xpub format and network compatibility
- [ ] Save xpub to key via backend

### 1.4 Filter/Search Bar (WS Manager Only) ✓
- [x] Add search/filter bar to organization selection page
  - [x] Filter organizations by name
  - [x] Only visible for WS Manager users
- [x] Add filter to wallet selection page
  - [x] "Hide finalized wallets" checkbox (WSManager only)
  - [x] Filter wallets by name (text search)

### 1.5 Better Keyboard Navigation in Login ✓
- [x] Improve keyboard navigation for login flow
  - [x] Tab navigation between input fields (via form IDs)
  - [x] Enter key to submit forms (email and code)
  - [x] Focus management between steps
  - [x] Auto-focus email input on initial load
  - [x] Auto-focus code input when code view appears
  - [x] Focus email input when navigating back from code view

### 1.6 Load Wallet Subflow
- [x] Implement `exit_maybe()` returning `NextState::LoginLianaLite`
  - [x] Added `exit_to_liana_lite` flag in AppState
  - [x] Finalized wallet selection triggers exit
  - [x] Build `WalletId` and `AuthConfig` for handoff
- [x] Only available for Final status wallets
- [ ] Store wallet settings to disk before exit (see Section 4)
- [ ] Store auth cache to disk before exit (see Section 4)
- [ ] `exit_maybe()` must return `NextState::LoginLianaBusiness`

### 1.7 Logout Feature ✓
- [x] Add logout button/action in UI (accessible from main views)
- [x] Clear authentication token from memory
- [x] Clear auth cache from disk (`connect.json`)
- [x] Close WebSocket connection
- [x] Reset application state to initial login view
- [x] Handle logout in debug mode (clear dummy token)

## 2. Back ✓

### 2.1 Auth Client
- [x] Implement auth client
  - [x] Export required auth types from liana-gui (AuthClient, AuthError,
  AccessTokenResponse, cache types, http traits, NetworkDirectory,
get_service_config)
  - [x] Add liana-gui dependency to liana-business Cargo.toml
  - [x] Extend Client struct with auth_client, network, network_dir, email fields
  - [x] Implement auth_request() using AuthClient::sign_in_otp() with async-to-sync
  bridge (spawn thread + block_on)
  - [x] Implement auth_code() using AuthClient::verify_otp() with token caching via
  update_connect_cache()
  - [x] Implement token caching in connect_ws() - check cached token, refresh if
  expired, use cached token for connection

### 2.2 Installer Trait Integration
- [x] Wrap complete app under the Installer trait of liana-gui
  - [x] Implement Installer trait for the application (BusinessInstaller in
business-installer crate)
  - [x] Support standalone mode (liana-business wraps BusinessInstaller)
  - [x] Support integration into liana-gui (via Installer trait interface)

### 2.3 WSS Protocol Extraction
- [x] Move shared WSS protocol types to liana-connect crate
  - [x] Create liana-connect/src/protocol.rs with JSON payload types
  - [x] Create liana-connect/src/models.rs with domain types (Wallet, Org, User,
Key, etc.)
  - [x] Move WssError, WssConversionError, ProtocolRequest, ProtocolResponse
  - [x] Move TryFrom/From conversions between JSON and domain types
  - [x] Update business-installer to import from liana-connect

### 2.4 Auth improvements
- [ ] Automatically refresh token
- [ ] Async instead threading?

## 3. Flows

### 3.0 Server Update Notifications

#### Backend (Dummy Server)
- [ ] Implement notification sending when server state is updated
  - [ ] Send `WalletUpdated` notification when wallet is modified
  - [ ] Send `KeyUpdated` notification when key is modified
  - [ ] Send `KeyDeleted` notification when key is deleted
  - [ ] Send `PathUpdated` notification when path is modified
  - [ ] Send `PathDeleted` notification when path is deleted
- [ ] Define notification payload structure in `liana-connect` protocol
- [ ] Update dummy server to broadcast notifications to all connected clients

#### Frontend (Notification Handling)
- [ ] Add notification handlers in backend update logic
- [ ] Most notifications require no action (state already updated via existing subscription)
- [ ] Implement edge case reconciliation for open modals

#### Edge Case: Key Modal Open During Server Update
- [ ] Detect when the key being edited was modified or deleted on server
  - [ ] Track the key ID being edited in modal state
  - [ ] On `KeyDeleted` notification: check if it matches the open modal
  - [ ] On `KeyUpdated` notification: check if it matches the open modal
- [ ] If key deleted: close modal, show info message to user
- [ ] If key modified: prompt user to reload or discard local changes
  - [ ] Option 1: "Reload" - fetch latest key data from server, update modal
  - [ ] Option 2: "Keep my changes" - continue editing with local changes
  - [ ] Show warning that server version differs

#### Edge Case: Path Modal Open During Server Update
- [ ] Detect when the path being edited was modified or deleted
  - [ ] Track the path being edited in modal state (primary vs recovery index)
  - [ ] On `PathDeleted` notification: check if it matches the open modal
  - [ ] On `PathUpdated` notification: check if it matches the open modal
- [ ] Detect when keys used in the path were removed
  - [ ] On `KeyDeleted` notification: check if key is in currently edited path
  - [ ] Track which keys are selected in the path modal
- [ ] If path deleted: close modal, show info message to user
- [ ] If path modified: prompt user to reload or discard local changes
  - [ ] Option 1: "Reload" - fetch latest path data, update modal
  - [ ] Option 2: "Keep my changes" - continue editing with local changes
  - [ ] Show warning that server version differs
- [ ] If key(s) removed from path: update selected keys in modal, show warning
  - [ ] Automatically uncheck deleted keys from selection
  - [ ] Display warning: "Key X was removed by another user"
  - [ ] Validate threshold is still valid after key removal

### 3.1 WS Manager Flow

WS Manager is the platform-side administrator with full access.

**Permissions by wallet status:**

| Status    | Can Edit Template | Can Add Xpubs | Can Load Wallet |
|-----------|-------------------|---------------|-----------------|
| Draft     | ✓ (any key/path)  | ✗             | ✗               |
| Validated | ✗                 | ✓ (any key)   | ✗               |
| Final     | ✗                 | ✗             | ✓               |

**Implementation tasks:**
- [x] Full template editing for Draft wallets
  - [ ] Create/edit/delete keys
  - [x] Create/edit/delete spending paths
  - [x] Set thresholds and timelocks
  - [x] Auto-save changes to server (status = Drafted)
- [ ] Full key info access for Validated wallets
  - [ ] View all keys regardless of email
  - [ ] Add xpub to any key
- [ ] Wallet loading for Final wallets
- [ ] Testing and validation

### 3.2 Owner Flow

Owner is the consumer-side wallet manager.

**Permissions by wallet status:**

| Status    | Can Edit Template | Can Validate | Can Add Xpubs | Can Load Wallet |
|-----------|-------------------|--------------|---------------|-----------------|
| Draft     | ✓ (any key/path)  | ✓            | ✗             | ✗               |
| Validated | ✗                 | ✗            | ✓ (any key)   | ✗               |
| Final     | ✗                 | ✗            | ✗             | ✓               |

**Implementation tasks:**
- [x] Template editing for Draft wallets (same as WS Manager)
  - [x] Role-based UI restrictions (paths read-only for non-WSManager)
- [x] Template validation action (Draft -> Validated transition)
  - [x] Add "Validate Template" button (Owner only)
  - [x] Backend API call to change status (status = Validated)
  - [ ] Confirm dialog before transition
- [ ] Key info entry for Validated wallets (any key)
- [ ] Wallet loading for Final wallets
- [ ] Testing and have a complete functional flow

### 3.3 Participant Flow

Participant has limited access - can only add xpub for their own keys.

**Permissions by wallet status:**

| Status    | Can Edit Template | Can Add Xpubs          | Can Load Wallet |
|-----------|-------------------|------------------------|-----------------|
| Draft     | ✗ (not visible)   | ✗                      | ✗               |
| Validated | ✗                 | ✓ (own keys only)      | ✗               |
| Final     | ✗                 | ✗                      | ✓               |

**Implementation tasks:**
- [x] Connect and authenticate
- [x] Draft wallets filtered from wallet selection view
- [x] Access denied modal if participant attempts Draft wallet access
- [ ] Add/edit xpub for own keys only in Validated status
  - [ ] Filter key list by `key.email == current_user.email`
  - [ ] Hide keys belonging to other users
- [ ] Wallet loading for Final wallets
- [ ] Testing and have a complete functional flow

## 4. Local Storage

Store data in `network_dir` matching liana-gui patterns. Reference:
`liana-gui/src/app/settings/mod.rs` and `liana-gui/src/services/connect/client/cache.rs`

### 4.1 Wallet Settings (`settings.json`)
- [ ] Implement `WalletSettings` struct matching liana-gui:
  ```rust
  pub struct WalletSettings {
      pub name: String,
      pub alias: Option<String>,
      pub descriptor_checksum: String,
      pub pinned_at: Option<i64>,
      pub keys: Vec<KeySetting>,           // Empty for remote backend
      pub hardware_wallets: Vec<HardwareWalletConfig>,
      pub remote_backend_auth: Option<AuthConfig>,
      pub start_internal_bitcoind: Option<bool>,
      pub fiat_price: Option<PriceSetting>,
  }
  ```
- [ ] Implement `AuthConfig` struct:
  ```rust
  pub struct AuthConfig {
      pub email: String,
      pub wallet_id: String,
  }
  ```
- [ ] Write `settings.json` on wallet load (before `exit_maybe`)
- [ ] Use `update_settings_file()` pattern with file locking

### 4.2 Auth Cache (`connect.json`)
- [ ] Implement `ConnectCache` for token storage:
  ```rust
  pub struct ConnectCache {
      pub accounts: Vec<Account>,
  }
  
  pub struct Account {
      pub email: String,
      pub tokens: AccessTokenResponse,
  }
  ```
- [ ] Write `connect.json` for token persistence
- [ ] Use `update_connect_cache()` pattern from liana-gui
- [ ] Token refresh before expiry

### 4.3 Hardware Wallet Config
- [ ] Store `HardwareWalletConfig` for registered devices:
  ```rust
  pub struct HardwareWalletConfig {
      pub kind: String,
      pub fingerprint: Fingerprint,
      pub token: String,  // Registration token (hex)
  }
  ```

## 5. Standalone Server Binary

Standalone WebSocket server for deployment on VPS. Supports multiple concurrent clients,
real-time notifications, and in-memory storage.

### 5.1 Server Implementation
- [x] Create `liana-business-server` crate
  - [x] Cargo.toml with dependencies (tungstenite, crossbeam, clap, etc.)
  - [x] Main binary entry point with CLI parsing
  - [x] Module structure (server, connection, handler, state, auth)
- [x] Extract and refactor server logic from `business-installer`
  - [x] Extract `DummyServer` logic into standalone server
  - [x] Extract request handlers from backend.rs
  - [x] Extract test data initialization
- [x] Implement multi-client connection management
  - [x] Connection registry for tracking active clients
  - [x] Per-client handler threads
  - [x] Non-blocking WebSocket I/O
  - [x] Connection lifecycle management (accept, authenticate, handle, cleanup)
- [x] Implement simple token-based authentication
  - [x] Pre-configured tokens for test users
  - [x] Token validation on connection and per-request
  - [x] Token-to-email mapping
- [x] Implement shared state management
  - [x] `ServerState` with `Arc<Mutex<...>>` for thread safety
  - [x] In-memory storage for orgs, wallets, users
  - [x] State initialization with test data
- [x] Implement notification broadcasting
  - [x] Broadcast channel for state change notifications
  - [x] Send unsolicited notifications to all clients (except originator)
  - [x] Support org, wallet, and user notifications
- [x] Add CLI argument parsing
  - [x] `--host` for bind address
  - [x] `--port` for port
  - [x] `--log-level` for logging verbosity
  - [x] Print tokens on startup

### 5.2 Deployment Support
- [x] Create systemd service file
  - [x] Basic service configuration
  - [x] Security hardening options
  - [x] Auto-restart on failure
- [x] Write comprehensive README.md
  - [x] Building and running instructions
  - [x] Configuration options
  - [x] Authentication token documentation
  - [x] VPS deployment guide (systemd)
  - [x] Troubleshooting section
  - [x] Architecture diagram

### 5.3 Testing
- [x] Add integration tests
  - [x] Server connection test
  - [x] Invalid token test
  - [x] Multi-client broadcast test
  - [x] Ping/pong heartbeat test

### 5.4 Client Integration
- [x] Update client to support remote server
  - [x] Document BACKEND_URL configuration options
  - [x] Keep debug mode for local development
  - [x] Add examples for different deployment scenarios

## 6. Integration Testing

Comprehensive integration tests for the WebSocket API using the Client to verify
all edge cases, access control rules, and multi-client synchronization.

### 6.1 Test Infrastructure Setup

- [ ] Create test module `business-installer/tests/integration_tests.rs`
- [ ] Implement test harness with helper functions:
  - [ ] `setup_server()` - Start liana-business-server with test data
  - [ ] `setup_client(token: &str)` - Create and connect a Client with specific user token
  - [ ] `create_test_wallets()` - Generate wallets in all statuses (Created, Drafted, Validated, Finalized)
  - [ ] `create_test_users()` - Generate users for each role (WSManager, Owner, Participant)
  - [ ] `wait_for_notification(receiver, expected, timeout)` - Helper to wait for specific notifications
  - [ ] `assert_error_response(result, expected_code)` - Verify error responses
- [ ] Define test constants:
  - [ ] Tokens for each role: `WS_MANAGER_TOKEN`, `OWNER_TOKEN`, `PARTICIPANT_TOKEN`
  - [ ] User emails: `ws@example.com`, `owner@example.com`, `user@example.com`
  - [ ] Pre-defined org, wallet, and user UUIDs for predictable testing

### 6.2 Connection and Authentication Tests

- [ ] Test successful WebSocket connection with valid token
  - [ ] Verify `Connected` notification received
  - [ ] Verify `is_connected()` returns true
- [ ] Test connection with invalid token
  - [ ] Verify connection fails or error response received
- [ ] Test connection without token
  - [ ] Verify notification channel created but no WSS connection attempted
- [ ] Test ping/pong heartbeat mechanism
  - [ ] Send ping, verify pong response within timeout
  - [ ] Verify connection stays alive after multiple ping/pong cycles
- [ ] Test graceful disconnection
  - [ ] Call `close()`, verify `Disconnected` notification
  - [ ] Verify `is_connected()` returns false after close
- [ ] Test reconnection scenario
  - [ ] Connect, disconnect, reconnect with same client
  - [ ] Verify state is maintained correctly

### 6.3 Basic CRUD Operations Tests

**Fetch Organization:**
- [ ] Test `fetch_org()` with valid org ID
  - [ ] Verify `Org` notification received
  - [ ] Verify org data cached correctly via `get_org()`
  - [ ] Verify associated wallets are auto-fetched
  - [ ] Verify associated users are auto-fetched
- [ ] Test `fetch_org()` with non-existent org ID
  - [ ] Verify error response with code "NOT_FOUND"

**Fetch Wallet:**
- [ ] Test `fetch_wallet()` with valid wallet ID
  - [ ] Verify `Wallet` notification received
  - [ ] Verify wallet data cached via `get_wallet()`
  - [ ] Verify owner user is auto-fetched if not cached
- [ ] Test `fetch_wallet()` with non-existent wallet ID
  - [ ] Verify error response with code "NOT_FOUND"
- [ ] Test fetching wallets in all statuses (Created, Drafted, Validated, Finalized)

**Fetch User:**
- [ ] Test `fetch_user()` with valid user ID
  - [ ] Verify `User` notification received
  - [ ] Verify user data cached via `get_user()`
- [ ] Test `fetch_user()` with non-existent user ID
  - [ ] Verify error response with code "NOT_FOUND"

**Create Wallet:**
- [ ] Test `create_wallet()` with valid parameters
  - [ ] Verify `Wallet` notification received
  - [ ] Verify new wallet has status `Created`
  - [ ] Verify wallet is added to org's wallet list
- [ ] Test `create_wallet()` with non-existent org
  - [ ] Verify wallet created but org list not updated (or error)
- [ ] Test `create_wallet()` with non-existent owner
  - [ ] Verify wallet created with placeholder owner data

### 6.4 Role-Based Access Control Tests

**WSManager Role - Draft Wallet Access:**
- [ ] Test WSManager can edit Draft wallet template
  - [ ] Add/remove keys via `edit_wallet()`
  - [ ] Add/modify/remove spending paths
  - [ ] Change thresholds and timelocks
  - [ ] Verify all changes persist (status remains `Drafted`)
- [ ] Test WSManager cannot validate template (reserved for Owner)
  - [ ] Attempt to change status from `Drafted` to `Validated`
  - [ ] Verify server rejects or no status change occurs

**WSManager Role - Validated Wallet Access:**
- [ ] Test WSManager cannot edit template structure in Validated wallet
  - [ ] Attempt to add/remove keys
  - [ ] Attempt to modify paths
  - [ ] Verify changes rejected or ignored
- [ ] Test WSManager can add xpub to any key via `edit_xpub()`
  - [ ] Add xpub to keys belonging to different users
  - [ ] Verify xpub updates succeed for all keys

**WSManager Role - Finalized Wallet Access:**
- [ ] Test WSManager cannot edit Finalized wallet
  - [ ] Attempt template changes
  - [ ] Attempt xpub changes
  - [ ] Verify all changes rejected

**Owner Role - Draft Wallet Access:**
- [ ] Test Owner can view Draft wallet template
  - [ ] Fetch wallet, verify template data accessible
- [ ] Test Owner can validate template (Draft → Validated)
  - [ ] Change wallet status to `Validated` via `edit_wallet()`
  - [ ] Verify status change persists
  - [ ] Verify template becomes immutable after validation
- [ ] Test Owner cannot edit template structure (paths/keys)
  - [ ] Attempt to modify paths or keys
  - [ ] Verify changes rejected (Owner has view-only access to template)

**Owner Role - Validated Wallet Access:**
- [ ] Test Owner can add xpub to any key
  - [ ] Add xpub via `edit_xpub()` for different keys
  - [ ] Verify xpub updates succeed
- [ ] Test Owner cannot edit template structure
  - [ ] Attempt to modify validated template
  - [ ] Verify changes rejected

**Owner Role - Finalized Wallet Access:**
- [ ] Test Owner can load Finalized wallet
  - [ ] Fetch wallet with status `Finalized`
  - [ ] Verify all data accessible read-only
- [ ] Test Owner cannot edit Finalized wallet
  - [ ] Attempt any modifications
  - [ ] Verify all rejected

**Participant Role - Draft Wallet Access:**
- [ ] Test Participant cannot access Draft wallets
  - [ ] Attempt `fetch_wallet()` for Draft wallet
  - [ ] Verify access denied or wallet filtered from view
  - [ ] Verify Draft wallets not included in org's wallet list for Participant

**Participant Role - Validated Wallet Access:**
- [ ] Test Participant can only edit own keys' xpubs
  - [ ] Add xpub to key where `key.email == user.email`
  - [ ] Verify xpub update succeeds
- [ ] Test Participant cannot edit other users' keys
  - [ ] Attempt to add xpub to key where `key.email != user.email`
  - [ ] Verify change rejected with appropriate error
- [ ] Test Participant cannot edit template structure
  - [ ] Attempt to modify paths or add/remove keys
  - [ ] Verify all changes rejected
- [ ] Test Participant can view but not edit keys without matching email
  - [ ] Fetch wallet, verify all keys visible
  - [ ] Verify keys with non-matching email are read-only

**Participant Role - Finalized Wallet Access:**
- [ ] Test Participant can load Finalized wallet
  - [ ] Fetch wallet, verify read-only access
- [ ] Test Participant cannot edit Finalized wallet

### 6.5 Edge Cases and Validation Tests

**Template Validation:**
- [ ] Test adding key with duplicate ID
  - [ ] Verify rejected or overwrites existing key
- [ ] Test creating path with invalid threshold (n > m)
  - [ ] Verify validation error or rejection
- [ ] Test creating path with zero threshold
  - [ ] Verify validation error
- [ ] Test creating path with non-existent key IDs
  - [ ] Verify validation error or graceful handling
- [ ] Test creating secondary path with insufficient timelock (< 144 blocks)
  - [ ] Verify validation error

**Status Transitions:**
- [ ] Test invalid status transitions
  - [ ] Attempt Created → Validated (skip Draft)
  - [ ] Attempt Validated → Drafted (backward transition)
  - [ ] Attempt Finalized → any other status
  - [ ] Verify all rejected
- [ ] Test valid status transitions
  - [ ] Created → Drafted
  - [ ] Drafted → Validated (Owner only)
  - [ ] Validated → Finalized (when all xpubs populated)

**XPub Operations:**
- [ ] Test adding valid xpub
  - [ ] Add properly formatted xpub string
  - [ ] Verify parsed and stored correctly
- [ ] Test adding invalid xpub format
  - [ ] Attempt to add malformed xpub
  - [ ] Verify error response or rejected
- [ ] Test removing xpub (set to None)
  - [ ] Remove previously set xpub
  - [ ] Verify xpub cleared
- [ ] Test overwriting existing xpub
  - [ ] Replace xpub with different value
  - [ ] Verify new value persists

**Org and Wallet Management:**
- [ ] Test `remove_wallet_from_org()`
  - [ ] Remove wallet from org
  - [ ] Verify wallet removed from org's wallet list
  - [ ] Verify wallet still exists independently
- [ ] Test removing non-existent wallet from org
  - [ ] Verify graceful handling (no error or "NOT_FOUND")

### 6.6 Multi-Client Synchronization Tests

**Basic Broadcast Verification:**
- [ ] Test wallet update broadcasts to other clients
  - [ ] Connect two clients (clientA, clientB) with different users
  - [ ] ClientA edits wallet via `edit_wallet()`
  - [ ] Verify clientB receives `Wallet` notification
  - [ ] Verify clientB's cache updated with new wallet data
- [ ] Test org update broadcasts to other clients
  - [ ] ClientA removes wallet from org
  - [ ] Verify clientB receives `Org` notification
  - [ ] Verify clientB's org cache updated
- [ ] Test user update broadcasts (if implemented)
  - [ ] Update user data from one client
  - [ ] Verify other clients notified

**Concurrent Edit Scenarios:**
- [ ] Test concurrent edits from WSManager and Owner
  - [ ] Both clients edit same wallet simultaneously
  - [ ] Verify last-write-wins or conflict resolution
  - [ ] Verify both clients eventually converge to same state
- [ ] Test concurrent xpub edits on different keys
  - [ ] Client1 edits key A, Client2 edits key B
  - [ ] Verify both changes succeed
  - [ ] Verify both clients receive both updates
- [ ] Test concurrent xpub edits on same key
  - [ ] Client1 and Client2 edit same key simultaneously
  - [ ] Verify conflict resolution (last-write-wins)
  - [ ] Verify both clients converge to final state

**Access Control with Multiple Clients:**
- [ ] Test Participant receives updates but cannot edit
  - [ ] Connect WSManager and Participant clients
  - [ ] WSManager edits Validated wallet
  - [ ] Verify Participant receives update notification
  - [ ] Verify Participant cannot make own edits
- [ ] Test Owner validation broadcasts to all roles
  - [ ] Owner changes Draft → Validated
  - [ ] Verify WSManager and Participant clients notified
  - [ ] Verify status change reflected in all clients

**Notification Filtering:**
- [ ] Test client doesn't receive own updates as unsolicited notifications
  - [ ] Client edits wallet
  - [ ] Verify client receives response to its request
  - [ ] Verify client does NOT receive unsolicited broadcast of its own change
- [ ] Test clients only receive relevant notifications
  - [ ] Edit wallet in org1
  - [ ] Verify only clients interested in org1 notified (if filtering implemented)

### 6.7 Stress and Reliability Tests

**Connection Stability:**
- [ ] Test sustained connection with periodic activity
  - [ ] Keep connection alive for extended period
  - [ ] Send requests every few seconds
  - [ ] Verify no disconnections or errors
- [ ] Test rapid sequential requests
  - [ ] Send 100+ requests in quick succession
  - [ ] Verify all responses received correctly
  - [ ] Verify no dropped requests

**Large Data Handling:**
- [ ] Test wallet with many keys (e.g., 20+ keys)
  - [ ] Create template with maximum expected keys
  - [ ] Verify serialization/deserialization works
  - [ ] Verify performance acceptable
- [ ] Test wallet with many secondary paths (e.g., 10+ paths)
  - [ ] Create complex template
  - [ ] Verify all paths handled correctly
- [ ] Test org with many wallets (e.g., 50+ wallets)
  - [ ] Fetch org with large wallet list
  - [ ] Verify all wallets auto-fetched correctly

**Error Recovery:**
- [ ] Test recovery from server restart
  - [ ] Connect client, make some requests
  - [ ] Restart server (simulate crash)
  - [ ] Verify client detects disconnection
  - [ ] Verify client can reconnect after server restart
- [ ] Test handling of malformed responses
  - [ ] Simulate corrupted JSON from server
  - [ ] Verify client handles gracefully (error logged, no crash)

### 6.8 Test Organization and Execution

**Test Structure:**
- [ ] Organize tests into modules:
  - [ ] `connection_tests` - Connection, auth, ping/pong
  - [ ] `crud_tests` - Basic fetch/create/edit operations
  - [ ] `access_control_tests` - Role-based permission tests
  - [ ] `multi_client_tests` - Broadcast and synchronization
  - [ ] `edge_case_tests` - Validation, error handling
  - [ ] `stress_tests` - Performance and reliability
- [ ] Use descriptive test names: `test_wsmanager_can_edit_draft_wallet_template`
- [ ] Add test documentation comments explaining what is being tested and why

**Test Execution:**
- [ ] Ensure tests can run in parallel (use unique ports per test)
- [ ] Add timeout for all tests (e.g., 30 seconds max)
- [ ] Implement proper cleanup (close connections, shutdown servers)
- [ ] Add CI integration (run tests on every commit)

**Test Documentation:**
- [ ] Create `business-installer/tests/README.md` with:
  - [ ] Overview of test coverage
  - [ ] How to run tests
  - [ ] How to add new tests
  - [ ] Test data reference (users, tokens, wallets)
  - [ ] Known limitations or test gaps

## Changelog

### 2025-12-18
- 5. Standalone Server Binary: Implemented complete standalone server
  - Created `liana-business-server` crate with full WebSocket server implementation
  - Extracted and refactored server logic from business-installer
  - Implemented multi-client connection management with per-client handler threads
  - Implemented simple token-based authentication with pre-configured test tokens
  - Implemented shared state management with Arc<Mutex<...>> for thread safety
  - Implemented notification broadcasting to all connected clients (except originator)
  - Added CLI argument parsing with clap (--host, --port, --log-level)
  - Created systemd service file for VPS deployment
  - Wrote comprehensive README.md with deployment instructions
  - Added integration tests for multi-client scenarios
  - Updated client BACKEND_URL documentation for remote server configuration
  - Server supports multiple concurrent clients with real-time state synchronization
  - In-memory storage with test data (2 orgs, 4 wallets, multiple users)
  - Ready for VPS deployment with systemd

### 2025-12-18
- Roadmap: Added 3.0 Server Update Notifications as next main priority
  - Backend tasks: Send notifications when server state updates (WalletUpdated, KeyUpdated, KeyDeleted, PathUpdated, PathDeleted)
  - Frontend tasks: Handle notifications and reconcile modal state conflicts
  - Edge case handling for Key Modal: detect conflicts when key being edited is modified/deleted on server
  - Edge case handling for Path Modal: detect conflicts when path or its keys are modified/deleted on server
  - Reconciliation options: reload from server or keep local changes
  - Testing plan for multi-client scenarios

### 2025-12-18
- Auth improvements: Connect WebSocket immediately after login success
  - WebSocket connection now established automatically after successful authentication
  - Updates global receiver for subscription handling
  - Ensures real-time updates are available as soon as user is authenticated

### 2025-12-18
- 1.2 Edit Wallet Template Subflow: Refactored Manage Keys view
  - Refactored keys view to use `layout_with_scrollable_list` helper (matching template_builder pattern)
  - Keys displayed as clickable cards (styled like path cards, no SVG r-shapes)
  - Clicking a key card opens edit modal
  - Added "+ Add a key" card at bottom for creating new keys
  - Removed individual pencil/trash buttons from cards (edit/delete now in modal)
  - Updated key modal to support both new and edit modes
  - Added Delete button in modal (only shown when editing existing keys)
  - Modal title changes based on mode ("New Key" vs "Edit Key")
  - Updated state handlers to open modal for both add and edit operations
  - Added navigation handling for Keys view (NavigateBack returns to WalletEdit)

### 2025-12-18
- 1.2 Edit Wallet Template Subflow: Implemented role-based access control and validation
  - Added `current_user_role` tracking in AppState to store user's role per wallet
  - Implemented role-based UI restrictions:
    - WSManager: Can edit paths, auto-save changes to server (status = Drafted)
    - Owner: Can validate templates, paths are read-only (view-only mode)
    - Path cards are clickable/editable only for WSManager users
    - Delete buttons and "Add recovery path" only visible for WSManager
  - Implemented "Validate Template" action for Owner role:
    - Validate button only visible to Owner users
    - Pushes template to server with status = Validated on validation
    - Validation restricted to Owner role only
  - Auto-save functionality for WSManager:
    - Automatically saves path changes (add/edit/delete) to server
    - Changes saved with status = Drafted
  - Role-based button visibility in template builder:
    - WSManager: Shows "Manage Keys" button only
    - Owner: Shows both "Manage Keys" and "Validate Template" buttons
  - Simplified role detection: Uses `current_user_role` from AppState instead of complex email-based checks

### 2025-12-18
- 1.2 Edit Wallet Template Subflow: Reworked template visualization and modals
  - Reworked Edit Path modal with comprehensive key management:
    - Full key selection UI with checkboxes for all available keys
    - Toggle keys in/out of the path
    - Threshold input (only enabled when key count > 1)
    - Threshold validation with proper error messages
    - Timelock unit dropdown: blocks, hours, days, months
    - Timelock validation (minimum 144 blocks for recovery paths)
    - Supports both "Edit" and "Create New Path" modes
  - Added TemplateToggleKeyInPath message for key selection in modal
  - Updated EditPathModalState with selected_key_ids and timelock_unit
  - Redesigned template_builder view layout:
    - Clean header with Previous/Logout button, title, and user email/role
    - Scrollable template visualization in center area
    - Action buttons (Manage Keys, New Path, Validate) fixed at bottom
  - Rewrote template_visualization with clickable card-based UI:
    - Replaced monolithic SVG with individual r-shape icons per path
    - Each path shown as row: colored r-shape + clickable card
    - Primary path: "Spendable anytime" with green r-shape
    - Secondary paths: human-readable timelocks ("After X hours/days/months")
    - Threshold display: "All of X, Y" or "N of X, Y, Z"
    - Color gradient from green to blue for paths
    - Click any path card to open Edit Path modal
  - Removed separate paths_view (path management now integrated into template_builder)
  - Fixed Previous/Logout button priority (Previous takes precedence when available)

### 2025-12-18
- UI improvement: Made only the org/wallet list scrollable in select views, keeping
  title, search bar, and filters fixed at the top. Added `layout_with_scrollable_list`
  helper function in views/mod.rs.
- UI improvement: Refactored template_builder to use `layout_with_scrollable_list` with
  optional footer support. Footer contains role-based action buttons (Manage Keys,
  Validate Template). Standardized path card widths and alignment constants.

### 2025-12-17
- 1.7 Logout Feature: Implemented
  - Added logout button that replaces "Previous" button when user is authenticated
  - Logout button uses back arrow icon for consistency with navigation
  - Clears authentication token from memory
  - Removes auth cache from disk (`connect.json`) using `filter_connect_cache`
  - Closes WebSocket connection
  - Resets application state (login state, form values, selected org/wallet)
  - Navigates to login view and focuses email input
  - Handles both production and debug modes

### 2025-12-17
- 1.4 Filter/Search Bar (WS Manager Only): Implemented
  - Added search bar to organization selection page (WS Manager only)
  - Case-insensitive filtering by organization name
  - Added search bar to wallet selection page (all users)
  - Case-insensitive filtering by wallet alias/name
  - Search inputs constrained to 500px width to match card width
  - Shows "No organizations/wallets found matching your search" when filter returns no results
  - Added `OrgSelectState` and `WalletSelectState.search_filter` for state management
  - Added `OrgSelectUpdateSearchFilter` and `WalletSelectUpdateSearchFilter` messages
- 1.5 Better Keyboard Navigation in Login: Implemented
  - Added `on_submit()`, `on_submit_maybe()`, and `id()` methods to `Form` component in liana-ui
  - Email form: Enter key submits when valid, ID `"login_email"`
  - Code form: Enter key submits when 6 digits entered, ID `"login_code"`
  - Auto-focus email input on initial app load (via `BusinessInstaller::new_internal`)
  - Auto-focus code input when transitioning to code entry view
  - Focus email input when navigating back from code to email view
  - Tab navigation works automatically via Iced focusable widgets

### 2025-12-16
- 1.1 Wallet Selection View: Implemented and enhanced
  - Added `derive_user_role()` to determine user's role per wallet
  - Added `status_badge()` component with colored pills (Draft/Validated)
  - Sort wallets by status (Draft first, Finalized last)
  - "Hide finalized wallets" checkbox for WSManager users
  - Participants cannot see Draft wallets (filtered in view)
  - "WS Manager" role badge in header for platform admins
  - Debug hints showing test emails/code in login views
  - Added access control in `on_org_wallet_selected()` - denies Draft access to Participants
- 1.6 Load Wallet Subflow: Implemented `exit_maybe()` -> `NextState::LoginLianaLite`
  - Added `exit_to_liana_lite` flag to AppState
  - Selecting Finalized wallet triggers exit to Liana Lite
  - Builds WalletId and AuthConfig for handoff
- View restructuring: Renamed `home` view to `template_builder`, added `xpub` view
- Improved test data: Comprehensive test users for role testing
  - ws@example.com → WSManager
  - owner@example.com → Owner
  - user@example.com → Participant
- Updated ROADMAP with detailed implementation plan

### 2025-12-16
- 2.2 Installer Trait Integration: Created `business-installer` crate with
`BusinessInstaller` implementing `Installer` trait from liana-gui
- 2.3 WSS Protocol Extraction: Moved protocol types and domain models to
`liana-connect` crate under `ws_business` module

### 2025-12-15
- 2.1 Auth Client: Implemented authentication using liana-gui's AuthClient with OTP
sign-in and token caching
