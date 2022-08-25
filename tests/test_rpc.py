from fixtures import *
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
