# BTCB2 Cube creation and reopening

A fork Cube is created by Home's Create Cube journey, as a Bitcoin Cube is
(Robert, 2026-11-03): a name with a PIN or passkey, then the master seed backup.
Cube settings retain the exact ChainId and master fingerprint. The new Cube
opens through its own unlock. With no Vault yet it opens the Vault-less App
(`App::new_fork_without_wallet`): the Bitcoin Cube's screens with Create Vault
next, and no Breez client, Liquid or Spark. That unlock needs no Connect session;
it runs under the local feature check.

Create Vault inside a fork Cube runs the Vault installer with the open Cube's
session. It does not ask for the PIN again. The Vault derives from the Cube's
master seed and may run on a managed local node without Connect
(`Context::in_cube_fork_vault`). When it is saved, the Cube reopens through the
fork Loader from the session's signer. If that signer is gone, the Cube's unlock
screen opens instead. Going back from the installer returns to the Vault-less
App.

The earlier route, in which the Vault installer created a fresh fork Cube with
its own PIN step and saved the Vault signer as the Cube seed, has been removed.
The installer refuses Vault creation without a Cube on every chain, so it never
generates a master seed outside a Cube.

Successful fork PIN verification keeps the decrypted signer in a private,
one-use slot owned by that unlock screen. It does not put the seed in cloned UI
messages or the legacy shared signer cache. The slot derives the Cube encryption
key directly. No Liquid or Spark SDK is constructed. A passkey fork Cube derives
the same key from the session signer its assertion opened. Remote legacy wallet
routes remain unavailable for the fork.

A pending fork PIN screen receives a candidate from the exact current completed
Connect login after its account safety gates, scoped to its data root and API
provider. The next PIN submission rechecks that account's identity and feature availability before decrypting; it
does not automatically resume an old PIN or reuse its signer. Logout, replacement
and stale generations withdraw prior work. Hosted Connect chain data retains its
account requirement; the separately admitted managed local-node path does not
require a Connect account.

The Loader requires the current authenticated Connect client and a local Vault.
It reads only that ChainId's wallet directory, rejects config/ChainId/datadir
mismatch, and starts the authenticated embedded daemon. It never attaches to an
external daemon socket, migrates provider fallback configuration, or starts a
local node. Startup retries are user-triggered, one attempt per click, with a
Loader-owned 30-second minimum interval; failed admission cannot lose that bound
when its temporary provider is dropped.

Home or App logout/token replacement invalidates every open fork tab before the
account change propagates. The fork App is dropped into a fresh Home, removing
its wallet, signing/export panels and matching Cube PIN session. App tasks are
owned and cancelled; task, subscription and view callbacks carry the session
generation so stale signing results cannot affect a later reopened App. A CPU
operation already running may finish, but its revoked completion is discarded.
Pending installer, metadata-save and Loader tasks are
aborted; their ephemeral authority is dropped. Save and PIN completions carry a
session generation, so already queued results cannot revive a replaced session.
A late blocking decrypt writes only to its detached old slot. The fork metadata
save never opens a PIN session; the current PIN screen owns that transition.
Fork settings updates share a stable sibling writer lock and commit by atomic
replacement after a complete synced temporary file. Cancellation before commit
leaves the prior settings file intact; after commit it leaves the full new file.
The installer applies its insert/update/remove delta to locked current settings.
Touched records must still match the original snapshot (or the desired result);
a conflicting concurrent edit refuses without writing. Unrelated records, wallet
settings and preferences remain intact.
The replacement Home executes its startup task; fork discovery is read-only.
Unlock credentials are cleared and the Cube must
be reopened with the current session. Changing a backend likewise requires a new
authenticated context.

The narrow authenticated Connect capability uses the Vault-only App constructor.
Home offers BTCB2 only after the current authenticated account reports its flag
true; missing/unloaded/false flags refuse. Installer, PIN and Loader recheck the
flag, then exact-chain anchor admission validates the backend before writes.
Generic runtime support stays dormant for migration, duress and unmanaged node
paths. Mainnet and developer-only testnet4 stay separate. Existing Bitcoin paths
keep their legacy constructors and SDK behavior. Full synthetic create/reopen/
failure validation and independent final-head review remain release gates.

## Operational acceptance

The selected Connect API must expose the authenticated BTCB2 status/anchor and
Esplora routes for the exact mainnet or testnet4 identity. No JWT is persisted in
fork daemon configuration. The server's Bitcoin Esplora port3000 must never serve
these routes; BTCB2 loopback3002 is not a Mac/droplet endpoint until infrastructure
finishes indexing and provides accepted tailnet access. Testnet4 remains separately
configured and fail-closed. Live connectivity, indexing stability, and correct
endpoint acceptance remain external gates; mocks do not satisfy them.

Rollback is to keep or restore BTCB2 feature exposure off and close its session.
No Bitcoin directory migration, service restart, deployment, or existing-wallet
rewrite is part of this change. Robert retains deployment and flag control.
