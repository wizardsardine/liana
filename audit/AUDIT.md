# COINCUBE Desktop Wallet — Pre-Launch Security Audit (PASS 1)

- **Repo:** `/Users/allen/git/coincubetech/coincube` (Liana-derived, self-custodial Bitcoin wallet)
- **Branch / commit:** `feature/code-security-audit` @ `0d8f9ae36e4ae5a01da6fbc87698651f6708d2ff`
- **Date:** 2026-07-02
- **Auditor:** senior security auditor (audit-only; no source modified)
- **Launch target:** 2026-08-08, feature freeze

## Scope

Funds-critical paths (PSBT construction/signing, descriptor generation/validation,
address/change derivation, fee calc, coin selection, RBF/CPFP, broadcast, Liquid/Spark
and Buy/Sell flows, amount/unit parsing); Cube Recovery Kit (Argon2id password CRK) and
time-locked Vault recovery (ECIES owner-self/heir escrow, `seal_to_xpub`, key-wrap,
restore paths, miniscript timelock semantics, duress interactions); and secret handling
(zeroization, logging, at-rest encryption).

## Methodology

Grounded in the company-brain architecture/decision records and `SPEC-ecies-v1.md`, then
mapped the crate layout (`coincube-core`, `coincubed`, `coincube-gui`) and read the
funds- and recovery-critical modules in full, cross-checking each against the four binding
invariants (I1 server-blind, I2 recovery key encryption-only, I4 ECIES byte-identical to
spec, I5 additive CRK). Two funds/descriptor sub-sweeps were run and every candidate
finding was re-verified against source before inclusion — several sub-agent findings were
**discarded as misgrounded** (see "Discarded candidates"). All findings below carry a
verified `file:line` citation.

---

## Findings

Overall this is a strong, defensively-written codebase. The ECIES and CRK crypto matches
the spec byte-for-byte (KAT-tested), the funds core is inherited from hardened Liana code
with checked integer arithmetic throughout, and invariants I1/I2/I4/I5 hold in the code
read. **No Critical or High findings.** The items below are Medium and lower.

### Medium

**CC-DESK-001 — Fee-estimator ingests unbounded/`NaN` external floats without validation**
Severity: Medium
Location: `coincube-gui/src/services/feeestimation/fee_estimation.rs:41-80`, consumed at `:19-32`
Description: `estimate_fees` deserializes fee rates as `f64` straight from `mempool.space`
and `blockstream.info` and averages them with no bounds/sanity check (no `> 0`, no upper
cap, no `is_finite`). The public getters return `fee.round().max(1.0) as usize`.
Impact: In practice this is contained — `NaN.max(1.0)` yields `1.0`, negatives clamp to 1,
and a pathologically high rate is later rejected by `create_spend`'s `MAX_FEERATE` (1000
sat/vb) guard in `coincube-core/src/spend.rs`. But the estimator itself performs no
defense-in-depth validation of network-provided data used in tx construction; a future
consumer that skips the `MAX_FEERATE` gate (e.g. the Liquid path at
`app/state/liquid/transactions.rs:689-695`) would inherit an unvalidated upstream rate.
Recommendation: Reject non-finite / non-positive rates in `estimate_fees` and clamp to a
sane ceiling (e.g. ≤ a few thousand sat/vb) at the source rather than relying on each
consumer.

### Low

**CC-DESK-002 — Escrow envelope `derivation` is built by string concat of a server-supplied path**
Severity: Low
Location: `coincube-gui/src/services/inheritance/escrow.rs:120,150`
Description: `full_derivation = format!("{}/{}", kh.account_derivation, ENCRYPTION_CHILD_INDEX)`
where `account_derivation` comes from the Connect vault response (`key.derivation_path`).
The `derivation` field is metadata only (not in the AAD; the actual key binding is
`CKDpub(xpub, 7000)` driven by the xpub), so a tampered path **cannot** re-target
ciphertext or break server-blindness — it fails closed at Keychain (heir derives wrong `d`,
tag fails). This is an availability/robustness note, not a confidentiality issue.
Impact: A malformed or malicious `derivation_path` from the server silently yields
undecryptable escrow for that keyholder, discoverable only at recovery time.
Recommendation: Validate `account_derivation` parses as a `DerivationPath` and matches the
keyholder xpub's registered account depth at seal time, and surface a hard error rather
than uploading an envelope that can never open.

**CC-DESK-003 — `broadcast_spend` re-validation is an acknowledged TODO**
Severity: Low
Location: `coincubed/src/commands/mod.rs` (broadcast path; `extract_tx_unchecked_fee_rate`, ~`:857`)
Description: The broadcast path relies on checks performed at spend-creation time and the
code comments the belt-and-suspenders re-check as an explicit TODO. Inherited Liana
behavior; the PSBT is validated by `sanity_check_psbt` (`coincube-core/src/spend.rs:100-173`)
at creation.
Impact: If in-flight PSBT state were mutated between creation and broadcast, no second
output/destination re-verification occurs before the tx hits the network. Low likelihood
given the daemon-owned state, but a defense-in-depth gap on a funds-critical path.
Recommendation: Add the noted re-verification of outputs/feerate immediately before
broadcast.

### Informational

**CC-DESK-004 — Signer blindly signs any PSBT by design**
Severity: Informational
Location: `coincube-core/src/signer.rs:595` (documented), `sign_p2wsh`/`sign_taproot`
Description: The signer explicitly performs no output/amount validation ("It will blindly
sign anything that's passed"). This is the correct layering — validation lives upstream in
`sanity_check_psbt` and the GUI confirm step — but it means the phishing-resistance
guarantee rests entirely on the UI presenting the correct, verified destination/amount to
the user before signing. Worth a focused UX review that the confirm screen cannot be
bypassed or spoofed.

**CC-DESK-005 — Argon2id CRK params default to the PIN-hash profile**
Severity: Informational
Location: `coincube-gui/src/services/recovery/envelope.rs:57-62` (`DEFAULT_V1`: m=19456, t=2, p=1)
Description: The recovery-password KDF reuses the PIN-hashing Argon2id profile. The envelope
is self-describing and versioned so params can be bumped without breaking existing kits,
but a recovery *password* has a different threat model than a device-local PIN and this
profile is on the low side for an offline-bruteforceable hosted blob. Matches the known
tradeoff recorded in the CRK decision doc.
Recommendation: Benchmark and consider a heavier profile (higher `t_cost`/`memory_kib`) for
the recovery-password path before wide launch.

---

## Positive observations

- **ECIES (`services/inheritance/ecies.rs`) is spec-exact and KAT-tested.** Compressed-SEC1
  raw-point ECDH (correctly *not* libsecp's hashed default, `:13`/`:229-239`), HKDF-SHA256
  with 32-byte zero salt, AES-256-GCM, domain-separated labels for envelope vs. §4b key-wrap,
  `cube_id`/`keyholder_key_id`/`artifact_kind` bound in AAD, and the §7.1/§7.2 known-answer
  vectors replayed in-tree. Fail-closed on wrong key / tampered CT / tampered AAD /
  unsupported scheme / malformed wrap. **I4 holds.**
- **I2 holds:** the recovery/encryption key is a dedicated non-hardened child at index
  `7000` (`ecies.rs:77`), derived xpub-only, and never enters the descriptor/quorum/signing
  path. `with_added_key` appends without changing the threshold
  (`descriptors/analysis.rs`). `owner_self.rs:23-25` restates and enforces the rule.
- **CRK envelope (`services/recovery/envelope.rs`):** AES-256-GCM + Argon2id, 6-byte header
  bound as AAD, per-call random salt+nonce, version/kdf downgrade rejected, plaintext in
  `Zeroizing`, KAT wire-format pin test. Fail-closed and indistinguishable wrong-password vs.
  tamper (`BadPasswordOrCorrupt`).
- **Descriptor handling:** three-layer validation on import (miniscript parse → `sanity_check`
  → policy extraction), CSV recovery semantics correct per BIP68 (`>= timelock`, tested),
  u16 timelock clamp prevents nSequence flag confusion, multipath/origin/wildcard key checks,
  duplicate-key rejection, network consistency checks.
- **Funds core:** checked `checked_mul`/`checked_add`/`checked_sub`/`checked_div` throughout
  spend and RBF; RBF min-feerate computed with integer arithmetic (no float rounding)
  (`coincubed/src/commands/mod.rs:1066-1098`); amount parser rejects scientific notation and
  overflows to `None` (`breez_liquid/assets.rs:177-213`); network-checked addresses.
- **Duress at-rest cipher (`services/duress/cipher.rs`):** ChaCha20-Poly1305 under a device
  key deliberately separated from Cube material and kept outside the wiped tree; `0o600`
  perms; `load` never mints a key (avoids clobbering sealed activations). Client honors the
  server `423 DURESS_LOCKED` gate with plausibly-deniable UX (`recovery/restore.rs:53-96`),
  consistent with I3.
- **Secret hygiene:** no secret values found interpolated into logs (only status strings
  like "mnemonic stored"); `SeedBlob`/`DescriptorBlob`/`Envelope`/`TransportKeypair` all have
  redacting manual `Debug` impls; secrets held in `Zeroizing`/`ZeroizeOnDrop`.

## Discarded candidates (verified NOT issues)

Re-checked against source and rejected as misgrounded (wrong line numbers / claimed
constructs absent):
- "RBF feerate uses f64 with precision loss" — the RBF path is pure integer arithmetic
  (`commands/mod.rs:1066-1098`); the cited lines were unrelated (`start_rescan`).
- "Unchecked integer overflow in amount parsing" — the real parser
  (`assets.rs:177-213`) uses `checked_mul`/`checked_add`; the cited lines were test code.
- Descriptor sub-sweep returned zero defects and confirmed I2 / BIP68 CSV correctness.

## Coverage

**Read in full:**
`services/inheritance/ecies.rs`, `services/recovery/envelope.rs`, `services/recovery/restore.rs`
(header + error taxonomy), `services/recovery/plaintext.rs` (blob types), `services/inheritance/escrow.rs`,
`services/inheritance/owner_self.rs` (header), `services/duress/cipher.rs`,
`services/feeestimation/fee_estimation.rs`, `coincube-core/src/descriptors/{mod,analysis,keys}.rs`,
`coincube-core/src/spend.rs`, `coincube-core/src/signer.rs` (signing paths),
`coincubed/src/commands/mod.rs` (create_spend / RBF / broadcast / rescan sections).

**Skimmed / grepped:**
`coincube-gui/src/installer/{mod,descriptor}.rs`, `app/breez_liquid/*`, `app/breez_spark/*`,
`app/state/{liquid,spark,vault}/*`, `services/{meld,mavapay}/*`, `services/duress/*` (beyond cipher),
`services/recovery/{keyholder,owner_keychain,password}.rs`, `services/inheritance/{owner,heir,wire}.rs`,
secret-logging grep across all `*.rs`.

**Not reviewed (out of scope / suggest PASS 2):**
gRPC signing session state machine and stream auth (`services/connect/grpc/*`), LAN
phone-signer TLS/pairing (`phone_signer/*`), passkey/WebAuthn webview bridge, update/download
integrity (`download.rs`), full dependency/supply-chain review of BDK/Breez/Breez-Spark.
