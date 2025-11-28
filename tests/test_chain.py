import copy

from fixtures import *
from test_framework.utils import (
    wait_for,
    wait_for_while_condition_holds,
    get_txid,
    spend_coins,
    RpcError,
    COIN,
    sign_and_broadcast,
    sign_and_broadcast_psbt,
    USE_TAPROOT,
)
from test_framework.serializations import PSBT


def get_coin(coincubed, outpoint_or_txid):
    return next(
        c for c in coincubed.rpc.listcoins()["coins"] if outpoint_or_txid in c["outpoint"]
    )


def test_reorg_detection(coincubed, bitcoind):
    """Test we detect block chain reorganization under various conditions."""
    initial_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == initial_height)

    # Re-mine the last block. We should detect it as a reorg.
    bitcoind.invalidate_remine(initial_height)
    coincubed.wait_for_logs(
        ["Block chain reorganization detected.", "Tip was rolled back."]
    )
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == initial_height)

    # Same if we re-mine the next-to-last block.
    bitcoind.invalidate_remine(initial_height - 1)
    coincubed.wait_for_logs(
        ["Block chain reorganization detected.", "Tip was rolled back."]
    )
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == initial_height)

    # Same if we re-mine a deep block.
    bitcoind.invalidate_remine(initial_height - 50)
    coincubed.wait_for_logs(
        ["Block chain reorganization detected.", "Tip was rolled back."]
    )
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == initial_height)

    # Same if the new chain is longer.
    bitcoind.simple_reorg(initial_height - 10, shift=20)
    coincubed.wait_for_logs(
        ["Block chain reorganization detected.", "Tip was rolled back."]
    )
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == initial_height + 10)


def test_reorg_exclusion(coincubed, bitcoind):
    """Test the unconfirmation by a reorg of a coin in various states."""
    initial_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == initial_height)

    # A confirmed received coin
    addr = coincubed.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 1)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 1)
    coin_a = coincubed.rpc.listcoins()["coins"][0]

    # A confirmed and 'spending' (unconfirmed spend) coin
    addr = coincubed.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 2)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 2)
    coin_b = get_coin(coincubed, txid)
    b_spend_tx = spend_coins(coincubed, bitcoind, [coin_b])

    # These are external deposits so not from self.
    assert coin_a["is_from_self"] is False
    assert coin_b["is_from_self"] is False

    # A confirmed and spent coin
    addr = coincubed.rpc.getnewaddress()["address"]
    txid_c = bitcoind.rpc.sendtoaddress(addr, 3)
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 3)
    # Now refresh this coin while it is unconfirmed.
    res = coincubed.rpc.createspend({}, [get_coin(coincubed, txid_c)["outpoint"]], 1)
    c_spend_psbt = PSBT.from_base64(res["psbt"])
    txid_d = sign_and_broadcast_psbt(coincubed, c_spend_psbt)
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 4)
    coin_c = get_coin(coincubed, txid_c)
    coin_d = get_coin(coincubed, txid_d)
    assert coin_c["is_from_self"] is False
    assert coin_c["block_height"] is None
    # Even though coin_d is from a self-send, coin_c is still unconfirmed
    # and is not from self. Therefore, coin_d is not from self either.
    assert coin_d["is_from_self"] is False

    bitcoind.generate_block(1)
    # Wait for confirmation to be detected.
    wait_for(lambda: get_coin(coincubed, txid_d)["block_height"] is not None)
    coin_c = get_coin(coincubed, txid_c)
    coin_d = get_coin(coincubed, txid_d)
    assert coin_c["is_from_self"] is False
    assert coin_c["block_height"] is not None
    assert coin_d["is_from_self"] is True
    assert coin_d["block_height"] is not None

    # Make sure the transaction were confirmed >10 blocks ago, so bitcoind won't update the
    # mempool during the reorg to the initial height.
    bitcoind.generate_block(10)

    # Reorg the chain down to the initial height, excluding all transactions.
    current_height = bitcoind.rpc.getblockcount()
    bitcoind.simple_reorg(initial_height, shift=-1)
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == current_height + 1)

    # During a reorg, bitcoind doesn't update the mempool for blocks too deep (>10 confs).
    # The deposit transactions were dropped. And we discard the unconfirmed coins whose deposit
    # tx isn't part of our mempool anymore: the coins must have been marked as unconfirmed and
    # subsequently discarded.
    wait_for(lambda: len(bitcoind.rpc.getrawmempool()) == 0)
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 0)

    # And if we now confirm everything, they'll be marked as such. The one that was 'spending'
    # will now be spent (its spending transaction will be confirmed) and the one that was spent
    # will be marked as such.
    deposit_txids = [c["outpoint"][:-2] for c in (coin_a, coin_b, coin_c)]
    for txid in deposit_txids:
        tx = bitcoind.rpc.gettransaction(txid)["hex"]
        bitcoind.rpc.sendrawtransaction(tx)
    bitcoind.rpc.sendrawtransaction(b_spend_tx)
    sign_and_broadcast_psbt(coincubed, c_spend_psbt)
    bitcoind.generate_block(1, wait_for_mempool=5)
    new_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == new_height)
    assert all(
        c["block_height"] == new_height for c in coincubed.rpc.listcoins()["coins"]
    ), (coincubed.rpc.listcoins()["coins"], new_height)
    new_coin_b = next(
        c
        for c in coincubed.rpc.listcoins()["coins"]
        if coin_b["outpoint"] == c["outpoint"]
    )
    b_spend_txid = get_txid(b_spend_tx)
    assert new_coin_b["spend_info"]["txid"] == b_spend_txid
    assert new_coin_b["spend_info"]["height"] == new_height
    new_coin_c = next(
        c
        for c in coincubed.rpc.listcoins()["coins"]
        if coin_c["outpoint"] == c["outpoint"]
    )
    assert new_coin_c["spend_info"]["txid"] == txid_d
    assert new_coin_c["spend_info"]["height"] == new_height
    new_coin_d = get_coin(coincubed, txid_d)
    assert new_coin_d["is_from_self"] is True
    assert new_coin_d["block_height"] == new_height

    # TODO: maybe test with some malleation for the deposit and spending txs?


def spend_confirmed_noticed(coincubed, outpoint):
    c = get_coin(coincubed, outpoint)
    if c["spend_info"] is None:
        return False
    if c["spend_info"]["height"] is None:
        return False
    return True


def test_reorg_status_recovery(coincubed, bitcoind):
    """
    Test the coins that were not unconfirmed recover their initial state after a reorg.
    """
    list_coins = lambda: coincubed.rpc.listcoins()["coins"]

    # Generate blocks in order to test locktime set correctly.
    bitcoind.generate_block(200)
    # Create two confirmed coins. Note how we take the initial_height after having
    # mined them, as we'll reorg back to this height and due to anti fee-sniping
    # these deposit transactions might not be valid anymore!
    addresses = (coincubed.rpc.getnewaddress()["address"] for _ in range(2))
    txids = [bitcoind.rpc.sendtoaddress(addr, 0.5670) for addr in addresses]
    bitcoind.generate_block(1, wait_for_mempool=txids)
    initial_height = bitcoind.rpc.getblockcount()
    assert initial_height > 100
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == initial_height)

    # Both coins are confirmed. Refresh the second one then get their infos.
    wait_for(lambda: len(list_coins()) == 2)
    wait_for(lambda: all(c["block_height"] is not None for c in list_coins()))
    coin_b = get_coin(coincubed, txids[1])
    # Refresh coin_b.
    res = coincubed.rpc.createspend({}, [coin_b["outpoint"]], 1)
    b_spend_psbt = PSBT.from_base64(res["psbt"])
    txid = sign_and_broadcast_psbt(coincubed, b_spend_psbt)
    coin_c = get_coin(coincubed, txid)
    # coin_c is unconfirmed and marked as from self as its parent is confirmed.
    assert coin_c["block_height"] is None
    assert coin_c["is_from_self"] is True

    locktime = b_spend_psbt.tx.nLockTime
    assert initial_height - 100 <= locktime <= initial_height
    bitcoind.generate_block(1, wait_for_mempool=1)
    wait_for(lambda: spend_confirmed_noticed(coincubed, coin_b["outpoint"]))
    coin_a = get_coin(coincubed, txids[0])
    coin_b = get_coin(coincubed, txids[1])
    coin_c = get_coin(coincubed, txid)

    # Reorg the chain down to the initial height without shifting nor malleating
    # any transaction. The coin info should be identical (except the spend info
    # of the transaction spending the second coin).
    bitcoind.simple_reorg(initial_height, shift=0)
    new_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: coincubed.rpc.getinfo()["block_height"] == new_height)
    new_coin_a = get_coin(coincubed, coin_a["outpoint"])
    assert coin_a == new_coin_a
    new_coin_b = get_coin(coincubed, coin_b["outpoint"])

    if locktime == initial_height:
        # Cannot be mined until next block (initial_height + 1).
        coin_b["spend_info"] = None
        # coin_c no longer exists.
        with pytest.raises(StopIteration):
            get_coin(coincubed, coin_c["outpoint"])
    else:
        # Otherwise, the tx will be mined at the height the reorg happened.
        coin_b["spend_info"]["height"] = initial_height
        new_coin_c = get_coin(coincubed, coin_c["outpoint"])
        assert new_coin_c["is_from_self"] is True
    assert new_coin_b == coin_b


def test_rescan_edge_cases(coincubed, bitcoind):
    """Test some specific cases that could arise when rescanning the chain."""
    initial_tip = bitcoind.rpc.getblockheader(bitcoind.rpc.getbestblockhash())

    # Some helpers
    list_coins = lambda: coincubed.rpc.listcoins()["coins"]
    sorted_coins = lambda: sorted(list_coins(), key=lambda c: c["outpoint"])
    wait_synced = lambda: wait_for(
        lambda: coincubed.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )

    def reorg_shift(height, txs):
        """Remine the chain from given height, shifting the txs by one block."""
        delta = bitcoind.rpc.getblockcount() - height + 1
        assert delta > 2
        h = bitcoind.rpc.getblockhash(height)
        bitcoind.rpc.invalidateblock(h)
        bitcoind.generate_block(1)
        for tx in txs:
            bitcoind.rpc.sendrawtransaction(tx)
        bitcoind.generate_block(delta - 1, wait_for_mempool=len(txs))

    # Create 3 coins and spend 2 of them. Keep the transactions in memory to
    # rebroadcast them on reorgs.
    txs = []
    for _ in range(3):
        addr = coincubed.rpc.getnewaddress()["address"]
        amount = 0.356
        txid = bitcoind.rpc.sendtoaddress(addr, amount)
        txs.append(bitcoind.rpc.gettransaction(txid)["hex"])
    wait_for(lambda: len(list_coins()) == 3)
    txs.append(spend_coins(coincubed, bitcoind, list_coins()[:2]))
    bitcoind.generate_block(1, wait_for_mempool=4)
    wait_synced()

    # Advance the blocktime by >2h in the future for the importdescriptors rescan
    added_time = 60 * 60 * 3
    bitcoind.rpc.setmocktime(initial_tip["time"] + added_time)
    bitcoind.generate_block(12)

    # Lose our state
    coins_before = sorted_coins()
    outpoints_before = set(c["outpoint"] for c in coins_before)
    bitcoind.generate_block(1)
    coincubed.restart_fresh(bitcoind)
    if BITCOIN_BACKEND_TYPE is BitcoinBackendType.Bitcoind:
        assert len(list_coins()) == 0

    # We can be stopped while we are rescanning
    coincubed.rpc.startrescan(initial_tip["time"])
    coincubed.stop()
    coincubed.start()
    wait_for(lambda: coincubed.rpc.getinfo()["rescan_progress"] is None)
    wait_synced()
    assert coins_before == sorted_coins()

    # Lose our state again
    bitcoind.generate_block(1)
    coincubed.restart_fresh(bitcoind)
    wait_for(lambda: coincubed.rpc.getinfo()["rescan_progress"] is None)
    if BITCOIN_BACKEND_TYPE is BitcoinBackendType.Bitcoind:
        assert len(list_coins()) == 0

    # There can be a reorg when we start rescanning
    reorg_shift(initial_tip["height"], txs)
    coincubed.rpc.startrescan(initial_tip["time"])
    wait_synced()
    wait_for(lambda: coincubed.rpc.getinfo()["rescan_progress"] is None)
    assert len(sorted_coins()) == len(coins_before)
    assert all(c["outpoint"] in outpoints_before for c in list_coins())

    # Advance the blocktime again
    bitcoind.rpc.setmocktime(initial_tip["time"] + added_time * 2)
    bitcoind.generate_block(12)

    # Lose our state again
    bitcoind.generate_block(1)
    coincubed.restart_fresh(bitcoind)
    wait_synced()
    wait_for(lambda: coincubed.rpc.getinfo()["rescan_progress"] is None)
    if BITCOIN_BACKEND_TYPE is BitcoinBackendType.Bitcoind:
        assert len(list_coins()) == 0

    # We can be rescanning when a reorg happens
    coincubed.rpc.startrescan(initial_tip["time"])
    reorg_shift(initial_tip["height"] + 1, txs)
    wait_synced()
    wait_for(lambda: coincubed.rpc.getinfo()["rescan_progress"] is None)
    assert len(sorted_coins()) == len(coins_before)
    assert all(c["outpoint"] in outpoints_before for c in list_coins())


def test_deposit_replacement(coincubed, bitcoind):
    """Test we discard an unconfirmed deposit that was replaced."""
    # Get some more coins.
    bitcoind.generate_block(1)

    # Create a new unconfirmed deposit.
    addr = coincubed.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 1)

    # Create a transaction conflicting with the deposit that pays more fee.
    deposit_tx = bitcoind.rpc.gettransaction(txid, False, True)["decoded"]
    bitcoind.rpc.lockunspent(
        False,
        [
            {"txid": deposit_tx["txid"], "vout": i}
            for i in range(len(deposit_tx["vout"]))
        ],
    )
    res = bitcoind.rpc.walletcreatefundedpsbt(
        [
            {"txid": txin["txid"], "vout": txin["vout"], "sequence": 0xFF_FF_FF_FD}
            for txin in deposit_tx["vin"]
        ],
        [
            {bitcoind.rpc.getnewaddress(): txout["value"]}
            for txout in deposit_tx["vout"]
        ],
        0,
        {"fee_rate": 10, "add_inputs": True},
    )
    res = bitcoind.rpc.walletprocesspsbt(res["psbt"])
    assert res["complete"]
    conflicting_tx = bitcoind.rpc.finalizepsbt(res["psbt"])["hex"]

    # Make sure we registered the unconfirmed coin. Then RBF the deposit tx.
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 1)
    txid = bitcoind.rpc.sendrawtransaction(conflicting_tx)

    # We must forget about the deposit.
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 0)

    # Send a new one, it'll be detected.
    addr = coincubed.rpc.getnewaddress()["address"]
    bitcoind.rpc.sendtoaddress(addr, 2)
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 1)


def test_rescan_and_recovery(coincubed, bitcoind):
    """Test user recovery flow"""
    # Get initial_tip to use for rescan later
    initial_tip = bitcoind.rpc.getblockheader(bitcoind.rpc.getbestblockhash())

    # Start by getting a few coins
    destination = coincubed.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(destination, 0.5)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(
        lambda: coincubed.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )
    assert len(coincubed.rpc.listcoins()["coins"]) == 1

    # Advance the blocktime by >2h in median-time past for rescan
    added_time = 60 * 60 * 3
    bitcoind.rpc.setmocktime(initial_tip["time"] + added_time)
    bitcoind.generate_block(12)

    # Clear coincubed state
    coincubed.restart_fresh(bitcoind)
    if BITCOIN_BACKEND_TYPE is BitcoinBackendType.Bitcoind:
        assert len(coincubed.rpc.listcoins()["coins"]) == 0

    # Start rescan
    coincubed.rpc.startrescan(initial_tip["time"])
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 1)
    wait_for(lambda: coincubed.rpc.getinfo()["rescan_progress"] is None)

    # Create a recovery tx that sweeps the first coin.
    res = coincubed.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)
    reco_psbt = PSBT.from_base64(res["psbt"])
    assert len(reco_psbt.tx.vin) == 1
    assert len(reco_psbt.tx.vout) == 1
    assert int(0.4999 * COIN) < int(reco_psbt.tx.vout[0].nValue) < int(0.5 * COIN)
    sign_and_broadcast(coincubed, bitcoind, reco_psbt, recovery=True)


@pytest.mark.skipif(
    USE_TAPROOT, reason="Needs a finalizer implemented in the Python test framework."
)
def test_conflicting_unconfirmed_spend_txs(coincubed, bitcoind):
    """Test we'll update the spending txid of a coin if a conflicting spend enters our mempool."""
    # Get an (unconfirmed, on purpose) coin to be spent by 2 different txs.
    addr = coincubed.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 0.01)
    wait_for(lambda: len(coincubed.rpc.listcoins()["coins"]) == 1)
    spent_coin = coincubed.rpc.listcoins()["coins"][0]

    # Create a first transaction, register it in our wallet.
    outpoints = [c["outpoint"] for c in coincubed.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 100_000,
    }
    res = coincubed.rpc.createspend(destinations, outpoints, 2)
    psbt_a = PSBT.from_base64(res["psbt"])
    txid_a = psbt_a.tx.txid()

    # Create a conflicting transaction, not to be registered in our wallet.
    psbt_b = copy.deepcopy(psbt_a)
    psbt_b.tx.vout[0].scriptPubKey = bytes.fromhex(
        "0014218612c653e0827f73a6a040d7805acefa6530cb"
    )
    psbt_b.tx.vout[0].nValue -= 10_000
    psbt_b.tx.rehash()
    txid_b = psbt_b.tx.txid()

    # Sign and broadcast the first Spend transaction.
    signed_psbt = coincubed.signer.sign_psbt(psbt_a)
    coincubed.rpc.updatespend(signed_psbt.to_base64())
    coincubed.rpc.broadcastspend(txid_a.hex())

    # We detect the coin as being spent by the first transaction.
    wait_for(lambda: get_coin(coincubed, spent_coin["outpoint"])["spend_info"] is not None)
    assert (
        get_coin(coincubed, spent_coin["outpoint"])["spend_info"]["txid"] == txid_a.hex()
    )

    # Now sign and broadcast the conflicting transaction, as if coming from an external
    # wallet.
    signed_psbt = coincubed.signer.sign_psbt(psbt_b)
    finalized_psbt = coincubed.finalize_psbt(signed_psbt)
    tx_hex = finalized_psbt.tx.serialize_with_witness().hex()
    bitcoind.rpc.sendrawtransaction(tx_hex)

    # We must now detect the coin as being spent by the second transaction.
    def is_spent_by(coincubed, outpoint, txid):
        coins = coincubed.rpc.listcoins([], [outpoint])["coins"]
        if len(coins) == 0:
            return False
        coin = coins[0]
        if coin["spend_info"] is None:
            return False
        return coin["spend_info"]["txid"] == txid.hex()

    wait_for_while_condition_holds(
        lambda: is_spent_by(coincubed, spent_coin["outpoint"], txid_b),
        lambda: coincubed.rpc.listcoins([], [spent_coin["outpoint"]])["coins"][0][
            "spend_info"
        ]
        is not None,  # The spend txid changes directly from txid_a to txid_b
    )


def test_spend_replacement(coincubed, bitcoind):
    """Test we detect the new version of the unconfirmed spending transaction."""
    # Get three coins.
    destinations = {
        coincubed.rpc.getnewaddress()["address"]: 0.03,
        coincubed.rpc.getnewaddress()["address"]: 0.04,
        coincubed.rpc.getnewaddress()["address"]: 0.05,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(coincubed.rpc.listcoins(["confirmed"])["coins"]) == 3)
    coins = coincubed.rpc.listcoins(["confirmed"])["coins"]

    # Create three conflicting spends, the two first spend two different set of coins
    # and the third one is just an RBF of the second one but as a send-to-self.
    first_outpoints = [c["outpoint"] for c in coins[:2]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 650_000,
    }
    first_res = coincubed.rpc.createspend(destinations, first_outpoints, 1)
    first_psbt = PSBT.from_base64(first_res["psbt"])
    second_outpoints = [c["outpoint"] for c in coins[1:]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 650_000,
    }
    second_res = coincubed.rpc.createspend(destinations, second_outpoints, 3)
    second_psbt = PSBT.from_base64(second_res["psbt"])
    destinations = {}
    third_res = coincubed.rpc.createspend(destinations, second_outpoints, 5)
    third_psbt = PSBT.from_base64(third_res["psbt"])

    # Broadcast the first transaction. Make sure it's detected.
    first_txid = sign_and_broadcast_psbt(coincubed, first_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == first_txid
            for c in coincubed.rpc.listcoins([], first_outpoints)["coins"]
        )
    )

    # Now RBF the first transaction by the second one. The third coin should be
    # newly marked as spending, the second one's spend_txid should be updated and
    # the first one's spend txid should be dropped.
    second_txid = sign_and_broadcast_psbt(coincubed, second_psbt)
    wait_for_while_condition_holds(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == second_txid
            for c in coincubed.rpc.listcoins([], second_outpoints)["coins"]
        ),
        lambda: coincubed.rpc.listcoins([], [coins[1]["outpoint"]])["coins"][0][
            "spend_info"
        ]
        is not None,  # The spend txid of coin from first spend is updated directly
    )
    wait_for(
        lambda: coincubed.rpc.listcoins([], [first_outpoints[0]])["coins"][0]["spend_info"]
        is None
    )

    # Now RBF the second transaction with a send-to-self, just because.
    third_txid = sign_and_broadcast_psbt(coincubed, third_psbt)
    wait_for_while_condition_holds(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == third_txid
            for c in coincubed.rpc.listcoins([], second_outpoints)["coins"]
        ),
        lambda: all(
            c["spend_info"] is not None
            for c in coincubed.rpc.listcoins([], second_outpoints)["coins"]
        ),  # The spend txid of all coins are updated directly
    )
    assert (
        coincubed.rpc.listcoins([], [first_outpoints[0]])["coins"][0]["spend_info"] is None
    )

    # Once the RBF is mined, we detect it as confirmed and the first coin is still unspent.
    bitcoind.generate_block(1, wait_for_mempool=third_txid)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["height"] is not None
            for c in coincubed.rpc.listcoins([], second_outpoints)["coins"]
        )
    )
    assert (
        coincubed.rpc.listcoins([], [first_outpoints[0]])["coins"][0]["spend_info"] is None
    )
