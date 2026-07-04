# COINCUBE Desktop Wallet — Maintainability Audit (PASS 2, Remediation)

- **Repo:** `coincube` (Rust) · **Branch:** `feature/code-quality-audit` · **Date:** 2026-07-04
- Fixes for `audit/CODE-QUALITY.md`. No push.

## Fixed

| ID | What changed | Verification |
|----|--------------|--------------|
| CQ-DESK-001 | Deleted the dead `coincube-gui/src/launcher.rs` (uncompiled 3,298-line clone of `home.rs`); fixed a stale `see launcher.rs` doc pointer → `home.rs`. | Proven safe by reference check: `lib.rs` has no `mod launcher` and no `launcher::` reference exists. Removes ~40% of the repo's flagged duplication. |

## Deferred (with rationale)

The remaining Rust items are helper extractions inside `coincube-gui`, which **cannot be compiled in the audit sandbox** (the crate links native GTK/webkit via `gdk-sys`, unavailable here). Rather than refactor GUI code blind, these are deferred to a pass on a toolchain-equipped machine (`cargo check -p coincube-gui`, `cargo clippy`):

- **CQ-DESK-003 (Medium)** — extract `recovery_path_section(...)` shared by the two multisig recovery-path templates in `installer/view/editor/template/multisig_security_wallet.rs`.
- **CQ-DESK-004 (Low)** — extract an `intro_description(...)` helper for the three multisig `*_description` intros.

Two items touch `coincubed` (which *is* buildable) and can be done independently next:

- **CQ-DESK-002 (Medium)** — extract the shared coin→`UTxO` projection from the Electrum/Esplora `BitcoinInterface` impls in `coincubed/src/bitcoin/mod.rs`.
- **CQ-DESK-005 (Low)** — extract the shared 7-tx test fixture in `coincubed/src/database/sqlite/mod.rs`.

Note: the `update_coins_from_self` migration copy (`sqlite/utils.rs`) is intentional and was explicitly **left as-is** (documented migration snapshot).
