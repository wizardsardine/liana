## Coincubed blackbox tests

Here we test `coincubed` by starting it on a regression testing Bitcoin network,
and by then talking to it as an user would, from the outside.

Python scripts are used for the automation, and specifically the [`pytest` framework](https://docs.pytest.org/en/stable/index.html).

Credits: this test framework was taken and adapted from revaultd, which was itself adapted from
[C-lightning's test framework](https://github.com/ElementsProject/lightning/tree/master/contrib/pyln-testing).

### Building the project for testing

To run the tests, we must build `coincubed`.

```
$ cargo build --release
```

The `coincubed` and `coincube-cli` binaries will be in the `target/release` directory at the root of the repository.

### Test dependencies

Before running the tests, you might need to install some system packages. Here's an example for Ubuntu:

```
sudo apt update
sudo apt install build-essential libssl-dev libffi-dev python3-dev
sudo apt install autoconf automake libtool
```

Functional tests dependencies can be installed using `pip`. Use a virtual environment:

```
# Create a new virtual environment, preferably.
python3 -m venv venv
. venv/bin/activate
# Get the deps
pip install -r tests/requirements.txt
```

Additionally you need to have `bitcoind` installed on your computer, please
refer to [bitcoincore](https://bitcoincore.org/en/download/) for installation. You may use a
specific `bitcoind` binary by specifying the `BITCOIND_PATH` env var.

### Running the tests

From the root of the repository:

```
pytest tests/
```

For running the tests under Taproot a `bitcoind` version 26.0 or superior must be used. It can be
pointed to using the `BITCOIND_PATH` variable. For now, one must also compile the `taproot_signer`
Rust program:

```
(cd tests/tools/taproot_signer && cargo build --release)
```

Then the test suite can be run by using Taproot descriptors instead of P2WSH descriptors by setting
the `USE_TAPROOT` environment variable to `1`.

### BTCB2 two-chain harness

`tests/test_btcb2_harness.py` brings up the Bitcoin Blake2b (BTCB2) regtest
harness from launch-ga Lane B4.1: Bitcoin Knots `29.3.knots20260507` (the pinned
non-enforcing build) and `29.5.1.knots20260508` with the BLAKE2b hardfork
scheduled at a chosen height, a BLAKE2b-aware Esplora indexer
(`retropex/electrs`, branch `mempool`, commit `4453cac`) on each chain, and
`coincubed` on the Esplora backend against each. Both nodes start from one
pre-fork history (node B is started on a copy of node A's datadir at
`activation_height - 1`) and are never peered, so they diverge at the height.
The fixture funds a 2-of-3 Vault with a recovery path before the fork and creates
one post-fork Bitcoin-only coin (spent from a post-fork coinbase), a coin that
exists on one chain only (what Split coins calls a split key). See `tests/test_framework/btcb2.py`.

All ordinary regtest nodes pass `-listenonion=0`: the suite does not test Tor,
and Knots' default automatic Tor launcher can enter the external descriptor
double-close tracked by #394 when `tor` is absent. The bounded block-file probe
explicitly re-enables that vendor failure path with a missing test command so it
can still validate a future upstream repair.

The harness is skipped unless all three binaries are configured:

```
(cd tests/tools/knots_verify && cargo build --release)   # or set CARGO_TARGET_DIR to where it was built
export KNOTS_LEGACY_PATH=$(tests/tools/fetch_knots.sh 29.3.knots20260507)
export KNOTS_BLAKE2B_PATH=$(tests/tools/fetch_knots.sh 29.5.1.knots20260508)
export ELECTRS_BLAKE2B_PATH=$(tests/tools/fetch_electrs_blake2b.sh)
pytest tests/test_btcb2_harness.py -vvv
```

`tests/test_btcb2_unified_sighash.py` needs no node: it pins a Python port of
Bitcoin Knots' unified sighash (`ALL|UNIFIED`, script types 0 and 1) to the
upstream vectors in `coincube-core/tests/data/unified_sighash.json`,
independently of the Rust port in `coincube-core/src/unified_sighash.rs`.

The Claim and foreign-Split consensus modules (`test_btcb2_claim_consensus.py`,
`test_btcb2_claim_ancestry_consensus.py`, `test_btcb2_claim_pair_consensus.py`,
`test_btcb2_split_consensus.py`) and their Rust bridges
(`claim_regtest_vectors`, `split_regtest_vectors`) were removed with those
flows in S7b; Split coins replaces them (company-brain decision
`2026-10-29-coincube-desktop-btcb2-split-protection-in-coin-control.md`).
`tests/test_btcb2_split_coins_consensus.py` checks Split coins' splits against
both nodes: a split-key split on Bitcoin (including after a reorg), reusing a
split output as a key, a coinbase key's maturity, and a Blake2b split signed
`ALL|UNIFIED`. Its tests share one harness and run in file order.

`fetch_knots.sh` downloads a release for the host platform and refuses to extract
it unless its checksum is listed in the release `SHA256SUMS` *and*
`SHA256SUMS.asc` verifies against the Knots signing key vendored in
`coincube-gui/assets/knots_signing_key.asc` — the same check the desktop
installer performs (`tests/tools/knots_verify`, no `gpg` needed). The check runs
on every invocation, cached or not, and the extracted `bitcoind` is reused only
if it matches the archive member byte for byte; there is no stored "verified"
marker to trust. No dedicated variable names the verifier outside the script's
own test mode (`KNOTS_TEST_MODE=1`, used only by `tests/test_btcb2_tools.py`);
in real runs it is located through `CARGO_TARGET_DIR` (then the repo-local
build), and the script prints the verifier it resolved on stderr every time so
a stale or foreign build is visible in the log.
`fetch_electrs_blake2b.sh` clones and builds the pinned indexer commit (RocksDB
compiles from source: a few minutes the first time; `clang`/`cmake` required)
under `~/.cache/coincube/electrs-blake2b` — outside the repository, or Cargo
would treat the checkout as part of Coincube's workspace. It refuses to build
or reuse anything if that checkout has local changes (edited, staged or
untracked files): `--locked` pins dependencies, not source. Your edits are left
in place; restore the tree or use another cache dir. `tests/test_btcb2_tools.py`
covers these checks without a build. Knots downloads cache
under `tests/tools/knots/` (gitignored). In CI `BTCB2_HARNESS_REQUIRED=1` turns
a missing binary into a failure rather than a skip.

Every node, indexer and daemon datadir is created under the test directory, and
every child process runs with `HOME`/`XDG_*` pointing at an empty sandbox inside
it; `test_every_datadir_is_pinned_under_the_test_directory` fails if anything
lands there. In CI the harness runs from `.github/workflows/btcb2-regtest.yml`
only on pull requests labelled `btcb2` (or on manual dispatch).

### Tips and tricks

#### Logging

We use the [Live Logging](https://docs.pytest.org/en/latest/logging.html#live-logs)
functionality from pytest. It is configured in (`pyproject.toml`)[../pyproject.toml] to
output `INFO`-level to the console. If a test fails, the entire `DEBUG` log is output.

You can override the config at runtime with the `--log-cli-level` option:

```
pytest -vvv --log-cli-level=DEBUG -k test_startup
```

Note that we record all logs from daemons, and we start them with `log_level = "debug"`.

#### Running tests in parallel

In order to run tests in parallel, you can use `-n` arg:

```
pytest -n 8 tests/
```

### Test lints

Just use [`black`](https://github.com/psf/black).

### More

See the environment variables in `test_framework/utils.py`.
