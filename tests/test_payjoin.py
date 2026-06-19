"""Integration test for the Payjoin v2 receiver flow.

Drives lianad as the receiver and the upstream `payjoin-cli` binary as the
sender.  The test uses a *local* payjoin directory and OHTTP relay spun up
by the ``payjoin_services`` fixture (from ``payjoin-ffi`` with the
``_test-utils`` feature).  Network access is **not** required.

Run with:
    PAYJOIN_CLI_PATH=$(which payjoin-cli) pytest tests/test_payjoin.py
"""

import logging
import os
import subprocess

import pytest

from bip32.utils import coincurve

from fixtures import *
from test_framework.bitcoind import BitcoindRpcInterface
from test_framework.lianad import Lianad
from test_framework.serializations import (
    PSBT,
    PSBT_IN_BIP32_DERIVATION,
    PSBT_IN_PARTIAL_SIG,
    PSBT_IN_WITNESS_SCRIPT,
    sighash_all_witness,
)
from test_framework.signer import SingleSigner
from test_framework.utils import USE_TAPROOT, wait_for


PAYJOIN_CLI_PATH = os.getenv("PAYJOIN_CLI_PATH")
# Long enough to cover the v2 directory poll cadence + OHTTP roundtrips.
PAYJOIN_TIMEOUT = int(os.getenv("PAYJOIN_TIMEOUT", 60))
SENDER_WALLET = "payjoin-sender"


pytestmark = [
    pytest.mark.skipif(
        PAYJOIN_CLI_PATH is None,
        reason="payjoin-cli not configured (set PAYJOIN_CLI_PATH)",
    ),
    pytest.mark.skipif(
        USE_TAPROOT,
        reason="payjoin integration test only covers wsh descriptors for now",
    ),
]


def _sign_receiver_inputs(psbt, hd):
    """Sign in place the PSBT inputs the receiver owns.

    The payjoin PSBT contains the sender's external input(s) which we cannot
    (and must not) sign. We must sign in place on the full PSBT — segwit
    sighashes commit to hashPrevouts / hashSequence / hashOutputs over the
    full transaction, so signing a stripped copy would yield invalid sigs.
    Receiver-owned inputs are the ones lianad populated via `update_psbt_in`
    (witness script + our BIP32 derivation present).
    """
    signed_any = False
    for i, psbt_in in enumerate(psbt.i):
        if PSBT_IN_WITNESS_SCRIPT not in psbt_in.map:
            continue
        if PSBT_IN_BIP32_DERIVATION not in psbt_in.map:
            continue

        fing_der = next(iter(psbt_in.map[PSBT_IN_BIP32_DERIVATION].values()))
        raw_der_path = fing_der[4:]
        der_path = [
            int.from_bytes(raw_der_path[j : j + 4], byteorder="little", signed=True)
            for j in range(0, len(raw_der_path), 4)
        ]
        script_code = psbt_in.map[PSBT_IN_WITNESS_SCRIPT]
        sighash = sighash_all_witness(script_code, psbt, i)
        privkey = coincurve.PrivateKey(hd.get_privkey_from_path(der_path))
        pubkey = privkey.public_key.format()
        if pubkey not in psbt_in.map[PSBT_IN_BIP32_DERIVATION]:
            # Not one of our keys — leave it alone.
            continue
        sig = privkey.sign(sighash, hasher=None) + b"\x01"
        psbt_in.map.setdefault(PSBT_IN_PARTIAL_SIG, {})[pubkey] = sig
        signed_any = True

    assert signed_any, "no receiver-owned PSBT input found to sign"
    return psbt


def _payjoin_cli_args(datadir, bitcoind, wallet, payjoin_services):
    """Build the global payjoin-cli flags (everything before the subcommand)."""
    cookie = os.path.join(bitcoind.bitcoin_dir, "regtest", ".cookie")
    db_path = os.path.join(datadir, "payjoin.sqlite")
    rpchost = f"http://127.0.0.1:{bitcoind.rpcport}/wallet/{wallet}"
    args = [
        "--bip77",
        "-r",
        rpchost,
        "-c",
        cookie,
        "-d",
        db_path,
        "--pj-directory",
        payjoin_services["directory_url"],
        "--ohttp-relays",
        payjoin_services["ohttp_relay_url"],
    ]
    cert_path = payjoin_services.get("cert_path")
    if cert_path:
        args.extend(["--root-certificate", cert_path])
    return args


def _fund_lianad(lianad, bitcoind, amount_btc=0.5):
    addr = lianad.rpc.getnewaddress()["address"]
    txid = bitcoind.rpc.sendtoaddress(addr, amount_btc)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: len(lianad.rpc.listcoins(["confirmed"])["coins"]) >= 1)


def _fund_sender(bitcoind, wallet_rpc, amount_btc=1.0):
    addr = wallet_rpc.getnewaddress()
    txid = bitcoind.rpc.sendtoaddress(addr, amount_btc)
    bitcoind.generate_block(1, wait_for_mempool=txid)
    wait_for(lambda: wallet_rpc.getbalance() >= amount_btc)


def _new_sender_wallet(bitcoind):
    bitcoind.node_rpc.createwallet(SENDER_WALLET, False, False, "", False, True, True)
    return BitcoindRpcInterface(
        bitcoind.bitcoin_dir, "regtest", bitcoind.rpcport, wallet=SENDER_WALLET
    )


def test_payjoin_receive(payjoin_services, bitcoind, directory):
    """End-to-end payjoin receive against a local directory + ohttp relay."""

    # Create a lianad configured to use the local payjoin services.
    datadir = os.path.join(directory, "lianad")
    os.makedirs(datadir, exist_ok=True)

    signer = SingleSigner(is_taproot=USE_TAPROOT)
    (prim_fingerprint, primary_xpub), (reco_fingerprint, recovery_xpub) = (
        (xpub_fingerprint(signer.primary_hd), signer.primary_hd.get_xpub()),
        (xpub_fingerprint(signer.recovery_hd), signer.recovery_hd.get_xpub()),
    )
    csv_value = 10
    main_desc = single_key_desc(
        prim_fingerprint,
        primary_xpub,
        reco_fingerprint,
        recovery_xpub,
        csv_value,
        is_taproot=USE_TAPROOT,
    )

    from bip380.descriptors import Descriptor
    main_desc = Descriptor.from_str(main_desc)

    pj_config = {
        "ohttp_relays": [payjoin_services["ohttp_relay_url"]],
        "payjoin_directory": payjoin_services["directory_url"],
        "root_certificate": payjoin_services["cert_path"],
    }
    lianad = Lianad(datadir, signer, main_desc, bitcoind, payjoin_config=pj_config)

    try:
        lianad.start()

        # 1. Fund both wallets so each side can contribute an input.
        _fund_lianad(lianad, bitcoind)
        sender_rpc = _new_sender_wallet(bitcoind)
        _fund_sender(bitcoind, sender_rpc, amount_btc=1.0)

        # 2. Open a receiver session and grab the BIP21 to hand to the sender.
        res = lianad.rpc.receivepayjoin()
        bip21 = res["bip21"]
        assert bip21 and bip21.lower().startswith("bitcoin:")
        receiver_address = res["address"]
        receiver_derivation_index = res["derivation_index"]

        # The receiver-side library does not embed an amount in the BIP21 URI; the
        # sender (`payjoin-cli send`) requires one to build the original PSBT. Tack
        # it on as an extra query parameter — the URI always already has a `?pj=`
        # part so we use `&` here.
        send_amount_btc = 0.0001
        bip21 = f"{bip21}&amount={send_amount_btc}"

        # 3. Spawn payjoin-cli as the sender. `send` polls the directory until the
        #    receiver returns a finalized proposal, then broadcasts.
        cli_datadir = os.path.join(directory, "payjoin-cli")
        os.makedirs(cli_datadir, exist_ok=True)
        cli_args = _payjoin_cli_args(cli_datadir, bitcoind, SENDER_WALLET, payjoin_services)

        log_path = os.path.join(cli_datadir, "payjoin-cli.log")
        log_file = open(log_path, "w")
        cli = subprocess.Popen(
            [PAYJOIN_CLI_PATH, *cli_args, "send", bip21, "--fee-rate", "2"],
            cwd=cli_datadir,
            stdout=log_file,
            stderr=subprocess.STDOUT,
        )

        try:
            # 4. Wait for lianad to ingest the original payload, build the payjoin
            #    PSBT and store it in its spend DB.
            def _payjoin_psbt():
                spends = lianad.rpc.listspendtxs().get("spend_txs", [])
                return spends[0] if spends else None

            wait_for(lambda: _payjoin_psbt() is not None, timeout=PAYJOIN_TIMEOUT)
            spend_entry = _payjoin_psbt()
            psbt = PSBT.from_base64(spend_entry["psbt"])
            txid = psbt.tx.txid().hex()

            # Status should be `WaitingToSign` once the proposal is in DB.
            wait_for(
                lambda: lianad.rpc.getpayjoininfo(txid) == "WaitingToSign",
                timeout=PAYJOIN_TIMEOUT,
            )

            # 5. Sign the receiver's input(s) and persist the signed PSBT.
            signed_psbt = _sign_receiver_inputs(psbt, lianad.signer.primary_hd)
            lianad.rpc.updatespend(signed_psbt.to_base64())

            # 6. Send the proposal back to the sender via the directory.
            lianad.rpc.sendpayjoinproposal(txid)

            # 7. The sender should now broadcast the payjoin tx; wait for the
            #    mempool to see it.
            wait_for(
                lambda: txid in bitcoind.rpc.getrawmempool(),
                timeout=PAYJOIN_TIMEOUT,
                debug_fn=lambda: f"waiting for {txid} in mempool, have {bitcoind.rpc.getrawmempool()}",
            )
            bitcoind.generate_block(1, wait_for_mempool=txid)

            # 8. Receiver should now hold a confirmed coin from the payjoin tx.
            def _payjoin_coin():
                for c in lianad.rpc.listcoins(["confirmed"])["coins"]:
                    outpoint_txid = c["outpoint"].split(":")[0]
                    if outpoint_txid == txid and c["block_height"] is not None:
                        return c
                return None

            wait_for(
                lambda: _payjoin_coin() is not None,
                timeout=PAYJOIN_TIMEOUT,
                debug_fn=lambda: f"all coins: {lianad.rpc.listcoins()['coins']}",
            )
            coin = _payjoin_coin()
            assert coin["derivation_index"] == receiver_derivation_index, (
                coin["derivation_index"],
                receiver_derivation_index,
            )
            assert coin["address"] == receiver_address, (coin["address"], receiver_address)

            # 9. After `sendpayjoinproposal` the session is in the payjoin
            #    crate's `Monitor` state; the bitcoin poller drives
            #    `check_payment`, which walks Monitor -> Closed(Success) once
            #    the payjoin tx is visible to the wallet (it is, since step 8
            #    asserted the receiver-owned coin from this txid is confirmed).
            wait_for(
                lambda: lianad.rpc.getpayjoininfo(txid)
                not in ("Pending", "WaitingToSign", "ReadyToSend"),
                timeout=PAYJOIN_TIMEOUT,
                debug_fn=lambda: f"payjoin status: {lianad.rpc.getpayjoininfo(txid)}",
            )
        finally:
            if cli.poll() is None:
                cli.terminate()
                try:
                    cli.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    cli.kill()
            log_file.close()
            if cli.returncode not in (0, None):
                with open(log_path) as f:
                    logging.error("payjoin-cli output:\n%s", f.read())
    finally:
        lianad.cleanup()
