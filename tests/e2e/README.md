# Testnet4 acceptance, without an agent in the loop

```sh
tests/e2e/run --qa-fresh --qa-run-dir /private/qa/run-001
```

Fresh runs use strict delivery and Home-refresh acceptance by default. Before
the phone process is pinned, the launcher builds and installs a signed QA app
from the exact Keychain revision in the private workstation configuration.
Every full run then executes environment, account, saved credential/network
and signer-capacity gates before creating a Cube or requesting funding.

The fresh flow generates its Cube, recovery material, 2-of-3 vault, phone signer,
funding request and transaction. It does not take an existing vault, descriptor,
PSBT, address, PIN or recovery phrase as input. Repeat the **same command and
run directory** to resume; choose a new directory for a separate disposable
vault. Pytest sequences the work and produces JUnit results. Appium/XCUITest
controls Keychain over USB. No model, camera, iPhone Mirroring, screenshots or
browser automation is involved in reruns.

Desktop provisioning exercises Tenshu's production Home, installer, storage and
Connect code through a native test adapter. It does not drive desktop widgets or
assert the balance shown in Tenshu's window; those visual acceptance checks
remain separate. The Keychain steps operate the actual iPhone interface.

## Fresh flow

1. Verify both public explorers' Testnet4 genesis and production Connect
   endpoints. Verify the configured QA account against the normal desktop
   Keychain session, then register the run profile through the production
   device-registration client. Persist its transport key before registration.
   Existing private auth caches can also be copied without changing their source.
2. Attach to the wired phone and verify the account. For a fresh signed-in QA
   keychain, select **Use a PIN**, generate and save its PIN, capture all twelve
   indexed recovery words, complete the real recovery challenge, and select
   Testnet4. Later runs reuse this account/device-bound credential.
3. Create a persistent Cube through the production Home state machine, complete
   the actual backup challenge, reopen its encrypted seed and unlock it.
   Register its stable identity and encryption key with production Connect.
4. Create a run-specific signer through Keychain's ordinary Add signer key
   flow. Reconcile the exact new key through Connect, then use the phone's
   **Attach to Cube** flow to bind and encrypt its public key. Require the
   Cube-scoped envelope to decrypt to the original exported public key before
   creating a vault.
5. Generate and reconstruct a Border Wallet grid phrase, pattern and checksum.
   Install a primary **2-of-3 Master / Keychain / Border Wallet** vault with a
   delayed Border Wallet recovery path through the production installer. Verify
   its Cube appears on the phone through Connect.
6. Request Testnet4 faucet funding once. If it is unavailable or delayed, use
   the configured disposable QA funding pool automatically. Require two independent explorers
   to agree on the funded outpoint and raw previous transaction. No prepared
   funding transaction or local Bitcoin Core synchronization is required.
7. Generate a recoverable test recipient and change address, construct a full
   PSBT, and add the Master signature. Pin its exact inputs, outputs and fee.
8. Create/recover one encrypted Connect request with a stable request ID. Verify
   the network, destinations, amounts and fee shown on the phone before using
   the generated PIN to approve it. Before entering the PIN, background/resume
   the app and record three seconds of challenge survival. This is lifecycle
   evidence; it does not claim to prove a Home-refresh event occurred.
9. Collect and verify the phone signature using production native crypto.
   Finalize against the saved spend intent, persist the signed bytes, and send
   exactly that transaction to the Testnet4 Esplora broadcast endpoint.
10. Require independent Mempool and Blockstream transaction acceptance. Add
    `--qa-confirm` to require a mined confirmation.

The fixed default spend is 10,000 sats with a 1,000-sat fee. Configured spend and
fee are bounded at 100,000 and 10,000 sats respectively. The native helper
verifies the funded script, previous transaction, primary quorum and actual
signatures. Unlike the legacy Core-backed mode below, fresh mode does not run a
separate local `testmempoolaccept`: the receiving Testnet4 node validates the
broadcast, and two independent explorers verify its acceptance.

## Workstation authentication and USB prerequisites

The fresh flow needs a signed-in disposable QA account shared by Tenshu and
Keychain. Account signup/email OTP, Apple trust/unlock/Developer Mode and
access to the team's Apple signing credentials remain workstation prerequisites. They are not
reported as successful test stages. The runner never logs out or replaces an
existing master. An existing passkey master cannot become an unattended PIN
signer merely by creating another key: it reports `PHONE_MASTER_NOT_OWNED`.
A fresh QA account at **Use a PIN** is supported; once the suite has provisioned
it, subsequent fresh-vault runs reuse its encrypted credentials automatically.

Store nonsecret workstation settings in private, gitignored
`tests/e2e/.local/workstation.json`, using `workstation.example.json` as its
schema. `account_email` is the exact disposable QA account; native bootstrap
refuses any different authenticated user before registering a device.
`profile_dir` names the normal signed-in desktop profile, not a prepared Cube
or vault. A legacy cache is optional: when absent, the suite reads the normal
desktop session from macOS Keychain and performs supported device registration.
`desktop_session_namespace` is `development` for a debug Tenshu build and
`production` for the release app; this selects only its existing Keychain
namespace. Both API endpoint values always remain pinned to production. `ios.minimum_build` must identify the minimum
reviewed Keychain build. Fresh onboarding also requires accessibility forwarding
through Keychain's screenshot-protection wrapper: build 36 hides the recovery
controls from XCTest, and cannot complete this flow. The example selects build
41. Build 38 introduced real PIN verification for PIN-backed signer entropy and
non-spoken signing-session/empty-entry accessibility identifiers. A build
number alone is not source or acceptance evidence.
The QA account must have capacity for a new signer key. The exact **Key limit
reached** screen produces `SIGNER_LIMIT` immediately; the runner never changes a
plan, deletes existing keys, or reuses an existing signer as a fresh fixture.
`ios.xctestrun` can identify an already-signed WebDriverAgent
runner so the suite starts it automatically when needed.

Set `ios.qa_build.checkout` to an absolute clone of
`coincubetech/coincube-mobile` (formerly `keychain-app`; either origin URL is accepted), `revision` to the full reviewed commit SHA, and
`flutter` to the absolute Flutter executable matching that source's `.fvmrc`.
The example's all-zero revision is a placeholder and cannot build. The source
is exported from Git into the private run directory, so dirty caller files and
its `.env` are never included. The build generates production endpoint settings,
enables `COINCUBE_QA_HOME_REFRESH=true`, archives the app, and exports a signed
release-testing IPA using the existing COINCUBE Apple account. No release tag,
TestFlight upload or distribution is performed by this command.

The app's signature, bundle/team, minimum version, production push/passkey
entitlements, production REST/gRPC settings, profile expiration and target
device coverage must pass before USB installation. The receipt records the
source revision, QA define and signed binary/IPA hashes. Compilation output
stays in private `qa-build/build.log`. The ordinary USB lease covers the build,
installation and entire test run, preventing another run from replacing its app.
On resume, the receipt and installed version are checked again, and the pinned
process must remain unchanged. Resume never rebuilds, reinstalls or relaunches
the app after its strict process identity has been recorded. A changed source,
device or installed build requires a new run directory.

Install Appium 3.9.0 and XCUITest 12.22.8, plus Python 3.11.4+, CocoaPods, Xcode and the pinned
Rust toolchain. The launcher installs pinned Python dependencies once. Fresh
mode builds both Rust helpers and signs the provisioning helper with the normal
COINCUBE Developer ID/profile automatically. No TestFlight upload is required
for a correctly signed build already installed over USB.

```sh
# Synthetic tests only: no phone, network, credentials or funds.
tests/e2e/run --self-test

# Build/install the QA app and check account, PIN setup, network and capacity.
# A fresh QA account may be onboarded; no Cube, signer key or funds are created.
tests/e2e/run --qa-fresh --qa-run-dir /private/qa/run-001 --qa-preflight

# Full live flow. Repeat this exact command after an interruption.
tests/e2e/run --qa-fresh --qa-run-dir /private/qa/run-001 --qa-deadline 180
```

Use `--qa-config /absolute/private/workstation.json` to override the default
workstation configuration. Only one physical acceptance process may own the
phone at a time; the suite holds a USB lease and exclusive run/account locks.

## Strict delivery and Home-refresh acceptance (version 1)

For a **new** fresh run, the launcher supplies `"strict_acceptance": {"version": 1}`
when omitted. The example also records it explicitly. Set `ios.minimum_build`
to the reviewed QA build, at least 39. The automated build enables the
`COINCUBE_QA_HOME_REFRESH=true` compile-time hook. Build number alone cannot
prove the hook exists: missing or ambiguous markers block the test. Null,
false and unsupported versions are rejected. The configuration and mode
are journal-bound, so an old or completed run cannot be relabeled strict.
For an intentional historical non-strict replay, `--qa-compatibility` must be
explicit and its configuration must omit both `strict_acceptance` and
`ios.qa_build`. It produces ordinary-mode evidence and does not build/install a
QA app. Existing old checkpoints cannot be silently upgraded to strict mode.

Strict mode attaches to the already foreground app without activating it. It
sets Appium's `defaultActiveApplication` to Keychain before binding the process,
so WDA's automatic SpringBoard selection cannot masquerade as a PID change.
Independent app-state checks still require foreground state around each identity
read. The read-only system-alert endpoint separately rejects detected system
Alert/Sheet surfaces and any uncertain alert-query result; alert text is discarded,
and no alert is accepted or dismissed. This guard does not guarantee detection
of transient system overlays that are neither Alert nor Sheet surfaces. Strict mode
pins the device, build and process, rejects every restart/upgrade/key-recovery
allowance, and rechecks the process on resume, during delivery, before PIN input
and during signature collection. After setup and before request creation, it
observes public Home for at least 90 seconds, recording sample count and maximum
observation gap (at most five seconds). An interrupted pre-request observation
starts a new full interval. This proves sampled foreground observations and no
runner-initiated restart; it cannot monitor the app while the runner is stopped
or exclude an unobserved brief lifecycle transition between samples.

Setup refreshes are recorded before request creation. After that boundary, the
runner neither refreshes Home nor restores setup navigation. It passively polls
native-validated status for the exact request/target/transaction until
**DELIVERED (2), zero submitted signatures**. The target-device acknowledgement
receipt, immutable approval binding and pinned process must be saved before
any pending-banner or queue navigation. A generic notification is not delivery
proof. After that receipt, normal queue navigation and exact-note selection
perform the full transaction review. Later VIEWED status can resume only with
that receipt; advanced status without it cannot retroactively prove arrival.
Delivery timeout never consumes a restart allowance in strict mode.

At the exact empty signing PIN challenge, the runner discovers separate,
unique accessibility elements for the QA scope, status, counters and trigger.
It saves intent before one trigger click. That trigger invokes the app's actual
Home refresh callback; success requires the same public session and positive
mount serial with `requested=1:started=1:completed=1:failed=0` and status
`completed`. Completion means the correlated pending-session Bloc outcome,
not merely an RPC returning. Missing/duplicate markers, remounts, regressing
counters, failure, timeout or an uncertain click block PIN input; the trigger
is never clicked again after an uncertain attempt.

Strict mode substitutes this actual Home-refresh probe for normal mode's
background/resume probe. It retains the full transaction review, VIEWED/zero
remote guards and protected credential CAS. It checks the completed mount after
the final remote guard, after credential retrieval and before every digit,
requiring an empty challenge before the first digit. The dispatch and successful
six-click receipts bind the exact scope and approval context. A completed probe
may resume only in the same process and live mount before any input attempt;
an uncertain input is never retried. Signature collection and broadcast require
all saved strict proofs, so an externally arriving valid signature alone cannot
make this UI acceptance pass. No raw accessibility tree or credential is saved.

These are additional acceptance conditions, not evidence that a physical
strict run has passed. A new disposable run is required to collect that evidence.

## Credentials and recovery after interruption

Generated Cube PIN, Master and recipient recovery phrases, Border Wallet phrase,
pattern/checksum, and phone PIN/recovery words are encrypted in macOS Data
Protection Keychain. The signed native helper verifies readback before use.
Phone updates use compare-and-swap revisions; a stale process cannot overwrite
newer recovery state. Nonsecret locator files and private checkpoints stay in
the run directory. Keep both the Mac's Keychain and the run directory for
recovery; copying the run directory alone does not copy its credentials.

No secrets are placed in argv, Git, JUnit, screenshots or raw Appium logs.
Authentication caches, descriptors, PSBTs and signed transactions remain private
files under the run directory. The generated Cube uses production encrypted
seed storage. Secret-get responses travel only over a captured child-process
pipe; do not invoke them with stdout forwarded to a terminal or log.

A faucet timeout leaves a durable claim intent. A rerun checks that address
instead of requesting another payment. Explicit empty/rate-limit responses
retain the service's retry window. The public faucet can be unavailable; that
uses the automatic disposable QA fallback when configured; an exhausted pool
remains `BLOCKED`. Its developer API uses `btc-testnet` for
Testnet4 ([official API](https://www.cypherfaucet.com/api),
[service source](https://github.com/Tech1k/cypherfaucet.com)).

The default workstation example enables `qa_funding`. Configure its `sources`
once with completed disposable QA runs belonging to the same account, optionally
including `source_sponsor_dir` for one original version-1 sponsor receipt (see
[SPONSOR.md](SPONSOR.md)). Normal `--qa-fresh` runs need no funding approval or
additional command. Historical configurations without this field keep their
faucet-only behavior. Automatic funding requires strict acceptance and the signed
QA build preflight; screenshot protection remains enabled.

After a bounded faucet grace period, the runner freezes the actual target's
public provisioning evidence and selects one unspent, explicitly authorized QA
change output. The native sponsor reconstructs its keys locally and signs only
the frozen Testnet4 plan: 100,000–500,000 sats (enough for this run's spend,
fee and change), a 100–1,000-sat fee and at least
1,000 sats returned to the source. A host-wide private registry locks and reserves
the source outpoint before signing, saves signed bytes before broadcasting, and
reconciles those exact bytes through both explorers after any uncertain response.
Retries retain the same source and transaction. Late faucet deposits are checked
before reservation and before the first sponsor broadcast. Conflicting, spent or
ambiguous outputs block rather than create another payment.

Only fully completed strict runs with two signatures, actual PIN/Home-refresh
proof and dual-explorer broadcast verification enter the reusable pool. Their
public source snapshots retain the original run/profile identity and are scoped
to the configured authorization roots; unrelated runs are never scanned or
imported merely because they share an account. One original sponsor-change hop
is supported; nested sponsor hops remain rejected. Keep the run profiles, native
credential storage and the registry at
`~/Library/Application Support/COINCUBE QA/funding-v1` for recovery. Reservations
never expire automatically. The pool is finite: depletion, revoked sources,
missing credentials, unavailable USB hardware or failed strict acceptance stop
the flow with a specific blocker. Signing, spend broadcast and final txid
verification continue through the normal strict stages after funding.

A late faucet releases a donor only when this invocation created its reservation
and has not invoked the helper or created helper output. The external funding
outcome and both reservation indices are saved atomically, so a crash cannot
reacquire that donor for the same target. Signed and resumed reservations remain
quarantined: an unspent outpoint does not prove that valid signed bytes cannot
still be broadcast. Explorer propagation disagreement is retried within the run
deadline, never treated as an empty address. Source account, descriptor and
binding failures deliberately stop the pool rather than silently switch donors.
Keep explicitly configured historical seed runs unchanged; their native parent
receipts bind exact public evidence. New completed-source snapshots isolate
ordinary verification updates without weakening those bindings.

A PIN attempt is recorded before entry and blocks further attempts across
process restarts and different run directories until the exact key/signature
is verified through Connect. A delayed signature is polled without reopening
approval. Before any first signing PIN, the runner checks the exact session's
empty-entry accessibility identifiers, the foreground process, and production
Connect's validated VIEWED status with zero submitted signatures. It records a
background/resume probe and durable dispatch intent before requesting the
protected credential. An interrupted probe or input is never retried.

In normal mode, a `REQUEST_DELIVERY` timeout before any approval or PIN activity permits one
journaled app restart solely to rediscover the same request. The runner preserves
the original timeout and exact run/account/device/build/transaction bindings,
requires public Home and a verified idle credential (including no unfinished key
reservation), and rechecks native-validated PENDING status with zero submitted
signatures immediately before termination. It records the attempt before the
restart and requires `NOT_RUNNING` followed by a different foreground process.
An uncertain response blocks all further navigation on resume; it never repeats
the restart. A completed receipt permits ordinary account/network checks and full
transaction review. A second delivery timeout stops without another restart.
If the timeout leaves the app on **Pending Requests**, recovery supports only
that exact route with one visible, enabled **Back** button (exposed by QA build
39; build 38 lacks it). It rejects authentication/review/conflicting surfaces
and native modals, rechecks the idle credential and exact session before one
journaled Back, and requires genuine Home in the same process before completing
that navigation. A missing/ambiguous Back is explicitly unsupported; generic
navigation recovery is never used by this recovery helper. The obsolete pending
queue restart fallback is also removed from ordinary approval and setup/resume
navigation: those paths require the accessible Back control too. An uncertain
Back blocks resume before setup navigation, even
across a build change. A completed Back still requires the existing final Home,
process and native eligibility checks before the separate journaled restart.
If the exact session advances to DELIVERED or VIEWED with zero submissions,
the runner proceeds to ordinary exact-request review without restarting or
consuming the restart allowance. Approved, terminal, unknown, or nonzero-submission
states cannot use this path; a verified signature proceeds to collection.
If the app build changes after an untouched delivery timeout and no restart
receipt exists, authentication preserves the original timeout and permanently
records that restart is disabled for this request before updating phone build
observations. This requires the same account/device/request and an idle verified
credential with no approval or PIN markers. Normal account/network/Cube checks
still run; then a fresh native PENDING, DELIVERED or VIEWED status with zero
submissions permits ordinary full review. A verified signature can instead be
reconciled without review. The disable decision survives another timeout,
rollback or later upgrade; it cannot coexist with an attempted, completed or
malformed restart receipt and never re-enables a restart allowance.
The exact known incoming-transaction notification also routes to ordinary queue
review after fresh idle-credential and session checks, without consuming a
restart. Partial, ambiguous, authentication, and unknown modal screens remain
blocked; the notification itself is never treated as the selected request.
No request, approval, or PIN marker is reset or replaced. This is a bounded
discovery repair, not evidence that the underlying delivery cause is resolved.

An approval interrupted before any PIN/probe may resume in the same process only
with the saved build, device, session and immutable transaction context. One
reviewed app upgrade may instead create a fresh UI review of that same request:
the runner archives the original attempt, requires a strictly newer build,
verifies termination (`NOT_RUNNING`), rechecks Connect, and repeats all account,
network, Cube and transaction checks. The original approval marker is never
cleared; a separate durable recovery receipt permits at most one new Approve
dispatch. Legacy build-37 evidence is eligible only when its saved phone check
precedes a recorded `PHONE_AUTH` block and no PIN attempt exists. Existing
signatures always switch to collection. Neither path creates another spend or
changes the saved transaction. Interrupted initial PIN entry fails closed if
partial input cannot be determined. One completed PIN setup whose recovery words were never captured
and whose challenge was never submitted may restart at the full fresh-setup
chooser after an app restart. The runner archives the previous entry markers
before acting and retains the same encrypted PIN. It never repeats an unlock,
resets an existing master, or discards a saved phrase/submission. A second such
restart is blocked. Keychain's normal local-blob and server-key guards must pass
again before a new recovery phrase appears, and Settings identity is verified
before Cube creation or funding. Recovery confirmation resolves the exact visible
button before saving dispatch intent; an uncertain submission never re-enters
answers or taps again. A completed Home/setup-success screen can reconcile it
only with the already saved recovery proof. Lost broadcast responses reconcile the same transaction ID;
retries can send only the previously pinned signed bytes.

A separately authorized replacement generation is available for the narrowly
reviewed build-38 capacity-blocked case. It requires an explicit authorization
reference and the private checkpoint of an earlier same-account/device run that
proved saved-PIN signing and broadcast:

```sh
tests/e2e/run --qa-fresh --qa-run-dir /private/qa/interrupted-run \
  --qa-authorize-key-generation EXPLICIT-AUTHORIZATION-REFERENCE \
  --qa-key-recovery-prior-run /private/qa/completed-run
```

This is not a retry of the old input. The operator must authorize one new attempt
and establish that the same controlled device/account/master had no intervening
PIN or master-change operations. The build-38 source resets the failure counter
before reaching the observed capacity screen; the runner records this and the
prior successful PIN signing as operational evidence, not a measured counter or
a definitive outcome for the old key. Production capacity must actually be
available. The prior checkpoint supplies recovery evidence only; it supplies no
Cube, vault, signer, PSBT or funding fixture for the new generation.

The runner archives the old pending purpose as **unresolved**, verifies a stopped
app process and live account/network/Keychain identity, then uses one encrypted
compare-and-swap to reserve a distinct key name for this run. The reservation
blocks every unrelated credential use until it is consumed by the newly reached,
empty PIN prompt. A second CAS records that new pending purpose before input.
Before any PIN input, the same build-38 foreground process can continue its
original newly created challenge with the exact unused encrypted reservation.
This continuation never navigates, recreates the key, restarts Keychain, or
clears input; an omitted WDA value is accepted only for that untouched prompt.
Once the durable input marker is set, only remote reconciliation is allowed.
Crashes reconcile the same reservation; they cannot clear a newer attempt or
create another allowance. An old key appearing, changed identity, nonempty field,
lockout/error, or uncertain input stops the run. Successful new registration
retains the old unresolved history. Repeat the ordinary command without the two
recovery options after the authorization has been checkpointed.

Exit **0** means selected stages passed, **1** an assertion failed, **2** a
selected live run was blocked. Preflight success is only preflight success.
`checkpoint.json` records stage outcomes, and `results.xml` is sanitized JUnit.
Ordinary repository collection skips live tests; these skips are not acceptance
evidence. This flow is opt-in and is not added to GitHub CI.

## Legacy prepared-vault replay

Schema version 1 remains available for the original saved draft. It uses a
prepared disposable vault, full locally signed PSBT, exact spend intent and a
synchronized local Testnet4 Core. It requires its own explicit configuration:

```sh
tests/e2e/run --qa-config /private/qa/config.json --qa-run-dir /private/qa/legacy-run
```

The following setup details apply only to that legacy mode.

## One-time workstation and fixture setup

Use a dedicated QA account and disposable Testnet4 signer/profile. The suite
never resets Keychain or signs the user out. Keep the phone signed in, trusted,
unlocked, in Developer Mode, with XCTest UI automation authorized. Install a
Keychain build containing #194 and set its real build number as the minimum
(the default minimum is 35; a build number alone is not source provenance).

Install the pinned Appium tools in the test user's normal environment:

```sh
npm install -g appium@3.9.0
appium driver install xcuitest@12.22.8
```

Python 3.11.4+ is required. `run` creates `tests/e2e/.venv` and installs the pinned
Python dependencies once. It also finds a versioned Homebrew Python when macOS
`python3` is older. Later runs reuse that environment.

Start an already-signed WebDriverAgent on the phone, or set `ios.xctestrun` to
its existing signed `.xctestrun` file; the suite can launch that runner with
`xcodebuild test-without-building`. This does not provision certificates or
change trust settings. The WDA tunnel is created and removed automatically;
no `iproxy`, Wi-Fi device address or iPhone Mirroring is needed. Only runner
processes started by this invocation are stopped.

Build the native helper once from the same checkout using the pinned Rust:

```sh
PATH=/opt/homebrew/opt/rustup/bin:$PATH \
  BREEZ_API_KEY=testnet4-fixture-placeholder \
  cargo build --locked -p coincube-gui --features integration-tests \
  --example testnet4_connect
```

The signed Cube test also requires the COINCUBE Developer ID and the repository's
normal macOS signing profile. Its runner uses the current checkout's own Cargo
target directory. It generates its own PIN/recovery material, verifies it and
removes the disposable Cube and device secret. It does not supply the persistent
live-vault fixture.

Copy `config.example.json` outside Git and fill in the **existing QA** fixture:

- `profile_dir`: isolated Tenshu root; `testnet4/connect.json` must hold the
  existing active account/device, and `testnet4/connect_transport_key` its
  existing transport key. The helper never creates/replaces one. An expired
  cached account session reports `connect_auth_expired`; refresh through the
  ordinary application before resuming.
- `psbt_file`: base64 **full** saved draft, with the valid local Master Signer
  signature, authenticated previous transactions and complete key origins.
  Keep the original immutable. Do not provide the phone-pruned wire payload.
- `descriptor_file`: the fixture vault's private descriptor text.
- `cube_id`, `vault_id`, `target_key_id`, `target_fingerprint`: the existing
  production Connect records for that disposable Testnet4 vault/signer.
- `intent`: exact input outpoints and two output addresses/amounts, expected
  unsigned transaction ID, and fee cap. This suite supports native P2WSH primary
  spends with one recipient and one change output. It refuses other policies.
- `core`: synchronized Testnet4 Core on a literal loopback address with its
  cookie file. No credential-bearing remote RPC URL is accepted.
- `ios.cube_label`: exact visible Cube label. `ios.pin_file` is optional for
  signers using the ordinary app PIN flow; never put a real credential in JSON.

Set private fixture files, including the cached auth/transport files, to mode
600 and their containing fixture/run directories to 700. No secrets, raw PSBTs,
descriptors, private identifiers, PINs, or real run reports belong in Git.
`config.example.json` contains placeholders and cannot accidentally run live.
The `run_id` is stable for one immutable spend; a different funded transaction
requires a new run ID and directory.

The historical GUI request had no QA note and cannot be adopted blindly.
This runner reuses its prepared draft but creates its own deterministic request
on a live invocation. It does not cancel any historical session. Do not run the
GUI's signing flow concurrently for the same draft.

## Commands and results

```sh
# Local, synthetic regression tests: no network, phone, credentials or funds.
tests/e2e/run --self-test

# Existing fixture/device checks without creating a session or broadcasting.
tests/e2e/run --qa-config /private/qa/config.json \
  --qa-run-dir /private/qa/run-001 --qa-preflight

# Live acceptance, automatically resuming the exact request/transaction.
tests/e2e/run --qa-config /private/qa/config.json \
  --qa-run-dir /private/qa/run-001 --qa-deadline 120
```

Exit **0** means all selected stages passed, **1** means an assertion failed,
and **2** means a selected live run was blocked. A preflight exit 0 is only
preflight success. Ordinary repository pytest collection skips live tests
unless configured; those skips are not acceptance evidence. No live tests are
added to GitHub CI, and xdist is refused for the physical-device scenario.

`checkpoint.json` records stage outcomes and the stable session/transaction
identity; `results.xml` is a sanitized JUnit report. `signed.psbt` and
`signed.hex`, if produced, remain private in the run directory. The process
holds an exclusive run lock and a shared USB automation lease. File writes use
atomic rename and fsync. Changed configuration or draft bytes cannot reuse a
checkpoint. In this legacy flow, a crash or timeout after the Approve tap never causes an automatic
second approval or PIN attempt; rerunning skips the phone and keeps polling
Connect for the signature, reporting `SIGNATURE_DELIVERY` if it still has not
arrived.

No raw Appium server logs, page source, screenshots, RPC payloads or native
errors are saved. Failures use stable codes such as `REQUEST_DELIVERY`,
`NATIVE_CONNECT_AUTH_EXPIRED`, `KEYCHAIN_BUILD`, `MEMPOOL_REJECTED` and
`PUBLIC_PROPAGATION`. These are what routine automation should report; an agent
is only needed to investigate an unfamiliar failure or update changed selectors.

The read-only standalone diagnostics remain available:
`diagnostics/ios_usb_wda.py` and `diagnostics/testnet4.py [--txid TXID]`.

References: [Appium attach to running WDA](https://appium.github.io/appium-xcuitest-driver/latest/guides/attach-to-running-wda/),
[real-device prerequisites](https://appium.github.io/appium-xcuitest-driver/latest/preparation/real-device-config/).

### Interrupted signing abort (separate evidence, no retry)

`./sign-abort --source-run /absolute/original-run --config /absolute/original-config.json
--output-dir /absolute/separate-abort-directory --native-binary /absolute/testnet4_provision
--signing-binary /absolute/testnet4_connect --authorize-abort 'owner instruction reference'`

This narrowly reconciles a saved, interrupted signing PIN dispatch. Stop the
original runner first. The helper locks but never rewrites its run directory,
validates its account, transaction, approval and credential bindings, then uses
production Connect's supported Cancel RPC only for the exact unsigned active
request (states 1–4), or verifies that same request was already cancelled (8).
It refetches and requires cancelled status with zero submissions. Other terminal
states, changed bindings and signature races block credential reconciliation.

A durable `abort-intent.json` pins the original encrypted credential revision.
`abort-terminal.json` is written and read back before any encrypted credential
change. The native secret store compare-and-swap removes only that original
pending signing purpose and appends `cancelled_signing_attempts`; the local PIN
outcome stays **unresolved** in this retained audit. Existing key-generation
audit and all credentials remain intact. Exact replay does not clear a newer
pending attempt. The helper cannot sign or broadcast and does not make the old
strict run pass. Use a new run for new acceptance evidence. After an uncertain
RPC/CAS response, rerun with the identical inputs and output directory.

### Repeatable disposable fixture retirement

The workstation template enables `qa_cleanup: {"version": 1, "preserve_runs": []}`.
This is separate authorization from `qa_funding`; configurations that omit it
remain read-only at the signer limit. Add **every historical QA funding source**
not already listed in `qa_funding.sources` to `preserve_runs` by absolute run
path. These protected paths are resolved to exact Cube/key provenance, including
recovered signers whose labels differ from their run UUID. Personal or shared
objects are never discovered or deleted by a `QA` name prefix.

A fully passed strict run enrolls its exact owner, Cube UUID/ID, vault descriptor
fingerprint, signer ID/fingerprint, immutable public snapshot and retained local
recovery files in the same private host registry used for funding. Older runs
are not auto-discovered: their one-time retirement requires a separate explicit
audit. At authenticated Testnet4 signer capacity, the next strict run can retire
at most one enrolled fixture. It first proves the native Master/Border backup
is readable, checks pending signing **and recovery-decrypt** requests over gRPC,
rechecks both explorers, and excludes explicit funding sources, usable unspent
managed donors and uncertain funding-claim dependencies. Available-capacity runs
and already-created signer resumes start no new retirement; an already journaled
cleanup finishes first.

Retirement deletes cloud metadata in order **Vault → Signer → Cube**, with a
private durable intent before every DELETE and exact-ID readback afterward.
Interrupted cleanup reconciles that same fixture and cannot select a second one
for the same consumer run. A failure deleting the signer prevents Cube deletion.
New shared members, key references, pending requests, unexpected funds, changed
balance or missing recovery material stop cleanup. Screenshot protection and
strict phone-process acceptance remain enforced.

No coins are swept and no local profile, checkpoint, descriptor, encrypted native
backup, signed transaction or sponsorship evidence is deleted. A small residual
(for example 89,000 Testnet4 sats) is explicitly recorded as still recoverable
with the retained Master and Border keys. A cloud tombstone excludes the retired
managed donor from future funding selection without invalidating its local
transaction/recovery history. Keep those local files and macOS Keychain items.

Enrollment has two durable phases: the live `verify` stage records the exact
strict proof and recovery tree, then the final pytest session callback seals it
only after the real JUnit file contains all 15 passed stages. A lost callback
can seal that same enrolled proof later; it never invents a new phone/process
proof. Unfinished or failed JUnit evidence cannot authorize retirement. Cleanup
holds the candidate's original `run.lock` through all guards, DELETEs, readbacks
and its final tombstone, and resumes a pending retirement before considering a
newly freed slot or an already-created current signer.

The retained manifest includes the entire owned profile and run history,
including encrypted mnemonic backups, wallet database/configuration, transport
key/cache, public funding/signing files, phone key, final JUnit and build
receipt/log/ExportOptions. Only known synchronization locks and the live
checkpoint are excluded; the completed checkpoint is pinned in the immutable
source snapshot. Reproducible `qa-build/source`, `derived`, `Runner.xcarchive`,
`export` and `validated` trees are excluded from the recovery/history manifest.
Nothing in those directories is deleted by cleanup. Any missing or changed
retained file, malformed tombstone or inconsistent explorer outspend/UTXO view
fails closed before further destructive work.

A spent change output is never treated as a zero balance. Both explorers must
agree on the exact spender transaction/input. Cleanup recognizes only already
accepted automatic QA sponsorships with matching private intent/receipt hashes,
source and target snapshots, native receipt, authorization roots and exact raw
transaction bytes. It follows the existing v1 sponsorship and its one supported
v2 hop, then checks the known remaining source change (for example 1,000 sats)
on both explorers. Usable unspent change remains protected as a funding source.
An unknown, conflicting, missing, deeper or unindexed spend blocks retirement;
cleanup neither constructs a transaction nor broadens the sponsorship protocol.

Shared-key protection discovers both owned Cubes and `shared-with-me` Cubes across
all networks. Every other discovered Cube requires a fresh, typed `/keys`
inventory and complete Cube detail before any DELETE; nullable `hasVault` and
omitted list projections never prove that no references exist. Owned Cube vaults
are also read independently. Shared vault membership comes from the member-visible
Cube detail and key inventory because the separate `/vault` read is owner-only.
Missing, failed, malformed or contradictory inventories block cleanup before it
deletes the disposable fixture's own Vault, even if the API would later refuse
signer deletion through its additional server-side reference guard.


### Ordinary native QA on an owned loopback backend

The integration-only `native_recovery` object optionally accepts
`"ordinary_backend": "qa_loopback_31004"`. This explicitly binds ordinary
Testnet4 native Recovery Kit admission, creation, restore and oracle operations
to `http://127.0.0.1:31004`; it permits no caller URL, bearer or fallback. An
independently admitted owned forward must already connect that port to the
reviewed Testnet4 backend. Its owner/source/deadline guards and the original
genesis/fullscan/current native oracles remain required. The option grants no
runtime authority and does not start a tunnel. A new signed source/config/run
binding is required; do not modify an existing fixture to switch its backend.

Omitting the option keeps existing public-provider behavior. B2 signed case
configs omit it and retain the authenticated Connect backend. Mainnet and B2
reject the loopback selection. No ordinary bearer is serialized in daemon.toml.
