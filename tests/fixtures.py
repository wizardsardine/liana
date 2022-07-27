from concurrent import futures
from ephemeral_port_reserve import reserve
from test_framework.bitcoind import Bitcoind
from test_framework.minisafed import Minisafed
from test_framework.utils import (
    EXECUTOR_WORKERS,
)

import os
import pytest
import shutil
import tempfile
import time


# A dict in which we count how often a particular test has run so far. Used to
# give each attempt its own numbered directory, and avoid clashes.
ATTEMPTS = {}


@pytest.fixture(scope="session")
def test_base_dir():
    d = os.getenv("TEST_DIR", "/tmp")

    directory = tempfile.mkdtemp(prefix="minisafed-tests-", dir=d)
    print("Running tests in {}".format(directory))

    yield directory

    content = os.listdir(directory)
    if content == []:
        shutil.rmtree(directory)
    else:
        print(f"Leaving base dir '{directory}' as it still contains {content}")


# Taken from https://docs.pytest.org/en/latest/example/simple.html#making-test-result-information-available-in-fixtures
@pytest.hookimpl(tryfirst=True, hookwrapper=True)
def pytest_runtest_makereport(item, call):
    # execute all other hooks to obtain the report object
    outcome = yield
    rep = outcome.get_result()

    # set a report attribute for each phase of a call, which can
    # be "setup", "call", "teardown"

    setattr(item, "rep_" + rep.when, rep)


@pytest.fixture
def directory(request, test_base_dir, test_name):
    """Return a per-test specific directory.

    This makes a unique test-directory even if a test is rerun multiple times.

    """
    global ATTEMPTS
    # Auto set value if it isn't in the dict yet
    ATTEMPTS[test_name] = ATTEMPTS.get(test_name, 0) + 1
    directory = os.path.join(
        test_base_dir, "{}_{}".format(test_name, ATTEMPTS[test_name])
    )

    if not os.path.exists(directory):
        os.makedirs(directory)

    yield directory

    # test_base_dir is at the session scope, so we can't use request.node as mentioned in
    # the doc linked in the hook above.
    if request.session.testsfailed == 0:
        try:
            shutil.rmtree(directory)
        except Exception:
            files = [
                os.path.join(dp, f) for dp, _, fn in os.walk(directory) for f in fn
            ]
            print("Directory still contains files:", files)
            raise
    else:
        print(f"Test failed, leaving directory '{directory}' intact")


@pytest.fixture
def test_name(request):
    yield request.function.__name__


@pytest.fixture
def executor(test_name):
    ex = futures.ThreadPoolExecutor(
        max_workers=EXECUTOR_WORKERS, thread_name_prefix=test_name
    )
    yield ex
    ex.shutdown(wait=False)


@pytest.fixture
def bitcoind(directory):
    bitcoind = Bitcoind(bitcoin_dir=os.path.join(directory, "bitcoind"))
    bitcoind.startup()

    bitcoind.rpc.createwallet(bitcoind.rpc.wallet_name, False, False, "", False, True)

    bitcoind.rpc.generatetoaddress(101, bitcoind.rpc.getnewaddress())
    while bitcoind.rpc.getbalance() < 50:
        time.sleep(0.01)

    yield bitcoind

    bitcoind.cleanup()


@pytest.fixture
def minisafed(bitcoind, directory):
    datadir = os.path.join(directory, "minisafed")
    os.makedirs(datadir, exist_ok=True)
    bitcoind_cookie = os.path.join(bitcoind.bitcoin_dir, "regtest", ".cookie")

    main_desc = "wsh(or_d(pk(02869ef67283b4bc9af9d8366efb31f718018bfd5970a69b3d16f22f51228f73dc),and_v(v:pkh(03bb4dc7ed08cc633893f457553ad941ff82195342467d350dbb63773dd17f113b),older(157680))))"

    minisafed = Minisafed(
        datadir,
        main_desc,
        bitcoind.rpcport,
        bitcoind_cookie,
    )

    try:
        minisafed.start()
        yield minisafed
    except Exception:
        minisafed.cleanup()
        raise

    minisafed.cleanup()
