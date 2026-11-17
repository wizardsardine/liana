# Spark Wallet

The Spark wallet is COINCUBE's backend for everyday Lightning UX: BOLT11, LNURL-pay, and incoming Lightning Address invoices. It sits beside the Vault in each Cube. (The former Liquid wallet has been removed; see [WALLETS.md](./WALLETS.md).)

## Networks

Spark ships **mainnet only**. The bridge pins upstream `breez-sdk-spark` tag 0.28.1 (`coincube-spark-bridge/Cargo.toml`). That SDK does not publish a regtest flow, and COINCUBE uses real mainnet sats in small amounts for end-to-end testing.

## Architecture

The Spark SDK's dependency graph (`rusqlite`, `tokio_with_wasm`, `frost_secp256k1_tr`, Spark tree primitives) is incompatible with the gui's at the `links = "sqlite3"` level: `coincubed` links its own rusqlite 0.38 fork. (This conflict was first with the since-removed Breez Liquid SDK.) Instead of forking and patching the two to coexist in a single binary, COINCUBE runs Spark in a sibling process and talks to it over a minimal JSON-RPC protocol.

```text
┌──────────────┐         stdin/stdout           ┌──────────────────────┐
│ coincube-gui │ ◄──────── JSON-RPC ──────────► │ coincube-spark-bridge │
│   (iced UI)  │                                │  (breez-sdk-spark)    │
└──────────────┘                                └──────────────────────┘
       │                                                    │
       │ links: coincubed (rusqlite 0.38 fork)              │ links: spark-sdk
       │                                                    │
       ▼                                                    ▼
┌──────────────┐                                   ┌──────────────────┐
│ Vault engine │                                   │    Spark SDK     │
└──────────────┘                                   └──────────────────┘
```

### Components

- **`coincube-spark-protocol`** — shared crate defining the wire types (`Method`, `Response`, `Event`, `OkPayload`, error envelope). Both the gui and bridge link this crate. Adding a new RPC means adding a variant here first.
- **`coincube-spark-bridge`** — standalone binary (own Cargo workspace, excluded from the main workspace via `[workspace.exclude]`). Owns the `breez_sdk_spark::BreezSdk` handle, exposes handlers for every protocol method, and forwards `SdkEvent` frames as `Event::*` messages on an async broadcast channel.
- **`coincube-gui/src/app/breez_spark/`** — gui-side subprocess client. `SparkClient` spawns the bridge binary, frames JSON-RPC requests/responses over line-delimited JSON on stdio, and surfaces an iced `Subscription` over the bridge's event stream.
- **`coincube-gui/src/app/wallets/spark.rs`** — `SparkBackend` wrapper that exposes the panel-facing read/write surface (`get_info`, `list_payments`, `prepare_send`, `send_payment`, `receive_bolt11`, `receive_onchain`, `list_unclaimed_deposits`, `claim_deposit`, `get_user_settings`, `set_stable_balance`, …). Panels never talk to `SparkClient` directly.
- **`coincube-gui/src/app/state/spark/`** and **`coincube-gui/src/app/view/spark/`** — panel state machines and renderers for Overview, Send, Receive, Transactions, and Settings.

### Bridge lifecycle

- The gui spawns the bridge lazily when the cube has a `master_signer_fingerprint` set (a single MasterSigner shared by all wallets). If the spawn fails (binary missing, handshake error) the `WalletRegistry` holds `spark = None`, and all Spark panels render an "unavailable" stub.
- The bridge binary is located via the `COINCUBE_SPARK_BRIDGE_PATH` env var, or falls back to `coincube-spark-bridge` sitting alongside the main `coincube` binary in the same directory, or — in a development checkout only — to `coincube-spark-bridge/target/{debug,release}/`. See [Packaging](#packaging) for what that means for released builds.
- On Windows the bridge is a console-subsystem binary and the GUI is a windows-subsystem binary. `SparkClient` passes `CREATE_NO_WINDOW` (`0x08000000`) at spawn so opening a Cube does not allocate a console. Stdin, stdout, and stderr stay piped, and `kill_on_drop` still ends the child when the client is dropped. The bridge holds the session mnemonic, so it must not be started with `node::detach_spawned_process`: that helper also sets `DETACHED_PROCESS` (and `setsid` on Unix) so a managed bitcoind or Tor outlives the app.
- Shutdown is cooperative: the gui sends a `Shutdown` method on app exit; the bridge drops its SDK handle and closes stdio.

### Packaging

The bridge is a second binary, and a release artifact that does not contain it has a Spark wallet that cannot start — every panel renders the "not configured" stub, on a cube that is perfectly fine. Every build shipped between 2026-04-15 and 2026-08-06 was in exactly that state, because `cargo build --package coincube-gui` cannot build a crate the root workspace excludes, and the third resolution step above makes any developer checkout look healthy.

So, for each platform, CI builds the bridge under the same `minimal` profile as the gui and ships it **in the same directory as the app executable, under its own name**:

| Platform | Location |
|---|---|
| macOS | `COINCUBE.app/Contents/MacOS/coincube-spark-bridge`, placed before `rcodesign sign` so the signature seals it |
| Windows | the MSI's `bin` directory, next to `coincube.exe` |
| Linux | inside `coincube-desktop-<version>-<target>.tar.gz`, next to `coincube` |

Two consequences worth knowing before editing a workflow or a `.wxs`:

- **The filename is load-bearing and the directory is load-bearing.** `resolve_bridge_path()` joins the literal `coincube-spark-bridge` (`.exe` on Windows) onto `current_exe().parent()`. The macOS main binary is renamed to `Coincube` in the bundle; the bridge must *not* be renamed to match, and must not be moved to `Contents/Resources/` or `Contents/Frameworks/`.
- **Nothing else in the pipeline notices when it goes missing.** `codesign`, notarization and `spctl` all pass on a bundle with no bridge in it. The only thing standing between that regression and a user is the explicit contents assertion in each workflow (`Verify Spark bridge is bundled…`, `Verify Spark bridge is in the MSI`, and the `tar tzf` check on Linux). Each of them also runs the packaged bridge through a `shutdown` round trip, which needs no API key, no mnemonic and no network.

The bridge carries no entitlements of its own: `rcodesign` signs it as nested code, and the entitlements XML applies to the main executable only. Nothing it does needs one — `BREEZ_API_KEY` reaches the bridge in the JSON-RPC `Init` message, not the environment.

### Seed handoff (Phase 3 compromise)

The gui decrypts the `MasterSigner` mnemonic on PIN entry and hands it to the bridge as part of the `Init` request. Seed stays in memory for the session lifetime inside the bridge process. The architectural win is that the mnemonic lives in a separate address space from the gui's MasterSigner on Unix, but on a single-user desktop a local attacker who can read one process's memory can usually read the other too.

A cleaner alternative — Breez Spark's `ExternalSigner` trait, which lets the SDK call back into the gui for every signing operation — exists as of spark-sdk 0.13.1. We're not using it yet because 11 of the 20 trait methods are Spark-specific (FROST round-2/aggregate, Shamir secret splitting, Spark tree node derivation) that `MasterSigner` can't answer today without pulling `spark_wallet` / `frost_secp256k1_tr` directly into the gui crate — exactly the dep-graph collision the subprocess architecture exists to avoid. Migrating to `ExternalSigner` is tracked as a deferred follow-up; see the integration plan at `/.claude/plans/crystalline-booping-wadler.md` §"Deferred follow-up: external signer migration".

## Setting up a Spark wallet

1. **Create a cube** the usual way.
2. **The Spark wallet is provisioned automatically.** Spark and the Cube's other seed-derived features share a single MasterSigner (`CubeSettings::master_signer_fingerprint`). New cubes set this at creation time; existing cubes are migrated on first unlock (legacy field names are mapped via serde aliases). No separate "Spark signer" setup step is needed.
3. **Set `BREEZ_API_KEY`** in the environment (or `.env`). The Breez API key is passed to the bridge in its `Init` message.
4. **Restart the cube.** The first launch spawns the bridge subprocess, runs `init`, and unlocks the Spark wallet for the session.

When you open the cube, the sidebar shows **Spark**. Spark hosts Overview, Send, Receive, Transactions, and Settings panels.

## Features

### Lightning (BOLT11 + LNURL-pay)

- **Receive**: amountless or fixed-amount BOLT11 invoices with a user-supplied description. QR code is rendered once per invoice to avoid re-encoding on every frame.
- **Send**: pastes a BOLT11 invoice, BIP21 URI, or Lightning Address into the Send panel. The bridge classifies the input via `parse_input` and routes to either `prepare_send` (BOLT11 / BIP21 / on-chain address) or `prepare_lnurl_pay` (LNURL / Lightning Address). Prepare responses are held in a bridge-side pending map keyed by an opaque UUID handle, so the gui never has to round-trip complex SDK types over JSON; the handle is consumed on `send_payment`.
- **Pending-prepare TTL**: prepared sends expire after 5 minutes if not consumed. A background task in the bridge sweeps both pending maps every 60 seconds to cap memory growth.

### On-chain (deposit address + claim lifecycle)

Spark uses a deposit-address model: the user sends BTC to a static address, the bridge notices incoming transactions, and the funds become spendable once the SDK's `claim_deposit` call succeeds.

- The Receive panel shows a "Pending deposits" card below the main body, listing every unclaimed deposit the SDK is aware of.
- Each row renders one of four actions: **Claim** (mature), **Claiming…** (in-flight), **Waiting for confirmation** (immature), or **Retry** + error hint (previous claim failed).
- A **Claim** that the SDK defers (0.28.0+) leaves a neutral "Not claimed yet" note on that deposit's row naming why. Each deferred deposit retains its own note until it is retried, claimed, or leaves the pending list. See Troubleshooting.
- The list refreshes automatically on `Event::DepositsChanged` events from the bridge (emitted for `SdkEvent::UnclaimedDeposits`, `ClaimedDeposits`, and `NewDeposits`), so the card appears the moment the SDK observes an incoming deposit without manual refresh.

### Receive from another network (SideShift)

Spark → Receive → **From another network** deposits a supported stablecoin and receives bitcoin. The flow is `coincube-gui/src/app/state/spark/sideshift_receive.rs` (it began as a copy of the removed Liquid wallet's receive flow; three differences are load-bearing):

- SideShift has no Lightning rail. The shift settles to the Spark wallet's **static** on-chain deposit address (`receive_onchain(new_address = false)`), which is reusable and therefore safe for a variable amount. Bitcoin shows up after confirmation, not when SideShift reports `Settled`. The rate is variable; there is no fixed-rate quote step.
- The user deposits the other asset and receives BTC. Copy must not say they receive the deposited coin.
- A refund address is collected **before** Generate, and it has to be an address on the **origin** network. `validate_refund_address` (`coincube-gui/src/services/sideshift/types.rs`) is a shape check, not a checksum: an empty address, an unrecognized shape, or an address that detects as a different network (a Bitcoin address pasted for a Tron deposit is the usual case) is refused. Generate stays disabled until that check passes (`can_generate`). A refund sent to the wrong network is not recoverable from this app.

Arrival is matched on SideShift's settle txid, not on an amount:

- Unclaimed deposits are compared with `txid`. A match is still pending; Home is asked to auto-claim it.
- Claimed payments are scanned newest-first, at most 5 pages of 100. A match is a `deposit` whose status is `Completed` and whose `details.tx_id` equals the settle hash (ASCII case-insensitive). The shown amount is that payment's sats, after the network fee.
- Home's watcher treats an empty unclaimed list as "claimed" and skips a swap it is not already tracking. That misses an SDK background claim while another transfer is tracked, and it waits forever if a restored shift is registered after the deposit was already claimed. The receive flow therefore watches the settle tx itself (`awaiting_claim`) and, for a shift restored from `PendingSparkShift`, does not register the arrival until it knows the bitcoin is still unclaimed (`arrival_unconfirmed`). Unknown, or a lookup that failed, is not guessed. While the phase is still Active, a 30-second timer retries that restored lookup and, once it has resolved, an arrival that is still waiting on the claim. The two timers do not run together.

The offered deposit assets are the stablecoin list in `deposit_options`. Ether, BNB, SOL, and TRX are intentionally absent.

### Lightning Address routing (Phase 5)

`@coincube.io` Lightning Addresses are fulfilled by Spark (`WalletRegistry::route_lightning_address`). A Cube without a running Spark bridge has no route.

Because Spark SDK 0.13.1's `ReceivePaymentMethod::Bolt11Invoice` only accepts a plain `description` (not a `description_hash`), the coincube-api sends the raw LNURL metadata preimage in each invoice-request event. Spark mints the invoice with that preimage as its `d` tag — the payer's wallet recomputes `SHA256(description)` and compares it against the `descriptionHash` the callback advertised. They match by construction because the API produces both from the same source.

NIP-57 zap requests frequently exceed BOLT11's 639-byte description cap. Liquid used to take those (it commits via `description_hash` directly); with Liquid removed there is no fallback for them.

### Stable Balance / auto-convert (Phase 6, S2b)

**Spark → Settings → Balances & conversion** has one switch, **Auto-convert received bitcoin**. While it is on, the bridge swaps each incoming bitcoin payment to USDB (`btkn1xgrvjwey5ngcagvap2dzzvsy4uk8ua9x69k82dwvt5e7ef9drm9qztux87` on mainnet) through the Flashnet AMM, so new receipts hold their dollar value. Turning it off stops converting *new* bitcoin and leaves the USDB already held as USDB. The switch is disabled while the bridge is unreachable (the Connections tab shows its state).

Since S2b the conversion is the **bridge's own** (`coincube-spark-bridge/src/auto_convert.rs`), not the SDK's Stable Balance, which `sdk_adapter::mainnet_config` leaves unconfigured (`stable_balance_config` is `None`) and the bridge never activates. The SDK feature would convert the existing bitcoin balance on activation and queue the whole USDB balance back to bitcoin on deactivation; ours does neither. In outline:

- every successful incoming bitcoin payment adds its sats to a pending total as the SDK reports it (`AutoConvert::note_receive`);
- one worker (`convert_pending`) converts the pending sats, bounded by the bitcoin balance, once they reach the pool's minimum, as a self-transfer to the wallet's own Spark address with a conversion attached;
- after a manual swap out of USDB (`Method::Swap`) receives are held (`AutoConvertState::held_after_swap`) so the swap isn't immediately undone;
- the switch, the breaker, the pending sats and the hold persist per Cube in `coincube-auto-convert.json` in the Cube's Spark storage directory. A Cube without that file adopts the "on" it had under SDK Stable Balance (`legacy_stable_balance_label`, read-only).

The wire methods are `get_auto_convert` / `set_auto_convert` and the `AutoConvertStateChanged` event. The older `get_user_settings` / `set_stable_balance` remain as aliases for a gui built before S2b: `stable_balance_active` now means auto-convert is on (`user_settings_ok`). The gui still saves the switch through `set_stable_balance`, so the per-Cube preference record keeps working. The label "USDB" never surfaces as the feature's name; the Overview shows USDB as its own balance row.

Four things the switch does *not* control, and how the app handles them:

- **Spending USDB.** The bridge attaches a USDB→BTC conversion to a sats send whenever the bitcoin balance falls short of `amount + fee` and USDB is held (`prepare_with_usdb_fallback` in `server.rs`), so the whole unified balance is spendable to any Bitcoin / Lightning / Spark destination whether auto-convert is on or off. A send funded that way goes through the Flashnet AMM and carries a token leg (`has_token_leg`, no idempotency key); the Amount field says so once the entered amount exceeds the bitcoin balance. If the AMM refuses the conversion, the bridge answers with a `bad_request` naming the bitcoin balance that is still sendable and the AMM's reason, rather than the SDK's raw JSON.
- **Poisoned pools (why the SDK must stay ≥ 0.22.0).** Flashnet pool creation is permissionless, and SDK releases before 0.22.0 quoted a V3 concentrated pool from its self-reported price alone, with no reserve check — so an empty pool created with an absurd tick quoted a near-zero `amount_in`, won `select_best_pool`'s 50%-weight fee-efficiency score against the real pool, and every conversion failed at the AMM (`FSAG-4201`). This bit us on mainnet BTC/USDB from 2026-09-07 (two zero-liquidity pools from host `auditlab`, prices 1e30 and 1e-30, one poisoning each direction) while the real pool was healthy and while we were still on 0.19.0. Upstream commit `7ba9363a` ("Flashnet: validate pool data and swap responses", first in 0.22.0) added the output-reserve check for every curve plus price/tick/fee validation; the bridge moved to 0.25.0 on 2026-09-11. **The same afternoon a pool with `"hostName": null` appeared and 0.25.0 failed to parse the whole listing** (`Pool.host_name` is a `String`), so every conversion for every SDK client reported `NoPoolsAvailable`. The bridge pinned `coincubetech/spark-sdk` branch `coincube/0.25.0-stable-balance-hardening` — upstream 0.25.0 plus four commits — until 2026-11-10, and now pins upstream tag 0.28.1. Upstream 0.28.0 carries three of the four: the per-pool listing parse with a null host (`a781273b`), backoff for failed Stable Balance conversions (`7afca718`), and `SdkEvent::StableBalanceConversionFailed`. The fourth — drop pools priced >2× from the deepest pool's before scoring — Breez declined in breez/spark-sdk#1114, with counterexamples that hold: a pool listing the assets in the other order is never compared on the same footing; a fake with an absurd price looks deepest, because TVL is computed from its own price, so it becomes the reference and the real pool is the one dropped; and a fake priced 1.5× better than the market sits inside the band. **Residual risk:** a pool can still take the full fee-efficiency score for a price it has almost no liquidity behind, win selection, and fail at the AMM. That fails the conversion (no funds move) rather than mispricing it, and the bridge's circuit breaker pauses auto-convert after repeated failures. The fix belongs in upstream's scoring (weight the fee credit by the liquidity behind the quote); that is the thing to raise with Breez. To inspect the live pool list: `curl 'https://api.flashnet.xyz/v1/pools?assetAAddress=<btc>&assetBAddress=<usdb-hex>'` with `<btc>` = `0202…02` (33 bytes) and `<usdb-hex>` the bech32m-decoded token id.
- **Where the decision lives.** The bridge's state file sits with the SDK's local store, not with the seed, so a Cube recovered on another machine starts with auto-convert off while possibly holding USDB. The app records the user's decision in `CubeSettings::spark_stable_balance` and reconciles it against the bridge after the session's first `Synced`; with no recorded decision and USDB held, a banner points the user at the switch. The decision is also mirrored to the Cube's Connect record (`sparkStableBalance` on `POST/PUT /connect/cubes`, echoed on every `CubeResponse`; same tri-state, absent-means-unchanged contract as `hasVault`): every local write is pushed, the registration request carries it, and a device with no decision adopts the server's when its registration answers — so a recovered Cube on a signed-in device picks the setting back up without the prompt.
- **Repeated failures (the breaker).** A conversion that fails at the AMM (e.g. `Pool has no liquidity`, or a thinly backed pool winning selection — see above) moves no funds. The bridge counts failures with no success in between (`FailureTracker`), and after `STABLE_BALANCE_PAUSE_THRESHOLD` (3) turns auto-convert off, marks the breaker `Paused` with the last reason, and emits `AutoConvertStateChanged` plus the legacy `StableBalancePaused`. Settings shows the pause and its reason; the gui records the decision as "off" so the next launch doesn't re-arm it. The SDK's own `StableBalanceConversionFailed` event (0.28.0) only concerns SDK Stable Balance, which the bridge never configures, so it is only logged.

## Troubleshooting

- **"Spark bridge unavailable" in every Spark panel** — the bridge subprocess either failed to spawn (binary not found at `COINCUBE_SPARK_BRIDGE_PATH` or next to `coincube`) or crashed during handshake. Check stderr from `coincube-spark-bridge` for the root cause. In a checkout, the usual cause is having run bare `cargo build` instead of `make build`; in an installed build, it means the artifact shipped without the bridge — check the app directory for it (see [Packaging](#packaging)).
- **A console window opens with the Cube on Windows** — the bridge was spawned without `CREATE_NO_WINDOW`. A current build sets that flag in `SparkClient` (`coincube-gui/src/app/breez_spark/client.rs`). Closing the console kills the bridge, and the Spark panels then show unavailable. Hiding the window by detaching the process would also leave the mnemonic-holding child running after the GUI exits.
- **Auto-convert switched itself off** — the breaker tripped after three failed conversions in a row (see above). **Spark → Settings** shows the last reason, which is the AMM's (e.g. `Pool has no liquidity`). Turn it back on once the pool is healthy; check the live pool list with the `curl` above.
- **"Not claimed yet" under Pending deposits** — since SDK 0.28.0 a claim can be *deferred*: an early claim would cost more than the fee ceiling (`NetworkRecommended` + 5 sat/vB), no early claim is offered at the deposit's depth yet, or the claim provider declined. Nothing failed: `claim_deposit` answers Ok with `ClaimDepositOk::deferred` naming the reason, the card shows it as a note rather than an error, and the SDK claims the deposit on its own (early as confirmations accrue, or at maturity). Home's auto-claim treats a deferral like a failure in its log and leaves its indicator pending.
- **A SideShift receive stays on "Arriving in Spark" after the settle tx is claimed** — the page finishes when a completed deposit payment's `tx_id` matches SideShift's settle hash, or when that hash is still in the unclaimed list. Matching on amount, or treating "the unclaimed list is empty" as claimed, leaves the flow waiting. A restored shift that cannot see the hash yet stays unconfirmed and looks again; it does not register the arrival with Home. See [Receive from another network](#receive-from-another-network-sideshift).
- **"sats … waiting to be recovered on-chain" under Spark → Settings → Connections** — the SDK (0.28.0+) reports funds that left the Spark balance and wait for an on-chain `recover_funds` (`GetInfoOk::recoverable_funds_sats`; the bridge also logs a warning on the SDK's `RecoverableFunds` event). COINCUBE Desktop has no recovery flow yet, so it only shows the figure.
- **"insufficient funds" on a send well under the shown balance** — the balance on the Send and Home cards is bitcoin *plus* USDB valued at the BTC/USD price. The `connected — identity …, balance N sats` line at bridge startup is bitcoin only. The bridge should cover the gap from USDB automatically; if it didn't, check whether `get_info` reports the USDB token balance.
- **Incoming Lightning Address payments don't arrive** — the bridge is down for this cube (check the bridge-status card in **Spark → Settings**), the API hasn't been updated to send the `description` field over SSE yet, or the payer is sending a NIP-57 zap (description exceeds 639 bytes, which Spark can't commit to).
- **Lost Spark balance after restarting the cube** — `storage_dir` may have moved between restarts. The SDK stores its local database there; pointing at a different directory on the next launch looks like an empty wallet until the SDK re-syncs. Ensure the `datadir` passed to `load_spark_client` is stable per cube.

## Testing

- **Mainnet-only**: per user direction, Spark development and testing happen on mainnet with small amounts. There is no regtest harness for Spark.
- **Unit tests**: the bridge is its own workspace, so a bare `cargo test` at the root does not reach it. Run `make test`, or `cargo test --manifest-path coincube-spark-bridge/Cargo.toml`. CI does this in the `bridge_tests` job.
- **Smoke test**: `coincube-spark-bridge --smoke-test` (Phase 2 test harness) connects to Spark mainnet, runs `init` + `get_info` + `list_payments`, and prints the balance/pubkey. It reads `BREEZ_API_KEY`, `COINCUBE_SPARK_MNEMONIC` and `COINCUBE_SPARK_STORAGE_DIR` from the environment — the one place the bridge reads the API key from the environment rather than the `Init` message — and it transacts against mainnet, so it is a local tool, not a CI check.
- **Liveness check**: `echo '{"type":"request","id":1,"method":"shutdown"}' | coincube-spark-bridge` prints a `shutdown` response and exits 0. No key, no mnemonic, no network — this is what the release workflows run against the packaged binary.
- **End-to-end journey**: create a cube → load Spark → receive sats via `@coincube.io` (should land on Spark) → send a BOLT11 payment → turn auto-convert on and off → verify everything in Transactions.

## References

- Breez Spark SDK rustdoc: <https://breez.github.io/spark-sdk/breez_sdk_spark/>
- Stable Balance guide: <https://sdk-doc-spark.breez.technology/guide/stable_balance.html>
- External signer guide (deferred follow-up): <https://sdk-doc-spark.breez.technology/guide/external_signer.html>
- Integration plan: `/.claude/plans/crystalline-booping-wadler.md`
- Wallet abstraction engineering note: [WALLETS.md](./WALLETS.md)
