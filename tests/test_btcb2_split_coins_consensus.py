"""Split coins (one-sided splits from Coin control) against both pinned nodes.

A split is the Vault's ordinary send-to-self of its both-chain coins, which
the twin chain rejects for one of two reasons
(`coincube-gui/src/services/split_coins.rs`; decision
`2026-10-29-coincube-desktop-btcb2-split-protection-in-coin-control`):

* **split key**: one more input that exists on this chain only, so the twin
  chain is missing an input;
* **replay-protected signature** (Bitcoin Blake2b only): every signature is
  `ALL|UNIFIED` (0x21), which Bitcoin does not verify.

Each split here is built the way Split coins builds it: `createspend` with no
destination over exactly the chosen coins, which gives one change output in
the same Vault. It is then signed with the harness keys, finalized and checked
against both nodes. This replaces the consensus coverage of the removed Claim
and foreign-Split modules for the flow that ships. It checks node consensus
only: no GUI, journal, Connect lookups or ancestry proofs. The Bitcoin-side
send warning and the hardware-signer gate on the signature method are GUI rules
with their own unit tests.

The tests share one module-scoped harness and spend its coins in order, so
they must run in file order (pytest's default).
"""
import pytest

import test_btcb2_unified_sighash as unified
from fixtures import *
from test_framework.authproxy import JSONRPCException
from test_framework.serializations import (
    PSBT,
    CTxOut,
    from_binary,
    PSBT_IN_BIP32_DERIVATION,
    PSBT_IN_PARTIAL_SIG,
    PSBT_IN_WITNESS_SCRIPT,
    PSBT_IN_WITNESS_UTXO,
)
from test_framework.signer import coincurve
from test_framework.utils import RpcError, wait_for

FEERATE_VB = 2
SPLIT_CONFIRMATIONS = 6
COINBASE_MATURITY = 100
UNIFIED_SIGHASH_ALL = 0x01 | unified.SIGHASH_UNIFIED
# What each split leaves behind for a later test in this module.
STATE = {}


def outpoint(txid, vout):
    return f"{txid}:{vout}"


def synced(daemon, node):
    tip = node.rpc.getblockcount()
    wait_for(lambda: daemon.rpc.getinfo()["block_height"] == tip)


def level(node, height):
    """Mine `node` up to `height`, so a transaction whose locktime is the
    other chain's tip is final on both."""
    missing = height - node.rpc.getblockcount()
    if missing > 0:
        node.generate_block(missing)


def build_split(daemon, coins):
    """Split coins' construction: a send-to-self of exactly `coins` into one
    change output of the Vault (`split_coins::check_construction`)."""
    res = daemon.rpc.createspend({}, coins, FEERATE_VB)
    assert "psbt" in res, res
    psbt = PSBT.from_base64(res["psbt"])
    spent = sorted(outpoint(f"{i.prevout.hash:064x}", i.prevout.n) for i in psbt.tx.vin)
    assert spent == sorted(coins), (spent, coins)
    assert len(psbt.tx.vout) == 1, psbt.tx.vout
    return psbt


def finalize_legacy(harness, daemon, psbt):
    signed = harness.signer.sign_psbt(psbt, [0, 1])
    final = daemon.finalize_psbt(signed)
    return final.tx.serialize_with_witness().hex(), final.tx.txid().hex()


def finalize_unified(harness, daemon, psbt):
    """Sign every input `ALL|UNIFIED` (0x21) over Knots' unified digest with
    two primary keys, the replay-protected signature a Blake2b split uses."""
    spent = [from_binary(CTxOut, i.map[PSBT_IN_WITNESS_UTXO]) for i in psbt.i]
    for index, psbt_in in enumerate(psbt.i):
        fing_der = next(iter(psbt_in.map[PSBT_IN_BIP32_DERIVATION].values()))
        raw = fing_der[4:]
        path = [int.from_bytes(raw[i:i + 4], "little", signed=True)
                for i in range(0, len(raw), 4)]
        code = psbt_in.map[PSBT_IN_WITNESS_SCRIPT]
        digest = unified.unified_sighash(
            psbt.tx, index, UNIFIED_SIGHASH_ALL, unified.SCRIPT_TYPE_WITNESS_V0, spent, code
        )
        for hd in harness.signer.prim_hds[:2]:
            privkey = coincurve.PrivateKey(hd.get_privkey_from_path(path))
            pubkey = privkey.public_key.format()
            assert pubkey in psbt_in.map[PSBT_IN_BIP32_DERIVATION], pubkey.hex()
            signature = privkey.sign(digest, hasher=None) + bytes([UNIFIED_SIGHASH_ALL])
            psbt_in.map.setdefault(PSBT_IN_PARTIAL_SIG, {})[pubkey] = signature
    final = daemon.finalize_psbt(psbt)
    return final.tx.serialize_with_witness().hex(), final.tx.txid().hex()


def refused_missing_inputs(node, raw):
    verdict = node.rpc.testmempoolaccept([raw])[0]
    assert not verdict["allowed"], verdict
    assert verdict["reject-reason"] == "missing-inputs", verdict
    tip = node.rpc.getbestblockhash()
    generate_block = node.rpc.generateblock
    block_address = node.rpc.getnewaddress()
    with pytest.raises(JSONRPCException) as rejected:
        generate_block(block_address, [raw])
    assert rejected.value.error["code"] == -25, rejected.value.error
    assert "bad-txns-inputs-missingorspent" in rejected.value.error["message"], (
        rejected.value.error
    )
    assert node.rpc.getbestblockhash() == tip


def confirm(node, daemon, raw, txid, blocks=SPLIT_CONFIRMATIONS):
    assert node.rpc.sendrawtransaction(raw) == txid
    height = node.rpc.getblockcount() + 1
    node.generate_block(blocks, wait_for_mempool=txid)
    synced(daemon, node)
    return node.rpc.getblockhash(height)


def test_split_key_split_is_one_sided_on_bitcoin(two_chain, record_property):
    """A Bitcoin split of a both-chain coin with a split key (a Vault coin
    that exists on Bitcoin only) is valid on Bitcoin and missing an input on
    Blake2b, before and after a reorg. Once deep, the coin's Bitcoin copy is
    spent and its Blake2b copy is not: a Blake2b spend of it can no longer be
    replayed onto Bitcoin."""
    a, b = two_chain.legacy, two_chain.blake2b
    daemon_a, daemon_b = two_chain.coincubed_legacy, two_chain.coincubed_blake2b
    synced(daemon_a, a)
    shared = two_chain.prefork_outpoints[0]
    key = two_chain.poison_outpoint
    for txid, vout, _ in (shared,):
        assert a.rpc.gettxout(txid, vout) is not None
        assert b.rpc.gettxout(txid, vout) is not None
    assert b.rpc.gettxout(key[0], key[1]) is None

    psbt = build_split(daemon_a, [outpoint(*shared[:2]), outpoint(*key[:2])])
    raw, txid = finalize_legacy(two_chain, daemon_a, psbt)
    level(a, b.rpc.getblockcount())
    level(b, a.rpc.getblockcount())
    verdict = a.rpc.testmempoolaccept([raw])[0]
    assert verdict["allowed"], verdict
    record_property("split_key_vsize", verdict["vsize"])
    refused_missing_inputs(b, raw)

    block = confirm(a, daemon_a, raw, txid)
    assert a.rpc.gettxout(shared[0], shared[1]) is None
    assert b.rpc.gettxout(shared[0], shared[1]) is not None
    assert a.rpc.gettxout(txid, 0) is not None
    assert b.rpc.gettxout(txid, 0) is None

    # A reorg on Bitcoin does not make the split valid on Blake2b.
    a.rpc.invalidateblock(block)
    assert b.rpc.testmempoolaccept([raw])[0]["reject-reason"] == "missing-inputs"
    a.rpc.reconsiderblock(block)
    assert a.rpc.getrawtransaction(txid, True, block)["confirmations"] >= SPLIT_CONFIRMATIONS
    synced(daemon_a, a)

    # The coin's Blake2b copy is now Blake2b's alone. Checked, never sent.
    synced(daemon_b, b)
    twin = build_split(daemon_b, [outpoint(*shared[:2])])
    twin_raw, _ = finalize_legacy(two_chain, daemon_b, twin)
    level(a, b.rpc.getblockcount())
    assert b.rpc.testmempoolaccept([twin_raw])[0]["allowed"]
    refused_missing_inputs(a, twin_raw)
    STATE["split_output"] = (txid, 0)


def test_a_split_output_is_reused_as_a_split_key(two_chain):
    """The output of a split-key split exists on Bitcoin only, so it can key
    the next split, as Split coins offers it once it is verified."""
    assert "split_output" in STATE, "needs the split from the previous test"
    a, b = two_chain.legacy, two_chain.blake2b
    daemon_a = two_chain.coincubed_legacy
    shared = two_chain.prefork_outpoints[1]
    key = STATE["split_output"]
    psbt = build_split(daemon_a, [outpoint(*shared[:2]), outpoint(*key)])
    raw, txid = finalize_legacy(two_chain, daemon_a, psbt)
    level(b, a.rpc.getblockcount())
    assert a.rpc.testmempoolaccept([raw])[0]["allowed"]
    refused_missing_inputs(b, raw)
    confirm(a, daemon_a, raw, txid)
    assert a.rpc.gettxout(shared[0], shared[1]) is None
    assert b.rpc.gettxout(shared[0], shared[1]) is not None


def test_a_coinbase_key_must_mature(two_chain):
    """A key acquired as a post-fork Bitcoin coinbase to the Vault is offered
    only once mature: the daemon refuses to spend it before, and after it
    keys a split Blake2b refuses."""
    a, b = two_chain.legacy, two_chain.blake2b
    daemon_a = two_chain.coincubed_legacy
    address = daemon_a.rpc.getnewaddress()["address"]
    block = a.rpc.generatetoaddress(1, address)[0]
    coinbase = a.rpc.getblock(block)["tx"][0]
    synced(daemon_a, a)
    coin = outpoint(coinbase, 0)

    def listed():
        coins = daemon_a.rpc.listcoins([], [coin])["coins"]
        assert len(coins) == 1, coins
        return coins[0]

    wait_for(lambda: daemon_a.rpc.listcoins([], [coin])["coins"])
    assert listed()["is_immature"], listed()
    with pytest.raises(RpcError, match="immature coinbase"):
        daemon_a.rpc.createspend({}, [coin], FEERATE_VB)

    a.generate_block(COINBASE_MATURITY)
    synced(daemon_a, a)
    assert not listed()["is_immature"], listed()
    psbt = build_split(daemon_a, [coin])
    raw, txid = finalize_legacy(two_chain, daemon_a, psbt)
    level(b, a.rpc.getblockcount())
    assert a.rpc.testmempoolaccept([raw])[0]["allowed"]
    refused_missing_inputs(b, raw)


def test_signature_split_is_one_sided_on_blake2b(two_chain, record_property):
    """A Blake2b split signed `ALL|UNIFIED` on every input is valid on
    Blake2b and refused by Bitcoin for the signature alone, in its mempool
    and in a block; the same transaction signed legacy is a valid Bitcoin
    transaction (checked, never sent). Once deep, the coin's Bitcoin copy is
    still unspent on Bitcoin."""
    a, b = two_chain.legacy, two_chain.blake2b
    daemon_b = two_chain.coincubed_blake2b
    shared = two_chain.prefork_outpoints[2]
    assert a.rpc.gettxout(shared[0], shared[1]) is not None
    assert b.rpc.gettxout(shared[0], shared[1]) is not None
    level(b, a.rpc.getblockcount())
    synced(daemon_b, b)

    psbt = build_split(daemon_b, [outpoint(*shared[:2])])
    legacy_twin, _ = finalize_legacy(two_chain, daemon_b, PSBT.from_base64(psbt.to_base64()))
    raw, txid = finalize_unified(two_chain, daemon_b, psbt)
    verdict = b.rpc.testmempoolaccept([raw])[0]
    assert verdict["allowed"], verdict
    record_property("signature_split_vsize", verdict["vsize"])

    level(a, b.rpc.getblockcount())
    refused = a.rpc.testmempoolaccept([raw])[0]
    assert not refused["allowed"], refused
    assert refused["reject-reason"].startswith("mempool-script-verify-flag-failed"), refused
    record_property("bitcoin_unified_mempool_reject", refused["reject-reason"])
    tip = a.rpc.getbestblockhash()
    generate_block = a.rpc.generateblock
    block_address = a.rpc.getnewaddress()
    with pytest.raises(JSONRPCException) as rejected:
        generate_block(block_address, [raw])
    assert rejected.value.error["code"] == -25, rejected.value.error
    assert "mandatory-script-verify-flag-failed" in rejected.value.error["message"], (
        rejected.value.error
    )
    assert a.rpc.getbestblockhash() == tip
    # The signatures are all that keep the chains apart.
    assert a.rpc.testmempoolaccept([legacy_twin])[0]["allowed"]

    confirm(b, daemon_b, raw, txid)
    assert b.rpc.gettxout(shared[0], shared[1]) is None
    assert a.rpc.gettxout(shared[0], shared[1]) is not None
    assert a.rpc.gettxout(txid, 0) is None
