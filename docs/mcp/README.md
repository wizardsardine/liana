# COINCUBE Desktop local AI access (MCP)

Local MCP supports **Bitcoin and Bitcoin Blake2b Vaults using COINCUBE Desktop's embedded
daemon on macOS, Linux and Windows**. Supported exact chain identities are
`bitcoin`, `testnet4`, `regtest`, `bitcoin-blake2b` and
`bitcoin-blake2b-testnet4`. Normal builds include wallet proposals, public-key
wallet preparation and unsigned transaction preparation. Liquid, Spark, imported
non-Vault policies and external daemons remain unsupported. MCP starts disabled. Explicitly paired clients keep their selected scope until
revoked; protected access pauses while the Cube is locked or COINCUBE Desktop is closed.
No Connect API or mobile Keychain changes are needed.

## Integration and release status

Local MCP access is integrated on `redesign`; the repository's primary branch is
`master`. Build from the checkout containing the changes being tested. Presence
on `redesign` does not imply inclusion in a production release. Follow-up work
assigned to this integration targets `redesign`.

Review and CI completion make a PR eligible for human review. Merging to
`redesign`, promoting it to `master`, and publishing a release remain separate
owner decisions. Packaged-client interoperability and signed-artifact checks
remain release gates.

## Build and install

Use the repository's pinned Rust toolchain and normal desktop prerequisites
(`CLAUDE.md`, `CONTRIBUTING.md`). The official Rust MCP SDK is pinned to `rmcp
3.6.0`; Cargo.lock pins its dependencies. No Node/Python runtime or AI API key is
needed by the MCP executable.

```sh
make build               # Spark bridge, desktop and tenshu-mcp
make run                 # builds bridge + adapter, runs desktop
# Adapter alone, for development:
cargo build -p coincube-mcp --features adapter --locked
cargo test -p coincube-mcp --features adapter --locked
```

Debug binaries are `target/debug/coincube` and `target/debug/tenshu-mcp`.
`make release` uses `target/release/`; production workflows use `target/<triple>/minimal/`.
The Linux tarball includes `tenshu-mcp` beside `coincube`. The macOS bundle includes
`COINCUBE.app/Contents/MacOS/tenshu-mcp` beside `Coincube`. The release/nightly workflows
build and stage it before signing. Packaging changes still need validation by CI
and a signed-release smoke test. Both macOS release architectures use native
runners and execute `contrib/check_mcp_binary.py` on the signed adapter before
publishing; the check covers initialization, discovery, authenticated fixture
reads and clean shutdown. The checker expects both preparation workflows and
schema version 2 in normal packaged builds. For deliberately minimal builds,
pass `--no-proposals` and/or `--no-transactions` to match the disabled features. Do not copy a development executable into an
already signed application bundle.

## Pair a client until revoked

1. Open and unlock the desired Cube normally. Go to **Cube → Settings → AI access**.
   The page shows this Cube's switch and Vault (with its exact chain tag), the paired
   clients, and an **Access** section summarizing each kind of access. If the Cube has no supported Vault
   loaded, the switch reads **Unavailable for this Cube** and names the supported scope.
2. Turn **Allow AI clients** on. This Cube-wide switch grants no new access by itself:
   it resumes existing clients with their stored scope, and turning it off later pauses
   every paired client without forgetting them.
3. Choose **Pair a client**. Enter a **Client name**, explicitly select **This Cube's
   Vault**, then choose one kind of **Access**: **Read wallet information** (then switch
   on each part you allow: Balances, Confirmed history, Spending-policy summary) or
   **Prepare unsigned transactions**. Each kind is its own pairing; a client that
   should do both needs two. The name is for your reference, not an authenticated
   application identity. Read the financial-data caution beside the pair action.
4. Choose **Pair client until revoked**. COINCUBE Desktop saves the exact client scope and a
   random bearer credential in a device-keystore-protected encrypted registry. The
   form closes and the new client's row opens.
5. Choose **Copy private client configuration** and paste it into your trusted MCP
   host's local configuration. COINCUBE Desktop confirms **Copied** beside the action. The
   configuration contains a credential: never paste it into a chat, issue, shared
   document or repository. Clear your clipboard after setup.
6. Restart/reconnect the MCP host. Discover tools, then call `tenshu_list_wallets`
   and use the returned opaque `wallet_id` for subsequent calls.

The copied command is derived from the running desktop executable's directory.
The adapter must be installed beside it. Each client row shows its status
(**Available**, **Paused** or **Suspended**) with the reason, its access, and, under
**Manage**, the requests answered this session. **Available** means COINCUBE Desktop will answer
that client while the Cube is unlocked; neither the status nor the count is a verified
host identity or a persistent-connection indicator (IPC connections are per request).

Up to 16 independently revocable clients can be paired per Cube. Give separate
clients and Cubes separate host entries. Anyone holding a copied configuration
can impersonate that client within its exact scope while the Cube is available.
The registry is bound to the canonical data directory, chain, Cube, wallet storage
identity and descriptor; a replaced Vault does not inherit its permissions.

Pairing survives normal lock, close, backend suspension and restart. Each unlocked
session publishes a fresh private endpoint through the same private reconnect
locator; the adapter resolves it on every call. Existing client configuration
continues working after ordinary unlock/relaunch. The assistant cannot unlock a
Cube, and no wallet data is returned while it is locked/closed. Disabling AI access
pauses stored clients; it does not forget them. Draft permission fields configure
a new pairing; they never silently expand an existing client's scope. To change
a client's permissions in this slice, revoke its row and pair it again explicitly.

**Revoke access** (under the client's **Manage**, then confirmed) removes that client's durable authorization and stops its
runtime immediately. Revoking cannot erase information already given to the AI
provider. Pairing files are also removed on Cube deletion and duress wipe; the
keystore wrapping item can remain inert. Storage/authentication failures restore
no access. If a durable revocation write fails, COINCUBE Desktop suspends all clients and
tries to invalidate only this registry's wrapping key; an error explicitly states
when durable revocation could not be confirmed. Do not treat that error as success.

Legacy configurations with TENSHU_MCP_SOCKET retain their one-hour session-only
semantics and are never silently upgraded. Pair again through the new UI once
to obtain a persistent configuration.

## STDIO client configuration

The GUI produces this shape, with actual paths and a private pairing credential:

```json
{
  "mcpServers": {
    "tenshu": {
      "command": "/Applications/COINCUBE.app/Contents/MacOS/tenshu-mcp",
      "env": {
        "TENSHU_MCP_PAIRING": "/PRIVATE_DATADIR/regtest/mcp-pairings/CUBE_SCOPE/CLIENT.endpoint",
        "TENSHU_MCP_CREDENTIAL": "REPLACE_WITH_THE_PRIVATE_PAIRING_FROM_TENSHU"
      }
    }
  }
}
```

On Windows the copied command ends in `tenshu-mcp.exe`; the MSI installs it beside
`coincube.exe`. The private locator resolves to a local pipe name such as
`\\.\pipe\tenshu-mcp-PID-RANDOM`, not a TCP address. Keep the generated name unchanged:
the adapter validates its format and checks the pipe server's OS process ID
before sending the credential. The pipe DACL permits only the current Windows
logon session and rejects remote clients. The bearer grant is still required.
The client grants identification-only SQOS, not impersonation rights.

Use the copied values, not these placeholders. On Linux/development builds the
command is the absolute installed/build path to `tenshu-mcp`. There are no tool
arguments for paths, credentials, shell commands or daemon RPC methods.

- **Maple AI Desktop:** preferred target, but its current custom-STDIO configuration
  UI has not been verified in this implementation. If the installed version supports
  local MCP servers, supply the command and two environment variables above in its
  local server settings. Do not confuse [Maple AI](https://maple.ai/docs) with the
  unrelated maple.dev telemetry service. No Maple inference/proxy integration is
  needed or included. Manual interoperability remains a release gate.
- **Claude Desktop:** use its local MCP developer configuration (`mcpServers` shape
  above). Local servers are distinct from cloud connectors. See [Claude's local
  MCP guide](https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop)
  and [local versus remote connectors](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp).
  A packaged `.mcpb` extension and marketplace distribution are future work.
- **ChatGPT:** this slice does not provide a direct ChatGPT connection. As checked
  2026-10-21, [OpenAI's custom MCP guide](https://help.openai.com/en/articles/12584461-connecting-custom-mcp-servers-in-chatgpt)
  describes remote connections, not direct local STDIO. No HTTP listener, tunnel,
  public relay or wallet-service deployment is added to work around this limitation.
- Other local STDIO MCP hosts can launch the same executable. The SDK handles
  negotiation, including legacy initialization and the 2026-07-28 protocol.

## Tools

All outputs are structured JSON with advertised schemas. Balance and unsigned
preparation amounts are satoshis; history rows omit exact amounts.
Tool discovery and `tenshu_get_capabilities` are public static build metadata.
They do not inspect installed wallets or grant authority. Every wallet-data call is authenticated
and scoped inside COINCUBE Desktop, regardless of what the adapter or host claims.

| Tool | Input | Result / limitations |
|---|---|---|
| `tenshu_get_capabilities` | `{}` | Public static build metadata only; no installed-wallet inventory or live consent. Schema version 2 reports supported exact chains and build capabilities; direct MCP wallet creation is prohibited by design. |
| `tenshu_list_wallets` | `{}` | Only the authorized active Vault; opaque session ID and generic display name. |
| `tenshu_get_wallet_summary` | `{"wallet_id":"…"}` | Confirmed, unconfirmed and available sats, network and sync state. |
| `tenshu_get_transactions` | `{"wallet_id":"…","limit":20}` | Up to 50 most recent confirmed activities, newest first. Rows contain a session activity id, coarse kind (`incoming`, `outgoing`, `self_transfer`) and `confirmed` status. No exact amount, time, block height, chain id, pending history or pagination cursor. |
| `tenshu_get_wallet_policy_summary` | `{"wallet_id":"…"}` | Deterministic script type, thresholds, key counts, relative recovery delays in blocks. |
| `tenshu_prepare_wallet_proposal` | Exact `network`, explicit 3-of-5 primary, recovery quorum/delay, bounded retry ID | Requires separate proposal consent in an empty unlocked Cube. Produces an in-memory topology draft; keys, actual descriptor validation and creation approval are native-only. |
| `tenshu_get_wallet_proposal` | `{"proposal_id":"…"}` | Only the current proposal grant's draft. Existing read grants cannot access proposals, and proposal grants cannot access wallet reads. |
| `tenshu_prepare_wallet_creation` | Exact `network`, public account keys and explicit topology | Separate setup pairing, actual descriptor validation and native builder; creation approval remains native. |
| `tenshu_prepare_transaction` | Exact `network`, opaque wallet/request IDs, recipients in sats, fee rate and fee ceiling | Separate transaction pairing; unsigned PSBT stays in COINCUBE Desktop, only a summary is returned. |
| `tenshu_get_prepared_transaction` | Opaque wallet and draft IDs | Current transaction pairing/session summary only; no PSBT or input/change-address export. |

The history output schema removes the former `incoming_sats`, `outgoing_sats`,
`timestamp` and `height` fields. They are absent, not optional or null. Clients
that used those fields must use the coarse `kind` and `status` instead.

Normal builds report `wallet_proposals:true` and `transaction_preparation:true`.
Schema version 2 uses `proposal_support.networks` and
`transaction_support.networks` to list the five exact supported chains. This
static metadata is not permission. Read-only AI access cannot grant preparation
authority. There is no direct MCP approve/install/sign/broadcast tool.
Wallet list entries include their exact `network`. Every preparation request
must explicitly supply the paired Cube's identity; missing or different chains
are rejected. Bitcoin and Bitcoin Blake2b share address/key encodings, as do their
Testnet4 variants, so an address or xpub prefix cannot establish chain identity.

Discovery, static capabilities and dispatch use the same companion build flags,
enabled by default:
`native-proposals` enables the three proposal tools, and `native-transactions`
enables the two transaction tools. Calling a disabled tool by name returns the
protocol error `unknown_tool` before opening desktop IPC, even if the desktop
has a matching grant. Enabled tools still require their separate desktop scope.

The initial proposal shape is deliberately limited to a 3-of-5 primary and one
explicitly chosen recovery path (1–5 slots, valid threshold, positive block delay)
for P2WSH on the selected supported chain. No recovery values are silently
selected by COINCUBE Desktop. The topology-only tool requires the intended `network` and
accepts no keys, descriptors, paths, labels or credentials. Separately scoped
public-key wallet assembly is described below.
Only one draft per proposal grant is retained. An exact retry returns that draft;
changed content or a second request returns `proposal_conflict`. Revocation clears
access, and native setup must independently verify the live session before using
a draft. An initial topology draft reports `needs_keys` and `descriptor_validated:false`; only a trusted native callback can advance that validation status.

Confirmed balance excludes coins with an ongoing spend and includes only confirmed
UTXOs. Unconfirmed includes unspent change; the UI groups self-change differently.
Available follows the existing owned-coin predicate and excludes immature/spending
coins. This is coin availability, not proof that signatures or recovery conditions
are satisfied. Snapshots can change between calls and during sync; do not interpret
an unsynced zero as authoritative history.

Balance `sync_state` is `syncing` during blockchain/wallet synchronization or a
pending rescan (even when its reported progress is 100%). It is `sync_failing`
for repeated poll failures, a refused reorganization, unresolved chain divergence,
or invalid blockchain sync progress. Only a healthy snapshot reports `synced`.
The wallet identity and health are checked before and after reading balances;
either unhealthy observation prevents a `synced` result. Backend diagnostics and
scan timestamps are never exported.

History has no completeness field, so it returns `wallet_unavailable` while
either health observation is not `synced`, instead of presenting an incomplete
or empty list as authoritative. It checks identity and health again after the
history query. These checks are not an atomic chain snapshot: ordinary tip
changes and later wallet updates can still change results. History remains
limited to the requested number of recent confirmed entries.

History kind is wallet-relative:
`incoming` means the wallet did not spend an input, `outgoing` means it spent
to a non-wallet output, and `self_transfer` means it spent only to itself.
The rows retain order and coarse type/status, so a client could still correlate
activity with information it already has. The reduced metadata does not guarantee
transaction anonymity.
Key counts count policy keys, not independently verified people/devices. Recovery
conditions do not mean recovery can be performed now.

No seeds, private keys, passwords, credentials, descriptors, wallet xpubs, raw
scripts, labels, PSBTs or chain transaction ids are exported. Read tools omit
addresses. History rows use a session activity id that is not a block-explorer
lookup key; a new unlock mints new ids. Exact history amounts, confirmation times
and block heights are omitted. A separately
authorized transaction tool echoes only its caller-supplied recipient addresses
and amounts; the AI provider may receive those results. Topology/wallet drafts
change bounded session memory. A successful transaction preparation reserves an
internal change index through the existing engine, without saving a spend, signing
or broadcasting. A preparation rejected for fee, rate, dust or another construction
error does not reserve that index. Preparation tools are annotated as state-changing
and are advertised only in builds that include them. Signing-session
status is deferred because the existing remote and LAN workflows do not share a
single authoritative read API.

## Privacy and troubleshooting

Local transport keeps this integration off the public network; your AI host may
still upload tool results to its provider. Check that host's privacy configuration.
The server makes no AI calls and adds no telemetry. Existing wallet synchronization
continues through the existing backend. A compromised OS account can inspect
memory/configuration; this feature does not sandbox arbitrary same-user malware.

- `access_denied`: invalid/expired/revoked grant, wallet outside the grant, or
  capability not selected. Wallet absence and lack of permission are intentionally
  indistinguishable. Review the grant in COINCUBE Desktop; never let the AI approve itself.
- `wallet_unavailable`: desktop/daemon closed or unavailable, stale socket path,
  ten-second read timeout, transport failure, or history requested while wallet
  synchronization is incomplete/unsafe. Inspect COINCUBE Desktop's sync state and retry
  after it recovers; this error does not mean the wallet has no activity.
- `invalid_input`: unknown fields, missing/wrong fields, wallet ID over 64 bytes,
  or history limit outside 1–50. Tool errors never return raw daemon exceptions.
- No executable: build `make mcp` with the same Cargo profile/target as the desktop,
  or install a bundle containing the adapter. Never use a shell wrapper from an
  untrusted source to work around a path error.
- Testing inside an OS sandbox may require permission to create Unix sockets.

## Validation and follow-up

`coincube-mcp/tests/security.rs` covers the SDK and actual STDIO process, typed
inputs, permissions, per-client isolation, expiration, revocation during reads,
shutdown, disconnection, Unix socket permissions, Windows logon-session ACLs and peer identity, bounded frames and sanitized errors.
`coincube-mcp/tests/production_proposals.rs` covers required exact-chain inputs,
pairwise chain rejection before validation/storage, and immutable retries. Native
creation tests check mainnet/test-network public-key versions and managed fork
backend admission without installing a wallet.
`app::local_mcp::tests` exercises trusted settings and the actual embedded daemon
against a disposable SQLite wallet containing public-only synthetic transactions;
the blockchain backend is a test double. It is not a funded regtest/full client UI
acceptance run.

An additional acceptance test launches the actual STDIO executable using the
private configuration issued by desktop Settings, reads all four tools from that
SQLite wallet, and verifies scope denial, locking, revocation and clean EOF. It
runs explicitly in the pinned platform CI jobs after building the companion binary:

```sh
cargo build -p coincube-mcp --features adapter --locked
TENSHU_MCP_TEST_ADAPTER="$PWD/target/debug/tenshu-mcp" BREEZ_API_KEY=DUMMY_BREEZ_API_KEY cargo test -p coincube-gui --lib stdio_client_reads_real_wallet_and_observes_revocation -- --ignored
TENSHU_MCP_TEST_ADAPTER="$PWD/target/debug/tenshu-mcp" BREEZ_API_KEY=DUMMY_BREEZ_API_KEY cargo test -p coincube-gui --lib stdio_acceptance_tests --locked -- --ignored --test-threads=1
```

The preparation acceptance tests launch the actual companion with native
Settings-issued persistent pairing configuration on every supported exact chain.
Public-key creation crosses STDIO, native IPC, descriptor compilation, installer
handoff and native review/admission. Unsigned transactions cross the same client
and IPC boundaries, use the embedded SQLite wallet engine, and execute the native
Review action into the Send panel. Both cover cross-chain/scope rejection,
immutable retries, retrieval, locking and revocation. The blockchain backend and
pairing wrapping key are synthetic. Creation stops before wallet installation;
transaction review never saves a spend, signs or broadcasts. These automated
client tests do not establish Maple/Claude UI, OS-keystore, hardware, real-node
or signed-package acceptance. Pinned Linux, macOS and Windows CI run them explicitly.

The shared test-only STDIO driver bounds initialization to ten seconds, replies
to fifteen seconds (the native IPC deadline is ten), and EOF shutdown to five.
It checks response identity and frame size, drains stderr, and reports the failed
phase/request ID, child exit status, stderr byte count and a recognized fixed
startup diagnostic without printing credentials or arbitrary process output.
Set `TENSHU_MCP_TEST_TRACE=1` for per-request phase timing without tool arguments.
The earlier three-second proposal response timeout was observed once in a smoke
run and passed unchanged in isolated and complete-suite reruns; its cause was
not established. The bounded driver adds diagnostic evidence and allows native
preparation to finish within its existing deadline; it does not alter production
transport timeouts or retry a failed operation automatically.

```sh
BREEZ_API_KEY=DUMMY_BREEZ_API_KEY cargo test -p coincube-gui --lib app::local_mcp::tests
cargo clippy -p coincube-mcp --features adapter --all-targets -- -D warnings
cargo fmt --all -- --check
```

Capability-contract regressions cover each independent companion feature
combination. Run all four when changing discovery, metadata or dispatch:

```sh
cargo test -p coincube-mcp --no-default-features --features adapter --locked
cargo test -p coincube-mcp --no-default-features --features adapter,native-proposals --locked
cargo test -p coincube-mcp --no-default-features --features adapter,native-transactions --locked
cargo test -p coincube-mcp --no-default-features --features adapter,native-proposals,native-transactions --locked
BREEZ_API_KEY=DUMMY_BREEZ_API_KEY cargo test -p coincube-gui --lib mcp_read_health --locked
```

Before production release, complete packaged-client acceptance on each platform with
Maple and Claude, verify signed packaging, and obtain independent security review.
Windows named-pipe and pairing-file security are included. Unsigned transaction
preparation has a separate capability, scoped operation ID and trusted desktop
review. Signing remains with existing user/Keychain mechanisms; read permissions
cannot authorize preparation or signing.

## Native wallet proposals

Normal desktop builds enable `mcp-proposals`; normal companion builds enable
`native-proposals`. The workflow requires an **empty, unlocked Cube** on one of
the five supported chains. Capability discovery reports the build support and
bounded topology limits; that report is never live consent. Minimal builds can
omit these features explicitly with `--no-default-features`.

1. Discuss signer independence, shared failure locations and loss of any two
   primary signers. Choose the recovery quorum and positive relative block delay
   explicitly. Block time varies; possession/control of keys and recovery
   eligibility are separate from a descriptor's mathematical validity.
2. In an empty Cube on the intended chain, turn on **Allow AI clients**, choose **Pair
   a client** and select the separate **Propose a wallet policy** access. Existing wallet-read grants cannot prepare a proposal.
   Save the private client configuration only in the trusted client's local file.
3. The client prepares one topology draft with `network` matching the paired
   Cube. It receives `needs_keys` with no public/private keys or descriptor. In COINCUBE Desktop, click **Review proposal and
   select keys**. This native click opens the existing key picker, backup and
   hardware registration flows. These preparatory actions have their existing
   local/device confirmations; they are not MCP approvals.
4. Select real public keys locally. The wallet engine compiles the descriptor.
   The final native review verifies that its actual 3-of-5 primary and selected
   recovery exactly match the proposal, and displays the account xpub fingerprint
   computed from each assistant-supplied key. The origin fingerprint in that key
   was supplied by the assistant and is not presented as a checked device identity.
   Only then does proposal status report
   `ready_for_native_review` / `descriptor_validated:true`. Revisions advance
   when validation is invalidated/restored. This proves neither independent
   custody nor a completed key/backup verification.
5. **Create this Vault** is a native action. Immediately before any
   wallet/daemon persistence, a shared per-Cube OS reservation, captured session
   generation, live grant and exact review snapshot must all pass. There is no
   approve/install/sign/broadcast MCP operation. Bitcoin-family creation uses
   a native-configured Bitcoin node. Bitcoin Blake2b creation uses COINCUBE Desktop's
   isolated managed Knots Blake2b node, including the existing saved beta/global
   feature gate and exact-chain backend admission. Arbitrary fork RPC and
   Connect/provider enrollment are not added by this workflow. Creating consumes
   the current proposal runtime authorization without deleting persistent
   client pairing.
6. Completion returns to the launcher. Unlock afresh. A proposal-only pairing
   stays registered but does not gain permission to read the new Vault. Explicitly
   pair a read-scoped client to inspect its policy/balance; future setup workflow
   orchestration is a later slice. Old proposal IDs never become wallet-read authority. Lock before admission discards setup; lock
   after admission hides setup content while admitted native completion may
   finish. A grant revocation cannot undo already-admitted durable work.

Failed/partial admitted installations retain a per-Cube admission tombstone and
never automatically delete an existing/partial wallet or retry it. Inspect the
local outcome before cleanup/retry; no MCP method can clear the tombstone. The
reservation is retained through native post-install association. Closing the
process can interrupt completion, which is a recovery/inspection case, not a
claim of rollback. Enabling this workflow in normal builds does not replace
packaged-client, hardware and signed-release acceptance.

Keychain guidance uses existing setup and verification screens. The owner's
Cube Recovery Kit encryption key is not a Vault signing key. Do not label a
manually entered key as phone-attested, or claim that a backup/recovery rehearsal
passed because a key or descriptor exists. Phone/Connect-dependent setup is a
separate manual checkpoint when unavailable in local setup.


## AI-assisted wallet preparation

With the default `mcp-proposals` GUI feature and matching `native-proposals`
companion, a separately paired client can call `tenshu_prepare_wallet_creation`.
It assembles one immutable 3-of-5 primary policy with an explicitly chosen timed recovery path
and public account-key assignments. The desktop compiles the actual P2WSH
descriptor using the existing wallet engine and fills the native builder. It does
not create a wallet or assert that devices, independent custody or backups were
verified.

In an unlocked empty Cube on a supported chain, open AI access, turn on **Allow AI
clients**, choose **Pair a client**, select the **Prepare a wallet from public keys**
access, and choose **Pair setup client until revoked**. This is a distinct durable scope. Existing balance/history/
policy and topology-only clients do not gain this permission. Each client still
needs the user's ordinary Cube unlock; drafts and approval do not survive lock.

Arguments are a JSON object with `request_id`, required `network`, `topology` and `keys`
(this example selects Testnet4):

```json
{
  "request_id": "inheritance-1",
  "network": "testnet4",
  "topology": {
    "primary": {"threshold": 3, "key_count": 5},
    "recovery": {
      "quorum": {"threshold": 2, "key_count": 3},
      "relative_delay_blocks": 144
    }
  },
  "keys": {
    "primary": ["[fingerprint]tpub…", "[fingerprint]tpub…", "[fingerprint]tpub…", "[fingerprint]tpub…", "[fingerprint]tpub…"],
    "recovery": ["[fingerprint]tpub…", "[fingerprint]tpub…", "[fingerprint]tpub…"]
  }
}
```

Replace the placeholders with public account xpubs supplied for this setup. Only
origin-bearing account keys without derivation suffixes or wildcards are accepted.
Use `xpub` encoding for Bitcoin/Bitcoin Blake2b and `tpub` encoding for
Testnet4/Bitcoin Blake2b Testnet4/Regtest; the origin path must match the account
depth. All assignments need
distinct fingerprints and underlying account keys in this first slice. For an
account deeper than the master key, the origin fingerprint is caller-supplied and
is not checked against the xpub. Native review shows the account xpub fingerprint
and says the origin was not checked. Distinct
fingerprints are not proof of separate ownership. Private keys, seed phrases,
PINs, arbitrary descriptors and filesystem paths are not inputs. The required
`network` confirms the paired chain; it cannot override it.
No hardware command, secret access or remote request is performed by this tool.

The returned minimized proposal identifies the topology and native descriptor
validation result, without returning xpubs or descriptors. Exact retries return
the same proposal; changed keys, topology or retry identity conflict. Use
`tenshu_get_wallet_proposal` with its opaque ID to inspect that session's state.
Then open **Review prepared wallet in COINCUBE Desktop**. The ordinary builder is populated
with the public keys and manual/unverified provenance. The user verifies hardware
and fingerprints, handles seed/backup/PIN/passkey steps, reviews the exact
configuration, and chooses **Create this Vault** in COINCUBE Desktop. Actual key
changes require a fresh proposal; aliases and other native configuration are
bound by the final native review. No MCP call can approve creation or skip these
steps. The existing local node restriction and once-only creation admission apply.

Creation consumes the proposal session and does not grant read access to the new
Vault. Wallet reads require their own explicit pairing scope. Hardware-device,
named-client and production-release validation remain separate acceptance work.


## Unsigned transaction preparation

Normal builds enable desktop `mcp-transactions` and companion
`native-transactions`. This workflow supports an existing Vault on an embedded
daemon on all five supported chains. Bitcoin Blake2b retains the existing native
backend/feature admission. Liquid/Spark, imported policies and external daemons
remain excluded. MCP cannot sign or broadcast.

In **Cube → Settings → AI access**, choose **Pair a client**, select **This Cube's
Vault**, choose the **Prepare unsigned transactions** access, then explicitly pair the
new client. This is separate
from all read and wallet-setup scopes: existing clients are not upgraded, and a
transaction client cannot read balances/history/policy. It can list only its
paired Vault to obtain its opaque wallet ID. Pairing persists until revoked;
unsigned drafts last only for that unlocked session.

`tenshu_prepare_transaction` accepts:

```json
{
  "wallet_id": "OPAQUE_ID_FROM_THIS_CLIENTS_WALLET_LIST",
  "request_id": "payment-1",
  "network": "testnet4",
  "recipients": [{"address": "BARE_TESTNET4_ADDRESS", "amount_sats": 10000}],
  "fee_rate_sat_vb": 2,
  "max_fee_sats": 1000
}
```

The request must match the Vault's exact chain, even when a different chain shares
its address encoding. The workflow uses exact integer satoshis and an explicit fee ceiling, with
1–8 distinct recipient addresses and 1–1000 sat/vB. Fiat conversion, payment URIs,
send-max, self-send, recovery-path selection, caller-supplied change addresses,
raw scripts/PSBTs and signing/broadcast arguments are excluded. The existing
engine selects eligible coins on the primary path and constructs the unsigned
PSBT. A successful draft reserves an internal change index so two drafts cannot
share a change address; that reservation happens before the user opens the review.
A draft rejected because the measured fee or sat/vB is above the request does not
reserve an index. Preparation does not reserve the
coins, save a spend, sign, broadcast, or guarantee future input availability.

The returned `fee_sats` is the actual fee. `fee_rate_sat_vb` is an estimate:
the fee divided by the maximum signed virtual size, rounded up. That size
includes the descriptor's maximum witness satisfaction and the SegWit marker
and flag, because the unsigned transaction's witness is still empty. A shorter
real signature can make the broadcast rate higher than this estimate. COINCUBE Desktop
rejects a draft when the estimate is already above the requested rate or the
fee is above the ceiling, before committing a change index. It stores the
requested rate on the PSBT. Before broadcast, it rechecks the total fee against
the ceiling and checks the fee rate against the same maximum signed size used
to prepare the draft. A shorter real signature does not cause a refusal.
For MCP drafts, recipient and change amounts must be at least their output
script's network dust limit. The exact minimum is accepted, including a
P2WPKH output at 294 sats. Ordinary spends keep COINCUBE Desktop's 500-sat wallet floor.
COINCUBE Desktop verifies the actual inputs, recipient amounts, wallet-owned change,
unsigned state and total fee before returning a minimized summary. The review
screen stays view-only until the user chooses Sign in COINCUBE Desktop. The raw PSBT,
input identities and change address stay inside COINCUBE Desktop. Request IDs are immutable
after an engine attempt: exact retries return the same draft or error without a
new change reservation, and changed input conflicts. At most eight engine
construction attempts (including failures) are retained per client/unlock. Use `tenshu_get_prepared_transaction` with the
returned draft ID and same opaque wallet ID to reread the summary; foreign IDs
and other scopes are denied.

Under the paired client's **Manage**, each draft shows as a **Prepared transaction**
summary with its chain tag and status (**Unsigned · ready for your review**, or **Out of
date** with a request to prepare it again), and offers **Review prepared transaction**.
It opens the exact unsigned construction in the existing review/signing screen,
without saving a spend or requesting a signature. Check every recipient address,
amount, total fee, change and recipient verification status there. Hardware, PIN,
passkey, signing and any later broadcast are user actions under the existing
native flow. Back/cancel returns to AI access. No tool claims recipient identity,
hardware possession or backup verification. Lock/close/revocation clears access
to the session drafts; ordinary unlock restores only the separately approved
pairing, not old draft identities.

Transaction construction is gated on a completed poll newer than the native
startup baseline. Failed engine constructions are cached under their immutable
request IDs and count toward the eight-attempt session bound; retrying a fee-limit
failure does not reserve change again.

The MCP-prepared native review also shows an always-visible unsigned/signature
state and change total/address section from the actual loaded PSBT and wallet
classification. It does not require expansion, saving or signing to inspect
change. Ordinary spend presentation and signing authorization are unchanged.
