# Spark Wallet

The Spark wallet is COINCUBE's default backend for everyday Lightning UX. It runs alongside the existing Liquid wallet: Spark handles BOLT11, LNURL-pay, and incoming Lightning Address invoices; Liquid remains the advanced wallet for L-BTC, USDt, and Liquid-specific flows.

## Networks

Spark ships **mainnet only**. The Breez Spark SDK (v0.13.1) does not publish a regtest flow, and COINCUBE uses real mainnet sats in small amounts for end-to-end testing. Liquid's regtest flow (`docs/BREEZ_SDK_REGTEST.md`) is untouched and still works.

## Architecture

The Spark SDK's dependency graph (`rusqlite`, `tokio_with_wasm`, `frost_secp256k1_tr`, Spark tree primitives) is incompatible with the Breez Liquid SDK's dep graph at the `links = "sqlite3"` level. Instead of forking and patching the two SDKs to coexist in a single binary, COINCUBE runs Spark in a sibling process and talks to it over a minimal JSON-RPC protocol.

```text
┌──────────────┐         stdin/stdout           ┌──────────────────────┐
│ coincube-gui │ ◄──────── JSON-RPC ──────────► │ coincube-spark-bridge │
│   (iced UI)  │                                │  (breez-sdk-spark)    │
└──────────────┘                                └──────────────────────┘
       │                                                    │
       │ links: breez-sdk-liquid                            │ links: spark-sdk
       │                                                    │
       ▼                                                    ▼
┌──────────────┐                                   ┌──────────────────┐
│  Liquid SDK  │                                   │    Spark SDK     │
└──────────────┘                                   └──────────────────┘
```

### Components

- **`coincube-spark-protocol`** — shared crate defining the wire types (`Method`, `Response`, `Event`, `OkPayload`, error envelope). Both the gui and bridge link this crate. Adding a new RPC means adding a variant here first.
- **`coincube-spark-bridge`** — standalone binary (own Cargo workspace, excluded from the main workspace via `[workspace.exclude]`). Owns the `breez_sdk_spark::BreezSdk` handle, exposes handlers for every protocol method, and forwards `SdkEvent` frames as `Event::*` messages on an async broadcast channel.
- **`coincube-gui/src/app/breez_spark/`** — gui-side subprocess client. `SparkClient` spawns the bridge binary, frames JSON-RPC requests/responses over line-delimited JSON on stdio, and surfaces an iced `Subscription` over the bridge's event stream.
- **`coincube-gui/src/app/wallets/spark.rs`** — `SparkBackend` wrapper that exposes the panel-facing read/write surface (`get_info`, `list_payments`, `prepare_send`, `send_payment`, `receive_bolt11`, `receive_onchain`, `list_unclaimed_deposits`, `claim_deposit`, `get_user_settings`, `set_stable_balance`, …). Panels never talk to `SparkClient` directly.
- **`coincube-gui/src/app/state/spark/`** and **`coincube-gui/src/app/view/spark/`** — panel state machines and renderers for Overview, Send, Receive, Transactions, and Settings. Structure mirrors `state/liquid/` and `view/liquid/` to keep both wallets navigable at a glance.

### Bridge lifecycle

- The gui spawns the bridge lazily when the cube has a `master_signer_fingerprint` set (a single MasterSigner shared by all wallets). If the spawn fails (binary missing, handshake error) the `WalletRegistry` holds `spark = None`, and all Spark panels render an "unavailable" stub.
- The bridge binary is located via the `COINCUBE_SPARK_BRIDGE_PATH` env var, or falls back to `coincube-spark-bridge` sitting alongside the main `coincube` binary in the same directory.
- Shutdown is cooperative: the gui sends a `Shutdown` method on app exit; the bridge drops its SDK handle and closes stdio.

### Seed handoff (Phase 3 compromise)

The gui decrypts the `MasterSigner` mnemonic on PIN entry and hands it to the bridge as part of the `Init` request. Seed stays in memory for the session lifetime inside the bridge process. The architectural win is that the mnemonic lives in a separate address space from the gui's MasterSigner on Unix, but on a single-user desktop a local attacker who can read one process's memory can usually read the other too.

A cleaner alternative — Breez Spark's `ExternalSigner` trait, which lets the SDK call back into the gui for every signing operation — exists as of spark-sdk 0.13.1. We're not using it yet because 11 of the 20 trait methods are Spark-specific (FROST round-2/aggregate, Shamir secret splitting, Spark tree node derivation) that `MasterSigner` can't answer today without pulling `spark_wallet` / `frost_secp256k1_tr` directly into the gui crate — exactly the dep-graph collision the subprocess architecture exists to avoid. Migrating to `ExternalSigner` is tracked as a deferred follow-up; see the integration plan at `/.claude/plans/crystalline-booping-wadler.md` §"Deferred follow-up: external signer migration".

## Setting up a Spark wallet

1. **Create a cube** the usual way.
2. **The Spark wallet is provisioned automatically.** Both the Liquid and Spark SDKs share a single MasterSigner (`CubeSettings::master_signer_fingerprint`). New cubes set this at creation time; existing cubes are migrated on first unlock (legacy field names are mapped via serde aliases). No separate "Spark signer" setup step is needed.
3. **Set `BREEZ_API_KEY`** in the environment (or `.env`). A single Breez API key covers both the Liquid and Spark SDKs.
4. **Restart the cube.** The first launch spawns the bridge subprocess, runs `init`, and unlocks the Spark wallet for the session.

When you open the cube, the sidebar shows the **Spark** submenu above the **Liquid** submenu. Spark hosts Overview, Send, Receive, Transactions, and Settings panels.

## Features

### Lightning (BOLT11 + LNURL-pay)

- **Receive**: amountless or fixed-amount BOLT11 invoices with a user-supplied description. QR code is rendered once per invoice to avoid re-encoding on every frame.
- **Send**: pastes a BOLT11 invoice, BIP21 URI, or Lightning Address into the Send panel. The bridge classifies the input via `parse_input` and routes to either `prepare_send` (BOLT11 / BIP21 / on-chain address) or `prepare_lnurl_pay` (LNURL / Lightning Address). Prepare responses are held in a bridge-side pending map keyed by an opaque UUID handle, so the gui never has to round-trip complex SDK types over JSON; the handle is consumed on `send_payment`.
- **Pending-prepare TTL**: prepared sends expire after 5 minutes if not consumed. A background task in the bridge sweeps both pending maps every 60 seconds to cap memory growth.

### On-chain (deposit address + claim lifecycle)

Spark uses a deposit-address model: the user sends BTC to a static address, the bridge notices incoming transactions, and the funds become spendable once the SDK's `claim_deposit` call succeeds.

- The Receive panel shows a "Pending deposits" card below the main body, listing every unclaimed deposit the SDK is aware of.
- Each row renders one of four actions: **Claim** (mature), **Claiming…** (in-flight), **Waiting for confirmation** (immature), or **Retry** + error hint (previous claim failed).
- The list refreshes automatically on `Event::DepositsChanged` events from the bridge (emitted for `SdkEvent::UnclaimedDeposits`, `ClaimedDeposits`, and `NewDeposits`), so the card appears the moment the SDK observes an incoming deposit without manual refresh.

### Lightning Address routing (Phase 5)

`@coincube.io` Lightning Addresses are fulfilled by whichever backend the cube's `default_lightning_backend` setting prefers. New cubes default to Spark; users can flip to Liquid in **Settings → Lightning**.

Because Spark SDK 0.13.1's `ReceivePaymentMethod::Bolt11Invoice` only accepts a plain `description` (not a `description_hash`), the coincube-api sends the raw LNURL metadata preimage in each invoice-request event. Spark mints the invoice with that preimage as its `d` tag — the payer's wallet recomputes `SHA256(description)` and compares it against the `descriptionHash` the callback advertised. They match by construction because the API produces both from the same source.

NIP-57 zap requests frequently exceed BOLT11's 639-byte description cap. The stream handler detects this and falls back to Liquid, which commits via `description_hash` directly. Falling back is silent — no user-visible error.

### Stable Balance (Phase 6)

The Spark SDK's built-in Stable Balance feature is exposed as a single **Stable Balance** toggle in **Spark → Settings**. Turning it on activates the SDK's automatic conversion of excess sats into a USD-pegged token internally; the user continues to see a single Bitcoin balance that stays stable against fiat. Turning it off unpegs and returns to a pure BTC balance. The toggle is disabled when the bridge is unreachable (the bridge-status card on the same page shows the connection state).

Implementation detail hidden from the UI: the SDK uses the USDB stable token (`btkn1xgrvjwey5ngcagvap2dzzvsy4uk8ua9x69k82dwvt5e7ef9drm9qztux87` on mainnet) under the label `"USDB"`. This label never surfaces in the gui — the panel always calls it "Stable Balance". The Overview panel shows a small "Stable" badge next to the balance line when the feature is active.

## Troubleshooting

- **"Spark bridge unavailable" in every Spark panel** — the bridge subprocess either failed to spawn (binary not found at `COINCUBE_SPARK_BRIDGE_PATH` or next to `coincube`) or crashed during handshake. Check stderr from `coincube-spark-bridge` for the root cause.
- **"Stable Balance is not configured" errors from `set_stable_balance`** — the bridge's `mainnet_config` always wires up `stable_balance_config` with the USDB token, so this should never fire in a release build. If it does, the bridge got started without the token constant — rebuild and re-deploy.
- **Incoming Lightning Address payments land on Liquid, not Spark** — either the cube's `default_lightning_backend` is still set to Liquid (check **Settings → Lightning**), the bridge is down for this cube (check the bridge-status card in **Spark → Settings**), the API hasn't been updated to send the `description` field over SSE yet, or the payer is sending a NIP-57 zap (description exceeds 639 bytes → automatic fallback).
- **Lost Spark balance after restarting the cube** — `storage_dir` may have moved between restarts. The SDK stores its local database there; pointing at a different directory on the next launch looks like an empty wallet until the SDK re-syncs. Ensure the `datadir` passed to `load_spark_client` is stable per cube.

## Testing

- **Mainnet-only**: per user direction, Spark development and testing happen on mainnet with small amounts. There is no regtest harness for Spark.
- **Smoke-test script**: `cargo run -p coincube-spark-bridge --bin coincube-spark-bridge-smoketest` (Phase 2 test harness) connects to Spark mainnet, runs `init` + `get_info`, and prints the balance/pubkey. Use it to verify the bridge binary + API key + mnemonic independently of the gui.
- **End-to-end journey**: create a cube → load Spark and Liquid → receive sats via `@coincube.io` (should land on Spark) → send a BOLT11 payment → send L-BTC from Liquid → toggle Stable Balance on and off → verify everything in Transactions.

## References

- Breez Spark SDK rustdoc: <https://breez.github.io/spark-sdk/breez_sdk_spark/>
- Stable Balance guide: <https://sdk-doc-spark.breez.technology/guide/stable_balance.html>
- External signer guide (deferred follow-up): <https://sdk-doc-spark.breez.technology/guide/external_signer.html>
- Integration plan: `/.claude/plans/crystalline-booping-wadler.md`
- Wallet abstraction engineering note: [WALLETS.md](./WALLETS.md)
