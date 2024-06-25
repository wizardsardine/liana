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

    # Generate blocks in order to test locktime set correctly.
    bitcoind.generate_block(200)
    # Create two confirmed coins. Note how we take the initial_height after having
    # mined them, as we'll reorg back to this height and due to anti fee-sniping
    # these deposit transactions might not be valid anymore!
    addresses = (lianad.rpc.getnewaddress()["address"] for _ in range(2))
    txids = [bitcoind.rpc.sendtoaddress(addr, 0.5670) for addr in addresses]
    bitcoind.generate_block(1, wait_for_mempool=txids)
    initial_height = bitcoind.rpc.getblockcount()
    assert initial_height > 100
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == initial_height)

    # Both coins are confirmed. Spend the second one then get their infos.
    wait_for(lambda: len(list_coins()) == 2)
    wait_for(lambda: all(c["block_height"] is not None for c in list_coins()))
    coin_b = get_coin(lianad, txids[1])
    tx = spend_coins(lianad, bitcoind, [coin_b])
    locktime = bitcoind.rpc.decoderawtransaction(tx)["locktime"]
    assert initial_height - 100 <= locktime <= initial_height
    bitcoind.generate_block(1, wait_for_mempool=1)
    wait_for(lambda: spend_confirmed_noticed(lianad, coin_b["outpoint"]))
    coin_a = get_coin(lianad, txids[0])
    coin_b = get_coin(lianad, txids[1])

    # Reorg the chain down to the initial height without shifting nor malleating
    # any transaction. The coin info should be identical (except the spend info
    # of the transaction spending the second coin).
    bitcoind.simple_reorg(initial_height, shift=0)
    new_height = bitcoind.rpc.getblockcount()
    wait_for(lambda: lianad.rpc.getinfo()["block_height"] == new_height)
    new_coin_a = get_coin(lianad, coin_a["outpoint"])
    assert coin_a == new_coin_a
    new_coin_b = get_coin(lianad, coin_b["outpoint"])

    if locktime == initial_height:
        # Cannot be mined until next block (initial_height + 1).
        coin_b["spend_info"] = None
    else:
        # Otherwise, the tx will be mined at the height the reorg happened.
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


def test_rescan_and_recovery(lianad, bitcoind):
    """Test user recovery flow"""
    # Get initial_tip to use for rescan later
    initial_tip = bitcoind.rpc.getblockheader(bitcoind.rpc.getbestblockhash())

    # Start by getting a few coins
    destination = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(destination, 0.5)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )
    assert len(lianad.rpc.listcoins()["coins"]) == 1

    # Advance the blocktime by >2h in median-time past for rescan
    added_time = 60 * 60 * 3
    bitcoind.rpc.setmocktime(initial_tip["time"] + added_time)
    bitcoind.generate_block(12)

    # Clear lianad state
    lianad.restart_fresh(bitcoind)
    assert len(lianad.rpc.listcoins()["coins"]) == 0

    # Start rescan
    lianad.rpc.startrescan(initial_tip["time"])
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    wait_for(lambda: lianad.rpc.getinfo()["rescan_progress"] is None)

    # Create a recovery tx that sweeps the first coin.
    res = lianad.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)
    reco_psbt = PSBT.from_base64(res["psbt"])
    assert len(reco_psbt.tx.vin) == 1
    assert len(reco_psbt.tx.vout) == 1
    assert int(0.4999 * COIN) < int(reco_psbt.tx.vout[0].nValue) < int(0.5 * COIN)
    sign_and_broadcast(lianad, bitcoind, reco_psbt, recovery=True)


@pytest.mark.skipif(
    USE_TAPROOT, reason="Needs a finalizer implemented in the Python test framework."
)
def test_conflicting_unconfirmed_spend_txs(lianad, bitcoind):
    """Test we'll update the spending txid of a coin if a conflicting spend enters our mempool."""
    # Get an (unconfirmed, on purpose) coin to be spent by 2 different txs.
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 0.01)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
    spent_coin = lianad.rpc.listcoins()["coins"][0]

    # Create a first transaction, register it in our wallet.
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 100_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 2)
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
    signed_psbt = lianad.signer.sign_psbt(psbt_a)
    lianad.rpc.updatespend(signed_psbt.to_base64())
    lianad.rpc.broadcastspend(txid_a.hex())

    # We detect the coin as being spent by the first transaction.
    wait_for(lambda: get_coin(lianad, spent_coin["outpoint"])["spend_info"] is not None)
    assert (
        get_coin(lianad, spent_coin["outpoint"])["spend_info"]["txid"] == txid_a.hex()
    )

    # Now sign and broadcast the conflicting transaction, as if coming from an external
    # wallet.
    signed_psbt = lianad.signer.sign_psbt(psbt_b)
    finalized_psbt = lianad.finalize_psbt(signed_psbt)
    tx_hex = finalized_psbt.tx.serialize_with_witness().hex()
    bitcoind.rpc.sendrawtransaction(tx_hex)

    # We must now detect the coin as being spent by the second transaction.
    def is_spent_by(lianad, outpoint, txid):
        coins = lianad.rpc.listcoins([], [outpoint])["coins"]
        if len(coins) == 0:
            return False
        coin = coins[0]
        if coin["spend_info"] is None:
            return False
        return coin["spend_info"]["txid"] == txid.hex()

    wait_for_while_condition_holds(
        lambda: is_spent_by(lianad, spent_coin["outpoint"], txid_b),
        lambda: lianad.rpc.listcoins([], [spent_coin["outpoint"]])["coins"][0][
            "spend_info"
        ]
        is not None,  # The spend txid changes directly from txid_a to txid_b
    )


def test_spend_replacement(lianad, bitcoind):
    """Test we detect the new version of the unconfirmed spending transaction."""
    # Get three coins.
    destinations = {
        lianad.rpc.getnewaddress()["address"]: 0.03,
        lianad.rpc.getnewaddress()["address"]: 0.04,
        lianad.rpc.getnewaddress()["address"]: 0.05,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 3)
    coins = lianad.rpc.listcoins(["confirmed"])["coins"]

    # Create three conflicting spends, the two first spend two different set of coins
    # and the third one is just an RBF of the second one but as a send-to-self.
    first_outpoints = [c["outpoint"] for c in coins[:2]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 650_000,
    }
    first_res = lianad.rpc.createspend(destinations, first_outpoints, 1)
    first_psbt = PSBT.from_base64(first_res["psbt"])
    second_outpoints = [c["outpoint"] for c in coins[1:]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 650_000,
    }
    second_res = lianad.rpc.createspend(destinations, second_outpoints, 3)
    second_psbt = PSBT.from_base64(second_res["psbt"])
    destinations = {}
    third_res = lianad.rpc.createspend(destinations, second_outpoints, 5)
    third_psbt = PSBT.from_base64(third_res["psbt"])

    # Broadcast the first transaction. Make sure it's detected.
    first_txid = sign_and_broadcast_psbt(lianad, first_psbt)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == first_txid
            for c in lianad.rpc.listcoins([], first_outpoints)["coins"]
        )
    )

    # Now RBF the first transaction by the second one. The third coin should be
    # newly marked as spending, the second one's spend_txid should be updated and
    # the first one's spend txid should be dropped.
    second_txid = sign_and_broadcast_psbt(lianad, second_psbt)
    wait_for_while_condition_holds(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == second_txid
            for c in lianad.rpc.listcoins([], second_outpoints)["coins"]
        ),
        lambda: lianad.rpc.listcoins([], [coins[1]["outpoint"]])["coins"][0][
            "spend_info"
        ]
        is not None,  # The spend txid of coin from first spend is updated directly
    )
    wait_for(
        lambda: lianad.rpc.listcoins([], [first_outpoints[0]])["coins"][0]["spend_info"]
        is None
    )

    # Now RBF the second transaction with a send-to-self, just because.
    third_txid = sign_and_broadcast_psbt(lianad, third_psbt)
    wait_for_while_condition_holds(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["txid"] == third_txid
            for c in lianad.rpc.listcoins([], second_outpoints)["coins"]
        ),
        lambda: all(
            c["spend_info"] is not None
            for c in lianad.rpc.listcoins([], second_outpoints)["coins"]
        ),  # The spend txid of all coins are updated directly
    )
    assert (
        lianad.rpc.listcoins([], [first_outpoints[0]])["coins"][0]["spend_info"] is None
    )

    # Once the RBF is mined, we detect it as confirmed and the first coin is still unspent.
    bitcoind.generate_block(1, wait_for_mempool=third_txid)
    wait_for(
        lambda: all(
            c["spend_info"] is not None and c["spend_info"]["height"] is not None
            for c in lianad.rpc.listcoins([], second_outpoints)["coins"]
        )
    )
    assert (
        lianad.rpc.listcoins([], [first_outpoints[0]])["coins"][0]["spend_info"] is None
    )
