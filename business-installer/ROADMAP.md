# ROADMAP

## Priority

- [x] **2. Back**
  - [x] 2.1 Auth Client
  - [x] 2.2 Installer Trait Integration
  - [x] 2.3 WSS Protocol Extraction
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

## Bugfixes

## Changelog

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
