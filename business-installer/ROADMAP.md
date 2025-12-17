# ROADMAP

## Priority

- [x] **2. Back**
  - [x] 2.1 Auth Client
  - [x] 2.2 Installer Trait Integration
  - [x] 2.3 WSS Protocol Extraction
- [ ] **3.1 WS Manager Flow**
- [ ] **1. Front** (needed for completion of WS Manager flow)
  - [ ] 1.1 Wallet Selection View
  - [ ] 1.2 Edit Wallet Template Subflow
  - [ ] 1.3 Add Key Information Subflow
  - [ ] 1.4 Load Wallet Subflow
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
3. **Load Wallet** - For Final wallets, triggers `exit_maybe()` -> `LoginLianaBusiness`

## 1. Front

### 1.1 Wallet Selection View
- [ ] Display wallet status badge (Draft/Validated/Final) for each wallet
- [ ] Show user's role for each wallet
- [ ] Route to appropriate subflow based on role + status:
  - Draft + (WSManager|Owner) -> Edit Template
  - Draft + Participant -> Access Denied
  - Validated -> Add Key Information
  - Final -> Load Wallet
- [ ] Differentiate UI styling per status

### 1.2 Edit Wallet Template Subflow
- [ ] Restrict access to WSManager and WalletOwner roles
- [ ] Restrict to Draft status wallets only
- [ ] Finalize key management panel
  - [ ] Complete UI implementation
  - [ ] Ensure all key operations are functional
  - [ ] Polish user experience
- [ ] Finalize path management panel
  - [ ] Complete UI implementation
  - [ ] Ensure all path operations are functional
  - [ ] Polish user experience
- [ ] Add "Validate Template" action for Owner (Draft -> Validated transition)

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

### 1.4 Load Wallet Subflow
- [ ] Implement `exit_maybe()` returning `NextState::LoginLianaBusiness`
  - Pattern from `liana-gui/src/installer/mod.rs`:
  ```rust
  NextState::LoginLianaBusiness {
      datadir: LianaDirectory,
      network: Network,
      directory_wallet_id: settings::WalletId,
      auth_cfg: settings::AuthConfig,
  }
  ```
- [ ] Store wallet settings to disk before exit (see Section 4)
- [ ] Store auth cache to disk before exit (see Section 4)
- [ ] Only available for Final status wallets

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
- [ ] Full template editing for Draft wallets
  - [ ] Create/edit/delete keys
  - [ ] Create/edit/delete spending paths
  - [ ] Set thresholds and timelocks
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
- [ ] Template editing for Draft wallets (same as WS Manager)
- [ ] Template validation action (Draft -> Validated transition)
  - [ ] Add "Accept Template" / "Validate" button
  - [ ] Confirm dialog before transition
  - [ ] Backend API call to change status
- [ ] Key info entry for Validated wallets (any key)
- [ ] Wallet loading for Final wallets
- [ ] Testing and have a complete functional flow

### 3.3 Participant Flow

Participant has limited access - can only add xpub for their own keys.

**Permissions by wallet status:**

| Status    | Can Edit Template | Can Add Xpubs          | Can Load Wallet |
|-----------|-------------------|------------------------|-----------------|
| Draft     | ✗                 | ✗                      | ✗               |
| Validated | ✗                 | ✓ (own keys only)      | ✗               |
| Final     | ✗                 | ✗                      | ✓               |

**Implementation tasks:**
- [ ] Connect and authenticate
- [ ] View wallet with restricted access (no edit buttons for Draft)
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

### 2025-12-17
- Updated ROADMAP with detailed implementation plan:
  - Restructured Section 1 (Front) with role-based subflows
  - Added wallet status concepts (Draft/Validated/Final)
  - Added user role concepts (WSManager/Owner/Participant)
  - Detailed Section 3 (Flows) with permission matrices
  - Added Section 4 (Local Storage) for persistence requirements
  - Documented `LoginLianaBusiness` NextState pattern

### 2025-12-16
- 2.2 Installer Trait Integration: Created `business-installer` crate with
`BusinessInstaller` implementing `Installer` trait from liana-gui
- 2.3 WSS Protocol Extraction: Moved protocol types and domain models to
`liana-connect` crate under `ws_business` module

### 2025-12-15
- 2.1 Auth Client: Implemented authentication using liana-gui's AuthClient with OTP
sign-in and token caching
