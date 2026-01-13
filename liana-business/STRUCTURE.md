# Liana Business Structure

This document provides an architectural overview of the liana-business project.

## Purpose

Liana Business is a wallet policy configuration application that connects to the Liana Business
service via WebSocket. It allows users to:

1. Authenticate via email/OTP
2. Select an organization
3. Select or create a wallet
4. Configure wallet policy templates (keys, spending paths, thresholds, timelocks)
5. Add xpub information from hardware wallets

## Architecture Overview

```
+------------------------------------------------------------------+
|                        liana-business                             |
|  (main.rs: type LianaBusiness = GUI<BI, BS, M>)                  |
+------------------------------------------------------------------+
                               |
              +----------------+----------------+
              |                                 |
              v                                 v
+---------------------------+     +---------------------------+
|    business-installer     |     |    business-settings      |
|    (Installer trait)      |     |    (SettingsTrait)        |
+---------------------------+     +---------------------------+
              |                                 |
              v                                 v
+------------------------------------------------------------------+
|                         liana-gui                                 |
|  GUI<I, S, M> framework: Pane, Tab, State management             |
+------------------------------------------------------------------+
              |
              v
+------------------------------------------------------------------+
|                         liana-ui                                  |
|  Theme, components, icons, fonts                                  |
+------------------------------------------------------------------+
```

## Crate Structure

```
liana-business/
├── src/
│   ├── main.rs              # Entry point, LianaBusiness type alias
│   └── lib.rs               # Re-exports
├── business-installer/      # Policy builder implementation
│   └── src/
│       ├── lib.rs           # Re-exports BusinessInstaller, Message
│       ├── installer.rs     # Implements Installer trait
│       ├── backend.rs       # Backend trait & Notification enum
│       ├── client.rs        # WebSocket client
│       ├── hw.rs            # Hardware wallet detection
│       ├── state/           # State management
│       │   ├── mod.rs       # State struct, View enum
│       │   ├── app.rs       # AppState (keys, paths, wallet data)
│       │   ├── message.rs   # Msg enum (all UI events)
│       │   ├── update.rs    # Message handlers
│       │   └── views/       # Per-view state
│       │       ├── mod.rs
│       │       ├── login/
│       │       ├── keys/
│       │       ├── path/
│       │       ├── xpub/
│       │       └── modals/
│       └── views/           # UI rendering
│           ├── mod.rs       # Layout helpers
│           ├── login/
│           ├── org_select.rs
│           ├── wallet_select.rs
│           ├── template_builder/
│           ├── keys/
│           ├── xpub/
│           └── modals/
└── business-settings/       # Settings implementation
    └── src/
        ├── lib.rs           # BusinessSettings, BusinessWalletSettings
        ├── ui.rs            # BusinessSettingsUI
        └── message.rs       # BusinessSettingsMessage
```

## How We Reuse liana-gui

### GUI Framework Integration

The main entry point creates a type alias that plugs our implementations into liana-gui's
generic `GUI<I, S, M>` framework:

```rust
// liana-business/src/main.rs
pub type LianaBusiness = GUI<BusinessInstaller, BusinessSettings, Message>;
```

This gives us:
- **Pane/Tab management** - Multi-pane, multi-tab window layout
- **State routing** - Launcher → Installer → Loader → App transitions
- **Window management** - Size persistence, Ctrl+C handling, panic hooks
- **Subscription infrastructure** - Unified event stream handling

### Skip Launcher Pattern

Unlike liana-gui (which shows a Launcher for network selection), liana-business starts
directly with the Installer because authentication is required first:

```rust
// business-installer/src/installer.rs
impl Installer for BusinessInstaller {
    fn skip_launcher() -> bool {
        true  // Start directly with Installer
    }
}
```

### Config and Initialization

We reuse `Config` from liana-gui but always provide a network (default: Signet):

```rust
let config = Config::new(datadir_path, Some(bitcoin::Network::Signet));
```

## Key Components

### BusinessInstaller (business-installer)

Implements `liana_gui::installer::Installer` trait:

```
+------------------+----------------------------------------------+
| Method           | Purpose                                      |
+------------------+----------------------------------------------+
| new()            | Create installer with datadir, network       |
| update()         | Handle all Msg variants                      |
| view()           | Render current view + modals                 |
| subscription()   | Stream backend notifications + HW events     |
| stop()           | Close backend connection                     |
| exit_maybe()     | Return NextState when user loads wallet      |
| skip_launcher()  | Returns true to bypass Launcher              |
+------------------+----------------------------------------------+
```

### BusinessSettings (business-settings)

Implements `liana_gui::app::settings::SettingsTrait`:

```
+--------------------------+-------------------------------------------+
| Type                     | Purpose                                   |
+--------------------------+-------------------------------------------+
| BusinessSettings         | Root settings (list of wallets)           |
| BusinessWalletSettings   | Per-wallet settings (NO bitcoind field)   |
| BusinessSettingsUI       | Settings panel (placeholder for now)      |
| BusinessSettingsMessage  | Settings UI messages                      |
+--------------------------+-------------------------------------------+
```

Key difference from LianaSettings: **No `start_internal_bitcoind` field** because
liana-business uses only Liana Connect (remote backend).

### State (business-installer/src/state/)

```rust
pub struct State {
    pub app: AppState,         // Domain data (keys, paths, selections)
    pub views: ViewsState,     // UI-specific state (modals, forms)
    pub backend: Client,       // WebSocket client
    pub hw: HwiService,        // Hardware wallet service
    pub current_view: View,    // Routing state
}
```

### AppState

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
    pub exit: bool,  // Triggers exit_maybe()
}
```

### View Enum

```rust
pub enum View {
    Login,        // Email/OTP authentication
    OrgSelect,    // Organization picker
    WalletSelect, // Wallet picker with status/role
    WalletEdit,   // Template overview
    Keys,         // Manage keys
    Xpub,         // Add xpub information
}
```

### Client (WebSocket)

Full WebSocket client for Liana Business service:

- Non-blocking message loop with `crossbeam::select!`
- Automatic ping/pong (60s interval, 30s timeout)
- Token caching in `connect.json`
- Request/response validation via `request_id`

### Hardware Wallet Support

Uses `async_hwi::Service` for device detection:

- Supported: Ledger, Trezor, BitBox02, Coldcard, Jade, Specter
- 2-second refresh interval
- Xpub fetching via `m/48'/coin'/account'/2'`

## Data Flow

```
User Action
    │
    ▼
Msg enum (state/message.rs)
    │
    ▼
State::update() (state/update.rs)
    │
    ├──▶ State mutation (app, views, current_view)
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
         Notification
              │
              ▼
         Msg::BackendNotif
              │
              ▼
         State::update() (cycle)
```

## Authentication Flow

```
App Start
    │
    ▼
Load connect.json & validate tokens
    │
    ├── No valid tokens ──▶ EmailEntry ──▶ CodeEntry ──▶ Connected
    │
    └── Valid tokens ──▶ AccountSelect ──▶ Click email ──▶ Connected
                              │
                              └── Click "new email" ──▶ EmailEntry
```

Token cache location: `~/.liana/<network>/connect.json`

## Dependencies

```
+----------------+--------------------------------------------------+
| Crate          | Purpose                                          |
+----------------+--------------------------------------------------+
| liana-gui      | GUI framework, Installer trait, Settings traits  |
| liana-ui       | Theme, components, icons                         |
| liana-connect  | Domain types (Key, Wallet, PolicyTemplate, etc.) |
| iced           | UI framework                                     |
| tungstenite    | WebSocket client                                 |
| crossbeam      | Thread communication                             |
| async-hwi      | Hardware wallet detection                        |
+----------------+--------------------------------------------------+
```

## Configuration

### Command-Line Arguments

```
liana-business [OPTIONS]

Options:
    --datadir <PATH>    Custom data directory
    --bitcoin           Use bitcoin network
    --testnet           Use testnet network
    --signet            Use signet network (default)
    --regtest           Use regtest network
    -v, --version       Show version
    -h, --help          Show help
```

### Environment Variables

- `LOG_LEVEL` - Logging verbosity (DEBUG, INFO, WARN, ERROR)
