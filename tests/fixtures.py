from bip32 import BIP32
from bip32.utils import _pubkey_to_fingerprint
from bip380.descriptors import Descriptor
from concurrent import futures
from test_framework.bitcoind import Bitcoind
from test_framework.electrs import Electrs
from test_framework.lianad import Lianad
from test_framework.signer import SingleSigner, MultiSigner
from test_framework.utils import (
    BITCOIN_BACKEND_TYPE,
    EXECUTOR_WORKERS,
    USE_TAPROOT,
    BitcoinBackendType,
)

import hashlib
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

    bitcoind.rpc.createwallet(
        bitcoind.rpc.wallet_name, False, False, "", False, True, True
    )

    bitcoind.rpc.generatetoaddress(101, bitcoind.rpc.getnewaddress())
    while bitcoind.rpc.getbalance() < 50:
        time.sleep(0.01)

    yield bitcoind

    bitcoind.cleanup()


@pytest.fixture
def bitcoin_backend(directory, bitcoind):

    if BITCOIN_BACKEND_TYPE is BitcoinBackendType.Bitcoind:
        yield bitcoind
        bitcoind.cleanup()
    elif BITCOIN_BACKEND_TYPE is BitcoinBackendType.Electrs:
        electrs = Electrs(
            electrs_dir=os.path.join(directory, "electrs"),
            bitcoind_dir=bitcoind.bitcoin_dir,
            bitcoind_rpcport=bitcoind.rpcport,
            bitcoind_p2pport=bitcoind.p2pport,
        )
        electrs.startup()
        yield electrs
        electrs.cleanup()
    else:
        raise NotImplementedError


def xpub_fingerprint(hd):
    return _pubkey_to_fingerprint(hd.pubkey).hex()


def single_key_desc(
    prim_fg,
    prim_xpub,
    reco_fg,
    reco_xpub,
    csv_value,
    is_taproot,
    prim_deriv_path="",
    reco_deriv_path="",
):
    if is_taproot:
        return f"tr([{prim_fg}{prim_deriv_path}]{prim_xpub}/<0;1>/*,and_v(v:pk([{reco_fg}{reco_deriv_path}]{reco_xpub}/<0;1>/*),older({csv_value})))"
    else:
        return f"wsh(or_d(pk([{prim_fg}{prim_deriv_path}]{prim_xpub}/<0;1>/*),and_v(v:pkh([{reco_fg}{reco_deriv_path}]{reco_xpub}/<0;1>/*),older({csv_value}))))"


@pytest.fixture
def lianad(bitcoin_backend, directory):
    datadir = os.path.join(directory, "lianad")
    os.makedirs(datadir, exist_ok=True)

    signer = SingleSigner(is_taproot=USE_TAPROOT)
    (prim_fingerprint, primary_xpub), (reco_fingerprint, recovery_xpub) = (
        (xpub_fingerprint(signer.primary_hd), signer.primary_hd.get_xpub()),
        (xpub_fingerprint(signer.recovery_hd), signer.recovery_hd.get_xpub()),
    )
    csv_value = 10
    # NOTE: origins are the actual xpub themselves which is incorrect but makes it
    # possible to differentiate them.
    main_desc = Descriptor.from_str(
        single_key_desc(
            prim_fingerprint,
            primary_xpub,
            reco_fingerprint,
            recovery_xpub,
            csv_value,
            is_taproot=USE_TAPROOT,
        )
    )

    lianad = Lianad(
        datadir,
        signer,
        main_desc,
        bitcoin_backend,
    )

    try:
        lianad.start()
        yield lianad
    except Exception:
        lianad.cleanup()
        raise

    lianad.cleanup()


# This can currently only be used with Taproot if no signing is required.
@pytest.fixture
def lianad_with_deriv_paths(bitcoin_backend, directory):
    datadir = os.path.join(directory, "lianad")
    os.makedirs(datadir, exist_ok=True)

    signer = SingleSigner(is_taproot=USE_TAPROOT)
    (prim_fingerprint, primary_xpub), (reco_fingerprint, recovery_xpub) = (
        (xpub_fingerprint(signer.primary_hd), signer.primary_hd.get_xpub()),
        (xpub_fingerprint(signer.recovery_hd), signer.recovery_hd.get_xpub()),
    )
    csv_value = 10
    main_desc = Descriptor.from_str(
        single_key_desc(
            prim_fingerprint,
            primary_xpub,
            reco_fingerprint,
            recovery_xpub,
            csv_value,
            USE_TAPROOT,
            "/48h/1h/0h/2h",
            "/46h/12h/10h/72h",
        )
    )

    lianad = Lianad(
        datadir,
        signer,
        main_desc,
        bitcoin_backend,
    )

    try:
        lianad.start()
        yield lianad
    except Exception:
        lianad.cleanup()
        raise

    lianad.cleanup()


def unspendable_internal_xpub(xpubs):
    """Deterministic, unique, unspendable internal key.
    See See https://delvingbitcoin.org/t/unspendable-keys-in-descriptors/304/21
    """
    chaincode = hashlib.sha256(b"".join(xpub.pubkey for xpub in xpubs)).digest()
    bip341_nums = bytes.fromhex(
        "0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0"
    )
    return BIP32(chaincode, pubkey=bip341_nums, network="test")


def multi_expression(thresh, keys, is_taproot):
    exp = f"multi_a({thresh}," if is_taproot else f"multi({thresh},"
    for i, key in enumerate(keys):
        # NOTE: origins are the actual xpub themselves which is incorrect but make it
        # possible to differentiate them.
        fingerprint = xpub_fingerprint(key)
        exp += f"[{fingerprint}]{key.get_xpub()}/<0;1>/*"
        if i != len(keys) - 1:
            exp += ","
    return exp + ")"


def multisig_desc(multi_signer, csv_value, is_taproot, prim_thresh, recov_thresh):
    prim_multi, recov_multi = (
        multi_expression(prim_thresh, multi_signer.prim_hds, is_taproot),
        multi_expression(recov_thresh, multi_signer.recov_hds[csv_value], is_taproot),
    )
    if is_taproot:
        all_xpubs = [
            hd for hd in multi_signer.prim_hds + multi_signer.recov_hds[csv_value]
        ]
        internal_key = unspendable_internal_xpub(all_xpubs).get_xpub()
        return f"tr([00000000]{internal_key}/<0;1>/*,{{{prim_multi},and_v(v:{recov_multi},older({csv_value}))}})"
    else:
        return f"wsh(or_d({prim_multi},and_v(v:{recov_multi},older({csv_value}))))"


@pytest.fixture
def lianad_multisig(bitcoin_backend, directory):
    datadir = os.path.join(directory, "lianad")
    os.makedirs(datadir, exist_ok=True)

    # A 3-of-4 that degrades into a 2-of-5 after 10 blocks
    csv_value = 10
    signer = MultiSigner(4, {csv_value: 5}, is_taproot=USE_TAPROOT)
    main_desc = Descriptor.from_str(multisig_desc(signer, csv_value, USE_TAPROOT, 3, 2))

    lianad = Lianad(
        datadir,
        signer,
        main_desc,
        bitcoin_backend,
    )

    try:
        lianad.start()
        yield lianad
    except Exception:
        lianad.cleanup()
        raise

    lianad.cleanup()


@pytest.fixture
def lianad_multisig_legacy_datadir(bitcoin_backend, directory):
    datadir = os.path.join(directory, "lianad")
    os.makedirs(datadir, exist_ok=True)

    # A 3-of-4 that degrades into a 2-of-5 after 10 blocks
    csv_value = 10
    signer = MultiSigner(4, {csv_value: 5}, is_taproot=USE_TAPROOT)
    main_desc = Descriptor.from_str(multisig_desc(signer, csv_value, USE_TAPROOT, 3, 2))

    lianad = Lianad(datadir, signer, main_desc, bitcoin_backend, legacy_datadir=True)

    try:
        lianad.start()
        yield lianad
    except Exception:
        lianad.cleanup()
        raise

    lianad.cleanup()


@pytest.fixture
def lianad_multisig_2_of_2(bitcoin_backend, directory):
    datadir = os.path.join(directory, "lianad")
    os.makedirs(datadir, exist_ok=True)

    # A 2-of-2 that degrades into a 1-of-1 after 10 blocks
    csv_value = 10
    signer = MultiSigner(2, {csv_value: 1}, is_taproot=USE_TAPROOT)
    main_desc = Descriptor.from_str(multisig_desc(signer, csv_value, USE_TAPROOT, 2, 1))

    lianad = Lianad(
        datadir,
        signer,
        main_desc,
        bitcoin_backend,
    )

    try:
        lianad.start()
        yield lianad
    except Exception:
        lianad.cleanup()
        raise

    lianad.cleanup()


def multipath_desc(multi_signer, csv_values, is_taproot):
    prim_multi = multi_expression(3, multi_signer.prim_hds, is_taproot)
    first_recov_multi = multi_expression(
        3, multi_signer.recov_hds[csv_values[0]], is_taproot
    )
    second_recov_multi = multi_expression(
        1, multi_signer.recov_hds[csv_values[1]], is_taproot
    )
    if is_taproot:
        all_xpubs = [
            hd
            for hd in multi_signer.prim_hds
            + multi_signer.recov_hds[csv_values[0]]
            + multi_signer.recov_hds[csv_values[1]]
        ]
        internal_key = unspendable_internal_xpub(all_xpubs).get_xpub()
        # On purpose we use a single leaf instead of 3 different ones. It shouldn't be an issue.
        return f"tr([00000000]{internal_key}/<0;1>/*,or_d({prim_multi},or_i(and_v(v:{first_recov_multi},older({csv_values[0]})),and_v(v:{second_recov_multi},older({csv_values[1]})))))"
    else:
        return f"wsh(or_d({prim_multi},or_i(and_v(v:{first_recov_multi},older({csv_values[0]})),and_v(v:{second_recov_multi},older({csv_values[1]})))))"


@pytest.fixture
def lianad_multipath(bitcoin_backend, directory):
    datadir = os.path.join(directory, "lianad")
    os.makedirs(datadir, exist_ok=True)

    # A 3-of-4 that degrades into a 3-of-5 after 10 blocks and into a 1-of-10 after 20 blocks.
    csv_values = [10, 20]
    signer = MultiSigner(
        4, {csv_values[0]: 5, csv_values[1]: 10}, is_taproot=USE_TAPROOT
    )
    main_desc = Descriptor.from_str(
        multipath_desc(signer, csv_values, is_taproot=USE_TAPROOT)
    )

    lianad = Lianad(
        datadir,
        signer,
        main_desc,
        bitcoin_backend,
    )

    try:
        lianad.start()
        yield lianad
    except Exception:
        lianad.cleanup()
        raise

    lianad.cleanup()
