from types import SimpleNamespace

import pytest

from harness.phone import Phone
from harness.state import Blocked, Failed, Journal


INTENT = {"outputs": [{"address": "tb1fixture-recipient", "sats": 10000},
                      {"address": "tb1fixture-change", "sats": 989388}]}
REVIEW = {"Approve transaction", "testnet4", "612 sats", "10000 sats", "989388 sats",
          "tb1fixture-recipient", "tb1fixture-change"}
HOME = {"Home", "Cubes", "Settings"}
INCOMING = {"Incoming transaction", "Review the details before approving", "View transaction", "Not now"}


def scripted_phone(views):
    phone = Phone(SimpleNamespace(execute_script=lambda *args: None), 35)
    screens = iter(views)
    phone.labels = lambda: next(screens)
    clicks = []
    phone.click = clicks.append
    phone.click_line = clicks.append
    phone.refresh = lambda: None
    return phone, clicks


@pytest.mark.parametrize("restored,expected", [
    ([{"Signature Submitted", "Done"}], ["Done"]),
    ([{"Transaction approved", "Done"}], ["Done"]),
    ([REVIEW | {"Back"}], ["Back"]),
    ([{"Enter PIN", "Back", "1", "2"}, REVIEW | {"Back"}], ["Back", "Back"]),
    ([{"Create new signer key", "Create Key", "Cancel", "Home", "Cubes", "Settings"}], ["Cancel"]),
])
def test_fixture_check_resumes_without_approval_or_pin(restored, expected, monkeypatch):
    monkeypatch.setattr("harness.phone.time.sleep", lambda _: None)
    phone, clicks = scripted_phone(restored + [
        {"Home", "Cubes", "Settings"}, {"Testnet4"}, {"Disposable QA Cube"},
    ])
    assert phone.check_fixture("Disposable QA Cube")["cube_visible"] is True
    assert clicks == expected + ["Settings", "Cubes", "Home"]


def test_unknown_restored_screen_never_guesses_navigation(monkeypatch):
    monkeypatch.setattr("harness.phone.time.sleep", lambda _: None)
    phone, clicks = scripted_phone([{"Approve", "Try Again", "Done", "Enter PIN"}] * 16)
    with pytest.raises(Blocked) as error:
        phone.restore_navigation()
    assert error.value.code == "PHONE_NAVIGATION"
    assert clicks == []


def test_result_then_unlabeled_pending_page_blocks_without_restart(monkeypatch):
    monkeypatch.setattr("harness.phone.time.sleep", lambda _: None)
    phone, clicks = scripted_phone([
        {"Signature Submitted", "Done"}, {"Pending Requests"}, set(),
        {"Home", "Cubes", "Settings"}, {"Testnet4"}, {"Disposable QA Cube"},
    ])
    lifecycle = []
    phone.driver.terminate_app = lambda bundle: lifecycle.append(("terminate", bundle))
    phone.driver.activate_app = lambda bundle: lifecycle.append(("activate", bundle))
    phone.driver.find_elements = lambda *args: []
    with pytest.raises(Blocked) as error:
        phone.check_fixture("Disposable QA Cube")
    assert error.value.code == "REQUEST_DISCOVERY_NAVIGATION_UNSUPPORTED"
    assert clicks == ["Done"]
    assert lifecycle == []


def test_pending_page_never_uses_a_restart_fallback(monkeypatch):
    monkeypatch.setattr("harness.phone.time.sleep", lambda _: None)
    phone, clicks = scripted_phone([{"Pending Requests"}] * 16)
    lifecycle = []
    phone.driver.terminate_app = lambda _: lifecycle.append("terminate")
    phone.driver.activate_app = lambda _: lifecycle.append("activate")
    phone.driver.find_elements = lambda *args: []
    with pytest.raises(Blocked) as error:
        phone.restore_navigation()
    assert error.value.code == "REQUEST_DISCOVERY_NAVIGATION_UNSUPPORTED"
    assert lifecycle == []
    assert clicks == []


def test_approval_never_selects_an_unrelated_request(tmp_path):
    phone, clicks = scripted_phone([HOME, {"some other request"}])
    journal = Journal(tmp_path / "run", {})
    try:
        with pytest.raises(Blocked) as error:
            phone.approve("fixture", INTENT, 612, None, journal, timeout=0)
        assert error.value.code == "REQUEST_DELIVERY"
        assert "Approve" not in clicks
        assert not journal.data["artifacts"].get("approval_started")
    finally:
        journal.close()


def test_uncertain_approval_never_retries_pin(tmp_path):
    phone, clicks = scripted_phone([{"Testnet4 QA fixture"}, REVIEW])
    journal = Journal(tmp_path / "run", {})
    journal.artifact("approval_started", True)
    try:
        with pytest.raises(Blocked) as error:
            phone.approve("fixture", INTENT, 612, None, journal, timeout=2)
        assert error.value.code == "APPROVAL_UNCERTAIN"
        assert "Approve" not in clicks
    finally:
        journal.close()


def test_wrong_phone_summary_never_approves(tmp_path, monkeypatch):
    monkeypatch.setattr("harness.phone.time.sleep", lambda _: None)
    wrong = REVIEW - {"tb1fixture-recipient"}
    phone, clicks = scripted_phone([HOME, {"Testnet4 QA fixture"}] + [wrong] * 8)
    journal = Journal(tmp_path / "run", {})
    try:
        with pytest.raises(Failed) as error:
            phone.approve("fixture", INTENT, 612, None, journal, timeout=2)
        assert error.value.code == "PHONE_REVIEW"
        assert "Approve" not in clicks
    finally:
        journal.close()


def test_real_banner_and_pin_flow_use_exact_labels(tmp_path, monkeypatch):
    monkeypatch.setattr("harness.phone.time.sleep", lambda _: None)
    phone, clicks = scripted_phone([HOME, {"1 signing request waiting"}, {"Testnet4 QA fixture"}, REVIEW,
                                   {"Enter PIN"}, {"Signature Submitted"}])
    pin = tmp_path / "pin"
    pin.write_text("987654")
    pin.chmod(0o600)
    journal = Journal(tmp_path / "run", {})
    try:
        phone.approve("fixture", INTENT, 612, str(pin), journal, timeout=5)
        assert clicks == ["Home", "1 signing request waiting", "Testnet4 QA fixture", "Approve", *"987654"]
        assert journal.data["artifacts"]["approval_started"] is True
        assert "987654" not in journal.path.read_text()
    finally:
        journal.close()


def test_key_creation_success_returns_home_without_retrying_creation(monkeypatch):
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    phone, clicks = scripted_phone([{'Signer key created', 'Back to home'}, {'Home', 'Cubes', 'Settings'}])
    phone.restore_navigation()
    assert clicks == ['Back to home']


@pytest.mark.parametrize('survives', [True, False])
def test_pin_resume_probe_records_only_observed_lifecycle_survival(tmp_path, monkeypatch, survives):
    ticks = iter([0, 0, 4])
    monkeypatch.setattr('harness.phone.time.monotonic', lambda: next(ticks))
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    phone, clicks = scripted_phone([{'Enter PIN'} if survives else {'Approve transaction'}] * 2)
    lifecycle = []
    phone.driver.execute_script = lambda *args: lifecycle.append(args)
    phone.driver.activate_app = lambda bundle: lifecycle.append(('activate', bundle))
    journal = Journal(tmp_path / 'probe', {})
    try:
        if survives:
            phone.verify_pin_resume(journal)
            result = journal.data['artifacts']['pin_resume_probe']
            assert result['status'] == 'passed'
            assert result['home_refresh_proven'] is False
        else:
            with pytest.raises(Failed) as error:
                phone.verify_pin_resume(journal)
            assert error.value.code == 'PIN_CHALLENGE_DISMISSED'
        assert clicks == []
        assert lifecycle[0] == ('mobile: backgroundApp', {'seconds': 1})
        assert lifecycle[1] == ('activate', 'io.coincube.keychainApp')
    finally:
        journal.close()


def test_merged_setup_card_matches_only_complete_title_line():
    from appium.webdriver.common.appiumby import AppiumBy
    clicked, predicates = [], []
    def find(by, predicate):
        assert by == AppiumBy.IOS_PREDICATE
        predicates.append(predicate)
        return [SimpleNamespace(click=lambda: clicked.append(True))]
    phone = Phone(SimpleNamespace(find_elements=find), 36)
    phone.click_card('Use a PIN')
    assert clicked == [True]
    assert 'BEGINSWITH "Use a PIN\\n"' in predicates[0]
    assert 'CONTAINS' not in predicates[0]
    assert 'visible == true AND enabled == true' in predicates[0]
    assert 'XCUIElementTypeStaticText' in predicates[0]
    with pytest.raises(Failed):
        phone.click_card('1')
    assert len(predicates) == 1


def test_merged_card_selector_refuses_ambiguous_ancestors():
    phone = Phone(SimpleNamespace(find_elements=lambda *args: [object(), object()]), 36)
    with pytest.raises(Blocked) as error:
        phone.click_card('Use a PIN')
    assert error.value.code == 'PHONE_SELECTOR'


def test_setup_card_waits_for_readiness_without_broadening_predicate(monkeypatch):
    waits, clicks, queries = [], [], []
    results = iter([[], [], [SimpleNamespace(click=lambda: clicks.append(True))]])
    def find(by, predicate):
        queries.append((by, predicate))
        return next(results)
    monkeypatch.setattr('harness.phone.time.sleep', waits.append)
    phone = Phone(SimpleNamespace(find_elements=find), 37)
    phone.click_card('Use a PIN')
    assert waits == [.25, .25]
    assert clicks == [True]
    assert len(set(queries)) == 1


def test_setup_card_absence_is_bounded_and_never_clicks():
    phone = Phone(SimpleNamespace(find_elements=lambda *args: []), 37)
    with pytest.raises(Blocked) as error:
        phone.click_card('Use a PIN', timeout=0)
    assert error.value.code == 'PHONE_SELECTOR'


def test_setup_card_never_retries_an_uncertain_click(monkeypatch):
    clicks = []
    def click():
        clicks.append(True)
        raise RuntimeError('Synthetic lost click response')
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: pytest.fail('must not retry'))
    phone = Phone(SimpleNamespace(find_elements=lambda *args: [SimpleNamespace(click=click)]), 37)
    with pytest.raises(RuntimeError):
        phone.click_card('Use a PIN')
    assert clicks == [True]


def test_exact_action_waits_for_enabled_control_after_success_heading(monkeypatch):
    waits, clicks, labels = [], [], []
    results = iter([[], [], [SimpleNamespace(click=lambda: clicks.append('back'))]])
    phone = Phone(SimpleNamespace(), 38)
    phone.labels = lambda: labels.append('guard') or {'Signer key created'}
    phone.matching_controls = lambda label: next(results) if label == 'Back to home' else pytest.fail('wrong label')
    monkeypatch.setattr('harness.phone.time.sleep', waits.append)
    phone.click('Back to home')
    assert waits == [.25, .25]
    assert clicks == ['back']
    assert len(labels) == 3


@pytest.mark.parametrize('count', [0, 2])
def test_exact_action_missing_or_ambiguous_never_dispatches(monkeypatch, count):
    phone = Phone(SimpleNamespace(), 38)
    phone.labels = lambda: HOME
    phone.matching_controls = lambda _: [object()] * count
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: pytest.fail('must not wait'))
    with pytest.raises(Blocked) as error:
        phone.click('Back to home', timeout=0 if count == 0 else 15)
    assert error.value.code == 'PHONE_SELECTOR'


def test_exact_action_never_retries_lost_click_response(monkeypatch):
    calls = []
    def dispatch():
        calls.append('click')
        raise RuntimeError('Synthetic lost click response')
    phone = Phone(SimpleNamespace(), 38)
    phone.labels = lambda: HOME
    phone.matching_controls = lambda _: [SimpleNamespace(click=dispatch)]
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: pytest.fail('must not retry'))
    with pytest.raises(RuntimeError):
        phone.click('Back to home')
    assert calls == ['click']


def test_exact_action_rechecks_foreground_before_waited_click(monkeypatch):
    phone = Phone(SimpleNamespace(), 38)
    guards = []
    def guard():
        guards.append(True)
        if len(guards) == 2:
            raise Blocked('PHONE_AUTH', 'Synthetic foreground auth transition')
        return HOME
    phone.labels = guard
    queries = []
    phone.matching_controls = lambda _: queries.append(True) or []
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    with pytest.raises(Blocked) as error:
        phone.click('Back to home')
    assert error.value.code == 'PHONE_AUTH'
    assert queries == [True]


def test_gesture_card_uses_only_explicit_container_and_exact_title_line():
    queries, clicks = [], []
    def find(by, predicate):
        queries.append(predicate)
        return [SimpleNamespace(click=lambda: clicks.append(True))]
    phone = Phone(SimpleNamespace(find_elements=find), 37)
    control = phone.card_control('QA synthetic cube', container=True)
    assert clicks == []  # Caller can record durable intent before dispatch.
    control.click()
    assert clicks == [True]
    assert 'type == "XCUIElementTypeOther"' in queries[0]
    assert 'BEGINSWITH "QA synthetic cube\\n"' in queries[0]
    assert 'CONTAINS' not in queries[0]


def test_persisted_key_detail_with_disabled_back_relaunches_without_reset(monkeypatch):
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    phone, clicks = scripted_phone([{'Signer key', 'Back'}, {'Home', 'Cubes', 'Settings'}])
    phone.matching_controls = lambda label: []
    lifecycle = []
    phone.driver.terminate_app = lambda bundle: lifecycle.append(('terminate', bundle))
    phone.driver.activate_app = lambda bundle: lifecycle.append(('activate', bundle))
    phone.restore_navigation()
    assert clicks == []
    assert lifecycle == [('terminate', 'io.coincube.keychainApp'), ('activate', 'io.coincube.keychainApp')]


def test_incoming_sheet_is_dismissed_before_exact_queue_selection(tmp_path, monkeypatch):
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    phone, clicks = scripted_phone([INCOMING, HOME, INCOMING,
        {'1 signing request waiting'}, {'Testnet4 QA fixture'}, REVIEW, {'Signature Submitted'}])
    journal = Journal(tmp_path / 'incoming', {})
    try:
        phone.approve('fixture', INTENT, 612, None, journal, timeout=5)
        assert clicks == ['Not now', 'Home', 'Not now', '1 signing request waiting',
                          'Testnet4 QA fixture', 'Approve']
        assert 'View transaction' not in clicks
    finally:
        journal.close()


def test_not_now_on_unrelated_screen_is_not_dismissed():
    phone, clicks = scripted_phone([])
    assert not phone.dismiss_incoming({'Signer store needs repair', 'Not now'})
    assert clicks == []


def line_element(identifier, descendants=(), clicks=None):
    def find(by, query):
        assert by == 'xpath'
        assert query == 'descendant::*'
        return list(descendants)
    return SimpleNamespace(id=identifier, find_elements=find,
                           click=lambda: clicks.append(identifier))


def test_request_line_selects_only_leaf_and_preserves_complete_line_boundaries():
    clicks, queries = [], []
    leaf = line_element('request-card', clicks=clicks)
    ancestor = line_element('screen', [leaf], clicks)
    def find(by, predicate):
        queries.append(predicate)
        return [ancestor, leaf]
    phone = Phone(SimpleNamespace(find_elements=find), 37)
    phone.click_line('Testnet4 QA fixture')
    assert clicks == ['request-card']
    query = queries[0]
    assert 'ENDS WITH' not in query
    assert 'ENDSWITH "\\nTestnet4 QA fixture"' in query
    assert 'CONTAINS "\\nTestnet4 QA fixture\\n"' in query
    assert 'CONTAINS "Testnet4 QA fixture"' not in query
    assert 'visible == true AND enabled == true' in query


@pytest.mark.parametrize('count', [0, 2])
def test_missing_or_duplicate_request_lines_never_tap(count):
    clicks = []
    elements = [line_element(str(i), clicks=clicks) for i in range(count)]
    phone = Phone(SimpleNamespace(find_elements=lambda *a: elements), 37)
    with pytest.raises(Blocked) as error:
        phone.click_line('Testnet4 QA fixture')
    assert error.value.code == 'PHONE_SELECTOR'
    assert clicks == []


def test_line_selector_cannot_be_used_for_pin_digits():
    phone = Phone(SimpleNamespace(), 37)
    with pytest.raises(Failed) as error:
        phone.click_line('1')
    assert error.value.code == 'PHONE_LINE'


def bound_phone():
    calls = []
    driver = SimpleNamespace(execute_script=lambda *args: {'bundleId': 'io.coincube.keychainApp', 'pid': 123},
        query_app_state=lambda _: 4, terminate_app=lambda bundle: calls.append('terminate'), activate_app=lambda bundle: calls.append('activate'))
    phone = Phone(driver, 38)
    phone.device_digest = 'synthetic-device'
    phone.matching_controls = lambda label: [object()]
    phone.labels = lambda: {'Enter PIN', 'signing-pin-session:session', 'signing-pin-entry:empty'}
    phone.click = calls.append
    return phone, calls


def bound_journal(tmp_path, phone):
    journal = Journal(tmp_path / 'bound', {})
    journal.artifact('approval_binding', {'session_id': 'session', 'request_id': 'request'})
    journal.artifact('approval_started', True)
    journal.artifact('approval_context', phone.approval_context('request', 'session', INTENT, 612, journal))
    return journal


@pytest.mark.parametrize('labels', [
    {'Enter PIN'},
    {'Enter PIN', 'signing-pin-session:other', 'signing-pin-entry:empty'},
    {'Enter PIN', 'signing-pin-session:session', 'signing-pin-entry:nonempty'},
    {'Enter PIN', 'signing-pin-session:session', 'signing-pin-entry:empty', 'Incorrect PIN'},
])
def test_bound_pin_never_enters_generic_wrong_nonempty_or_error_challenge(tmp_path, labels):
    phone, calls = bound_phone()
    journal = bound_journal(tmp_path, phone)
    phone.labels = lambda: labels
    try:
        with pytest.raises((Blocked, Failed)):
            phone.resume_first_pin('request', 'session', INTENT, 612, journal, 0,
                pin_provider=lambda: pytest.fail('credential must not be requested'), before_pin=lambda: True)
        assert calls == []
        assert 'pin_dispatch_started' not in journal.data['artifacts']
    finally:
        journal.close()


@pytest.mark.parametrize('changed', ['pid', 'device', 'build', 'intent', 'session', 'fee'])
def test_bound_pin_refuses_changed_approval_context(tmp_path, changed):
    phone, calls = bound_phone()
    journal = bound_journal(tmp_path, phone)
    intent, session, fee = INTENT, 'session', 612
    if changed == 'pid':
        phone.driver.execute_script = lambda *a: {'bundleId': 'io.coincube.keychainApp', 'pid': 456}
    elif changed == 'device':
        phone.device_digest = 'other'
    elif changed == 'build':
        phone.build = 39
    elif changed == 'intent':
        intent = {'outputs': []}
    elif changed == 'session':
        session = 'other'
    else:
        fee = 613
    try:
        with pytest.raises(Blocked):
            phone.resume_first_pin('request', session, intent, fee, journal, 0,
                pin_provider=lambda: pytest.fail('credential must not be requested'), before_pin=lambda: True)
        assert calls == []
    finally:
        journal.close()


@pytest.mark.parametrize('marker', ['pin_resume_probe', 'pin_dispatch_started'])
def test_bound_pin_never_replays_durable_attempt_marker(tmp_path, marker):
    phone, calls = bound_phone()
    journal = bound_journal(tmp_path, phone)
    journal.artifact(marker, {'status': 'started'})
    try:
        with pytest.raises(Blocked) as error:
            phone.resume_first_pin('request', 'session', INTENT, 612, journal, 0,
                pin_provider=lambda: pytest.fail('must not retry'), before_pin=lambda: True)
        assert error.value.code == 'PIN_ATTEMPT_UNCERTAIN'
        assert calls == []
    finally:
        journal.close()


def test_signature_arrival_before_pin_prevents_dispatch(tmp_path):
    phone, calls = bound_phone()
    journal = bound_journal(tmp_path, phone)
    try:
        phone.resume_first_pin('request', 'session', INTENT, 612, journal, 0,
            pin_provider=lambda: pytest.fail('already signed'), before_pin=lambda: False)
        assert calls == []
        assert 'pin_resume_probe' not in journal.data['artifacts']
    finally:
        journal.close()


def test_pin_input_records_intent_before_provider_and_never_retries_lost_click(tmp_path):
    phone, calls = bound_phone()
    journal = bound_journal(tmp_path, phone)
    phone.verify_pin_resume = lambda journal, **kw: journal.artifact('pin_resume_probe', {'status': 'passed'})
    checks = []
    def provider():
        assert journal.data['artifacts']['pin_dispatch_started']['context'] == journal.data['artifacts']['approval_context']
        calls.append('provider')
        return '987654'
    def click(digit):
        calls.append(digit)
        raise RuntimeError('Synthetic lost input response')
    phone.click = click
    try:
        with pytest.raises(RuntimeError):
            phone.resume_first_pin('request', 'session', INTENT, 612, journal, 0,
                pin_provider=provider, before_pin=lambda: checks.append(True) or True)
        assert checks == [True, True]
        assert calls == ['provider', '9']
        with pytest.raises(Blocked):
            phone.resume_first_pin('request', 'session', INTENT, 612, journal, 0,
                pin_provider=provider, before_pin=lambda: True)
        assert calls == ['provider', '9']
        assert '987654' not in journal.path.read_text()
    finally:
        journal.close()


@pytest.mark.parametrize('stopped', [True, False])
def test_upgrade_boundary_requires_verified_stop_before_activation(tmp_path, stopped):
    phone, calls = bound_phone()
    journal = bound_journal(tmp_path, phone)
    receipt = {'original_build': 37, 'current_build': 38, 'phase': 'planned'}
    states = iter([1 if stopped else 4, 4])
    phone.driver.query_app_state = lambda bundle: next(states, 4)
    def activate(bundle):
        assert journal.data['artifacts']['approval_upgrade_recovery']['phase'] == 'process_stopped'
        calls.append('activate')
    phone.driver.activate_app = activate
    try:
        if stopped:
            phone.restart_for_upgrade(journal, receipt)
            assert receipt['phase'] == 'ready_for_review'
            assert receipt['current_process']['pid'] == 123
            assert calls == ['terminate', 'activate']
        else:
            with pytest.raises(Blocked):
                phone.restart_for_upgrade(journal, receipt)
            assert calls == ['terminate']
    finally:
        journal.close()


def test_upgrade_approval_dispatch_consumes_receipt_before_uncertain_click(tmp_path):
    phone, calls = bound_phone()
    journal = bound_journal(tmp_path, phone)
    context = journal.data['artifacts']['approval_context']
    receipt = {'original_build': 37, 'current_build': 38, 'phase': 'ready_for_review',
               'binding': context['binding'], 'current_process': context['process']}
    journal.artifact('approval_upgrade_recovery', receipt)
    phone.restore_navigation = lambda: None
    phone.refresh = lambda: None
    views = iter([{'Testnet4 QA request'}, REVIEW])
    phone.labels = lambda: next(views)
    phone.click_line = calls.append
    def click(label):
        calls.append(label)
        if label == 'Approve':
            assert journal.data['artifacts']['approval_started'] is True
            assert journal.data['artifacts']['approval_upgrade_recovery']['phase'] == 'approval_dispatched'
            raise RuntimeError('Synthetic lost approval response')
    phone.click = click
    try:
        with pytest.raises(RuntimeError):
            phone.approve('request', INTENT, 612, None, journal, 10, session_id='session',
                before_approve=lambda: True, before_pin=lambda: True, pin_provider=lambda: '987654', reapproval=True)
        with pytest.raises(Blocked):
            phone.approve('request', INTENT, 612, None, journal, 10, session_id='session',
                before_approve=lambda: True, before_pin=lambda: True, pin_provider=lambda: '987654', reapproval=True)
        assert calls.count('Approve') == 1
    finally:
        journal.close()


@pytest.mark.parametrize('visible', ['true', 'false'])
def test_account_signer_limit_is_a_prompt_prerequisite_block_without_mutation(visible):
    calls = []
    driver = SimpleNamespace(
        execute_script=lambda command: {'bundleId': 'io.coincube.keychainApp'},
        page_source=f'<root><node visible="{visible}" label="Key limit reached"/></root>',
        find_elements=lambda *a: pytest.fail('must not select controls'),
        activate_app=lambda *a: pytest.fail('must not restart'),
    )
    phone = Phone(driver, 38)
    phone.click = calls.append
    if visible == 'true':
        with pytest.raises(Blocked) as error:
            phone.restore_navigation()
        assert error.value.code == 'SIGNER_LIMIT'
    else:
        assert phone.labels() == set()
    assert calls == []


def test_capacity_observation_records_current_process_without_claiming_old_outcome():
    phone, calls = bound_phone()
    phone.driver.page_source = '<root><node visible="true" label="Key limit reached"/></root>'
    del phone.labels
    observation = phone.observe_key_limit()
    assert observation == {'build': 38, 'code': 'SIGNER_LIMIT', 'process': {'pid': 123, 'bundle_id': 'io.coincube.keychainApp'},
                           'observation': 'visible_capacity_modal'}
    assert calls == []
    phone.build = 39
    with pytest.raises(Blocked):
        phone.observe_key_limit()


def test_new_generation_boundary_cannot_repeat_after_creation_dispatch(tmp_path):
    phone, calls = bound_phone()
    journal = bound_journal(tmp_path, phone)
    receipt = {'phase': 'planned', 'creation_started': False, 'pin_entered': False}
    states = iter([1, 4])
    phone.driver.query_app_state = lambda _: next(states, 4)
    try:
        phone.restart_for_key_generation(journal, receipt)
        assert receipt['phase'] == 'ready_for_reconciliation'
        assert receipt['current_process']['pid'] == 123
        assert calls == ['terminate', 'activate']
        receipt['phase'] = 'creation_dispatched'
        receipt['creation_started'] = True
        with pytest.raises(Blocked):
            phone.restart_for_key_generation(journal, receipt)
        assert calls == ['terminate', 'activate']
    finally:
        journal.close()


def home_carousel(monkeypatch, *, target=1, start=0, refreshed=False, ambiguous=False):
    """Source-backed Home ListView refresh plus horizontal PageView fixture."""
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    state = {'page': start, 'refreshed': refreshed}
    swipes, clicks = [], []
    title = 'QA exact replacement'
    def find(by, predicate):
        assert 'XCUIElementTypeOther' in predicate
        if 'label == ' in predicate:
            assert 'label == "QA exact replacement"' in predicate
            assert 'BEGINSWITH "QA exact replacement\\n"' in predicate
            assert 'CONTAINS' not in predicate
            if ambiguous:
                return [object(), object()]
            if state['refreshed'] and state['page'] == target:
                return [SimpleNamespace(id='exact-card', click=lambda: clicks.append(title))]
            return []
        assert 'BEGINSWITH "QA "' in predicate
        return [SimpleNamespace(id='visible-card-' + str(state['page']))]
    def execute(command, arguments):
        assert command == 'mobile: swipe'
        swipes.append(dict(arguments))
        direction = arguments['direction']
        if direction == 'down':
            state['refreshed'] = True  # Retain current page across the refresh.
        else:
            assert arguments['elementId'] == 'visible-card-' + str(state['page'])
            state['page'] = min(2, state['page'] + 1) if direction == 'left' else max(0, state['page'] - 1)
    phone = Phone(SimpleNamespace(find_elements=find, execute_script=execute), 38)
    phone.labels = lambda: HOME | {'Add signer key'}
    return phone, title, swipes, clicks


def test_home_key_card_refreshes_stale_list_and_scrolls_exact_horizontal_card(monkeypatch):
    phone, title, swipes, clicks = home_carousel(monkeypatch, target=1)
    control = phone.home_key_card(title)
    assert clicks == []
    assert swipes == [{'direction': 'down'}, {'direction': 'left', 'elementId': 'visible-card-0'}]
    control.click()
    assert clicks == [title]


def test_home_key_card_can_appear_from_refresh_without_scrolling(monkeypatch):
    phone, title, swipes, clicks = home_carousel(monkeypatch, target=0)
    phone.home_key_card(title).click()
    assert swipes == [{'direction': 'down'}]
    assert clicks == [title]


def test_home_key_card_sweeps_back_from_retained_nonzero_page(monkeypatch):
    phone, title, swipes, clicks = home_carousel(monkeypatch, target=0, start=1)
    phone.home_key_card(title).click()
    directions = [gesture['direction'] for gesture in swipes]
    assert directions == ['down'] + ['left'] * 8 + ['right'] * 2
    assert clicks == [title]


def test_home_key_card_ambiguity_blocks_before_refresh_swipe_or_tap(monkeypatch):
    phone, title, swipes, clicks = home_carousel(monkeypatch, ambiguous=True)
    with pytest.raises(Blocked) as error:
        phone.home_key_card(title)
    assert error.value.code == 'PHONE_SELECTOR'
    assert swipes == []
    assert clicks == []


def test_home_key_card_absence_has_hard_gesture_bound(monkeypatch):
    phone, title, swipes, clicks = home_carousel(monkeypatch, target=-1)
    with pytest.raises(Blocked) as error:
        phone.home_key_card(title)
    assert error.value.code == 'PHONE_CARD_MISSING'
    assert len(swipes) == 25  # One refresh, eight forward and sixteen reverse gestures.
    assert clicks == []


def test_home_key_card_refuses_route_change_without_another_swipe(monkeypatch):
    phone, title, swipes, clicks = home_carousel(monkeypatch)
    states = iter([HOME | {'Add signer key'}, {'Signer key', 'Back'}])
    phone.labels = lambda: next(states)
    with pytest.raises(Blocked) as error:
        phone.home_key_card(title)
    assert error.value.code == 'PHONE_HOME'
    assert swipes == [{'direction': 'down'}]
    assert clicks == []


def test_home_key_card_refuses_ambiguous_carousel_gesture_scope(monkeypatch):
    phone, title, swipes, clicks = home_carousel(monkeypatch)
    find = phone.driver.find_elements
    phone.driver.find_elements = lambda by, predicate: find(by, predicate) if 'label == ' in predicate else [object(), object()]
    with pytest.raises(Blocked) as error:
        phone.home_key_card(title)
    assert error.value.code == 'PHONE_CAROUSEL'
    assert swipes == [{'direction': 'down'}]
    assert clicks == []


def discovery_phone(*, failure=None, modal=False, signed=False):
    journal = SimpleNamespace(data={'artifacts': {}})
    journal.artifact = lambda name, value: journal.data['artifacts'].update({name: value})
    calls, identities = [], iter([10, 10, 11 if failure != 'same-pid' else 10])
    def terminate(bundle):
        assert journal.data['artifacts']['request_discovery_restart']['phase'] == 'attempted'
        calls.append('terminate')
        if failure == 'terminate': raise RuntimeError('Synthetic lost terminate response')
    def activate(bundle):
        calls.append('activate')
        if failure == 'activate': raise RuntimeError('Synthetic lost activate response')
    states = iter([2 if failure == 'not-stopped' else 1, 4])
    phone = Phone(SimpleNamespace(terminate_app=terminate, activate_app=activate,
        query_app_state=lambda _: next(states), find_elements=lambda *a: [object()] if modal else []), 38)
    phone.labels = lambda: HOME | {'Add signer key'}
    phone.process_identity = lambda: {'pid': next(identities), 'bundle_id': 'io.coincube.keychainApp'}
    receipt = {'binding': {'build': 38}}
    return phone, journal, receipt, calls


def test_discovery_restart_persists_intent_before_terminate_and_verifies_new_process():
    phone, journal, receipt, calls = discovery_phone()
    assert phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: True)
    assert calls == ['terminate', 'activate']
    saved = journal.data['artifacts']['request_discovery_restart']
    assert saved['phase'] == 'completed'
    assert saved['stopped_verified'] is True
    assert saved['before_process']['pid'] == 10
    assert saved['after_process']['pid'] == 11
    with pytest.raises(Blocked): phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: True)
    assert calls == ['terminate', 'activate']


@pytest.mark.parametrize('failure', ['terminate', 'activate', 'not-stopped', 'same-pid'])
def test_uncertain_discovery_restart_stays_attempted_and_never_repeats(failure):
    phone, journal, receipt, calls = discovery_phone(failure=failure)
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: True)
    assert error.value.code == 'REQUEST_DISCOVERY_UNCERTAIN'
    assert journal.data['artifacts']['request_discovery_restart']['phase'] == 'attempted'
    prior_calls = list(calls)
    with pytest.raises(Blocked): phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: True)
    assert calls == prior_calls


@pytest.mark.parametrize('blocked', ['native-modal', 'Enter PIN', 'Incoming transaction', 'not-home'])
def test_discovery_restart_requires_public_home_without_modal_or_authentication(blocked):
    phone, journal, receipt, calls = discovery_phone(modal=blocked == 'native-modal')
    phone.labels = lambda: ({'Signer key', 'Back'} if blocked == 'not-home' else HOME | {'Add signer key', blocked})
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: pytest.fail('Public route first'))
    assert error.value.code == 'REQUEST_DISCOVERY_HOME'
    assert calls == []
    assert not journal.data['artifacts']


def test_remote_signature_race_leaves_discovery_restart_allowance_unused():
    phone, journal, receipt, calls = discovery_phone()
    assert phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: False) is False
    assert calls == []
    assert not journal.data['artifacts']


def test_modal_arriving_during_remote_guard_prevents_discovery_restart():
    phone, journal, receipt, calls = discovery_phone()
    def incoming():
        phone.labels = lambda: HOME | {'Add signer key', 'Incoming transaction'}
        return True
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=incoming)
    assert error.value.code == 'REQUEST_DISCOVERY_HOME'
    assert calls == []
    assert not journal.data['artifacts']


@pytest.mark.parametrize('after_remote', [False, True])
def test_known_incoming_notification_selects_ordinary_discovery_without_restart(after_remote):
    phone, journal, receipt, calls = discovery_phone()
    phone.matching_controls = lambda label: [object()] if label == 'Not now' else []
    phone.labels = lambda: HOME | {'Add signer key'} if after_remote else INCOMING
    def remote():
        assert after_remote
        phone.labels = lambda: INCOMING
        return True
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=remote)
    assert error.value.code == 'REQUEST_DISCOVERY_INCOMING'
    assert calls == []
    assert not journal.data['artifacts']


@pytest.mark.parametrize('case', ['partial', 'ambiguous', 'missing-button', 'auth', 'unknown-modal', 'unrelated'])
def test_incoming_restart_refusal_is_narrow_and_never_excuses_other_screens(case):
    phone, journal, receipt, calls = discovery_phone(modal=case == 'unknown-modal')
    labels = set(INCOMING)
    if case == 'partial': labels.remove('Review the details before approving')
    if case == 'auth': labels.add('Enter PIN')
    if case == 'unrelated': labels = {'Signer store needs repair', 'Not now'}
    phone.labels = lambda: labels
    phone.matching_controls = lambda label: [object()] * (2 if case == 'ambiguous' else 0 if case == 'missing-button' else 1)
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: pytest.fail('Unknown UI must block first'))
    assert error.value.code == 'REQUEST_DISCOVERY_HOME'
    assert calls == []
    assert not journal.data['artifacts']


def test_known_incoming_does_not_hide_a_pin_rejection():
    phone, journal, receipt, calls = discovery_phone()
    phone.labels = lambda: INCOMING | {'Incorrect PIN', '2 attempts remaining'}
    phone.matching_controls = lambda label: [object()]
    with pytest.raises(Failed) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: pytest.fail('PIN rejection must block first'))
    assert error.value.code == 'PIN_REJECTED'
    assert calls == []
    assert not journal.data['artifacts']


def pending_discovery_phone(*, click_failure=False, after_back=None):
    phone, journal, receipt, calls = discovery_phone()
    phone.build = 39
    receipt.update(binding={'build': 39, 'approval_binding': {'request_id': 'exact-run'}}, timeout_sha256='a' * 64)
    view = {'labels': {'Pending Requests', 'Back'}, 'pid': 10, 'state': 4, 'back_count': 1, 'modal': False}
    phone.labels = lambda: set(view['labels'])
    phone.process_identity = lambda: {'pid': view['pid'], 'bundle_id': 'io.coincube.keychainApp'}
    def click():
        saved = journal.data['artifacts']['request_discovery_back']
        assert saved['phase'] == 'attempted'
        assert saved['binding'] == receipt['binding']
        assert saved['process']['pid'] == 10
        calls.append('Back')
        view['labels'] = HOME | {'Add signer key'}
        if after_back: after_back(view)
        if click_failure: raise RuntimeError('Lost Back response')
    control = SimpleNamespace(click=click)
    def find(_, predicate):
        if 'XCUIElementTypeAlert' in predicate:
            return [object()] if view['modal'] else []
        assert 'XCUIElementTypeButton' in predicate
        assert 'visible == true' in predicate
        assert 'enabled == true' in predicate
        return [control] * view['back_count']
    phone.driver.find_elements = find
    def terminate(_):
        assert journal.data['artifacts']['request_discovery_restart']['phase'] == 'attempted'
        calls.append('terminate')
        view['state'] = 1
    def activate(_):
        calls.append('activate')
        view.update(state=4, pid=11)
    phone.driver.terminate_app = terminate
    phone.driver.activate_app = activate
    phone.driver.query_app_state = lambda _: view['state']
    return phone, journal, receipt, calls, view


def test_pending_queue_back_is_once_journaled_and_guarded_before_restart():
    phone, journal, receipt, calls, _ = pending_discovery_phone()
    def guard():
        calls.append('guard')
        return True
    assert phone.restart_for_request_discovery(journal, receipt, before_restart=guard)
    assert calls == ['guard', 'Back', 'guard', 'terminate', 'activate']
    assert journal.data['artifacts']['request_discovery_back']['phase'] == 'completed'
    assert journal.data['artifacts']['request_discovery_restart']['phase'] == 'completed'


@pytest.mark.parametrize('count', [0, 2])
def test_pending_queue_missing_or_ambiguous_back_is_unsupported(count):
    phone, journal, receipt, calls, view = pending_discovery_phone()
    view['back_count'] = count
    if count == 0:
        phone.build = receipt['binding']['build'] = 38
        view['labels'].remove('Back')
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: pytest.fail('Unsupported route must block first'))
    assert error.value.code == 'REQUEST_DISCOVERY_NAVIGATION_UNSUPPORTED'
    assert calls == []
    assert journal.data['artifacts'] == {}


@pytest.mark.parametrize('conflict', ['Enter PIN', 'Approve transaction', 'Create new signer key', 'Home', 'Approve', 'Reject', 'Cancel', 'Done', 'modal'])
def test_pending_queue_does_not_back_out_of_conflicting_or_modal_surface(conflict):
    phone, journal, receipt, calls, view = pending_discovery_phone()
    if conflict == 'modal': view['modal'] = True
    else: view['labels'].add(conflict)
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: pytest.fail('No guard/input on conflicting screen'))
    assert error.value.code == 'REQUEST_DISCOVERY_HOME'
    assert calls == []
    assert not journal.data['artifacts']


@pytest.mark.parametrize('after_remote', [False, True])
@pytest.mark.parametrize('arrival', ['exact', 'incoming'])
def test_queue_arrival_before_back_preserves_request_without_navigation(after_remote, arrival):
    phone, journal, receipt, calls, view = pending_discovery_phone()
    phone.matching_controls = lambda _: [object()]
    def arrive():
        if arrival == 'exact': view['labels'].add('Testnet4 QA exact-run')
        else: view['labels'] = INCOMING
    if not after_remote: arrive()
    def guard():
        assert after_remote
        arrive()
        return True
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=guard)
    assert error.value.code == ('REQUEST_DISCOVERY_READY' if arrival == 'exact' else 'REQUEST_DISCOVERY_INCOMING')
    assert calls == []
    assert not journal.data['artifacts']


def test_remote_signature_arrival_on_queue_does_not_click_back():
    phone, journal, receipt, calls, _ = pending_discovery_phone()
    assert phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: False) is False
    assert calls == []
    assert not journal.data['artifacts']


@pytest.mark.parametrize('code', ['REQUEST_DISCOVERY_INPUT', 'REQUEST_DISCOVERY_ADVANCED'])
@pytest.mark.parametrize('late', [False, True])
def test_queue_native_guard_races_never_restart(code, late):
    phone, journal, receipt, calls, _ = pending_discovery_phone()
    count = 0
    def guard():
        nonlocal count
        count += 1
        if count == (2 if late else 1): raise Blocked(code, 'Synthetic guard race')
        return True
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=guard)
    assert error.value.code == code
    assert calls == (['Back'] if late else [])
    assert 'request_discovery_restart' not in journal.data['artifacts']


@pytest.mark.parametrize('when', ['remote', 'back', 'final_guard'])
def test_queue_process_change_stops_before_any_restart(when):
    phone, journal, receipt, calls, view = pending_discovery_phone(
        after_back=(lambda v: v.update(pid=12)) if when == 'back' else None)
    count = 0
    def guard():
        nonlocal count
        count += 1
        if count == (2 if when == 'final_guard' else 1) and when != 'back': view['pid'] = 12
        return True
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=guard)
    assert error.value.code == 'REQUEST_DISCOVERY_PROCESS'
    assert calls == ([] if when == 'remote' else ['Back'])
    assert 'request_discovery_restart' not in journal.data['artifacts']


def test_lost_back_response_is_durable_and_cannot_retry_even_if_home_visible():
    phone, journal, receipt, calls, _ = pending_discovery_phone(click_failure=True)
    for _ in range(2):
        with pytest.raises(Blocked) as error:
            phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: True)
        assert error.value.code == 'REQUEST_DISCOVERY_BACK_UNCERTAIN'
    assert calls == ['Back']
    assert journal.data['artifacts']['request_discovery_back']['phase'] == 'attempted'
    assert 'request_discovery_restart' not in journal.data['artifacts']


def test_returned_back_click_with_unknown_destination_is_not_completion():
    phone, journal, receipt, calls, _ = pending_discovery_phone(after_back=lambda view: view.update(labels={'Unknown route'}))
    with pytest.raises(Blocked):
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: True)
    assert calls == ['Back']
    assert journal.data['artifacts']['request_discovery_back']['phase'] == 'attempted'
    assert 'request_discovery_restart' not in journal.data['artifacts']


def test_known_incoming_after_completed_back_keeps_normal_review_race():
    phone, journal, receipt, calls, view = pending_discovery_phone()
    phone.matching_controls = lambda _: [object()]
    count = 0
    def guard():
        nonlocal count
        count += 1
        if count == 2: view['labels'] = INCOMING
        return True
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=guard)
    assert error.value.code == 'REQUEST_DISCOVERY_INCOMING'
    assert journal.data['artifacts']['request_discovery_back']['phase'] == 'completed'
    assert calls == ['Back']


def test_permanent_no_restart_decision_blocks_queue_navigation_at_phone_boundary():
    phone, journal, receipt, calls, _ = pending_discovery_phone()
    journal.artifact('request_discovery_no_restart', {'reason': 'build_changed_without_restart'})
    phone.labels = lambda: pytest.fail('Permanent disable must precede UI inspection')
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: True)
    assert error.value.code == 'REQUEST_DISCOVERY_DISABLED'
    assert calls == []


def test_null_back_receipt_cannot_be_bypassed_when_home_is_already_visible():
    phone, journal, receipt, calls, view = pending_discovery_phone()
    view['labels'] = HOME | {'Add signer key'}
    journal.artifact('request_discovery_back', None)
    with pytest.raises(Blocked) as error:
        phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: pytest.fail('Malformed Back blocks first'))
    assert error.value.code == 'REQUEST_DISCOVERY_BACK_UNCERTAIN'
    assert calls == []


@pytest.mark.parametrize('entry', ['restore', 'fixture', 'approve'])
@pytest.mark.parametrize('back_count', [0, 2])
@pytest.mark.parametrize('incoming', [False, True])
def test_actual_pending_queue_entrypoints_never_relaunch_without_unique_back(tmp_path, monkeypatch, entry, back_count, incoming):
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    pending = {'Pending Requests', 'Testnet4 QA fixture'}
    if back_count: pending.add('Back')
    phone, clicks = scripted_phone(([INCOMING] if incoming else []) + [pending])
    phone.build = 38 if not back_count else 39
    lifecycle = []
    phone.driver.terminate_app = lambda _: lifecycle.append('terminate')
    phone.driver.activate_app = lambda _: lifecycle.append('activate')
    phone.driver.find_elements = lambda *args: [SimpleNamespace(click=lambda: clicks.append('Back'))] * back_count
    journal = Journal(tmp_path / 'run', {})
    try:
        def exercise_entrypoint():
            if entry == 'restore': phone.restore_navigation()
            elif entry == 'fixture': phone.check_fixture('Disposable QA Cube')
            else: phone.approve('fixture', INTENT, 612, None, journal, timeout=1)
        with pytest.raises(Blocked) as error:
            exercise_entrypoint()
        assert error.value.code == 'REQUEST_DISCOVERY_NAVIGATION_UNSUPPORTED'
        assert lifecycle == []
        assert clicks == (['Not now'] if incoming else [])
        assert 'approval_started' not in journal.data['artifacts']
    finally:
        journal.close()


def test_actual_approve_after_exact_note_ready_cannot_take_legacy_queue_restart(tmp_path):
    phone, _, receipt, lifecycle, view = pending_discovery_phone()
    phone.build = receipt['binding']['build'] = 38
    view.update(labels={'Pending Requests', 'Testnet4 QA fixture'}, back_count=0)
    receipt['binding']['approval_binding']['request_id'] = 'fixture'
    journal = Journal(tmp_path / 'run', {})
    try:
        with pytest.raises(Blocked) as error:
            phone.restart_for_request_discovery(journal, receipt, before_restart=lambda: pytest.fail('Exact arrival must precede native restart guard'))
        assert error.value.code == 'REQUEST_DISCOVERY_READY'
        with pytest.raises(Blocked) as error:
            phone.approve('fixture', INTENT, 612, None, journal, timeout=1)
        assert error.value.code == 'REQUEST_DISCOVERY_NAVIGATION_UNSUPPORTED'
        assert lifecycle == []
        assert journal.data['artifacts'] == {}
    finally:
        journal.close()


def test_actual_approve_from_supported_queue_still_reviews_and_approves_without_lifecycle(tmp_path, monkeypatch):
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    phone, clicks = scripted_phone([
        {'Pending Requests', 'Back', 'Testnet4 QA fixture'}, HOME,
        {'Testnet4 QA fixture'}, REVIEW, {'Signature Submitted'}])
    phone.build = 39
    lifecycle = []
    phone.driver.terminate_app = lambda _: lifecycle.append('terminate')
    phone.driver.activate_app = lambda _: lifecycle.append('activate')
    def find(_, predicate):
        assert 'visible == true' in predicate
        assert 'enabled == true' in predicate
        assert 'XCUIElementTypeButton' in predicate
        return [SimpleNamespace(click=lambda: clicks.append('Back'))]
    phone.driver.find_elements = find
    journal = Journal(tmp_path / 'run', {})
    try:
        phone.approve('fixture', INTENT, 612, None, journal, timeout=1)
        assert clicks == ['Back', 'Home', 'Testnet4 QA fixture', 'Approve']
        assert journal.data['artifacts']['approval_started'] is True
        assert lifecycle == []
    finally:
        journal.close()


@pytest.mark.parametrize('incoming', [False, True])
def test_next_run_verified_onboarding_cannot_relaunch_unsupported_queue(monkeypatch, incoming):
    from harness.onboarding import Onboarding
    monkeypatch.setattr('harness.phone.time.sleep', lambda _: None)
    queue = {'Pending Requests'}
    phone, clicks = scripted_phone([queue] + ([INCOMING] if incoming else []) + [queue])
    phone.device_digest = 'synthetic-device'
    phone.build = 38
    lifecycle = []
    phone.driver.terminate_app = lambda _: lifecycle.append('terminate')
    phone.driver.activate_app = lambda _: lifecycle.append('activate')
    phone.driver.find_elements = lambda *args: []
    state = {'identity': 'synthetic-account:synthetic-device', 'verified': True, 'pin': '123456'}
    store = SimpleNamespace(load=lambda: dict(state))
    onboarding = Onboarding(phone, store, 'synthetic-account', 1)
    with pytest.raises(Blocked) as error:
        onboarding.setup()
    assert error.value.code == 'REQUEST_DISCOVERY_NAVIGATION_UNSUPPORTED'
    assert lifecycle == []
    assert clicks == (['Not now'] if incoming else [])
    assert state['verified'] is True


@pytest.mark.parametrize('state', [0, 1, 2, 3])
def test_process_identity_rejects_background_even_if_preferred_info_would_match(state):
    calls = []
    phone = Phone(SimpleNamespace(query_app_state=lambda _: state,
        execute_script=lambda *args: calls.append('identity') or {'bundleId': 'io.coincube.keychainApp', 'pid': 4022}), 39)
    with pytest.raises(Blocked) as error:
        phone.process_identity()
    assert error.value.code == 'PHONE_PROCESS'
    assert calls == []


def test_process_identity_rejects_foreground_loss_during_preferred_identity_read():
    states = iter([4, 3])
    phone = Phone(SimpleNamespace(query_app_state=lambda _: next(states),
        execute_script=lambda *args: {'bundleId': 'io.coincube.keychainApp', 'pid': 4022}), 39)
    with pytest.raises(Blocked) as error:
        phone.process_identity()
    assert error.value.code == 'PHONE_PROCESS'


class NoSystemAlert:
    @property
    def alert(self):
        from selenium.common.exceptions import NoAlertPresentException
        raise NoAlertPresentException()


@pytest.mark.parametrize('state', [3, 4])
@pytest.mark.parametrize('setting_failure', [False, True])
def test_strict_attach_selects_observation_target_before_binding_without_lifecycle(tmp_path, monkeypatch, state, setting_failure):
    import json
    from appium import webdriver
    from harness import phone as module
    from harness.strict import StrictPolicy
    events, selected = [], []
    def update(settings):
        events.append('settings')
        assert settings == {'defaultActiveApplication': module.BUNDLE}
        if setting_failure:
            raise RuntimeError('synthetic-sensitive-settings-error')
        selected.append(settings)
    def app_state(bundle):
        assert bundle == module.BUNDLE
        events.append('state')
        return state
    def identity(command):
        assert command == 'mobile: activeAppInfo'
        events.append('identity')
        return {'bundleId': module.BUNDLE, 'pid': 4022} if selected else {'bundleId': 'com.apple.springboard', 'pid': 35}
    driver = SimpleNamespace(update_settings=update, query_app_state=app_state, switch_to=NoSystemAlert(),
                             execute_script=identity, quit=lambda: events.append('quit'))
    def remote(*, options, client_config):
        events.append('remote')
        caps = options.to_capabilities()
        assert caps['appium:autoLaunch'] is False
        assert caps['appium:forceAppLaunch'] is False
        assert caps['appium:shouldTerminateApp'] is False
        return driver  # No activate/launch/terminate methods: any call fails the test.
    monkeypatch.setattr(webdriver, 'Remote', remote)
    monkeypatch.setattr(module, 'wired_device', lambda _: ({'hardwareProperties': {'udid': 'synthetic-usb-device'}}, 39))
    monkeypatch.setattr(module.tempfile, 'gettempdir', lambda: str(tmp_path))
    monkeypatch.setattr(module.usb, 'wired_phone', lambda: object())
    monkeypatch.setattr(module.usb, 'wda_get', lambda *args: {'ready': True})
    proxy = SimpleNamespace(server_address=('127.0.0.1', 12345), serve_forever=lambda: None,
                            shutdown=lambda: None, server_close=lambda: None)
    monkeypatch.setattr(module, 'USBProxy', lambda _: proxy)
    monkeypatch.setattr(module.threading, 'Thread', lambda **kwargs: SimpleNamespace(start=lambda: None))
    class Socket:
        def __enter__(self): return self
        def __exit__(self, *args): pass
        def bind(self, address): pass
        def getsockname(self): return ('127.0.0.1', 12346)
    monkeypatch.setattr(module.socket, 'socket', Socket)
    monkeypatch.setattr(module.subprocess, 'run', lambda args, **kwargs: SimpleNamespace(
        stdout=b'3.9.0' if '--version' in args else json.dumps({'xcuitest': {'version': '12.22.8'}}).encode()))
    monkeypatch.setattr(module.subprocess, 'Popen', lambda *args, **kwargs: SimpleNamespace(poll=lambda: None))
    monkeypatch.setattr(module, 'stop_owned', lambda _: None)
    monkeypatch.setattr(module, 'http', lambda *args, **kwargs: b'{"value":{"ready":true}}')
    journal = Journal(tmp_path / 'strict-run', {'strict_acceptance': {'version': 1}})
    policy = StrictPolicy(journal)
    bind = policy.bind_phone
    def ordered_bind(phone):
        events.append('bind')
        return bind(phone)
    policy.bind_phone = ordered_bind
    try:
        if setting_failure:
            context_under_test = module.attach({'minimum_build': 39}, strict=policy)
            with pytest.raises(Blocked) as error:
                with context_under_test:
                    pytest.fail('Failed preferred-app selection must not bind or fall back')
            assert error.value.code == 'APPIUM_USB'
            assert 'synthetic-sensitive-settings-error' not in str(error.value)
            assert 'strict_process' not in journal.data['artifacts']
        elif state == 4:
            with module.attach({'minimum_build': 39}, strict=policy) as attached:
                assert attached is policy.phone
                assert journal.data['artifacts']['strict_process']['process']['pid'] == 4022
        else:
            context_under_test = module.attach({'minimum_build': 39}, strict=policy)
            with pytest.raises(Blocked) as error:
                with context_under_test:
                    pytest.fail('Background Keychain must not bind')
            assert error.value.code == 'STRICT_PROCESS'
            assert 'strict_process' not in journal.data['artifacts']
        if setting_failure:
            assert events == ['remote', 'settings', 'quit']
        else:
            assert events[:3] == ['remote', 'settings', 'bind']
            assert events[-1] == 'quit'
    finally:
        journal.close()


@pytest.mark.parametrize('method', ['labels', 'process_identity'])
@pytest.mark.parametrize('alert', ['present', 'empty', 'transport-error', 'missing'])
def test_strict_observation_checks_system_alerts_independently_of_preferred_bundle(method, alert):
    from selenium.common.exceptions import NoAlertPresentException
    events = []
    secret = 'synthetic-sensitive-system-prompt'
    class Alert:
        @property
        def text(self):
            events.append('alert-text')
            if alert == 'transport-error': raise RuntimeError(secret)
            return '' if alert == 'empty' else secret
    class Switch:
        @property
        def alert(self):
            events.append('alert')
            if alert == 'missing': raise NoAlertPresentException(secret)
            return Alert()
    driver = SimpleNamespace(query_app_state=lambda _: 4, switch_to=Switch(),
        execute_script=lambda *args: {'bundleId': 'io.coincube.keychainApp', 'pid': 4022},
        page_source='<root><node visible="true" label="Home"/></root>')
    phone = Phone(driver, 39)
    phone.strict = SimpleNamespace(observe=phone.process_identity)
    if alert == 'missing':
        result = getattr(phone, method)()
        assert result == ({'Home'} if method == 'labels' else {'pid': 4022, 'bundle_id': 'io.coincube.keychainApp'})
        assert events == ['alert']
    else:
        operation_under_test = getattr(phone, method)
        with pytest.raises(Blocked) as error:
            operation_under_test()
        assert error.value.code == 'PHONE_AUTH'
        assert secret not in str(error.value)
        assert events == ['alert', 'alert-text']
