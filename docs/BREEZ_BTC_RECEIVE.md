# BTC onchain receive in the Liquid wallet

## Two wallets, two "Bitcoin" flows

Coincube ships two distinct wallets:

- **Vault** — native Bitcoin. Address derivation, coin selection, PSBT signing, descriptor-based. Code lives under [coincube-gui/src/app/state/vault/](../coincube-gui/src/app/state/vault/). **This doc does not cover the Vault.**
- **Liquid wallet** — Liquid-native, powered by the Breez Liquid SDK. Code lives under [coincube-gui/src/app/state/liquid/](../coincube-gui/src/app/state/liquid/) and [coincube-gui/src/app/breez/](../coincube-gui/src/app/breez/).

The Liquid wallet's Receive screen has a "Bitcoin" tab. That tab is **not native Bitcoin** — it returns a Boltz-style swap address where incoming BTC is converted to L-BTC. When writing code, comments, docs or user-facing copy, always disambiguate which wallet you're talking about.

## The BTC → L-BTC swap: what can go wrong

The swap has dynamic minimum/maximum amounts set by Boltz, and the app must respect them. Two failure modes matter:

1. **Out-of-range deposit.** If the user sends BTC below the current min or above the current max, the swap cannot settle and the funds become refundable.
2. **Swap failure / timeout.** Even an in-range deposit can fail (e.g., Boltz is down, lockup tx doesn't confirm in time). Funds also become refundable.

In both cases the SDK returns the swap from [`list_refundables()`](https://github.com/breez/breez-sdk-liquid) and the user must broadcast a BTC refund transaction to recover the funds.

## Dynamic limits

- Fetched via [`BreezClient::fetch_onchain_limits`](../coincube-gui/src/app/breez/client.rs). **Never hardcode** (there is no 25k sat minimum baked into this app).
- Cached in `LiquidReceive::onchain_receive_limits: Option<(u64, u64)>` in [coincube-gui/src/app/state/liquid/receive.rs](../coincube-gui/src/app/state/liquid/receive.rs).
- Fetched eagerly inside `LiquidReceive::fetch_limits()` so the min/max warning box is visible before the user can generate an address. The generate button is replaced with a "Fetching swap limits…" indicator while limits are still loading.

## Refundable swap discovery

- [`BreezClient::list_refundables`](../coincube-gui/src/app/breez/client.rs) wraps `LiquidSdk::list_refundables`.
- `LiquidTransactions::reload()` pulls this on panel load.
- Event-driven refresh: [coincube-gui/src/app/mod.rs](../coincube-gui/src/app/mod.rs) `App::refresh_refundables_task()` is a debounced helper (30s) invoked from `SdkEvent::PaymentFailed`, `PaymentRefundable`, `PaymentRefundPending`, `PaymentRefunded`, and `Synced`. The result is routed directly to `LiquidTransactions` via an explicit `Message::RefundablesLoaded` arm in `App::update`, so the panel updates even when it is not currently visible.
- For swaps that expired while the app was offline, the `Synced` arm fires `list_refundables` after the initial sync so they still surface without user action.

## The in-app refund flow

1. The user opens Transactions → refundable card (list row labeled "Refundable Swap"). Refundables only show under the **All** and **L-BTC** asset filters — they never appear under USDt.
2. Detail view [coincube-gui/src/app/view/liquid/transactions.rs](../coincube-gui/src/app/view/liquid/transactions.rs) `refundable_detail_view` shows the swap address (middle-truncated + copy button), amount, and a refund form.
3. The user either pastes a BTC address or taps **Use Vault address** which routes `Message::GenerateVaultRefundAddress` → `daemon.get_new_address()` → `Message::RefundAddressEdited`. This reuses the Vault's existing fresh-address derivation rather than duplicating any descriptor logic.
4. Fee rate is populated via Low/Medium/High priority buttons. Primary source is the local `FeeEstimator`; if it errors the GUI falls back to `BreezClient::recommended_fees()` (SDK → Esplora/Electrum). The pressed button shows a "…" label while the async fetch is in flight.
5. On submit, `LiquidTransactions::in_flight_refunds` optimistically records the swap so the card stays visible with a "Refund broadcasting…" banner. On success the banner becomes "Refund broadcast · txid …".
6. `RefundablesLoaded` reconciles: when the SDK no longer returns a swap we previously recorded in `in_flight_refunds`, we drop the local entry. On `RefundCompleted(Err)` we clear any local entry without a txid so a failed attempt doesn't leave a stale banner.

## Status model

[coincube-gui/src/app/breez/swap_status.rs](../coincube-gui/src/app/breez/swap_status.rs) defines `BtcSwapReceiveStatus` — an enum that maps the SDK's raw `PaymentState` into the UI lifecycle (`AwaitingDeposit`, `PendingConfirmation`, `PendingSwapCompletion`, `WaitingFeeAcceptance`, `Refundable`, `Refunding`, `Refunded`, `Completed`, `Failed`).

Use `classify_payment(&payment, &refundable_swap_addresses)` to get the status for a `Payment`. The second argument is needed because a `Payment` in `PaymentState::Failed` is only effectively refundable if the SDK still returns it from `list_refundables` — otherwise it's genuinely terminal.

When the SDK adds a new `PaymentState` or `SdkEvent`, extend `classify_payment` and add a matching test in the `#[cfg(test)]` block in the same file. Don't spread the mapping logic across call sites.

## Logging

All refund/limit/swap activity logs under `target: "breez_swap"`. `truncate_addr` in [coincube-gui/src/app/breez/client.rs](../coincube-gui/src/app/breez/client.rs) keeps only the first and last 6 chars of any address-like value — use it when logging so logs remain debuggable without leaking full on-chain identifiers. Never log seeds, xprivs, or full addresses from this target.

## Where to extend

| Change | File |
|---|---|
| New SDK wrapper method | [coincube-gui/src/app/breez/client.rs](../coincube-gui/src/app/breez/client.rs) |
| New `SdkEvent` handling | [coincube-gui/src/app/mod.rs](../coincube-gui/src/app/mod.rs) `Message::BreezEvent` arm |
| New lifecycle state | [coincube-gui/src/app/breez/swap_status.rs](../coincube-gui/src/app/breez/swap_status.rs) |
| Receive UI copy / limits display | [coincube-gui/src/app/view/liquid/receive.rs](../coincube-gui/src/app/view/liquid/receive.rs) |
| Refund UI / in-flight banner | [coincube-gui/src/app/view/liquid/transactions.rs](../coincube-gui/src/app/view/liquid/transactions.rs) + [coincube-gui/src/app/state/liquid/transactions.rs](../coincube-gui/src/app/state/liquid/transactions.rs) |
