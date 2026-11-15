import sys
import json
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent))
from harness.runner import Runner, load_config
from harness.state import Blocked, Failed
from harness.state import private_read
from harness.provisioning import FreshRunner, validate_config
from harness.qa_build import configure_fresh
from harness import qa_cleanup


def pytest_addoption(parser):
    group = parser.getgroup("testnet4")
    group.addoption("--qa-gui-config", help="Private desktop GUI recovery configuration; does not select the mobile flow.")
    group.addoption("--qa-gui-run-dir", help="Separate persistent private desktop GUI run directory.")
    group.addoption("--qa-config", help="Private JSON fixture configuration (mode 600).")
    group.addoption("--qa-run-dir", help="Persistent private checkpoints (mode 700).")
    group.addoption("--qa-preflight", action="store_true", help="Prepare the QA app, verify account/PIN setup/network/capacity, then stop before Cube creation or funding.")
    group.addoption("--qa-deadline", type=int, default=90, help="Bounded delivery/propagation wait in seconds.")
    group.addoption("--qa-authorize-key-generation", help="Explicit authorization reference for one quarantined signer-key replacement generation.")
    group.addoption("--qa-key-recovery-prior-run", help="Absolute private completed-run directory proving prior PIN signing on this device.")
    group.addoption("--qa-confirm", action="store_true", help="Require a mined confirmation instead of mempool acceptance.")
    group.addoption("--qa-compatibility", action="store_true", help="Explicitly select the legacy fresh flow without strict build/delivery/refresh acceptance.")


def pytest_configure(config):
    config.addinivalue_line("markers", "testnet4_live: real USB iPhone and production Connect using disposable Testnet4 fixtures")
    config._testnet4_blocked = False
    config._gui_recovery_blocked = False
    config.addinivalue_line("markers", "gui_recovery_live: real desktop GUI full-kit restore/hot-sign export, never broadcast")


@pytest.fixture(scope="session")
def acceptance(request):
    __tracebackhide__ = True
    path = request.config.getoption("--qa-config")
    if not path:
        pytest.skip("Testnet4 live suite is opt-in; supply --qa-config and --qa-run-dir.")
    directory = request.config.getoption("--qa-run-dir")
    deadline = request.config.getoption("--qa-deadline")
    if not directory or not 1 <= deadline <= 3600:
        pytest.fail("Specify a persistent --qa-run-dir and a deadline of 1–3600 seconds.", pytrace=False)
    if getattr(request.config.option, "numprocesses", None):
        pytest.fail("The live suite requires one serial worker; do not use pytest-xdist.", pytrace=False)
    try:
        config = json.loads(private_read(path))
        if config.get("version") == 2:
            config = configure_fresh(config, compatibility=request.config.getoption("--qa-compatibility"))
            runner = FreshRunner(validate_config(config), directory, deadline=deadline, confirm=request.config.getoption("--qa-confirm"),
                                 authorize_key_generation=request.config.getoption("--qa-authorize-key-generation"),
                                 key_recovery_prior_run=request.config.getoption("--qa-key-recovery-prior-run"))
        else:
            if request.config.getoption("--qa-compatibility"):
                raise Failed("CONFIG_SCHEMA", "Compatibility mode applies only to the version-2 fresh flow.")
            if request.config.getoption("--qa-authorize-key-generation") or request.config.getoption("--qa-key-recovery-prior-run"):
                raise Failed("KEY_RECOVERY_OPTIONS", "Key-generation recovery is restricted to the fresh version-2 scenario.")
            runner = Runner(load_config(path), directory, deadline=deadline, confirm=request.config.getoption("--qa-confirm"))
    except Failed as error:
        pytest.fail(error.code + ": " + str(error), pytrace=False)
    except Blocked as error:
        request.config._testnet4_blocked = True
        pytest.skip(error.code + ": " + str(error))
    except Exception:
        request.config._testnet4_blocked = True
        pytest.skip("FIXTURE_CONFIG: private fixture configuration could not be loaded.")
    request.config._testnet4_runner = runner
    try:
        yield runner
    finally:
        runner.close()


@pytest.fixture(scope="session")
def gui_recovery(request):
    from harness.gui_recovery import GuiRecovery
    path = request.config.getoption("--qa-gui-config")
    directory = request.config.getoption("--qa-gui-run-dir")
    if not path:
        pytest.skip("Desktop GUI recovery is opt-in; supply its separate config and run directory.")
    if not directory or getattr(request.config.option, "numprocesses", None):
        pytest.fail("The desktop GUI recovery suite needs a private run directory and one serial worker.", pytrace=False)
    try:
        runner = GuiRecovery(json.loads(private_read(path)), directory)
    except Failed as error:
        pytest.fail(error.code + ": " + str(error), pytrace=False)
    except Blocked as error:
        request.config._gui_recovery_blocked = True
        pytest.skip(error.code + ": " + str(error))
    try:
        yield runner
    finally:
        runner.close()


@pytest.hookimpl(trylast=True)
def pytest_sessionfinish(session, exitstatus):
    # pytest normally treats skips as success. A selected live run with a
    # missing prerequisite must instead be non-green to unattended callers.
    if session.config._testnet4_blocked and exitstatus == 0:
        session.exitstatus = 2
    if session.config._gui_recovery_blocked and exitstatus == 0:
        session.exitstatus = 2
    runner = getattr(session.config, "_testnet4_runner", None)
    if (session.exitstatus == 0 and runner is not None and not runner.stop
            and "qa_cleanup" in runner.config and not session.config.getoption("--qa-preflight")):
        try:
            qa_cleanup.seal_completed(runner)
        except Blocked as error:
            session.config._testnet4_blocked = True
            session.config._testnet4_cleanup_code = error.code
            session.exitstatus = 2
        except Exception:
            session.config._testnet4_blocked = True
            session.config._testnet4_cleanup_code = "QA_CLEANUP_SEAL"
            session.exitstatus = 2


def pytest_terminal_summary(terminalreporter, exitstatus, config):
    if config._gui_recovery_blocked:
        terminalreporter.write_line("GUI RECOVERY BLOCKED — no real GUI acceptance or broadcast authority was established.")
    if getattr(config, "_testnet4_cleanup_code", None):
        terminalreporter.write_line(config._testnet4_cleanup_code + ": completed retirement enrollment was not sealed; evidence remains private.")
    if config._testnet4_blocked:
        terminalreporter.write_line("TESTNET4 BLOCKED — checkpoint preserved; fix the reported prerequisite and repeat the same command.")
