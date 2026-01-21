# Registration Step Implementation Plan

## Overview

Implement a Registration step in WalletStatus flow where participants register the wallet descriptor on their hardware devices after validation.

## User Decisions

- Use `async-hwi::register_wallet` for device registration
- Proof of registration: HMAC (hex string) for Ledger only, None for others
- Users with no devices to register: show waiting screen
- New `View::Registration` variant (separate from WalletEdit)
- Modal uses ModalStep pattern (like xpub modal)

---

## Part 1: Protocol (liana-connect)

### File: `liana-connect/src/ws_business/models.rs`

1. **Make `RegistrationInfos` fields public** (lines 436-441):
```rust
pub struct RegistrationInfos {
    pub user: Uuid,
    pub fingerprint: Fingerprint,
    pub registered: bool,
    pub proof_of_registration: Option<String>,
}
```

2. **Add constructor**:
```rust
impl RegistrationInfos {
    pub fn new(user: Uuid, fingerprint: Fingerprint) -> Self {
        Self { user, fingerprint, registered: false, proof_of_registration: None }
    }
}
```

### File: `liana-connect/src/ws_business/protocol.rs`

1. **`DeviceRegistered` variant already updated** (line 277-280):
```rust
DeviceRegistered {
    wallet_id: Uuid,
    infos: RegistrationInfos,  // Contains user, fingerprint, registered, proof_of_registration
},
```

2. **Add method constant** (after line 300):
```rust
pub const METHOD_DEVICE_REGISTERED: &'static str = "device_registered";
```

3. **Add to `method()` match** (line 313):
```rust
Request::DeviceRegistered { .. } => Self::METHOD_DEVICE_REGISTERED,
```

4. **Add to `payload()` match** (line 331):
```rust
Request::DeviceRegistered { wallet_id, infos } =>
    Some(device_registered_payload(wallet_id, infos)),
```

5. **Add payload helper** and **parser function** for serialization/deserialization.

6. **Add to `from_ws_message` match** (around line 359):
```rust
Self::METHOD_DEVICE_REGISTERED => parse_device_registered_request(protocol_request.payload)?,
```

---

## Part 2: Server (liana-business-server)

### File: `liana-business-server/src/handler.rs`

1. **Add `DeviceRegistered` handler** in `handle_request`:
```rust
Request::DeviceRegistered { wallet_id, infos } => {
    handle_device_registered(state, wallet_id, infos, editor_id)
}
```

2. **Implement `handle_device_registered` function**:
   - Validate wallet is in `Registration(Pending)` status
   - Verify user owns this fingerprint (check `infos.user == editor_id` and `registered_devices[infos.fingerprint].user == editor_id`)
   - Update `registered_devices[infos.fingerprint]` with `infos` (registered=true, proof_of_registration)
   - If ALL devices registered, transition to `RegistrationStatus::Registered`
   - Return `Response::Wallet { wallet }`

3. **Note**: Transition from `Validated` to `Registration(Pending)` is assumed to already happen server-side when WalletManager validates. If not, add descriptor generation logic using internal keys' fingerprints.

---

## Part 3: Client (business-installer)

### 3.1 State Changes

#### File: `business-installer/src/state/mod.rs`
- Add `Registration` variant to `View` enum (line 39)
- Add `registration_view` to imports (line 5-8)
- Update `view()` method to handle `View::Registration`

#### File: `business-installer/src/state/views/mod.rs`
- Add `pub mod registration;`
- Add `registration: RegistrationViewState` to `ViewsState`

#### New File: `business-installer/src/state/views/registration.rs`
```rust
pub enum RegistrationModalStep {
    Registering,  // "Confirm on device..."
    Error,        // Show error with retry
}

pub struct RegistrationModalState {
    pub fingerprint: Fingerprint,
    pub device_kind: Option<DeviceKind>,
    pub step: RegistrationModalStep,
    pub error: Option<String>,
}

pub struct RegistrationViewState {
    pub descriptor: Option<String>,
    pub user_devices: Vec<(Fingerprint, RegistrationInfos)>,
    pub modal: Option<RegistrationModalState>,
}
```

### 3.2 Messages

#### File: `business-installer/src/state/message.rs`
Add:
```rust
RegistrationSelectDevice(Fingerprint),
RegistrationResult(Result<(Fingerprint, Option<[u8; 32]>), String>),
RegistrationCancelModal,
RegistrationRetry,
```

### 3.3 Update Handlers

#### File: `business-installer/src/state/update.rs`
Add handlers:
- `on_registration_select_device(fp)`: Open modal, start `hw.register_wallet()` via Task::perform
- `on_registration_result(result)`: On success, build `RegistrationInfos` and send `DeviceRegistered` request; on error, show in modal
- `on_registration_cancel_modal()`: Close modal
- Update wallet routing to go to `View::Registration` when status is `Registration(Pending)`

### 3.4 Backend

#### File: `business-installer/src/backend.rs` (trait)
Add: `fn device_registered(&mut self, wallet_id: Uuid, infos: RegistrationInfos);`

#### File: `business-installer/src/client.rs`
Implement `device_registered` to send `Request::DeviceRegistered { wallet_id, infos }` over WebSocket.

### 3.5 Views

#### New File: `business-installer/src/views/registration/mod.rs`
Main view logic:
- If `user_devices.is_empty()`: Show waiting screen
- If all user devices registered: Show "Your devices registered, waiting for others..."
- Else: Show device list with cards:
  - Match `state.hw.list()` by fingerprint
  - Connected + Supported: clickable card -> `Msg::RegistrationSelectDevice(fp)`
  - Connected + Locked/Unsupported: greyed card with status
  - Not connected: greyed card "Connect device to register"
  - Already registered: card with checkmark

#### New File: `business-installer/src/views/registration/modal.rs`
Modal following xpub modal pattern:
- `Registering` step: "Please confirm on your device..." with Cancel button
- `Error` step: Error message with Cancel and Retry buttons

#### File: `business-installer/src/views/modals/mod.rs`
Add registration modal to stacking:
```rust
.or_else(|| registration::modal::registration_modal_view(state))
```

#### File: `business-installer/src/views/mod.rs`
Add `pub mod registration;` and export `registration_view`

---

## Part 4: Documentation (liana-connect)

### File: `liana-connect/WSS_BUSINESS.md`

1. **Update Wallet Object status documentation** (around line 363-368):
Add `Registration` status with its sub-states:
```markdown
- `"Validated"`: Policy validated by owner, keys metadata not yet completed
- `{"Registration": {"Pending": {...}}}`: Descriptor generated, awaiting device registration
- `{"Registration": "Registered"}`: All devices have registered the descriptor
- `"Finalized"`: All key metadata filled, ready for production
```

2. **Add RegistrationInfos Object** (new section after Xpub Object):
```markdown
### RegistrationInfos Object

Used in `device_registered` request payloads and in wallet registration status:

```json
{
  "user": "<uuid>",
  "fingerprint": "<fingerprint>",
  "registered": <boolean>,
  "proof_of_registration": "<string>" | null
}
```

**Note:** The `proof_of_registration` field contains the HMAC (hex-encoded) for Ledger devices, or `null` for other device types.
```

3. **Add `device_registered` message type** (new section in Wallet Management):
```markdown
#### `device_registered`
Report that a device has registered the wallet descriptor.

**Request:**
```json
{
  "type": "device_registered",
  "token": "<auth_token>",
  "request_id": "<uuid>",
  "payload": {
    "wallet_id": "<uuid>",
    "infos": <RegistrationInfos>
  }
}
```

**Response:** [`wallet`](#wallet-notification)

**Maps to:** `Response::Wallet { wallet: Wallet }`

**Note:** The `infos.registered` field should be `true`. The `infos.proof_of_registration`
field should contain the HMAC (hex-encoded) for Ledger devices, or `null` for other devices.
```

4. **Add Device Registration Flow** (new section in Example Message Flows):

### Device Registration Flow

After owner validates the wallet, the server transitions the wallet to `Registration(Pending)`
status with a generated descriptor. Each participant with Internal keys must register the
descriptor on their hardware devices.

**Actor:** Participant with Internal keys

**Precondition:** Wallet is in `Registration(Pending)` status, user has devices to register

```
// Step 1: Receive wallet with Registration status (after validation)
Server -> Client: Response::Wallet {
    wallet: Wallet {
        id: "wallet-uuid-001",
        status: WalletStatus::Registration(RegistrationStatus::Pending {
            descriptor: "wsh(or_d(multi(2,[d34db33f/48'/0'/0'/2']xpub.../0/*,...),and_v(...)))",
            registered_devices: {
                "d34db33f": RegistrationInfos {
                    user: "user-uuid-001",
                    fingerprint: "d34db33f",
                    registered: false,
                    proof_of_registration: None,
                },
                "cafebabe": RegistrationInfos {
                    user: "user-uuid-002",
                    fingerprint: "cafebabe",
                    registered: false,
                    proof_of_registration: None,
                },
            },
        }),
        ...
    }
}

// Step 2: User connects hardware device (fingerprint: d34db33f)
// Client detects device via HwiService, matches fingerprint to registered_devices

// Step 3: User initiates registration on device
// Client calls async-hwi register_wallet() which prompts user to confirm on device

// Step 4: After successful registration, client reports to server
Client -> Server: Request::DeviceRegistered {
    wallet_id: "wallet-uuid-001",
    infos: RegistrationInfos {
        user: "user-uuid-001",
        fingerprint: "d34db33f",
        registered: true,
        proof_of_registration: Some("a1b2c3d4..."),  // HMAC for Ledger, None for others
    },
}

// Step 5: Server updates registration status and responds
Server -> Client: Response::Wallet {
    wallet: Wallet {
        id: "wallet-uuid-001",
        status: WalletStatus::Registration(RegistrationStatus::Pending {
            descriptor: "wsh(...)",
            registered_devices: {
                "d34db33f": RegistrationInfos {
                    user: "user-uuid-001",
                    fingerprint: "d34db33f",
                    registered: true,  // Now registered
                    proof_of_registration: Some("a1b2c3d4..."),
                },
                "cafebabe": RegistrationInfos {
                    user: "user-uuid-002",
                    fingerprint: "cafebabe",
                    registered: false,  // Still waiting
                    proof_of_registration: None,
                },
            },
        }),
        ...
    }
}

// Step 6: When ALL devices are registered, server transitions to Registered
// (After user-uuid-002 also registers their device)
Server -> Client: Response::Wallet {
    wallet: Wallet {
        id: "wallet-uuid-001",
        status: WalletStatus::Registration(RegistrationStatus::Registered),
        ...
    }
}
```

5. **Update Request/Response Enum Reference** (Appendix):
Add `DeviceRegistered` to Request enum:
```rust
pub enum Request {
    // ... existing variants ...
    DeviceRegistered {
        wallet_id: Uuid,
        infos: RegistrationInfos,
    },
}
```

---

## Critical Files Summary

| Component | File | Changes |
|-----------|------|---------|
| Protocol | `liana-connect/src/ws_business/models.rs` | Make RegistrationInfos public, add constructor |
| Protocol | `liana-connect/src/ws_business/protocol.rs` | Complete DeviceRegistered handling |
| Docs | `liana-connect/WSS_BUSINESS.md` | Add Registration status, device_registered message, flow example |
| Server | `liana-business-server/src/handler.rs` | Add handle_device_registered |
| Client | `business-installer/src/state/mod.rs` | Add View::Registration |
| Client | `business-installer/src/state/views/registration.rs` | NEW: View state |
| Client | `business-installer/src/state/message.rs` | Add Registration messages |
| Client | `business-installer/src/state/update.rs` | Add Registration handlers |
| Client | `business-installer/src/backend.rs` | Add device_registered trait method |
| Client | `business-installer/src/client.rs` | Implement device_registered |
| Client | `business-installer/src/views/registration/mod.rs` | NEW: Main view |
| Client | `business-installer/src/views/registration/modal.rs` | NEW: Modal view |
