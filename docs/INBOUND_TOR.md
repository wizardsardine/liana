# Inbound connectivity over Tor for the managed node

COINCUBE's managed Bitcoin node (Knots default, `consensusrules=rdts`, pruned)
can make itself **reachable** as a Tor v3 onion service — no port-forwarding, no
public IP exposure. A reachable, RDTS-enforcing node relays valid blocks and
transactions to more peers, extending the enforcing surface of the gossip
network. This is "Level 2 of the wedge": *run an enforcing node* → *make it
reachable and defend the network*.

This doc covers only the **Vault**'s managed native-Bitcoin node. It is unrelated
to the Liquid wallet (see [BREEZ_BTC_RECEIVE.md](BREEZ_BTC_RECEIVE.md)).

## What it does

When enabled, a co-located **managed Tor daemon** (the Tor Project's Tor Expert
Bundle) runs alongside `bitcoind`, and `bitcoind` is configured with:

- `listen=1`, `listenonion=1`, `discover=0` — accept inbound peers as a v3
  onion service, publish only the onion address (no clearnet address leaks).
- `torcontrol=127.0.0.1:<port>` — `bitcoind` owns the onion service, creating it
  over Tor's control port and managing its key. No manual hidden-service stanzas.
- `proxy=127.0.0.1:<port>` — optionally route **outbound** peer connections
  through Tor too (default on when inbound is on; nearly free while Tor runs).
- `maxuploadtarget` (default **~1 GB/day**) and `maxconnections` (default **20**)
  — always set when inbound is on; the metered-data protection.

Clearnet inbound, UPnP/NAT-PMP, and I2P are **out of scope** for v1.

## Default ON, with a one-click opt-out

Inbound-over-Tor is **enabled by default** for a freshly set-up Knots managed
node (early adopters skew unmetered NA/EU; more enforcing nodes is the point of
the wedge). Safeguards replace the opt-in:

- The **~1 GB/day upload cap is always on by default** (`maxuploadtarget`).
- It is **disclosed at node setup** and can be turned off in Settings → Node →
  **"Help defend the network"** at any time.
- On platforms with no Tor build (Linux/Windows **aarch64**), the feature is
  hidden and the node runs outbound-only.

It is **mainnet-only**: the managed enforcing node exists to defend mainnet, and
onion reachability has no value on test networks. On signet/testnet/regtest the
Settings section is hidden, no default-ON preference is written, and the runtime
never starts Tor (see `prepare_inbound_tor`).

## Architecture

| Concern | Where |
| --- | --- |
| Config emission/parse (`listen`/`torcontrol`/`proxy`/caps) | [`InternalBitcoindConfig`](../coincube-gui/src/node/bitcoind.rs) `to_ini`/`from_ini` |
| Tor binary URLs, version pin, dirs | [`node/bitcoind.rs`](../coincube-gui/src/node/bitcoind.rs) (`TOR_VERSION`, `tor_asset_url`, `internal_tor_*`) |
| Download + PGP-verified install | [`installer/step/node/bitcoind.rs`](../coincube-gui/src/installer/step/node/bitcoind.rs) (`install_tor`, `DownloadVerification::for_tor`) |
| Tor lifecycle, preference, fail-safe | [`node/tor.rs`](../coincube-gui/src/node/tor.rs) |
| Runtime wiring (start/stop) | [`loader.rs`](../coincube-gui/src/loader.rs), [`app/mod.rs`](../coincube-gui/src/app/mod.rs) |
| Settings UI | [`view/vault/settings/mod.rs`](../coincube-gui/src/app/view/vault/settings/mod.rs) `inbound_tor_section` |
| Duress wipe of identifying material | [`node/tor.rs`](../coincube-gui/src/node/tor.rs) `duress_identifying_targets` → [`gui/tab.rs`](../coincube-gui/src/gui/tab.rs) `duress_wipe_targets` |

### Binary acquisition & verification

The Tor Expert Bundle is pinned to one version (`TOR_VERSION`) and downloaded per
platform. Its `sha256sums-unsigned-build.txt` manifest is verified against the
vendored **Tor Browser Developers** signing key
([`assets/tor_signing_key.asc`](../coincube-gui/assets/tor_signing_key.asc),
fingerprint `EF6E286D…298290`) before the archive's own checksum is trusted —
the same signed-manifest path as Knots. The Tor manifest is signed by a *signing
subkey*, so `verify_detached_signature` accepts a subkey signature only when the
subkey's binding to the pinned primary validates.

### Preference vs. runtime config (the fail-safe split)

The **user preference** lives in a sidecar, `bitcoind/inbound_tor.json`
([`InboundTorPreference`](../coincube-gui/src/node/tor.rs)) — kept out of
`bitcoin.conf`, which `bitcoind` would reject unknown keys in and which is
rewritten each run to reflect the *actual* runtime state.

On every managed-node start, [`prepare_inbound_tor`](../coincube-gui/src/node/tor.rs):

1. Loads the preference.
2. If enabled + supported, self-installs the Tor binary if missing
   (`ensure_tor_installed_if_wanted`), starts Tor, waits for bootstrap.
3. On success: injects the fresh control/SOCKS ports into `bitcoin.conf` and
   starts `bitcoind` → onion service.
4. On **any** failure (binary missing, bootstrap timeout, crash, I/O): strips
   the inbound knobs from `bitcoin.conf` so `bitcoind` runs **outbound-only,
   exactly as today**. The preference sidecar is never touched, so a transient
   failure is retried on the next launch.

Inbound is an *enhancement, never a dependency*: the wallet cannot be
destabilised by Tor. The managed Tor is a process-wide singleton held in a
registry (`MANAGED_TOR`); `stop_managed_tor()` is called from every shutdown path
alongside stopping `bitcoind`.

### Duress wipe

The onion-service key is identifying material. The duress wipe otherwise
preserves the `bitcoind/` tree (the blockchain is expensive to re-sync and not
sensitive), so `duress_identifying_targets` explicitly adds the managed Tor data
directory (`bitcoind/tor-data`) and `bitcoind`'s `onion_v3_private_key` /
`onion_private_key` files to the wipe target list. The blockchain is preserved;
the onion key and Tor state are obliterated.

## QA matrix

Run on mainnet with a Knots managed node unless noted. "External Tor client"
means a separate machine/host running `tor` + a Bitcoin client, or
`torify bitcoin-cli`/an onion-capable peer.

| # | Scenario | Steps | Expected |
| --- | --- | --- | --- |
| 1 | **Enable → reachable** | Fresh Knots setup (default ON) → let Tor bootstrap and `bitcoind` start | `bitcoin-cli getnetworkinfo` shows a `.onion` in `localaddresses`; the onion is reachable from an external Tor client; `getpeerinfo` eventually shows peers with `inbound: true` |
| 2 | **Disable → listen off** | Settings → "Help defend the network" → toggle off → restart node | `bitcoin.conf` has no `listen`/`listenonion`/`torcontrol`/`proxy`; `getnetworkinfo.localaddresses` has no onion; no managed `tor` process; node runs outbound-only |
| 3 | **Tor crash → fail-safe** | With inbound on, kill the `tor` process (or point the binary at a bad path) → restart node | `bitcoind` still starts and syncs (outbound-only); logs show "inbound unavailable, running outbound-only"; the preference sidecar still says enabled (retried next launch) |
| 4 | **Bandwidth cap honoured** | Inbound on, default cap | `getnetworkinfo.uploadtarget.target_bytes` ≈ the configured MiB/day; `getnettotals` upload stays bounded. Toggle "Limit upload" off → `maxuploadtarget` omitted → `uploadtarget.target_bytes = 0` |
| 5 | **Outbound-via-Tor sub-toggle** | Toggle the sub-toggle, restart | On: `bitcoin.conf` has `proxy=127.0.0.1:<socks>`, outbound peers are `.onion`/via Tor. Off: no `proxy` line, outbound is clearnet |
| 6 | **Config round-trips** | Enable, restart twice | `bitcoin.conf` inbound keys stable across restarts; fresh Tor ports each run; `bitcoind` re-adds the *same* onion address (persistent key) |
| 7 | **Unsupported platform** | Linux/Windows aarch64 | Settings section shows "Not available on this platform"; node runs outbound-only; no download attempted |
| 8 | **Duress wipe** | Enable, let the onion key + `tor-data` exist, trigger a duress wipe | `bitcoind/tor-data` and `bitcoind/datadir[/**]/onion_v3_private_key` are gone; the blockchain (`blocks`/`chainstate`) survives |
| 9 | **Signature failure refused** | Point the Tor download at a tampered archive/manifest (or wrong key) | Install fails with a signature/checksum error; no `tor` binary is written; inbound stays unavailable (fail-safe) |
| 10 | **Signet/regtest (mainnet-only)** | Set up a managed node on a non-mainnet network | The "Help defend the network" section is **hidden**; no default-ON preference is written; `prepare_inbound_tor` no-ops to outbound-only regardless of the sidecar. Only `Network::Bitcoin` runs Tor inbound. |

### Automated coverage

- `node::bitcoind::tests::tor_inbound_emission` — config emission/round-trip.
- `installer::step::node::bitcoind::tests::{tor_detached_signature_verification, tor_asset_urls, tor_verification}` — the vendored key verifies the real Tor manifest; URL construction; verification wiring.
- `node::tor::tests::{bootstrap_line_detection, torrc_has_required_directives, preference_defaults_and_roundtrip, prepare_is_failsafe_without_tor_binary, duress_targets_tor_data_and_onion_keys_not_blockchain}`.
- `gui::tab::duress_wipe_target_tests::wipes_all_cube_material_and_preserves_connect_auth` — onion key wiped, blockchain preserved.

## Open items / follow-ups

- **Status card polish**: the Settings section shows a running/not-running status
  line; richer live stats (onion address copy button, inbound peer count from
  `getpeerinfo`, upload-used-vs-target from `getnettotals`) are post-launch polish.
- **Tor version bumps** are deliberate, like `KNOTS_VERSIONS` — bump `TOR_VERSION`
  and confirm the manifest still verifies against the vendored key.
