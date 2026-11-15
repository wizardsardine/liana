"""Real desktop GUI Recovery Kit acceptance; a distinct no-broadcast suite."""
import pytest

from harness.gui_recovery import CHAINS, STAGES
from harness.state import Blocked, Failed


@pytest.mark.gui_recovery_live
@pytest.mark.parametrize("chain,stage", [(chain, stage) for chain in CHAINS for stage in STAGES])
def test_gui_recovery(chain, stage, gui_recovery, request, record_property):
    __tracebackhide__ = True
    record_property("chain", chain)
    record_property("stage", stage)
    record_property("scope", "real desktop GUI; pre-broadcast only")
    try:
        gui_recovery.run(chain, stage)
    except Failed as error:
        record_property("outcome", "failed"); record_property("code", error.code)
        pytest.fail(error.code + ": " + str(error), pytrace=False)
    except Blocked as error:
        request.config._gui_recovery_blocked = True
        record_property("outcome", "blocked"); record_property("code", error.code)
        pytest.skip(error.code + ": " + str(error))
    record_property("outcome", "passed")
