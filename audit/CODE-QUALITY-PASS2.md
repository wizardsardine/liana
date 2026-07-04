# COINCUBE Desktop Wallet — Maintainability Audit (PASS 2, Remediation)

- **Repo:** `coincube` (Rust) · **Branch:** `feature/code-quality-audit` · **Date:** 2026-07-04
- Fixes for `audit/CODE-QUALITY.md`. No push.

## Fixed

| ID | What changed | Verification |
|----|--------------|--------------|
| CQ-DESK-001 | Deleted the dead `coincube-gui/src/launcher.rs` (uncompiled 3,298-line clone of `home.rs`); fixed a stale `see launcher.rs` doc pointer → `home.rs`. | Proven safe by reference check: `lib.rs` has no `mod launcher` and no `launcher::` reference exists. Removes ~40% of the repo's flagged duplication. |
| CQ-DESK-002 | Extracted `received_coins_from` / `confirmed_coins_from` / `spending_coins_from` / `spent_coins_from` in `coincubed/src/bitcoin/mod.rs`; the Electrum and Esplora `BitcoinInterface` impls now delegate instead of duplicating 4 byte-identical bodies. | `cargo check` + `cargo fmt --check` + `cargo clippy --all-targets -D warnings` on `coincubed` all clean. |
| CQ-DESK-005 | Extracted `seed_txid_fixture(&mut conn)` for the two txid-listing tests in `coincubed/src/database/sqlite/mod.rs` (was ~110 duplicated lines each). | `cargo test -p coincubed sqlite_list` (both pass) + fmt + clippy clean. |
| CQ-DESK-003 | Extracted `second_recovery_path_section(use_taproot, recovery_path)` in `installer/view/editor/template/multisig_security_wallet.rs`; `multisig_inheritance_recovery_template` and `expanding_multisig_inheritance_template` now delegate instead of holding two byte-identical 48-line "Second recovery option" path blocks. | `cargo check -p coincube-gui` + `cargo clippy -p coincube-gui --all-targets -D warnings` + `cargo fmt` clean. |
| CQ-DESK-004 | Extracted `intro_description(progress, heading, body)` scaffold + `intro_paragraph(text)` helper in the same file; the three `*_description` intros now build from these instead of repeating the layout scaffold (×3) and the secondary-paragraph `Container` (×9). | Same GUI check/clippy/fmt run as CQ-DESK-003. |

## Toolchain note

The PASS-1 deferral of CQ-DESK-003/004 assumed `coincube-gui` **could not be compiled** here (native GTK via `gdk-sys`). On this machine `cargo check -p coincube-gui` and `cargo clippy -p coincube-gui --all-targets -- -D warnings` both build clean, so the two GUI extractions were completed and verified rather than deferred.

Note: the `update_coins_from_self` migration copy (`sqlite/utils.rs`) is intentional and was explicitly **left as-is** (documented migration snapshot).
