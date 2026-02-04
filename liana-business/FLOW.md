# Liana Business Application Flow

This document describes the application flow for liana-business.

## Application Architecture

Liana Business uses the `GUI<I, S, M>` framework from liana-gui with three main components:

```
+------------------------------------------------------------------+
|                        LianaBusiness                              |
|         GUI<BusinessInstaller, BusinessSettings, Message>         |
+------------------------------------------------------------------+
                               |
          +--------------------+--------------------+
          |                    |                    |
          v                    v                    v
+------------------+  +------------------+  +------------------+
|    INSTALLER     |  |       APP        |  |     SETTINGS     |
| BusinessInstaller|  |   (liana-gui)    |  | BusinessSettings |
+------------------+  +------------------+  +------------------+
| - Authentication |  | - Wallet ops     |  | - Configuration  |
| - Org/Wallet sel |  | - Transactions   |  | - Preferences    |
| - Template edit  |  | - Coin selection |  |                  |
| - Xpub entry     |  | - History        |  |                  |
+------------------+  +------------------+  +------------------+
        |                    |                    |
        v                    v                    v
   [This document]      [liana-gui docs]        [TBD]
```

### Component Responsibilities

```
+-------------------+--------------------------------------------------+
| Component         | Responsibility                                   |
+-------------------+--------------------------------------------------+
| BusinessInstaller | Policy template configuration before wallet load |
|                   | - Email/OTP authentication                       |
|                   | - Organization and wallet selection              |
|                   | - Template editing (keys, paths, thresholds)     |
|                   | - Xpub information entry from hardware wallets   |
|                   | - Handoff to App via exit_maybe()                |
+-------------------+--------------------------------------------------+
| App (liana-gui)   | Wallet operations after successful load          |
|                   | - Transaction creation and signing               |
|                   | - Coin selection and UTXO management             |
|                   | - Transaction history                            |
|                   | - Recovery path monitoring                       |
+-------------------+--------------------------------------------------+
| BusinessSettings  | Application configuration (TBD)                  |
|                   | - No bitcoind settings (uses Liana Connect only) |
|                   | - Wallet list management                         |
+-------------------+--------------------------------------------------+
```

### Key Difference from liana-gui

```
+---------------------------+---------------------------+
|        liana-gui          |     liana-business        |
+---------------------------+---------------------------+
| Launcher → Installer → App| Installer → App           |
|                           |                           |
| Launcher handles:         | Installer handles:        |
| - Network selection       | - Authentication (login)  |
| - Wallet creation choice  | - Network is preset       |
|                           | - Org/wallet selection    |
+---------------------------+---------------------------+
```

**Why liana-business skips the Launcher:**

The authentication/login flow was originally implemented in the Installer for
liana-business. Since users must authenticate before any wallet operations,
and network selection is not needed (Liana Connect provides the backend),
the Launcher phase is bypassed via `skip_launcher() -> true`.

### Application Flow Overview

```
App Start
    │
    ▼
skip_launcher() = true ──▶ Skip Launcher (login is in Installer)
    │
    ▼
+------------------+
|    INSTALLER     |  ◄── This document covers this phase
|  (business-      |      (includes authentication)
|   installer)     |
+------------------+
    │
    │ [User selects Final wallet]
    │ [exit_maybe() returns NextState::RunLianaBusiness]
    │
    │ NOTE: Goes DIRECTLY to App, skipping Login and Loader!
    │       - User already authenticated in Installer
    │       - Backend already connected (BackendWalletClient)
    │       - No bitcoind sync needed (Liana Connect only)
    ▼
+------------------+
|       APP        |  ◄── liana-gui documentation covers this
|   (liana-gui)    |
+------------------+
    │
    │ [User opens settings]
    ▼
+------------------+
|     SETTINGS     |  ◄── TBD (BusinessSettings)
+------------------+
```

**Comparison with liana-gui LianaLite flow:**

```
LianaLite (liana-gui):
  Installer → LoginLianaLite → LianaLiteLogin (OTP) → Loader (sync) → App

LianaBusiness:
  BusinessInstaller → RunLianaBusiness → App (DIRECT!)
```

---

# Installer Flow (BusinessInstaller)

The following sections describe the state management, message flow, and patterns
specific to the **Installer** phase implemented in `business-installer`.

## Installer State Management

The installer maintains its own `State` struct in `business-installer/src/state/mod.rs`:

```
State (state/mod.rs)
├── app: AppState        # Domain data (keys, paths, wallet template)
├── views: ViewsState    # UI-specific state per view
├── backend: Client      # WebSocket communication
├── hw: HwiService       # Hardware wallet service
└── current_view: View   # Routing state
```

### Installer State Hierarchy

- **State** - Root container in `state/mod.rs`
- **AppState** - Domain data: keys, paths, selected org/wallet, user role
- **ViewsState** - Per-view UI state (modals, form fields, edit states)
- **Client** - WebSocket connection and caches (orgs, wallets, users)

### AppState Fields

```rust
pub struct AppState {
    pub keys: BTreeMap<u8, Key>,
    pub primary_path: SpendingPath,
    pub secondary_paths: Vec<(SpendingPath, Timelock)>,
    pub next_key_id: u8,
    pub selected_org: Option<Uuid>,
    pub selected_wallet: Option<Uuid>,
    pub current_wallet_template: Option<PolicyTemplate>,
    pub current_user_role: Option<UserRole>,
    pub exit: bool,
}
```

### ViewsState Structure

```rust
pub struct ViewsState {
    pub modals: ModalsState,      // Warning/conflict modals
    pub keys: KeysViewState,      // Key edit modal state
    pub paths: PathsViewState,    // Path edit modal state
    pub xpub: XpubViewState,      // Xpub entry modal state
    pub login: Login,             // Login form state
    pub org_select: OrgSelectState,
    pub wallet_select: WalletSelectState,
}
```

## Installer Message Flow

```
User Action
    │
    ▼
Msg enum (state/message.rs)
    │
    ▼
State::update() (state/update.rs)
    │
    ├──▶ State mutation (self.app, self.views, self.current_view)
    │
    └──▶ Backend call (self.backend.*)
              │
              ▼
         WebSocket Request
              │
              ▼
         WSS Response
              │
              ▼
         Notification (backend.rs)
              │
              ▼
         NotifListener (installer.rs)
              │
              ▼
         Msg::BackendNotif(Notification)
              │
              ▼
         State::update() → State::on_backend_notif()
```

### Installer Message Categories

```
+---------------------+----------------------------------------------------------+
| Category            | Messages                                                 |
+---------------------+----------------------------------------------------------+
| Login/Auth          | LoginUpdateEmail, LoginUpdateCode, LoginSendToken,       |
|                     | LoginResendToken, LoginSendAuthCode, Logout              |
+---------------------+----------------------------------------------------------+
| Account Select      | AccountSelectConnect, AccountSelectDelete,               |
|                     | AccountSelectNewEmail                                    |
+---------------------+----------------------------------------------------------+
| Org Management      | OrgSelected, OrgWalletSelected, OrgCreateNewWallet,      |
|                     | OrgSelectUpdateSearchFilter                              |
+---------------------+----------------------------------------------------------+
| Wallet Select       | WalletSelectToggleHideFinalized,                         |
|                     | WalletSelectUpdateSearchFilter                           |
+---------------------+----------------------------------------------------------+
| Key Management      | KeyAdd, KeyEdit, KeyDelete, KeySave, KeyCancelModal,     |
|                     | KeyUpdate*                                               |
+---------------------+----------------------------------------------------------+
| Xpub Management     | XpubSelectKey, XpubUpdateInput, XpubSelectSource,        |
|                     | XpubSelectDevice, XpubDeviceBack, XpubFetchFromDevice,   |
|                     | XpubRetry, XpubLoadFromFile, XpubFileLoaded, XpubPaste,  |
|                     | XpubPasted, XpubUpdateAccount, XpubSave, XpubClear,      |
|                     | XpubCancelModal, XpubToggleOptions                       |
+---------------------+----------------------------------------------------------+
| Template Management | TemplateAddKeyTo*, TemplateDelKeyFrom*,                  |
|                     | TemplateAddSecondaryPath, TemplateDeleteSecondaryPath,   |
|                     | TemplateEditPath, TemplateNewPathModal,                  |
|                     | TemplateToggleKeyInPath, TemplateSavePath,               |
|                     | TemplateCancelPathModal, TemplateUpdate*,                |
|                     | TemplateLock, TemplateUnlock, TemplateValidate           |
+---------------------+----------------------------------------------------------+
| Navigation          | NavigateToHome, NavigateToKeys, NavigateToOrgSelect,     |
|                     | NavigateToWalletSelect, NavigateBack                     |
+---------------------+----------------------------------------------------------+
| Backend             | BackendNotif, BackendDisconnected                        |
+---------------------+----------------------------------------------------------+
| Hardware Wallet     | HardwareWallets                                          |
+---------------------+----------------------------------------------------------+
| Warnings            | WarningShowModal, WarningCloseModal                      |
+---------------------+----------------------------------------------------------+
| Conflicts           | ConflictReload, ConflictKeepLocal, ConflictDismiss       |
+---------------------+----------------------------------------------------------+
```

### Backend Notifications

```
+----------------+-------------------------------------+
| Notification   | Triggered By                        |
+----------------+-------------------------------------+
| Connected      | Successful WebSocket connection     |
| Disconnected   | Connection lost or closed           |
| AuthCodeSent   | OTP sent to email                   |
| InvalidEmail   | Email validation failed             |
| AuthCodeFail   | Failed to send OTP                  |
| LoginSuccess   | OTP verified successfully           |
| LoginFail      | OTP verification failed             |
| Org(Uuid)      | Org data received/updated           |
| Wallet(Uuid)   | Wallet data received/updated        |
| User(Uuid)     | User data received/updated          |
| Error(Error)   | Backend error occurred              |
+----------------+-------------------------------------+
```

## Installer View Routing

Installer views are determined by `State::current_view` and `State::route()`:

```
+--------------+---------------------+------------------------------------+
| View         | Entry Point         | Purpose                            |
+--------------+---------------------+------------------------------------+
| Login        | Initial view        | Email/OTP authentication           |
| OrgSelect    | After auth          | Organization picker                |
| WalletSelect | After org select    | Wallet picker with status/role     |
| WalletEdit   | After wallet select | Template overview (home)           |
| Keys         | From home           | Manage keys                        |
| Xpub         | From wallet select  | Add xpub (Validated wallets only)  |
+--------------+---------------------+------------------------------------+
```

### Wallet Selection Routing

When user selects a wallet, routing depends on wallet status and user role:

```
+-----------+-----------------+---------------------------------------------+
| Status    | Role            | Destination                                 |
+-----------+-----------------+---------------------------------------------+
| Draft     | WS Admin/Wallet Manager | WalletEdit (template builder)               |
| Draft     | Participant     | Warning Modal (access denied)               |
| Validated | Any             | Xpub (key information entry)                |
| Final     | Any             | exit_maybe() -> RunLianaBusiness -> App     |
+-----------+-----------------+---------------------------------------------+
```

### Installer Navigation Flow

```
Login (EmailEntry)
    │
    ▼ [LoginSendToken]
Login (CodeEntry)
    │
    ▼ [LoginSuccess]
OrgSelect
    │
    ▼ [OrgSelected]
WalletSelect (shows status badges + roles)
    │
    ├──▶ [OrgWalletSelected] → Access check → Route by status
    │                              │
    │                              ├──▶ Draft → WalletEdit
    │                              │
    │                              ├──▶ Validated → Xpub
    │                              │
    │                              ├──▶ Final → exit_maybe() → RunLianaBusiness → App
    │                              │
    │                              └──▶ (Draft + Participant) → Warning Modal
    │
    └──▶ [OrgCreateNewWallet] → WalletEdit

WalletEdit ◄──▶ Keys [NavigateToKeys / NavigateBack]

Xpub View:
    Opens XpubEntryModal on key card click
        └──▶ [XpubSelectKey] → Modal with two-step UX:
                │
                Step 1: SELECT (device selection)
                ├──▶ Hardware Wallet section (prominently displayed)
                │       - Device list with status indicators
                │       - Click device card to open Details step
                │
                └──▶ "Other options" collapsible section
                        - Import extended public key file
                        - Paste extended public key
                │
                Step 2: DETAILS (account selection + fetch)
                └──▶ [XpubSelectDevice] → Opens Details step:
                        - Back button to return to Select
                        - Device card (shows processing state)
                        - Account picker (changing triggers re-fetch)
                        - Error display with Retry button
                        - Save button (enabled when valid xpub)

### Dynamic Status Transitions in Xpub View

When on the Xpub view, wallet status updates can trigger automatic view transitions:

```
+-----------------+-----------------------------+-------------------------------+
| Effective Status| Action                      | Destination                   |
+-----------------+-----------------------------+-------------------------------+
| Registration    | Close modal, setup state    | View::Registration            |
|                 | start_hw()                  | (user has devices to register)|
+-----------------+-----------------------------+-------------------------------+
| Finalized       | Close modal, stop_hw()      | app.exit = true               |
|                 | set exit flag               | (opens main wallet app)       |
+-----------------+-----------------------------+-------------------------------+
| Other           | Stay on Xpub view           | No change                     |
+-----------------+-----------------------------+-------------------------------+
```

Note: Server always sends `WalletStatus::Finalized`, but the app uses
`wallet.effective_status(&user_email)` to infer `Registration` if the user
has devices pending registration.
```

### Dynamic Status Transitions in Registration View

When on the Registration view, wallet status updates can trigger automatic view transitions:

```
+-----------------+-----------------------------+-------------------------------+
| Effective Status| Action                      | Destination                   |
+-----------------+-----------------------------+-------------------------------+
| Registration    | Update user_devices list    | Stay on Registration          |
|                 |                             | (refresh device list)         |
+-----------------+-----------------------------+-------------------------------+
| Finalized       | Close modal, stop_hw()      | app.exit = true               |
|                 | set exit flag               | (opens main wallet app)       |
+-----------------+-----------------------------+-------------------------------+
```

When all devices are registered or skipped, the wallet's effective status becomes
`Finalized`, triggering automatic exit to the main wallet application.

## Adding New Installer Features

The following checklists are for adding features to the **Installer** (business-installer).

### New Installer View Checklist

1. **Create state struct** in `state/views/new_view.rs`:
   ```rust
   #[derive(Debug, Clone, Default)]
   pub struct NewViewState {
       // View-specific fields
   }
   ```

2. **Add to ViewsState** in `state/views/mod.rs`:
   ```rust
   pub struct ViewsState {
       pub new_view: NewViewState,
   }
   ```

3. **Add View variant** in `state/mod.rs`:
   ```rust
   pub enum View {
       NewView,
   }
   ```

4. **Create render function** in `views/new_view.rs`:
   ```rust
   pub fn new_view(state: &State) -> Element<'_, Message> {
       layout_with_scrollable_list(...)
   }
   ```

5. **Export from views** in `views/mod.rs`

6. **Add routing** in `State::view()` in `state/mod.rs`

7. **Add navigation messages** if needed in `state/message.rs`

### New Installer Message Checklist

1. **Add variant** to `Msg` enum in `state/message.rs`

2. **Add handler** in `State::update()` in `state/update.rs`:
   ```rust
   Msg::NewAction(data) => self.on_new_action(data),
   ```

3. **Implement handler** on `State`

4. **Wire up in view** with `.on_press()`

### New Installer Backend Request Checklist

1. **Add trait method** in `backend.rs`

2. **Implement in Client** in `client.rs`

3. **Add Notification variant** if new response type

4. **Handle notification** in `State::on_backend_notif()`

## Installer Modal Pattern

### Opening a Modal

```rust
fn on_key_edit(&mut self, key_id: u8) {
    if let Some(key) = self.app.keys.get(&key_id) {
        self.views.keys.edit_key = Some(EditKeyModalState {
            key_id,
            alias: key.alias.clone(),
            // ...
        });
    }
}
```

### Rendering Modals

Modal priority order in `views/modals/mod.rs`:
1. Warning modal (highest)
2. Conflict modal
3. Xpub modal
4. Key edit modal
5. Path edit modal

### Closing a Modal

```rust
fn on_key_cancel_modal(&mut self) {
    self.views.keys.edit_key = None;
}
```

## Installer Conflict Detection

When the server sends a `Wallet` notification during modal editing:

1. Compare new wallet state with current modal state
2. If key/path was modified → Show choice modal ("Reload" / "Keep my changes")
3. If key/path was deleted → Show info modal ("Key was deleted")

## Installer Exit Flow (Handoff to App)

When the user selects a **Final** wallet, the installer hands off **directly** to the App:

```
BusinessInstaller                       liana-gui Framework
    │                                          │
    │ [User selects Final wallet]              │
    ▼                                          │
Set self.app.exit = true                       │
    │                                          │
    ▼                                          │
Prepare handoff data:                          │
  - BackendWalletClient (already connected)    │
  - Wallet data & coins                        │
  - Auth config (for re-auth on disconnect)    │
    │                                          │
    ▼                                          │
BusinessInstaller::exit_maybe() ──────────────▶│
    returns Some(NextState::RunLianaBusiness)  │
                                               │
                    [DIRECT TRANSITION - No Login/Loader!]
                                               │
                                               ▼
                                        App Phase (liana-gui)
```

**Why direct to App (skipping Login and Loader)?**

1. **Already authenticated** - User logged in during Installer with valid tokens
2. **Backend connected** - `BackendWalletClient` established and ready to use
3. **No bitcoind** - Liana Connect handles everything, no local sync needed

**Handoff data in `NextState::RunLianaBusiness`:**

```
+----------------------+------------------------------------------------+
| Field                | Purpose                                        |
+----------------------+------------------------------------------------+
| datadir              | LianaDirectory for configuration               |
| network              | Bitcoin network (Signet/Mainnet)               |
| gui_config           | App configuration                              |
| daemon               | BackendWalletClient (already connected!)       |
| wallet               | api::Wallet data                               |
| cache                | Coins and transaction cache                    |
| email                | User's email for re-authentication             |
+----------------------+------------------------------------------------+
```

Note: Token is already valid (user authenticated in Installer) and stored in
`connect.json` cache. No need to pass token explicitly in the handoff.

**Re-authentication handling:**

If the App loses connection (session expiry), it sends `RedirectLianaConnectLogin`.
The stored email enables initiating a new OTP authentication flow if needed.

---

# App Flow (liana-gui)

The App phase handles wallet operations after successful load. This is implemented
in `liana-gui` and documented separately. Key features include:

- Transaction creation and signing
- Coin selection and UTXO management
- Transaction history viewing
- Recovery path monitoring
- Spend policy visualization

See `liana-gui` documentation for App flow details.

---

# Settings Flow (BusinessSettings)

**Status: TBD**

BusinessSettings will provide configuration for liana-business. Key differences
from LianaSettings (liana-gui):

- **No bitcoind settings** - liana-business uses Liana Connect exclusively
- **No electrum settings** - remote backend only
- Wallet list management
- User preferences

The settings UI is implemented in `business-settings/src/ui.rs` but is currently
a placeholder pending full implementation.
