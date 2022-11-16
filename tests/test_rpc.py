import os
import pytest
import random
import shutil
import time

from fixtures import *
from test_framework.serializations import PSBT, PSBT_IN_PARTIAL_SIG
from test_framework.utils import wait_for, COIN, RpcError, get_txid, spend_coins


def test_getinfo(minisafed):
    res = minisafed.rpc.getinfo()
    assert res["version"] == "0.1"
    assert res["network"] == "regtest"
    wait_for(lambda: minisafed.rpc.getinfo()["blockheight"] == 101)
    res = minisafed.rpc.getinfo()
    assert res["sync"] == 1.0
    assert "main" in res["descriptors"]
    assert res["rescan_progress"] is None


def test_getaddress(minisafed):
    res = minisafed.rpc.getnewaddress()
    assert "address" in res
    # We'll get a new one at every call
    assert res["address"] != minisafed.rpc.getnewaddress()["address"]


def test_listcoins(minisafed, bitcoind):
    # Initially empty
    res = minisafed.rpc.listcoins()
    assert "coins" in res
    assert len(res["coins"]) == 0

    # If we send a coin, we'll get a new entry. Note we monitor for unconfirmed
    # funds as well.
    addr = minisafed.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 1)
    wait_for(lambda: len(minisafed.rpc.listcoins()["coins"]) == 1)
    res = minisafed.rpc.listcoins()["coins"]
    assert txid == res[0]["outpoint"][:64]
    assert res[0]["amount"] == 1 * COIN
    assert res[0]["block_height"] is None
    assert res[0]["spend_info"] is None

    # If the coin gets confirmed, it'll be marked as such.
    bitcoind.generate_block(1, wait_for_mempool=txid)
    block_height = bitcoind.rpc.getblockcount()
    wait_for(
        lambda: minisafed.rpc.listcoins()["coins"][0]["block_height"] == block_height
    )

    # Same if the coin gets spent.
    spend_tx = spend_coins(minisafed, bitcoind, (res[0],))
    spend_txid = get_txid(spend_tx)
    wait_for(lambda: minisafed.rpc.listcoins()["coins"][0]["spend_info"] is not None)
    spend_info = minisafed.rpc.listcoins()["coins"][0]["spend_info"]
    assert spend_info["txid"] == spend_txid
    assert spend_info["height"] is None

    # And if this spending tx gets confirmed.
    bitcoind.generate_block(1, wait_for_mempool=spend_txid)
    curr_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: minisafed.rpc.getinfo()["blockheight"] == curr_height)
    spend_info = minisafed.rpc.listcoins()["coins"][0]["spend_info"]
    assert spend_info["txid"] == spend_txid
    assert spend_info["height"] == curr_height


def test_jsonrpc_server(minisafed, bitcoind):
    """Test passing parameters as a list or a mapping."""
    addr = minisafed.rpc.getnewaddress()["address"]
    bitcoind.rpc.sendtoaddress(addr, 1)
    wait_for(lambda: len(minisafed.rpc.listcoins()["coins"]) == 1)
    outpoints = [minisafed.rpc.listcoins()["coins"][0]["outpoint"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 20_000,
    }
    res = minisafed.rpc.createspend(outpoints, destinations, 18)
    assert "psbt" in res
    res = minisafed.rpc.createspend(
        outpoints=outpoints, destinations=destinations, feerate=18
    )
    assert "psbt" in res


def test_create_spend(minisafed, bitcoind):
    # Receive a number of coins in different blocks on different addresses, and
    # one more on the same address.
    for _ in range(15):
        addr = minisafed.rpc.getnewaddress()["address"]
        txid = bitcoind.rpc.sendtoaddress(addr, 0.01)
        bitcoind.generate_block(1, wait_for_mempool=txid)
    txid = bitcoind.rpc.sendtoaddress(addr, 0.3556)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(minisafed.rpc.listcoins()["coins"]) == 16)

    # Stop the daemon, should be a no-op
    minisafed.stop()
    minisafed.start()

    # Now create a transaction spending all those coins to a few addresses
    outpoints = [c["outpoint"] for c in minisafed.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 200_000,
        bitcoind.rpc.getnewaddress(): 400_000,
        bitcoind.rpc.getnewaddress(): 1_000_000,
    }
    res = minisafed.rpc.createspend(outpoints, destinations, 18)
    assert "psbt" in res

    # The transaction must contain a change output.
    spend_psbt = PSBT.from_base64(res["psbt"])
    assert len(spend_psbt.o) == 4
    assert len(spend_psbt.tx.vout) == 4

    # We can sign it and broadcast it.
    signed_psbt = minisafed.sign_psbt(PSBT.from_base64(res["psbt"]))
    finalized_psbt = minisafed.finalize_psbt(signed_psbt)
    tx = finalized_psbt.tx.serialize_with_witness().hex()
    bitcoind.rpc.sendrawtransaction(tx)


def test_list_spend(minisafed, bitcoind):
    # Start by creating two conflicting Spend PSBTs. The first one will have a change
    # output but not the second one.
    addr = minisafed.rpc.getnewaddress()["address"]
    value_a = 0.2567
    bitcoind.rpc.sendtoaddress(addr, value_a)
    wait_for(lambda: len(minisafed.rpc.listcoins()["coins"]) == 1)
    outpoints = [c["outpoint"] for c in minisafed.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): int(value_a * COIN // 2),
    }
    res = minisafed.rpc.createspend(outpoints, destinations, 6)
    assert "psbt" in res

    addr = minisafed.rpc.getnewaddress()["address"]
    value_b = 0.0987
    bitcoind.rpc.sendtoaddress(addr, value_b)
    wait_for(lambda: len(minisafed.rpc.listcoins()["coins"]) == 2)
    outpoints = [c["outpoint"] for c in minisafed.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): int((value_a + value_b) * COIN - 1_000),
    }
    res_b = minisafed.rpc.createspend(outpoints, destinations, 2)
    assert "psbt" in res_b

    # Store them both in DB.
    assert len(minisafed.rpc.listspendtxs()["spend_txs"]) == 0
    minisafed.rpc.updatespend(res["psbt"])
    minisafed.rpc.updatespend(res_b["psbt"])

    # Listing all Spend transactions will list them both. It'll tell us which one has
    # change and which one doesn't.
    list_res = minisafed.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 2
    first_psbt = next(entry for entry in list_res if entry["psbt"] == res["psbt"])
    assert first_psbt["change_index"] == 1
    second_psbt = next(entry for entry in list_res if entry["psbt"] == res_b["psbt"])
    assert second_psbt["change_index"] is None

    # If we delete the first one, we'll get only the second one.
    first_psbt = PSBT.from_base64(res["psbt"])
    minisafed.rpc.delspendtx(first_psbt.tx.txid().hex())
    list_res = minisafed.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 1
    assert list_res[0]["psbt"] == res_b["psbt"]

    # If we delete the second one, result will be empty.
    second_psbt = PSBT.from_base64(res_b["psbt"])
    minisafed.rpc.delspendtx(second_psbt.tx.txid().hex())
    list_res = minisafed.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 0


def test_update_spend(minisafed, bitcoind):
    # Start by creating a Spend PSBT
    addr = minisafed.rpc.getnewaddress()["address"]
    bitcoind.rpc.sendtoaddress(addr, 0.2567)
    wait_for(lambda: len(minisafed.rpc.listcoins()["coins"]) > 0)
    outpoints = [c["outpoint"] for c in minisafed.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 200_000,
    }
    res = minisafed.rpc.createspend(outpoints, destinations, 6)
    assert "psbt" in res

    # Now update it
    assert len(minisafed.rpc.listspendtxs()["spend_txs"]) == 0
    minisafed.rpc.updatespend(res["psbt"])
    list_res = minisafed.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 1
    assert list_res[0]["psbt"] == res["psbt"]

    # We can add a signature and update it
    psbt_sig_a = PSBT.from_base64(res["psbt"])
    dummy_pk_a = bytes.fromhex(
        "0375e00eb72e29da82b89367947f29ef34afb75e8654f6ea368e0acdfd92976b7c"
    )
    dummy_sig_a = bytes.fromhex(
        "304402202b925395cfeaa0171a7a92982bb4891acc4a312cbe7691d8375d36796d5b570a0220378a8ab42832848e15d1aedded5fb360fedbdd6c39226144e527f0f1e19d539801"
    )
    psbt_sig_a.i[0].map[PSBT_IN_PARTIAL_SIG] = {dummy_pk_a: dummy_sig_a}
    psbt_sig_a_ser = psbt_sig_a.to_base64()
    minisafed.rpc.updatespend(psbt_sig_a_ser)

    # We'll get it when querying
    list_res = minisafed.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 1
    assert list_res[0]["psbt"] == psbt_sig_a_ser

    # We can add another signature to the empty PSBT and update it again
    psbt_sig_b = PSBT.from_base64(res["psbt"])
    dummy_pk_b = bytes.fromhex(
        "03a1b26313f430c4b15bb1fdce663207659d8cac749a0e53d70eff01874496feff"
    )
    dummy_sig_b = bytes.fromhex(
        "3044022005aebcd649fb8965f0591710fb3704931c3e8118ee60dd44917479f63ceba6d4022018b212900e5a80e9452366894de37f0d02fb9c89f1e94f34fb6ed7fd71c15c4101"
    )
    psbt_sig_b.i[0].map[PSBT_IN_PARTIAL_SIG] = {dummy_pk_b: dummy_sig_b}
    psbt_sig_b_ser = psbt_sig_b.to_base64()
    minisafed.rpc.updatespend(psbt_sig_b_ser)

    # It will have merged both.
    list_res = minisafed.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 1
    psbt_merged = PSBT.from_base64(list_res[0]["psbt"])
    assert len(psbt_merged.i[0].map[PSBT_IN_PARTIAL_SIG]) == 2
    assert psbt_merged.i[0].map[PSBT_IN_PARTIAL_SIG][dummy_pk_a] == dummy_sig_a
    assert psbt_merged.i[0].map[PSBT_IN_PARTIAL_SIG][dummy_pk_b] == dummy_sig_b


def test_broadcast_spend(minisafed, bitcoind):
    # Create a new coin and a spending tx for it.
    addr = minisafed.rpc.getnewaddress()["address"]
    bitcoind.rpc.sendtoaddress(addr, 0.2567)
    wait_for(lambda: len(minisafed.rpc.listcoins()["coins"]) > 0)
    outpoints = [c["outpoint"] for c in minisafed.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 200_000,
    }
    res = minisafed.rpc.createspend(outpoints, destinations, 6)
    psbt = PSBT.from_base64(res["psbt"])
    txid = psbt.tx.txid().hex()

    # We can't broadcast an unknown Spend
    with pytest.raises(RpcError, match="Unknown spend transaction.*"):
        minisafed.rpc.broadcastspend(txid)
    minisafed.rpc.updatespend(res["psbt"])

    # We can't broadcast an unsigned transaction
    with pytest.raises(RpcError, match="Failed to finalize the spend transaction.*"):
        minisafed.rpc.broadcastspend(txid)
    signed_psbt = minisafed.sign_psbt(PSBT.from_base64(res["psbt"]))
    minisafed.rpc.updatespend(signed_psbt.to_base64())

    # Now we've signed and stored it, the daemon will take care of finalizing
    # the PSBT before broadcasting the transaction.
    minisafed.rpc.broadcastspend(txid)


def test_start_rescan(minisafed, bitcoind):
    """Test we successfully retrieve all our transactions after losing state by rescanning."""
    initial_timestamp = int(time.time())

    # Some utility functions to DRY
    list_coins = lambda: minisafed.rpc.listcoins()["coins"]
    unspent_coins = lambda: (
        c for c in minisafed.rpc.listcoins()["coins"] if c["spend_info"] is None
    )
    sorted_coins = lambda: sorted(list_coins(), key=lambda c: c["outpoint"])

    def all_spent(coins):
        unspent = set(c["outpoint"] for c in unspent_coins())
        for c in coins:
            if c["outpoint"] in unspent:
                return False
        return True

    # We can rescan from one second before the tip timestamp, that's almost a no-op.
    tip_timestamp = bitcoind.rpc.getblockheader(bitcoind.rpc.getbestblockhash())["time"]
    minisafed.rpc.startrescan(tip_timestamp - 1)
    wait_for(lambda: minisafed.rpc.getinfo()["rescan_progress"] is None)
    # We can't rescan from an insane timestamp though.
    with pytest.raises(RpcError, match="Insane timestamp.*"):
        minisafed.rpc.startrescan(tip_timestamp)
    assert minisafed.rpc.getinfo()["rescan_progress"] is None
    future_timestamp = tip_timestamp + 60 * 60
    with pytest.raises(RpcError, match="Insane timestamp.*"):
        minisafed.rpc.startrescan(future_timestamp)
    assert minisafed.rpc.getinfo()["rescan_progress"] is None
    prebitcoin_timestamp = 1231006505 - 1
    with pytest.raises(RpcError, match="Insane timestamp."):
        minisafed.rpc.startrescan(prebitcoin_timestamp)
    assert minisafed.rpc.getinfo()["rescan_progress"] is None

    # First, get some coins
    for _ in range(10):
        addr = minisafed.rpc.getnewaddress()["address"]
        amount = random.randint(1, COIN * 10) / COIN
        txid = bitcoind.rpc.sendtoaddress(addr, amount)
        bitcoind.generate_block(random.randint(1, 10), wait_for_mempool=txid)
    wait_for(lambda: len(list_coins()) == 10)

    # Then simulate some regular activity (spend and receive)
    # TODO: instead of having randomness we should lay down all different cases (with or
    # without change, single or multiple inputs, sending externally or to self).
    for _ in range(5):
        addr = minisafed.rpc.getnewaddress()["address"]
        amount = random.randint(1, COIN * 10) / COIN
        txid = bitcoind.rpc.sendtoaddress(addr, amount)
        avail = list(unspent_coins())
        to_spend = random.sample(avail, random.randint(1, len(avail)))
        spend_coins(minisafed, bitcoind, to_spend)
        bitcoind.generate_block(random.randint(1, 5), wait_for_mempool=2)
        wait_for(lambda: all_spent(to_spend))
    wait_for(
        lambda: minisafed.rpc.getinfo()["blockheight"] == bitcoind.rpc.getblockcount()
    )

    # Move time forward one day as bitcoind will rescan the last 2 hours of block upon
    # importing a descriptor.
    now = int(time.time())
    added_time = 60 * 60 * 24
    bitcoind.rpc.setmocktime(now + added_time)
    bitcoind.generate_block(10)

    # Now delete the wallet state. When starting up we'll re-create a fresh database
    # and watchonly wallet. Those won't be aware of past coins for the configured
    # descriptor.
    coins_before = sorted_coins()
    minisafed.restart_fresh(bitcoind)
    assert len(list_coins()) == 0

    # Once the rescan is done, we must have detected all previous transactions.
    minisafed.rpc.startrescan(initial_timestamp)
    rescan_progress = minisafed.rpc.getinfo()["rescan_progress"]
    assert rescan_progress is None or 0 <= rescan_progress <= 1
    wait_for(lambda: minisafed.rpc.getinfo()["rescan_progress"] is None)
    wait_for(
        lambda: minisafed.rpc.getinfo()["blockheight"] == bitcoind.rpc.getblockcount()
    )
    assert coins_before == sorted_coins()
