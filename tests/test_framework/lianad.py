import logging
import os
import shutil

from bip380.descriptors import Descriptor
from bip380.miniscript import SatisfactionMaterial
from test_framework.utils import (
    BITCOIN_BACKEND_TYPE,
    BitcoinBackendType,
    UnixDomainSocketRpc,
    TailableProc,
    VERBOSE,
    LOG_LEVEL,
    LIANAD_PATH,
    wait_for,
)
from test_framework.serializations import (
    PSBT,
    CTxInWitness,
    CScriptWitness,
    PSBT_IN_BIP32_DERIVATION,
    PSBT_IN_PARTIAL_SIG,
    PSBT_IN_FINAL_SCRIPTWITNESS,
)


class Lianad(TailableProc):
    def __init__(
        self, datadir, signer, multi_desc, bitcoin_backend, legacy_datadir=False
    ):
        TailableProc.__init__(self, datadir, verbose=VERBOSE)

        self.datadir = datadir
        self.prefix = os.path.split(datadir)[-1]

        self.signer = signer
        self._poll_interval_secs = 1
        self.multi_desc = multi_desc
        self.receive_desc, self.change_desc = multi_desc.singlepath_descriptors()

        self.conf_file = os.path.join(datadir, "config.toml")
        self.cmd_line = [LIANAD_PATH, "--conf", f"{self.conf_file}"]
        data_directory = os.path.join(datadir, "regtest")
        socket_path = os.path.join(data_directory, "lianad_rpc")
        self.rpc = UnixDomainSocketRpc(socket_path)
        self.bitcoin_backend = bitcoin_backend

        with open(self.conf_file, "w") as f:
            if legacy_datadir:
                f.write(f"data_dir = '{datadir}'\n")
            else:
                f.write(f"data_directory = '{data_directory}'\n")

            f.write(f"log_level = '{LOG_LEVEL}'\n")

            f.write(f'main_descriptor = "{multi_desc}"\n')

            f.write("[bitcoin_config]\n")
            f.write('network = "regtest"\n')
            f.write(f"poll_interval_secs = {self._poll_interval_secs}\n")
        bitcoin_backend.append_to_lianad_conf(self.conf_file)

    @property
    def poll_interval_secs(self):
        """Return the poll interval in seconds as defined in the config file."""
        return self._poll_interval_secs

    def finalize_psbt(self, psbt):
        """Create a valid witness for all inputs in the PSBT.
        This will fail if the PSBT input does not contain enough material.

        :param psbt: PSBT of the transaction to be finalized.
        :returns: PSBT with finalized inputs.
        """
        assert isinstance(psbt, PSBT)

        # Create a witness for each input of the transaction.
        for i, psbt_in in enumerate(psbt.i):
            # First, gather the needed information from the PSBT input.
            # 'hd_keypaths' is of the form {pubkey: (fingerprint, derivation index)}
            fing_der = next(iter(psbt_in.map[PSBT_IN_BIP32_DERIVATION].values()))
            raw_der_path = fing_der[4:]
            der_path = [
                int.from_bytes(raw_der_path[i : i + 4], byteorder="little", signed=True)
                for i in range(0, len(raw_der_path), 4)
            ]
            assert len(der_path) == 2

            # Create a copy of the descriptor to derive it at the index used in this input.
            # Then create a satisfaction for it using the signature we just created.
            desc = Descriptor.from_str(
                str(self.receive_desc if der_path[0] == 0 else self.change_desc)
            )
            desc.derive(der_path[1])
            sat_material = SatisfactionMaterial(
                signatures=psbt_in.map[PSBT_IN_PARTIAL_SIG],
            )
            stack = desc.satisfy(sat_material)
            logging.debug(f"Satisfaction for {desc} is {[e.hex() for e in stack]}")

            # Update the transaction inside the PSBT directly.
            assert stack is not None
            psbt_in.map[PSBT_IN_FINAL_SCRIPTWITNESS] = CTxInWitness(
                CScriptWitness(stack)
            )
            psbt.tx.wit.vtxinwit.append(psbt_in.map[PSBT_IN_FINAL_SCRIPTWITNESS])

        return psbt

    def restart_fresh(self, bitcoind):
        """Delete the internal state of the wallet and restart."""
        self.stop()
        dir_path = os.path.join(self.datadir, "regtest")
        shutil.rmtree(dir_path)
        if BITCOIN_BACKEND_TYPE is BitcoinBackendType.Bitcoind:
            wallet_path = os.path.join(dir_path, "lianad_watchonly_wallet")
            bitcoind.node_rpc.unloadwallet(wallet_path)
        self.start()
        wait_for(
            lambda: self.rpc.getinfo()["block_height"] == bitcoind.rpc.getblockcount()
        )

    def start(self):
        TailableProc.start(self)
        self.wait_for_logs(
            [
                "Database initialized and checked",
                "JSONRPC server started.",
            ]
        )

    def stop(self, timeout=5):
        try:
            self.rpc.stop()
            self.wait_for_log(
                "Stopping the liana daemon.",
            )
            self.proc.wait(timeout)
        except Exception as e:
            logging.error(f"{self.prefix} : error when calling stop: '{e}'")
        return TailableProc.stop(self)

    def cleanup(self):
        try:
            self.stop()
        except Exception:
            self.proc.kill()
