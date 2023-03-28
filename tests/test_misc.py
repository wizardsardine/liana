import pytest

from fixtures import *
from test_framework.serializations import PSBT
from test_framework.utils import wait_for, RpcError


def receive_and_send(lianad, bitcoind):
    # Receive 3 coins in different blocks on different addresses.
    for _ in range(3):
        addr = lianad.rpc.getnewaddress()["address"]
        txid = bitcoind.rpc.sendtoaddress(addr, 0.01)
        bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 3)

    # Create a spend that will create a change output, sign and broadcast it.
    outpoints = [lianad.rpc.listcoins()["coins"][0]["outpoint"]]
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
    with pytest.raises(RpcError, match="Miniscript Error: could not satisfy at index 0"):
        signed_psbt = lianad.signer.sign_psbt(psbt, range(2))
        lianad.rpc.updatespend(signed_psbt.to_base64())
        lianad.rpc.broadcastspend(txid)
    # We can sign with different keys as long as there are 3 sigs
    signed_psbt = lianad.signer.sign_psbt(psbt, range(1, 4))
    lianad.rpc.updatespend(signed_psbt.to_base64())
    lianad.rpc.broadcastspend(txid)

    # Generate 10 blocks to test the recovery path
    bitcoind.generate_block(10, wait_for_mempool=txid)
    wait_for(
        lambda: lianad.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
    )


def test_multisig(lianad_multisig, bitcoind):
    """Test using lianad with a descriptor that contains multiple keys for both
    the primary and recovery paths."""
    receive_and_send(lianad_multisig, bitcoind)

    # Sweep all coins through the recovery path. It needs 2 signatures out of
    # 5 keys. Sign with the second and the fifth ones.
    res = lianad_multisig.rpc.createrecovery(bitcoind.rpc.getnewaddress(), 2)
    reco_psbt = PSBT.from_base64(res["psbt"])
    txid = reco_psbt.tx.txid().hex()
    signed_psbt = lianad_multisig.signer.sign_psbt(reco_psbt, {10: [1, 4]})
    lianad_multisig.rpc.updatespend(signed_psbt.to_base64())
    lianad_multisig.rpc.broadcastspend(txid)


def test_coinbase_deposit(lianad, bitcoind):
    """Check we detect deposits from (mature) coinbase transactions."""
    # Create a new deposit in a coinbase transaction.
    addr = lianad.rpc.getnewaddress()["address"]
    bitcoind.rpc.generatetoaddress(1, addr)
    assert len(lianad.rpc.listcoins()["coins"]) == 0

    # Generate 100 blocks to make the coinbase mature.
    bitcoind.generate_block(100)

    # We must have detected a new deposit.
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)
