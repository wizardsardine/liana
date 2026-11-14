# Native full Recovery Kit QA

This suite is native product integration, **not GUI acceptance**. It does not
control the desktop/iPhone, disable screenshot protection, fund a wallet or
broadcast. The final PSBT is explicitly unfunded key-control evidence. The
separate GUI suite remains incomplete; native results cannot satisfy its stages.

## Automated invocation

```sh
tests/e2e/run-native-recovery --config /private/native-recovery.json --run-dir /private/new-native-run
```

The launcher selects supported Python, provisions its private pinned virtual
environment, verifies source ancestry/cleanliness, builds and signs the exact
native helper, then runs both Testnet4 chains without hand-entered receipts.
A private configuration has this shape (no credentials in JSON):

```json
{
  "version": 1,
  "native_recovery": {"version": 1, "checkout": "/absolute/coincube", "revision": "current"},
  "account_email": "disposable-qa@example.invalid",
  "connect_cache": "/private/qa-profile/testnet4/connect.json"
}
```

`revision` may instead pin a full SHA. Source must include reviewed PR #1174's
cold loader (`9e9a1f25995b4be9a6ab188240a2bf92d5f3209b` ancestor); otherwise
preflight is BLOCKED before credentials or fixtures. Do not cherry-pick or
silently bypass this dependency. An explicitly reviewed unpublished integration
candidate must be identified as such; it cannot establish primary-branch live
acceptance. A source-bound offline test or native helper build alone is never live full-kit acceptance.

New `workstation.example.json` enables `native_recovery`. The fresh mobile
pipeline performs read-only native source/dependency preflight in environment,
before mobile creation. After real phone setup, it completes the native suite
before creating the mobile Cube, so the temporary native fixtures do not require
another spare slot after mobile creation. Immediately before every funding/resume
branch it revalidates the same completed run-bound native proof, blocking faucet/sponsor activity on
missing prerequisites, malformed evidence or native failure. Historical configs
without this field preserve their original fifteen stages and record
`legacy_mobile_only`; they do not gain full Recovery Kit coverage retroactively.
The new native lifecycle proof is version1 and bound to the exact mobile consumer, source,
helper hash and both chains. No existing strict stage or cleanup proof is weakened.

## Product lifecycle and isolation

Each chain gets a distinct initially absent profile/UUID. The feature-gated
adapter invokes Home's real PIN creation and backup ceremony, normal isolated
Developer Mode/beta preference setters and actual chain capability checks. It
creates the Vault inside that Cube through `install_local_wallet`; it does not
use the obsolete no-Cube launcher path. The shared Settings/creation
`encrypt_and_upload` produces the full seed+descriptor kit. Server readback,
account, Cube UUID/chain, full persisted descriptor and complete root xpub are
checked before retaining the ciphertext. No phrase-only backup counts.

The signed helper stores original/new four-digit desktop PINs and a random
recovery password only in device-only Data Protection Keychain records. Tokens
enter a separate auth-binding process through stdin and never argv/env/logs or
new plaintext files. The restore process receives ciphertext and scoped handles;
it never loads the original PIN, old seed files or quarantined original factor.
The signing process loads only the new PIN, never the recovery password/old
factor. These are enforced signed-program role boundaries on one host, **not a
claim of a separate OS user, sandbox or fresh physical device**.

The oracle requires a real committed full-scan generation and a fresh poll.
Running address counters return to zero after completion and do not prove
coverage. A fresh startup full scan may satisfy the baseline; a synced database
requests a new full scan from the actual admitted chain's genesis time through
the normal rescan guard. No projected Bitcoin genesis or epoch-zero date is used.
The scan keeps the product gap of 200, with an in-memory one-second integration
poll interval and a bounded 1200-second wait (1380-second oracle child timeout). Sanitized
private progress records show chain/sync/scan generations/failure presence, not
keys or backend error bodies. Ordinary Testnet4 additionally paces SPK starts at
1200ms in memory, with one shared rate gate across branches and retries and BDK
parallelism one, enabled only by a private runtime option in the dedicated
`native-recovery` feature. No serialized or environment pacing knob exists. The
gap remains 200; pagination and metadata are not claimed to
have a universal provider quota guarantee. B2 stays unpaced and Connect-only.
Unknown/funded state remains BLOCKED. The standalone Darwin launcher owns one
parent-bound awake assertion capped at two hours and releases it on exit; it
never unlocks an intentionally locked host or changes lock settings. Initially
locked Data Protection access remains BLOCKED. Non-oracle actions retain the
660-second child timeout; cooldowns are never reset and partial scans never pass.

Before wiping, the original process must have exited (PID plus kernel start
identity), the sealed creation provenance/tree must still match, and the real
exact-chain backend must report synced, empty known coins. Unavailable, unknown
or nonzero state blocks. Only the new fixture's exact production-service
Cube-UUID factor is copied to a separate protected QA item and read back before
removal. The original profile is moved to retained quarantine, proving absence
at its old path while preserving recovery. A distinct newly created production
service canary verifies narrow mutation; no personal factor values are read or
enumerated. Canary coverage plus static singleton-query review is not a global
atomic proof about every Keychain record.

A new process stages both kit halves through the real installer restore service,
chooses its distinct new PIN and mints a cryptographically different factor.
Another process tests an actual wrong-PIN outcome before correct cold loading,
reads the saved Cube/wallet ID and actual persisted daemon descriptor/chain,
and opens Vault hot signers using the real chain-aware loader. The ordinary
Bitcoin Testnet4 cache-miss SeedSource regression is additionally mandatory;
SeedSource is a Spark/Liquid Network-only API, not a Blake2b chain loader.

Product signing retains both known descriptor signatures (primary `[0,0]` and
recovery `[2,0]` from the same disposable root). Independent Python reference
verification checks canonical PSBT maps, full descriptor-derived keys/scripts,
exact two origins and signatures, Bitcoin BIP143 `0x01` versus proprietary
Blake2b Unified `0x21`, and wrong-domain/altered-message refusal. This is a
controlled unfunded PSBT, never a recognized UTXO or transaction acceptance.

## Repeatability, retirement and interruption

Before any native PIN, password, profile or production canary is created, a
signed read-only process validates the real account, chain capability, all-network
owned/shared UUID absence, pending approvals/sessions/decrypt requests and exact
Cube quota. A full quota can retire at most one already enrolled disposable QA
fixture through the existing cleanup policy. This legacy cleanup admission is
ordinary Testnet4 only: full Blake2b Testnet4 capacity blocks without selecting
or deleting an ordinary Testnet4 fixture. Available Blake2b capacity proceeds
normally. It reuses its retained-file and
backup checks, candidate lease, pending/sharing graph, residual accounting and
funding-source protections. Native admission is a separate typed consumer; it
never fabricates a phone PID or mobile acceptance. Initial selection requires a
fresh FULL quota. An already selected retirement finishes even if its earlier
DELETE has made capacity available. No second fixture is selected to chase a
concurrent quota change. Complete native proofs perform no new admission or
cleanup when the mobile Cube later occupies the freed slot.

Standalone runs can add `"cleanup_workstation": "/private/workstation.json"` to
the private launcher config. This references the existing QA workstation's exact
account/cache, preservation roots and funding policy. It does not adopt its phone
identity. Backup and pending helpers are built from the same frozen source and
pinned by hash and signing profile before and after private recovery operations;
caller-selected binaries are not accepted. Without this explicit cleanup config,
full capacity remains BLOCKED before native fixture secrets.

One narrow source-upgrade handoff supports the reviewed historical capacity
failure from source `74da94d2848783951ff9b305724340afbb230ab8` and its pinned signed
helper. The launcher accepts `--abandon-capacity-from /private/old-native-run`
and `--legacy-capacity-helper /private/archive/RecoveryNative.app/Contents/MacOS/recovery_kit_native`
together, with a fresh `--run-dir`. It verifies the archived helper (never executes
it), exact old source/checkpoint/derived UUID/account, dead creator, NEW pending
create with no identity/kit/profiles/factor, production factor and all-network
remote UUID absence, and the fixed QA-service canary handle. It does not trust or
read the mutable production-canary UUID. All old files and Keychain items remain
unchanged. A new add-only protected metadata claim binds that one historical
failure to one fresh consumer; retrying that same consumer is idempotent. This
handoff is neither native acceptance nor a general interrupted-fixture importer.

After independent public verification, only the newly created native remote
Cube is eligible for retirement. This flow never registers a remote Vault/key.
Retirement requires the exact sealed native source, retained full ciphertext kit,
matching server ciphertexts/account, synced empty restored wallet, raw successful
Cube/key inventories with explicit sharing fields, no Vault, no keys, no
Lightning/Stable Balance/encryption pubkey, approvals empty and authoritative
owner gRPC pending sessions/decrypt requests empty. Fresh inventories and pending
checks immediately precede one DELETE, with a separate durable delete-started
intent and 404 readback. No label authorizes deletion; unknown fields or schemas
block. There is no global atomic graph snapshot; concurrent changes remain a
backend/TOCTOU limitation, so only this fresh private unfunded fixture is eligible.
The local full kit, profiles, credential handles, factors/quarantine and public
proofs remain intact. Personal keys, mobile keys and QA funding sources are untouched.

Future fixtures retain exact per-file inventory rows under the protected origin
binding. Historical source 215 stored only an aggregate digest; those missing
historical rows are never reconstructed or represented as verified.

`--retire-orphan-215-from /private/old-run` and
`--orphan-215-helper /private/archive/RecoveryNative.app/Contents/MacOS/recovery_kit_native`
provide one explicit remote-only route for the reviewed source 215/helper pair.
It accepts only that origin's exported, pending-prewipe fixture and takes both
old leases. It checks the protected origin binding, current owner, full kit
ciphertexts/root/descriptor/chain, retained factor and no restore/quarantine.
It snapshots the CURRENT whole old file tree and protected item audits, then
checks them unchanged before DELETE and afterward. PIN audit digests use HMAC
keyed by the retained high-entropy factor, never an unsalted four-digit PIN hash.
Old files, pending journal, factors and auth item remain unchanged. Only a new
auth scope is updated.

The orphan oracle creates an initially absent descriptor-only database from the
validated kit: it imports no old database, seed or factor. Actual configured and
global QA funding-source protections hold the funding registry lease through the
operation. The normal native sharing/key/Vault/pending guards are reused and
freshened before the single DELETE. Add-only consumer and DELETE claims bind the
exact old remote ID/UUID/source/kit; a first 404 never manufactures intent. The
result says only remote metadata retired, local recovery retained, old flow
BLOCKED, native acceptance false. A separate normal fresh suite must still prove
both full lifecycles. No orphan result can satisfy a signing/GUI/funding stage.

Each child PID and kernel start identity is durably recorded before request
input/wait, including failed children. Pending backend actions refresh only the
exact auth item after checking the current token's owner; completed lifecycle
proof revalidation remains read-only and does not refresh auth.

Actions journal intent before effects. Only a previously durable DELETE-started intent can reconcile
an already absent remote Cube; a first 404 without that intent is BLOCKED. Other interrupted actions stay BLOCKED with all
recovery material retained. `state.next` residue is not blindly deleted and a
failed fixture is not replaced under a new UUID automatically. This conservative
uncertainty handling is a limitation, not an automatic crash-recovery claim.

## Validation limits

Offline tests exercise guards and actual product unfunded signing bytes only.
They cannot establish full-kit cloud execution, native Keychain runtime access,
restored signing or GUI behavior. Real signed execution requires accepted source,
working Connect/backend availability and an unlocked usable macOS Keychain.
Missing services are BLOCKED, never fixture-injected PASS. No new broadcast is
part of this suite.
