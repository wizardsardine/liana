# Breez Liquid SDK USDt Development Plan

## Goal

Enable USDt (Liquid) support in the Rust app using Breez Liquid SDK, with a clean user experience for:

- viewing USDt balances
- receiving USDt on Liquid
- sending USDt on Liquid
- previewing fees clearly
- laying the groundwork for future cross-asset payments

This plan is written to be directly useful inside Windsurf for implementation planning and execution.

---

## Product Scope

### In scope for v1
- Detect and display USDt balances from Breez Liquid SDK
- Add USDt asset constants for mainnet and testnet
- Add a Receive USDt flow
- Add a Send USDt flow
- Add fee preview before send
- Add transaction history support for USDt payments
- Add validation and error handling for common failure cases
- Test end-to-end on testnet

### Out of scope for v1
- General support for arbitrary Liquid assets
- Full conversion UX between BTC and USDt
- Advanced portfolio/accounting views
- On-chain swap orchestration outside Breez SDK
- Multi-wallet asset routing logic

### Nice-to-have after v1
- Cross-asset payments using `from_asset`
- Unified "Pay with any balance" flow
- Asset-specific filtering in history
- Better amount formatting, localization, and fiat display
- Deeper telemetry/logging around asset payment failures

---

## Key Facts / Assumptions

- Breez Liquid SDK supports USDt on Liquid.
- USDt metadata is NOT automatically available — `asset_metadata: None` is set in `BreezConfig::sdk_config()` and must be populated for display names/tickers/precision to appear.
- Mainnet and **Regtest** use different Liquid asset IDs. **Testnet is not supported** by the existing Breez integration (`load_breez_client` returns `NetworkNotSupported` for Testnet/Testnet4/Signet).
- USDt precision is 8 decimals (amounts in SDK are `u64` base units; display as `amount / 10^8`).
- The current app already has Breez Liquid SDK working for Liquid wallet functionality (`src/app/breez/`, `src/app/state/liquid/`, `src/app/view/liquid/`).
- `BreezClient::info()` already fetches `wallet_info` — USDt balances come from `wallet_info.asset_balances` in the same response, no new API call needed.
- `BreezClient::list_payments()` already exists and returns all payments including future USDt ones — filtering is the only new work.
- `BreezClient::receive_liquid()` exists for L-BTC but USDt receive needs a separate method using `ReceiveAmount::Asset`.
- This work should be integrated in a way that keeps asset-specific logic out of the UI where possible.

---

## Asset Constants

Use strongly defined constants and keep them in a single module.

```rust
pub const LBTC_ASSET_ID_MAINNET: &str =
    "6f0279e9ed041c3d710a9f57d0c02928416460c4b722ae3457a11eec381c526d";

pub const USDT_ASSET_ID_MAINNET: &str =
    "ce091c998b83c78bb71a632313ba3760f1763d9cfcffae02258ffa9865a37bd2";

pub const LBTC_ASSET_ID_TESTNET: &str =
    "144c654344aa716d6f3abcc1ca90e5641e4e2a7f633bc09fe3baf64585819a49";

pub const USDT_ASSET_ID_TESTNET: &str =
    "b612eb46313a2cd6ebabd8b7a8eed5696e29898b87a43bff41c94f51acef9d73";
```

---

## Recommended Architecture

Create or extend the following layers:

### 1. Asset constants module
File:
- `src/app/breez/assets.rs` *(new)*

Responsibilities:
- define asset IDs by network (Mainnet and Regtest only)
- define ticker/name helpers
- define precision helpers (USDt = 8 decimals)
- expose helper to resolve USDt asset ID based on active network

### 2. BreezClient extensions
File:
- `src/app/breez/client.rs` *(extend existing)*

Add methods directly to `BreezClient` — no separate adapter layer is needed since `BreezClient` already wraps the SDK:
- `receive_usdt(amount: Option<u64>, asset_id: &str)` — new; uses `ReceiveAmount::Asset`
- `prepare_send_usdt(destination, amount: u64, asset_id: &str)` — new; uses `PayAmount::Asset`
- Asset balance extraction is done from the existing `info()` response
- Payment filtering is done on the existing `list_payments()` response

### 3. Domain types
File:
- `src/app/breez/types.rs` *(new, or inline with state)*

Suggested types (amounts are `u64`, not `f64`):
- `AssetKind`
- `AssetBalanceView`
- `ReceiveRequestView`
- `PreparedSendQuote`
- `AssetPaymentRecord`
- `AssetFeePreview`

### 4. UI layer
The app uses a consistent pattern — extend existing files:
- **Balance:** extend `src/app/state/liquid/overview.rs` + `src/app/view/liquid/overview.rs`
- **Receive:** extend `src/app/state/liquid/receive.rs` + `src/app/view/liquid/receive.rs`
- **Send:** extend `src/app/state/liquid/send.rs` + `src/app/view/liquid/send.rs`
- **History:** extend `src/app/state/liquid/transactions.rs` + `src/app/view/liquid/transactions.rs`
- **Messages:** add variants to existing enums in `src/app/view/message.rs`

Responsibilities:
- present balances cleanly
- allow choosing Bitcoin or USDt
- show fee previews and validation errors
- avoid using raw asset IDs directly

---

## Milestone Plan

## Milestone 1: Foundation and asset plumbing

### Objective
Add the basic asset abstractions and constants needed for USDt support.

### Tasks
- [ ] Create `src/app/breez/assets.rs` with Liquid asset constants (Mainnet + Regtest)
- [ ] Add helper to resolve USDt asset ID by network
- [ ] Add helper to resolve L-BTC asset ID by network
- [ ] Add asset metadata mapping helpers:
  - ticker
  - display name
  - precision
- [ ] Add domain enum for supported assets:
  - Bitcoin
  - USDt
- [ ] Add formatting helper for 8-decimal asset amounts (`u64` base units → display string)
- [ ] **Populate `asset_metadata` in `BreezConfig::sdk_config()`** with USDt name/ticker/precision so the SDK returns labeled balances
- [ ] Add tests for asset ID/network mapping

### Deliverable
A reusable asset module that can be referenced everywhere else, and the SDK config correctly labels USDt.

---

## Milestone 2: Balance retrieval and wallet overview

### Objective
Show USDt balance in the wallet using Breez SDK wallet info.

### Tasks
- [ ] `LiquidOverview::load_balance()` already calls `breez_client.info()` — extend it to also read `wallet_info.asset_balances` (no new SDK call needed)
- [ ] Extract USDt balance from the existing `GetInfoResponse` and add `usdt_balance: u64` field to `LiquidOverview`
- [ ] Extract and normalize:
  - asset ID (match against `USDT_ASSET_ID_*` constant)
  - balance (`u64` base units)
  - ticker / precision (from `asset_metadata` or constants fallback)
- [ ] Add USDt balance display to `src/app/view/liquid/overview.rs`
- [ ] Ensure L-BTC balance display is unchanged
- [ ] Handle missing USDt entry in `asset_balances` as zero
- [ ] Add loading and failure states

### Edge cases
- [ ] USDt entry absent from `asset_balances`
- [ ] unknown asset IDs returned by SDK
- [ ] network mismatch assumptions
- [ ] stale UI state after reconnect/reload

### Deliverable
Wallet screen displays USDt alongside Bitcoin.

---

## Milestone 3: Receive USDt flow

### Objective
Allow the user to generate a Liquid receive request for USDt.

### Tasks
- [ ] Add "Receive USDt" action in wallet UI (extend `src/app/view/liquid/receive.rs`)
- [ ] Add amount input with:
  - optional amount
  - 8 decimal precision validation
- [ ] Support fixed-amount and amountless receive requests
- [ ] Add `receive_usdt(amount: Option<u64>, asset_id: &str)` to `BreezClient` (note: existing `receive_liquid()` is L-BTC only and uses `amount: None` — a separate method is needed)
- [ ] Use:
  - `PaymentMethod::LiquidAddress`
  - `ReceiveAmount::Asset { asset_id, receiver_amount_sat }`
- [ ] Return app-friendly receive model containing:
  - destination
  - QR/URI payload
  - amount
  - asset ticker
- [ ] Display generated destination in UI
- [ ] Add copy/share actions
- [ ] Add QR rendering if not already present

### Validation
- [ ] Reject negative or zero fixed amounts where appropriate
- [ ] Enforce max decimal precision of 8
- [ ] Clearly label the request as USDt on Liquid
- [ ] Show correct network indicator

### Deliverable
User can request USDt on Liquid and share the resulting address/URI.

---

## Milestone 4: Send USDt flow

### Objective
Allow the user to send USDt to a Liquid address or BIP21 URI.

### Tasks
- [ ] Add "Send USDt" action in wallet UI
- [ ] Add destination input field
- [ ] Add amount input field
- [ ] Detect whether destination is:
  - raw Liquid address
  - BIP21 URI
- [ ] Implement Breez wrapper for:
  - `prepare_send_payment`
  - `send_payment`
- [ ] Use `PayAmount::Asset`
- [ ] Set `to_asset` to USDt asset ID
- [ ] Set `receiver_amount`
- [ ] Support `estimate_asset_fees`
- [ ] Build send confirmation screen from prepare response
- [ ] Execute send only after explicit confirmation
- [ ] Return payment result to UI and persist/update history

### Validation
- [ ] Require amount for raw Liquid address sends
- [ ] Handle amount precedence correctly for BIP21 URIs
- [ ] Reject malformed destinations
- [ ] Reject unsupported network destinations
- [ ] Reject insufficient balance cleanly
- [ ] Handle insufficient fee balance cleanly

### Deliverable
User can send USDt with a review/confirm step.

---

## Milestone 5: Fee handling and UX polish

### Objective
Make asset fee behavior understandable and safe.

### Tasks
- [ ] Parse fee info from prepare response:
  - `estimated_asset_fees`
  - fallback `fees_sat`
- [ ] Add app model for fee preview
- [ ] Show whether fee is charged in:
  - USDt
  - sats
- [ ] Decide default behavior for `use_asset_fees`
- [ ] Add toggle only if needed; otherwise use sensible defaults
- [ ] Ensure total spend is clearly shown before confirmation

### Recommended v1 behavior
- Prefer simple UX
- If asset fees are available and sufficient, use them
- Otherwise fall back to sats
- Always show the user what will happen before they confirm

### Deliverable
Fee behavior is visible and understandable before sending.

---

## Milestone 6: Payment history support

### Objective
Expose USDt transactions in wallet history.

### Tasks
- [ ] `BreezClient::list_payments()` already exists and returns all payments — **do not re-wrap it**; filter in the state layer
- [ ] Filter returned `Payment` objects by `asset_id` matching USDt constant
- [ ] Normalize into `AssetPaymentRecord` for display
- [ ] Capture/display:
  - direction
  - amount (`u64` base units → formatted string)
  - asset label
  - fee
  - status
  - timestamp
  - txid/payment identifier if available
- [ ] Add USDt-aware rendering in `src/app/view/liquid/transactions.rs`
- [ ] Add filter options (extend existing `LiquidTransactionsMessage`):
  - All
  - L-BTC
  - USDt
- [ ] Add empty state and loading state

### Deliverable
Wallet history shows USDt-related transactions clearly.

---

## Milestone 7: Error handling, resilience, and QA

### Objective
Prevent fragile behavior and make errors understandable.

### Tasks
- [ ] Add typed error mapping from Breez SDK errors into app-level errors
- [ ] Create user-friendly messages for:
  - insufficient USDt balance
  - insufficient sats for fee
  - invalid address
  - wrong network
  - failed prepare step
  - failed send step
  - failed receive step
  - connectivity issues
- [ ] Add retries only where safe
- [ ] Improve logs for development/debugging
- [ ] Ensure no sensitive data is leaked in logs
- [ ] Verify screen state recovers from failed operations

### Deliverable
Stable user-facing flows with comprehensible failures.

---

## Milestone 8: Regtest end-to-end validation

### Objective
Fully validate the USDt flow before enabling on mainnet.

> **Note:** Testnet, Testnet4, and Signet are not currently supported — `load_breez_client` returns `BreezError::NetworkNotSupported` for those networks. Validation must be done on **Regtest** (already supported) or mainnet. If testnet support is needed, add it as a prerequisite task here.

### Tasks
- [ ] *(Optional prerequisite)* Enable Breez Testnet in `load_breez_client` if regtest is insufficient
- [ ] Verify wallet loads USDt balance on regtest
- [ ] Generate amountless USDt receive request
- [ ] Generate fixed-amount USDt receive request
- [ ] Fund test wallet with regtest USDt
- [ ] Send USDt to another compatible Liquid destination
- [ ] Verify fee display correctness
- [ ] Verify payment history entries
- [ ] Test app restart and balance refresh
- [ ] Test malformed input cases
- [ ] Test insufficient balance cases
- [ ] Test wrong-network cases

### Deliverable
Confidence that the full USDt workflow works on regtest (and testnet if enabled).

---

## Milestone 9: Mainnet rollout plan

### Objective
Launch safely once testnet validation is complete.

### Tasks
- [ ] Add feature flag for USDt support
- [ ] Enable internal/dev builds first
- [ ] Validate balance parsing on mainnet wallets
- [ ] Validate receive flow with small amounts
- [ ] Validate send flow with small amounts
- [ ] Monitor logs and user-reported issues
- [ ] Prepare rollback path by disabling feature flag if needed

### Deliverable
Controlled release of USDt support.

---

## Suggested Rust API Surface

Below is a suggested internal interface for your app:

```rust
pub enum AssetKind {
    Bitcoin,
    Usdt,
}

pub struct AssetBalanceView {
    pub asset: AssetKind,
    pub asset_id: String,
    pub ticker: String,
    pub balance: u64,      // base units (e.g. 1 USDt = 100_000_000 units)
    pub precision: u8,     // 8 for USDt
}

pub struct ReceiveRequestView {
    pub asset: AssetKind,
    pub destination: String,
    pub amount: Option<u64>,  // base units
    pub uri: Option<String>,
    pub network: String,
}

pub struct AssetFeePreview {
    pub fee_asset: String,
    pub fee_amount: u64,           // base units of fee_asset
    pub fee_amount_sat: Option<u64>, // if fee is in sats
}

pub struct PreparedSendQuote {
    pub asset: AssetKind,
    pub destination: String,
    pub receiver_amount: u64,  // base units
    pub fee_preview: Option<AssetFeePreview>,
}

pub struct AssetPaymentRecord {
    pub asset: AssetKind,
    pub direction: String,
    pub amount: u64,    // base units
    pub status: String,
    pub timestamp: i64,
    pub tx_id: Option<String>,
}
```

---

## Suggested Breez Adapter Functions

These are new methods to add to `BreezClient` in `src/app/breez/client.rs`:

```rust
// Extract USDt balance from an already-fetched GetInfoResponse
// (no new SDK call — info() is already called in load_balance())
pub fn extract_usdt_balance(
    info: &breez::GetInfoResponse,
    asset_id: &str,
) -> u64;

// New receive method — existing receive_liquid() is L-BTC only
pub async fn receive_usdt(
    &self,
    amount: Option<u64>,  // base units; None for amountless
    asset_id: &str,
) -> Result<breez::ReceivePaymentResponse, BreezError>;

// New prepare method using PayAmount::Asset
pub async fn prepare_send_usdt(
    &self,
    destination: String,
    amount: u64,  // base units
    asset_id: &str,
) -> Result<breez::PrepareSendResponse, BreezError>;

// Reuse existing send_payment() — just pass the prepared response
// No new send_usdt wrapper needed

// Reuse existing list_payments() — filter by asset_id in state layer
// No new list_asset_payments wrapper needed
```

---

## Windsurf Execution Prompts

These can be pasted into Windsurf as focused implementation prompts.

### Prompt 1: Asset constants and helpers
Implement `src/app/breez/assets.rs` with Mainnet and Regtest constants for L-BTC and USDt asset IDs, plus helper functions to resolve asset IDs by network and return ticker/name/precision metadata. Also populate `asset_metadata` in `BreezConfig::sdk_config()` with USDt metadata. Include unit tests.

### Prompt 2: Balance plumbing
Extend the Breez SDK integration to fetch wallet info, parse `wallet_info.asset_balances`, and map balances into app-level types for Bitcoin and USDt. Update the wallet overview UI to display both balances safely.

### Prompt 3: Receive USDt flow
Implement a USDt receive flow using Breez Liquid SDK with `prepare_receive_payment` and `receive_payment`. Support both fixed-amount and amountless requests. Return app-level models and wire them into the receive UI.

### Prompt 4: Send USDt flow
Implement a USDt send flow using Breez Liquid SDK with `prepare_send_payment` and `send_payment`. Support Liquid addresses and BIP21 URIs. Add validation, confirmation UI state, and fee preview support.

### Prompt 5: History and error mapping
Implement payment history mapping for USDt-related transactions and add clean error handling that converts SDK failures into user-friendly app-level messages.

### Prompt 6: Test coverage
Add tests for asset resolution, amount validation, wrong-network handling, malformed destinations, and mapping of Breez responses into app-level view models.

---

## Acceptance Criteria

### Functional
- [ ] User can see USDt balance
- [ ] User can generate a USDt receive request
- [ ] User can send USDt
- [ ] User can preview fees before sending
- [ ] User can see USDt payments in history
- [ ] User sees clear validation and error messages

### Technical
- [ ] Asset IDs are network-safe
- [ ] UI does not depend directly on raw Breez structs
- [ ] Error handling is mapped into stable app-level types
- [ ] Tests cover core helpers and validation logic
- [ ] Feature can be gated for rollout

### UX
- [ ] USDt is clearly labeled as Liquid USDt
- [ ] Amount precision is handled correctly
- [ ] Fee behavior is understandable
- [ ] Wrong-network and malformed inputs are clearly rejected

---

## Risks and Watchouts

- Breez responses may differ slightly depending on destination type and route conditions.
- Asset fee behavior can confuse users if not displayed clearly.
- Wrong-network addresses/URIs could lead to failed flows or user confusion.
- Cross-asset support should not be mixed into v1 unless the core USDt path is already stable.
- Testnet and mainnet asset IDs must never be mixed.

---

## Recommended Delivery Sequence

1. Foundation/constants
2. Balance display
3. Receive USDt
4. Send USDt
5. Fee preview polish
6. History support
7. Error handling
8. Testnet QA
9. Feature-flagged mainnet rollout

---

## Post-v1 Extension Plan

After the base USDt work is stable, consider:

- adding cross-asset payments via `from_asset`
- adding "pay in USDt from BTC balance"
- adding "pay in BTC from USDt balance"
- supporting more Liquid assets
- building a cleaner asset selection UX
- adding a conversion/quote screen

---

## Final Notes for Implementation

Keep the first release narrow and reliable.

Do not over-generalize the architecture too early, but do isolate:
- network-specific asset IDs
- Breez-specific request/response handling
- formatting and validation rules

A good v1 is:
- visible USDt balance
- receive works
- send works
- fees are understandable
- history is readable
- errors are clear

That is enough to ship a strong first USDt-enabled version.
