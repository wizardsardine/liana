"""Bitcoin Blake2b (BTCB2) two-chain regtest harness — launch-ga Lane B4.1.

These tests prove the harness itself: two Knots builds sharing one pre-fork
history and diverging at the scheduled height, a BLAKE2b-aware Esplora indexer
on each chain, the Coincube daemon syncing each through its Esplora backend, the
Vault fixtures the B4.2 matrices consume, and that every datadir is pinned to
the test directory. They do not exercise any BTCB2 feature code — that is B4.2,
which sits on top of this fixture once Lanes B1/B2/B3 land.

Skipped unless the three binaries are configured (see tests/README.md).
"""

import os
import subprocess

import pytest

from fixtures import *
from test_framework.btcb2 import (
    ELECTRS_BLAKE2B_COMMIT,
    KNOTS_CAT21_LOCKTIME,
    KNOTS_BLAKE2B_VERSION,
    KNOTS_LEGACY_VERSION,
    MIN_ACTIVATION_HEIGHT,
    PREFORK_FUNDING_START_HEIGHT,
    RDTS_EXPIRY_FAR_FUTURE,
    TwoChainRegtest,
    WALLET_ANTI_FEE_SNIPING_MAX_DEPTH,
)
from test_framework.authproxy import JSONRPCException
from test_framework.utils import wait_for


def _subversion(node):
    return node.rpc.getnetworkinfo()["subversion"]


def test_node_builds_are_the_pinned_ones(two_chain):
    """Node A runs the pinned non-enforcing Knots, node B the fork build, and the
    indexer reports the pinned retropex/electrs commit."""
    # Secondary check behind the fetcher's clean-checkout gate (upstream build.rs
    # stamps the commit, and "(dirty)" for an unclean tree, when GIT_HASH is not
    # preset); outside the labelled workflow ELECTRS_BLAKE2B_PATH may point at
    # any binary, so the harness asserts what it is actually talking to.
    version = subprocess.run(
        [two_chain.electrs_legacy.electrs_path, "--version"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,  # electrs prints its version on stderr
        text=True,
        timeout=30,
    ).stdout.splitlines()[0]
    assert ELECTRS_BLAKE2B_COMMIT[:7] in version, version
    assert "dirty" not in version, version
    # Knots reports itself as `/Satoshi:<core version>/Knots:<build date>/`.
    assert (
        _subversion(two_chain.legacy) == "/Satoshi:29.3.0/Knots:20260507/"
    ), KNOTS_LEGACY_VERSION
    assert (
        _subversion(two_chain.blake2b) == "/Satoshi:29.5.1/Knots:20260508/"
    ), KNOTS_BLAKE2B_VERSION
    assert two_chain.legacy.rpc.getblockchaininfo()["chain"] == "regtest"
    assert two_chain.blake2b.rpc.getblockchaininfo()["chain"] == "regtest"


def test_prefork_funding_start_is_clear_of_cat21_locktime():
    """Every possible anti-fee-sniping locktime stays above Knots' CAT-21 value."""
    assert (
        PREFORK_FUNDING_START_HEIGHT - WALLET_ANTI_FEE_SNIPING_MAX_DEPTH
        > KNOTS_CAT21_LOCKTIME
    )
    with pytest.raises(ValueError, match=f"at least {MIN_ACTIVATION_HEIGHT}"):
        TwoChainRegtest("/unused", activation_height=MIN_ACTIVATION_HEIGHT - 1)


def test_prefork_wallet_funding_cannot_choose_cat21_locktime(two_chain):
    """The live fixture's randomized funding locktimes stay above CAT-21."""
    assert two_chain.activation_height > PREFORK_FUNDING_START_HEIGHT + len(
        two_chain.prefork_txids
    )
    for txid in two_chain.prefork_txids:
        decoded = two_chain.legacy.rpc.gettransaction(txid, True, True)["decoded"]
        assert decoded["locktime"] > KNOTS_CAT21_LOCKTIME


def test_chains_share_history_then_diverge_at_activation(two_chain):
    """Same block at N-1 on both nodes; different blocks from N on."""
    n = two_chain.activation_height
    a, b = two_chain.legacy, two_chain.blake2b
    assert (
        a.rpc.getblockhash(n - 1)
        == b.rpc.getblockhash(n - 1)
        == two_chain.fork_parent_hash
    )
    assert a.rpc.getblockhash(n) != b.rpc.getblockhash(n)
    assert a.rpc.getblockcount() > n
    assert b.rpc.getblockcount() > n

    # Node B: the hardfork and RDTS are scheduled at N and active past it.
    info_b = b.rpc.getdeploymentinfo()
    assert info_b["blake2b"] == {"height": n, "active": True}
    rdts = info_b["deployments"]["reduced_data"]
    assert rdts['type'] == 'flagday'
    assert rdts['height'] == n
    assert rdts['expiry_time'] == RDTS_EXPIRY_FAR_FUTURE
    assert rdts['active'] is True
    # `active` describes the block *after* the queried one: false at N-2, true
    # at N-1 (whose successor is the fork block).
    info_before = b.rpc.getdeploymentinfo(b.rpc.getblockhash(n - 2))
    assert info_before["blake2b"]["active"] is False
    assert info_before["deployments"]["reduced_data"]["active"] is False
    info_parent = b.rpc.getdeploymentinfo(two_chain.fork_parent_hash)
    assert info_parent["blake2b"]["active"] is True
    assert info_parent["deployments"]["reduced_data"]["active"] is True

    # Node A knows nothing of either.
    info_a = a.rpc.getdeploymentinfo()
    assert "blake2b" not in info_a
    assert "reduced_data" not in info_a["deployments"]

    # Header shape: 80-byte v1 headers before N on both, 164-byte v2 on B from N.
    assert len(a.rpc.getblockheader(a.rpc.getblockhash(n - 1), False)) == 80 * 2
    assert len(b.rpc.getblockheader(b.rpc.getblockhash(n - 1), False)) == 80 * 2
    assert len(b.rpc.getblockheader(b.rpc.getblockhash(n), False)) == 164 * 2
    assert b.rpc.getblockheader(b.rpc.getblockhash(n))["header_version"] == 2
    assert len(a.rpc.getblockheader(a.rpc.getblockhash(n), False)) == 80 * 2


def test_nodes_refuse_each_others_post_fork_blocks(two_chain):
    """A post-fork block from either chain is not accepted by the other node."""
    n = two_chain.activation_height
    a, b = two_chain.legacy, two_chain.blake2b
    tip_a, tip_b = a.rpc.getbestblockhash(), b.rpc.getbestblockhash()

    # B's first BLAKE2b block into A (which cannot even parse a v2 header).
    block_b = b.rpc.getblock(b.rpc.getblockhash(n), 0)
    try:
        result = a.rpc.submitblock(block_b)
    except JSONRPCException as e:
        result = str(e)
    assert result is not None, "node A accepted a BLAKE2b block"
    assert a.rpc.getbestblockhash() == tip_a

    # A's SHA256d block at N into B (which requires BLAKE2b from N on).
    block_a = a.rpc.getblock(a.rpc.getblockhash(n), 0)
    try:
        result = b.rpc.submitblock(block_a)
    except JSONRPCException as e:
        result = str(e)
    assert result is not None, "node B accepted a SHA256d block at the fork height"
    assert b.rpc.getbestblockhash() == tip_b


def test_vault_fixtures_exist_where_expected(two_chain):
    """Pre-fork Vault coins on both chains; the poison coin on the Bitcoin side only."""
    a, b = two_chain.legacy, two_chain.blake2b
    n = two_chain.activation_height
    assert len(two_chain.prefork_outpoints) == 3
    for txid, vout, amount in two_chain.prefork_outpoints:
        block_hash = two_chain.prefork_block_hashes[txid]
        for node in (a, b):
            # No -txindex: look the tx up by the block both chains share.
            tx = node.rpc.getrawtransaction(txid, True, block_hash)
            assert tx["txid"] == txid
            assert node.rpc.getblockheader(block_hash)["height"] < n
            utxo = node.rpc.gettxout(txid, vout)
            assert utxo is not None
            assert utxo['value'] == amount

    txid, vout, amount = two_chain.poison_outpoint
    utxo_a = a.rpc.gettxout(txid, vout)
    assert utxo_a is not None
    assert utxo_a['value'] == amount
    assert a.rpc.getblockheader(two_chain.poison_block_hash)["height"] > n
    assert (
        a.rpc.getrawtransaction(txid, True, two_chain.poison_block_hash)["txid"] == txid
    )
    # Its only input is node A's block-N coinbase, which node B does not have.
    assert b.rpc.gettxout(txid, vout) is None
    with pytest.raises(JSONRPCException):
        b.rpc.getblock(two_chain.poison_block_hash)
    verdict = b.rpc.testmempoolaccept([two_chain.poison_raw_hex])[0]
    assert verdict["allowed"] is False
    assert verdict["reject-reason"] == "missing-inputs"
    assert (
        two_chain.poison_coinbase_txid
        not in b.rpc.getblock(b.rpc.getblockhash(n))["tx"]
    )


def test_esplora_indexers_follow_their_own_chain(two_chain):
    """retropex/electrs indexes the v1 chain and the v2 (BLAKE2b) chain alike."""
    a, b = two_chain.legacy, two_chain.blake2b
    ea, eb = two_chain.electrs_legacy, two_chain.electrs_blake2b
    n = two_chain.activation_height
    assert ea.rest("/blocks/tip/hash") == a.rpc.getbestblockhash()
    assert eb.rest("/blocks/tip/hash") == b.rpc.getbestblockhash()
    assert ea.rest("/blocks/tip/height") == a.rpc.getblockcount()
    assert eb.rest("/blocks/tip/height") == b.rpc.getblockcount()

    # Raw header route: 80 bytes on the legacy chain, 164 on the BLAKE2b chain
    # (this is the one path coincubed's `tip_time()` cannot parse — audit F5).
    assert len(ea.rest(f"/block/{a.rpc.getblockhash(n)}/header")) == 80 * 2
    assert len(eb.rest(f"/block/{b.rpc.getblockhash(n)}/header")) == 164 * 2
    block_b = eb.rest(f"/block/{b.rpc.getblockhash(n)}")
    assert block_b["header_v2"]["height"] == n

    # Pre-fork transactions are on both indexers; the poison only on the legacy one.
    for txid, _, _ in two_chain.prefork_outpoints:
        assert ea.rest(f"/tx/{txid}/status")["confirmed"] is True
        assert eb.rest(f"/tx/{txid}/status")["confirmed"] is True
    poison_txid = two_chain.poison_outpoint[0]
    assert ea.rest(f"/tx/{poison_txid}/status")["confirmed"] is True
    assert eb.rest_status(f"/tx/{poison_txid}") == 404

    # The indexers keep following: mine one more block on each side.
    a.generate_block(1)
    b.generate_block(1)
    ea.wait_for_tip(a.rpc.getbestblockhash())
    eb.wait_for_tip(b.rpc.getbestblockhash())


def test_esplora_serves_the_genesis_summary_the_rescan_bound_reads(two_chain):
    """`GET /blocks/0` answers in the shape the rescan lower bound is read from.

    `Client::genesis_block_timestamp` (`coincubed/src/bitcoin/esplora/client.rs`)
    accepts *exactly one* height-zero summary carrying a `u32` timestamp, and
    refuses anything else — an empty list, a wrong height, an ambiguous window,
    or an out-of-range timestamp. The refusal is fail-safe but silent: the
    `BitcoinInterface` forwarding maps it to `0`
    (`coincubed/src/bitcoin/mod.rs`), which only turns `start_rescan`'s
    `timestamp < genesis_timestamp` guard into a no-op. So a shape mismatch on
    this route would not announce itself anywhere downstream — it has to be
    pinned here, against the indexer build the fork actually runs on.

    The unit tests cover what the reader does with each shape; what they cannot
    cover is which shape `retropex/electrs` emits. That is this test.
    """
    for node, esplora in (
        (two_chain.legacy, two_chain.electrs_legacy),
        (two_chain.blake2b, two_chain.electrs_blake2b),
    ):
        genesis_hash = node.rpc.getblockhash(0)
        summaries = esplora.rest("/blocks/0")
        # Exactly one. Esplora's `/blocks/:start_height` counts *down* from
        # `start_height`, so height 0 bottoms out at a single entry; an indexer
        # that counted up would answer with a window and the reader would
        # refuse it as ambiguous.
        assert isinstance(summaries, list), summaries
        assert len(summaries) == 1, summaries
        genesis = summaries[0]
        assert genesis["height"] == 0, genesis
        assert genesis["id"] == genesis_hash, (genesis, genesis_hash)
        # Cross-checked against the node rather than a regtest constant: this
        # is the value that becomes the rescan lower bound.
        expected_time = node.rpc.getblock(genesis_hash)["time"]
        assert genesis["timestamp"] == expected_time, (genesis, expected_time)
        assert 0 <= genesis["timestamp"] <= 0xFFFFFFFF, genesis

    # Both chains share their pre-fork history, so the bound is the same value
    # on either side of the fork. The JSON route stays usable exactly where the
    # raw header route diverges (80 vs 164 bytes at the activation height,
    # asserted above) — which is why the reader no longer decodes headers.
    legacy_genesis = two_chain.electrs_legacy.rest("/blocks/0")[0]
    blake2b_genesis = two_chain.electrs_blake2b.rest("/blocks/0")[0]
    assert legacy_genesis["id"] == blake2b_genesis["id"]
    assert legacy_genesis["timestamp"] == blake2b_genesis["timestamp"]


def test_daemon_syncs_each_chain_through_esplora(two_chain):
    """coincubed on the Esplora backend sees the right coins on each chain."""
    a, b = two_chain.legacy, two_chain.blake2b
    da, db = two_chain.coincubed_legacy, two_chain.coincubed_blake2b
    wait_for(lambda: da.rpc.getinfo()["block_height"] == a.rpc.getblockcount())
    wait_for(lambda: db.rpc.getinfo()["block_height"] == b.rpc.getblockcount())

    def confirmed_outpoints(daemon):
        return {
            (c["outpoint"].split(":")[0], int(c["outpoint"].split(":")[1]))
            for c in daemon.rpc.listcoins()["coins"]
            if c["block_height"] is not None
        }

    prefork = {(txid, vout) for txid, vout, _ in two_chain.prefork_outpoints}
    poison = (two_chain.poison_outpoint[0], two_chain.poison_outpoint[1])
    wait_for(lambda: prefork | {poison} <= confirmed_outpoints(da))
    wait_for(lambda: prefork <= confirmed_outpoints(db))
    assert poison not in confirmed_outpoints(db)

    # Same descriptor, same addresses, two independent daemons and datadirs.
    assert da.rpc.getinfo()["descriptors"] == db.rpc.getinfo()["descriptors"]
    assert da.datadir != db.datadir


def test_every_datadir_is_pinned_under_the_test_directory(two_chain):
    """No process uses a default/user directory; nothing lands in the HOME sandbox."""
    root = os.path.realpath(two_chain.directory)
    for name, path in two_chain.datadir_claims().items():
        assert os.path.realpath(path).startswith(root + os.sep), (name, path)
        assert os.path.isdir(path), (name, path)
    # The command lines carry the pins explicitly.
    for node in (two_chain.legacy, two_chain.blake2b):
        assert f"-datadir={node.bitcoin_dir}" in node.cmd_line
    for electrs in (two_chain.electrs_legacy, two_chain.electrs_blake2b):
        assert electrs.db_dir in electrs.cmd_line
    for daemon in (two_chain.coincubed_legacy, two_chain.coincubed_blake2b):
        with open(daemon.conf_file) as f:
            conf = f.read()
        assert f"data_directory = '{os.path.join(daemon.datadir, 'regtest')}'" in conf
        assert daemon.env["HOME"] == two_chain.home_dir
    for node in (two_chain.legacy, two_chain.blake2b):
        assert node.env["HOME"] == two_chain.home_dir
    two_chain.assert_home_sandbox_untouched()
