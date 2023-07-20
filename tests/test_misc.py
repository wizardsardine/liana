import pytest

from fixtures import *
from test_framework.serializations import PSBT
from test_framework.utils import wait_for, RpcError, OLD_LIANAD_PATH, LIANAD_PATH, COIN

from threading import Thread


def receive_and_send(lianad, bitcoind):
    n_coins = len(lianad.rpc.listcoins()["coins"])

    # Receive 3 coins in different blocks on different addresses.
    for _ in range(3):
        addr = lianad.rpc.getnewaddress()["address"]
        txid = bitcoind.rpc.sendtoaddress(addr, 0.01)
        bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == n_coins + 3)

    # Create a spend that will create a change output, sign and broadcast it.
    outpoints = [
        next(
            c["outpoint"]
            for c in lianad.rpc.listcoins()["coins"]
            if c["spend_info"] is None
        )
    ]
    destinations = {
        bitcoind.rpc.getnewaddress(): 200_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 42)
    psbt = PSBT.from_base64(res["psbt"])
    txid = psbt.tx.txid().hex()
    signed_psbt = lianad.signer.sign_psbt(psbt, range(3))
    lianad.rpc.updatespend(signed_psbt.to_base64())
    lianad.rpc.broadcastspend(txid)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )

    # Spend all coins to check we can spend from change too. Re-create some deposits.
    outpoints = [
        c["outpoint"]
        for c in lianad.rpc.listcoins()["coins"]
        if c["spend_info"] is None
    ]
    destinations = {
        bitcoind.rpc.getnewaddress(): 400_000,
        lianad.rpc.getnewaddress()["address"]: 300_000,
        lianad.rpc.getnewaddress()["address"]: 800_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 42)
    psbt = PSBT.from_base64(res["psbt"])
    txid = psbt.tx.txid().hex()
    # If we sign only with two keys it won't be able to finalize
    with pytest.raises(
        RpcError, match="Miniscript Error: could not satisfy at index 0"
    ):
        signed_psbt = lianad.signer.sign_psbt(psbt, range(2))
        lianad.rpc.updatespend(signed_psbt.to_base64())
        lianad.rpc.broadcastspend(txid)
    # We can sign with different keys as long as there are 3 sigs
    signed_psbt = lianad.signer.sign_psbt(psbt, range(1, 4))
    lianad.rpc.updatespend(signed_psbt.to_base64())
    lianad.rpc.broadcastspend(txid)
    bitcoind.generate_block(1, wait_for_mempool=txid)


def test_multisig(lianad_multisig, bitcoind):
    """Test using lianad with a descriptor that contains multiple keys for both
    the primary and recovery paths."""
    receive_and_send(lianad_multisig, bitcoind)

    # Generate 10 blocks to test the recovery path
    bitcoind.generate_block(10)
    wait_for(
        lambda: lianad_multisig.rpc.getinfo()["block_height"]
        == bitcoind.rpc.getblockcount()
    )

    # Sweep all coins through the recovery path. It needs 2 signatures out of
    # 5 keys. Sign with the second and the fifth ones.
    res = lianad_multisig.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)
    reco_psbt = PSBT.from_base64(res["psbt"])
    txid = reco_psbt.tx.txid().hex()
    signed_psbt = lianad_multisig.signer.sign_psbt(reco_psbt, {10: [1, 4]})
    lianad_multisig.rpc.updatespend(signed_psbt.to_base64())
    lianad_multisig.rpc.broadcastspend(txid)


def test_multipath(lianad_multipath, bitcoind):
    """Exercise various commands as well as recovery with a descriptor with multiple
    recovery paths."""
    receive_and_send(lianad_multipath, bitcoind)

    # Generate 10 blocks to test the recovery path
    bitcoind.generate_block(10)
    wait_for(
        lambda: lianad_multipath.rpc.getinfo()["block_height"]
        == bitcoind.rpc.getblockcount()
    )

    # We can't create a recovery tx for the second recovery path, as all coins were confirmed
    # within the last 19 blocks.
    with pytest.raises(
        RpcError,
        match="No coin currently spendable through this timelocked recovery path",
    ):
        lianad_multipath.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2, 20)

    # Sweep all coins through the first recovery path (that is available after 10 blocks).
    # It needs 3 signatures out of 5 keys.
    res = lianad_multipath.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)
    reco_psbt = PSBT.from_base64(res["psbt"])
    txid = reco_psbt.tx.txid().hex()

    # Try to sign with the keys for the next recovery spending path, it'll fail.
    signed_psbt = lianad_multipath.signer.sign_psbt(reco_psbt, {20: range(3)})
    lianad_multipath.rpc.updatespend(signed_psbt.to_base64())
    with pytest.raises(RpcError, match="Failed to finalize"):
        lianad_multipath.rpc.broadcastspend(txid)

    # Try to sign with the right keys but only two of them, it'll fail.
    signed_psbt = lianad_multipath.signer.sign_psbt(reco_psbt, {10: range(2)})
    lianad_multipath.rpc.updatespend(signed_psbt.to_base64())
    with pytest.raises(RpcError, match="Failed to finalize"):
        lianad_multipath.rpc.broadcastspend(txid)

    # Finally add one more signature with an unused key from the right keyset.
    signed_psbt = lianad_multipath.signer.sign_psbt(reco_psbt, {10: [2]})
    lianad_multipath.rpc.updatespend(signed_psbt.to_base64())
    lianad_multipath.rpc.broadcastspend(txid)

    # Receive 3 more coins and make the second recovery path (20 blocks) available.
    txids = []
    for _ in range(3):
        addr = lianad_multipath.rpc.getnewaddress()["address"]
        txids.append(bitcoind.rpc.sendtoaddress(addr, 0.42))
    bitcoind.generate_block(20, wait_for_mempool=txids)
    wait_for(
        lambda: lianad_multipath.rpc.getinfo()["block_height"]
        == bitcoind.rpc.getblockcount()
    )

    # We can create a recovery transaction for an earlier timelock.
    lianad_multipath.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)

    # Sweep all coins through the second recovery path (that is available after 20 blocks).
    # It needs 3 signatures out of 5 keys.
    res = lianad_multipath.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2, 20)
    reco_psbt = PSBT.from_base64(res["psbt"])
    txid = reco_psbt.tx.txid().hex()

    # We can sign with any keys for the second recovery path (we need only 1 out of 10)
    signed_psbt = lianad_multipath.signer.sign_psbt(reco_psbt, {20: [8]})
    lianad_multipath.rpc.updatespend(signed_psbt.to_base64())
    lianad_multipath.rpc.broadcastspend(txid)

    # Now do this again but with signing using keys for the first recovery path.
    # Receive 3 more coins and make the second recovery path (20 blocks) available. Note this
    # is possible since the CSV checks the nSequence is >= to the value, not ==.
    txids = []
    for _ in range(3):
        addr = lianad_multipath.rpc.getnewaddress()["address"]
        txids.append(bitcoind.rpc.sendtoaddress(addr, 0.398))
    bitcoind.generate_block(20, wait_for_mempool=txids)
    wait_for(
        lambda: lianad_multipath.rpc.getinfo()["block_height"]
        == bitcoind.rpc.getblockcount()
    )
    # Sweep all coins through the second recovery path (that is available after 20 blocks).
    # It needs 3 signatures out of 5 keys.
    res = lianad_multipath.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2, 20)
    reco_psbt = PSBT.from_base64(res["psbt"])
    txid = reco_psbt.tx.txid().hex()
    # We can sign with keys for the first recovery path (we need 3 out of 5)
    signed_psbt = lianad_multipath.signer.sign_psbt(reco_psbt, {10: range(2, 5)})
    lianad_multipath.rpc.updatespend(signed_psbt.to_base64())
    lianad_multipath.rpc.broadcastspend(txid)


def test_coinbase_deposit(lianad, bitcoind):
    """Check we detect deposits from (mature) coinbase transactions."""
    wait_for_sync = lambda: wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )
    wait_for_sync()

    # Create a new deposit in a coinbase transaction. We must detect it and treat it as immature.
    addr = lianad.rpc.getnewaddress()["address"]
    bitcoind.rpc.generatetoaddress(1, addr)
    wait_for_sync()
    coins = lianad.rpc.listcoins()["coins"]
    assert (
        len(coins) == 1 and coins[0]["is_immature"] and coins[0]["spend_info"] is None
    )

    # Generate 100 blocks to make the coinbase mature. We should detect it as such.
    bitcoind.generate_block(100)
    wait_for_sync()
    coin = lianad.rpc.listcoins()["coins"][0]
    assert not coin["is_immature"] and coin["block_height"] is not None

    # We must be able to spend the mature coin.
    destinations = {bitcoind.rpc.getnewaddress(): int(0.999999 * COIN)}
    res = lianad.rpc.createspend(destinations, [coin["outpoint"]], 42)
    psbt = PSBT.from_base64(res["psbt"])
    txid = psbt.tx.txid().hex()
    signed_psbt = lianad.signer.sign_psbt(psbt)
    lianad.rpc.updatespend(signed_psbt.to_base64())
    lianad.rpc.broadcastspend(txid)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for_sync()
    coin = next(
        c for c in lianad.rpc.listcoins()["coins"] if c["outpoint"] == coin["outpoint"]
    )
    assert (
        not coin["is_immature"]
        and coin["block_height"] is not None
        and coin["spend_info"] is not None
    )


@pytest.mark.skipif(
    OLD_LIANAD_PATH is None, reason="Need the old lianad binary to create the datadir."
)
def test_migration(lianad_multisig, bitcoind):
    """Test we can start a newer lianad on a datadir created by an older lianad."""
    lianad = lianad_multisig

    # Set the old binary and re-create the datadir.
    lianad.cmd_line[0] = OLD_LIANAD_PATH
    lianad.restart_fresh(bitcoind)
    assert lianad.rpc.getinfo()["version"] == "0.3.0"

    # Perform some transactions. On Liana v0.3 there was no "updated_at" for Spend
    # transaction drafts.
    receive_and_send(lianad, bitcoind)
    spend_txs = lianad.rpc.listspendtxs()["spend_txs"]
    assert len(spend_txs) == 2 and all("updated_at" not in s for s in spend_txs)

    # Set back the new binary. We should be able to read and, if necessary, upgrade
    # the old database and generally all files from the datadir.
    lianad.cmd_line[0] = LIANAD_PATH
    lianad.restart_fresh(bitcoind)

    # And we can go on to create more deposits and transactions. Make sure we now have
    # the "updated_at" field on tx drafts.
    receive_and_send(lianad, bitcoind)
    spend_txs = lianad.rpc.listspendtxs()["spend_txs"]
    assert len(spend_txs) == 2 and all(s["updated_at"] is not None for s in spend_txs)


def test_retry_on_workqueue_exceeded(lianad, bitcoind):
    """Make sure we retry requests to bitcoind if it is temporarily overloaded."""
    # Start by reducing the work queue to a single slot. Note we need to stop lianad
    # as we don't support yet restarting a bitcoind due to the cookie file getting
    # overwritten.
    lianad.stop()
    bitcoind.cmd_line += ["-rpcworkqueue=1", "-rpcthreads=1"]
    bitcoind.stop()
    bitcoind.start()
    lianad.start()

    # Stuck the bitcoind RPC server working queue with a command that takes 5 seconds
    # to be replied to, and make lianad send it a request. Make sure we detect this is
    # a transient HTTP 503 error and we retry the request. Once the 5 seconds are past
    # our request succeeds and we get the reply to the lianad RPC command.
    t = Thread(target=bitcoind.rpc.waitfornewblock, args=(5_000,))
    t.start()
    lianad.rpc.getinfo()
    lianad.wait_for_logs(
        [
            "Transient error when sending request to bitcoind.*(status: 503, body: Work queue depth exceeded)",
            "Retrying RPC request to bitcoind",
        ]
    )
    t.join()
