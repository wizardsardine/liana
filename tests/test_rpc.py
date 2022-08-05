from fixtures import *


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
