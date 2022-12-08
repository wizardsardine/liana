import pytest
import random
import time

from fixtures import *
from test_framework.serializations import PSBT, PSBT_IN_PARTIAL_SIG
from test_framework.utils import wait_for, COIN, RpcError, get_txid, spend_coins


def test_getinfo(lianad):
    res = lianad.rpc.getinfo()
    assert res["version"] == "0.1"
    assert res["network"] == "regtest"
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == 101)
    res = lianad.rpc.getinfo()
    assert res["sync"] == 1.0
    assert "main" in res["descriptors"]
    assert res["rescan_progress"] is None


def test_getaddress(lianad):
    res = lianad.rpc.getnewaddress()
    assert "address" in res
    # We'll get a new one at every call
    assert res["address"] != lianad.rpc.getnewaddress()["address"]


def test_listcoins(lianad, bitcoind):
    # Initially empty
    res = lianad.rpc.listcoins()
    assert "coins" in res
    assert len(res["coins"]) == 0

    # If we send a coin, we'll get a new entry. Note we monitor for unconfirmed
    # funds as well.
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 1)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    res = lianad.rpc.listcoins()["coins"]
    assert txid == res[0]["outpoint"][:64]
    assert res[0]["amount"] == 1 * COIN
    assert res[0]["block_height"] is None
    assert res[0]["spend_info"] is None

    # If the coin gets confirmed, it'll be marked as such.
    bitcoind.generate_block(1, wait_for_mempool=txid)
    block_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.listcoins()["coins"][0]["block_height"] == block_height)

    # Same if the coin gets spent.
    spend_tx = spend_coins(lianad, bitcoind, (res[0],))
    spend_txid = get_txid(spend_tx)
    wait_for(lambda: lianad.rpc.listcoins()["coins"][0]["spend_info"] is not None)
    spend_info = lianad.rpc.listcoins()["coins"][0]["spend_info"]
    assert spend_info["txid"] == spend_txid
    assert spend_info["height"] is None

    # And if this spending tx gets confirmed.
    bitcoind.generate_block(1, wait_for_mempool=spend_txid)
    curr_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == curr_height)
    spend_info = lianad.rpc.listcoins()["coins"][0]["spend_info"]
    assert spend_info["txid"] == spend_txid
    assert spend_info["height"] == curr_height


def test_jsonrpc_server(lianad, bitcoind):
    """Test passing parameters as a list or a mapping."""
    addr = lianad.rpc.getnewaddress()["address"]
    bitcoind.rpc.sendtoaddress(addr, 1)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    outpoints = [lianad.rpc.listcoins()["coins"][0]["outpoint"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 20_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 18)
    assert "psbt" in res
    res = lianad.rpc.createspend(
        outpoints=outpoints, destinations=destinations, feerate=18
    )
    assert "psbt" in res


def test_create_spend(lianad, bitcoind):
    # Receive a number of coins in different blocks on different addresses, and
    # one more on the same address.
    for _ in range(15):
        addr = lianad.rpc.getnewaddress()["address"]
        txid = bitcoind.rpc.sendtoaddress(addr, 0.01)
        bitcoind.generate_block(1, wait_for_mempool=txid)
    txid = bitcoind.rpc.sendtoaddress(addr, 0.3556)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 16)

    # Stop the daemon, should be a no-op
    lianad.stop()
    lianad.start()

    # Now create a transaction spending all those coins to a few addresses
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 200_000,
        bitcoind.rpc.getnewaddress(): 400_000,
        bitcoind.rpc.getnewaddress(): 1_000_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 18)
    assert "psbt" in res

    # The transaction must contain a change output.
    spend_psbt = PSBT.from_base64(res["psbt"])
    assert len(spend_psbt.o) == 4
    assert len(spend_psbt.tx.vout) == 4

    # We can sign it and broadcast it.
    signed_psbt = lianad.sign_psbt(PSBT.from_base64(res["psbt"]))
    finalized_psbt = lianad.finalize_psbt(signed_psbt)
    tx = finalized_psbt.tx.serialize_with_witness().hex()
    bitcoind.rpc.sendrawtransaction(tx)


def test_list_spend(lianad, bitcoind):
    # Start by creating two conflicting Spend PSBTs. The first one will have a change
    # output but not the second one.
    addr = lianad.rpc.getnewaddress()["address"]
    value_a = 0.2567
    bitcoind.rpc.sendtoaddress(addr, value_a)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): int(value_a * COIN // 2),
    }
    res = lianad.rpc.createspend(destinations, outpoints, 6)
    assert "psbt" in res

    addr = lianad.rpc.getnewaddress()["address"]
    value_b = 0.0987
    bitcoind.rpc.sendtoaddress(addr, value_b)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 2)
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): int((value_a + value_b) * COIN - 1_000),
    }
    res_b = lianad.rpc.createspend(destinations, outpoints, 2)
    assert "psbt" in res_b

    # Store them both in DB.
    assert len(lianad.rpc.listspendtxs()["spend_txs"]) == 0
    lianad.rpc.updatespend(res["psbt"])
    lianad.rpc.updatespend(res_b["psbt"])

    # Listing all Spend transactions will list them both. It'll tell us which one has
    # change and which one doesn't.
    list_res = lianad.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 2
    first_psbt = next(entry for entry in list_res if entry["psbt"] == res["psbt"])
    assert first_psbt["change_index"] == 1
    second_psbt = next(entry for entry in list_res if entry["psbt"] == res_b["psbt"])
    assert second_psbt["change_index"] is None

    # If we delete the first one, we'll get only the second one.
    first_psbt = PSBT.from_base64(res["psbt"])
    lianad.rpc.delspendtx(first_psbt.tx.txid().hex())
    list_res = lianad.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 1
    assert list_res[0]["psbt"] == res_b["psbt"]

    # If we delete the second one, result will be empty.
    second_psbt = PSBT.from_base64(res_b["psbt"])
    lianad.rpc.delspendtx(second_psbt.tx.txid().hex())
    list_res = lianad.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 0


def test_update_spend(lianad, bitcoind):
    # Start by creating a Spend PSBT
    addr = lianad.rpc.getnewaddress()["address"]
    bitcoind.rpc.sendtoaddress(addr, 0.2567)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) > 0)
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 200_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 6)
    assert "psbt" in res

    # Now update it
    assert len(lianad.rpc.listspendtxs()["spend_txs"]) == 0
    lianad.rpc.updatespend(res["psbt"])
    list_res = lianad.rpc.listspendtxs()["spend_txs"]
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
    lianad.rpc.updatespend(psbt_sig_a_ser)

    # We'll get it when querying
    list_res = lianad.rpc.listspendtxs()["spend_txs"]
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
    lianad.rpc.updatespend(psbt_sig_b_ser)

    # It will have merged both.
    list_res = lianad.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 1
    psbt_merged = PSBT.from_base64(list_res[0]["psbt"])
    assert len(psbt_merged.i[0].map[PSBT_IN_PARTIAL_SIG]) == 2
    assert psbt_merged.i[0].map[PSBT_IN_PARTIAL_SIG][dummy_pk_a] == dummy_sig_a
    assert psbt_merged.i[0].map[PSBT_IN_PARTIAL_SIG][dummy_pk_b] == dummy_sig_b


def test_broadcast_spend(lianad, bitcoind):
    # Create a new coin and a spending tx for it.
    addr = lianad.rpc.getnewaddress()["address"]
    bitcoind.rpc.sendtoaddress(addr, 0.2567)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) > 0)
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 200_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 6)
    psbt = PSBT.from_base64(res["psbt"])
    txid = psbt.tx.txid().hex()

    # We can't broadcast an unknown Spend
    with pytest.raises(RpcError, match="Unknown spend transaction.*"):
        lianad.rpc.broadcastspend(txid)
    lianad.rpc.updatespend(res["psbt"])

    # We can't broadcast an unsigned transaction
    with pytest.raises(RpcError, match="Failed to finalize the spend transaction.*"):
        lianad.rpc.broadcastspend(txid)
    signed_psbt = lianad.sign_psbt(PSBT.from_base64(res["psbt"]))
    lianad.rpc.updatespend(signed_psbt.to_base64())

    # Now we've signed and stored it, the daemon will take care of finalizing
    # the PSBT before broadcasting the transaction.
    lianad.rpc.broadcastspend(txid)


def test_start_rescan(lianad, bitcoind):
    """Test we successfully retrieve all our transactions after losing state by rescanning."""
    initial_timestamp = int(time.time())
    first_address = lianad.rpc.getnewaddress()
    second_address = lianad.rpc.getnewaddress()

    # Some utility functions to DRY
    list_coins = lambda: lianad.rpc.listcoins()["coins"]
    unspent_coins = lambda: (
        c for c in lianad.rpc.listcoins()["coins"] if c["spend_info"] is None
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
    lianad.rpc.startrescan(tip_timestamp - 1)
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)
    # We can't rescan from an insane timestamp though.
    with pytest.raises(RpcError, match="Insane timestamp.*"):
        lianad.rpc.startrescan(tip_timestamp)
    assert lianad.rpc.getinfo()["rescan_progress"] is None
    future_timestamp = tip_timestamp + 60 * 60
    with pytest.raises(RpcError, match="Insane timestamp.*"):
        lianad.rpc.startrescan(future_timestamp)
    assert lianad.rpc.getinfo()["rescan_progress"] is None
    prebitcoin_timestamp = 1231006505 - 1
    with pytest.raises(RpcError, match="Insane timestamp."):
        lianad.rpc.startrescan(prebitcoin_timestamp)
    assert lianad.rpc.getinfo()["rescan_progress"] is None

    # First, get some coins
    for _ in range(10):
        addr = lianad.rpc.getnewaddress()["address"]
        amount = random.randint(1, COIN * 10) / COIN
        txid = bitcoind.rpc.sendtoaddress(addr, amount)
        bitcoind.generate_block(random.randint(1, 10), wait_for_mempool=txid)
    wait_for(lambda: len(list_coins()) == 10)

    # Then simulate some regular activity (spend and receive)
    # TODO: instead of having randomness we should lay down all different cases (with or
    # without change, single or multiple inputs, sending externally or to self).
    for _ in range(5):
        addr = lianad.rpc.getnewaddress()["address"]
        amount = random.randint(1, COIN * 10) / COIN
        txid = bitcoind.rpc.sendtoaddress(addr, amount)
        avail = list(unspent_coins())
        to_spend = random.sample(avail, random.randint(1, len(avail)))
        spend_coins(lianad, bitcoind, to_spend)
        bitcoind.generate_block(random.randint(1, 5), wait_for_mempool=2)
        wait_for(lambda: all_spent(to_spend))
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )

    # Receiving addresses are derived at much higher indexes now.
    assert lianad.rpc.getnewaddress() not in (first_address, second_address)

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
    lianad.restart_fresh(bitcoind)
    assert len(list_coins()) == 0

    # The wallet isn't aware what derivation indexes were used. Necessarily it'll start
    # from 0.
    assert lianad.rpc.getnewaddress() == first_address

    # Once the rescan is done, we must have detected all previous transactions.
    lianad.rpc.startrescan(initial_timestamp)
    rescan_progress = lianad.rpc.getinfo()["rescan_progress"]
    assert rescan_progress is None or 0 <= rescan_progress <= 1
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )
    assert coins_before == sorted_coins()

    # Now that it caught up it noticed which one were used onchain, so it won't reuse
    # this derivation indexes anymore.
    assert lianad.rpc.getnewaddress() not in (first_address, second_address)


def test_listtransactions(lianad, bitcoind):
    """Test listing of transactions by txid and timespan"""

    def sign_and_broadcast(psbt):
        txid = psbt.tx.txid().hex()
        psbt = lianad.sign_psbt(psbt)
        lianad.rpc.updatespend(psbt.to_base64())
        lianad.rpc.broadcastspend(txid)
        return txid

    def wait_synced():
        wait_for(
            lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
        )

    best_block = bitcoind.rpc.getbestblockhash()
    initial_timestamp = bitcoind.rpc.getblockheader(best_block)["time"]
    wait_synced()

    # Deposit multiple coins in a single transaction
    destinations = {
        lianad.rpc.getnewaddress()["address"]: 0.0123456,
        lianad.rpc.getnewaddress()["address"]: 0.0123457,
        lianad.rpc.getnewaddress()["address"]: 0.0123458,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 3)
    bitcoind.generate_block(1, wait_for_mempool=txid)

    # Mine 12 blocks to force the blocktime to increase
    bitcoind.generate_block(12)
    wait_synced()
    best_block = bitcoind.rpc.getbestblockhash()
    second_timestamp = bitcoind.rpc.getblockheader(best_block)["time"]
    assert second_timestamp > initial_timestamp

    # Deposit a coin that will be unspent
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 0.123456)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 4)
    bitcoind.generate_block(1, wait_for_mempool=txid)

    # Deposit a coin that will be spent with a change output
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 0.23456)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 5)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    outpoint = next(
        c["outpoint"] for c in lianad.rpc.listcoins()["coins"] if txid in c["outpoint"]
    )
    destinations = {
        bitcoind.rpc.getnewaddress(): 100_000,
    }
    res = lianad.rpc.createspend(destinations, [outpoint], 6)
    psbt = PSBT.from_base64(res["psbt"])
    txid = sign_and_broadcast(psbt)
    bitcoind.generate_block(1, wait_for_mempool=txid)

    # Mine 12 blocks to force the blocktime to increase
    bitcoind.generate_block(12)
    wait_synced()
    best_block = bitcoind.rpc.getbestblockhash()
    third_timestamp = bitcoind.rpc.getblockheader(best_block)["time"]
    assert third_timestamp > second_timestamp
    bitcoind.generate_block(12)
    wait_synced()

    # Deposit a coin that will be spent with a change output and also two new deposits
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 0.3456)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 7)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    outpoint = next(
        c["outpoint"] for c in lianad.rpc.listcoins()["coins"] if txid in c["outpoint"]
    )
    destinations = {
        bitcoind.rpc.getnewaddress(): 11_000,
        addr: 12_000,  # Even with address reuse! Booooh
        lianad.rpc.getnewaddress()["address"]: 13_000,
    }
    res = lianad.rpc.createspend(destinations, [outpoint], 6)
    psbt = PSBT.from_base64(res["psbt"])
    txid = sign_and_broadcast(psbt)
    bitcoind.generate_block(1, wait_for_mempool=txid)

    # Deposit a coin that will be spending (unconfirmed spend transaction)
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 0.456)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 11)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    outpoint = next(
        c["outpoint"] for c in lianad.rpc.listcoins()["coins"] if txid in c["outpoint"]
    )
    destinations = {
        bitcoind.rpc.getnewaddress(): 11_000,
    }
    res = lianad.rpc.createspend(destinations, [outpoint], 6)
    psbt = PSBT.from_base64(res["psbt"])
    txid = sign_and_broadcast(psbt)

    # At this point we have 12 spent and unspent coins, one of them is unconfirmed.
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 12)

    # However some of them share the same txid! This is the case of the 3 first coins
    # for instance, or the Spend transactions with multiple outputs at one of our addresses.
    # In total, that's 8 transactions.
    txids = set(c["outpoint"][:-2] for c in lianad.rpc.listcoins()["coins"])
    assert len(txids) == 8

    # We can query all of them at once using listtransactions. The result contains all
    # the correct transactions as hex, with no duplicate.
    all_txs = lianad.rpc.listtransactions(list(txids))["transactions"]
    assert len(all_txs) == 8
    for tx in all_txs:
        txid = bitcoind.rpc.decoderawtransaction(tx["tx"])["txid"]
        txids.remove(txid)  # This will raise an error if it isn't there

    # We can also query them one by one.
    txids = set(c["outpoint"][:-2] for c in lianad.rpc.listcoins()["coins"])
    for txid in txids:
        txs = lianad.rpc.listtransactions([txid])["transactions"]
        bit_txid = bitcoind.rpc.decoderawtransaction(txs[0]["tx"])["txid"]
        assert bit_txid == txid

    # We can query all confirmed transactions
    best_block = bitcoind.rpc.getbestblockhash()
    final_timestamp = bitcoind.rpc.getblockheader(best_block)["time"]
    txs = lianad.rpc.listconfirmed(initial_timestamp, final_timestamp, 10)[
        "transactions"
    ]
    assert len(txs) == 7, "The last spend tx is unconfirmed"
    for tx in txs:
        txid = bitcoind.rpc.decoderawtransaction(tx["tx"])["txid"]
        txids.remove(txid)  # This will raise an error if it isn't there

    # We can limit the size of the result
    txs = lianad.rpc.listconfirmed(initial_timestamp, final_timestamp, 5)[
        "transactions"
    ]
    assert len(txs) == 5

    # We can restrict the query to a certain time window.
    # First get the txid of all the transactions that happened during this timespan.
    txids = set()
    for coin in lianad.rpc.listcoins()["coins"]:
        if coin["block_height"] is None:
            continue
        block_hash = bitcoind.rpc.getblockhash(coin["block_height"])
        block_time = bitcoind.rpc.getblockheader(block_hash)["time"]
        spend_time = None
        if coin["spend_info"] is not None and coin["spend_info"]["height"] is not None:
            spend_bhash = bitcoind.rpc.getblockhash(coin["spend_info"]["height"])
            spend_time = bitcoind.rpc.getblockheader(spend_bhash)["time"]
        if (block_time >= second_timestamp and block_time <= third_timestamp) or (
            spend_time is not None
            and spend_time >= second_timestamp
            and spend_time <= third_timestamp
        ):
            txids.add(coin["outpoint"][:-2])
    # It's all 7 minus the first deposit and the last confirmed spend. So that's 5 of them.
    assert len(txids) == 3
    # Now let's compare with what lianad is giving us.
    txs = lianad.rpc.listconfirmed(second_timestamp, third_timestamp, 10)[
        "transactions"
    ]
    assert len(txs) == 3
    bit_txids = set(bitcoind.rpc.decoderawtransaction(tx["tx"])["txid"] for tx in txs)
    assert bit_txids == txids
