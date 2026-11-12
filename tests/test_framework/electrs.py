import json
import logging
import os
import socket

from ephemeral_port_reserve import reserve
from test_framework.utils import (
    BitcoinBackend,
    TailableProc,
    ELECTRS_PATH,
    TIMEOUT,
    wait_for,
)


class Electrs(BitcoinBackend):
    def __init__(
        self,
        bitcoind_dir,
        bitcoind_rpcport,
        bitcoind_p2pport,
        electrs_dir,
        rpcport=None,
    ):
        TailableProc.__init__(self, electrs_dir, verbose=False)

        if rpcport is None:
            rpcport = reserve()

        # Prometheus metrics can't be deactivated in Electrs. Configure the port so it doesn't
        # conflict with other instances when running tests in parallel.
        monitoring_port = reserve()

        self.electrs_dir = electrs_dir
        self.rpcport = rpcport
        self.use_rest = os.getenv("ELECTRS_USE_REST") == "1"

        regtestdir = os.path.join(electrs_dir, "regtest")
        if not os.path.exists(regtestdir):
            os.makedirs(regtestdir)

        self.cmd_line = [
            ELECTRS_PATH,
            "--conf",
            "{}/electrs.toml".format(regtestdir),
        ]
        electrs_conf = {
            "daemon_dir": bitcoind_dir,
            "cookie_file": os.path.join(bitcoind_dir, "regtest", ".cookie"),
            "daemon_rpc_addr": f"127.0.0.1:{bitcoind_rpcport}",
            "db_dir": electrs_dir,
            "network": "regtest",
            "electrum_rpc_addr": f"127.0.0.1:{self.rpcport}",
            "monitoring_addr": f"127.0.0.1:{monitoring_port}",
        }
        if not self.use_rest:
            electrs_conf["daemon_p2p_addr"] = f"127.0.0.1:{bitcoind_p2pport}"
        self.conf_file = os.path.join(regtestdir, "electrs.toml")
        with open(self.conf_file, "w") as f:
            for k, v in electrs_conf.items():
                f.write(f'{k} = "{v}"\n')

        self.env = {"RUST_LOG": "DEBUG"}

    def has_indexed_blocks(self):
        try:
            with socket.create_connection(("127.0.0.1", self.rpcport), timeout=1) as peer:
                peer.sendall(
                    b'{"id":1,"method":"blockchain.headers.subscribe","params":[]}\n'
                )
                with peer.makefile("rb") as response:
                    result = json.loads(response.readline()).get("result") or {}
                    return result.get("height", 0) > 0
        except (OSError, ValueError):
            return False

    def start(self):
        TailableProc.start(self)
        if self.use_rest:
            wait_for(self.has_indexed_blocks)
        else:
            self.wait_for_log("auto-compactions enabled", timeout=TIMEOUT)
        logging.info("Electrs started")

    def startup(self):
        try:
            self.start()
        except Exception:
            self.stop()
            raise

    def stop(self):
        return TailableProc.stop(self)

    def cleanup(self):
        try:
            self.stop()
        except Exception:
            self.proc.kill()
        self.proc.wait()

    def append_to_lianad_conf(self, conf_file):
        with open(conf_file, "a") as f:
            f.write("[electrum_config]\n")
            f.write(f"addr = '127.0.0.1:{self.rpcport}'\n")
