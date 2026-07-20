import logging
import os
import re
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
    COINCUBED_PATH,
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


class Coincubed(TailableProc):
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
        self.cmd_line = [COINCUBED_PATH, "--conf", f"{self.conf_file}"]
        data_directory = os.path.join(datadir, "regtest")
        socket_path = os.path.join(data_directory, "coincubed_rpc")
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
        bitcoin_backend.append_to_coincubed_conf(self.conf_file)

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
            wallet_path = os.path.join(dir_path, "coincubed_watchonly_wallet")
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
        self._sync_rpc_socket_path()

    def _sync_rpc_socket_path(self):
        """Point the RPC client at the control socket the daemon actually bound.

        Since coincubed 29f100ca the socket is no longer at
        ``{data_directory}/coincubed_rpc``: to stay under the 104-byte
        ``sun_path`` limit (notably on macOS) the daemon hashes the data
        directory and binds at ``{tmpdir}/cc<hash>.sock`` (see
        ``coincubed_rpc_socket_path`` in ``coincubed/src/datadir.rs``). Rather
        than replicate Rust's hashing in Python, read the real path straight
        from the daemon's own startup log — it logs ``Binding socket at
        <path>`` (debug) immediately before the ``JSONRPC server started.``
        line we just waited on, so the entry is already captured. Older daemons
        that bound at the legacy path don't emit this line, so keep the
        ``{data_directory}/coincubed_rpc`` path set in ``__init__`` as a
        fallback.

        NB: ``TailableProc.tail`` stores each log line as ``str(bytes)`` — the
        *repr* of the raw stdout bytes, e.g. ``b'... Binding socket at
        /tmp/cc<hash>.sock'`` — so a bare ``(.+)`` capture would swallow the
        trailing repr quote and connect to a bogus path. Anchor the capture on
        the ``.sock`` suffix the daemon always uses (see
        ``coincubed_rpc_socket_path`` in ``coincubed/src/datadir.rs``); this
        ends the match before any trailing quote and works whether the stored
        line is repr-wrapped or a plain decoded string.
        """
        pattern = r"Binding socket at (.+\.sock)"
        line = self.is_in_log(pattern)
        if line is None:
            return
        socket_path = re.search(pattern, line).group(1)
        self.rpc = UnixDomainSocketRpc(socket_path)

    def stop(self, timeout=5):
        try:
            self.rpc.stop()
            self.wait_for_log(
                "Stopping the coincube daemon.",
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
