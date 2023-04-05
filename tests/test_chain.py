from fixtures import *
from test_framework.utils import wait_for, get_txid, spend_coins


def get_coin(lianad, outpoint_or_txid):
    return next(
        c for c in lianad.rpc.listcoins()["coins"] if outpoint_or_txid in c["outpoint"]
    )


def test_reorg_detection(lianad, bitcoind):
    """Test we detect block chain reorganization under various conditions."""
    initial_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == initial_height)

    # Re-mine the last block. We should detect it as a reorg.
    bitcoind.invalidate_remine(initial_height)
    lianad.wait_for_logs(
        ["Block chain reorganization detected.", "Tip was rolled back."]
    )
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == initial_height)

    # Same if we re-mine the next-to-last block.
    bitcoind.invalidate_remine(initial_height - 1)
    lianad.wait_for_logs(
        ["Block chain reorganization detected.", "Tip was rolled back."]
    )
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == initial_height)

    # Same if we re-mine a deep block.
    bitcoind.invalidate_remine(initial_height - 50)
    lianad.wait_for_logs(
        ["Block chain reorganization detected.", "Tip was rolled back."]
    )
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == initial_height)

    # Same if the new chain is longer.
    bitcoind.simple_reorg(initial_height - 10, shift=20)
    lianad.wait_for_logs(
        ["Block chain reorganization detected.", "Tip was rolled back."]
    )
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == initial_height + 10)


def test_reorg_exclusion(lianad, bitcoind):
    """Test the unconfirmation by a reorg of a coin in various states."""
    initial_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == initial_height)

    # A confirmed received coin
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 1)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    coin_a = lianad.rpc.listcoins()["coins"][0]

    # A confirmed and 'spending' (unconfirmed spend) coin
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 2)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 2)
    coin_b = get_coin(lianad, txid)
    b_spend_tx = spend_coins(lianad, bitcoind, [coin_b])

    # A confirmed and spent coin
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 3)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 3)
    coin_c = get_coin(lianad, txid)
    c_spend_tx = spend_coins(lianad, bitcoind, [coin_c])
    bitcoind.generate_block(1, wait_for_mempool=1)

    # Make sure the transaction were confirmed >10 blocks ago, so bitcoind won't update the
    # mempool during the reorg to the initial height.
    bitcoind.generate_block(10)

    # Reorg the chain down to the initial height, excluding all transactions.
    current_height = bitcoind.rpc.getblockcount()
    bitcoind.simple_reorg(initial_height, shift=-1)
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == current_height + 1)

    # During a reorg, bitcoind doesn't update the mempool for blocks too deep (>10 confs).
    # The deposit transactions were dropped. And we discard the unconfirmed coins whose deposit
    # tx isn't part of our mempool anymore: the coins must have been marked as unconfirmed and
    # subsequently discarded.
    wait_for(lambda: len(bitcoind.rpc.getrawmempool()) == 0)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 0)

    # And if we now confirm everything, they'll be marked as such. The one that was 'spending'
    # will now be spent (its spending transaction will be confirmed) and the one that was spent
    # will be marked as such.
    deposit_txids = [c["outpoint"][:-2] for c in (coin_a, coin_b, coin_c)]
    for txid in deposit_txids:
        tx = bitcoind.rpc.gettransaction(txid)["hex"]
        bitcoind.rpc.sendrawtransaction(tx)
    bitcoind.rpc.sendrawtransaction(b_spend_tx)
    bitcoind.rpc.sendrawtransaction(c_spend_tx)
    bitcoind.generate_block(1, wait_for_mempool=5)
    new_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == new_height)
    assert all(
        c["block_height"] == new_height for c in lianad.rpc.listcoins()["coins"]
    ), (lianad.rpc.listcoins()["coins"], new_height)
    new_coin_b = next(
        c
        for c in lianad.rpc.listcoins()["coins"]
        if coin_b["outpoint"] == c["outpoint"]
    )
    b_spend_txid = get_txid(b_spend_tx)
    assert new_coin_b["spend_info"]["txid"] == b_spend_txid
    assert new_coin_b["spend_info"]["height"] == new_height
    new_coin_c = next(
        c
        for c in lianad.rpc.listcoins()["coins"]
        if coin_c["outpoint"] == c["outpoint"]
    )
    c_spend_txid = get_txid(c_spend_tx)
    assert new_coin_c["spend_info"]["txid"] == c_spend_txid
    assert new_coin_c["spend_info"]["height"] == new_height

    # TODO: maybe test with some malleation for the deposit and spending txs?


def spend_confirmed_noticed(lianad, outpoint):
    c = get_coin(lianad, outpoint)
    if c["spend_info"] is None:
        return False
    if c["spend_info"]["height"] is None:
        return False
    return True


def test_reorg_status_recovery(lianad, bitcoind):
    """
    Test the coins that were not unconfirmed recover their initial state after a reorg.
    """
    list_coins = lambda: lianad.rpc.listcoins()["coins"]

    # Create two confirmed coins. Note how we take the initial_height after having
    # mined them, as we'll reorg back to this height and due to anti fee-sniping
    # these deposit transactions might not be valid anymore!
    addresses = (lianad.rpc.getnewaddress()["address"] for _ in range(2))
    txids = [bitcoind.rpc.sendtoaddress(addr, 0.5670) for addr in addresses]
    bitcoind.generate_block(1, wait_for_mempool=txids)
    initial_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == initial_height)

    # Both coins are confirmed. Spend the second one then get their infos.
    wait_for(lambda: len(list_coins()) == 2)
    wait_for(lambda: all(c["block_height"] is not None for c in list_coins()))
    coin_b = get_coin(lianad, txids[1])
    spend_coins(lianad, bitcoind, [coin_b])
    bitcoind.generate_block(1, wait_for_mempool=1)
    wait_for(lambda: spend_confirmed_noticed(lianad, coin_b["outpoint"]))
    coin_a = get_coin(lianad, txids[0])
    coin_b = get_coin(lianad, txids[1])

    # Reorg the chain down to the initial height without shifting nor malleating
    # any transaction. The coin info should be identical (except the transaction
    # spending the second coin will be mined at the height the reorg happened).
    bitcoind.simple_reorg(initial_height, shift=0)
    new_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == new_height)
    new_coin_a = get_coin(lianad, coin_a["outpoint"])
    assert coin_a == new_coin_a
    new_coin_b = get_coin(lianad, coin_b["outpoint"])
    coin_b["spend_info"]["height"] = initial_height
    assert new_coin_b == coin_b


def test_rescan_edge_cases(lianad, bitcoind):
    """Test some specific cases that could arise when rescanning the chain."""
    initial_tip = bitcoind.rpc.getblockheader(bitcoind.rpc.getbestblockhash())

    # Some helpers
    list_coins = lambda: lianad.rpc.listcoins()["coins"]
    sorted_coins = lambda: sorted(list_coins(), key=lambda c: c["outpoint"])
    wait_synced = lambda: wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )

    def reorg_shift(height, txs):
        """Remine the chain from given height, shifting the txs by one block."""
        delta = bitcoind.rpc.getblockcount() - height + 1
        assert delta > 2
        h = bitcoind.rpc.getblockhash(initial_tip["height"])
        bitcoind.rpc.invalidateblock(h)
        bitcoind.generate_block(1)
        for tx in txs:
            bitcoind.rpc.sendrawtransaction(tx)
        bitcoind.generate_block(delta - 1, wait_for_mempool=len(txs))

    # Create 3 coins and spend 2 of them. Keep the transactions in memory to
    # rebroadcast them on reorgs.
    txs = []
    for _ in range(3):
        addr = lianad.rpc.getnewaddress()["address"]
        amount = 0.356
        txid = bitcoind.rpc.sendtoaddress(addr, amount)
        txs.append(bitcoind.rpc.gettransaction(txid)["hex"])
    wait_for(lambda: len(list_coins()) == 3)
    txs.append(spend_coins(lianad, bitcoind, list_coins()[:2]))
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
    lianad.restart_fresh(bitcoind)
    assert len(list_coins()) == 0

    # We can be stopped while we are rescanning
    lianad.rpc.startrescan(initial_tip["time"])
    lianad.stop()
    lianad.start()
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)
    wait_synced()
    assert coins_before == sorted_coins()

    # Lose our state again
    bitcoind.generate_block(1)
    lianad.restart_fresh(bitcoind)
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)
    assert len(list_coins()) == 0

    # There can be a reorg when we start rescanning
    reorg_shift(initial_tip["height"], txs)
    lianad.rpc.startrescan(initial_tip["time"])
    wait_synced()
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)
    assert len(sorted_coins()) == len(coins_before)
    assert all(c["outpoint"] in outpoints_before for c in list_coins())

    # Advance the blocktime again
    bitcoind.rpc.setmocktime(initial_tip["time"] + added_time * 2)
    bitcoind.generate_block(12)

    # Lose our state again
    bitcoind.generate_block(1)
    lianad.restart_fresh(bitcoind)
    wait_synced()
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)
    assert len(list_coins()) == 0

    # We can be rescanning when a reorg happens
    lianad.rpc.startrescan(initial_tip["time"])
    reorg_shift(initial_tip["height"] + 1, txs)
    wait_synced()
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)
    assert len(sorted_coins()) == len(coins_before)
    assert all(c["outpoint"] in outpoints_before for c in list_coins())


def test_deposit_replacement(lianad, bitcoind):
    """Test we discard an unconfirmed deposit that was replaced."""
    # Get some more coins.
    bitcoind.generate_block(1)

    # Create a new unconfirmed deposit.
    addr = lianad.rpc.getnewaddress()["address"]
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
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    txid = bitcoind.rpc.sendrawtransaction(conflicting_tx)

    # We must forget about the deposit.
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 0)

    # Send a new one, it'll be detected.
    addr = lianad.rpc.getnewaddress()["address"]
    bitcoind.rpc.sendtoaddress(addr, 2)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
