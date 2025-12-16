# Business Installer Application Flow

This document describes the data flow, state management, and patterns for adding new features
to the `business-installer` crate.

## State Management

```
State (state/mod.rs)
+-- app: AppState        # Domain data (keys, paths, wallet template)
+-- views: ViewsState    # UI-specific state per view
+-- backend: Client      # WebSocket communication
+-- current_view: View   # Routing state
```

### State Hierarchy

- **State** - Root container in `state/mod.rs`
- **AppState** - Domain data: keys, primary_path, secondary_paths, selected_org/wallet
- **ViewsState** - Per-view UI state (modals, form fields, edit states)
- **Client** - WebSocket connection and caches (orgs, wallets, users)

### AppState Fields

```rust
pub struct AppState {
    pub keys: BTreeMap<u8, Key>,              // All defined keys
    pub primary_path: SpendingPath,           // Primary spending path
    pub secondary_paths: Vec<(SpendingPath, Timelock)>,  // Recovery paths
    pub next_key_id: u8,                      // Auto-increment for new keys
    pub selected_org: Option<Uuid>,           // Currently selected org
    pub selected_wallet: Option<Uuid>,        // Currently selected wallet
    pub current_wallet_template: Option<PolicyTemplate>,
    pub reconnecting: bool,                   // Flag for intentional reconnection
}
```

### ViewsState Structure

```rust
pub struct ViewsState {
    pub modals: ModalsState,      // Warning modals
    pub keys: KeysViewState,      // Key edit modal state
    pub paths: PathsViewState,    // Path edit modal state
    pub login: Login,             // Login form state (email, code, auth status)
}
```

## Message Flow

```
User Action
    |
    v
Msg enum (state/message.rs)
    |
    v
State::update() (state/update.rs)
    |
    +-------> State mutation (self.app, self.views, self.current_view)
    |
    +-------> Backend call (self.backend.*)
                  |
                  v
             WebSocket Request
                  |
                  v
             WSS Response
                  |
                  v
             Notification (backend.rs)
                  |
                  v
             BackendSubscription (installer.rs)
                  |
                  v
             Msg::BackendNotif(Notification)
                  |
                  v
             State::update() -> State::on_backend_notif()
```

### Message Categories

```
+----------------------------+----------------------------------------+
| Category                   | Messages                               |
+----------------------------+----------------------------------------+
| Login/Auth                 | LoginUpdateEmail, LoginUpdateCode,     |
|                            | LoginSendToken, LoginSendAuthCode      |
+----------------------------+----------------------------------------+
| Org Management             | OrgSelected, OrgWalletSelected,        |
|                            | OrgCreateNewWallet                     |
+----------------------------+----------------------------------------+
| Key Management             | KeyAdd, KeyEdit, KeyDelete, KeySave,   |
|                            | KeyCancelModal, KeyUpdate*             |
+----------------------------+----------------------------------------+
| Template Management        | TemplateAddKey*, TemplateDelKey*,      |
|                            | TemplateAdd/DeletePath, TemplateEdit*, |
|                            | TemplateSavePath, TemplateValidate     |
+----------------------------+----------------------------------------+
| Navigation                 | NavigateTo*, NavigateBack              |
+----------------------------+----------------------------------------+
| Backend                    | BackendNotif, BackendDisconnected      |
+----------------------------+----------------------------------------+
| Warnings                   | WarningShowModal, WarningCloseModal    |
+----------------------------+----------------------------------------+
```

### Backend Notifications

```
+-------------------+------------------------------------------+
| Notification      | Triggered By                             |
+-------------------+------------------------------------------+
| Connected         | Successful WebSocket connection          |
| Disconnected      | Connection lost or closed                |
| AuthCodeSent      | OTP sent to email                        |
| InvalidEmail      | Email validation failed                  |
| AuthCodeFail      | Failed to send OTP                       |
| LoginSuccess      | OTP verified successfully                |
| LoginFail         | OTP verification failed                  |
| Org(Uuid)         | Org data received/updated                |
| Wallet(Uuid)      | Wallet data received/updated             |
| User(Uuid)        | User data received/updated               |
| Error(Error)      | Backend error occurred                   |
+-------------------+------------------------------------------+
```

## View Routing

Views are determined by `State::current_view` and `State::route()`:

```
+---------------+------------------+------------------------------------+
| View          | Entry Point      | Purpose                            |
+---------------+------------------+------------------------------------+
| Login         | Initial view     | Email/OTP authentication           |
| OrgSelect     | After auth       | Organization picker                |
| WalletSelect  | After org select | Wallet picker or create new        |
| WalletEdit    | After wallet     | Template overview (home)           |
| Paths         | From home        | Configure spending paths           |
| Keys          | From home        | Manage keys                        |
+---------------+------------------+------------------------------------+
```

### Navigation Flow

```
Login (EmailEntry)
    |
    v [LoginSendToken]
Login (CodeEntry)
    |
    v [LoginSuccess]
OrgSelect
    |
    v [OrgSelected]
WalletSelect
    |
    +-------> [OrgWalletSelected] --> WalletEdit
    |
    +-------> [OrgCreateNewWallet] --> WalletEdit

WalletEdit <--> Paths [NavigateToPaths / NavigateToHome]
WalletEdit <--> Keys  [NavigateToKeys / NavigateToHome]
```

## Adding New Features

### New View Checklist

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
       // ... existing fields
       pub new_view: NewViewState,
   }
   ```

3. **Add View variant** in `state/mod.rs`:
   ```rust
   pub enum View {
       // ... existing variants
       NewView,
   }
   ```

4. **Create render function** in `views/new_view.rs`:
   ```rust
   pub fn new_view(state: &State) -> Element<'_, Message> {
       layout(
           (step, total),
           email,
           "Title",
           content,
           padding_left,
           previous_message,
       )
   }
   ```

5. **Export from views** in `views/mod.rs`:
   ```rust
   pub mod new_view;
   pub use new_view::new_view;
   ```

6. **Add routing** in `State::view()` in `state/mod.rs`:
   ```rust
   View::NewView => new_view(self),
   ```

7. **Add navigation messages** if needed in `state/message.rs`

### New Message Checklist

1. **Add variant** to `Msg` enum in `state/message.rs`:
   ```rust
   pub enum Msg {
       // ... existing variants
       NewAction(SomeData),
   }
   ```

2. **Add handler** in `State::update()` in `state/update.rs`:
   ```rust
   Msg::NewAction(data) => self.on_new_action(data),
   ```

3. **Implement handler** on `State`:
   ```rust
   impl State {
       fn on_new_action(&mut self, data: SomeData) {
           // Mutate state
       }
   }
   ```

4. **Wire up in view**:
   ```rust
   button::primary("Action")
       .on_press(Message::NewAction(data))
   ```

### New Backend Request Checklist

1. **Add trait method** in `backend.rs`:
   ```rust
   pub trait Backend {
       // ... existing methods
       fn new_request(&mut self, param: Type);
   }
   ```

2. **Implement in Client** in `client.rs`:
   ```rust
   impl Backend for Client {
       fn new_request(&mut self, param: Type) {
           check_connection!(self);
           if let Some(sender) = &self.request_sender {
               let _ = sender.send(Request::NewRequest { param });
           }
       }
   }
   ```

3. **Add Notification variant** if new response type in `backend.rs`:
   ```rust
   pub enum Notification {
       // ... existing variants
       NewResponse(ResponseData),
   }
   ```

4. **Handle notification** in `State::on_backend_notif()`:
   ```rust
   Notification::NewResponse(data) => self.on_backend_new_response(data),
   ```

5. **Update DummyServer** handler in `client.rs` for testing:
   ```rust
   Request::NewRequest { param } => handle_new_request(param),
   ```

## Modal Pattern

### Opening a Modal

```rust
// In message handler
fn on_key_edit(&mut self, key_id: u8) {
    if let Some(key) = self.app.keys.get(&key_id) {
        self.views.keys.edit_key = Some(EditKeyModalState {
            key_id,
            alias: key.alias.clone(),
            // ... other fields
        });
    }
}
```

### Rendering Modals

```rust
// In views/modals/mod.rs
pub fn render_modals(state: &State) -> Option<Element<'_, Message>> {
    // Warning modal has priority (rendered on top)
    if let Some(warning) = &state.views.modals.warning {
        return Some(warning_modal(warning));
    }
    // Then other modals
    if let Some(edit_key) = &state.views.keys.edit_key {
        return Some(key_modal(edit_key, &state.app.keys));
    }
    None
}
```

### Closing a Modal

```rust
fn on_key_cancel_modal(&mut self) {
    self.views.keys.edit_key = None;
}
```

## Testing Patterns

### Debug Mode

When `BACKEND_URL == "debug"`:
- `Client::connect_ws()` spawns local `DummyServer`
- Test data pre-populated via `init_client_with_test_data()`
- Auth accepts code `"123456"` without network

### Integration Test Template

```rust
#[test]
fn test_feature() {
    let port = 30XXX;  // Unique port
    let mut server = DummyServer::new(port);

    let handler: Box<dyn Fn(Request) -> Response + Send + Sync> =
        Box::new(|req| match req {
            Request::YourRequest { .. } => Response::YourResponse { /* */ },
            _ => Response::Pong,
        });

    server.start(handler);
    thread::sleep(Duration::from_millis(200));

    let mut client = Client::new();
    client.set_token("test-token".to_string());
    let receiver = client.connect_ws(format!("ws://127.0.0.1:{}", port), 1)
        .expect("Should return receiver");

    // Wait for connection
    thread::sleep(Duration::from_millis(500));

    // Test interactions...

    client.close();
    server.close();
}
```

