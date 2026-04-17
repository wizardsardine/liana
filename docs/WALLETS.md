# Wallets

COINCUBE ships three wallets today: a multisig Bitcoin **Vault**, a **Liquid** wallet (`breez-sdk-liquid`), and a **Spark** wallet (`breez-sdk-spark`). This note explains how the three fit together and how to add a fourth.

## Layout

```text
coincube-gui/src/app/
├── wallets/                   ← domain types + registry
│   ├── mod.rs                 pub-use surface
│   ├── types.rs               WalletKind, DomainPayment*, DomainRefundableSwap
│   ├── registry.rs            WalletRegistry + LightningRoute
│   ├── liquid.rs              LiquidBackend (wraps BreezClient)
│   └── spark.rs               SparkBackend (wraps SparkClient)
├── breez_liquid/              ← Liquid SDK wrapper (in-process)
│   ├── mod.rs                 loader (load_breez_client)
│   ├── config.rs              BreezConfig::from_env
│   ├── client.rs              BreezClient — full Liquid read/write API
│   └── assets.rs              L-BTC / USDt descriptors
├── breez_spark/               ← Spark SDK wrapper (subprocess)
│   ├── mod.rs                 loader (load_spark_client)
│   ├── config.rs              SparkConfig::from_env
│   ├── client.rs              SparkClient — spawns + owns the bridge subprocess
│   └── assets.rs              Spark asset registry (BTC-only today)
├── state/
│   ├── liquid/                LiquidOverview, LiquidSend, LiquidReceive, …
│   └── spark/                 SparkOverview, SparkSend, SparkReceive, …
└── view/
    ├── liquid/                view renderers
    └── spark/                 view renderers
```

Separately: `coincube-spark-bridge/` (sibling binary, own Cargo workspace) and `coincube-spark-protocol/` (shared wire types).

## The abstraction layer

`wallets/types.rs` defines SDK-agnostic domain types:

- **`WalletKind`** — `Spark` (default) or `Liquid`.
- **`DomainPayment`** — the shape the UI consumes. Backends map SDK-native payment types (`breez_sdk_liquid::Payment`, `coincube_spark_protocol::PaymentSummary`, …) into `DomainPayment` at the boundary.
- **`DomainPaymentDetails`** — enum of `Lightning`, `LiquidAsset`, `OnChainBitcoin` (add variants as new payment shapes land).
- **`DomainPaymentStatus`** / **`DomainPaymentDirection`** — composite status + direction normalized across backends.
- **`DomainRefundableSwap`** — Liquid-specific refundable-swap summary (Spark has no equivalent today).

Panels **never** import `breez_sdk_liquid::*` or `coincube_spark_protocol::*` types for display — they go through the domain layer. The backend crates own the mapping functions (`impl From<LiquidPayment> for DomainPayment`, etc.).

## WalletRegistry

`wallets/registry.rs` owns the per-cube wallet backends and exposes routing hooks:

```rust
pub struct WalletRegistry {
    liquid: Arc<LiquidBackend>,
    spark: Option<Arc<SparkBackend>>,
}
```

- `liquid` is always present — the Liquid SDK is in-process and initialized at cube unlock.
- `spark` is `Some` only when the cube has a `master_signer_fingerprint` **and** the bridge subprocess spawned successfully. Panels that need Spark gate their UI on `WalletRegistry::spark().is_some()`.

The registry is also the single place the app decides which backend handles which payment type. Today it exposes one routing method:

```rust
pub fn route_lightning_address(&self, preferred: WalletKind) -> LightningRoute
```

Returns `LightningRoute::Spark(Arc<SparkBackend>)` when the cube prefers Spark and the bridge is up, otherwise `LightningRoute::Liquid(Arc<LiquidBackend>)`. The LNURL subscription site consults this on every incoming invoice request; `default_lightning_backend` flips are a one-cube-setting change that takes effect on the next event, no code change needed.

When future routing decisions arise (BOLT12 → Liquid, cross-chain → SideShift via Liquid, etc.), add them as `route_*` methods on `WalletRegistry` so the policy stays in one file.

## Two wallet wrapper shapes

The Liquid and Spark wrapper crates deliberately don't share a `WalletBackend` trait:

- **Liquid** (`breez_liquid/`) is sync/local — `BreezClient` holds `Arc<LiquidSdk>` directly, implements the `breez_sdk_liquid::Signer` trait through a `MasterSignerAdapter` so the mnemonic never leaves the MasterSigner, and exposes a rich set of methods including swap refunds, L-BTC/USDt asset handling, and LNURL fulfillment via `receive_lnurl_invoice(amount_sat, description_hash)`.
- **Spark** (`breez_spark/`) is async/IPC — `SparkClient` spawns a sibling binary and JSON-RPCs over stdio. Cheap operations round-trip in a few ms; expensive ones live in the bridge. The bridge holds the mnemonic in its own address space.

A premature trait would paper over those differences. Instead, `WalletRegistry` is the enum-dispatch site: callers that need "a backend" branch on `WalletKind` and pick a concrete handle, and the domain types in `wallets/types.rs` carry the shared UI-facing shape. Extract a trait only when a **third** backend appears and you can see the common surface empirically — not before.

## Settings plumbing

Per-cube settings live in `coincube-gui/src/app/settings/mod.rs::CubeSettings`. Spark-relevant fields:

- `master_signer_fingerprint: Option<Fingerprint>` — identifies the MasterSigner that drives all wallets (Vault, Liquid, Spark). All wallet backends derive keys from this single master seed at different BIP-32 paths.
- `default_lightning_backend: WalletKind` — cube-level override for which backend fulfills incoming Lightning Address invoices. Serde default is `Spark` post-Phase-5.

The `Cache` struct mirrors `default_lightning_backend` and `cube_id` so panels can read them without threading `CubeSettings` through the `State::update(daemon, cache, message)` signature. The authoritative copy lives on `App::cube_settings` and is re-read from disk on `Message::SettingsSaved`.

## Events

Each backend has its own iced subscription + message variant:

- `Message::LiquidEvent(BreezClientEvent)` — in-process Liquid SDK events.
- `Message::SparkEvent(SparkClientEvent)` — bridge events (`Synced`, `PaymentSucceeded { id, amount_sat, bolt11 }`, `DepositsChanged`, …).

Liquid and Spark events are **not** unified into a generic `WalletEvent`. Unification makes sense only once a third backend arrives and shared handlers emerge empirically.

## Adding a third wallet

If you're wiring up e.g. `breez-sdk-greenlight` or a Nostr Wallet Connect client:

1. **Create a protocol crate** if the new SDK can't live in-process (dep-graph conflicts, WASM/non-WASM split, etc.). Mirror `coincube-spark-protocol` + `coincube-spark-bridge`.
2. **Add a new module** under `coincube-gui/src/app/breez_<name>/` (or `nwc/`, etc.) that wraps the client and handles config / load / shutdown.
3. **Add a new backend** under `coincube-gui/src/app/wallets/<name>.rs` that maps the SDK's payment types to `DomainPayment` and exposes the panel-facing read/write surface.
4. **Extend `WalletKind`** in `wallets/types.rs` and make sure `Default` still points at the right wallet for new cubes.
5. **Extend `WalletRegistry`** with a new field + getter + routing-method updates.
6. **Add a Menu variant** in `app/menu.rs` (`Menu::<Name>(<Name>SubMenu)`) and sidebar buttons in `app/view/mod.rs`.
7. **Create parallel `state/<name>/` and `view/<name>/`** trees with Overview / Send / Receive / Transactions / Settings panels. Copy the Spark panels as a starting point — they're the most abstracted of the three today.
8. **Derive keys from the master seed.** The new wallet should use `master_signer_fingerprint` to load the shared `MasterSigner` and derive wallet-specific keys at a dedicated BIP-32 path. Do not add a separate signer fingerprint field — all wallets share one master seed. Add any wallet-specific config (e.g., storage path, feature toggles) to `CubeSettings` and the corresponding `Cache` mirror if panels need it.
9. **Wire events** into `App::subscription` and `App::update` under a new `Message::<Name>Event` variant.
10. **Update routing rules** in `WalletRegistry::route_*` methods so the new backend participates where appropriate.
11. **Update docs** — add a `docs/<NAME>_WALLET.md` mirroring [SPARK_WALLET.md](./SPARK_WALLET.md), and extend this file's Layout section.

Resist the temptation to extract a shared panel component on the first pass. Wait until the third wallet is working and the genuine duplication is visible, then extract. Extracting prematurely couples multiple still-moving targets.

## References

- [SPARK_WALLET.md](./SPARK_WALLET.md) — Spark-specific setup, architecture, and feature notes.
- [BREEZ_SDK_REGTEST.md](./BREEZ_SDK_REGTEST.md) — Liquid regtest harness.
- `/.claude/plans/crystalline-booping-wadler.md` — full Spark integration plan with phase-by-phase rollout.
