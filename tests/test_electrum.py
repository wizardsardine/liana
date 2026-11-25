import os
import pytest

from fixtures import bitcoind, directory, lianad, test_base_dir, test_name
from test_framework.electrs import MaliciousElectrs
from test_framework.serializations import CTransaction, PSBT
from test_framework.utils import (
    BITCOIN_BACKEND_TYPE,
    BitcoinBackendType,
    RpcError,
    sign_and_broadcast_psbt,
    wait_for,
)

pytestmark = pytest.mark.skipif(
    BITCOIN_BACKEND_TYPE is not BitcoinBackendType.Electrs,
    reason="Needs an Electrum backend",
)


@pytest.fixture
def bitcoin_backend(directory, bitcoind):
    electrs = MaliciousElectrs(
        electrs_dir=os.path.join(directory, "electrs"),
        bitcoind_dir=bitcoind.bitcoin_dir,
        bitcoind_rpcport=bitcoind.rpcport,
        bitcoind_p2pport=bitcoind.p2pport,
    )
    electrs.startup()
    yield electrs
    electrs.cleanup()


def test_forged_spend_from_server(lianad, bitcoind, bitcoin_backend):
    """A spend of our coins forged by the Electrum server must be ignored."""
    # Fund the wallet with two coins from bitcoind's wallet.
    destinations = {
        lianad.rpc.getnewaddress()["address"]: 0.003,
        lianad.rpc.getnewaddress()["address"]: 0.004,
    }
    txid = bitcoind.rpc.sendmany("", destinations)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) == 2)
    outpoints = [c["outpoint"] for c in lianad.rpc.listcoins(["confirmed"])["coins"]]

    # Our real payment: both coins to an external recipient, the rest as change.
    recipient = bitcoind.rpc.getnewaddress()
    recipient_spk = bytes.fromhex(
        bitcoind.rpc.getaddressinfo(recipient)["scriptPubKey"]
    )
    res = lianad.rpc.createspend({recipient: 650_000}, outpoints, 1)
    psbt = PSBT.from_base64(res["psbt"])

    # Same inputs and change as our payment, unsigned, paying the attacker instead.
    # Witnesses are not part of the txid, so the forgery has a regular txid.
    attacker_spk = bytes.fromhex(
        bitcoind.rpc.getaddressinfo(bitcoind.rpc.getnewaddress())["scriptPubKey"]
    )
    forged_tx = CTransaction(psbt.tx)
    for txout in forged_tx.vout:
        if txout.scriptPubKey == recipient_spk:
            txout.scriptPubKey = attacker_spk
    forged_tx.rehash()
    forged_txid = forged_tx.txid().hex()
    bitcoin_backend.forge_spend(psbt.tx.txid().hex(), forged_tx)

    # The server acks our broadcast but drops it, then serves the forgery instead.
    # lianad must reject it and stop polling: our coins stay unspent, so a new payment
    # would reuse them and could not pay twice, and there is no forged payment to bump.
    sign_and_broadcast_psbt(lianad, psbt)
    lianad.wait_for_log(
        f"Error syncing wallet: 'Electrum server sent transaction '{forged_txid}'.*No longer polling."
    )

    # Any request to the backend now fails with the disconnection error.
    res = lianad.rpc.createspend({recipient: 650_000}, outpoints, 1)
    new_psbt = PSBT.from_base64(res["psbt"])
    new_txid = new_psbt.tx.txid().hex()
    lianad.rpc.updatespend(lianad.signer.sign_psbt(new_psbt).to_base64())
    with pytest.raises(RpcError, match="Electrum client is disconnected"):
        lianad.rpc.broadcastspend(new_txid)

    assert bitcoind.rpc.getrawmempool() == []
    coins = lianad.rpc.listcoins(["confirmed"], outpoints)["coins"]
    assert len(coins) == 2 and all(c["spend_info"] is None for c in coins)
    for rbf_args in [(forged_txid, False, 10), (forged_txid, True)]:
        with pytest.raises(
            RpcError, match=f"Unknown spend transaction '{forged_txid}'."
        ):
            lianad.rpc.rbfpsbt(*rbf_args)
