import pytest
import random
import re
import time

from fixtures import *
from test_framework.serializations import (
    PSBT,
    PSBT_IN_BIP32_DERIVATION,
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
)


def test_getinfo(lianad):
    res = lianad.rpc.getinfo()
    assert res["version"] == "2.0.0-dev"
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
    addr_a = lianad.rpc.getnewaddress()["address"]
    txid_a = bitcoind.rpc.sendtoaddress(addr_a, 1)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    res = lianad.rpc.listcoins()["coins"]
    outpoint_a = res[0]["outpoint"]
    assert txid_a == outpoint_a[:64]
    assert res[0]["amount"] == 1 * COIN
    assert res[0]["block_height"] is None
    assert res[0]["spend_info"] is None

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

    # Same if the coin gets spent.
    spend_tx = spend_coins(lianad, bitcoind, (res[0],))
    spend_txid = get_txid(spend_tx)
    wait_for(lambda: lianad.rpc.listcoins()["coins"][0]["spend_info"] is not None)
    spend_info = lianad.rpc.listcoins()["coins"][0]["spend_info"]
    assert spend_info["txid"] == spend_txid
    assert spend_info["height"] is None
    assert len(lianad.rpc.listcoins(["spent"])["coins"]) == 0
    assert len(lianad.rpc.listcoins(["spending"])["coins"]) == 1

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

    # Add a second coin.
    addr_b = lianad.rpc.getnewaddress()["address"]
    txid_b = bitcoind.rpc.sendtoaddress(addr_b, 2)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 2)
    res = lianad.rpc.listcoins(["unconfirmed"], [])["coins"]
    outpoint_b = res[0]["outpoint"]

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

    # Now confirm the second coin.
    bitcoind.generate_block(1, wait_for_mempool=txid_b)
    block_height = bitcoind.rpc.getblockcount()
    wait_for(
        lambda: lianad.rpc.listcoins([], [outpoint_b])["coins"][0]["block_height"]
        == block_height
    )

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

    # The transaction must contain the spent transaction for each input
    spent_txs = [bitcoind.rpc.gettransaction(op[:64]) for op in outpoints]
    for i, psbt_in in enumerate(spend_psbt.i):
        assert psbt_in.map[PSBT_IN_NON_WITNESS_UTXO] == bytes.fromhex(
            spent_txs[i]["hex"]
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

    # Receive a coin and make it immediately available for the recovery path.
    txid = bitcoind.rpc.sendtoaddress(lianad.rpc.getnewaddress()["address"], 1)
    bitcoind.generate_block(10, wait_for_mempool=txid)
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )

    # Create both a spend transaction and recovery transaction spending this coin.
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins(["confirmed"])["coins"]]
    res_spend = lianad.rpc.createspend(destinations, outpoints, 1)
    res_reco = lianad.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)

    # The two PSBTs don't share any BIP32 derivation paths in their inputs.
    res_spend_psbt = PSBT.from_base64(res_spend["psbt"])
    res_spend_keys = set()
    for i in res_spend_psbt.i:
        res_spend_keys = res_spend_keys | set(i.map[PSBT_IN_BIP32_DERIVATION])
    res_reco_psbt = PSBT.from_base64(res_reco["psbt"])
    res_reco_keys = set()
    for i in res_reco_psbt.i:
        res_reco_keys = res_reco_keys | set(i.map[PSBT_IN_BIP32_DERIVATION])
    assert res_spend_keys.intersection(res_reco_keys) == set()


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
        psbt = lianad.signer.sign_psbt(psbt)
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


def test_create_recovery(lianad, bitcoind):
    """Test the sweep of coins that are available through the timelocked path."""
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

    # There's nothing to sweep
    with pytest.raises(
        RpcError,
        match="No coin currently spendable through this timelocked recovery path",
    ):
        lianad.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)

    # Receive another coin, it will be one block after the others
    txid = bitcoind.rpc.sendtoaddress(lianad.rpc.getnewaddress()["address"], 0.4)

    # Make the timelock of the 3 first coins mature (we use a csv of 10 in the fixture)
    bitcoind.generate_block(9, wait_for_mempool=txid)

    # Now we can create a recovery tx that sweeps the first 3 coins.
    res = lianad.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 18)
    reco_psbt = PSBT.from_base64(res["psbt"])
    assert len(reco_psbt.tx.vin) == 3, "The last coin's timelock hasn't matured yet"
    assert len(reco_psbt.tx.vout) == 1
    assert int(0.5999 * COIN) < int(reco_psbt.tx.vout[0].nValue) < int(0.6 * COIN)
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


def test_create_recovery_specific_paths(lianad_multipath, bitcoind):
    """Test creating recovery PSBTs for specific recovery paths."""
    # We can't create a recovery for a specific recovery path without specifying the precise
    # timelock value of this recovery path.
    with pytest.raises(
        RpcError,
        match="Provided timelock does not correspond to any recovery path: '42424'",
    ):
        lianad_multipath.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2, 42424)

    # Receive a coin and make it immediately available for both reco paths.
    txid = bitcoind.rpc.sendtoaddress(
        lianad_multipath.rpc.getnewaddress()["address"], 1
    )
    bitcoind.generate_block(20, wait_for_mempool=txid)
    wait_for(
        lambda: lianad_multipath.rpc.getinfo()["block_height"]
        == bitcoind.rpc.getblockcount()
    )

    # But we can create one for both existing recovery paths.
    res_10 = lianad_multipath.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2, 10)
    res_20 = lianad_multipath.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2, 20)

    # Both don't have the same BIP32 derivations set in their input, unfortunately. This
    # is because we only set them for the keys from a specific spending path.
    res_10_psbt = PSBT.from_base64(res_10["psbt"])
    res_20_psbt = PSBT.from_base64(res_20["psbt"])
    res_10_keys = set(res_10_psbt.i[0].map[PSBT_IN_BIP32_DERIVATION])
    res_20_keys = set(res_20_psbt.i[0].map[PSBT_IN_BIP32_DERIVATION])
    assert res_10_keys.intersection(res_20_keys) == set()


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
