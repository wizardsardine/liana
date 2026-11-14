"""QA pipeline regressions. All signing, device and build operations are synthetic."""
from copy import deepcopy
from datetime import datetime, timedelta
import io
import json
from pathlib import Path
import plistlib
import tarfile
from types import SimpleNamespace

import pytest

from harness import qa_build
from harness.provisioning import FreshRunner, validate_config
from harness.state import Blocked, Failed, Journal
from harness.strict import StrictPolicy


@pytest.fixture
def config(tmp_path):
    return {"version": 2, "account_email": "qa@example.invalid", "profile_dir": str(tmp_path / "desktop"),
            "connect_rest": "https://api.coincube.io", "connect_grpc": "https://grpc.coincube.io:443",
            "ios": {"minimum_build": 41, "qa_build": {"checkout": str(tmp_path / "keychain"),
                    "revision": "a" * 40, "flutter": str(tmp_path / "flutter")}},
            "amount_sats": 10000, "fee_sats": 1000}


def test_default_is_strict_and_does_not_mutate_private_config(config):
    before = deepcopy(config)
    resolved = validate_config(qa_build.configure_fresh(config))
    assert resolved["strict_acceptance"] == {"version": 1}
    assert config == before


def test_compatibility_is_explicit_and_cannot_downgrade_strict(config):
    del config["ios"]["qa_build"]
    assert "strict_acceptance" not in qa_build.configure_fresh(config, compatibility=True)
    with pytest.raises(Failed, match="Strict QA requires"):
        qa_build.configure_fresh(config)
    config["strict_acceptance"] = {"version": 1}
    with pytest.raises(Failed, match="cannot override"):
        qa_build.configure_fresh(config, compatibility=True)


@pytest.mark.parametrize("mode", [False, None, {}, {"version": True}, {"version": 2}])
def test_invalid_strict_selection_never_becomes_compatibility(config, mode):
    config["strict_acceptance"] = mode
    validate_strict = validate_config
    fresh_config = qa_build.configure_fresh(config)
    with pytest.raises(Failed) as caught:
        validate_strict(fresh_config)
    assert caught.value.code == "STRICT_CONFIG"


@pytest.mark.parametrize("field,value", [("revision", "main"), ("revision", "a" * 39),
                                          ("checkout", "relative"), ("flutter", False)])
def test_build_source_must_be_pinned(config, field, value):
    config["ios"]["qa_build"][field] = value
    with pytest.raises(Failed) as caught:
        qa_build.configure_fresh(config)
    assert caught.value.code == "QA_BUILD_CONFIG"


@pytest.fixture
def build(tmp_path, config, monkeypatch):
    journal = Journal(tmp_path / "run", config)
    policy = StrictPolicy(journal)
    device = {"identifier": "synthetic-device", "hardwareProperties": {"udid": "synthetic-udid"}}
    calls = []
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as stream:
        for name, content in {".fvmrc": b'{"flutter":"3.41.9"}', "lib/main.dart": b"reviewed source"}.items():
            item = tarfile.TarInfo(name)
            item.size = len(content)
            stream.addfile(item, io.BytesIO(content))

    def command(args, **kwargs):
        args = [str(a) for a in args]
        calls.append(args)
        if "get-url" in args:
            return b"git@github.com:coincubetech/keychain-app.git\n"
        if args[:2] == ["git", "archive"]:
            return archive.getvalue()
        if "--machine" in args:
            return b'{"frameworkVersion":"3.41.9"}'
        if "-exportArchive" in args:
            export = Path(args[args.index("-exportPath") + 1])
            export.mkdir(parents=True, exist_ok=True)
            (export / "Keychain.ipa").write_bytes(b"synthetic ipa")
        return b""

    def query(*args, **kwargs):
        if args[:3] == ("device", "install", "app"):
            assert kwargs == {"timeout": 180}
        elif args[:3] == ("device", "process", "launch"):
            assert kwargs == {"timeout": 60}
        else:
            assert not kwargs
        calls.append(list(args))
        return {"apps": [{"bundleVersion": "41"}]}

    app = {"build": 41, "binary_sha256": "b" * 64, "flutter_sha256": "c" * 64}
    monkeypatch.setattr(qa_build, "command", command)
    monkeypatch.setattr(qa_build, "validate_app", lambda *args: dict(app))
    yield SimpleNamespace(config=config["ios"]["qa_build"], policy=policy, journal=journal,
                          device=device, query=query, calls=calls, app=app)
    journal.close()


def prepare(build):
    qa_build.prepare(build.config, build.policy, build.device, 41, build.query)


def test_build_defines_qa_hook_before_install_and_resume_never_relaunches(build):
    prepare(build)
    calls = build.calls
    assert any("--dart-define=COINCUBE_QA_HOME_REFRESH=true" in args for args in calls)
    install = next(i for i, args in enumerate(calls) if args[:3] == ["device", "install", "app"])
    launch = next(i for i, args in enumerate(calls) if args[:3] == ["device", "process", "launch"])
    assert install < launch
    receipt = build.journal.data["artifacts"]["qa_build"]
    assert receipt["source"] == "a" * 40
    assert receipt["app"] == build.app
    calls.clear()
    build.journal.artifact("strict_process", {"pid": 123})
    prepare(build)
    assert calls == [["device", "info", "apps", "--device", "synthetic-device", "--bundle-id",
                      qa_build.BUNDLE, "--include-removable-apps", "--include-default-apps"]]


def test_interrupted_build_reextracts_instead_of_trusting_mutable_source(build, monkeypatch):
    real = qa_build.command
    def fail(args, **kwargs):
        if "pub" in args:
            raise Blocked("QA_BUILD", "synthetic interrupted build")
        return real(args, **kwargs)
    monkeypatch.setattr(qa_build, "command", fail)
    with pytest.raises(Blocked):
        prepare(build)
    source = build.journal.directory / "qa-build/source/lib/main.dart"
    source.write_text("unreviewed replacement")
    monkeypatch.setattr(qa_build, "command", real)
    prepare(build)
    assert source.read_text() == "reviewed source"


def test_missing_receipt_cannot_replace_a_pinned_process(build):
    build.journal.artifact("strict_process", {"pid": 123})
    with pytest.raises(Failed) as caught:
        prepare(build)
    assert caught.value.code == "QA_BUILD_RESUME"
    assert not build.calls


def test_install_success_still_requires_exact_installed_version_before_launch(build):
    def query(*args, **kwargs):
        build.calls.append(list(args))
        return {"apps": [{"bundleVersion": "42"}]}
    build.query = query
    with pytest.raises(Blocked) as caught:
        prepare(build)
    assert caught.value.code == "QA_BUILD_CHANGED"
    assert not any(args[:3] == ["device", "process", "launch"] for args in build.calls)
    assert "qa_build" not in build.journal.data["artifacts"]


@pytest.mark.parametrize("change,code", [("device", "QA_BUILD_BINDING"), ("binary", "QA_BUILD_CHANGED"),
                                        ("installed", "QA_BUILD_CHANGED")])
def test_resume_refuses_changed_device_binary_or_installed_build(build, change, code):
    prepare(build)
    build.calls.clear()
    if change == "device":
        build.device["hardwareProperties"]["udid"] = "another-device"
    elif change == "binary":
        build.app["binary_sha256"] = "d" * 64
    else:
        build.query = lambda *args: {"apps": [{"bundleVersion": "42"}]}
    with pytest.raises(Blocked) as caught:
        prepare(build)
    assert caught.value.code == code
    assert not any(args[:2] == ["device", "install"] for args in build.calls)


def test_real_package_validation_rejects_wrong_profile_endpoints_and_build(tmp_path, monkeypatch):
    app = tmp_path / "Runner.app"
    assets = app / "Frameworks/App.framework/flutter_assets"
    assets.mkdir(parents=True)
    (app / "Runner").write_bytes(b"synthetic binary")
    framework = assets.parent / "App"
    framework.write_bytes(b"synthetic flutter qa-home-refresh-trigger")
    info = {"CFBundleIdentifier": qa_build.BUNDLE, "CFBundleVersion": "41"}
    (app / "Info.plist").write_bytes(plistlib.dumps(info))
    env = "API_URL=https://api.coincube.io/api/v1/\nGRPC_HOST=grpc.coincube.io\nGRPC_PORT=443\nGRPC_TLS=true\n"
    (assets / ".env").write_text(env)
    ent = {"application-identifier": qa_build.TEAM + "." + qa_build.BUNDLE, "get-task-allow": False,
           "aps-environment": "production", "com.apple.developer.associated-domains": ["webcredentials:coincube.io"]}
    profile = {"TeamIdentifier": [qa_build.TEAM], "ProvisionedDevices": ["synthetic-udid"],
               "Entitlements": dict(ent),
               "ExpirationDate": datetime.now() + timedelta(days=1)}
    monkeypatch.setattr(qa_build, "command", lambda *a, **k: b"")
    monkeypatch.setattr(qa_build, "plist_output", lambda args: ent if args[0] == "codesign" else profile)
    device = {"hardwareProperties": {"udid": "synthetic-udid"}}
    assert qa_build.validate_app(app, device, 41)["build"] == 41
    framework.write_bytes(b"synthetic release build without QA probe")
    with pytest.raises(Failed) as caught:
        qa_build.validate_app(app, device, 41)
    assert caught.value.code == "QA_BUILD_HOOK"
    framework.write_bytes(b"synthetic flutter qa-home-refresh-trigger")
    with pytest.raises(Failed, match="identity"):
        qa_build.validate_app(app, device, 42)
    profile["ProvisionedDevices"] = []
    with pytest.raises(Failed, match="identity"):
        qa_build.validate_app(app, device, 41)
    profile["ProvisionedDevices"] = ["synthetic-udid"]
    (assets / ".env").write_text(env.replace("grpc.coincube.io", "localhost"))
    with pytest.raises(Failed) as caught:
        qa_build.validate_app(app, device, 41)
    assert caught.value.code == "QA_BUILD_ENDPOINTS"


def test_capacity_gate_blocks_before_creating_a_cube_or_entering_pin(config, tmp_path, monkeypatch):
    config = qa_build.configure_fresh(config)
    runner = FreshRunner(config, tmp_path / "run")
    runner.account = {"tokens": {"access_token": "synthetic-secret"}}
    runner.account_user_id = 1
    runner.keys = lambda: [{"network": "testnet4", "name": "prior key"}]
    replies = [{"success": True, "data": {"entitlements": {"personalKeyLimit": 1}}},
               {"success": True, "data": {"userId": 1, "network": "testnet4", "noOfKeys": 1, "keyLimit": 1}}]
    monkeypatch.setattr("harness.provisioning.http", lambda *a, **k: json.dumps(replies.pop(0)).encode())
    try:
        with pytest.raises(Blocked) as caught:
            runner.preflight_capacity()
        assert caught.value.code == "SIGNER_LIMIT"
        assert "synthetic-secret" not in str(caught.value)
        assert "cube_id" not in runner.artifacts
    finally:
        runner.close()


@pytest.mark.parametrize("stage", ["environment", "authentication", "phone_setup", "cube_creation", "funding", "sign", "broadcast"])
def test_preflight_executes_account_and_phone_gates_before_skipping_mutations(stage):
    from test_full_flow import test_fresh_testnet4 as execute
    calls = []
    runner = SimpleNamespace(config={"version": 2}, run=calls.append)
    request = SimpleNamespace(config=SimpleNamespace(getoption=lambda option: True))
    if stage in ("environment", "authentication", "phone_setup"):
        execute(stage, runner, request, lambda *a: None)
        assert calls == [stage]
    else:
        with pytest.raises(pytest.skip.Exception):
            execute(stage, runner, request, lambda *a: None)
        assert not calls


def test_qa_build_holds_the_phone_lease_and_releases_it_on_failure(config, tmp_path, monkeypatch):
    import fcntl
    import os
    from harness import phone
    monkeypatch.setattr(phone.tempfile, "gettempdir", lambda: str(tmp_path))
    monkeypatch.setattr(phone, "single_wired_device", lambda: {})
    monkeypatch.setattr(phone.subprocess, "run", lambda args, **kwargs: SimpleNamespace(
        stdout=b"3.9.0" if "--version" in args else b'{"xcuitest":{"version":"12.22.8"}}'))
    called = []
    path = tmp_path / ("coincube-testnet4-ios-" + str(os.getuid()) + ".lock")
    def build_under_lease(*args):
        called.append(True)
        with path.open("rb") as competing:
            with pytest.raises(BlockingIOError):
                fcntl.flock(competing, fcntl.LOCK_EX | fcntl.LOCK_NB)
        raise Blocked("QA_BUILD", "synthetic signing prerequisite missing")
    monkeypatch.setattr(phone, "prepare_qa_build", build_under_lease)
    attachment = phone.attach(config["ios"], strict=object())
    with pytest.raises(Blocked) as caught:
        with attachment:
            pytest.fail("Build failure cannot attach to the phone")
    assert caught.value.code == "QA_BUILD"
    assert called == [True]
    with path.open("rb") as competing:
        fcntl.flock(competing, fcntl.LOCK_EX | fcntl.LOCK_NB)


def test_interrupted_request_helper_uses_the_same_default_strict_binding(config, tmp_path, monkeypatch):
    from contextlib import nullcontext
    from harness import sign_abort
    from harness.state import atomic_write, digest
    config["phone_state_dir"] = str(tmp_path / "phone")
    path = tmp_path / "config.json"
    atomic_write(path, json.dumps(config).encode())
    expected = digest(qa_build.configure_fresh(config))
    def inspect_binding(directory, effective):
        assert digest(effective) == expected
        assert "strict_acceptance" not in config
        raise Blocked("BINDING_CHECKED", "stopped before remote mutation")
    monkeypatch.setattr(sign_abort, "source_lock", lambda *a: nullcontext())
    monkeypatch.setattr(sign_abort, "output_lock", lambda *a: nullcontext())
    monkeypatch.setattr(sign_abort, "bind_source", inspect_binding)
    args = SimpleNamespace(source_run=str(tmp_path / "run"), config=str(path),
                           output_dir=str(tmp_path / "abort"), native_binary=str(tmp_path / "native"),
                           signing_binary=str(tmp_path / "signing"), authorize_abort="synthetic owner authorization")
    with pytest.raises(Blocked) as caught:
        sign_abort.execute(args)
    assert caught.value.code == "BINDING_CHECKED"


@pytest.mark.parametrize("timeout", [15, 60, 180])
def test_device_command_preserves_operation_timeout(monkeypatch, timeout):
    from harness import phone
    calls = []
    def run(args, **kwargs):
        calls.append((args, kwargs))
        return SimpleNamespace(stdout=b'{"result":{}}')
    monkeypatch.setattr(phone.subprocess, "run", run)
    assert phone.device_query("device", "install", "app", timeout=timeout) == {}
    args, options = calls[0]
    assert args[args.index("--timeout") + 1] == str(timeout)
    assert options["timeout"] == timeout + 5


def test_launcher_rejects_unsupported_python_patch_without_reexec_loop(monkeypatch):
    import runpy
    import sys
    import subprocess
    import shutil
    monkeypatch.setattr(sys, "version_info", (3, 11, 3))
    monkeypatch.setattr(shutil, "which", lambda name: "/synthetic/" + name)
    calls = []
    def probe(args, **kwargs):
        calls.append(args)
        return SimpleNamespace(returncode=1)
    monkeypatch.setattr(subprocess, "run", probe)
    run_launcher = runpy.run_path
    launcher_path = str(Path(__file__).resolve().parents[1] / "run")
    with pytest.raises(SystemExit, match="Python 3.11.4 or newer"):
        run_launcher(launcher_path, run_name="__main__")
    assert len(calls) == 3
