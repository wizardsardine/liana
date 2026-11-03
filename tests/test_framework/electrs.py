import json
import logging
import os
import socket
import socketserver
import threading

from ephemeral_port_reserve import reserve
from io import BytesIO
from test_framework.serializations import CTransaction, sha256
from test_framework.utils import (
    BitcoinBackend,
    TailableProc,
    ELECTRS_PATH,
    TIMEOUT,
    get_txid,
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


class ProxyHandler(socketserver.StreamRequestHandler):
    def handle(self):
        self.server.electrs.proxy_connection(self.rfile, self.wfile)


class MaliciousElectrs(Electrs):
    """Electrs behind a proxy that can replace one of our transactions by a forged one.

    Once `forge_spend` is called, the proxy pretends to broadcast the dropped
    transaction and serves the forged one as unconfirmed in the history of every
    script it touches.
    """

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.forgery = None
        self.proxy_server = socketserver.ThreadingTCPServer(
            ("127.0.0.1", 0), ProxyHandler
        )
        self.proxy_server.daemon_threads = True
        self.proxy_server.electrs = self
        threading.Thread(target=self.proxy_server.serve_forever, daemon=True).start()

    def stop(self):
        self.proxy_server.shutdown()
        self.proxy_server.server_close()
        return super().stop()

    def append_to_lianad_conf(self, conf_file):
        with open(conf_file, "a") as f:
            f.write("[electrum_config]\n")
            f.write(f"addr = '127.0.0.1:{self.proxy_server.server_address[1]}'\n")

    def forge_spend(self, dropped_txid, forged_tx):
        scripts = [txout.scriptPubKey for txout in forged_tx.vout]
        with socket.create_connection(("127.0.0.1", self.rpcport)) as upstream:
            upstream_r = upstream.makefile("rb")
            for txin in forged_tx.vin:
                request = {
                    "jsonrpc": "2.0",
                    "id": 0,
                    "method": "blockchain.transaction.get",
                    "params": ["%064x" % txin.prevout.hash],
                }
                upstream.sendall(json.dumps(request).encode() + b"\n")
                prev_tx = CTransaction()
                prev_tx.deserialize(
                    BytesIO(bytes.fromhex(json.loads(upstream_r.readline())["result"]))
                )
                scripts.append(prev_tx.vout[txin.prevout.n].scriptPubKey)
        self.forgery = {
            "dropped_txid": dropped_txid,
            "tx": forged_tx,
            "txid": forged_tx.txid().hex(),
            "scripthashes": {sha256(spk)[::-1].hex() for spk in scripts},
        }

    def proxy_connection(self, client_r, client_w):
        with socket.create_connection(("127.0.0.1", self.rpcport)) as upstream:
            upstream_r = upstream.makefile("rb")
            for line in client_r:
                request = json.loads(line)
                if isinstance(request, list):
                    reply = self.handle_batch(request, upstream, upstream_r, client_w)
                else:
                    reply = self.handle_batch(
                        [request], upstream, upstream_r, client_w
                    )[0]
                client_w.write(json.dumps(reply).encode() + b"\n")
                client_w.flush()

    def handle_batch(self, requests, upstream, upstream_r, client_w):
        replies = {}
        forwarded = {}
        for request in requests:
            result = self.forged_result(request)
            if result is None:
                forwarded[request["id"]] = request
            else:
                replies[request["id"]] = {
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": result,
                }
        if forwarded:
            upstream.sendall(json.dumps(list(forwarded.values())).encode() + b"\n")
            forgery = self.forgery
            for reply in self.read_upstream_reply(upstream_r, client_w):
                request = forwarded[reply["id"]]
                if (
                    forgery is not None
                    and request["method"] == "blockchain.scripthash.get_history"
                    and request["params"][0] in forgery["scripthashes"]
                    and "result" in reply
                ):
                    reply["result"].append({"tx_hash": forgery["txid"], "height": 0})
                replies[reply["id"]] = reply
        return [replies[request["id"]] for request in requests]

    def forged_result(self, request):
        """The reply to a request the proxy answers itself, or None to forward it."""
        forgery = self.forgery
        if forgery is None:
            return None
        params = request.get("params", [])
        if (
            request["method"] == "blockchain.transaction.broadcast"
            and get_txid(params[0]) == forgery["dropped_txid"]
        ):
            return forgery["dropped_txid"]
        if (
            request["method"] == "blockchain.transaction.get"
            and params[0] == forgery["txid"]
        ):
            return forgery["tx"].serialize().hex()
        return None

    def read_upstream_reply(self, upstream_r, client_w):
        """Read the next upstream reply, relaying notifications received meanwhile."""
        for line in upstream_r:
            message = json.loads(line)
            if isinstance(message, dict) and "method" in message:
                client_w.write(line)
                client_w.flush()
            else:
                return message
        raise ConnectionError("Electrs closed the connection")
