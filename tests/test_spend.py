from fixtures import *
from test_framework.serializations import PSBT
from test_framework.utils import wait_for


def test_spend_change(lianad, bitcoind):
    """We can spend a coin that was received on a change address."""
    # Receive a coin on a receive address
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, 0.01)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 1)

    # Create a transaction that will spend this coin to 1) one of our receive
    # addresses 2) an external address 3) one of our change addresses.
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins()["coins"]]
    destinations = {
        bitcoind.rpc.getnewaddress(): 100_000,
        lianad.rpc.getnewaddress()["address"]: 100_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 2)
    assert "psbt" in res

    # The transaction must contain a change output.
    spend_psbt = PSBT.from_base64(res["psbt"])
    assert len(spend_psbt.o) == 3
    assert len(spend_psbt.tx.vout) == 3

    # Sign and broadcast this first Spend transaction.
    signed_psbt = lianad.sign_psbt(spend_psbt)
    lianad.rpc.updatespend(signed_psbt.to_base64())
    spend_txid = signed_psbt.tx.txid().hex()
    lianad.rpc.broadcastspend(spend_txid)
    bitcoind.generate_block(1, wait_for_mempool=spend_txid)
    wait_for(lambda: len(lianad.rpc.listcoins()["coins"]) == 3)

    # Now create a new transaction that spends the change output as well as
    # the output sent to the receive address.
    outpoints = [
        c["outpoint"]
        for c in lianad.rpc.listcoins()["coins"]
        if c["spend_info"] is None
    ]
    destinations = {
        bitcoind.rpc.getnewaddress(): 100_000,
    }
    res = lianad.rpc.createspend(destinations, outpoints, 2)
    spend_psbt = PSBT.from_base64(res["psbt"])

    # We can sign and broadcast it.
    signed_psbt = lianad.sign_psbt(spend_psbt)
    lianad.rpc.updatespend(signed_psbt.to_base64())
    spend_txid = signed_psbt.tx.txid().hex()
    lianad.rpc.broadcastspend(spend_txid)
    bitcoind.generate_block(1, wait_for_mempool=spend_txid)
