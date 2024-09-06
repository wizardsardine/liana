import logging
import os

from ephemeral_port_reserve import reserve
from test_framework.utils import BitcoinBackend, TailableProc, ELECTRS_PATH, TIMEOUT


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
            "daemon_p2p_addr": f"127.0.0.1:{bitcoind_p2pport}",
            "db_dir": electrs_dir,
            "network": "regtest",
            "electrum_rpc_addr": f"127.0.0.1:{self.rpcport}",
            "monitoring_addr": f"127.0.0.1:{monitoring_port}",
        }
        self.conf_file = os.path.join(regtestdir, "electrs.toml")
        with open(self.conf_file, "w") as f:
            for k, v in electrs_conf.items():
                f.write(f'{k} = "{v}"\n')

        self.env = {"RUST_LOG": "DEBUG"}

    def start(self):
        TailableProc.start(self)
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
