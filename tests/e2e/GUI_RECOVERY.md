# Desktop Recovery Kit GUI suite — draft, blocked

This is a separate pre-broadcast suite for Bitcoin Testnet4 and Bitcoin Blake2b
Testnet4. It is **not currently a runnable real GUI acceptance test**. PR #1174
(head `9e9a1f25995b4be9a6ab188240a2bf92d5f3209b`) is not integrated in the
pinned source. The session-bound `CuaWorker` adapter, fixture-only Keychain
isolator and approved two-chain funding integration are not implemented. The
CLI exits BLOCKED instead of accepting imported receipts or executing a
user-supplied driver command. Offline selftests never establish GUI acceptance.

The product MCP interface does not provide this full GUI lifecycle. Native
proposal execution, synthetic wallet UTXOs and direct signer calls cannot replace
GUI actions. All future desktop UI interaction must use the supported CUA
session API; this suite does not authorize AppleScript or CGEvent substitutes.

## Intended invocation and gates

Copy `gui.example.json` into protected local storage, pin the exact reviewed
source/app receipt and use a new private run directory:

```sh
tests/e2e/run --qa-gui-recovery --qa-gui-config /private/gui.json --qa-gui-run-dir /private/gui-run
```

`--qa-flow` requires both GUI flags plus the existing strict mobile `--qa-config`
and `--qa-run-dir`. It runs this GUI suite first, stops on any nonzero result,
and only then invokes the unchanged strict mobile flow. Direct `--qa-fresh`
continues to select the mobile suite independently. GUI and mobile run directories
must be separate and not nested. GUI cases never broadcast; this does not change
the mobile flow's separately authorized broadcast acceptance.

The hardcoded prerequisite is deliberately conservative: a squash/replacement
merge requires a reviewed prerequisite update. Missing dependencies, drivers,
network access or recognized funding remain BLOCKED, never PASS.

## Required real worker implementation

The reserved adapter has no callable CLI override or receipt-import interface.
Before enabling it, implement and independently review these requirements:

* Bind a fresh coordinator challenge to one live CUA session, exact source
  revision and a signed, independently verified executable/build receipt. Verify
  executable bytes, signature, kernel PID/start time and isolated profile path;
  the current coordinator's process-field checks alone do not establish kernel
  provenance. Never select a pre-existing personal app or unknown PIN/seed window.
* Launch only owned empty original and initially absent restore profiles. Save
  new disposable credentials in a fixture-scoped macOS Data Protection Keychain
  namespace; never return PINs, seeds or plaintext recovery material. Disable
  sensitive logging and secret screenshots. Public observation digests must come
  from actual fresh UI observations, not arbitrary supplied JSON.
* Perform every requested GUI action through CUA, returning the exact fresh
  nonce, scope, session and request digest. A pending action is durable before
  execution; interruption quarantines the run. No automatic replay or guessed
  reconciliation is supported yet. Preserve the full journal and profiles.
* Create a Cube/Vault with Master hot signer with Developer Mode ON. Record exact
  Cube UUID, root fingerprint, full public descriptor and chain privately.
  Verify OFF hides testnet creation and OFF/ON preserve derivation, restoring ON
  without bypassing product gates. This is a contract, not an existing GUI parity
  result.
* Export the genuine full encrypted product Recovery Kit, including encrypted
  seed and descriptor; phrase-only backups are insufficient. Exit the original
  process before restore. Both ciphertext fields are retained, bounded and
  hashed; successful real GUI restoration must establish their usability.

### Original secret-store exclusion

Production `services/unlock/device_secret/mod.rs` uses service
`io.coincube.tenshu.device-secret.v1` and **Cube UUID as the account**. The
profile only scopes a lock file. A fresh datadir/PID does not exclude the original
Keychain factor used by the recovery loader.

Before any funding, a future reviewed isolator must quarantine only the newly
created fixture's original record after verified full-kit export and process
exit. Require fixture ownership and exact service/account binding, independently
read back original-record absence and prove all unrelated records unchanged.
Retain the quarantine material securely for recovery. The private isolation proof
binds fresh scope/nonce, original process, full-kit hash and identity hash; the
coordinator rejects profile-only isolation, stale/cross-Cube proofs or funded
fixtures. **No record is deleted or quarantined by this draft.** If a safe,
fixture-scoped isolation capability cannot be established, remain BLOCKED.

Continue through real GUI restore with a distinct new PIN and exact identity,
normal restart/unlock, wrong-PIN refusal without fixture damage and correct-PIN
Cube/Vault opening. The original process/cache must remain excluded. Then obtain
funds only through an approved facility on the exact chain and observe the wallet
recognize them. Ordinary Bitcoin Testnet4 UTXOs are not Blake2b Testnet4 funds.

Use the recovered hot signer in the GUI to sign a self-transfer and export its
PSBT through the native save dialog, stopping before broadcast. Retain funded
roots, both profiles, credentials, full encrypted kits, journals and public
artifacts. There is no destructive cleanup or automatic uncertain-run resume.

## Independent verification and evidence limits

`gui_verify.py` checks canonical PSBT maps, exact funding parent/outpoint, full
public descriptor-derived input/change scripts, signer origin/path, bounded fee,
canonical low-S DER signatures and public-key ECDSA with OpenSSL. Bitcoin uses
BIP143 `0x01`; Blake2b uses the independent test-framework Unified `0x21` digest
and must fail ordinary BIP143 verification. This public-artifact check does not
establish GUI provenance, wallet recognition or absence of cached secrets.

Run offline checks with `tests/e2e/run --self-test`. Injected test workers require
an explicit internal offline mode and produce `offline_passed` outcomes and
`offline_selftest` acceptance. Synthetic fixtures are not funded wallets and
never satisfy a live CLI run. Prior #1174 receipts describe earlier evidence;
they are neither imported nor counted as a fresh two-chain GUI acceptance run.

### Current CUA capability boundary

The documented native API exposes `setValue(index, value: string)`,
`typeText(text: string)`, `paste(text: string)`, key presses and observations.
It exposes no opaque secret-reference input, Keychain lookup or protected-file
reader/transport. A chat can drive public GUI steps, but a protected random PIN
cannot currently be loaded into those string arguments through a documented
secret handle. No unsupported Codex IPC, internal token discovery or runtime
filesystem assumption is part of this design. The owner has explicitly authorized creation and storage of disposable QA PINs;
that scope does not require a new permission prompt. The missing supported opaque
secret-input and protected artifact transport capabilities remain concrete
blockers to unattended operation, not a completed adapter.

The verifier intentionally accepts only a one-input, one-output self-transfer
sweep: fund exactly one recognized fixture coin, choose the GUI self-transfer
mode and sweep the available value minus the bounded fee to one descriptor-derived
change output. This is not a general recipient-send or two-output verifier. Read-only parsing of the prior public exported PSBTs confirmed one input and one
output on each chain (Bitcoin output 999585 sats; Blake2b output 999738 sats).
This shape check is historical evidence only, not a new GUI pass or full verifier
compatibility claim. A future real worker must inspect the actual exported map
shape and stop if it differs; do not silently remove outputs or regenerate it.
