"""Broadcast-path regression tests under the node's relay policy.

Bitcoin Knots `20260508` tightens relay defaults (e.g. `subdustfeepenalty` on,
`datacarrier` changes) and, on the Knots CI leg, runs with `consensusrules=rdts`
(BIP-110) enforced. COINCUBE only ever emits standard P2WPKH / P2TR payments, so
those must keep relaying. These tests build, sign, and broadcast such sends
through the node via `sendrawtransaction` — a raw broadcast is rejected with an
RpcError if policy refuses it, so a clean broadcast is the assertion.

The tests use only the `bitcoind` fixture, so they run on every backend leg; the
Knots + RDTS matrix entry is where they actually exercise the stricter policy.
"""

from decimal import Decimal

from fixtures import *
from test_framework.utils import IS_NOT_BITCOIND_24, wait_for


def _broadcast_standard_send(bitcoind, addr_type):
    """Fund one coin of `addr_type`, then build, sign, and broadcast a standard
    send of it through the node via `sendrawtransaction`. Returns the broadcast
    txid; raises (RpcError) if the node's policy rejects the transaction."""
    dest = bitcoind.rpc.getnewaddress("", addr_type)
    funding_txid = bitcoind.rpc.sendtoaddress(dest, 1)
    bitcoind.generate_block(1, wait_for_mempool=funding_txid)

    # Locate the output paying our destination.
    decoded = bitcoind.rpc.gettransaction(funding_txid, True, True)["decoded"]
    vout = next(
        out["n"]
        for out in decoded["vout"]
        if out["scriptPubKey"].get("address") == dest
    )

    # Spend it to a fresh address of the same type, leaving ~0.001 BTC fee.
    change = bitcoind.rpc.getnewaddress("", addr_type)
    raw = bitcoind.rpc.createrawtransaction(
        [{"txid": funding_txid, "vout": vout}],
        {change: Decimal("0.999")},
    )
    signed = bitcoind.rpc.signrawtransactionwithwallet(raw)
    assert signed["complete"], signed

    # The assertion: no policy rejection — sendrawtransaction returns the txid.
    txid = bitcoind.rpc.sendrawtransaction(signed["hex"])
    wait_for(lambda: txid in bitcoind.rpc.getrawmempool())
    bitcoind.generate_block(1, wait_for_mempool=txid)
    return txid


def test_standard_p2wpkh_send_relays(bitcoind):
    """A standard P2WPKH (segwit v0) send broadcasts without policy rejection."""
    assert _broadcast_standard_send(bitcoind, "bech32")


def test_standard_p2tr_send_relays(bitcoind):
    """A standard P2TR (segwit v1) send broadcasts without policy rejection.

    Wallet taproot signing isn't available on the 24.0.1 minimum-version leg, so
    this is only exercised on newer nodes (Core 29.0 and Knots 29.x)."""
    if not IS_NOT_BITCOIND_24:
        return
    assert _broadcast_standard_send(bitcoind, "bech32m")
