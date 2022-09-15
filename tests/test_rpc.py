from fixtures import *
from test_framework.serializations import PSBT
from test_framework.utils import wait_for, COIN


def test_getinfo(minisafed):
    res = minisafed.rpc.getinfo()
    assert res["version"] == "0.1"
    assert res["network"] == "regtest"
    assert res["blockheight"] == 101
    assert res["sync"] == 1.0
    assert "main" in res["descriptors"]


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

    # If the coin gets confirmed, it'll be marked as such.
    bitcoind.generate_block(1, wait_for_mempool=txid)
    block_height = bitcoind.rpc.getblockcount()
    wait_for(
        lambda: minisafed.rpc.listcoins()["coins"][0]["block_height"] == block_height
    )


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
    spend_psbt = PSBT()
    spend_psbt.deserialize(res["psbt"])
    assert len(spend_psbt.outputs) == 4
    assert len(spend_psbt.tx.vout) == 4

    # We can sign it and broadcast it.
    signed_tx_hex = minisafed.sign_psbt(spend_psbt)
    bitcoind.rpc.sendrawtransaction(signed_tx_hex)


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
    minisafed.rpc.updatespend(res["psbt"])

    # TODO: check it's stored once we implement 'listspendtxs'
    # TODO: check with added signatures once we implement 'listspendtxs'
