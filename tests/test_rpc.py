import pytest
import random
import re
import time

from fixtures import *
from test_framework.serializations import (
    PSBT,
    PSBT_IN_PARTIAL_SIG,
    PSBT_IN_NON_WITNESS_UTXO,
)
from test_framework.utils import (
    wait_for,
    COIN,
    RpcError,
    get_txid,
    spend_coins,
    sign_and_broadcast,
    sign_and_broadcast_psbt,
    USE_TAPROOT,
)

MAX_DERIV = 2**31 - 1


def test_getinfo(lianad):
    res = lianad.rpc.getinfo()
    assert "timestamp" in res.keys()
    assert res["version"] == "11.0.0-dev"
    assert res["network"] == "regtest"
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == 101)
    res = lianad.rpc.getinfo()
    assert res["sync"] == 1.0
    assert "main" in res["descriptors"]
    assert res["rescan_progress"] is None
    last_poll_timestamp = res["last_poll_timestamp"]
    assert last_poll_timestamp is not None
    time.sleep(lianad.poll_interval_secs + 1)
    res = lianad.rpc.getinfo()
    assert res["last_poll_timestamp"] > last_poll_timestamp
    assert res["receive_index"] == 0
    assert res["change_index"] == 0


def test_update_derivation_indexes(lianad):
    info = lianad.rpc.getinfo()
    assert info["receive_index"] == 0
    assert info["change_index"] == 0

    ret = lianad.rpc.updatederivationindexes(0, 0)
    info = lianad.rpc.getinfo()
    assert info["receive_index"] == 0
    assert info["change_index"] == 0
    assert ret["receive"] == 0
    assert ret["change"] == 0

    ret = lianad.rpc.updatederivationindexes(receive=3)
    info = lianad.rpc.getinfo()
    assert info["receive_index"] == 3
    assert info["change_index"] == 0
    assert ret["receive"] == 3
    assert ret["change"] == 0

    ret = lianad.rpc.updatederivationindexes(change=4)
    info = lianad.rpc.getinfo()
    assert info["receive_index"] == 3
    assert info["change_index"] == 4
    assert ret["receive"] == 3
    assert ret["change"] == 4

    ret = lianad.rpc.updatederivationindexes(receive=1, change=2)
    info = lianad.rpc.getinfo()
    assert info["receive_index"] == 3
    assert info["change_index"] == 4
    assert ret["receive"] == 3
    assert ret["change"] == 4

    ret = lianad.rpc.updatederivationindexes(5, 6)
    info = lianad.rpc.getinfo()
    assert info["receive_index"] == 5
    assert info["change_index"] == 6
    assert ret["receive"] == 5
    assert ret["change"] == 6

    ret = lianad.rpc.updatederivationindexes(0, 0)
    info = lianad.rpc.getinfo()
    assert info["receive_index"] == 5
    assert info["change_index"] == 6
    assert ret["receive"] == 5
    assert ret["change"] == 6

    # Will explicitly error on invalid indexes
    with pytest.raises(
        RpcError,
        match=re.escape("Invalid params: Invalid value for 'receive' param"),
    ):
        lianad.rpc.updatederivationindexes(-1)

    with pytest.raises(
        RpcError,
        match=re.escape("Invalid params: Invalid value for 'change' param"),
    ):
        lianad.rpc.updatederivationindexes(0, -1)

    with pytest.raises(
        RpcError,
        match=re.escape("Unhardened or overflowing BIP32 derivation index."),
    ):
        lianad.rpc.updatederivationindexes(MAX_DERIV + 1, 2)

    with pytest.raises(
        RpcError,
        match=re.escape("Unhardened or overflowing BIP32 derivation index."),
    ):
        lianad.rpc.updatederivationindexes(0, MAX_DERIV + 1)

    with pytest.raises(
        RpcError,
        match=re.escape("Unhardened or overflowing BIP32 derivation index."),
    ):
        lianad.rpc.updatederivationindexes(receive=(MAX_DERIV + 1))

    with pytest.raises(
        RpcError,
        match=re.escape("Unhardened or overflowing BIP32 derivation index."),
    ):
        lianad.rpc.updatederivationindexes(change=(MAX_DERIV + 1))

    with pytest.raises(
        RpcError,
        match=re.escape("Invalid params: Missing 'receive' or 'change' parameter"),
    ):
        lianad.rpc.updatederivationindexes()

    last_derivs = lianad.rpc.updatederivationindexes(0, 0)
    last_receive = last_derivs["receive"]
    last_change = last_derivs["change"]

    ret = lianad.rpc.updatederivationindexes(0, (MAX_DERIV - 1))
    assert ret["receive"] == last_receive
    assert ret["change"] == last_change + 1000

    last_derivs = lianad.rpc.updatederivationindexes(0, 0)
    last_receive = last_derivs["receive"]
    last_change = last_derivs["change"]

    ret = lianad.rpc.updatederivationindexes((MAX_DERIV - 1), 0)
    assert ret["receive"] == last_receive + 1000
    assert ret["change"] == last_change


def test_getaddress(lianad):
    res = lianad.rpc.getnewaddress()
    assert "address" in res
    # The first new wallet address has index 1
    assert res["derivation_index"] == 1
    # We'll get a new one at every call
    assert res["address"] != lianad.rpc.getnewaddress()["address"]
    # new address has derivation_index higher than the previous one
    assert lianad.rpc.getnewaddress()["derivation_index"] == res["derivation_index"] + 2
    info = lianad.rpc.getinfo()
    assert info["receive_index"] == res["derivation_index"] + 2  # 3 == 1 + 2
    assert info["change_index"] == 0


def test_listaddresses(lianad):
    list1 = lianad.rpc.listaddresses(2, 5)
    list2 = lianad.rpc.listaddresses(start_index=2, count=5)
    assert list1 == list2
    assert "addresses" in list1
    addr = list1["addresses"]
    assert addr[0]["index"] == 2
    assert addr[-1]["index"] == 6

    list3 = (
        lianad.rpc.listaddresses()
    )  # start_index = 0, receive_index = 0 (returns 1 "used" address for index 0)
    _ = lianad.rpc.getnewaddress()  # start_index = 0, receive_index = 1
    _ = lianad.rpc.getnewaddress()  # start_index = 0, receive_index = 2
    # list4 returns all indexes from 0 up to last used.
    # The first new address has index 1, so returned indexes are 0, 1, 2:
    list4 = lianad.rpc.listaddresses()
    assert len(list4["addresses"]) == len(list3["addresses"]) + 2 == 3
    list5 = lianad.rpc.listaddresses(0)
    assert list4 == list5

    # Will explicitly error on invalid start_index.
    with pytest.raises(
        RpcError,
        match=re.escape(
            "Invalid params: Invalid value for \\'start_index\\': \"blabla\""
        ),
    ):
        lianad.rpc.listaddresses("blabla", None)

    # Will explicitly error on invalid count.
    with pytest.raises(
        RpcError,
        match=re.escape("Invalid params: Invalid value for \\'count\\': \"blb\""),
    ):
        lianad.rpc.listaddresses(0, "blb")


def test_listrevealedaddresses(lianad, bitcoind):

    # Get addresses for reference:
    addresses = lianad.rpc.listaddresses(0, 10)["addresses"]

    # We start with index 0 already "revealed":
    list_rec = lianad.rpc.listrevealedaddresses(False, False, 10)
    assert list_rec["continue_from"] is None  # there are no more addresses to list
    assert len(list_rec["addresses"]) == 1
    assert list_rec["addresses"][0]["index"] == 0
    assert list_rec["addresses"][0]["address"] == addresses[0]["receive"]
    assert list_rec["addresses"][0]["used_count"] == 0
    assert list_rec["addresses"][0]["label"] is None

    # Generate some addresses.
    addr_1 = lianad.rpc.getnewaddress()["address"]
    addr_2 = lianad.rpc.getnewaddress()["address"]
    addr_3 = lianad.rpc.getnewaddress()["address"]
    addr_4 = lianad.rpc.getnewaddress()["address"]
    addr_5 = lianad.rpc.getnewaddress()["address"]
    addr_6 = lianad.rpc.getnewaddress()["address"]
    addr_7 = lianad.rpc.getnewaddress()["address"]

    # Last revealed receive index is 7.
    assert lianad.rpc.getinfo()["receive_index"] == 7

    # Set some labels
    lianad.rpc.updatelabels(
        {addr_1: "my test label 1", addr_5: "my test label 5"},
    )

    # Passing None or omitting start_index parameter is the same:
    assert lianad.rpc.listrevealedaddresses(
        False, False, 10
    ) == lianad.rpc.listrevealedaddresses(False, False, 10, None)

    # If we continue_from a value above our last revealed index, we'll start from the last index.
    assert lianad.rpc.listrevealedaddresses(
        False, False, 10
    ) == lianad.rpc.listrevealedaddresses(False, False, 10, 100)

    # Similarly if we start from a hardened index:
    assert lianad.rpc.listrevealedaddresses(
        False, False, 10
    ) == lianad.rpc.listrevealedaddresses(False, False, 10, 4_294_967_295)

    # Get 3 addresses starting at last revealed index:
    list_rec = lianad.rpc.listrevealedaddresses(False, False, 3)
    assert list_rec["continue_from"] == 4
    assert len(list_rec["addresses"]) == 3
    assert list_rec["addresses"][0]["index"] == 7
    assert list_rec["addresses"][0]["address"] == addr_7
    assert list_rec["addresses"][0]["used_count"] == 0
    assert list_rec["addresses"][0]["label"] is None
    assert list_rec["addresses"][1]["index"] == 6
    assert list_rec["addresses"][1]["address"] == addr_6
    assert list_rec["addresses"][1]["used_count"] == 0
    assert list_rec["addresses"][1]["label"] is None
    assert list_rec["addresses"][2]["index"] == 5
    assert list_rec["addresses"][2]["address"] == addr_5
    assert list_rec["addresses"][2]["used_count"] == 0
    assert list_rec["addresses"][2]["label"] == "my test label 5"

    # Get next 3 using continue_from returned above as start_index:
    list_rec = lianad.rpc.listrevealedaddresses(False, False, 3, 4)
    assert list_rec["continue_from"] == 1
    assert len(list_rec["addresses"]) == 3
    assert list_rec["addresses"][0]["index"] == 4
    assert list_rec["addresses"][0]["address"] == addr_4
    assert list_rec["addresses"][0]["used_count"] == 0
    assert list_rec["addresses"][0]["label"] is None
    assert list_rec["addresses"][1]["index"] == 3
    assert list_rec["addresses"][1]["address"] == addr_3
    assert list_rec["addresses"][1]["used_count"] == 0
    assert list_rec["addresses"][1]["label"] is None
    assert list_rec["addresses"][2]["index"] == 2
    assert list_rec["addresses"][2]["address"] == addr_2
    assert list_rec["addresses"][2]["used_count"] == 0
    assert list_rec["addresses"][2]["label"] is None

    # Get final page of results consisting of 2 addresses:
    list_rec = lianad.rpc.listrevealedaddresses(False, False, 3, 1)
    assert list_rec["continue_from"] is None  # final page
    assert len(list_rec["addresses"]) == 2  # num addresses remaining is below limit
    assert list_rec["addresses"][0]["index"] == 1
    assert list_rec["addresses"][0]["address"] == addr_1
    assert list_rec["addresses"][0]["used_count"] == 0
    assert list_rec["addresses"][0]["label"] == "my test label 1"
    assert list_rec["addresses"][1]["index"] == 0
    assert list_rec["addresses"][1]["address"] == addresses[0]["receive"]
    assert list_rec["addresses"][1]["used_count"] == 0
    assert list_rec["addresses"][1]["label"] is None

    # Receive funds at a couple of addresses.
    destinations = {
        addr_2: 0.003,
        addr_4: 0.004,
        addr_7: 0.005,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 3)

    # The addresses are shown as used.
    list_rec = lianad.rpc.listrevealedaddresses(False, False, 3)
    assert list_rec["continue_from"] == 4
    assert len(list_rec["addresses"]) == 3
    assert list_rec["addresses"][0]["index"] == 7
    assert list_rec["addresses"][0]["address"] == addr_7
    assert list_rec["addresses"][0]["used_count"] == 1
    assert list_rec["addresses"][0]["label"] is None
    assert list_rec["addresses"][1]["index"] == 6
    assert list_rec["addresses"][1]["address"] == addr_6
    assert list_rec["addresses"][1]["used_count"] == 0
    assert list_rec["addresses"][1]["label"] is None
    assert list_rec["addresses"][2]["index"] == 5
    assert list_rec["addresses"][2]["address"] == addr_5
    assert list_rec["addresses"][2]["used_count"] == 0
    assert list_rec["addresses"][2]["label"] == "my test label 5"

    list_rec = lianad.rpc.listrevealedaddresses(False, False, 3, 4)
    assert list_rec["continue_from"] == 1
    assert len(list_rec["addresses"]) == 3
    assert list_rec["addresses"][0]["index"] == 4
    assert list_rec["addresses"][0]["address"] == addr_4
    assert list_rec["addresses"][0]["used_count"] == 1
    assert list_rec["addresses"][0]["label"] is None
    assert list_rec["addresses"][1]["index"] == 3
    assert list_rec["addresses"][1]["address"] == addr_3
    assert list_rec["addresses"][1]["used_count"] == 0
    assert list_rec["addresses"][1]["label"] is None
    assert list_rec["addresses"][2]["index"] == 2
    assert list_rec["addresses"][2]["address"] == addr_2
    assert list_rec["addresses"][2]["used_count"] == 1
    assert list_rec["addresses"][2]["label"] is None

    # We can exclude used addresses:
    list_rec = lianad.rpc.listrevealedaddresses(False, True, 3)
    assert list_rec["continue_from"] == 2
    assert len(list_rec["addresses"]) == 3
    assert list_rec["addresses"][0]["index"] == 6
    assert list_rec["addresses"][0]["address"] == addr_6
    assert list_rec["addresses"][0]["used_count"] == 0
    assert list_rec["addresses"][0]["label"] is None
    assert list_rec["addresses"][1]["index"] == 5
    assert list_rec["addresses"][1]["address"] == addr_5
    assert list_rec["addresses"][1]["used_count"] == 0
    assert list_rec["addresses"][1]["label"] == "my test label 5"
    assert list_rec["addresses"][2]["index"] == 3  # index 4 was skipped
    assert list_rec["addresses"][2]["address"] == addr_3
    assert list_rec["addresses"][2]["used_count"] == 0
    assert list_rec["addresses"][2]["label"] is None

    # We can exclude used also if we continue from the value in the response above:
    list_rec = lianad.rpc.listrevealedaddresses(False, True, 3, 2)
    assert list_rec["continue_from"] is None
    assert len(list_rec["addresses"]) == 2
    assert list_rec["addresses"][0]["index"] == 1  # index 2 was skipped
    assert list_rec["addresses"][0]["address"] == addr_1
    assert list_rec["addresses"][0]["used_count"] == 0
    assert list_rec["addresses"][0]["label"] == "my test label 1"
    assert list_rec["addresses"][1]["index"] == 0
    assert list_rec["addresses"][1]["address"] == addresses[0]["receive"]
    assert list_rec["addresses"][1]["used_count"] == 0
    assert list_rec["addresses"][1]["label"] is None

    # Receive funds at some of the same addresses again.
    destinations = {
        addr_2: 0.0031,
        addr_4: 0.0041,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 5)

    # One more coin to addr_2.
    destinations = {
        addr_2: 0.0032,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 6)

    # The counts have updated:
    list_rec = lianad.rpc.listrevealedaddresses(False, False, 3, 4)
    assert list_rec["continue_from"] == 1
    assert len(list_rec["addresses"]) == 3
    assert list_rec["addresses"][0]["index"] == 4
    assert list_rec["addresses"][0]["address"] == addr_4
    assert list_rec["addresses"][0]["used_count"] == 2
    assert list_rec["addresses"][0]["label"] is None
    assert list_rec["addresses"][1]["index"] == 3
    assert list_rec["addresses"][1]["address"] == addr_3
    assert list_rec["addresses"][1]["used_count"] == 0
    assert list_rec["addresses"][1]["label"] is None
    assert list_rec["addresses"][2]["index"] == 2
    assert list_rec["addresses"][2]["address"] == addr_2
    assert list_rec["addresses"][2]["used_count"] == 3
    assert list_rec["addresses"][2]["label"] is None

    # If we request limit 0, we get empty list:
    list_rec = lianad.rpc.listrevealedaddresses(False, False, 0)
    assert list_rec["continue_from"] == 7  # same as starting index
    assert len(list_rec["addresses"]) == 0

    # The poller currently sets the change index to match the receive index.
    # See https://github.com/wizardsardine/liana/issues/1333.
    assert lianad.rpc.getinfo()["receive_index"] == 7
    assert lianad.rpc.getinfo()["change_index"] == 7

    # We can get change addresses:
    list_cha = lianad.rpc.listrevealedaddresses(True, False, 3)
    assert list_cha["continue_from"] == 4
    assert len(list_cha["addresses"]) == 3
    assert list_cha["addresses"][0]["index"] == 7
    assert list_cha["addresses"][0]["address"] == addresses[7]["change"]
    assert list_cha["addresses"][0]["used_count"] == 0
    assert list_cha["addresses"][0]["label"] is None
    assert list_cha["addresses"][1]["index"] == 6
    assert list_cha["addresses"][1]["address"] == addresses[6]["change"]
    assert list_cha["addresses"][1]["used_count"] == 0
    assert list_cha["addresses"][1]["label"] is None
    assert list_cha["addresses"][2]["index"] == 5
    assert list_cha["addresses"][2]["address"] == addresses[5]["change"]
    assert list_cha["addresses"][2]["used_count"] == 0
    assert list_cha["addresses"][2]["label"] is None


def test_listcoins(lianad, bitcoind):
    # Initially empty
    res = lianad.rpc.listcoins()
    assert "coins" in res
    assert len(res["coins"]) == 0

    # If we send a coin, we'll get a new entry. Note we monitor for unconfirmed
    # funds as well.
    addr_a = lianad.rpc.getnewaddress()
    txid_a = bitcoind.rpc.sendtoaddress(addr_a["address"], 1)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    res = lianad.rpc.listcoins()["coins"]
    outpoint_a = res[0]["outpoint"]
    assert txid_a == outpoint_a[:64]
    assert res[0]["amount"] == 1 * COIN
    assert res[0]["derivation_index"] == addr_a["derivation_index"]
    assert res[0]["is_change"] == False
    assert res[0]["block_height"] is None
    assert res[0]["spend_info"] is None
    assert res[0]["is_from_self"] is False

    assert len(lianad.rpc.listcoins(["confirmed", "spent", "spending"])["coins"]) == 0
    assert (
        lianad.rpc.listcoins()
        == lianad.rpc.listcoins([], [outpoint_a])
        == lianad.rpc.listcoins(["unconfirmed"])
        == lianad.rpc.listcoins(["unconfirmed"], [outpoint_a])
        == lianad.rpc.listcoins(["unconfirmed", "confirmed"])
        == lianad.rpc.listcoins(["spent", "unconfirmed", "confirmed"])
        == lianad.rpc.listcoins(["spent", "unconfirmed", "confirmed"], [outpoint_a])
    )
    # If the coin gets confirmed, it'll be marked as such.
    bitcoind.generate_block(1, wait_for_mempool=txid_a)
    block_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.listcoins()["coins"][0]["block_height"] == block_height)

    assert (
        len(lianad.rpc.listcoins())
        == len(lianad.rpc.listcoins(["confirmed"])["coins"])
        == 1
    )
    assert (
        lianad.rpc.listcoins()
        == lianad.rpc.listcoins([], [outpoint_a])
        == lianad.rpc.listcoins(["confirmed"])
        == lianad.rpc.listcoins(["confirmed"], [outpoint_a])
        == lianad.rpc.listcoins(["unconfirmed", "confirmed"])
        == lianad.rpc.listcoins(["spent", "unconfirmed", "confirmed"])
        == lianad.rpc.listcoins(["spent", "unconfirmed", "confirmed"], [outpoint_a])
    )

    assert lianad.rpc.listcoins()["coins"][0]["is_from_self"] is False

    # Same if the coin gets spent.
    spend_tx = spend_coins(lianad, bitcoind, (res[0],))
    spend_txid = get_txid(spend_tx)
    wait_for(lambda: lianad.rpc.listcoins()["coins"][0]["spend_info"] is not None)
    spend_info = lianad.rpc.listcoins()["coins"][0]["spend_info"]
    assert spend_info["txid"] == spend_txid
    assert spend_info["height"] is None
    assert len(lianad.rpc.listcoins(["spent"])["coins"]) == 0
    assert len(lianad.rpc.listcoins(["spending"])["coins"]) == 1
    assert lianad.rpc.listcoins(["spending"])["coins"][0]["is_from_self"] is False

    # And if this spending tx gets confirmed.
    bitcoind.generate_block(1, wait_for_mempool=spend_txid)
    curr_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == curr_height)
    spend_info = lianad.rpc.listcoins()["coins"][0]["spend_info"]
    assert spend_info["txid"] == spend_txid
    assert spend_info["height"] == curr_height
    assert len(lianad.rpc.listcoins(["unconfirmed", "confirmed"])["coins"]) == 0
    assert (
        lianad.rpc.listcoins()
        == lianad.rpc.listcoins(["spent"])
        == lianad.rpc.listcoins(["spent", "unconfirmed", "confirmed"])
    )
    assert len(lianad.rpc.listcoins()["coins"]) == 1
    assert lianad.rpc.listcoins()["coins"][0]["is_from_self"] is False

    # Add a second coin.
    addr_b = lianad.rpc.getnewaddress()["address"]
    txid_b = bitcoind.rpc.sendtoaddress(addr_b, 2)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 2)
    res = lianad.rpc.listcoins(["unconfirmed"], [])["coins"]
    outpoint_b = res[0]["outpoint"]
    assert res[0]["is_from_self"] is False

    # We have one unconfirmed coin and one spent coin.
    assert (
        len(lianad.rpc.listcoins()["coins"])
        == len(lianad.rpc.listcoins([], [outpoint_a, outpoint_b])["coins"])
        == len(lianad.rpc.listcoins(["unconfirmed", "spent"])["coins"])
        == len(
            lianad.rpc.listcoins(["unconfirmed", "spent"], [outpoint_a, outpoint_b])[
                "coins"
            ]
        )
        == 2
    )
    assert (
        lianad.rpc.listcoins([], [outpoint_b])
        == lianad.rpc.listcoins(["unconfirmed"])
        == lianad.rpc.listcoins(["unconfirmed"], [outpoint_b])
        == lianad.rpc.listcoins(["unconfirmed", "confirmed"])
        == lianad.rpc.listcoins(["spending", "unconfirmed", "confirmed"])
        == lianad.rpc.listcoins(["spending", "unconfirmed", "confirmed"], [outpoint_b])
    )
    assert lianad.rpc.listcoins([], [outpoint_b])["coins"][0]["is_from_self"] is False

    # Now confirm the second coin.
    bitcoind.generate_block(1, wait_for_mempool=txid_b)
    block_height = bitcoind.rpc.getblockcount()
    wait_for(
        lambda: lianad.rpc.listcoins([], [outpoint_b])["coins"][0]["block_height"]
        == block_height
    )
    assert lianad.rpc.listcoins([], [outpoint_b])["coins"][0]["is_from_self"] is False

    # We have one confirmed coin and one spent coin.
    assert (
        len(lianad.rpc.listcoins()["coins"])
        == len(lianad.rpc.listcoins([], [outpoint_a, outpoint_b])["coins"])
        == len(lianad.rpc.listcoins(["confirmed", "spent"])["coins"])
        == len(
            lianad.rpc.listcoins(["confirmed", "spent"], [outpoint_a, outpoint_b])[
                "coins"
            ]
        )
        == 2
    )
    assert (
        lianad.rpc.listcoins([], [outpoint_b])
        == lianad.rpc.listcoins(["confirmed"])
        == lianad.rpc.listcoins(["confirmed"], [outpoint_b])
        == lianad.rpc.listcoins(["unconfirmed", "confirmed"])
        == lianad.rpc.listcoins(["unconfirmed", "confirmed", "spending"])
        == lianad.rpc.listcoins(["unconfirmed", "confirmed", "spending"], [outpoint_b])
    )

    # Add a third coin.
    addr_c = lianad.rpc.getnewaddress()["address"]
    txid_c = bitcoind.rpc.sendtoaddress(addr_c, 3)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 3)
    res = lianad.rpc.listcoins(["unconfirmed"], [])["coins"]
    outpoint_c = res[0]["outpoint"]

    # We have three different statuses: unconfirmed, confirmed and spent.
    assert (
        len(lianad.rpc.listcoins()["coins"])
        == len(lianad.rpc.listcoins([], [outpoint_a, outpoint_b, outpoint_c])["coins"])
        == len(lianad.rpc.listcoins(["unconfirmed", "confirmed", "spent"])["coins"])
        == len(
            lianad.rpc.listcoins(
                ["unconfirmed", "confirmed", "spent"],
                [outpoint_a, outpoint_b, outpoint_c],
            )["coins"]
        )
        == 3
    )
    assert (
        lianad.rpc.listcoins([], [outpoint_c])
        == lianad.rpc.listcoins(["unconfirmed"])
        == lianad.rpc.listcoins(["unconfirmed"], [outpoint_c])
        == lianad.rpc.listcoins(["unconfirmed", "spending"])
        == lianad.rpc.listcoins(["spending", "unconfirmed"])
        == lianad.rpc.listcoins(["spending", "unconfirmed", "confirmed"], [outpoint_c])
    )

    # Spend third coin, even though it is still unconfirmed.
    spend_tx = spend_coins(lianad, bitcoind, (res[0],))
    spend_txid = get_txid(spend_tx)
    wait_for(
        lambda: lianad.rpc.listcoins([], [outpoint_c])["coins"][0]["spend_info"]
        is not None
    )

    assert len(lianad.rpc.listcoins(["unconfirmed"])["coins"]) == 0
    assert (
        len(lianad.rpc.listcoins()["coins"])
        == len(lianad.rpc.listcoins([], [outpoint_a, outpoint_b, outpoint_c])["coins"])
        == len(lianad.rpc.listcoins(["confirmed", "spending", "spent"])["coins"])
        == len(
            lianad.rpc.listcoins(
                ["confirmed", "spending", "spent"], [outpoint_a, outpoint_b, outpoint_c]
            )["coins"]
        )
        == 3
    )
    # The unconfirmed coin now has spending status.
    assert (
        lianad.rpc.listcoins([], [outpoint_c])
        == lianad.rpc.listcoins(["spending"])
        == lianad.rpc.listcoins(["spending"], [outpoint_c])
        == lianad.rpc.listcoins(["spending", "unconfirmed"])
        == lianad.rpc.listcoins(["spending", "unconfirmed"], [outpoint_c])
    )

    # Add a fourth coin.
    addr_d = lianad.rpc.getnewaddress()["address"]
    txid_d = bitcoind.rpc.sendtoaddress(addr_d, 4)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 4)
    res = lianad.rpc.listcoins(["unconfirmed"], [])["coins"]
    outpoint_d = res[0]["outpoint"]

    # We now have all four statuses.
    assert (
        len(lianad.rpc.listcoins(["unconfirmed"])["coins"])
        == len(lianad.rpc.listcoins(["confirmed"])["coins"])
        == len(lianad.rpc.listcoins(["spending"])["coins"])
        == len(lianad.rpc.listcoins(["spent"])["coins"])
        == 1
    )
    assert (
        len(lianad.rpc.listcoins()["coins"])
        == len(
            lianad.rpc.listcoins([], [outpoint_a, outpoint_b, outpoint_c, outpoint_d])[
                "coins"
            ]
        )
        == len(
            lianad.rpc.listcoins(["unconfirmed", "confirmed", "spending", "spent"])[
                "coins"
            ]
        )
        == len(
            lianad.rpc.listcoins(
                ["unconfirmed", "confirmed", "spending", "spent"],
                [outpoint_a, outpoint_b, outpoint_c, outpoint_d],
            )["coins"]
        )
        == 4
    )

    # We can filter for specific statuses/outpoints.
    assert (
        sorted(
            lianad.rpc.listcoins(["spending", "spent"])["coins"],
            key=lambda c: c["outpoint"],
        )
        == sorted(
            lianad.rpc.listcoins(["spending", "spent"], [outpoint_a, outpoint_c])[
                "coins"
            ],
            key=lambda c: c["outpoint"],
        )
        == sorted(
            lianad.rpc.listcoins(
                ["unconfirmed", "confirmed", "spending", "spent"],
                [outpoint_a, outpoint_c],
            )["coins"],
            key=lambda c: c["outpoint"],
        )
        == sorted(
            lianad.rpc.listcoins(
                ["spending", "spent"], [outpoint_a, outpoint_b, outpoint_c, outpoint_d]
            )["coins"],
            key=lambda c: c["outpoint"],
        )
    )

    # Finally, check that we return errors for invalid parameter values.
    for statuses, outpoints in [
        (["fake_status"], []),
        (["spent", "fake_status"], []),
        (["fake_status", "fake_status_2"], []),
        (["confirmed", "spending", "fake_status"], ["fake_outpoint"]),
        (["fake_status"], [outpoint_a, outpoint_b]),
    ]:
        with pytest.raises(
            RpcError,
            match=re.escape(
                "Invalid params: Invalid value \"fake_status\" in \\'statuses\\' parameter."
            ),
        ):
            lianad.rpc.listcoins(statuses, outpoints)

    for statuses, outpoints in [
        ([], ["fake_outpoint"]),
        ([], [outpoint_a, "fake_outpoint", outpoint_b]),
        ([], [outpoint_a, "fake_outpoint", "fake_outpoint_2"]),
        ([], [outpoint_a, outpoint_b, "fake_outpoint"]),
    ]:
        with pytest.raises(
            RpcError,
            match=re.escape(
                "Invalid params: Invalid value \"fake_outpoint\" in \\'outpoints\\' parameter."
            ),
        ):
            lianad.rpc.listcoins(statuses, outpoints)


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

    # 15 new receive addresses have been generated (starting at index 1),
    # so last used value is 15:
    assert lianad.rpc.getinfo()["receive_index"] == 15
    # For each received coin, the change index has also been updated by the poller
    # (see https://github.com/wizardsardine/liana/issues/1333), so is also 15.
    # Then `createspend` will use the next index for change and update the DB value accordingly:
    assert lianad.rpc.getinfo()["change_index"] == 16

    # The transaction must contain the spent transaction for each input for P2WSH. But not for Taproot.
    # We don't make assumptions about the ordering of PSBT inputs.
    if USE_TAPROOT:
        assert all(
            PSBT_IN_NON_WITNESS_UTXO not in psbt_in.map for psbt_in in spend_psbt.i
        )
    else:
        assert sorted(
            [psbt_in.map[PSBT_IN_NON_WITNESS_UTXO] for psbt_in in spend_psbt.i]
        ) == sorted(
            [
                bytes.fromhex(bitcoind.rpc.gettransaction(op[:64])["hex"])
                for op in outpoints
            ]
        )

    # We can sign it and broadcast it.
    sign_and_broadcast(lianad, bitcoind, PSBT.from_base64(res["psbt"]))

    # Try creating a transaction that spends an immature coinbase deposit.
    addr = lianad.rpc.getnewaddress()["address"]
    bitcoind.rpc.generatetoaddress(1, addr)
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )
    imma_coin = next(c for c in lianad.rpc.listcoins()["coins"] if c["is_immature"])
    with pytest.raises(RpcError, match=".*is from an immature coinbase transaction."):
        lianad.rpc.createspend(destinations, [imma_coin["outpoint"]], 1)


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
    time_before_update = int(time.time())
    assert len(lianad.rpc.listspendtxs()["spend_txs"]) == 0
    lianad.rpc.updatespend(res["psbt"])
    lianad.rpc.updatespend(res_b["psbt"])

    # Check 'txids' parameter
    list_res = lianad.rpc.listspendtxs()["spend_txs"]

    txid = PSBT.from_base64(list_res[0]["psbt"]).tx.txid().hex()

    filtered_res = lianad.rpc.listspendtxs(txids=[txid])
    assert filtered_res["spend_txs"][0]["psbt"] == list_res[0]["psbt"]
    assert len(filtered_res) == 1

    with pytest.raises(
        RpcError, match="Filter list is empty, should supply None instead."
    ):
        lianad.rpc.listspendtxs(txids=[])

    with pytest.raises(RpcError, match="Invalid params: Invalid 'txids' parameter."):
        lianad.rpc.listspendtxs(txids=[txid, 123])

    with pytest.raises(RpcError, match="Invalid params: Invalid 'txids' parameter."):
        lianad.rpc.listspendtxs(txids=[0])

    with pytest.raises(RpcError, match="Invalid params: Invalid 'txids' parameter."):
        lianad.rpc.listspendtxs(txids=[123])

    with pytest.raises(RpcError, match="Invalid params: Invalid 'txids' parameter."):
        lianad.rpc.listspendtxs(txids=["abc"])

    with pytest.raises(RpcError, match="Invalid params: Invalid 'txids' parameter."):
        lianad.rpc.listspendtxs(txids=["123"])

    # Listing all Spend transactions will list them both. It'll tell us which one has
    # change and which one doesn't.
    list_res = lianad.rpc.listspendtxs()["spend_txs"]
    assert len(list_res) == 2
    first_psbt = next(entry for entry in list_res if entry["psbt"] == res["psbt"])
    assert time_before_update <= first_psbt["updated_at"] <= int(time.time())
    second_psbt = next(entry for entry in list_res if entry["psbt"] == res_b["psbt"])
    assert time_before_update <= second_psbt["updated_at"] <= int(time.time())

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
    signed_psbt = lianad.signer.sign_psbt(PSBT.from_base64(res["psbt"]))
    lianad.rpc.updatespend(signed_psbt.to_base64())

    # Now we've signed and stored it, the daemon will take care of finalizing
    # the PSBT before broadcasting the transaction.
    lianad.rpc.broadcastspend(txid)


# Use a descriptor that includes hardened derivation paths so that we can check
# there is no problem regarding the use of `h` and `'`.
def test_start_rescan_does_not_error(lianad_with_deriv_paths, bitcoind):
    """Test we can successfully start a rescan."""
    tip_timestamp = bitcoind.rpc.getblockheader(bitcoind.rpc.getbestblockhash())["time"]
    lianad_with_deriv_paths.rpc.startrescan(tip_timestamp - 1)


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
    block_hash = bitcoind.rpc.getblockhash(0)
    genesis_timestamp = bitcoind.rpc.getblock(block_hash)["time"]
    prebitcoin_timestamp = genesis_timestamp - 1
    with pytest.raises(RpcError, match="Insane timestamp."):
        lianad.rpc.startrescan(prebitcoin_timestamp)
    assert lianad.rpc.getinfo()["rescan_progress"] is None
    # we can rescan from genesis block
    lianad.rpc.startrescan(genesis_timestamp)
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)

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
    if BITCOIN_BACKEND_TYPE is BitcoinBackendType.Bitcoind:
        assert len(list_coins()) == 0

    # The wallet isn't aware what derivation indexes were used. Necessarily it'll start
    # from 0.
    if BITCOIN_BACKEND_TYPE is BitcoinBackendType.Bitcoind:
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
    txid = sign_and_broadcast_psbt(lianad, psbt)
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
    txid = sign_and_broadcast_psbt(lianad, psbt)
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
    txid = sign_and_broadcast_psbt(lianad, psbt)

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


def test_create_recovery(lianad, bitcoind):
    """Test the sweep of coins that are available through the timelocked path."""
    # Generate blocks in order to test locktime set correctly.
    bitcoind.generate_block(200)
    # Start by getting a few coins
    destinations = {
        lianad.rpc.getnewaddress()["address"]: 0.1,
        lianad.rpc.getnewaddress()["address"]: 0.2,
        lianad.rpc.getnewaddress()["address"]: 0.3,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )
    first_outpoints = [c["outpoint"] for c in lianad.rpc.listcoins()["coins"]]

    # There's nothing to sweep
    with pytest.raises(
        RpcError,
        match="No coin currently spendable through this timelocked recovery path",
    ):
        lianad.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)
    # Same if we specify timelock:
    with pytest.raises(
        RpcError,
        match="No coin currently spendable through this timelocked recovery path",
    ):
        lianad.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2, 10)
    # And if we use empty array for outpoints:
    with pytest.raises(
        RpcError,
        match="No coin currently spendable through this timelocked recovery path",
    ):
        lianad.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2, 10, [])
    # If we specify a coin, the error will be different:
    with pytest.raises(
        RpcError,
        match=f"Coin at '{first_outpoints[0]}' is not recoverable with timelock '10'",
    ):
        lianad.rpc.createrecovery(
            bitcoind.rpc.getnewaddress(), 2, 10, [f"{first_outpoints[0]}"]
        )

    # Receive another coin, it will be one block after the others
    txid = bitcoind.rpc.sendtoaddress(lianad.rpc.getnewaddress()["address"], 0.4)

    # Make the timelock of the 3 first coins mature (we use a csv of 10 in the fixture)
    bitcoind.generate_block(9, wait_for_mempool=txid)

    # Now we can create a recovery tx that sweeps the first 3 coins.
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )
    new_outpoint = [
        c["outpoint"] for c in lianad.rpc.listcoins()["coins"] if txid in c["outpoint"]
    ][0]
    reco_address = bitcoind.rpc.getnewaddress()
    res = lianad.rpc.createrecovery(reco_address, 18)
    reco_psbt = PSBT.from_base64(res["psbt"])

    # Do the same passing all three coins explicitly:
    res_op = lianad.rpc.createrecovery(reco_address, 18, 10, first_outpoints)
    reco_psbt_op = PSBT.from_base64(res_op["psbt"])

    # Check locktime being set correctly.
    tip_height = bitcoind.rpc.getblockcount()
    assert tip_height > 100
    locktime = reco_psbt.tx.nLockTime
    assert tip_height - 100 <= locktime <= tip_height

    assert len(reco_psbt.tx.vin) == 3, "The last coin's timelock hasn't matured yet"
    assert len(reco_psbt.tx.vout) == len(reco_psbt_op.tx.vout) == 1
    # The inputs are the same for both explicit and implicit outpoints:
    assert sorted(i.prevout.serialize() for i in reco_psbt.tx.vin) == sorted(
        i.prevout.serialize() for i in reco_psbt_op.tx.vin
    )
    assert reco_psbt.tx.vout[0].nValue == reco_psbt_op.tx.vout[0].nValue
    assert reco_psbt.tx.vout[0].scriptPubKey == reco_psbt_op.tx.vout[0].scriptPubKey
    assert int(0.5999 * COIN) < int(reco_psbt.tx.vout[0].nValue) < int(0.6 * COIN)

    # Now use only 2 of the 3 coins:
    res_op_2 = lianad.rpc.createrecovery(
        bitcoind.rpc.getnewaddress(), 18, 10, first_outpoints[:2]
    )
    reco_psbt_op_2 = PSBT.from_base64(res_op_2["psbt"])
    assert len(reco_psbt_op_2.tx.vin) == 2
    assert sorted(
        f"{i.prevout.hash:064x}:{i.prevout.n}" for i in reco_psbt_op_2.tx.vin
    ) == sorted(first_outpoints[:2])

    # If we try to include the newest coin, an error will be returned:
    with pytest.raises(
        RpcError,
        match=f"Coin at '{new_outpoint}' is not recoverable with timelock '10'",
    ):
        lianad.rpc.createrecovery(
            bitcoind.rpc.getnewaddress(), 2, 10, [first_outpoints[0], new_outpoint]
        )

    txid = sign_and_broadcast(lianad, bitcoind, reco_psbt, recovery=True)

    # And by mining one more block we'll be able to sweep the last coin.
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )
    res = lianad.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 1)
    reco_psbt = PSBT.from_base64(res["psbt"])
    assert len(reco_psbt.tx.vin) == 1
    assert len(reco_psbt.tx.vout) == 1
    assert int(0.39999 * COIN) < int(reco_psbt.tx.vout[0].nValue) < int(0.4 * COIN)
    sign_and_broadcast(lianad, bitcoind, reco_psbt, recovery=True)


def test_labels(lianad, bitcoind):
    """Test the creation and updating of labels."""
    # We can set a label for an address.
    addr = lianad.rpc.getnewaddress()["address"]
    lianad.rpc.updatelabels({addr: "first-addr"})
    assert lianad.rpc.getlabels([addr])["labels"] == {addr: "first-addr"}
    # And also update it.
    lianad.rpc.updatelabels({addr: "first-addr-1"})
    assert lianad.rpc.getlabels([addr])["labels"] == {addr: "first-addr-1"}
    # But we can't set a label larger than 100 characters
    with pytest.raises(RpcError, match=".*must be less or equal than 100 characters"):
        lianad.rpc.updatelabels({addr: "".join("a" for _ in range(101))})

    # We can set a label for a coin.
    sec_addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(sec_addr, 1)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    coin = lianad.rpc.listcoins()["coins"][0]
    lianad.rpc.updatelabels({coin["outpoint"]: "first-coin"})
    assert lianad.rpc.getlabels([coin["outpoint"]])["labels"] == {
        coin["outpoint"]: "first-coin"
    }
    # And also update it.
    lianad.rpc.updatelabels({coin["outpoint"]: "first-coin-1"})
    assert lianad.rpc.getlabels([coin["outpoint"]])["labels"] == {
        coin["outpoint"]: "first-coin-1"
    }
    # Its address though has no label.
    assert lianad.rpc.getlabels([sec_addr])["labels"] == {}
    # But we can receive a coin to the address that has a label set, and query both.
    sec_txid = bitcoind.rpc.sendtoaddress(addr, 1)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 2)
    sec_coin = next(
        c for c in lianad.rpc.listcoins()["coins"] if sec_txid in c["outpoint"]
    )
    lianad.rpc.updatelabels({sec_coin["outpoint"]: "sec-coin"})
    res = lianad.rpc.getlabels([sec_coin["outpoint"], addr])["labels"]
    assert len(res) == 2
    assert res[sec_coin["outpoint"]] == "sec-coin"
    assert res[addr] == "first-addr-1"
    # We can also query the labels for both coins, of course.
    res = lianad.rpc.getlabels([coin["outpoint"], sec_coin["outpoint"]])["labels"]
    assert len(res) == 2
    assert res[coin["outpoint"]] == "first-coin-1"
    assert res[sec_coin["outpoint"]] == "sec-coin"

    # We can set, update and query labels for deposit transactions.
    lianad.rpc.updatelabels({txid: "first-deposit"})
    assert lianad.rpc.getlabels([txid, sec_txid])["labels"] == {txid: "first-deposit"}
    lianad.rpc.updatelabels({txid: "first-deposit-1", sec_txid: "second-deposit"})
    res = lianad.rpc.getlabels([txid, sec_txid])["labels"]
    assert len(res) == 2
    assert res[txid] == "first-deposit-1"
    assert res[sec_txid] == "second-deposit"

    # We can set and update a label for a spend transaction.
    spend_txid = get_txid(spend_coins(lianad, bitcoind, [coin, sec_coin]))
    lianad.rpc.updatelabels({spend_txid: "spend-tx"})
    assert lianad.rpc.getlabels([spend_txid])["labels"] == {spend_txid: "spend-tx"}
    lianad.rpc.updatelabels({spend_txid: "spend-tx-1"})
    assert lianad.rpc.getlabels([spend_txid])["labels"] == {spend_txid: "spend-tx-1"}

    # We can set labels for inexistent stuff, as long as the format of the item being
    # labelled is valid.
    inexistent_txid = "".join("0" for _ in range(64))
    inexistent_outpoint = "".join("1" for _ in range(64)) + ":42"
    random_address = bitcoind.rpc.getnewaddress()
    lianad.rpc.updatelabels(
        {
            inexistent_txid: "inex_txid",
            inexistent_outpoint: "inex_outpoint",
            random_address: "bitcoind-addr",
        }
    )
    res = lianad.rpc.getlabels([inexistent_txid, inexistent_outpoint, random_address])[
        "labels"
    ]
    assert len(res) == 3
    assert res[inexistent_txid] == "inex_txid"
    assert res[inexistent_outpoint] == "inex_outpoint"
    assert res[random_address] == "bitcoind-addr"

    # We'll confirm everything, shouldn't affect any of the labels.
    bitcoind.generate_block(1, wait_for_mempool=spend_txid)
    wait_for(
        lambda: bitcoind.rpc.getblockcount() == lianad.rpc.getinfo()["block_height"]
    )
    res = lianad.rpc.getlabels(
        [
            addr,
            sec_addr,  # No label for this one.
            txid,
            sec_txid,
            coin["outpoint"],
            sec_coin["outpoint"],
            spend_txid,
            inexistent_txid,
            inexistent_outpoint,
            random_address,
        ]
    )["labels"]
    assert len(res) == 9
    assert res[sec_coin["outpoint"]] == "sec-coin"
    assert res[addr] == "first-addr-1"
    assert res[coin["outpoint"]] == "first-coin-1"
    assert res[txid] == "first-deposit-1"
    assert res[sec_txid] == "second-deposit"
    assert res[spend_txid] == "spend-tx-1"
    assert res[inexistent_txid] == "inex_txid"
    assert res[inexistent_outpoint] == "inex_outpoint"
    assert res[random_address] == "bitcoind-addr"

    # Delete 2 of the labels set above. They shouldn't be returned anymore.
    lianad.rpc.updatelabels(
        {
            addr: None,
            sec_addr: None,
            random_address: "this address is random",
        }
    )
    res = lianad.rpc.getlabels([addr, sec_addr, random_address])["labels"]
    assert len(res) == 1
    assert addr not in res
    assert sec_addr not in res
    assert res[random_address] == "this address is random"


def test_labels_bip329(lianad, bitcoind):
    # Label 5 addresses
    addresses = []
    for i in range(0, 5):
        addr = lianad.rpc.getnewaddress()["address"]
        addresses.append(addr)
        lianad.rpc.updatelabels({addr: f"addr{i}"})

    # Label 5 coin
    txids = []
    for i in range(0, 5):
        addr = lianad.rpc.getnewaddress()["address"]
        txid = bitcoind.rpc.sendtoaddress(addr, 1)
        txids.append(txid)
        wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == i + 1)

    coins = lianad.rpc.listcoins()["coins"]
    for i in range(0, 5):
        coin = coins[i]
        lianad.rpc.updatelabels({coin["outpoint"]: f"coin{i}"})

    # Label 5 transactions
    for i, txid in enumerate(txids):
        lianad.rpc.updatelabels({txid: f"tx{i}"})

    # Get Bip-0329 labels
    bip329_labels = lianad.rpc.getlabelsbip329(0, 100)["labels"]
    assert len(bip329_labels) == 15

    def label_found(name, labels):
        for label in labels:
            if label["label"] == name:
                return True
        return False

    # All transactions are labelled
    for i in range(0, len(txids)):
        assert label_found(f"tx{i}", bip329_labels)

    # All adresses are labelled
    for i in range(0, len(addresses)):
        assert label_found(f"addr{i}", bip329_labels)

    # All coins are labelled
    for i in range(0, len(coins)):
        assert label_found(f"coin{i}", bip329_labels)

    # There is no conflict between batches
    batch1 = lianad.rpc.getlabelsbip329(0, 5)["labels"]
    assert len(batch1) == 5

    batch2 = lianad.rpc.getlabelsbip329(5, 5)["labels"]
    assert len(batch2) == 5

    batch3 = lianad.rpc.getlabelsbip329(10, 5)["labels"]
    assert len(batch3) == 5

    for label in batch1:
        print(label)
        name = label["label"]

        assert not label_found(name, batch2)
        assert not label_found(name, batch3)

    for label in batch2:
        name = label["label"]
        assert not label_found(name, batch1)
        assert not label_found(name, batch3)


def test_rbfpsbt_bump_fee(lianad, bitcoind):
    """Test the use of RBF to bump the fee of a transaction."""

    # Generate blocks in order to test locktime set correctly.
    bitcoind.generate_block(200)
    # Get three coins.
    destinations = {
        lianad.rpc.getnewaddress()["address"]: 0.003,
        lianad.rpc.getnewaddress()["address"]: 0.004,
        lianad.rpc.getnewaddress()["address"]: 0.005,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 3)
    coins = lianad.rpc.listcoins(["confirmed"])["coins"]
    assert all(c["is_from_self"] is False for c in coins)

    # Create a spend that will later be replaced.
    first_outpoints = [c["outpoint"] for c in coins[:2]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 650_000,
    }
    first_res = lianad.rpc.createspend(destinations, first_outpoints, 1)
    first_psbt = PSBT.from_base64(first_res["psbt"])
    # The transaction has a change output.
    assert len(first_psbt.o) == len(first_psbt.tx.vout) == 2
    first_txid = first_psbt.tx.txid().hex()
    # We must provide a valid feerate.
    for bad_feerate in [-1, "foo", 18_446_744_073_709_551_616]:
        with pytest.raises(RpcError, match=f"Invalid 'feerate' parameter."):
            lianad.rpc.rbfpsbt(first_txid, False, bad_feerate)
    # We cannot RBF yet as first PSBT has not been saved.
    with pytest.raises(RpcError, match=f"Unknown spend transaction '{first_txid}'."):
        lianad.rpc.rbfpsbt(first_txid, False, 1)
    # Now save the PSBT.
    lianad.rpc.updatespend(first_res["psbt"])
    # The RBF command succeeds even if transaction has not been signed.
    lianad.rpc.rbfpsbt(first_txid, False, 2)
    # The RBF command also succeeds if transaction has been signed but not broadcast.
    first_psbt = lianad.signer.sign_psbt(first_psbt)
    lianad.rpc.updatespend(first_psbt.to_base64())
    lianad.rpc.rbfpsbt(first_txid, False, 2)
    # Now broadcast the spend and wait for it to be detected.
    lianad.rpc.broadcastspend(first_txid)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == first_txid
            for c in lianad.rpc.listcoins([], first_outpoints)["coins"]
        )
    )
    # The change output is from self as its parent is confirmed.
    lc_res = lianad.rpc.listcoins(["unconfirmed"], [])["coins"]
    assert (
        len(lc_res) == 1
        and first_txid in lc_res[0]["outpoint"]
        and lc_res[0]["is_from_self"] is True
    )
    # We can now use RBF, but the feerate must be higher than that of the first transaction.
    with pytest.raises(RpcError, match=f"Feerate 1 too low for minimum feerate 2."):
        lianad.rpc.rbfpsbt(first_txid, False, 1)
    # Using a higher feerate works.
    lianad.rpc.rbfpsbt(first_txid, False, 2)

    # We can still use RBF if the PSBT is no longer in the DB.
    lianad.rpc.delspendtx(first_txid)
    lianad.rpc.rbfpsbt(first_txid, False, 2)

    # Let's use an even higher feerate.
    rbf_1_res = lianad.rpc.rbfpsbt(first_txid, False, 10)
    rbf_1_psbt = PSBT.from_base64(rbf_1_res["psbt"])

    # Check the locktime is being set.
    tip_height = bitcoind.rpc.getblockcount()
    locktime = rbf_1_psbt.tx.nLockTime
    assert tip_height - 100 <= locktime <= tip_height

    # The inputs are the same in both (no new inputs needed in the replacement).
    assert sorted(i.prevout.serialize() for i in first_psbt.tx.vin) == sorted(
        i.prevout.serialize() for i in rbf_1_psbt.tx.vin
    )
    # Check non-change output is the same in both.
    assert first_psbt.tx.vout[0].nValue == rbf_1_psbt.tx.vout[0].nValue
    assert first_psbt.tx.vout[0].scriptPubKey == rbf_1_psbt.tx.vout[0].scriptPubKey
    # Change address is the same but change amount will be lower in the replacement to pay higher fee.
    assert first_psbt.tx.vout[1].nValue > rbf_1_psbt.tx.vout[1].nValue
    assert first_psbt.tx.vout[1].scriptPubKey == rbf_1_psbt.tx.vout[1].scriptPubKey
    # Broadcast the replacement and wait for it to be detected.
    rbf_1_txid = sign_and_broadcast_psbt(lianad, rbf_1_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == rbf_1_txid
            for c in lianad.rpc.listcoins([], first_outpoints)["coins"]
        )
    )
    # The change output of the replacement is also from self as its parent is confirmed.
    lc_res = lianad.rpc.listcoins(["unconfirmed"], [])["coins"]
    assert (
        len(lc_res) == 1
        and rbf_1_txid in lc_res[0]["outpoint"]
        and lc_res[0]["is_from_self"] is True
    )
    mempool_rbf_1 = bitcoind.rpc.getmempoolentry(rbf_1_txid)
    # Note that in the mempool entry, "ancestor" includes rbf_1_txid itself.
    rbf_1_feerate = (
        mempool_rbf_1["fees"]["ancestor"] * COIN / mempool_rbf_1["ancestorsize"]
    )
    assert 10 <= rbf_1_feerate < 10.25
    # If we try to RBF the first transaction again, it will not be possible as we
    # deleted the PSBT above and the tx is no longer part of our wallet's
    # spending txs (even though it's saved in the DB).
    with pytest.raises(RpcError, match=f"Unknown spend transaction '{first_txid}'."):
        lianad.rpc.rbfpsbt(first_txid, False, 2)
    # If we resave the PSBT, then we can use RBF and it will use the first RBF's
    # feerate to set the min feerate, instead of 1 sat/vb of the first transaction:
    lianad.rpc.updatespend(first_psbt.to_base64())
    with pytest.raises(
        RpcError,
        match=f"Feerate {int(rbf_1_feerate)} too low for minimum feerate {int(rbf_1_feerate) + 1}.",
    ):
        lianad.rpc.rbfpsbt(first_txid, False, int(rbf_1_feerate))
    # Using 1 more for feerate works.
    feerate = int(rbf_1_feerate) + 1
    lianad.rpc.rbfpsbt(first_txid, False, feerate)
    # Add a new transaction spending the change from the first RBF.
    desc_1_destinations = {
        bitcoind.rpc.getnewaddress(): 500_000,
    }
    desc_1_outpoints = [f"{rbf_1_txid}:1", coins[2]["outpoint"]]
    wait_for(lambda: len(lianad.rpc.listcoins([], desc_1_outpoints)["coins"]) == 2)
    desc_1_res = lianad.rpc.createspend(desc_1_destinations, desc_1_outpoints, 1)
    desc_1_psbt = PSBT.from_base64(desc_1_res["psbt"])
    assert len(desc_1_psbt.tx.vout) == 2
    desc_1_txid = sign_and_broadcast_psbt(lianad, desc_1_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == desc_1_txid
            for c in lianad.rpc.listcoins([], desc_1_outpoints)["coins"]
        )
    )
    lc_res = [c for c in lianad.rpc.listcoins(["unconfirmed"], [])["coins"]]
    assert (
        len(lc_res) == 1
        and desc_1_txid in lc_res[0]["outpoint"]
        and lc_res[0]["is_from_self"] is True
    )
    # Add a new transaction spending the change from the first descendant.
    desc_2_destinations = {
        bitcoind.rpc.getnewaddress(): 25_000,
    }
    desc_2_outpoints = [f"{desc_1_txid}:1"]
    wait_for(lambda: len(lianad.rpc.listcoins([], desc_2_outpoints)["coins"]) == 1)
    desc_2_res = lianad.rpc.createspend(desc_2_destinations, desc_2_outpoints, 1)
    desc_2_psbt = PSBT.from_base64(desc_2_res["psbt"])
    assert len(desc_2_psbt.tx.vout) == 2
    desc_2_txid = sign_and_broadcast_psbt(lianad, desc_2_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == desc_2_txid
            for c in lianad.rpc.listcoins([], desc_2_outpoints)["coins"]
        )
    )
    lc_res = [c for c in lianad.rpc.listcoins(["unconfirmed"], [])["coins"]]
    assert (
        len(lc_res) == 1
        and desc_2_txid in lc_res[0]["outpoint"]
        and lc_res[0]["is_from_self"] is True
    )
    # Now replace the first RBF, which will also remove its descendants.
    rbf_2_res = lianad.rpc.rbfpsbt(rbf_1_txid, False, feerate)
    rbf_2_psbt = PSBT.from_base64(rbf_2_res["psbt"])
    # The inputs are the same in both (no new inputs needed in the replacement).
    assert sorted(i.prevout.serialize() for i in rbf_1_psbt.tx.vin) == sorted(
        i.prevout.serialize() for i in rbf_2_psbt.tx.vin
    )
    # Check non-change output is the same in both.
    assert rbf_1_psbt.tx.vout[0].nValue == rbf_2_psbt.tx.vout[0].nValue
    assert rbf_1_psbt.tx.vout[0].scriptPubKey == rbf_2_psbt.tx.vout[0].scriptPubKey
    # Change address is the same but change amount will be lower in the replacement to pay higher fee.
    assert rbf_1_psbt.tx.vout[1].nValue > rbf_2_psbt.tx.vout[1].nValue
    assert rbf_1_psbt.tx.vout[1].scriptPubKey == rbf_2_psbt.tx.vout[1].scriptPubKey

    # Broadcast the replacement and wait for it to be detected.
    rbf_2_txid = sign_and_broadcast_psbt(lianad, rbf_2_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == rbf_2_txid
            for c in lianad.rpc.listcoins([], first_outpoints)["coins"]
        )
    )
    lc_res = [c for c in lianad.rpc.listcoins(["unconfirmed"], [])["coins"]]
    assert (
        len(lc_res) == 1
        and rbf_2_txid in lc_res[0]["outpoint"]
        and lc_res[0]["is_from_self"] is True
    )
    # The unconfirmed coins used in the descendant transactions have been removed so that
    # only one of the input coins remains, and its spend info has been wiped so that it is as before.
    assert lianad.rpc.listcoins([], desc_1_outpoints + desc_2_outpoints)["coins"] == [
        coins[2]
    ]
    # Now confirm the replacement transaction.
    bitcoind.generate_block(1, wait_for_mempool=rbf_2_txid)
    wait_for(
        lambda: all(
            c["spend_info"]["txid"] == rbf_2_txid
            and c["spend_info"]["height"] is not None
            for c in lianad.rpc.listcoins([], first_outpoints)["coins"]
        )
    )
    final_coins = lianad.rpc.listcoins()["coins"]
    # We have the three original coins plus the change output from the last RBF.
    assert len(final_coins) == 4
    assert len(coins + lc_res) == 4
    for fc in final_coins:
        assert fc["outpoint"] in [c["outpoint"] for c in coins + lc_res]
        # Original coins are not from self, but RBF change output is.
        assert fc["is_from_self"] is (fc["outpoint"] in [c["outpoint"] for c in lc_res])


def test_rbfpsbt_insufficient_funds(lianad, bitcoind):
    """Trying to increase the fee too much returns the missing funds amount."""
    # Get a coin.
    deposit_txid_1 = bitcoind.rpc.sendtoaddress(
        lianad.rpc.getnewaddress()["address"], 30_000 / COIN
    )
    bitcoind.generate_block(1, wait_for_mempool=deposit_txid_1)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 1)

    # Create a spend that we will then attempt to replace.
    destinations_1 = {
        bitcoind.rpc.getnewaddress(): 29_800,
    }
    spend_res_1 = lianad.rpc.createspend(destinations_1, [], 1)
    spend_psbt_1 = PSBT.from_base64(spend_res_1["psbt"])
    spend_txid_1 = sign_and_broadcast_psbt(lianad, spend_psbt_1)

    # We don't have sufficient funds to bump the fee.
    feerate = 3 if USE_TAPROOT else 2
    assert "missing" in lianad.rpc.rbfpsbt(spend_txid_1, False, feerate)
    # We can still cancel it as the coin has enough value to create a single
    # output at a higher feerate.
    assert "psbt" in lianad.rpc.rbfpsbt(spend_txid_1, True)

    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 0)
    # Get another coin.
    deposit_txid_2 = bitcoind.rpc.sendtoaddress(
        lianad.rpc.getnewaddress()["address"], 5_200 / COIN
    )
    bitcoind.generate_block(1, wait_for_mempool=deposit_txid_2)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 1)

    # Create a spend that we will then attempt to cancel.
    destinations_2 = {
        bitcoind.rpc.getnewaddress(): 5_000,
    }
    spend_res_2 = lianad.rpc.createspend(destinations_2, [], 1)
    spend_psbt_2 = PSBT.from_base64(spend_res_2["psbt"])
    spend_txid_2 = sign_and_broadcast_psbt(lianad, spend_psbt_2)

    # We don't have enough to create a transaction with feerate 2 sat/vb.
    assert "missing" in lianad.rpc.rbfpsbt(spend_txid_2, True)


def test_rbfpsbt_cancel(lianad, bitcoind):
    """Test the use of RBF to cancel a transaction."""

    # Get three coins.
    destinations = {
        lianad.rpc.getnewaddress()["address"]: 0.003,
        lianad.rpc.getnewaddress()["address"]: 0.004,
        lianad.rpc.getnewaddress()["address"]: 0.005,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 3)
    coins = lianad.rpc.listcoins(["confirmed"])["coins"]

    # Create a spend that will later be replaced.
    first_outpoints = [c["outpoint"] for c in coins[:2]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 650_000,
    }
    first_res = lianad.rpc.createspend(destinations, first_outpoints, 1)
    first_psbt = PSBT.from_base64(first_res["psbt"])
    # The transaction has a change output.
    assert len(first_psbt.o) == len(first_psbt.tx.vout) == 2
    first_txid = first_psbt.tx.txid().hex()
    # Broadcast the spend and wait for it to be detected.
    first_txid = sign_and_broadcast_psbt(lianad, first_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == first_txid
            for c in lianad.rpc.listcoins([], first_outpoints)["coins"]
        )
    )
    # We can use RBF and let the command choose the min possible feerate (1 larger than previous).
    rbf_1_res = lianad.rpc.rbfpsbt(first_txid, True)
    # But we can't set the feerate explicitly.
    with pytest.raises(
        RpcError,
        match=re.escape("A feerate must not be provided if creating a cancel."),
    ):
        rbf_1_res = lianad.rpc.rbfpsbt(first_txid, True, 2)
    rbf_1_psbt = PSBT.from_base64(rbf_1_res["psbt"])
    # Replacement only has a single input.
    assert len(rbf_1_psbt.i) == 1
    # This input is one of the two from the previous transaction.
    assert rbf_1_psbt.tx.vin[0].prevout.serialize() in [
        i.prevout.serialize() for i in first_psbt.tx.vin
    ]
    # The replacement only has a change output.
    assert len(rbf_1_psbt.tx.vout) == 1
    # Change address is the same but change amount will be higher in the replacement as it is the only output.
    assert first_psbt.tx.vout[1].nValue < rbf_1_psbt.tx.vout[0].nValue
    assert first_psbt.tx.vout[1].scriptPubKey == rbf_1_psbt.tx.vout[0].scriptPubKey
    # Broadcast the replacement and wait for it to be detected.
    rbf_1_txid = sign_and_broadcast_psbt(lianad, rbf_1_psbt)
    # The spend info of the coin used in the replacement will be updated.

    rbf_1_outpoint = (
        f"{rbf_1_psbt.tx.vin[0].prevout.hash:064x}:{rbf_1_psbt.tx.vin[0].prevout.n}"
    )
    assert rbf_1_outpoint in first_outpoints

    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == rbf_1_txid
            for c in lianad.rpc.listcoins([], [rbf_1_outpoint])["coins"]
        )
    )
    # The other coin will have its spend info removed.
    wait_for(
        lambda: all(
            c["spend_info"] is None
            for c in lianad.rpc.listcoins(
                [], [op for op in first_outpoints if op != rbf_1_outpoint]
            )["coins"]
        )
    )
    # Add a new transaction spending the only output (change) from the first RBF.
    desc_1_destinations = {
        bitcoind.rpc.getnewaddress(): 500_000,
    }
    desc_1_outpoints = [f"{rbf_1_txid}:0", coins[2]["outpoint"]]
    wait_for(lambda: len(lianad.rpc.listcoins([], desc_1_outpoints)["coins"]) == 2)
    desc_1_res = lianad.rpc.createspend(desc_1_destinations, desc_1_outpoints, 1)
    desc_1_psbt = PSBT.from_base64(desc_1_res["psbt"])
    assert len(desc_1_psbt.tx.vout) == 2
    desc_1_txid = sign_and_broadcast_psbt(lianad, desc_1_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == desc_1_txid
            for c in lianad.rpc.listcoins([], desc_1_outpoints)["coins"]
        )
    )
    # Add a new transaction spending the change from the first descendant.
    desc_2_destinations = {
        bitcoind.rpc.getnewaddress(): 25_000,
    }
    desc_2_outpoints = [f"{desc_1_txid}:1"]
    wait_for(lambda: len(lianad.rpc.listcoins([], desc_2_outpoints)["coins"]) == 1)
    desc_2_res = lianad.rpc.createspend(desc_2_destinations, desc_2_outpoints, 1)
    desc_2_psbt = PSBT.from_base64(desc_2_res["psbt"])
    assert len(desc_2_psbt.tx.vout) == 2
    desc_2_txid = sign_and_broadcast_psbt(lianad, desc_2_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == desc_2_txid
            for c in lianad.rpc.listcoins([], desc_2_outpoints)["coins"]
        )
    )
    # Now cancel the first RBF, which will also remove its descendants.
    rbf_2_res = lianad.rpc.rbfpsbt(rbf_1_txid, True)
    rbf_2_psbt = PSBT.from_base64(rbf_2_res["psbt"])
    # The inputs are the same in both (no new inputs needed in the replacement).
    assert len(rbf_2_psbt.tx.vin) == 1
    assert (
        rbf_1_psbt.tx.vin[0].prevout.serialize()
        == rbf_2_psbt.tx.vin[0].prevout.serialize()
    )

    # Only a single output (change) in the replacement.
    assert len(rbf_2_psbt.tx.vout) == 1
    # Change address is the same but change amount will be lower in the replacement to pay higher fee.
    assert rbf_1_psbt.tx.vout[0].nValue > rbf_2_psbt.tx.vout[0].nValue
    assert rbf_1_psbt.tx.vout[0].scriptPubKey == rbf_2_psbt.tx.vout[0].scriptPubKey

    # Broadcast the replacement and wait for it to be detected.
    rbf_2_txid = sign_and_broadcast_psbt(lianad, rbf_2_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == rbf_2_txid
            for c in lianad.rpc.listcoins([], [rbf_1_outpoint])["coins"]
        )
    )
    # The unconfirmed coins used in the descendant transactions have been removed so that
    # only one of the input coins remains, and its spend info has been wiped so that it is as before.
    assert lianad.rpc.listcoins([], desc_1_outpoints + desc_2_outpoints)["coins"] == [
        coins[2]
    ]
    # Now confirm the replacement transaction.
    bitcoind.generate_block(1, wait_for_mempool=rbf_2_txid)
    wait_for(
        lambda: all(
            c["spend_info"]["txid"] == rbf_2_txid
            and c["spend_info"]["height"] is not None
            for c in lianad.rpc.listcoins([], [rbf_1_outpoint])["coins"]
        )
    )
