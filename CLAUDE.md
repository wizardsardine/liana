@AGENTS.md

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

COINCUBE Desktop (formerly COINCUBE | Tenshu; the OS shows it as "COINCUBE") — a desktop Bitcoin wallet (Rust, iced GUI) hard-forked from Wizardsardine's Liana (v13). Each **Cube** (PIN- or passkey-encrypted container) holds up to two wallets: a **Vault** (miniscript multisig with time-locked recovery paths, served by the `coincubed` daemon) and a **Spark** wallet (Breez Spark SDK, out-of-process). The former Liquid wallet has been removed; its on-disk data is left untouched (see `docs/WALLETS.md`). A server-side **Connect** account (the sibling `coincube-api` repo) adds auth/OTP, billing, Cube registration, multi-signer Keychain phone signing over gRPC, duress and recovery features.

## Build, lint, test

Toolchain is pinned by `rust-toolchain.toml` (1.97.1); `protoc` is required (`coincube-gui/build.rs` compiles `grpc/*.proto`). Linux also needs `libudev-dev pkg-config libdbus-1-dev libwebkit2gtk-4.1-dev libfontconfig1-dev`.

```bash
make build            # bridge + gui (debug)   — PREFER over raw cargo, see below
make run              # build bridge, then cargo run -p coincube-gui
make release          # both halves, --release (PROFILE=minimal is what releases use)
make test             # cargo test (root workspace) + bridge tests
make sync-proto       # copy ../coincube-api/grpc/connect.proto -> grpc/ (COINCUBE_API_PATH overrides)

cargo fmt -- --check
cargo clippy --all-targets -- -D warnings                                     # hard CI gate
cargo clippy --manifest-path coincube-spark-bridge/Cargo.toml --all-targets -- -D warnings
cargo test -p coincubed <name>                                                # single Rust test
cargo test --manifest-path coincube-spark-bridge/Cargo.toml
```

**`coincube-spark-bridge` is NOT in the root workspace** (`exclude` in `Cargo.toml`, own `Cargo.lock`): `breez-sdk-spark`'s `rusqlite`/`libsqlite3-sys` requirement can't share a dep graph with `coincubed`'s rusqlite 0.38 fork, so the Spark SDK runs in a sibling process. A bare `cargo build`/`cargo run -p coincube-gui` never builds it and the GUI then shows "Spark is not configured for this cube". The gui finds the binary via `COINCUBE_SPARK_BRIDGE_PATH`, next to its own exe, or `coincube-spark-bridge/target/{debug,release}/`.

### Functional (pytest) tests

Python blackbox tests boot a **release** `coincubed` against regtest bitcoind and drive it over JSON-RPC:

```bash
cargo build --release --package coincubed
(cd tests/tools/taproot_signer && cargo build --release)   # only for USE_TAPROOT=1
python3 -m venv venv && . venv/bin/activate && pip install -r tests/requirements.txt
pytest tests/ -n 8
pytest tests/test_rpc.py::test_startup -vvv --log-cli-level=DEBUG
```

Env knobs (`tests/test_framework/utils.py`): `COINCUBED_PATH` (default `target/release/coincubed`), `BITCOIND_PATH`, `BITCOIN_BACKEND_TYPE=bitcoind|electrs`, `ELECTRS_PATH`, `USE_TAPROOT=1` (needs bitcoind ≥26), `TIMEOUT`, `TEST_DIR` (failing tests keep their datadir there), `OLD_COINCUBED_PATH` (else `test_migration` skips). The four reorg tests in `test_chain.py` are run serially in CI with `TIMEOUT=240`.

Other test surfaces: `cargo test -p coincube-gui --features integration-tests` hits a real Connect API: production with a dedicated test account, or a local API (there is no dev server; needs `COINCUBE_API_URL` and `COINCUBE_INTEGRATION_TOKEN`; the connect-integration workflow is off unless enabled); `contrib/coverage.sh` / `contrib/coverage-bridge.sh` run cargo-llvm-cov; `fuzz/` is a cargo-fuzz crate over `coincube_core::descriptors`.

### `.env` is baked in at compile time

`coincube-gui/build.rs` loads the repo-root `.env` (gitignored; template `.env.example`) and re-emits every key as `rustc-env`, so `COINCUBE_API_URL`, `COINCUBE_CONNECT_GRPC_URL`, `BREEZ_API_KEY`, `COINCUBE_PASSKEY_RP_ID`, `COINCUBE_ENABLE_PASSKEY`, etc. are read via `env!`/`option_env!`. Changing `.env` requires a rebuild; `main.rs` also loads `.env` at runtime and runtime values win. Release builds panic at compile time if `COINCUBE_API_URL` is unset. Product feature flags are `const`s in `coincube-gui/src/feature_flags.rs` driven by these keys — they are not Cargo features (the only Cargo feature in the gui is `integration-tests`).

## Architecture

### Crates

| Crate | Role |
|---|---|
| `coincube-core` | Pure lib: `CoincubeDescriptor`/policy analysis (`descriptors/`), coin selection + spend creation (`spend.rs`), `MasterSigner` mnemonic signer, versioned encrypted seed file (`seed_crypt.rs`, Argon2id + AES-GCM), border-wallet grids. No network deps. |
| `coincubed` | Lib + bins `coincubed` and `coincube-cli`. The Vault engine (inherited Liana daemon): bitcoin backends, SQLite DB, poller, JSON-RPC over Unix socket. |
| `coincube-gui` | The iced desktop app (binary `coincube`). Embeds `coincubed` as a library. |
| `coincube-ui` | Design system: custom iced `Theme` (dark/light), widget type aliases bound to it (`widget::Element`), composed components, fonts/icons in `static/`. |
| `coincube-spark-protocol` | Serde wire types (`Request`/`Response`/`Event`/`Frame`) shared by gui and bridge. `u128` amounts travel as decimal strings. |
| `coincube-spark-bridge` | Standalone workspace; hosts breez-sdk-spark, speaks newline-delimited JSON on stdin/stdout. `Init` must be the first request. |

Root `Cargo.toml` carries one `[patch.crates-io]` entry, the vendored `async-hwi` (BitBox client) — read the comments there before touching dependencies. `coincubed`/`coincube-core` are still edition 2018.

### coincubed

`DaemonHandle::start(config, bitcoin, db, with_rpc_server)` in `coincubed/src/lib.rs` wires: data dir → SQLite (`database/sqlite/`, versioned schema) → a `BitcoinInterface` backend (`bitcoin/{bitcoind,electrum,esplora}`) → the poller thread (`bitcoin/poller/looper.rs`) → optionally the RPC server (`jsonrpc/server/unix.rs`). All commands are methods on `DaemonControl` in `commands/mod.rs`; `jsonrpc/api.rs` is a thin string dispatch over them (documented in `docs/API.md`). The handle is `Server` (standalone daemon) or `Controller` (what the GUI's `daemon/embedded.rs` uses — it calls `start_default(config, false)` and invokes `DaemonControl` directly). Both interfaces are injectable; `testutils.rs` has the fakes. The RPC socket path is `$TMPDIR/cc<hash-of-datadir>.sock` (`datadir.rs`), not inside the datadir. Config is TOML (`config.rs`, sample `contrib/coincubed_config_example.toml` — stale w.r.t. the Esplora/fallback fields).

### coincube-gui

- **Shell**: `gui/mod.rs` owns a pane-grid of tabs; each tab is `gui/tab.rs::State { Home, Installer, Loader, PinEntry, PasskeyUnlock, App, DuressActive }`. Pre-app screens live at crate root (`home.rs` = Cube launcher, `loader.rs`, `pin_entry.rs`, `passkey_unlock.rs`, `installer/`).
- **Elm split by layer, not screen**: `app/state/**` implements `trait State { view, update, subscription, reload, close }` (`app/state/mod.rs`); `app/view/**` is pure render functions. Subtrees mirror each other: `state/vault` ↔ `view/vault`, `spark`, `connect`. `app/message.rs::Message` (async results) wraps `app/view/message.rs::Message` (user intents). `app/mod.rs` holds `App` and the `Panels` registry; `app/menu.rs::Menu` is navigation. A Cube may have no Vault — vault panels are `Option`.
- **Daemon**: `daemon/mod.rs::trait Daemon` with `DaemonBackend::{EmbeddedCoincubed, ExternalCoincubed}`. `loader.rs` first tries to connect to an already-running daemon's socket (`daemon/client/jsonrpc.rs`), else starts the embedded one (and optionally a managed bitcoind/Knots node under `node/`).
- **Wallets**: `app/wallets/registry.rs::WalletRegistry` owns the optional Spark backend, routes Lightning-address payments to it, and exposes SDK-agnostic types (`wallets/types.rs`) so panels never see SDK types. `app/cube_signer.rs::CubeSigner` is the Cube master signer the seed-derived features (P2P identity, Connect blinding, Vault builder) read. `app/breez_spark/client.rs` spawns the bridge (`kill_on_drop`), with writer/reader/stderr tasks and an id→oneshot pending map. See `docs/WALLETS.md` for adding a wallet.
- **Cube & unlock**: `CubeSettings` in `app/settings/mod.rs`. There is no PIN hash: `services/unlock/` verifies a PIN by trial-decrypting the seed file (Argon2id m=256 MiB), with a timing-matched duress marker. Passkey Cubes have no seed file — the seed is re-derived from a WebAuthn PRF assertion (`services/passkey/`, macOS native; RP ID must match `coincube.io` entitlements). `app/seed_source.rs::SeedSource` abstracts both for the Cube signer and the Spark loader; `app/session.rs` caches the unlocked signer.
- **Connect / API**: `services/coincube/client.rs` is the REST client for coincube-api (auth, plans, Cubes, vault members, recovery kits, duress, Lightning addresses); `services/connect/grpc/` is the tonic client generated from `grpc/connect.proto` (streams signing sessions and duress events into iced subscriptions); `services/connect/crypto/` handles the encryption envelopes. `grpc/local_envelope.proto` is the LAN phone-signer transport (`phone_signer/`, `coincube-gui/PAIRING_PROTOCOL.md`). `grpc/connect.proto` is a vendored copy — `coincube-api` is the source of truth (`make sync-proto`). `docs/SIGNING_FLOW.md` covers the multi-signer flow end to end.
- **Datadir**: `~/.coincube` (Linux) or `<config_dir>/Coincube`; layout `<datadir>/<network>/data/<wallet_id>/`, `WalletId = "<descriptor_checksum>-<unix_ts>"` (legacy wallets without a timestamp sit directly in the network dir).

### Liana lineage

Identifiers were bulk-renamed (`LianaDescriptor`→`CoincubeDescriptor`, `lianad`→`coincubed`). Wizardsardine's hosted Liana Connect backend (`lianalite.com` auth + remote wallet store) has been removed: `services/connect/` is Coincube Connect only. Its `client/cache.rs` (`connect.json`) and `client/auth.rs::AccessTokenResponse` are Liana-era names now holding coincube-api tokens. `WalletSettings::remote_backend_auth` survives only to recognise legacy Liana Connect Vaults, which open to a delete-only notice pointing at the Liana app. The Wizardsardine key-provider token service (`keys.wizardsardine.com`, Safety Net / Cosigner tokens) has been removed too; `KeySetting::provider_key` / `settings::ProviderKey` survive only so existing Vaults that recorded such keys still load and label them. `CHANGELOG_COINCUBE.md` has an inherited-vs-added-vs-modified table if you need to know which subsystem came from upstream.

## Conventions and gotchas

- Commits: atomic, don't mix formatting/moves with logic; each commit must build clean and pass tests (CONTRIBUTING.md). New features need tests — functional (pytest) and/or unit.
- When bumping the Rust pin, also update every `toolchain: 1.x.y` in `.github/workflows/` (`git grep -n 'toolchain: [0-9]' .github/workflows/`); `coverage.yml` pins its own nightly and is exempt.
- Windows CI runs `cargo test --no-default-features`; `coincubed`/`coincube-cli` are Unix-only (Unix socket).
- Ubuntu unit tests need a Secret Service (gnome-keyring under `dbus-run-session`) for the device-secret tests; on macOS the `keychain-access-groups` entitlement means an unsigned/unprovisioned bundle is SIGKILLed at launch (`docs/MACOS_KEYCHAIN_ENTITLEMENT.md`).
- `plans/` (design docs for in-flight features) and `.claude/` are gitignored — don't expect them on other checkouts.
- Docs to open first: `docs/WALLETS.md` (wallet abstraction), `docs/SPARK_WALLET.md` (bridge lifecycle/packaging), `docs/SIGNING_FLOW.md` (Keychain signing), `docs/API.md` (daemon RPC), `docs/RELEASE.md` (release/verification).
