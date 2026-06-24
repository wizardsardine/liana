import shutil
import tempfile
from pathlib import Path

import pytest

# Re-export the daemon test fixtures so GUI tests can compose with the existing
# regtest harness from this nested conftest.
from fixtures import *  # noqa: F401,F403

from .datadir import GuiWallet
from .driver import GuiApp
from .x11 import X11Session


def pytest_configure(config):
    for marker in (
        "gui_smoke: quick GUI launch and navigation tests",
        "gui_core: core GUI wallet workflows",
        "gui_filepicker: GUI workflows that use native file pickers",
        "gui_slow: slow GUI workflows such as rescans and reorgs",
        "gui_electrs: GUI workflows using an Electrum backend",
        "gui_taproot: GUI workflows using Taproot descriptors",
    ):
        config.addinivalue_line("markers", marker)


@pytest.fixture
def x11_session(directory):
    session = X11Session(Path(directory) / "x11").start()
    try:
        yield session
    finally:
        session.close()


@pytest.fixture
def gui_wallet(request, test_base_dir, bitcoind):
    datadir = Path(tempfile.mkdtemp(prefix="lg-", dir=test_base_dir))
    wallet = GuiWallet.single_sig(datadir, bitcoind)
    try:
        yield wallet
    finally:
        rep_call = getattr(request.node, "rep_call", None)
        if rep_call is not None and not rep_call.failed:
            shutil.rmtree(datadir)
        else:
            print(f"Test failed, leaving GUI datadir '{datadir}' intact")


@pytest.fixture
def liana_gui(request, x11_session, gui_wallet):
    app = GuiApp(x11_session, gui_wallet.datadir).start()
    try:
        yield app
    finally:
        if getattr(request.node, "rep_call", None) and request.node.rep_call.failed:
            app.save_debug_artifacts("failure")
        app.stop()


@pytest.fixture
def opened_liana_gui(liana_gui):
    try:
        liana_gui.click_text("GUI regtest wallet", timeout=30)
        liana_gui.assert_text("Balance", timeout=60)
    except Exception:
        liana_gui.save_debug_artifacts("open-wallet-failure")
        raise
    return liana_gui
