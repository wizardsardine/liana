from bip32 import BIP32
from bip380.descriptors import Descriptor
from concurrent import futures
from test_framework.bitcoind import Bitcoind
from test_framework.lianad import Lianad
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

    directory = tempfile.mkdtemp(prefix="lianad-tests-", dir=d)
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
def lianad(bitcoind, directory):
    datadir = os.path.join(directory, "lianad")
    os.makedirs(datadir, exist_ok=True)
    bitcoind_cookie = os.path.join(bitcoind.bitcoin_dir, "regtest", ".cookie")

    owner_hd = BIP32.from_seed(os.urandom(32), network="test")
    owner_xpub = owner_hd.get_xpub()
    main_desc = Descriptor.from_str(f"wsh(or_d(pk({owner_xpub}/<0;1>/*),and_v(v:pkh(tpubD9vQiBdDxYzU4cVFtApWj4devZrvcfWaPXX1zHdDc7GPfUsDKqGnbhraccfm7BAXgRgUbVQUV2v2o4NitjGEk7hpbuP85kvBrD4ahFDtNBJ/<0;1>/*),older(65000))))")

    lianad = Lianad(
        datadir,
        owner_hd,
        main_desc,
        bitcoind.rpcport,
        bitcoind_cookie,
    )

    try:
        lianad.start()
        yield lianad
    except Exception:
        lianad.cleanup()
        raise

    lianad.cleanup()
