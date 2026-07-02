# COINCUBE Desktop Wallet — Pre-Launch Security Audit (PASS 2, Remediation)

- **Repo:** `coincube` (Rust; `coincube-core`, `coincubed`, `coincube-gui`)
- **Branch:** `feature/code-security-audit`
- **Date:** 2026-07-02
- **Scope:** Remediation of the findings recorded in `audit/AUDIT.md` (PASS 1).
- **Fix commit:** `824898c` — *security(audit pass 2): fix flagged coincube desktop findings*
- **Verification:** `coincube-core` builds and its unit tests pass. The new fee
  sanitizer and the BIP-32 derivation validation were type-checked/executed
  against the real `coincube-core` API. `coincube-gui` could not be built in the
  audit environment (missing native GTK/webkit); its two changed files were
  validated by extraction. No push.

| ID | Sev | Status | Remediation |
|----|-----|--------|-------------|
| CC-DESK-001 | Medium | Fixed | `fee_estimation.rs` now sanitizes every externally-supplied rate at the source: non-finite/non-positive values are dropped and excessive values clamped to `MAX_SANE_FEERATE` (5000 sat/vB) before averaging. Unit tests added. |
| CC-DESK-002 | Low | Fixed | `escrow.rs` validates each keyholder's account derivation path parses as a BIP-32 `DerivationPath` at seal time (`EscrowError::BadKeyholderDerivation`), failing closed rather than uploading an envelope the heir could never open. |
| CC-DESK-003 | Low | Fixed | `broadcast_spend` calls `spend::reverify_spend_before_broadcast` on the pre-finalization PSBT, re-checking outputs/absolute-fee/feerate (accepting either spending path) immediately before broadcast. |
| CC-DESK-004 | Info | No code change | Signer blindly signs by design; guarantee rests on the upstream confirm UX. Recommend a focused UX review that the confirm screen cannot be bypassed. |
| CC-DESK-005 | Info | No code change | Argon2id CRK params match the recorded tradeoff; consider a heavier recovery-password profile before wide launch. |

## Follow-ups (verify on a full toolchain)
- Run `cargo check -p coincube-gui` and the GUI test suite on a machine with the
  native GTK/webkit dependencies to confirm the two `coincube-gui` edits
  (`fee_estimation.rs`, `escrow.rs`) compile in-crate.
- `MAX_SANE_FEERATE` (5000 sat/vB) is a defensive ceiling; adjust if any
  legitimate flow could exceed it.
