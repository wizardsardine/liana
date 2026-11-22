# BTCB2 managed pruned node

The authenticated Bitcoin Blake2b installer offers a pruned local Bitcoin Knots
node, pinned to `29.5.1.knots20260508`, for mainnet and testnet4. Connect remains
the wallet backend while the companion node completes initial block download;
the app then switches automatically after exact-chain local admission. The
installer labels the software, chain, pruning and outbound-only connectivity.

Generic fork startup remains dormant: this feature uses explicit authenticated
Connect and embedded local-node entry points, native P2WSH descriptors only.
External daemon sockets, Electrum, generic migration, Taproot and duress routes
are unchanged. Local installation is offered in the installer; installing into
an existing Connect-only Cube, inbound Tor and post-install resource editing
remain unavailable for Blake2b in this release.

The local wallet trusts its own consensus-validating node. Before wallet or
database writes, and again before wallet RPCs, the daemon reads a bounded,
coherent snapshot: the selected network, exact activation height (961640 mainnet,
150308 testnet4), active top-level `blake2b` deployment, completed IBD and a
version-2 block header at the same tip. A final best-hash read rejects a moving
snapshot; a moving tip retries the complete snapshot at most three times,
while wrong-chain admission fails immediately. Schedule-only checks admit a syncing companion, never a wallet.
Ordinary Bitcoin, absent or different schedules, pre-fork headers, inactive forks
and inconsistent tips are refused. This is local-node trust, not an independent
header proof or Connect anchor. RDTS expiry is not an admission requirement.

Connect authentication remains runtime-only, including tokens on a saved local
node's fallback configuration. The loader can restart an already admitted local
backend without a Connect feature-status request. Account unlock/session setup
continues to use the existing authenticated Cube flow.

## Upgrading an install provisioned with `29.4.1.knots20260508`

The pin moved to `29.5.1.knots20260508` because it changes consensus on both
scheduled networks; a `29.4.1` node would follow a different chain. No released
build has shipped the Blake2b node, so this affects internal installs only.

Opening the Cube handles each step of the upgrade itself; the same applies to
any later pin bump.

- **The pinned binary is re-provisioned on open.** Only the pinned version is
  searched or started, so after a bump the old binary no longer counts as
  installed. Before the loader starts the managed node it checks for
  `<coincube datadir>/bitcoind-blake2b/bitcoin-29.5.1.knots20260508/bin/bitcoind`
  and, when it is missing, downloads the pinned release and verifies it exactly
  as the installer does (`DownloadVerification::for_flavor`: the signed
  `SHA256SUMS` under the pinned Knots key, then the archive's checksum in it)
  before unpacking it under the family's lifecycle lock
  (`ensure_pinned_fork_binary`). The archive is unpacked into a private
  staging directory and its version directory moved into place only once the
  executable is complete, so an interrupted or failed unpack never leaves a
  binary that would count as installed. A failed download or verification
  installs nothing. It does not stop the start by itself: a pinned node that
  is already running needs no binary on disk and is reused. Its reason is
  shown (with Retry) only when the start then needs the missing binary. The
  unpack is refused while another app process owns the node. The old `bitcoin-29.4.1.knots20260508/`
  directory is left on disk and never started.
- **A still-running older build is replaced, not reused.** A Blake2b node's
  flavour cannot be read from its subversion, but its build can: the start
  reads `getnetworkinfo.subversion` and reuses the node only when it is the
  pinned build (`/Satoshi:29.5.1/Knots:20260508/`, `classify_blake2b_build`).
  The build date comes from the explicit `Knots:` field, not from a
  user-agent comment inside the client segment. A positively read other build
  is stopped and the pinned binary started in its place, as a Bitcoin flavour
  switch is. A subversion that cannot be read (no answer, an empty string, or
  not the `/Satoshi:<version>…/` shape) refuses the start and leaves that node
  running: it may be a busy, healthy node another Cube shares. Identifying the
  build uses a short-timeout probe and is bounded to about 5 s in total,
  since the family lifecycle lock is held. The
  replacement binary is resolved before anything is stopped, and the stop is
  refused while another app process owns the node. A freshly started node that
  reports another build is killed and refused.
- **First start rewinds the chainstate.** `29.4.1` wrote no
  chainstate-revalidation markers, so `29.5.1` rewinds once to just below the
  earliest revalidation deployment's start and reconnects from there: mainnet
  from `973440` (tip `973439`), testnet4 from `151550` (tip `151549`; the RDTS
  extension range from `152963` is covered by the same rewind). Headless
  `bitcoind` does not prompt. On the mainnet service node this took about nine
  minutes before `Done loading`; the Cube shows "Connecting" meanwhile. The
  "Found invalid chain … Chain state database corruption likely" warnings
  logged during the reconnect are expected and are not treated as a failure.
- **A pruned datadir that cannot be revalidated offers a resync.** If the
  block or undo data the rewind needs has been pruned (likely with the 550 MB
  or 5 GB prune targets), Knots exits with "The active chain must be
  revalidated for current consensus rules, but required block data has been
  pruned". The start reads only what that launch appended to the node's
  `debug.log` (after a startup log shrink, only what follows the last
  startup banner), recognises the message, stops respawning and reports
  `StartInternalBitcoindError::PrunedRevalidationRequired`. The Cube's error
  page then offers **Delete chain data and resync** beside Retry (the Retry
  cooldown never hides it). It deletes only `blocks/` and `chainstate/` for
  that network (directly under `bitcoind-blake2b/datadir/` on mainnet, since
  Knots keeps mainnet in the datadir root; under
  `bitcoind-blake2b/datadir/testnet4/` on testnet4) and starts the Cube again.
  `bitcoin.conf`, the cookie, node wallets, the log and the Bitcoin node's
  datadir are untouched. While the error page is shown and the deletion runs,
  nothing serves the wallet: the start fails before the wallet daemon is
  started.
  - **Which backend serves the wallet during the resync.** A Cube still on
    Connect with the node pending keeps that configuration: once the
    replacement node starts, Connect serves the wallet (with a Connect
    session) while the node downloads the chain, and the Cube switches to the
    node when it has synced, as after installation.
  - **A Cube whose wallet already runs on its local node** (the common case:
    pruned by default, promoted after its first sync) cannot stay on the node
    for the resync: a node in initial block download cannot admit the wallet
    (`DaemonHandle::start_with_local_node` requires a completed IBD), so every
    open would fail. The resync therefore moves it back to Connect first, the
    inverse of the App's promotion: `daemon.toml` gets the chain's Connect
    endpoint as its backend (no token is written), the local node becomes
    `pending_bitcoind` with `auto_switch_to_pending = true`, `fallback_esplora`
    is cleared and the poll interval returns to the Esplora one. The loader
    then reopens the Cube on Connect with the session it carries, and from
    there it is a Cube before its first promotion: Connect serves the wallet
    while the node resyncs, and the App switches back to the node once it has
    synced, under the usual holds (not during a Vault scan, and not to a
    pruned node that no longer holds this Vault's history). The local-backend
    loader carries the unlock's Connect session for this purpose only; the
    local open never uses it, and any sign-in change clears it.
    Nothing is deleted unless the Cube will be able to reopen:
    - Without an authenticated Connect session the resync is refused, and the
      page says to sign in from Home, reopen the Cube and resync.
    - Connect must first confirm Bitcoin Blake2b for the account (the check
      the reopen itself makes); if it cannot, the failure is shown and the
      resync can be retried.
    - `daemon.toml` is rewritten (atomically, and only if it is unchanged
      since it was read) after every refusal below has passed, under the same
      locks, immediately before the deletion
      (`resync_managed_fork_chain_data_then`). If the rewrite fails, nothing
      is deleted. If the deletion then fails, the Cube is already on Connect
      and the next resync finishes it.
    - A sign-out after the move revokes the loader like any Connect loader's;
      the Cube then opens on Connect after the next sign-in.
  - **Refusals.** The resync is refused while any Cube owns the node, while
    the node's own datadir lock (`.lock`, the same `fcntl`/`LockFileEx` lock
    `bitcoind` takes) is held by another process, while anything accepts
    connections on the recorded RPC port (a plain TCP connect, so a missing
    cookie never makes an occupied port look idle), or when that network has
    no readable managed configuration (`resync_managed_fork_chain_data`).
    The datadir lock is held through the deletion.
  - **Results.** A failed resync is shown with its reason and offered again;
    a partial deletion is finished by the next one. A result is bound to the
    loader and resync that started it, so it never reaches another Cube
    opened in the same tab, and one that finishes after the Connect session
    was revoked keeps the revocation's refusal.

Not covered: the background installer and Settings still drop Blake2b
node-setup messages, so the loader is the only re-provisioning path. The
positive install path is only exercised against the real signed release; unit
tests cover the skip, fetch, refusal and staged-publish cases with the
vendored manifest. The resync is one click with no confirmation, and an
owner on either Blake2b network blocks it. A pruned local-backend Cube is
recovered only through Connect: without a Connect account that has Bitcoin
Blake2b, it has no in-app recovery, and the resync refuses it with nothing
deleted. The move back to Connect sends the Vault's address queries to Connect
until the node is promoted again.

## What is isolated, and where

Bitcoin Blake2b keeps Bitcoin's network identity (magic, address and key
encodings), so a `bitcoin::Network` cannot tell the two chains apart. The
managed-node layer therefore keys everything on a second axis,
`NodeChainFamily`, derived from the chain's *identity* (`ChainId`), never from
its encoding:

| | Bitcoin family (unchanged) | Bitcoin Blake2b family |
|---|---|---|
| Providers | `Core`, `Knots` | `KnotsBlake2b` only |
| Root under the COINCUBE datadir | `bitcoind/` | `bitcoind-blake2b/` |
| Binaries | `bitcoind/bitcoin-<version>/bin/bitcoind` | `bitcoind-blake2b/bitcoin-29.5.1.knots20260508/bin/bitcoind` |
| Node datadir, `bitcoin.conf`, cookies, chainstate | `bitcoind/datadir/…` | `bitcoind-blake2b/datadir/…` |
| Flavour ledger | `bitcoind/managed_node_state.json` | `bitcoind-blake2b/managed_node_state.json` |
| Lock directory | `bitcoind/` | `bitcoind-blake2b/` |

Every legacy (family-less) helper — `internal_bitcoind_directory`,
`internal_bitcoind_datadir`, `internal_bitcoind_exe_path`,
`ManagedNodeState::path`, `CoincubeDirectory::bitcoind_directory` — is the
Bitcoin family's, byte for byte, so nothing an existing install relies on moves.

### Ports

Upstream gives the Blake2b build **the same default ports as Bitcoin**
(`src/kernel/chainparams.cpp`: `nDefaultPort = 8333` mainnet, `48333`
testnet4; `src/chainparamsbase.cpp`: RPC `8332` / `48332`), so "distinct
ports" cannot come from chain parameters. COINCUBE never uses the defaults for
a managed node anyway: each network section gets OS-allocated ports at setup.

Both places that create a network section — the installer's `DefineConfig`
and the settings path's `write_internal_bitcoind_config` — go through one
bounded policy (`node::bitcoind::reserved_managed_ports` +
`allocate_managed_ports`, reached through the locked transaction's
`ManagedConfTxn::allocate_ports`):

- candidates come from the OS (`get_available_port`); the policy itself is
  deterministic and tested with an injected candidate sequence;
- a candidate is refused if it is a bitcoind default, a repeat, or **already
  recorded** by any managed-node conf under the datadir: this conf's other
  networks (e.g. testnet4 when allocating mainnet), any section of the other
  chain family's `bitcoin.conf`, and either conf's recorded Tor control/SOCKS
  ports (`torcontrol`/`proxy`) — bound right now or not, since a stopped node
  still owns its ports;
- an existing section keeps its ports; nothing already assigned is rewritten;
- absent confs reserve nothing, but an other-family conf that **exists and
  cannot be read fails the allocation closed** — its ports are unknown, and
  live socket probing would not notice a stopped node. "Absent" means a true
  `NotFound` from `fs::metadata` (`InternalBitcoindConfig::from_file`); a
  conf under a directory the process cannot traverse is a read error, never
  absence, for own-family and other-family reads alike. Refusals happen
  before the flavour ledger is touched.

### Serialisation and atomic persistence (`node/managed_conf.rs`)

Allocation and every other writer of a managed `bitcoin.conf` are serialised
by **one datadir-wide, OS-backed exclusive lock**, `<coincube_datadir>/managed-node.lock`
(`ManagedConfLock`: `fs4` `try_lock_exclusive`, i.e. `flock`/`LockFileEx`).
It lives outside both family roots because it guards the shared port space
and both families' files at once. It is never deleted or truncated in normal
use and carries no state; it is released explicitly on drop (including on a
panic) and by the OS when the holding process dies, so there is no stale lock
to break. Acquisition is bounded (40 × 50 ms, the same convention as the
node-identity marker lock); exhaustion is `Busy`, a retryable refusal.

`update_managed_conf(datadir, family, edit)` holds the lock from a **fresh**
read of the family's conf, through the edit (reservation across both
families, candidate selection, the ledger record the edit chooses to make),
to the atomic replacement of the file. The writers that go through it:

Two failure classes are worth telling apart. An **early refusal** — the lock
is `Busy`, the family's own conf exists but cannot be read, the other
family's conf cannot be read, no acceptable port — happens before the edit
records anything: conf *and* ledger are untouched. A **write failure**
(`NotReplaced`) happens after the edit ran: the conf still holds its previous
bytes, but whatever the edit recorded on its way — for the installer and
settings writers, the flavour ledger (`record_configured` runs inside the
edit, ahead of the conf write) — stays recorded. The two files are not one
transaction and no rollback is attempted; the next successful write brings
the conf in line with the ledger.

| Writer | When | Early refusal (`Busy` / unreadable / no port) | Write failure (`NotReplaced`) |
|---|---|---|---|
| installer `DefineConfig` | node setup | step error; conf and ledger untouched | step error; conf unchanged, ledger may already name the flavour |
| settings `write_internal_bitcoind_config` | setup, flavour switch, restart-to-apply, resources apply | `Err` before the ledger; nothing started | `Err`; nothing started; conf unchanged, ledger may already name the flavour |
| `tor::prepare_inbound_tor` (outbound-only reset, then the inbound merge once Tor is up) | every managed start | **start refused** (`StartInternalBitcoindError::ConfigUnavailable`) — the file may still name a Tor that is not running, and a node must not be started from stale privacy configuration | same refusal; it records nothing else |
| `bitcoind::maybe_start` (rewrite of the parsed conf before the spawn; drops the legacy `consensusrules` line `from_ini` tolerates) | every spawn of a managed Bitcoin-family node: after the same-flavour reuse check (which takes no lock) and before a mismatched running node is stopped for a flavour switch | **start refused** (`ConfigUnavailable`, the Tor preparation's class); the file is untouched, so a legacy line stays until the next start, and a running node the call would have switched away from is left running. Two routes stop the node *before* reaching any locked rewrite and can therefore be left with no node until the retry: the loader when inbound-over-Tor is being applied (`loader.rs`, after the Tor preparation's own locked rewrite) and the settings "restart to apply" (`settings/bitcoind.rs`, which stops before the Tor preparation; pre-existing) | same refusal; it records nothing else |

Because each rewrite reads under the lock, a section another setup persisted
a moment earlier is never erased by a stale snapshot. Tor's bootstrap — the
one slow step — runs **outside** the lock: the conf is reset to outbound-only
under the lock, the previous managed Tor of this process is stopped **only
after that reset is durably in place**, Tor is started with ports chosen
around everything recorded, and only then are the inbound fields merged onto
a second fresh read. A refusal at the reset (lock busy, conf unreadable or
not replaceable) therefore leaves both the conf and the already running Tor
exactly as they were — a sibling session's node in the same process keeps
the Tor its conf still names — while the success and "no conf" paths stop
it as they always did. The lock is a leaf: nothing else is acquired while it
is held, and it is never taken while the marker lock is held.

Replacement is atomic (`write_conf_atomically`): the bytes are staged in a
uniquely named private sibling (`bitcoin.conf.<pid>.<seq>.tmp`, mode `0600`,
created with `create_new`; a name that already exists belongs to someone else
and is left untouched while the write moves to the next name, bounded),
flushed, given the destination's existing permissions if there is one (a
user's restrictive mode survives; a fresh conf starts private, since it can
carry `rpcauth`), renamed into place, and on unix the parent directory is
flushed. A reader sees the previous complete file or the new complete file.
The two failure classes are reported apart and handled differently: a failure
**before** the rename (`NotReplaced`) leaves the destination byte-identical
and removes only the staging file this call created; a failure **after** it
(`ReplacedNotDurable`) leaves the complete new bytes in place with only the
directory entry's crash-durability unconfirmed — callers proceed and log it.
Not every error means "nothing was written", and the conf and the flavour
ledger are two files, not one crash-atomic transaction (the ledger is still
recorded first; see the table above for what a write failure leaves behind).

What the lock does **not** do: it does not reserve a port against a process
that never takes it — a third-party program, or an older COINCUBE binary
running against the same datadir — and OS bind-and-release remains the
candidate source, so a port can in principle be taken between our release
and the node's (or Tor's) own bind; that collision fails the start loudly
rather than silently. Tor's fresh ports are only persisted once Tor is
actually up. Cancelling an `iced` task or dropping a `JoinHandle` does not
abort a `spawn_blocking` worker: the lock is released when that worker
returns or unwinds, not before. Cross-process behaviour is established by a
real child-process test (contention, explicit release, and release on the
child's death), in addition to the in-process thread tests.

### Fail-closed guards

`NodeFlavor::check_chain(chain)` refuses a provider/chain pairing across
families, and it runs before any side effect:

- installer step: on `SelectFlavor`, `DefineConfig` (before the conf or ledger
  is written), `Download` (before the request) and `Install` (before unpack);
- Vault settings (`app/state/vault/settings/bitcoind.rs`,
  `provider_serves_network`): the managed-flavour pick and retry
  (`start_internal_node_setup`, before the download or binary lookup), the
  completed-download handler (before the manifest fetch and install), the
  node-card flavour switch (never armed, and refused on confirm),
  `RestartNodeToApply` and `NodeResourceApply` (before the node is stopped,
  Tor touched or the conf rewritten) — and, independently, the helpers behind
  them: `ensure_tor_and_start_managed` (before Tor is provisioned),
  `configure_and_start_internal_bitcoind` (before the archive is unpacked)
  and `write_internal_bitcoind_config` (before the conf, ledger or identity
  marker is written). The pickers list Core/Knots only; these guards pin the
  boundary for a provider that reaches the pending setup or the ledger by any
  other route;
- `Bitcoind::maybe_start` (every start path: loader, installer, settings):
  before the identity marker is written, the conf migrated or a binary
  resolved;
- `Bitcoind::maybe_start_for_chain` selects the family by `ChainId`, checks
  the configured provider, exact managed cookie and persisted RPC address, and
  checks the running node's fork schedule before reuse. A wrong-family node is
  refused. The fork path never runs Bitcoin's flavour reconciliation.

`select_managed_bitcoind_exe` searches only the provider's own family root, so
a Bitcoin chain never resolves a Blake2b binary and the Blake2b provider has no
Bitcoin fallback. `InternalBitcoindConfig` never emits `consensusrules` for any
flavour.

`reconcile_after_start` (`node/revalidate.rs`; the flavour-ledger record is
all that remains of the RDTS repair machinery after sunset PR 4, #510) refuses
a Blake2b chain, or a Blake2b provider on any chain, **before** loading the
ledger, so a Blake2b start can neither read nor rewrite the Bitcoin node's
flavour ledger.

### Release verification

The Blake2b release is published under the Knots URL scheme
(`https://bitcoinknots.org/files/29.x/29.5.1.knots20260508/`) and signed by the
already-pinned Knots key (`KNOTS_SIGNING_KEY_FINGERPRINT`,
`1A3E761F19D2CC7785C5502EA291A2C45D0C504A`). No new trust root is introduced:
`DownloadVerification::for_flavor` builds the same `ReleaseManifest`
verification for both Knots providers, and the version alone selects the
manifest. The real `SHA256SUMS` + `SHA256SUMS.asc` for this release are vendored
under `coincube-gui/src/installer/step/node/test_fixtures/knots_blake2b_*`; the
test proves the signature verifies under the pinned key and that every platform
archive name the provider derives (`arm64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-linux-gnu`, `aarch64-linux-gnu`, `win64-pgpverifiable.zip`) is listed.

## Daemon dependency: chain health on a Blake2b node

Activating the provider needs `coincubed` to read the hardfork's status. The
fields below were taken from Knots tag `v29.4.1.knots20260508` and rechecked
at the pinned `v29.5.1.knots20260508` (`src/rpc/blockchain.cpp`,
`src/kernel/chainparams.cpp`, `src/chainparamsbase.cpp`); they are **not** what
the desktop's existing `reduced_data` probe assumes. `29.5.1` adds a
`deployments.long_coinbase_maturity` entry, which the probe ignores, and omits
`difficulty` from `getblockchaininfo`/`getblockheader` for header-v2 blocks
(reporting `difficulty_blake2b` instead); `coincubed` reads neither, and the
launcher's local-node activity reader converts `difficulty_blake2b` to a
difficulty multiple.

### `getdeploymentinfo`

```json
{
  "hash": "...",
  "height": N,
  "deployments": {
    "...": { },
    "reduced_data": {
      "type": "flagday",
      "height": <Blake2bHeight>,
      "expiry_time": <RdtsExpiryTime>,
      "active": true | false
    }
  },
  "blake2b": {
    "height": <Blake2bHeight>,
    "active": true | false
  }
}
```

- **`blake2b` is a top-level object, a sibling of `deployments`, present only
  when a hardfork height is configured** (`blockchain.cpp`, the
  `getdeploymentinfo` result: `"blake2b", /*optional=*/true, "hardfork
  schedule, present only when one is configured"`; emitted when
  `consensus.Blake2bHeight != std::numeric_limits<int>::max()`). `active` is
  `DeploymentActiveAfter(blockindex, DEPLOYMENT_BLAKE2B)`: whether the hardfork
  rules apply to the block *after* the queried one. This — not a `deployments`
  entry — is the fork-activation signal.
- **`deployments.reduced_data` is a flag-day deployment with an expiry**
  (`RdtsFlagDayDescPushBack`): `type` is `"flagday"`, `height` is the RDTS
  activation height (the BLAKE2b hardfork height), `expiry_time` is
  `RdtsExpiryTime`, and `active` is `RdtsActiveAt(height + 1, parent MTP)` —
  it turns **`false` again once the parent's median-time-past reaches
  `expiry_time`**. It is omitted entirely when unscheduled. There is no `bip9`
  sub-object on this build.
  RDTS previously activated via versionbits; that deployment was removed after
  the stall at 961633 (`chainparams.cpp`, mainnet comment).

### Scheduled heights and expiries

| Network | `Blake2bHeight` | `RdtsExpiryTime` |
|---|---|---|
| mainnet | `961640` | `1819756800` (2027-09-01 00:00 UTC) |
| testnet4 | `150308` | `1806537600` (2027-04-01 00:00 UTC; `1791903600`, 2026-10-13, before `29.5.1`) |
| regtest | unscheduled by default | unscheduled by default |

Source: `src/kernel/chainparams.cpp` at the tag (mainnet `CMainParams`,
testnet4 `CTestNet4Params`, regtest `CRegTestParams`).

### Regtest

Regtest schedules nothing unless asked (`src/chainparamsbase.cpp` argument
help; `src/chainparams.cpp` `ReadRegTestArgs`; `src/kernel/chainparams.cpp`
`CRegTestParams`, lines 633–665 at the tag). The two settings are independent
in what they emit:

| Flags | `Blake2bHeight` | `RdtsExpiryTime` | `getdeploymentinfo` |
|---|---|---|---|
| none | unset (`INT_MAX`) | unset (`INT64_MIN`) | neither `blake2b` nor `deployments.reduced_data` |
| `-testactivationheight=blake2b@N` only | `N` | unset | top-level `blake2b: {height: N, active}` **is emitted** (`blockchain.cpp:2029`, gated on height alone); `reduced_data` **absent** (`RdtsFlagDayDescPushBack`, `blockchain.cpp:1951–1952`, needs height *and* expiry) |
| `-testactivationheight=blake2b@N -rdtsexpiry=T` | `N` | `T` | both: `blake2b` and `reduced_data {type: flagday, height: N, expiry_time: T, active}` |
| `-rdtsexpiry=T` without the height | — | — | **startup error**: `-rdtsexpiry requires -testactivationheight=blake2b@<height>` (`src/chainparams.cpp:84`); `T <= 1296688602` (regtest genesis time) is likewise rejected (`:91`) |

`-blake2b_headline=<headline>` overrides the consensus-critical headline and
also requires the activation height (`src/chainparams.cpp:98`). RDTS activates
at the BLAKE2b height; only its expiry is separately settable
(`kernel/chainparams.cpp:660–665`).

A chain-health probe must therefore read the two objects independently: a
node with `blake2b` but no `reduced_data` is a scheduled hardfork with RDTS
unscheduled — not a failed deployment, and not "unscheduled" either.

## Runtime and Tor isolation

Fork starts resolve only the fork release under `bitcoind-blake2b`, with that
family's cookie, marker lock and flavour ledger. Existing Bitcoin files retain
their paths and contents. A missing fork binary cannot fall back to Bitcoin.

`prepare_inbound_tor_for_chain` clears stale inbound fields in the fork's own
configuration under the shared managed-conf transaction. It never provisions,
replaces or stops the Bitcoin Tor process. Fork nodes use outbound P2P connections
and cannot accept inbound peers in this release. A separate managed Tor lifecycle
is required before inbound Tor can be offered for Blake2b.

Validation covers real HTTP RPC snapshots and rejected wallet mutations, family
ledger/config/port isolation, installer choice persistence and the release's
signature/checksum fixtures. A disposable real-node smoke test can validate startup
and fork RPC fields without syncing a production chain; full mainnet IBD and a
real funded Cube are not automated tests.
