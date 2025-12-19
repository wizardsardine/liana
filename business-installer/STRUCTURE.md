# Business Installer Crate Structure

This document provides an architectural overview of the `business-installer` crate for LLM agents
working on this codebase.

## Purpose

The `business-installer` crate implements a wallet policy configuration wizard that connects to the
Liana Business service via WebSocket. It allows users to:

1. Authenticate via email/OTP
2. Select an organization
3. Select or create a wallet
4. Configure wallet policy templates (keys, spending paths, thresholds, timelocks)

The crate implements the `Installer` trait from `liana-gui`, enabling integration as an installer
component within the main Liana GUI application.

## Architecture Overview

```
+----------------------------------+
|        BusinessInstaller         |  <-- Entry point, implements liana_gui::Installer trait
+----------------+-----------------+
                 |
                 v
+----------------+-----------------+
|             State                |  <-- Main state container
|  +----------+  +---------------+ |
|  | AppState |  | ViewsState    | |
|  +----------+  +---------------+ |
|       |              |           |
|       v              v           |
|  [Keys, Paths]  [Login, Modals]  |
+----------------+-----------------+
                 |
                 v
+----------------+-----------------+
|        Backend (Client)          |  <-- WebSocket communication layer
+----------------------------------+
```

## File Structure

```
src/
+-- lib.rs              # Crate entry point, re-exports public API
+-- installer.rs        # BusinessInstaller struct, implements Installer trait
+-- backend.rs          # Backend trait & DevBackend (test implementation)
+-- client.rs           # WSS Client, DummyServer for testing
|
+-- state/
|   +-- mod.rs          # State struct, View enum, routing logic
|   +-- app.rs          # AppState (keys, paths, wallet data)
|   +-- message.rs      # Msg enum (all UI events/actions)
|   +-- update.rs       # Message handlers (State::update)
|   +-- views/          # Per-view state
|       +-- mod.rs      # ViewsState container
|       +-- home.rs     # HomeViewState
|       +-- login/      # Login states (email, code)
|       +-- keys/       # KeysViewState, EditKeyModalState
|       +-- path/       # PathsViewState, EditPathModalState
|       +-- modals/     # WarningModalState
|       +-- org_select.rs
|       +-- wallet_select.rs
|
+-- views/              # UI rendering functions
    +-- mod.rs          # layout(), menu_entry() helpers
    +-- home/           # Wallet edit view
    +-- login/          # Email + code entry views
    +-- keys/           # Key management views
    +-- paths/          # Path configuration views
    +-- modals/         # Modal rendering
    +-- org_select.rs   # Organization selection
    +-- wallet_select.rs# Wallet selection
```

## Key Modules

### `lib.rs`

Entry point. Re-exports:
- `BusinessInstaller` - main public type
- `Message` (aliased as `Msg`) - for external message handling

### `installer.rs`

Implements `liana_gui::installer::Installer` trait:

```
+----------------------------------------------------------+
| Installer Trait Methods                                  |
+----------------------------------------------------------+
| new()         -> Creates installer with datadir/network  |
| update()      -> Handles all Msg variants                |
| subscription()-> Stream of BackendNotifications          |
| view()        -> Renders current view + modals           |
| stop()        -> Closes backend connection               |
| exit_maybe()  -> Returns NextState (not implemented)     |
+----------------------------------------------------------+
```

Uses a `BackendSubscription` (implements `iced::futures::Stream`) to receive notifications from the
WebSocket backend and convert them to `Message::BackendNotif`.

### `backend.rs`

Defines:

```
+------------------+     +-------------------+
|  Backend trait   |     |   DevBackend      |
+------------------+     +-------------------+
| auth_request()   |     | Uses DummyServer  |
| auth_code()      |     | for local testing |
| connect_ws()     |     +-------------------+
| ping/close()     |
| fetch_org/user/wallet() |
| create/edit_wallet()    |
+------------------+
```

Key types re-exported from `liana-connect`:
- `Org`, `OrgData`, `User`, `Wallet`
- `Key`, `KeyType`
- `WalletStatus`, `UserRole`

Notifications:
- `Connected`, `Disconnected`
- `AuthCodeSent`, `InvalidEmail`, `AuthCodeFail`
- `LoginSuccess`, `LoginFail`
- `Org(Uuid)`, `Wallet(Uuid)`, `User(Uuid)`
- `Error(Error)`

### `client.rs`

Full WebSocket client implementation:

```
+-----------------+          +-----------------+
|     Client      |   WSS    |   DummyServer   |
+-----------------+  <--->   +-----------------+
| orgs, wallets,  |          | For debug mode  |
| users (cached)  |          | url == "debug"  |
| token           |          +-----------------+
| auth_client     |
+-----------------+
```

Features:
- Non-blocking message loop with `crossbeam::select!`
- Automatic ping/pong (60s interval, 30s timeout)
- Token refresh from cache
- Request/response validation via `request_id`
- Debug mode: spawns `DummyServer` on localhost

### `state/mod.rs`

Main state orchestration:

```rust
pub struct State {
    pub app: AppState,         // Domain data
    pub views: ViewsState,     // UI-specific state
    pub backend: Client,       // WebSocket client
    pub current_view: View,    // Routing state
}
```

View enum:
```rust
enum View {
    Login,        // Email/code authentication
    OrgSelect,    // Pick organization
    WalletSelect, // Pick or create wallet
    WalletEdit,   // Main home view (template overview)
    Paths,        // Configure spending paths
    Keys,         // Manage keys
}
```

Key methods:
- `route()` - Determines actual view based on auth state
- `view()` - Renders current view with modal overlay
- `is_template_valid()` - Validates policy template

### `state/app.rs`

Domain state:

```rust
pub struct AppState {
    pub keys: BTreeMap<u8, Key>,
    pub primary_path: SpendingPath,
    pub secondary_paths: Vec<(SpendingPath, Timelock)>,
    pub next_key_id: u8,
    pub selected_org: Option<Uuid>,
    pub selected_wallet: Option<Uuid>,
    pub current_wallet_template: Option<PolicyTemplate>,
    pub reconnecting: bool,
}
```

Implements bidirectional conversion with `PolicyTemplate`:
- `From<AppState> for PolicyTemplate`
- `From<PolicyTemplate> for AppState`

### `state/message.rs`

All application messages:

```
+--------------------------+
| Message Categories       |
+--------------------------+
| Login/Auth               |
|   LoginUpdateEmail       |
|   LoginUpdateCode        |
|   LoginSendToken         |
|   LoginSendAuthCode      |
+--------------------------+
| Org Management           |
|   OrgSelected(Uuid)      |
|   OrgWalletSelected(Uuid)|
|   OrgCreateNewWallet     |
+--------------------------+
| Key Management           |
|   KeyAdd/Edit/Delete     |
|   KeySave/CancelModal    |
|   KeyUpdate*             |
+--------------------------+
| Template Management      |
|   TemplateAddKey*        |
|   TemplateDelKey*        |
|   TemplateAdd/DeletePath |
|   TemplateEditPath       |
|   TemplateSavePath       |
|   TemplateValidate       |
+--------------------------+
| Navigation               |
|   NavigateTo*            |
|   NavigateBack           |
+--------------------------+
| Backend                  |
|   BackendNotif           |
|   BackendDisconnected    |
+--------------------------+
| Warnings                 |
|   WarningShowModal       |
|   WarningCloseModal      |
+--------------------------+
```

### `state/update.rs`

Message handler implementations. Organized by category:
- Login/Auth handlers
- Org management handlers
- Key management handlers
- Template management handlers
- Navigation handlers
- Backend notification handlers
- Warning handlers

### `state/views/`

View-specific state containers:

```
ViewsState
+-- modals: ModalsState
|   +-- warning: Option<WarningModalState>
+-- keys: KeysViewState
|   +-- edit_key: Option<EditKeyModalState>
+-- paths: PathsViewState
|   +-- edit_path: Option<EditPathModalState>
+-- login: Login
    +-- current: LoginState (EmailEntry|CodeEntry|Authenticated)
    +-- email: EmailState
    +-- code: CodeState
```

### `views/`

UI rendering functions. Each module exports a main view function:
- `login_view(state)` - Authentication flow
- `org_select_view(state)` - Organization picker
- `wallet_select_view(state)` - Wallet picker
- `home_view(state)` - Template overview
- `paths_view(state)` - Path configuration
- `keys_view(state)` - Key management
- `modals::render_modals(state)` - Modal overlay

Common helpers in `views/mod.rs`:
- `layout()` - Standard page layout with header, progress, content
- `menu_entry()` - Clickable card component

## Data Flow

```
User Action
    |
    v
Message (Msg enum)
    |
    v
State::update()  -----> Backend call (if needed)
    |                         |
    v                         v
State mutation          WebSocket Request
    |                         |
    v                         v
State::view()           WSS Response
    |                         |
    v                         v
Element<Msg>            Notification
                              |
                              v
                        BackendSubscription
                              |
                              v
                        Message::BackendNotif
                              |
                              v
                        State::update() (cycle)
```

## Dependencies

Key external crates:
- `iced` - UI framework (with wgpu renderer)
- `liana-gui` - Parent crate, provides `Installer` trait
- `liana-ui` - UI components and theming
- `liana-connect` - Domain types (`Key`, `Wallet`, `PolicyTemplate`, etc.)
- `tungstenite` - WebSocket client
- `crossbeam` - Channels for thread communication

## Authentication Flow

On startup, the application checks for cached tokens in `connect.json` (same location as liana-gui).
If valid cached accounts exist, the user can select one to connect directly without re-authenticating.

```
App Start
    │
    ▼
Initialize Client with network_dir
    │
    ▼
Load connect.json & validate tokens
    │
    ├─── No valid tokens ───▶ EmailEntry view (OTP flow)
    │                              │
    │                              ▼
    │                         User enters email
    │                              │
    │                              ▼
    │                         CodeEntry view
    │                              │
    │                              ▼
    │                         User enters OTP code
    │                              │
    │                              ▼
    │                         Token cached & connected
    │                              │
    │                              └───────────────────────┐
    │                                                      │
    └─── Valid tokens ───▶ AccountSelect view              │
                              │                            │
                              ├─── Click email card ───▶ Set token & connect_ws()
                              │                            │
                              │                            ├─── Connected ───▶ OrgSelect
                              │                            │
                              │                            └─── Error ───▶ Warning modal
                              │                                              │
                              │                                              ▼
                              │                                        Clear token from cache
                              │                                              │
                              │                                              ▼
                              │                                        Re-validate tokens
                              │                                              │
                              │                                    ┌─────────┴─────────┐
                              │                                    │                   │
                              │                              Has valid          No valid
                              │                              tokens             tokens
                              │                                    │                   │
                              │                                    ▼                   ▼
                              │                              AccountSelect      EmailEntry
                              │
                              └─── Click "new email" ───▶ EmailEntry view
```

Token cache location:
- **Integrated mode** (via liana-gui): `~/.liana/<network>/connect.json`
- **Standalone mode** (liana-business binary): `~/.liana/signet/connect.json`

## Debug Mode

When `BACKEND_URL == "debug"`:
1. `Client::connect_ws()` spawns a local `DummyServer`
2. Test data is pre-populated via `init_client_with_test_data()`
3. Auth accepts code `"123456"` without network calls
4. Useful for UI development without a running server
