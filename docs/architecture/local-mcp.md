# Local MCP architecture

## Implementation

COINCUBE Desktop is a Rust Cargo workspace with an Iced 0.14 desktop, not a web frontend.
`coincube-gui/src/gui/tab.rs` owns unlocked `App` instances and lock transitions.
`app/mod.rs` owns the active Cube, daemon, wallet, settings panels and shutdown.
`daemon/mod.rs::Daemon` is the existing async wallet interface. The embedded
implementation calls `coincubed::DaemonControl`; external implementations use
local JSON-RPC. The existing daemon socket includes mutating commands and is
**not** an MCP authorization boundary. Never proxy arbitrary daemon RPC methods.

The daemon stores Vault coins, transactions and unsigned spends in SQLite under
the network/wallet datadir. `list_coins`, `get_info` and `list_history_txs` supply
the current UI. `Wallet.main_descriptor.policy()` exposes deterministic primary
and relative-block recovery paths through `coincube-core::descriptors`. Spend
creation/coin selection live in `coincube-core::spend` and daemon commands; the
GUI coordinates PSBT review and existing Connect/phone signing. MCP can prepare
an unsigned transaction through a separately scoped native service; approval,
signing and broadcasting remain native actions. PIN/passkey unlocking lives in `services/unlock`
and `services/passkey`; `app/session` caches unlocked material. MCP never reads
these secrets. Liquid and Spark have distinct SDK-backed interfaces through
`app/wallets`; they are not represented as Vaults by this implementation.

Tests use Rust unit/integration tests, injectable daemon backends, and pytest
regtest tests (`tests/`). GUI build needs protoc and the existing SDK dependency
patches. Make builds the separate Spark bridge; packaging must also include the
new adapter executable.

## Integration

- `coincube-mcp`: small shared typed read/preparation protocol, revocable session gate and
  Unix socket host; official Rust `rmcp` SDK adapter behind the `adapter` feature.
- `coincube-gui/src/app/local_mcp.rs`: trusted settings, active-Vault service
  implementation, minimized projections from existing wallet APIs.
- `tenshu-mcp`: STDIO binary launched by the AI host; one fixed typed request per
  authenticated local IPC connection, no wallet engine, database or AI SDK.
- Cube Settings → AI access: opt-in, wallet scope and capability selection,
  persistent client pairing, private configuration and immediate revocation.

The service supports macOS/Linux/Windows and embedded Vault daemons on Bitcoin,
Testnet4, Regtest, Bitcoin Blake2b and Bitcoin Blake2b Testnet4. External daemon
clients log raw RPC responses at debug level and are excluded; imported non-Vault
policies, Liquid and Spark are also excluded. Unix sockets live in a randomly
created mode-0700 session directory; no TCP/HTTP listener exists. A 256-bit random bearer secret,
separate from all wallet/Connect credentials, authenticates each request. Client
names are display labels, never authentication. The UI can pair up to 16 clients
per Cube with independently selected scopes. Pairing persists until revoked;
lock/close/backend transitions suspend runtime access. After ordinary unlock,
fresh session-guarded runtimes resume the same pairing without replacing client
configuration. Legacy direct-socket grants retain their one-hour lifetime and
are not upgraded. Other Cubes require separate pairings. Windows uses a protected named pipe with logon-session ACLs and server-PID
verification (see the extension below); there is no TCP fallback.

## Trust and threat model

AI hosts, models, tool arguments and metadata are untrusted. Wallet data may leave
the device through the host even though this server is local. Consent explicitly
covers one Cube or Vault and an exact read, wallet-preparation or
transaction-preparation scope. Credentials belong in private
host configuration, never prompts or tool responses. Theft of a credential permits
its scoped operations while available until revocation (or expiry for legacy grants).
This does not defend against malware
running as the user's OS account that can inspect application memory or control
its UI; filesystem permissions alone are not client authentication.

A non-secret session generation invalidates every old runtime when the Cube
locks. Restoring a persistent pairing creates a new runtime after unlock; it
does not revive a revoked old runtime. Synchronous daemon/SQLite reads run in a bounded blocking pool; the
worker retains its permit even if its async request times out.
The desktop gate validates credential, expiry, wallet and capability immediately
before service access and again before releasing results. Revocation closes the
listener and cancels queued operations. Lock/close/backend change revokes the
runtime synchronously without deleting pairing. A request already delivered to a client cannot be recalled.
Requests/responses and concurrency are bounded and reads have deadlines. Generic
errors contain no daemon messages, descriptors, filesystem paths or wallet
existence hints. Explicit response DTOs omit seeds, keys, xpubs, descriptors,
wallet addresses, scripts, labels and PSBTs. Transaction summaries echo only the
caller-supplied recipient addresses and amounts. Unknown commands cannot reach wallet APIs.
User-controlled text never controls routing or authorization.

### Persistent pairing storage and reconnect

`app/local_mcp/pairings.rs` stores an authenticated ChaCha20-Poly1305 encrypted
registry under each network's `mcp-pairings` directory. The dedicated wrapping
key uses the existing device-secret OS-keystore backend with a separate account
derived from canonical datadir, chain and Cube identity; it is never the seed,
PIN or Cube unlock key. Existing ciphertext with a missing/unavailable wrapping
key is not reset. Context and schema are authenticated; each read pairing also
pins the exact wallet storage ID and descriptor digest. The real reader checks
the daemon's chain and descriptor before returning wallet data.

A cross-process exclusive owner lock covers registry mutation and the entire
runtime endpoint lifetime. Atomic private-file publication gives each client a
stable non-secret `.endpoint` locator. The companion resolves it for every call;
it never falls back to a remembered endpoint after errors. Locator permissions,
ownership and redirected paths are checked before transmitting credentials.
Windows storage uses a protected user-SID DACL (surviving logon changes); runtime
pipes retain their narrower logon-SID ACL and OS server-PID check. No TCP listener
or extra remote trust is introduced.

New pairing is persisted before usable configuration is exposed. Explicit revoke
stops runtimes first and atomically removes the chosen stored client; other clients
retain their credentials. A failed client addition rereads and authenticates the
registry under the same owner lock: exact unchanged storage preserves existing
clients and reports the add error; the exact intended replacement is accepted
only after retrying the directory durability barrier. Unreadable or unexpected
storage, failed durability confirmation, and failed removal/enable-state writes
stop all runtimes and attempt to invalidate this registry's dedicated wrapping key
so stale ciphertext cannot restore access. If both storage and keystore invalidation fail, the UI
explicitly reports that durable revocation is unconfirmed. This error is not a
successful revoke. Cube deletion and duress wipe remove ciphertext and locators;
keystore deletion is not required on the critical wipe path.

Proposal pairing stays separate from read scope. Its session-local proposal store
and one-shot native approval are discarded/consumed as before. Persistent pairing
does not make drafts durable, auto-renew a consumed proposal in the same runtime,
or grant reads of a newly created Vault. Those setup workflow extensions are later
slices. Backup restoration of pairing state and same-user malware are not claimed
to be rollback-proof; protect the client configuration and use explicit revoke
or reset when it may have leaked.

## Tools and semantics

- `tenshu_get_capabilities`: static build support, independent of installed
  wallets and live consent. It cannot authorize operations or reach the desktop.
  Schema version 2 reports both preparation workflows enabled in normal builds
  and exact supported `networks` lists, never live consent.
- `tenshu_list_wallets`: one authorized active Vault, opaque session wallet ID and
  exact chain identity in `network`.
- `tenshu_get_wallet_summary`: current daemon balances in satoshis and sync state;
  no promise that a policy is presently satisfiable. Identity and health are
  checked before and after the coin read. A pending rescan prevents `synced`;
  chain alerts and invalid sync progress report `sync_failing`. Existing
  full-scan, startup-poll and repeated-poll-failure checks also apply. Only coarse
  health is exposed, never backend messages or scan timestamps.
- `tenshu_get_transactions`: bounded confirmed history, newest-first; no labels,
  scripts, addresses or chain transaction ids. Each row carries a session activity
  id that changes on the next unlock and differs per client. Limits avoid inventing
  unsupported cursor semantics. History returns `wallet_unavailable` unless
  identity and health checks pass before and after the query, so an incomplete
  empty result is not presented as authoritative. This is not an atomic chain
  snapshot and does not freeze subsequent updates.
- `tenshu_get_wallet_policy_summary`: script type, deterministic signing thresholds
  and key counts, recovery delays in blocks. No prediction of recovery eligibility.

Companion tool dispatch is gated by the same default-enabled `native-proposals`
and `native-transactions` flags as discovery and static capabilities. Disabled names
return `unknown_tool` before IPC even with a matching desktop grant; enabled
operations still pass through the desktop's independent authorization gate.

Signing-session status is deferred: Connect/LAN signing state has multiple owners
and no single read-only authoritative session API. No fake status tool is shipped.
Unsigned preparation has a separate capability and trusted UI review;
no MCP operation can call signing, broadcast, unlock, import or recovery.

## Wallet preparation and native creation

The closed protocol includes `PrepareProposal` and `Proposal` for an
explicit preparation-only grant. Legacy wallet grants retain their exact read
permissions and cannot invoke either operation. Preparation grants cannot list
or read a wallet. A fresh bounded store belongs to each grant and contains at
most one topology draft: 3-of-5 primary, explicitly chosen recovery and P2WSH
on the exact paired chain. It contains no keys/descriptor, never opens a window and cannot
install or approve anything. Unknown arguments and invalid topology bounds fail
before service use. Exact retries share one draft; conflicting requests fail.

GUI `mcp-proposals` and companion `native-proposals` are enabled by default.
The trusted empty-Cube setting creates a fresh proposal-only grant/store bound
to its exact `ChainId`. Both topology preparation and public-key assembly require
an explicit `network` in the request. The store rejects a different chain before
validation or occupying its draft slot. The immutable proposal retains that chain.
Public-key assembly accepts only origin-bearing account xpubs with the matching
mainnet/test-network encoding and compiles the actual descriptor in the desktop.
The shared address/key encoding of Bitcoin and Blake2b is never used as identity. A
native review click transfers ownership into the installer; MCP cannot trigger
the transition. Existing key selection and engine validation compile real keys.
A final native review binds descriptor, target root/chain, signer provenance,
backends and security metadata in a private exact-state digest. Returning to
setup invalidates approval. No digest, descriptor or signer material is returned
to the AI.

`Grant::admit_native` serializes commit admission with revocation; its trusted
callback holds the captured native session generation lock, consumes the exact
review approval and writes an admission tombstone. A per-Cube OS lock shared by
native/proposal local installers is held across async wallet persistence and
native association. Admission checks happen inside the actual installation
future, before directory/database/daemon initialization, not merely before
scheduling it. No UI mutex is held across await. Revocation first prevents
admission; after admission the native operation may finish. Partial outcomes
are retained for inspection, never blindly deleted/retried.

The outer Tab controller enforces lifecycle routing as well as Installer.update:
pre-admission lock drops setup to Home; admitted work cannot navigate back and
renders a locked view after session end. Completion always lands at Home, never
reconstructs an App from old unlocked clients. Fresh unlock/consent is required
for reads; old proposal IDs and grants are not revived. Existing hardware
registration/backup preparatory actions retain their native confirmations and
are distinct from the wallet/daemon persistence admission boundary.

Creation uses native-configured Bitcoin nodes. Fork creation requires COINCUBE Desktop's
isolated managed Knots Blake2b backend; review classifies its exact chain and
managed configuration without creating wallet files. The existing install path
then rechecks the saved beta preference, global feature availability and exact
backend chain before daemon writes. Generic fork RPC and Connect/provider
enrollment are not enabled by this proposal path. Review displays the exact
chain and node address and binds them to the native approval digest.

No MCP tool signs, broadcasts or asserts device/backup verification. Source-specific
Keychain/backup verification remains human work in existing flows. Default build
availability does not replace packaged-client or release acceptance.

## Unsigned transaction preparation

A separate transaction grant permits bounded unsigned preparation and retrieval
for its existing embedded Vault. Requests must include the exact paired `network`,
recipient amounts in satoshis, fee rate and total-fee ceiling. The native reader
checks the trusted Cube, daemon chain and descriptor before and after engine work.
Shared Bitcoin/Blake2b address encodings cannot override the selected chain.

The wallet engine performs coin selection and constructs the PSBT. The service
validates the actual outputs, wallet-owned change, unsigned state and fee limits
before retaining the draft; raw PSBTs and input/change identities stay in COINCUBE Desktop.
Exact request retries return the same cached result; changed content conflicts.
The separate native review opens that exact draft without authorizing a signature
or broadcast. Existing native signing and broadcast checks remain mandatory.
Lock, close and revocation invalidate drafts without upgrading other scopes.

## References and startup evidence

Repository: verified `git@github.com:coincubetech/coincube.git`, base `8495c3c0`.
Instructions read: `/Users/macstudio/git/coincubetech/AGENTS.md`, brain
`MEMORY.md`, `USER.md`, `AGENTS.md`, `PROJECTS.md`, desktop `PROJECT.md` and
`ARCHITECTURE.md`; repository `CLAUDE.md`, `CONTRIBUTING.md`. No repository
`AGENTS.md` or nested instruction files were present in the tracked tree.
Shared Agent Ops origin verified and main fetched; committed policy pinned at
`b4e4fa3c3ecedad93f40d08f71baca7cea707a25` (AGENTS, README, company profile,
task contract, routing, delivery and review/repair workflow read).

Official sources checked: [Rust SDK](https://github.com/modelcontextprotocol/rust-sdk),
[MCP specification](https://modelcontextprotocol.io/specification/2026-07-28),
[security guidance](https://modelcontextprotocol.io/docs/draft/tutorials/security/security_best_practices).
Use the SDK's actual supported protocol negotiation; do not claim support for a
revision merely because it appears in a planning reference. Rust avoids adding
a Node runtime; TypeScript SDK v2 is not applicable to this codebase.

## Windows transport extension (Phase 2)

The Windows adapter retains the same grant checks and wire protocol. A random
local `\\.\pipe\tenshu-mcp-<desktop-pid>-<random>` endpoint is created with
first-instance protection and remote-client rejection. Its protected DACL allows
only the desktop process's logon SID, excluding other users and logon sessions;
no default permissive pipe ACL is used. Missing logon identity fails closed.
Before sending any credential, the adapter validates the local endpoint format
and verifies its server process ID with the OS. Client SQOS permits identification
only, preventing server-side impersonation of the adapter. These controls do not
claim protection from arbitrary malware already controlling the same OS session.

Shared input limits, deadlines, bounded workers, grant expiry and session
revocation apply unchanged. Windows CI must execute named-pipe and STDIO tests;
cross-compilation alone is insufficient. The desktop and MSI install the `.exe`
companion as a sibling. No TCP fallback or remotely addressable endpoint is added.

References: [Microsoft pipe security](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights),
[Tokio named pipes](https://docs.rs/tokio/latest/tokio/net/windows/named_pipe/index.html).
