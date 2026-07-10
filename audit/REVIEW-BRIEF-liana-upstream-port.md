# Review brief — Liana upstream port (Phases 1+2)

**For:** reviewing engineer. **Scope:** the six PRs from `plans/PLAN-liana-upstream-port.md`. **Background:** `plans/LIANA-UPSTREAM-AUDIT-2026-07.md`.
**Posture:** the code was AI-ported from upstream Liana across renamed crates and 9 months of divergence. Upstream's originals already went through Liana's own review — so the question is NOT "is this good code," it is **"did the port preserve upstream's semantics, and did it disturb anything of ours."** Review PR-by-PR against the cited upstream SHAs, not as one blob.

## Method: AI first pass, human judgment second

1. **AI pass (Claude Code, per PR):** fetch upstream (`git fetch liana master`), then for each ported change have it produce a semantic comparison of our commit vs the upstream original (`git show <upstream-sha>`), prompted adversarially: *"Try to prove this port changed behavior relative to upstream — different conditions, dropped hunks, resolved conflicts that altered logic."* Also have it run the cross-cutting checks below. Treat its output as leads, not verdicts.
2. **Human pass:** line-by-line on Tier 1, spot-check Tier 2, skim Tier 3 + everything the AI flagged. The items marked ⚑ below cannot be delegated to AI at all.

## Tier 1 — line-by-line human review

**PR 2, encrypted backup (highest priority).** Verify the crate pin is exactly `=0.0.2`. Verify the post-encrypt verification round-trip is present and **fails closed**: every spendable key decrypts ✓; the unspendable key and the BIP-341 NUMS point do **not** ✗ — and that a violation aborts the export with an error, not a log line. Confirm the NUMS negative test exists and actually asserts decryption *failure* (a test that passes vacuously is the trap). Run a full backup → restore round-trip yourself.

**PR 1, async-hwi 0.0.32.** Read the `Cargo.lock` delta in full — the bump should move `async-hwi`, `ledger_bitcoin_client`, `coldcard`, `hidapi`, `serialport`, `noise-protocol` and little else; anything additional needs an explanation. Check `hw.rs` compile fixes didn't alter enumeration/reconnect logic. ⚑ **Run the physical device matrix** (Ledger, Coldcard Edge ≥6.4.0, BitBox02, Specter): sign a Vault PSBT on each; on Coldcard, set up two descriptors sharing a key and confirm signing targets the right one; confirm a locked BitBox02 with taproot shows its pairing code.

**PR 3, dust 500.** `grep -rn DUST_OUTPUT_SATS` — review every use site, especially change-output logic (`spend.rs` ≈L385 `saturating_sub(1)` neighborhood) and coin selection. Demand boundary tests at 499/500/501 sats and a send-max-under-dust case. This changes what transactions we construct; it deserves paranoia.

## Tier 2 — targeted review

**PR 5a, ccxp parser.** It parses an untrusted file from disk. Check the failure paths: malformed JSON, missing `xfp`/`p2wsh_key_exp`, oversized input, wrong network — all should reject cleanly, never panic. Ask for a couple of malformed-input tests beyond upstream's happy-path assets.

**PR 5b, save-before-export.** Walk the PSBT state machine: no path exports unpersisted signing state, and no path *loses* a signature when the user declines the save.

## Tier 3 — skim

PR 4 (small fixes — confirm each matches its upstream diff, e.g. the `Length::Shrink` one-liner) and PR 6 (fiat — UA header present on all price requests; default only affects fresh installs, existing users' saved source untouched).

## Cross-cutting checks (AI-runnable, human-verified)

- `cargo tree | grep -E 'bitcoin|miniscript|secp256k1'` — byte-identical before/after the whole sprint. Non-negotiable.
- Full lockfile diff: **zero downgrades** anywhere (we are ahead of upstream on iced/rusqlite/etc.; a "sync" downward is a port error).
- `cargo audit` (or `cargo deny check advisories`) on the final branch; compare against pre-sprint baseline.
- Blast-radius check: `git diff master...<branch> --stat -- coincube-gui/src/services/duress coincube-gui/src/app/*/spark coincube-gui/src/app/*/liquid coincube-gui/src/services/connect grpc/` — should be **empty** (PR 6 fiat and any copy-sweep exceptions must be explicitly justified). No feature-flag or default-feature changes in any `Cargo.toml`.
- Clippy clean; no new `unwrap`/`expect` on user-input paths introduced by conflict resolution.

## Sign-off

Short note in `audit/` (house convention): per-PR verdict, device-matrix results, lockfile-diff attestation, and any deviations from upstream semantics found + how resolved. Estimated total effort: roughly a day — half of it on Tier 1 and the device bench.
