# Connect account ownership of a Cube

A Cube is a file on this device. Connect accounts do not own the directory. `CubeSettings.connect_owner` records which account created the Cube, so a later sign-in on the same computer does not list, open, rename, or delete someone else's Cubes.

`None` means the owner is unknown. It does not mean the Cube is unowned or free to claim.

## Where it is stored

| Piece | Path |
|---|---|
| Field | `CubeSettings.connect_owner` in `coincube-gui/src/app/settings/mod.rs` |
| On disk | `<datadir>/<chain>/settings.json`, on each `cubes[]` entry |
| Visibility rule | `cube_hidden_from_account` |
| Launcher | `coincube-gui/src/home.rs` (`creating_account`, `restoring_account`, `hide_foreign_cubes`) |
| BTCB2 paired copy | `coincube-gui/src/services/bitcoin_import/pairing.rs` (`paired_target`) |

The datadir is `~/.coincube` on Linux and `Coincube` under the OS config directory elsewhere. `connect_owner` is `{ "user_id", "email" }`. `user_id` is the stable Connect id. `email` is the address at registration, for a "sign in as …" line, and matches case-insensitively. Hiding compares `user_id` only.

## Who is written, and when

The owner is stamped once, at the moment the Cube is written, from the account that is signed in with a loaded profile:

- PIN creation and passkey creation use `creating_account`.
- A phrase restore uses `restoring_account`, which is the same account, except a ghost restore whose listed account is not the one signed in now. That Cube is written with no owner rather than the account that happens to be active.
- A BTCB2 paired copy copies the source Cube's owner. An ownerless source produces an ownerless copy. The copy is the same person's coins on the other chain, so the same accounts see it.

Signed out, or signed in before the profile has loaded, the stamp is `None`. That Cube stays ownerless. The first account to sign in later is not evidence of who made it.

## Who can see it

`cube_hidden_from_account` decides:

- No recorded owner: never hidden. Unknown is not foreign.
- Signed out: never hidden. The PIN or passkey still gates the Cube, and it has to stay reachable offline.
- Signed in, profile loaded: hidden when `user_id` is a different account.
- Signed in, profile not loaded yet: an owned Cube is hidden. This fails closed so the previous account's Cubes do not flash during sign-in. An ownerless Cube still shows.

The launcher applies that filter to the list it just read, to the other networks' catalogs, and to the list parked by the creation-backup step. Open, rename, and delete only see what remains. A list that becomes empty is the empty-launcher state.

Catch-up registration skips Cubes this filter hides, so signing in does not register another account's Cube under the new account.

## What must not claim an owner

`mark_cube_synced` can set `connect_owner` only on the first sync of a Cube that has none, and only when the caller passes an owner. Every live caller passes `None`:

- the creation registration reply
- catch-up sync on sign-in
- a later remote update, such as a rename

A registration that lands after the user has switched accounts, or after a signed-out creation, must leave an ownerless Cube ownerless. A rename is not the creation's registration.

## Auto-open after creation

A Cube just written opens only for an account that is allowed to see it.

- Already registered (a Recovery Kit was uploaded during creation) or restored from a ghost whose account is not the current one: open only when it is not hidden. It is not registered again with whoever is signed in now.
- Signed in, and the Cube's owner is a different user than the loaded profile (or the profile is not loaded): do not register it here and do not queue an open. A queued open would fire, unprompted, when the owner signs back in. Catch-up registers it once that account returns.
- Signed in and this account may register it: register, then open when the reply arrives. A failed registration still opens the local Cube; catch-up retries the register.
- Signed out: open it.

## Constraints that are easy to break

- Do not treat `connect_owner: null` as "assign it to the active account." Cubes created before the field existed, and Cubes created signed out, are null on purpose.
- Do not pass the active user into `mark_cube_synced`. The creation paths write the owner themselves.
- Do not filter only the on-screen list. The parked backup list and the other networks' catalogs use the same rule; a reload that hid every parked Cube must not have Cancel bring them back.
- Do not give a BTCB2 paired copy the account that is signed in at pairing time. It keeps the source's owner, including `None`.
