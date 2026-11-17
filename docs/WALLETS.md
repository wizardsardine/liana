# Wallets

COINCUBE ships two wallets today: a Bitcoin **Vault** and a **Spark** wallet (`breez-sdk-spark`). This note explains how they fit together and how to add another.

The Cube used to hold a third, **Liquid** wallet (`breez-sdk-liquid`, in-process). It was hidden in Tenshu v175 and removed from the code in the first point release after launch (plan `company-brain/plans/liquid-removal/`). Nothing on disk was deleted: an old Liquid working directory (`<datadir>/breez/`) and legacy `-liquid_` seed files stay where they are, unread. The same Cube seed still derives the same keys, so Liquid funds remain recoverable from the seed in another wallet.

A Vault may use a newly configured inheritance policy or an imported single-signature/multisignature Bitcoin descriptor. Both use the same GUI `Wallet`, App daemon lifecycle and Vault navigation. Policy-specific capabilities determine signing and recovery actions; the imported wallet retains its external signers. See [BITCOIN_WALLET_IMPORT.md](./BITCOIN_WALLET_IMPORT.md).

## Layout

```text
coincube-gui/src/app/
├── cube_signer.rs             ← the Cube master signer (P2P identity, Connect key, Vault builder)
├── wallets/                   ← domain types + registry
│   ├── mod.rs                 pub-use surface
│   ├── types.rs               DomainPaymentStatus
│   ├── registry.rs            WalletRegistry + LightningRoute
│   └── spark.rs               SparkBackend (wraps SparkClient)
├── breez_spark/               ← Spark SDK wrapper (subprocess)
│   ├── mod.rs                 loader (load_spark_client)
│   ├── config.rs              SparkConfig::from_env
│   ├── client.rs              SparkClient — spawns + owns the bridge subprocess
│   └── assets.rs              Spark asset registry (BTC + USDB)
├── state/
│   ├── vault/                 Vault panels
│   └── spark/                 SparkOverview, SparkSend, SparkReceive, …
└── view/
    ├── vault/                 view renderers
    └── spark/                 view renderers
```

Separately: `coincube-spark-bridge/` (sibling binary, own Cargo workspace) and `coincube-spark-protocol/` (shared wire types).

## The Cube master signer

`app/cube_signer.rs::CubeSigner` is the Cube's master signer, resolved once at unlock from `SeedSource::resolve` (PIN Cubes keep the session-cache fast path; passkey Cubes use their in-memory seed; a seed-less Cube has none). The seed-derived features — the P2P/Mostro identity, the Connect-blinding `CubeEncryptionKey` and the Vault builder's copy of that key — read it. Its golden-vector tests pin every derived value.

## The abstraction layer

`wallets/types.rs` defines SDK-agnostic domain types. Today that is **`DomainPaymentStatus`**, the composite status the UI distinguishes. Panels never import `coincube_spark_protocol::*` types for display beyond the summaries the Spark panels map at their boundary.

## WalletRegistry

`wallets/registry.rs` owns the per-cube wallet backends and exposes routing hooks:

```rust
pub struct WalletRegistry {
    spark: Option<Arc<SparkBackend>>,
}
```

- `spark` is `Some` only when the cube has a `master_signer_fingerprint` **and** the bridge subprocess spawned successfully. Panels that need Spark gate their UI on `WalletRegistry::spark().is_some()`. A Bitcoin Blake2b Cube never has one.

The registry is also the single place the app decides which backend handles which payment type. Today it exposes one routing method:

```rust
pub fn route_lightning_address(&self) -> Option<LightningRoute>
```

It returns `LightningRoute::Spark(Arc<SparkBackend>)` when the bridge is up, otherwise `None`. When future routing decisions arise, add them as `route_*` methods on `WalletRegistry` so the policy stays in one file.

## Spark wrapper shape

`breez_spark/` is async/IPC — `SparkClient` spawns a sibling binary and JSON-RPCs over stdio. Cheap operations round-trip in a few ms; expensive ones live in the bridge. The bridge holds the mnemonic in its own address space. There is deliberately no shared `WalletBackend` trait: extract one only when a second wallet backend appears and the common surface is visible empirically.

## Settings plumbing

Per-cube settings live in `coincube-gui/src/app/settings/mod.rs::CubeSettings`. Wallet-relevant fields:

- `master_signer_fingerprint: Option<Fingerprint>` — identifies the Cube MasterSigner for Spark, the seed-derived features and eligible locally created Vault signing. Imported Vault policies retain their original external signers; the Cube seed cannot sign their Bitcoin transactions. Its serde aliases (`liquid_wallet_signer_fingerprint`, `breez_wallet_signer_fingerprint`) keep older settings files readable.
- `pending_spark_shift` — a live Spark cross-network receive (SideShift), so its page survives a lock or restart.

`CubeSettings` does not set `deny_unknown_fields`: keys an older build wrote (for example the retired `liquid_granted` and `pending_liquid_to_vault_transfer`) are ignored on read.

## Events

The Spark backend has its own iced subscription and message variant, `Message::SparkEvent(SparkClientEvent)` — bridge events (`Synced`, `PaymentSucceeded { id, amount_sat, bolt11 }`, `DepositsChanged`, …).

## Adding another wallet

1. **Create a protocol crate** if the new SDK can't live in-process (dep-graph conflicts, WASM/non-WASM split, etc.). Mirror `coincube-spark-protocol` + `coincube-spark-bridge`.
2. **Add a new module** under `coincube-gui/src/app/<name>/` that wraps the client and handles config / load / shutdown.
3. **Add a new backend** under `coincube-gui/src/app/wallets/<name>.rs` that exposes the panel-facing read/write surface, mapping SDK types to domain types at the boundary.
4. **Extend `WalletRegistry`** with a new field + getter + routing-method updates.
5. **Add a Menu variant** in `app/menu.rs` (`Menu::<Name>(<Name>SubMenu)`) and its entries in the sidebar (`app/view/nav/`).
6. **Create parallel `state/<name>/` and `view/<name>/`** trees with Overview / Send / Receive / Transactions / Settings panels. Copy the Spark panels as a starting point.
7. **Derive keys from the master seed.** Use `master_signer_fingerprint` (or the unlocked `CubeSigner`) to load the shared `MasterSigner` and derive wallet-specific keys at a dedicated BIP-32 path. This applies to Cube-derived backends; imported Vault policies keep their own public signer origins and never substitute the Cube seed.
8. **Wire events** into `App::subscription` and `App::update` under a new `Message::<Name>Event` variant.
9. **Update routing rules** in `WalletRegistry::route_*` methods so the new backend participates where appropriate.
10. **Update docs** — add a `docs/<NAME>_WALLET.md` mirroring [SPARK_WALLET.md](./SPARK_WALLET.md), and extend this file's Layout section.

## References

- [SPARK_WALLET.md](./SPARK_WALLET.md) — Spark-specific setup, architecture, and feature notes.
