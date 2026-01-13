# ROADMAP

This file tracks all milestones for liana-business (wrapper + installer + settings).

## Architectural Constraints

- **Backend**: liana-business uses ONLY Liana Connect (no bitcoind, no electrum)
- **Node settings**: Not applicable - no node configuration needed
- **Fiat prices**: May be provided by Liana Connect backend in future (not external APIs)

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

## Completed

### 1. Installer Integration
- [x] 1.1 BusinessInstaller implements `Installer` trait from liana-gui
- [x] 1.2 Basic wrapper (`PolicyBuilder`) around BusinessInstaller
- [x] **1.5 Full GUI Integration**
  - Use `GUI<BusinessInstaller, BusinessSettings, Message>` instead of custom `PolicyBuilder` wrapper
  - This enables full app experience: Installer → Loader → App panels (skips Launcher via `skip_launcher()`)
  - [x] 1.5.1 Update Cargo.toml dependencies
    - Added `business-settings`, `iced_runtime`, `backtrace`, `tokio` (with signal), `log`
    - Removed `futures` (no longer needed)
  - [x] 1.5.2 Rewrite main.rs
    - Removed `PolicyBuilder` struct and impl
    - Import `GUI`, `Config` from `liana_gui::gui`
    - Created type alias: `type LianaBusiness = GUI<BusinessInstaller, BusinessSettings, Message>`
    - Added command-line argument parsing (--datadir, --network, --help, --version)
    - Set up panic hook (without bitcoind cleanup - uses Liana Connect only)
    - Configured Iced settings and window (1200x800 default, 1000x650 min, "LianaBusiness" app ID)
    - Run app via `iced::application()` with `LianaBusiness`
  - [x] 1.5.3 Verify build with `cargo build`
  - [x] 1.5.4 Added `skip_launcher() -> true` to BusinessInstaller (starts directly with Installer)

### 2. Settings Trait Design

- [x] **Phase 1: Data Layer**
  - [x] 2.1 Define Settings traits in liana-gui
    - [x] Create `SettingsTrait` with common operations (load, wallets)
    - [x] Create `WalletSettingsTrait` for wallet-level settings
    - [x] Add associated type for wallet settings
  - [x] 2.2 Implement LianaSettings in liana-gui
    - [x] Rename existing `Settings` struct to `LianaSettings`
    - [x] Rename existing `WalletSettings` struct to `LianaWalletSettings`
    - [x] Implement `SettingsTrait` for `LianaSettings`
    - [x] Implement `WalletSettingsTrait` for `LianaWalletSettings`
    - [x] Add type aliases for backward compatibility
  - [x] 2.3 Make GUI framework generic over Settings
    - [x] Add `S: SettingsTrait` to `GUI<I, S, M>`
    - [x] Update `LianaGUI` type alias to use `LianaSettings`
  - [x] 2.4 Propagate S to Tab/Pane
    - [x] Add `S: SettingsTrait` to `Tab<I, S, M>`, `State<I, S, M>`
    - [x] Add `S: SettingsTrait` to `Pane<I, S, M>`
  - [x] 2.5 Create BusinessSettings in business-settings crate
    - [x] Define `BusinessSettings` struct implementing `SettingsTrait`
    - [x] Define `BusinessWalletSettings` implementing `WalletSettingsTrait`
      - NO `start_internal_bitcoind` field
  - [x] 2.6 Verify builds
- [x] **Phase 2: UI Layer (2.7 Settings UI Trait Design)**
  - [x] 2.7.1 Define `SettingsUI` trait in liana-gui
    - Location: `liana-gui/src/app/settings/ui.rs`
  - [x] 2.7.2 Implement `LianaSettingsUI` in liana-gui
    - Location: `liana-gui/src/app/state/settings/mod.rs`
  - [x] 2.7.3 Create `BusinessSettingsUI` in business-settings
    - Location: `liana-business/business-settings/src/ui.rs`
  - [x] 2.7.4 Verify builds
- [x] **Phase 3: Full Integration (2.8 Make App Generic)**
  - [x] Make `App<S: SettingsTrait>` generic over settings
  - [x] Update `State::App(App)` in tab.rs to use generic `App<S>`
  - [x] Add `create_app_for_remote_backend` method to `SettingsTrait`
  - [x] Move remote backend App creation logic to `settings/mod.rs`
  - [x] Add `State` as bound on `SettingsTrait::UI`
  - [x] Update `BusinessSettingsUI` to implement `State` trait

### 3. Backend (Auth & Protocol)
- [x] **3.1 Auth Client**
  - [x] Export required auth types from liana-gui
  - [x] Implement auth_request() using AuthClient::sign_in_otp()
  - [x] Implement auth_code() using AuthClient::verify_otp() with token caching
  - [x] Implement token caching in connect_ws()
- [x] **3.2 Installer Trait Integration**
  - [x] Wrap complete app under the Installer trait of liana-gui
  - [x] Support standalone mode (liana-business wraps BusinessInstaller)
  - [x] Support integration into liana-gui (via Installer trait interface)
- [x] **3.3 WSS Protocol Extraction**
  - [x] Move shared WSS protocol types to liana-connect crate
  - [x] Create liana-connect/src/protocol.rs with JSON payload types
  - [x] Create liana-connect/src/models.rs with domain types

### 4. Frontend Views

- [x] **4.1 Wallet Selection View**
  - [x] Display wallet status badge (Draft/Validated/Final)
  - [x] Show user's role for each wallet
  - [x] Route to appropriate subflow based on role + status
  - [x] Sort wallets by status (Draft first, Finalized last)
  - [x] Filter out Draft wallets from Participant view
  - [x] "Hide finalized wallets" checkbox for WSManager
  - [x] Show "WS Manager" badge in header

- [x] **4.2 Edit Wallet Template Subflow**
  - [x] Restrict access to WSManager and WalletOwner roles
  - [x] Restrict to Draft status wallets only
  - [x] Finalize key management panel (UI, operations, UX)
  - [x] Finalize path management panel
    - [x] Clickable path cards in template visualization
    - [x] Edit Path modal with key selection (checkboxes)
    - [x] Threshold input with validation
    - [x] Timelock input with unit dropdown (blocks/hours/days/months)
  - [x] Add "Validate Template" action for Owner (Draft -> Validated)

- [x] **4.3 Add Key Information Subflow**
  - [x] Create xpub entry view (reuse `SelectKeySource` pattern)
  - [x] Integrate hardware wallet detection
    - [x] Add `HardwareWallets` subscription
    - [x] Support: Ledger, Trezor, BitBox02, Coldcard, Jade, Specter
    - [x] Fetch xpub from connected devices
  - [x] Support multiple key sources:
    - [x] Hardware wallet (detected devices)
    - [x] Manual xpub entry (paste)
    - [x] Load xpub from file
  - [x] Filter keys by user email for Participant role
  - [x] Validate xpub format and network compatibility
  - [x] Save xpub to key via backend

- [x] **4.4 Filter/Search Bar (WS Manager Only)**
  - [x] Add search bar to organization selection page
  - [x] Add filter to wallet selection page
  - [x] "Hide finalized wallets" checkbox (WSManager only)

- [x] **4.5 Keyboard Navigation in Login**
  - [x] Tab navigation between input fields
  - [x] Enter key to submit forms
  - [x] Auto-focus email/code inputs

- [x] **4.6 Load Wallet Subflow**
  - [x] Implement `exit_maybe()` returning `NextState::LoginLianaLite`
  - [x] Added `exit` flag in AppState
  - [x] Build `WalletId` and `AuthConfig` for handoff

- [x] **4.7 Logout Feature**
  - [x] Add logout button in UI
  - [x] Clear authentication token from memory
  - [x] Clear auth cache from disk (`connect.json`)
  - [x] Close WebSocket connection
  - [x] Reset application state to initial login view

- [x] **4.8 UI/Wording Improvements**
  - [x] Wallet Status Labels ("Set keys", "Active")
  - [x] Manage Key Modal Field Labels
  - [x] Key Information Screen improvements
  - [x] Header/Breadcrumb Pluralization

- [x] **4.9 UI Feedback (2026-01-08)**
  - [x] 4.9.2 Key Type Dropdown - Tooltip Wording
  - [x] 4.9.3 Set Keys View - Waiting State
  - [x] 4.9.4 Set Keys View - Changed "Populated" to "Set"

### 5. Server Update Notifications
- [x] Uses existing `Wallet` notifications from server
- [x] Conflict detection by comparing new wallet state with modal state
- [x] **Key Modal Conflict Detection**
  - [x] Detect when key being edited was modified or deleted
  - [x] Show info/choice modal
- [x] **Path Modal Conflict Detection**
  - [x] Detect when path being edited was modified or deleted
  - [x] Detect when keys in current path were removed

### 6. Standalone Server Binary
- [x] **6.1 Server Implementation**
  - [x] Create `liana-business-server` crate
  - [x] Multi-client connection management
  - [x] Token-based authentication
  - [x] Shared state management with Arc<Mutex<...>>
  - [x] Notification broadcasting
  - [x] CLI argument parsing (--host, --port, --log-level)
- [x] **6.2 Deployment Support**
  - [x] Systemd service file
  - [x] Comprehensive README.md
- [x] **6.3 Testing**
  - [x] Integration tests (connection, auth, multi-client, ping/pong)
- [x] **6.4 Client Integration**
  - [x] Separate AUTH_API_URL and WS_URL constants
  - [x] Removed embedded dummy server

### 7. Auth Cache
- [x] Implemented `ConnectCache` for token storage
- [x] Read `connect.json` on startup and validate tokens
- [x] Token refresh before expiry
- [x] Account selection view for cached tokens

## In Progress

- [ ] **WS Manager Flow (3.1)**
  - [x] Full template editing for Draft wallets
  - [ ] Wallet loading for Final wallets
  - [ ] Testing and validation

- [ ] **Owner Flow (3.2)**
  - [x] Template editing for Draft wallets
  - [x] Template validation action
  - [x] Key info entry for Validated wallets
  - [ ] Wallet loading for Final wallets
  - [ ] Testing and complete functional flow

- [ ] **Participant Flow (3.3)**
  - [x] Connect and authenticate
  - [x] Draft wallets filtered from view
  - [x] Add/edit xpub for own keys only
  - [ ] Wallet loading for Final wallets
  - [ ] Testing and complete functional flow

## Not Started

- [ ] **Local Storage (4)**
  - [ ] 4.1 Wallet Settings (`settings.json`)
    - [ ] Implement `WalletSettings` struct
    - [ ] Write `settings.json` on wallet load (before `exit_maybe`)
  - [ ] 4.3 Hardware Wallet Config
    - [ ] Store `HardwareWalletConfig` for registered devices

- [x] **Auth Improvements**
  - [x] Automatically refresh token

- [x] **Separate Backend URL**
  - liana-business uses:
    - Mainnet: `api.business.lianawallet.com`
    - Signet: `api.signet.business.lianawallet.com`
  - liana-gui uses:
    - Mainnet: `api.lianalite.com`
    - Signet: `api.signet.lianalite.com`
  - Initially DNS will point to the same server
  - Enables future backend decoupling via DNS without software update
  - [x] Update liana-business to use separate backend URL constant
    - `client.rs`: `auth_api_url(network)` and `ws_url(network)` functions
  - [x] Add env var override for backend URL (both liana-gui & liana-business)
    - liana-business:
      - `LIANA_BUSINESS_API_URL` / `LIANA_BUSINESS_WS_URL` (any network)
      - `LIANA_BUSINESS_SIGNET_API_URL` / `LIANA_BUSINESS_SIGNET_WS_URL`
      - `LIANA_BUSINESS_MAINNET_API_URL` / `LIANA_BUSINESS_MAINNET_WS_URL`
    - liana-gui:
      - `LIANA_LITE_API_URL` (any network)
      - `LIANA_LITE_SIGNET_API_URL`, `LIANA_LITE_MAINNET_API_URL`
  - [ ] Verify DNS records are configured

- [x] **NextState::RunLianaBusiness Variant (Direct to App)**
  - liana-business users are already authenticated with connected backend in installer
  - `NextState::RunLianaBusiness` goes **directly to App**, skipping Login and Loader
  - **Implementation:**
    - [x] Added `RunLianaBusiness` variant to `NextState` enum in `liana-gui/src/installer/mod.rs`
      - Fields: `datadir`, `network`, `wallet_id`, `email`
    - [x] Updated `tab.rs` to handle new variant
      - Spawns async `connect_for_business()` using cached tokens
      - On success: creates App via `S::create_app_for_remote_backend()`
      - On failure: falls back to `LianaLiteLogin` for re-authentication
    - [x] Updated `BusinessInstaller::exit_maybe()` to return `RunLianaBusiness`
    - [x] Tokens cached in `connect.json` during installer auth flow
    - [x] App's `RedirectLianaConnectLogin` handled by falling back to Login state

- [ ] **Reproducible Build Integration**
  - [ ] Add liana-business to Guix build script
  - [ ] Add liana-business to release packaging

- [ ] **Datadir Conflict Verification**
  - Note: Server-side conflict detection exists (modal UI for concurrent editing)
  - [ ] Local datadir conflict detection between liana-business and liana-gui

- [ ] **Custom Icon**
  - [ ] Add liana-business icon asset

- [ ] **Custom Theme**
  - [ ] Implement custom theme for liana-business
  - [ ] Integrate theme into liana-business app
  - [ ] Custom warning pill color (amber/yellow instead of red)
    - Blocked: 4.9.1 Badge colors (Set Keys, Not Set)
    - Blocked: 4.9.4 Locked label amber color

- [ ] **Hardware Wallet Testing**
  - [x] Ledger (HID)
  - [x] Coldcard
  - [ ] BitBox02
    - [ ] Cache pairing code
  - [ ] Jade (serial)
  - [ ] Specter

- [ ] **CI Integration**
  - [ ] Add liana-business to CI pipeline

## Known Issues

- [x] WS Manager role: Org list wallet count includes non-visible wallets
- [x] Login page: Wrong email greys out "Send token" button, user stuck (dummy server issue)
- [x] Owner role: "Manage Key" button displayed after wallet approved (should be hidden)

## Bugs Fixed

### Iced Backend Subscription with Tokio Executor
- [x] Fixed: Now uses Tokio executor
  - Previously: Backend subscription's `poll_next()` hung with Tokio executor
  - Resolution: Iced subscription polling issue resolved

### Hardware Wallet Runtime Compatibility
- [x] Now uses `async-hwi::Service` - no runtime incompatibility
  - Previously: async-hwi required Tokio runtime for some devices
  - Resolution: Service abstraction handles runtime requirements

## Changelog

### 2026-01-12
- Fixed: WS Manager org list wallet count now respects "Hide finalized wallets" filter
- Fixed: Login page "Send token" button re-enabled after backend errors
- Fixed: Owner role "Manage Keys" button hidden after wallet approved (Validated/Finalized)

### 2026-01-09
- Separate Backend URL with network-specific endpoints:
  - liana-business: api.business.lianawallet.com (mainnet), api.signet.business.lianawallet.com (signet)
  - liana-gui: api.lianalite.com (mainnet), api.signet.lianalite.com (signet)
- Added env var overrides for local testing:
  - liana-business: LIANA_BUSINESS_API_URL, LIANA_BUSINESS_WS_URL (global)
  - liana-business: LIANA_BUSINESS_SIGNET_API_URL, LIANA_BUSINESS_MAINNET_API_URL (network-specific)
  - liana-gui: LIANA_LITE_API_URL, LIANA_LITE_SIGNET_API_URL, LIANA_LITE_MAINNET_API_URL
- NextState::RunLianaBusiness: Direct Installer→App transition (skips Login/Loader)
  - Added `connect_for_business()` in tab.rs for token-based connection
  - BusinessInstaller exits directly to App using cached tokens
  - Falls back to LianaLiteLogin on connection failure
- Auth Improvements: Background token refresh thread (checks every 60s, refreshes 5 min before expiry)
- 1.5 Full GUI Integration: Replaced PolicyBuilder with GUI<BusinessInstaller, BusinessSettings, Message>
- Added skip_launcher() -> true to BusinessInstaller
- Added command-line argument parsing (--datadir, --network, --help, --version)
- Set up panic hook (without bitcoind cleanup)
- Configured Iced settings and window (1200x800 default, 1000x650 min, "LianaBusiness" app ID)

### 2026-01-08
- 4.9.2: Updated key type tooltips for Co-signer and Safety Net
- 4.9.3: Added waiting state in Set Keys view when user's keys are all set
- 4.9.4: Changed "Populated" label to "Set" in xpub view
- Deferred 4.9.1 (badge colors) and 4.9.4 (Locked amber) pending custom theme

### 2026-01-07
- Fixed Iced executor issue: Now uses Tokio executor
- Fixed hardware wallet runtime compatibility: Now uses `async-hwi::Service`
- HW wallet testing status: Ledger, Coldcard tested; BitBox02, Jade, Specter pending
- Added `last_edited` and `last_editor` fields to `SpendingPathJson` protocol struct
- Template view now displays "Edited by [user] [time ago]" for each path

### 2026-01-05
- Refactored `exit_to_liana_lite` flag to generic `exit` flag in AppState
- Moved default `subscription()` implementation to `Installer` trait

### 2025-12-30
- Xpub modal: Implemented two-step device selection pattern (matching liana-gui installer)
  - Step 1 (Select): Device list
  - Step 2 (Details): Account picker, processing state, error handling

### 2025-12-22
- 4.3 Add Key Information Subflow: Implemented complete xpub entry flow with hardware wallet support
- Removed Manual Entry feature from xpub modal (two-tab pattern now)
- Updated xpub modal to use SelectKeySource-style UX
- Added "Paste extended public key" option to xpub modal
- Improved xpub reset/replacement UX
- Fixed xpub view to always show key cards

### 2025-12-19
- 7: Auth Cache - Implemented cached token authentication
- UI: Added breadcrumb navigation to layout headers
- 6.4 Client Integration: Removed embedded dummy server, now uses standalone server only
- 5: Server Update Notifications - Implemented conflict detection for concurrent editing

### 2025-12-18
- 6: Standalone Server Binary - Implemented complete standalone server
- Auth improvements: Connect WebSocket immediately after login success
- 4.2 Edit Wallet Template Subflow: Refactored Manage Keys view
- Implemented role-based access control and validation
- Reworked template visualization and modals
- UI improvement: Made only the org/wallet list scrollable

### 2025-12-17
- 4.7 Logout Feature: Implemented
- 4.4 Filter/Search Bar (WS Manager Only): Implemented
- 4.5 Better Keyboard Navigation in Login: Implemented

### 2025-12-16
- 4.1 Wallet Selection View: Implemented and enhanced
- 4.6 Load Wallet Subflow: Implemented `exit_maybe()` -> `NextState::LoginLianaLite`
- View restructuring: Renamed `home` view to `template_builder`, added `xpub` view
- 3.2 Installer Trait Integration: Created `business-installer` crate
- 3.3 WSS Protocol Extraction: Moved protocol types to `liana-connect` crate

### 2025-12-15
- 3.1 Auth Client: Implemented authentication using liana-gui's AuthClient
