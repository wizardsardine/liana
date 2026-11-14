"""Exercise wrapper argument routing without bootstrap or live acceptance."""
import ast
from pathlib import Path
from types import SimpleNamespace

import pytest


class ExecIntercept(Exception):
    def __init__(self, argv):
        self.argv = argv


@pytest.fixture
def launcher():
    script = Path(__file__).resolve().parents[1] / "run"
    tree = ast.parse(script.read_text(), filename=str(script))
    # Exclude interpreter/venv/pip bootstrap; run the actual argument dispatcher.
    start = next(index for index, node in enumerate(tree.body)
                 if isinstance(node, ast.Assign)
                 and any(isinstance(target, ast.Name) and target.id == "args"
                         for target in node.targets))
    code = compile(ast.Module(body=tree.body[start:], type_ignores=[]), str(script), "exec")
    events = []

    def invoke(args, gui_status=0):
        def run(argv):
            events.append(("gui", argv))
            return SimpleNamespace(returncode=gui_status)

        def replace(executable, argv):
            events.append(("exec", argv))
            raise ExecIntercept(argv)

        namespace = {"__file__": str(script), "Path": Path,
                     "root": script.parent, "python": Path("/offline/python"),
                     "sys": SimpleNamespace(argv=[str(script), *args]),
                     "subprocess": SimpleNamespace(run=run),
                     "os": SimpleNamespace(execv=replace, umask=lambda mode: None, environ={})}
        with pytest.raises(ExecIntercept) as replaced:
            exec(code, namespace)
        return replaced.value.argv

    return invoke, events, script


@pytest.mark.parametrize("fresh_flags", [[], ["--qa-fresh"], ["--qa-fresh", "--qa-fresh"]])
def test_flow_reaches_strict_pytest_without_wrapper_flags(launcher, tmp_path, fresh_flags):
    invoke, events, script = launcher
    gui_options = ["--qa-gui-config", "gui.json", "--qa-gui-run-dir", str(tmp_path / "gui")]
    mobile_options = ["--qa-config", "mobile.json", "--qa-run-dir", str(tmp_path / "mobile"),
                      "--qa-confirm", "--qa-deadline", "60"]
    child = invoke(["--qa-flow", *fresh_flags, *gui_options, *mobile_options])
    assert events == [("gui", ["/offline/python", str(script), "--qa-gui-recovery", *gui_options]),
                      ("exec", child)]
    # Follow the intercepted re-exec through the same dispatcher, never a process.
    pytest_argv = invoke(child[2:])
    assert pytest_argv[:4] == ["/offline/python", "-m", "pytest", str(script.parent / "test_full_flow.py")]
    assert "--qa-fresh" not in pytest_argv
    assert pytest_argv[pytest_argv.index("--qa-config"):] == [
        *mobile_options, "--junitxml=" + str(tmp_path / "mobile" / "results.xml")]
    assert not (tmp_path / "gui").exists()


@pytest.mark.parametrize("gui_status", [1, 2])
def test_flow_gui_failure_or_block_never_starts_mobile(launcher, tmp_path, gui_status):
    invoke, events, _ = launcher
    with pytest.raises(SystemExit) as stopped:
        invoke(["--qa-flow", "--qa-fresh", "--qa-gui-config", "gui.json",
                "--qa-gui-run-dir", str(tmp_path / "gui"), "--qa-config", "mobile.json",
                "--qa-run-dir", str(tmp_path / "mobile")], gui_status=gui_status)
    assert stopped.value.code == gui_status
    assert [kind for kind, _ in events] == ["gui"]
    assert not (tmp_path / "mobile").exists()


@pytest.mark.parametrize("mode", ["--qa-compatibility", "--qa-preflight", "--self-test"])
def test_flow_rejects_substitutes_before_gui_or_mobile(launcher, mode):
    invoke, events, _ = launcher
    with pytest.raises(SystemExit, match="full strict acceptance"):
        invoke(["--qa-flow", "--qa-fresh", mode])
    assert events == []
