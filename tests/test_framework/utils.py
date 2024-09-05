import itertools
import json
import logging
import os
import re
import socket
import subprocess
import threading
import time

from io import BytesIO
from .serializations import CTransaction, PSBT

TIMEOUT = int(os.getenv("TIMEOUT", 20))
EXECUTOR_WORKERS = int(os.getenv("EXECUTOR_WORKERS", 5))
VERBOSE = os.getenv("VERBOSE", "0") == "1"
LOG_LEVEL = os.getenv("LOG_LEVEL", "debug")
assert LOG_LEVEL in ["trace", "debug", "info", "warn", "error"]
DEFAULT_MS_PATH = os.path.join(
    os.path.dirname(__file__), "..", "..", "target/debug/lianad"
)
LIANAD_PATH = os.getenv("LIANAD_PATH", DEFAULT_MS_PATH)
DEFAULT_BITCOIND_PATH = "bitcoind"
BITCOIND_PATH = os.getenv("BITCOIND_PATH", DEFAULT_BITCOIND_PATH)
OLD_LIANAD_PATH = os.getenv("OLD_LIANAD_PATH", None)
IS_NOT_BITCOIND_24 = bool(int(os.getenv("IS_NOT_BITCOIND_24", True)))
USE_TAPROOT = bool(
    int(os.getenv("USE_TAPROOT", False))
)  # TODO: switch to True in a couple releases.


COIN = 10**8


def wait_for(success, timeout=TIMEOUT, debug_fn=None):
    """
    Run success() either until it returns True, or until the timeout is reached.
    debug_fn is logged at each call to success, it can be useful for debugging
    when tests fail.
    """
    wait_for_while_condition_holds(success, lambda: True, timeout, debug_fn)


def wait_for_while_condition_holds(success, condition, timeout=TIMEOUT, debug_fn=None):
    """
    Run success() either until it returns True, or until the timeout is reached,
    as long as condition() holds.
    debug_fn is logged at each call to success, it can be useful for debugging
    when tests fail.
    """
    start_time = time.time()
    interval = 0.25
    while True:
        if time.time() >= start_time + timeout:
            raise ValueError("Error waiting for {}", success)
        if not condition():
            raise ValueError(
                "Condition {} not met while waiting for {}", condition, success
            )
        if success():
            return
        if debug_fn is not None:
            logging.info(debug_fn())
        time.sleep(interval)
        interval *= 2
        if interval > 5:
            interval = 5


def get_txid(hex_tx):
    """Get the txid (as hex) of the given (as hex) transaction."""
    tx = CTransaction()
    tx.deserialize(BytesIO(bytes.fromhex(hex_tx)))
    return tx.txid().hex()


def sign_and_broadcast(lianad, bitcoind, psbt, recovery=False):
    """Sign a PSBT, finalize it, extract the transaction and broadcast it."""
    signed_psbt = lianad.signer.sign_psbt(psbt, recovery)
    # Under Taproot i didn't bother implementing a finalizer in the test suite.
    if USE_TAPROOT:
        lianad.rpc.updatespend(signed_psbt.to_base64())
        txid = signed_psbt.tx.txid().hex()
        lianad.rpc.broadcastspend(txid)
        lianad.rpc.delspendtx(txid)
        return txid
    finalized_psbt = lianad.finalize_psbt(signed_psbt)
    tx = finalized_psbt.tx.serialize_with_witness().hex()
    return bitcoind.rpc.sendrawtransaction(tx)


def spend_coins(lianad, bitcoind, coins):
    """Spend these coins, no matter how.
    This will create a single transaction spending them all at once at the minimum
    feerate. This will broadcast but not confirm the transaction.

    :param coins: a list of dict as returned by listcoins. The coins must all exist.
    :returns: the broadcasted transaction, as hex.
    """
    total_value = sum(c["amount"] for c in coins)
    destinations = {
        bitcoind.rpc.getnewaddress(): total_value - 11 - 31 - 300 * len(coins)
    }
    res = lianad.rpc.createspend(destinations, [c["outpoint"] for c in coins], 1)
    txid = sign_and_broadcast(lianad, bitcoind, PSBT.from_base64(res["psbt"]))
    return bitcoind.rpc.getrawtransaction(txid)


def sign_and_broadcast_psbt(lianad, psbt):
    """Sign a PSBT, save it to the DB and broadcast it."""
    txid = psbt.tx.txid().hex()
    psbt = lianad.signer.sign_psbt(psbt)
    lianad.rpc.updatespend(psbt.to_base64())
    lianad.rpc.broadcastspend(txid)
    return txid


class RpcError(ValueError):
    def __init__(self, method: str, params: dict, error: str):
        super(ValueError, self).__init__(
            "RPC call failed: method: {}, params: {}, error: {}".format(
                method, params, error
            )
        )

        self.method = method
        self.params = params
        self.error = error


class UnixSocket(object):
    """A wrapper for socket.socket that is specialized to unix sockets.

    Some OS implementations impose restrictions on the Unix sockets.

     - On linux OSs the socket path must be shorter than the in-kernel buffer
       size (somewhere around 100 bytes), thus long paths may end up failing
       the `socket.connect` call.

    This is a small wrapper that tries to work around these limitations.

    """

    def __init__(self, path: str):
        self.path = path
        self.sock = None
        self.connect()

    def connect(self) -> None:
        try:
            self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            self.sock.connect(self.path)
            self.sock.settimeout(TIMEOUT)
        except OSError as e:
            self.close()

            if e.args[0] == "AF_UNIX path too long" and os.uname()[0] == "Linux":
                # If this is a Linux system we may be able to work around this
                # issue by opening our directory and using `/proc/self/fd/` to
                # get a short alias for the socket file.
                #
                # This was heavily inspired by the Open vSwitch code see here:
                # https://github.com/openvswitch/ovs/blob/master/python/ovs/socket_util.py

                dirname = os.path.dirname(self.path)
                basename = os.path.basename(self.path)

                # Open an fd to our home directory, that we can then find
                # through `/proc/self/fd` and access the contents.
                dirfd = os.open(dirname, os.O_DIRECTORY | os.O_RDONLY)
                short_path = "/proc/self/fd/%d/%s" % (dirfd, basename)
                self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                self.sock.connect(short_path)
            else:
                # There is no good way to recover from this.
                raise

    def close(self) -> None:
        if self.sock is not None:
            self.sock.close()
        self.sock = None

    def sendall(self, b: bytes) -> None:
        if self.sock is None:
            raise socket.error("not connected")

        self.sock.sendall(b)

    def recv(self, length: int) -> bytes:
        if self.sock is None:
            raise socket.error("not connected")

        return self.sock.recv(length)

    def __del__(self) -> None:
        self.close()


class UnixDomainSocketRpc(object):
    def __init__(self, socket_path, logger=logging):
        self.socket_path = socket_path
        self.logger = logger
        self.next_id = 0

    def _readobj(self, sock):
        """Read a JSON object"""
        buff = b""
        while True:
            n_to_read = max(2048, len(buff))
            chunk = sock.recv(n_to_read)
            buff += chunk
            if len(chunk) != n_to_read:
                try:
                    return json.loads(buff)
                except json.JSONDecodeError:
                    # There is more to read, continue
                    # FIXME: this is a workaround for large reads taken from lianad.
                    # We should use the '\n' marker instead since lianad uses that.
                    continue

    def __getattr__(self, name):
        """Intercept any call that is not explicitly defined and call @call.

        We might still want to define the actual methods in the subclasses for
        documentation purposes.
        """

        def wrapper(*args, **kwargs):
            if len(args) != 0 and len(kwargs) != 0:
                raise RpcError(
                    name, {}, "Cannot mix positional and non-positional arguments"
                )
            return self.call(name, params=args or kwargs)

        return wrapper

    def call(self, method, params={}):
        self.logger.debug(f"Calling {method} with params {params}")

        # FIXME: we open a new socket for every readobj call...
        sock = UnixSocket(self.socket_path)
        msg = json.dumps(
            {
                "jsonrpc": "2.0",
                "id": 0,
                "method": method,
                "params": params,
            }
        )
        sock.sendall(msg.encode() + b"\n")
        this_id = self.next_id
        resp = self._readobj(sock)

        self.logger.debug(f"Received response for {method} call: {resp}")
        if "id" in resp and resp["id"] != this_id:
            raise ValueError(
                "Malformed response, id is not {}: {}.".format(this_id, resp)
            )
        sock.close()

        if not isinstance(resp, dict):
            raise ValueError(
                f"Malformed response, response is not a dictionary: {resp}"
            )
        elif "error" in resp:
            raise RpcError(method, params, resp["error"])
        elif "result" not in resp:
            raise ValueError('Malformed response, "result" missing.')
        return resp["result"]


class TailableProc(object):
    """A monitorable process that we can start, stop and tail.

    This is the base class for the daemons. It allows us to directly
    tail the processes and react to their output.
    """

    def __init__(self, outputDir=None, verbose=True):
        self.logs = []
        self.logs_cond = threading.Condition(threading.RLock())
        self.env = os.environ.copy()
        self.running = False
        self.proc = None
        self.outputDir = outputDir
        self.logsearch_start = 0

        # Set by inherited classes
        self.cmd_line = []
        self.prefix = ""

        # Should we be logging lines we read from stdout?
        self.verbose = verbose

        # A filter function that'll tell us whether to filter out the line (not
        # pass it to the log matcher and not print it to stdout).
        self.log_filter = lambda _: False

    def start(self, stdin=None, stdout=None, stderr=None):
        """Start the underlying process and start monitoring it."""
        logging.debug("Starting '%s'", " ".join(self.cmd_line))
        self.proc = subprocess.Popen(
            self.cmd_line,
            stdin=stdin,
            stdout=stdout if stdout else subprocess.PIPE,
            stderr=stderr if stderr else subprocess.STDOUT,
            env=self.env,
        )
        self.thread = threading.Thread(target=self.tail)
        self.thread.daemon = True
        self.thread.start()
        self.running = True

    def save_log(self):
        if self.outputDir:
            logpath = os.path.join(self.outputDir, "log")
            with open(logpath, "w") as f:
                for l in self.logs:
                    f.write(l + "\n")

    def stop(self, timeout=10):
        self.save_log()
        self.proc.terminate()

        # Now give it some time to react to the signal
        rc = self.proc.wait(timeout)

        if rc is None:
            self.proc.kill()
            self.proc.wait()

        self.thread.join()

        return self.proc.returncode

    def kill(self):
        """Kill process without giving it warning."""
        self.proc.kill()
        self.proc.wait()
        self.thread.join()

    def tail(self):
        """Tail the stdout of the process and remember it.

        Stores the lines of output produced by the process in
        self.logs and signals that a new line was read so that it can
        be picked up by consumers.
        """
        out = self.proc.stdout.readline
        err = self.proc.stderr.readline if self.proc.stderr else lambda: ""
        for line in itertools.chain(iter(out, ""), iter(err, "")):
            if len(line) == 0:
                break
            if self.log_filter(line.decode("utf-8")):
                continue
            if self.verbose:
                logging.debug(f"{self.prefix}: {line.decode().rstrip()}")
            with self.logs_cond:
                self.logs.append(str(line.rstrip()))
                self.logs_cond.notifyAll()
        self.running = False
        self.proc.stdout.close()
        if self.proc.stderr is not None:
            self.proc.stderr.close()

    def is_in_log(self, regex, start=0):
        """Look for `regex` in the logs."""

        ex = re.compile(regex)
        for l in self.logs[start:]:
            if ex.search(l):
                logging.debug("Found '%s' in logs", regex)
                return l

        logging.debug(f"{self.prefix} : Did not find {regex} in logs")
        return None

    def wait_for_logs(self, regexs, timeout=TIMEOUT):
        """Look for `regexs` in the logs.

        We tail the stdout of the process and look for each regex in `regexs`,
        starting from last of the previous waited-for log entries (if any).  We
        fail if the timeout is exceeded or if the underlying process
        exits before all the `regexs` were found.

        If timeout is None, no time-out is applied.
        """
        logging.debug("Waiting for {} in the logs".format(regexs))

        exs = [re.compile(r) for r in regexs]
        start_time = time.time()
        pos = self.logsearch_start

        while True:
            if timeout is not None and time.time() > start_time + timeout:
                print("Time-out: can't find {} in logs".format(exs))
                for r in exs:
                    if self.is_in_log(r):
                        print("({} was previously in logs!)".format(r))
                raise TimeoutError('Unable to find "{}" in logs.'.format(exs))

            with self.logs_cond:
                if pos >= len(self.logs):
                    if not self.running:
                        raise ValueError("Process died while waiting for logs")
                    self.logs_cond.wait(1)
                    continue

                for r in exs.copy():
                    self.logsearch_start = pos + 1
                    if r.search(self.logs[pos]):
                        logging.debug("Found '%s' in logs", r)
                        exs.remove(r)
                        break
                if len(exs) == 0:
                    return self.logs[pos]
                pos += 1

    def wait_for_log(self, regex, timeout=TIMEOUT):
        """Look for `regex` in the logs.

        Convenience wrapper for the common case of only seeking a single entry.
        """
        return self.wait_for_logs([regex], timeout)
