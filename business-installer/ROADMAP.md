# ROADMAP

## Priority

- [x] **2. Back**
  - [x] 2.1 Auth Client
  - [x] 2.2 Installer Trait Integration
  - [x] 2.3 WSS Protocol Extraction
- [x] **3.0 Server Update Notifications**
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
- [ ] **1.8 UI/Wording Improvements**
  - [x] 1.8.1 Wallet Status Labels
    - [x] Change "Validated" badge: text to "Set keys", style to warning (amber)
    - [x] Add "Active" badge for Finalized wallets (green, success style)
  - [ ] 1.8.2 Manage Key Modal Field Labels
    - [ ] Rename "Alias" to "Key Alias"
    - [ ] Rename "Description" to "Key Description"
    - [ ] Rename "Email" to "Email Address of the Key Manager"
    - [ ] Add tooltip to "Key Type" with descriptions for each type
  - [ ] 1.8.3 Key Information Screen (Set Keys)
    - [ ] Change "Missing" badge text to "Not Set"
    - [ ] Rename screen title from "Add Key Information" to "Set Keys"
    - [ ] Update breadcrumb from "Key Information" to "Set Keys"
    - [ ] Update intro text with hardware device recommendation
  - [ ] 1.8.4 Header/Breadcrumb Pluralization
    - [ ] Change "Organization" breadcrumb to "Organizations"
    - [ ] Change "Select Organization" title to "Select an Organization"
    - [ ] Change "Wallet" breadcrumb to "Wallets"

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

### 1.3 Add Key Information Subflow ✓
- [x] Create xpub entry view (reuse `SelectKeySource` pattern from liana-gui)
- [x] Integrate hardware wallet detection
  - [x] Add `HardwareWallets` subscription (from `liana-gui/src/hw.rs`)
  - [x] Support device detection: Ledger, Trezor, BitBox02, Coldcard, Jade, Specter
  - [x] Fetch xpub from connected devices
- [x] Support multiple key sources:
  - [x] Hardware wallet (detected devices)
  - [x] Manual xpub entry (paste)
  - [x] Load xpub from file
- [x] Filter keys by user email for Participant role
  - [x] WSManager/Owner: can edit any key
  - [x] Participant: can only edit keys where `key.email == user.email`
- [x] Validate xpub format and network compatibility
- [x] Save xpub to key via backend

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
  - [x] Added `exit` flag in AppState (generic exit signal)
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

### 3.0 Server Update Notifications ✓

Uses existing `Wallet` notifications from server - no protocol changes required.
Conflict detection done by comparing new wallet state with current modal state.

#### Backend (Server)
- [x] Server already broadcasts `Wallet` notifications when wallet is modified
- [x] No protocol changes needed - reuses existing notification infrastructure

#### Frontend (Notification Handling)
- [x] On `Wallet` notification, check if any modal is affected
- [x] Compare new wallet state with modal state to detect conflicts
- [x] Show conflict modal if changes detected

#### Conflict Detection: Key Modal
- [x] Detect when key being edited was modified or deleted
- [x] If key deleted: show info modal ("Key was deleted")
- [x] If key modified: show choice modal ("Reload" / "Keep my changes")

#### Conflict Detection: Path Modal
- [x] Detect when path being edited was modified or deleted
- [x] If path deleted: show info modal ("Path was deleted")
- [x] If path modified: show choice modal ("Reload" / "Keep my changes")
- [x] Detect when keys in current path were removed
- [x] If key removed: show info modal ("Key X was removed")

### 3.1 WS Manager Flow

WS Manager is the platform-side administrator with full access.

**Permissions by wallet status:**

| Status    | Can Edit Template | Can Add Xpubs | Can Load Wallet |
|-----------|-------------------|---------------|-----------------|
| Draft     | ✓ (any key/path)  | ✗             | ✗               |
| Validated | ✗                 | ✗             | ✗               |
| Final     | ✗                 | ✗             | ✗               |

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
| Draft     | ✗                 | ✓            | ✗             | ✗               |
| Validated | ✗                 | ✗            | ✓ (own key)   | ✗               |
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
- [x] Implement `ConnectCache` for token storage:
  ```rust
  pub struct ConnectCache {
      pub accounts: Vec<Account>,
  }

  pub struct Account {
      pub email: String,
      pub tokens: AccessTokenResponse,
  }
  ```
- [x] Read `connect.json` on startup and validate tokens
- [x] Use `update_connect_cache()` pattern from liana-gui
- [x] Token refresh before expiry
- [x] Account selection view for cached tokens
- [x] Handle connection failures with cached tokens (clear and retry)

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
  - [x] Separate AUTH_API_URL and WS_URL constants for REST and WebSocket endpoints
  - [x] Remove embedded dummy server (use standalone liana-business-server only)
  - [x] Add local get_service_config_blocking() for fetching config from server

## Bugs to Fix

### Iced Backend Subscription with Tokio Executor
- [ ] Figure out why backend subscription won't work with Tokio executor
  - **Root cause**: When using Iced with Tokio executor, backend subscription's `poll_next()` hangs forever
  - This is an Iced bug that prevents us from using Tokio executor
  - Forced to use ThreadPool executor instead, which works for backend subscription
  - But ThreadPool executor doesn't provide Tokio runtime context needed by some HW wallets
  - Need to investigate and fix this Iced subscription polling bug

### Hardware Wallet Runtime Compatibility
- [ ] Make async-hwi runtime agnostic
  - Currently async-hwi requires Tokio runtime for some devices (BitBox02, Specter)
  - Need to make it work with any async executor (ThreadPool, Tokio, async-std)
  - Or provide way to run HW operations in isolated Tokio runtime without affecting main executor

- [ ] Ensure all supported signing devices work with business-installer
  - Ledger (HID) - ✓ Working
  - Coldcard - ✓ Working
  - Jade (serial) - ✓ Working
  - BitBox02 - ✗ Disabled (requires `runtime::TokioRuntime`)
  - Specter - ✗ Disabled (requires `tokio::net::TcpStream`)

## Changelog

### 2026-01-07
- Added `last_edited` and `last_editor` fields to `SpendingPathJson` protocol struct
- Server now sets path-level last_edited info when paths are created or modified
- Template view now displays "Edited by [user] [time ago]" for each path

### 2026-01-05
- Refactored `exit_to_liana_lite` flag to generic `exit` flag in AppState
- Moved default `subscription()` implementation to `Installer` trait (no longer needs override in each implementer)

### 2025-12-30
- Xpub modal: Implemented two-step device selection pattern (matching liana-gui installer)
  - Step 1 (Select): Device list, clicking a device opens Details step
  - Step 2 (Details): Account picker, processing state, error handling with Retry
  - Added ModalStep enum (Select, Details) to XpubEntryModalState
  - Added XpubDeviceBack message to return from Details to Select
  - Added XpubRetry message to retry fetch after error
  - Account picker change now triggers re-fetch from device
  - Separated fetch_error from validation_error for Details step
  - Updated FLOW.md with new message categories and navigation flow

### 2025-12-22
- Iced Executor Bug and Hardware Wallet Compatibility Issues Identified
  - **Root issue**: Iced backend subscription's `poll_next()` hangs forever when using Tokio executor
  - Forced to use ThreadPool executor to work around Iced bug
  - ThreadPool executor doesn't provide Tokio runtime context needed by some HW wallets
  - BitBox02 disabled: Requires `runtime::TokioRuntime`
  - Specter disabled: Uses `tokio::net::TcpStream` for TCP communication
  - Ledger/Jade simulators disabled: Use Tokio TCP (not needed for production)
  - Currently working: Ledger (HID), Coldcard (HID), Jade (serial)
  - Application now starts successfully with limited HW support
  - Added "Bugs to Fix" section to track Iced executor bug and HW runtime compatibility

### 2025-12-22
- Fixed xpub view to always show key cards
  - Removed "All keys have been populated!" completion message
  - Key cards are now always visible, even when all xpubs are populated
  - Users can now click on any key card to edit/reset its xpub
  - Fixes issue where users couldn't reset populated keys

### 2025-12-22
- Improved xpub reset/replacement UX
  - Added info banner when key already has an xpub explaining replacement options
  - Added small X button to clear input field in "Other options" section
  - Clarified that Clear button (in footer) removes xpub completely from key
  - Made it clearer that paste/import replaces existing xpub

### 2025-12-22
- Added "Paste extended public key" option to xpub modal
  - Added XpubPaste message and clipboard integration
  - Paste button in "Other options" section alongside file import
  - Reads from system clipboard and extracts first non-empty line as xpub
  - Matches liana-gui "Paste extended public key" feature

### 2025-12-22
- Updated xpub modal to use SelectKeySource-style UX
  - Changed from tab-based to collapsible "Other options" layout
  - Hardware wallet devices now prominently displayed at top
  - File loading moved to collapsible "Other options" section
  - Added options_collapsed state field to XpubEntryModalState
  - Added XpubToggleOptions message for expanding/collapsing options
  - Matches liana-gui SelectKeySource UX pattern for better consistency

### 2025-12-22
- Removed Manual Entry feature from xpub modal
  - Changed from three-tab to two-tab modal (Hardware Wallet | Load from File)
  - Removed XpubSource::ManualEntry variant
  - Updated documentation (FLOW.md, UI_GUIDELINES.md) to reflect two-tab pattern

### 2025-12-22
- 1.3 Add Key Information Subflow: Implemented complete xpub entry flow with hardware wallet support
  - Created xpub view (src/views/xpub/view.rs) displaying key cards with status badges (populated/missing)
  - Implemented two-tab modal for xpub entry: Hardware Wallet, Load from File
  - Hardware wallet infrastructure (src/hw.rs): Device detection for Ledger, Trezor, BitBox02, Coldcard, Jade, Specter
  - Added HardwareWallets subscription with automatic device refresh every 2 seconds
  - Hardware wallet xpub fetching using standard derivation path m/48'/coin'/account'/2'
  - File loading tab with file picker integration
  - Network-agnostic xpub validation using miniscript DescriptorPublicKey parser
  - Role-based key filtering: Participants see only their assigned keys, WSManager/Owner see all keys
  - Status badges showing populated (✓) vs missing (⚠) xpub state
  - Clear functionality to remove xpub from keys
  - Full backend integration for saving xpubs to server
  - Added 12 xpub-related messages for complete modal interaction
  - View accessible from Wallet Select for Validated status wallets

### 2025-12-19
- 4.2 Auth Cache: Implemented cached token authentication
  - Uses same datadir as liana-gui (`~/.liana/<network>/connect.json`)
  - Validates cached tokens on startup (refresh if expired, remove if invalid)
  - AccountSelect view shows list of valid cached accounts
  - "Connect with another email" option for fresh login
  - Handles connection failures with warning modal and cache cleanup
  - See STRUCTURE.md "Authentication Flow" for full diagram

### 2025-12-19
- UI: Added breadcrumb navigation to layout headers
  - Wallet Select shows: `<org_name> > Wallet`
  - Template Builder shows: `<org_name> > <wallet_name> > Template`
  - Keys Management shows: `<org_name> > <wallet_name> > Keys`
  - XPub view shows: `<org_name> > <wallet_name> > Key Information`
  - Login views show: `Login`
  - Org Select shows: `Organization`
  - All segments use same font size (h3), with `>` separators in secondary style

### 2025-12-19
- 5.4 Client Integration: Removed embedded dummy server, now uses standalone server only
  - Removed `init_client_with_test_data()` and all embedded test data generation
  - Added separate `AUTH_API_URL` (http://127.0.0.1:8099) and `WS_URL` (ws://127.0.0.1:8100) constants
  - Added `get_service_config_blocking()` to fetch auth config from local server's `/v1/desktop` endpoint
  - Replaced `block_on` with `tokio::runtime::Runtime::new().block_on()` for async operations
  - Removed `dummy_server_handle`, `dummy_server_shutdown`, and `backend_url` fields from Client
  - Improved error handling with `Notification::Error` for WebSocket message failures

### 2025-12-19
- 3.0 Server Update Notifications: Implemented conflict detection for concurrent editing
  - Uses existing `Wallet` notifications from server (no protocol changes required)
  - Frontend (business-installer): Created `ConflictModalState` and `ConflictType` for conflict resolution UI
  - Frontend (business-installer): Implemented `check_modal_conflicts()` to detect changes by comparing wallet state
  - Frontend (business-installer): Conflict detection for Key Modal - detects when key is modified/deleted during edit
  - Frontend (business-installer): Conflict detection for Path Modal - detects when path is modified/deleted during edit
  - Frontend (business-installer): Key deletion detection in Path Modal - detects when keys in path are removed
  - Frontend (business-installer): Created conflict resolution modal with "Reload" / "Keep my changes" options
  - Frontend (business-installer): Info-only modal for deletion conflicts (can't keep local changes)
  - Frontend (business-installer): Modal rendering priority: warning > conflict > underlying

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
