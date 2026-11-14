"""Strict acceptance boundaries, with synthetic clocks, AX and native responses only."""
from copy import deepcopy
import json
import re
from types import SimpleNamespace

import pytest

from harness import strict
from harness.phone import Phone
from harness.provisioning import FreshRunner
from harness.state import Blocked, Failed, Journal, digest

HOME = {"Home", "Cubes", "Settings", "Add signer key"}
BINDING = {"request_id": "request", "intent": {"txid": "a" * 64, "outputs": []}, "fee_sats": 1000}


class Clock:
    now = 0.0
    jump = 0

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds + self.jump


class Control:
    def __init__(self, identity, label, children=(), click=None, name=None,
                 kind="XCUIElementTypeButton", visible=True, enabled=True):
        self.id, self.label, self.children = identity, label, children
        self.name = label if name is None else name
        self.onclick = click
        self.clicks = 0
        self.kind, self.visible, self.enabled = kind, visible, enabled

    def get_attribute(self, name):
        return self.label if name == "label" else self.name

    def find_elements(self, *_):
        return self.children

    def click(self):
        self.clicks += 1
        if self.onclick:
            self.onclick()


class NoSystemAlert:
    @property
    def alert(self):
        from selenium.common.exceptions import NoAlertPresentException
        raise NoAlertPresentException()


class Driver:
    switch_to = NoSystemAlert()
    state = 4
    pid = 123
    controls = ()
    modal = False

    def query_app_state(self, _):
        return self.state

    def execute_script(self, name, *args):
        assert name == "mobile: activeAppInfo", "No lifecycle, navigation or gestures allowed by this fake"
        return {"bundleId": strict.BUNDLE, "pid": self.pid}

    def find_elements(self, _, predicate):
        if 'XCUIElementTypeAlert' in predicate:
            return [object()] if self.modal else []
        types = re.findall(r'type == "([^"]+)"', predicate)
        matches = re.findall(r'(label|name) (==|BEGINSWITH) ("(?:[^"\\]|\\.)*")', predicate)
        return [c for c in self.controls
                if ('visible == true' not in predicate or c.visible)
                and ('enabled == true' not in predicate or c.enabled)
                and (not types or c.kind in types)
                and any((value := getattr(c, field)) is not None and
                        (value == json.loads(literal) if operator == '==' else value.startswith(json.loads(literal)))
                        for field, operator, literal in matches)]


@pytest.fixture
def policy(tmp_path, monkeypatch):
    clock = Clock()
    monkeypatch.setattr(strict.time, "monotonic", clock.monotonic)
    monkeypatch.setattr(strict.time, "sleep", clock.sleep)
    journal = Journal(tmp_path / "run", {"strict_acceptance": {"version": 1}})
    result = strict.StrictPolicy(journal)
    phone = Phone(Driver(), 39)
    phone.device_digest = "device"
    phone.labels = lambda: HOME
    phone.require_empty_pin = lambda session: None
    result.bind_phone(phone)
    result.clock = clock
    yield result
    journal.close()


def requested(policy):
    policy.begin_request(BINDING)
    policy.bind_session("session")
    policy.journal.artifact("session_id", "session")
    policy.journal.artifact("approval_binding", {**BINDING, "session_id": "session", "account": "account"})


def delivered(policy):
    requested(policy)
    policy.wait_delivery("request", "session", lambda: {"ready": False, "status": 2, "submitted_signature_count": 0}, 0)


def markers(policy, *, scope="session:1", status="idle", counts=(0, 0, 0, 0), click=None):
    values = ["qa-home-refresh-scope:" + scope, "qa-home-refresh-status:" + status,
              "qa-home-refresh-counts:" + ":".join(f"{name}={value}" for name, value in zip(
                  ("requested", "started", "completed", "failed"), counts)), "qa-home-refresh-trigger"]
    controls = [Control(str(i), label, click=click if i == 3 else None) for i, label in enumerate(values)]
    policy.phone.driver.controls = controls
    return controls


def complete(policy):
    delivered(policy)
    markers(policy, click=lambda: markers(policy, status="completed", counts=(1, 1, 1, 0)))
    policy.probe("request", "session", 3)
    context = {"binding": deepcopy(policy.artifacts["approval_binding"]), "request_id": "request", "session_id": "session",
               "intent_sha256": digest(BINDING["intent"]), "fee_sats": 1000, "build": 39,
               "device": "device", "process": {"pid": 123, "bundle_id": strict.BUNDLE}}
    policy.journal.artifact("approval_started", True)
    policy.journal.artifact("approval_context", context)
    policy.journal.artifact("pin_dispatch_started", {"context": deepcopy(context), "strict_scope": "qa-home-refresh-scope:session:1"})
    policy.journal.artifact("strict_pin_input", {"context": deepcopy(context), "scope": "qa-home-refresh-scope:session:1", "completed": True})


@pytest.mark.parametrize("kind", ["XCUIElementTypeButton", "XCUIElementTypeOther"])
def test_exact_qa_trigger_semantic_types_complete_correlated_probe(policy, kind):
    delivered(policy)
    trigger = markers(policy, click=lambda: markers(policy, status="completed", counts=(1, 1, 1, 0)))[3]
    trigger.kind = kind
    policy.probe("request", "session", 3)
    assert trigger.clicks == 1
    assert policy.artifacts["strict_home_refresh"]["phase"] == "completed"
    assert policy.artifacts["strict_home_refresh"]["scope"] == "qa-home-refresh-scope:session:1"


@pytest.mark.parametrize("change", ["disabled", "invisible", "text", "input", "wrong_identifier", "ambiguous"])
def test_semantic_trigger_refusals_precede_intent_and_click(policy, change):
    delivered(policy)
    controls = markers(policy)
    trigger = controls[3]
    trigger.kind = "XCUIElementTypeOther"
    if change == "disabled":
        trigger.enabled = False
    elif change == "invisible":
        trigger.visible = False
    elif change == "text":
        trigger.kind = "XCUIElementTypeStaticText"
    elif change == "input":
        trigger.kind = "XCUIElementTypeTextField"
    elif change == "wrong_identifier":
        trigger.label = trigger.name = "qa-home-refresh-trigger-extra"
    else:
        controls.append(Control("duplicate", "qa-home-refresh-trigger"))
    with pytest.raises(Blocked) as error:
        policy.probe("request", "session", 3)
    assert error.value.code == "STRICT_HOOK"
    assert "strict_home_refresh" not in policy.artifacts
    assert all(c.clicks == 0 for c in controls)


def test_semantic_trigger_preserves_matching_leaf_and_ambiguity_rules(policy):
    child = Control("button", "qa-home-refresh-trigger")
    wrapper = Control("wrapper", "qa-home-refresh-trigger", children=[child], kind="XCUIElementTypeOther")
    policy.phone.driver.controls = [wrapper, child]
    assert policy._leaf_controls("qa-home-refresh-trigger", button=True) is child
    policy.phone.driver.controls.append(Control("other-wrapper", "qa-home-refresh-trigger", kind="XCUIElementTypeOther"))
    with pytest.raises(Blocked) as error:
        policy._leaf_controls("qa-home-refresh-trigger", button=True)
    assert error.value.code == "STRICT_HOOK"


def test_other_semantics_are_not_a_generic_button_fallback(policy):
    policy.phone.driver.controls = [Control("other", "Different button", kind="XCUIElementTypeOther")]
    with pytest.raises(Blocked) as error:
        policy._leaf_controls("Different button", button=True)
    assert error.value.code == "STRICT_HOOK"


@pytest.mark.parametrize("value", [None, False, True, {}, {"version": True}, {"version": 2}, {"version": 1, "override": True}, "1"])
def test_mode_is_explicit_versioned_and_immutable(value):
    with pytest.raises(Failed, match="version 1"):
        strict.validate_mode({"strict_acceptance": value, "ios": {"minimum_build": 39}})


def test_normal_config_unchanged_and_strict_build_required():
    strict.validate_mode({"ios": {"minimum_build": 35}})
    strict.validate_mode({"strict_acceptance": {"version": 1}, "ios": {"minimum_build": 39}})
    with pytest.raises(Failed) as error:
        strict.validate_mode({"strict_acceptance": {"version": 1}, "ios": {"minimum_build": 38}})
    assert error.value.code == "STRICT_BUILD"


@pytest.mark.parametrize("saved", ["artifact", "stage"])
def test_cannot_retrofit_previous_runs(tmp_path, saved):
    journal = Journal(tmp_path / "run", {})
    try:
        if saved == "artifact":
            journal.artifact("broadcast_txid", "old")
        else:
            journal.outcome("phone", "passed")
        with pytest.raises(Failed) as error:
            strict.StrictPolicy(journal)
        assert error.value.code == "STRICT_RETROFIT"
    finally:
        journal.close()


@pytest.mark.parametrize("field", strict.RECOVERIES)
def test_any_recovery_receipt_blocks_resume_before_navigation(policy, field):
    policy.journal.artifact(field, {"phase": "completed"})
    with pytest.raises(Failed) as error:
        strict.StrictPolicy(policy.journal)
    assert error.value.code == "STRICT_RECOVERY"


@pytest.mark.parametrize("change", ["pid", "state", "build", "device"])
def test_process_bound_from_attach_and_never_reactivated(policy, change):
    if change == "pid":
        policy.phone.driver.pid += 1
    elif change == "state":
        policy.phone.driver.state = 1
    elif change == "build":
        policy.phone.build += 1
    else:
        policy.phone.device_digest = "other"
    with pytest.raises(Blocked) as error:
        policy.observe()
    assert error.value.code == "STRICT_PROCESS"


def test_idle_requires_full_interval_and_request_resume_preserves_it(policy):
    requested(policy)
    assert policy.clock.now == 90
    assert policy.artifacts["strict_idle"]["observed_seconds"] == 90
    assert policy.artifacts["strict_idle"]["samples"] == 91
    policy.begin_request(BINDING)
    assert policy.clock.now == 90
    changed = dict(BINDING, fee_sats=1001)
    with pytest.raises(Failed):
        policy.begin_request(changed)


@pytest.mark.parametrize("failure", ["gap", "background", "modal", "pin"])
def test_idle_does_not_count_interruption_or_unknown_modal(policy, failure):
    if failure == "gap":
        policy.clock.jump = 6
    elif failure == "background":
        policy.phone.driver.state = 3
    elif failure == "modal":
        policy.phone.driver.modal = True
    else:
        policy.phone.labels = lambda: HOME | {"Enter PIN"}
    with pytest.raises(Blocked):
        policy.begin_request(BINDING)
    assert not policy.requested


def test_pre_request_refresh_recorded_post_request_refresh_blocked(policy):
    policy.before_refresh()
    assert policy.artifacts["strict_setup_refreshes"]["count"] == 1
    requested(policy)
    with pytest.raises(Blocked) as error:
        policy.phone.refresh()
    assert error.value.code == "STRICT_REFRESH"


@pytest.mark.parametrize("method,args,kwargs", [
    ("restart_for_key_generation", (None, {}), {}),
    ("restart_for_request_discovery", (None, {}), {"before_restart": lambda: True}),
    ("restart_for_upgrade", (None, {}), {}),
    ("verify_pin_resume", (None,), {}),
])
def test_all_lifecycle_helpers_reject_before_driver_mutation(policy, method, args, kwargs):
    operation_under_test = getattr(policy.phone, method)
    with pytest.raises(Blocked) as error:
        operation_under_test(*args, **kwargs)
    assert error.value.code == "STRICT_RESTART"


@pytest.mark.parametrize("method,args", [("restore_navigation", ()), ("check_fixture", ("cube",)), ("home_key_card", ("key",))])
def test_no_setup_navigation_after_request(policy, method, args):
    requested(policy)
    operation_under_test = getattr(policy.phone, method)
    with pytest.raises(Blocked) as error:
        operation_under_test(*args)
    assert error.value.code == "STRICT_NAVIGATION"


def test_passive_delivery_waits_without_navigation_then_pins_ack(policy):
    requested(policy)
    sequence = iter([1, 1, 2])
    policy.wait_delivery("request", "session", lambda: {"ready": False, "status": next(sequence), "submitted_signature_count": 0}, 5)
    receipt = deepcopy(policy.artifacts["strict_delivery"])
    assert receipt["source"] == "target_device_stream_acknowledgement"
    assert policy.clock.now == 92
    policy.wait_delivery("request", "session", lambda: pytest.fail("Saved receipt permits later VIEWED without re-demanding DELIVERED"), 5)
    assert policy.artifacts["strict_delivery"] == receipt


@pytest.mark.parametrize("status,count,ready", [(1, 0, False), (3, 0, False), (4, 0, False), (2, 1, False), (True, 0, False), (2, False, False), (2, 0, True)])
def test_no_banner_navigation_without_exact_delivery_receipt(policy, status, count, ready):
    requested(policy)
    with pytest.raises(Blocked):
        policy.wait_delivery("request", "session", lambda: {"ready": ready, "status": status, "submitted_signature_count": count}, 0)
    assert "strict_delivery" not in policy.artifacts
    assert "approval_started" not in policy.artifacts


def test_delivery_receipt_binds_approval_identity(policy):
    delivered(policy)
    policy.artifacts["approval_binding"]["account"] = "different"
    with pytest.raises(Failed) as error:
        policy.require_delivery("request", "session")
    assert error.value.code == "STRICT_DELIVERY"


@pytest.mark.parametrize("change", ["missing", "duplicate", "name_disagrees", "wrong_session", "bad_serial", "invalid_status", "invalid_counts"])
def test_marker_contract_is_unique_and_exact(policy, change):
    delivered(policy)
    controls = markers(policy)
    if change == "missing":
        policy.phone.driver.controls = controls[1:]
    elif change == "duplicate":
        controls.append(Control("other", controls[0].label))
    elif change == "name_disagrees":
        controls[0].name = "qa-home-refresh-scope:other:1"
    elif change == "wrong_session":
        markers(policy, scope="other:1")
    elif change == "bad_serial":
        markers(policy, scope="session:xyz")
    elif change == "invalid_status":
        markers(policy, status="success")
    else:
        markers(policy, counts=(0, 1, 0, 0))
    with pytest.raises(Blocked):
        policy.probe("request", "session", 0)
    assert "strict_home_refresh" not in policy.artifacts


def test_ancestor_aggregation_same_element_alias_dedup_but_siblings_not(policy):
    controls = markers(policy)
    controls.extend([controls[0], Control("ancestor", controls[0].label, [controls[0]])])
    assert policy.marker("qa-home-refresh-scope:") == "qa-home-refresh-scope:session:1"
    controls.append(Control("sibling", controls[0].label))
    with pytest.raises(Blocked):
        policy.marker("qa-home-refresh-scope:")


@pytest.mark.parametrize("status,counts", [("failed", (1, 1, 0, 1)), ("timed-out", (1, 1, 0, 0)), ("invalidated", (1, 0, 0, 0)), ("completed", (1, 1, 0, 0))])
def test_failed_or_invalid_completed_hook_never_authorizes_pin(policy, status, counts):
    delivered(policy)
    markers(policy, click=lambda: markers(policy, status=status, counts=counts))
    with pytest.raises(Blocked):
        policy.probe("request", "session", 1)
    assert policy.artifacts["strict_home_refresh"]["phase"] == "attempted"
    assert "pin_dispatch_started" not in policy.artifacts


def test_probe_intent_precedes_click_and_lost_response_never_retries(policy):
    delivered(policy)
    def lost():
        assert policy.artifacts["strict_home_refresh"]["phase"] == "attempted"
        raise OSError("uncertain")
    trigger = markers(policy, click=lost)[3]
    for _ in range(2):
        with pytest.raises(Blocked) as error:
            policy.probe("request", "session", 1)
        assert error.value.code == "STRICT_PROBE_UNCERTAIN"
    assert trigger.clicks == 1
    with pytest.raises(Blocked):
        policy.bind_phone(policy.phone)


@pytest.mark.parametrize("failure", ["timeout", "remount", "regression", "pid"])
def test_probe_lifecycle_failure_preserves_attempt_and_blocks_retry(policy, failure, monkeypatch):
    delivered(policy)
    if failure == "remount":
        callback = lambda: markers(policy, scope="session:2", status="completed", counts=(1, 1, 1, 0))
    elif failure == "pid":
        callback = lambda: setattr(policy.phone.driver, "pid", 124)
    elif failure == "regression":
        states = iter([{"scope": "qa-home-refresh-scope:session:1", "status": "idle", "counts": (0, 0, 0, 0)}] * 2 + [
            {"scope": "qa-home-refresh-scope:session:1", "status": "running", "counts": (1, 1, 0, 0)},
            {"scope": "qa-home-refresh-scope:session:1", "status": "running", "counts": (1, 0, 0, 0)}])
        monkeypatch.setattr(policy, "read_probe", lambda _: next(states))
        callback = None
    else:
        callback = None
    trigger = markers(policy, click=callback)[3]
    with pytest.raises(Blocked):
        policy.probe("request", "session", .5)
    assert trigger.clicks == 1
    assert policy.artifacts["strict_home_refresh"]["phase"] == "attempted"


def test_correlated_completion_required_before_pin_and_final_acceptance(policy):
    complete(policy)
    policy.require_completion("request", "session")
    # A completed probe is read back at the original still-empty mount, never clicked twice.
    policy.probe("request", "session", 1)
    assert policy.phone.driver.controls[3].clicks == 0


@pytest.mark.parametrize("field", ["strict_delivery", "strict_home_refresh", "approval_context", "pin_dispatch_started", "strict_pin_input", "strict_idle", "approval_started"])
def test_valid_signature_alone_cannot_replace_any_ui_proof(policy, field):
    complete(policy)
    del policy.artifacts[field]
    with pytest.raises(Blocked):
        policy.require_completion("request", "session")


@pytest.mark.parametrize("field,value", [("binding", {}), ("intent_sha256", "other"), ("fee_sats", 2), ("process", {"pid": 999}), ("device", "other")])
def test_final_proof_rejects_corrupted_pin_context(policy, field, value):
    complete(policy)
    policy.artifacts["approval_context"][field] = value
    policy.artifacts["pin_dispatch_started"]["context"] = deepcopy(policy.artifacts["approval_context"])
    with pytest.raises(Blocked):
        policy.require_completion("request", "session")


def synthetic_runner(policy, result):
    runner = object.__new__(FreshRunner)
    runner.journal, runner.strict, runner.phone = policy.journal, policy, policy.phone
    runner.request_id, runner.intent, runner.deadline = "request", BINDING["intent"], 0
    runner.deferred_phone_purpose = None
    runner.native_call = lambda *args, **kwargs: deepcopy(result)
    runner.approval_binding = lambda: deepcopy(policy.artifacts["approval_binding"])
    runner.onboarding = SimpleNamespace(store=SimpleNamespace(load=lambda: {}))
    return runner


@pytest.mark.parametrize("status,approved", [(1, False), (2, True), (3, False), (4, False)])
def test_stage_sign_never_opens_queue_before_ack(policy, status, approved):
    requested(policy)
    runner = synthetic_runner(policy, {"ready": False, "status": status, "submitted_signature_count": 0})
    calls = []
    def approve():
        policy.require_delivery("request", "session")
        calls.append("review")
    runner.approve_phone = approve
    if approved:
        runner.stage_sign()
    else:
        with pytest.raises(Blocked):
            runner.stage_sign()
    assert calls == (["review"] if approved else [])
    assert "request_discovery_restart" not in policy.artifacts


@pytest.mark.parametrize("stage", ["stage_sign", "stage_collect", "stage_broadcast"])
@pytest.mark.parametrize("already_broadcast", [False, True])
def test_early_ready_and_broadcast_paths_require_actual_strict_proof(policy, stage, already_broadcast):
    requested(policy)
    runner = synthetic_runner(policy, {"ready": True, "signature_count_min": 2})
    if already_broadcast:
        policy.journal.artifact("already_broadcast", True)
    operation_under_test = getattr(runner, stage)
    with pytest.raises(Blocked):
        operation_under_test()
    assert "signature_count_min" not in policy.artifacts
    assert "broadcast_txid" not in policy.artifacts


def test_ready_collection_accepts_complete_ui_proof_and_pinned_process_only(policy):
    complete(policy)
    runner = synthetic_runner(policy, {"ready": True, "signature_count_min": 2})
    assert runner.collect_once() is True
    assert policy.artifacts["signature_count_min"] == 2
    policy.phone.driver.pid += 1
    with pytest.raises(Blocked):
        runner.collect_once()


@pytest.mark.parametrize("method", ["stage_phone_setup", "stage_phone"])
def test_resumed_stages_do_not_restore_or_refresh_post_request(policy, method):
    requested(policy)
    runner = synthetic_runner(policy, {})
    runner.onboarding = None  # Any setup activity would fail this test.
    getattr(runner, method)()
    assert policy.clock.now == 90


@pytest.mark.parametrize("method,args", [("recover_key_generation", ({},)), ("upgrade_approval", ()), ("restart_request_discovery", ())])
def test_runner_recovery_entrypoints_cannot_consume_allowance(policy, method, args):
    runner = synthetic_runner(policy, {})
    operation_under_test = getattr(runner, method)
    with pytest.raises(Blocked) as error:
        operation_under_test(*args)
    assert error.value.code == "STRICT_RESTART"


def pin_ready(policy):
    delivered(policy)
    phone = policy.phone
    context = phone.approval_context("request", "session", BINDING["intent"], 1000, policy.journal)
    policy.journal.artifact("approval_started", True)
    policy.journal.artifact("approval_context", context)
    return phone


def test_strict_first_pin_uses_actual_refresh_not_background_and_cannot_repeat(policy):
    phone = pin_ready(policy)
    events = []
    def refreshed():
        events.append("home-refresh")
        markers(policy, status="completed", counts=(1, 1, 1, 0))
    trigger = markers(policy, click=refreshed)[3]
    phone.click = lambda digit: events.append("digit")
    def provider():
        assert policy.artifacts["strict_home_refresh"]["phase"] == "completed"
        assert "pin_dispatch_started" in policy.artifacts
        events.append("credential-cas")
        return "123456"
    assert phone.enter_first_pin("request", "session", BINDING["intent"], 1000, policy.journal, provider, lambda: True)
    assert events == ["home-refresh", "credential-cas"] + ["digit"] * 6
    assert trigger.clicks == 1
    assert "pin_resume_probe" not in policy.artifacts
    policy.require_completion("request", "session")
    with pytest.raises(Blocked) as error:
        phone.enter_first_pin("request", "session", BINDING["intent"], 1000, policy.journal, provider, lambda: True)
    assert error.value.code == "PIN_ATTEMPT_UNCERTAIN"
    assert events.count("credential-cas") == 1


@pytest.mark.parametrize("failure", ["refresh_timeout", "remote_advanced", "process_changed", "lost_digit"])
def test_pin_dispatch_never_precedes_refresh_and_final_remote_process_checks(policy, failure):
    phone = pin_ready(policy)
    markers(policy, click=None if failure == "refresh_timeout" else
            lambda: markers(policy, status="completed", counts=(1, 1, 1, 0)))
    provider_calls = []
    phone.click = lambda _: None
    def provider():
        provider_calls.append(True)
        return "123456"
    checks = 0
    def before_pin():
        nonlocal checks
        checks += 1
        if checks == 2 and failure == "remote_advanced":
            return False
        if checks == 2 and failure == "process_changed":
            phone.driver.pid += 1
        return True
    if failure == "lost_digit":
        def lost(_):
            raise OSError("uncertain input response")
        phone.click = lost
    if failure == "remote_advanced":
        assert phone.enter_first_pin("request", "session", BINDING["intent"], 1000, policy.journal, provider, before_pin) is False
    else:
        with pytest.raises((Blocked, OSError)):
            phone.enter_first_pin("request", "session", BINDING["intent"], 1000, policy.journal, provider, before_pin)
    assert provider_calls == ([True] if failure == "lost_digit" else [])
    assert "strict_pin_input" not in policy.artifacts


def test_completed_probe_resume_does_not_click_again_before_first_pin(policy):
    phone = pin_ready(policy)
    markers(policy, click=lambda: markers(policy, status="completed", counts=(1, 1, 1, 0)))
    policy.probe("request", "session", 1)
    strict.StrictPolicy(policy.journal).bind_phone(phone)
    digits = []
    phone.click = digits.append
    assert phone.enter_first_pin("request", "session", BINDING["intent"], 1000, policy.journal, lambda: "123456", lambda: True)
    assert digits == list("123456")
    assert phone.driver.controls[3].clicks == 0


@pytest.mark.parametrize("boundary", ["remote_guard", "credential_provider", "second_digit"])
def test_same_session_same_pid_remount_never_reuses_old_refresh_proof(policy, boundary):
    phone = pin_ready(policy)
    markers(policy, click=lambda: markers(policy, status="completed", counts=(1, 1, 1, 0)))
    events, guards = [], []
    def remount():
        # Even another completed refresh in a different mount cannot replace A.
        markers(policy, scope="session:2", status="completed", counts=(1, 1, 1, 0))
    def before_pin():
        guards.append(True)
        if boundary == "remote_guard" and len(guards) == 2:
            remount()
        return True
    def provider():
        events.append("provider")
        if boundary == "credential_provider":
            remount()
        return "123456"
    def click(_):
        events.append("digit")
        if boundary == "second_digit":
            remount()
    phone.click = click
    with pytest.raises(Blocked) as error:
        phone.enter_first_pin("request", "session", BINDING["intent"], 1000, policy.journal, provider, before_pin)
    assert error.value.code == "STRICT_SCOPE"
    assert events == {"remote_guard": [], "credential_provider": ["provider"], "second_digit": ["provider", "digit"]}[boundary]
    assert ("pin_dispatch_started" in policy.artifacts) is (boundary != "remote_guard")
    assert "strict_pin_input" not in policy.artifacts


def test_request_mutation_only_after_idle_and_full_phone_validation(policy):
    runner = synthetic_runner(policy, {})
    runner.run_id = "run"
    artifacts = {"account_identity": "account", "desktop_identity": "desktop", "request_id": "request",
                 "intent": BINDING["intent"], "psbt_sha256": "a" * 64, "descriptor_id": "descriptor",
                 "target_key_id": "key", "target_fingerprint": "01234567", "cube_id": 1,
                 "cube_uuid": "cube", "vault_id": "vault", "fee_sats": 1000,
                 "phone_validation": {"account": "account", "device": "device", "network": "testnet4", "source": "settings_ui"},
                 "phone": {"build": 39, "network": "testnet4", "cube_visible": True}}
    for name, value in artifacts.items():
        policy.journal.artifact(name, value)
    runner.core = SimpleNamespace(chain=lambda: None)
    mutations = []
    def request(action):
        assert action == "request"
        assert policy.clock.now == 90
        assert policy.artifacts["strict_idle"]["phase"] == "completed"
        assert policy.artifacts["strict_request"]["binding"]["target_key_id"] == "key"
        mutations.append(action)
        return {"session_id": "session"}
    runner.native_call = request
    runner.stage_request()
    assert mutations == ["request"]
    assert policy.artifacts["strict_request"]["session_id"] == "session"
    artifacts["phone_validation"]["network"] = "bitcoin"
    policy.journal.artifact("phone_validation", artifacts["phone_validation"])
    with pytest.raises(Failed) as error:
        runner.stage_request()
    assert error.value.code == "STRICT_PHONE_VALIDATION"
    assert mutations == ["request"]


@pytest.mark.parametrize("labels,code", [
    ({"Pending Requests"}, "REQUEST_DISCOVERY_NAVIGATION_UNSUPPORTED"),
    ({"Signer key", "Back"}, "STRICT_RESTART"),
])
def test_restore_navigation_cannot_restart_even_before_request(policy, labels, code):
    policy.phone.labels = lambda: labels
    with pytest.raises(Blocked) as error:
        policy.phone.restore_navigation()
    assert error.value.code == code
