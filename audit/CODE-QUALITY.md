# COINCUBE Desktop Wallet — Maintainability Audit (PASS 1)

- **Repo:** `coincube` (Rust; `coincube-core`, `coincubed`, `coincube-gui`, `coincube-ui`)
- **Branch:** `feature/code-quality-audit`
- **Date:** 2026-07-04
- **Scope:** Redundant/duplicated code and style/consistency (maintainability, not correctness/security). Findings only — no code changed.
- **Method:** `jscpd` copy-paste detection (min 12 lines / 70 tokens, generated/target/fonts excluded), `cargo fmt --all --check`, plus manual verification of every cited block.

## Summary

Overall **duplication is ~5%** and, unusually, is dominated by a **single dead file** rather than pervasive copy-paste. Style is in good shape: CI already enforces `cargo fmt -- --check` **and** `cargo clippy --all-targets -- -D warnings` (`.github/workflows/main.yml`), and the tree is rustfmt-clean — so there are **no formatting findings**. The items below are structural duplication.

## Findings

### High

**CQ-DESK-001 — `launcher.rs` is a 3,298-line dead near-clone of `home.rs`**
Location: `coincube-gui/src/launcher.rs` (whole file) vs `coincube-gui/src/home.rs` (whole file).
`launcher.rs` is a stale wholesale copy of `home.rs` with types mechanically renamed (`Home`→`Launcher`). It is **not compiled**: `coincube-gui/src/lib.rs:11` declares `pub mod home;` and there is **no `mod launcher;`** anywhere; no `launcher::` reference exists outside the file itself. It accounts for essentially the entire top of the jscpd report (`home.rs:359-1389` == `launcher.rs:316-1346` = 1,031 lines, plus ~8 more 90–460-line blocks). It has already **drifted** — `home.rs` carries the RecoverVault / RecoverOwnCube inheritance feature (~21 refs) that `launcher.rs` lacks entirely.
Impact: ~3,300 dead lines that inflate the codebase and are a real "edit the wrong file" hazard (grep hits both).
Recommendation: **Delete `coincube-gui/src/launcher.rs`.** This one change removes ~40% of all flagged duplication.

### Medium

**CQ-DESK-002 — Electrum & Esplora `BitcoinInterface` impls duplicate the coin-accessor bodies**
Location: `coincubed/src/bitcoin/mod.rs` — `impl BitcoinInterface for electrum::Electrum` (~L466) vs `for esplora::Esplora` (~L653); jscpd `483-572` == `672-759`, and the neighbouring `received_coins`/`confirmed_coins`/`spending_coins`/`spent_coins` bodies are near-identical, each delegating to `self.wallet_coins(None)` with the same filter/map into `UTxO`.
Recommendation: Extract the shared coin→UTxO projection into a free helper (`fn coins_to_utxos(...)`) or a default trait method, and have both backends delegate. Keeps the two backends from silently diverging (~90 lines).

**CQ-DESK-003 — Two multisig recovery-path view builders duplicate the `path(...)` section**
Location: `coincube-gui/src/installer/view/editor/template/multisig_security_wallet.rs` — block `530-615` (in `multisig_inheritance_recovery_template`) == `749-834` (in `expanding_multisig_inheritance_template`); identical except the `KeysEdit` message mapping.
Recommendation: Extract `recovery_path_section(recovery_path, use_taproot, on_edit: impl Fn(..) -> Message)` and call from both templates.

### Low

**CQ-DESK-004 — Multisig `*_description` intro builders are structurally duplicated**
Location: `multisig_security_wallet.rs` — `multisig_security_template_description` (L27), `multisig_inheritance_recovery_description` (L96), `expanding_multisig_inheritance_recovery_description` (L338) share the same `layout(progress, None, "Introduction", Column…)` scaffold, differing mostly in copy strings.
Recommendation: Extract an `intro_description(progress, heading, body, …)` helper. Low churn / low urgency.

**CQ-DESK-005 — Duplicated inline test fixture**
Location: `coincubed/src/database/sqlite/mod.rs:2290-2399` (`sqlite_list_txids`) vs `2414-2523` (`sqlite_list_all_txids`) build the identical 7-tx / Coin fixture inline.
Recommendation: Extract a shared test fixture helper. Test-only.

## Positive observations / explicitly NOT issues

- **Style is enforced and clean.** `cargo fmt -- --check` and `clippy -D warnings` gate CI; `cargo fmt --all --check` reports no diffs. No formatting/lint findings.
- **`update_coins_from_self` copy in the V7→V8 migration is intentional.** `coincubed/src/database/sqlite/utils.rs:404-475` duplicates `mod.rs:794` deliberately, with a comment explaining migrations must be pinned to the historical schema. **Do not DRY this** — it would defeat the migration-snapshot intent. (Optional: add a one-line cross-reference comment on the prod method.)
- Funds-core (`coincube-core`) showed no material duplication.

## Coverage

jscpd across the workspace (excluding `target/`, generated, `fuzz/`, `coincube-ui/static/fonts/*`); `cargo fmt --all --check`; manual verification of the `home.rs`/`launcher.rs` module wiring (`lib.rs`), the `bitcoin/mod.rs` impls, and the multisig template blocks. Not deeply reviewed: `coincube-spark-*` crates, `coincube-ui` static assets.
