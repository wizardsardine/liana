"""Synthetic provisioning boundary tests: never contact services or a device."""
from copy import deepcopy
import hashlib
import io
import json
from types import SimpleNamespace
from urllib.error import HTTPError, URLError

import pytest

from harness import provisioning as provision
from harness.state import Blocked, Failed, private_read


TXID = "a" * 64
FUNDING_TXID = "b" * 64
ADDRESS = "tb1q" + "c" * 58


@pytest.fixture(autouse=True)
def forbid_external_io(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Synthetic provisioning tests must not use external I/O")
    monkeypatch.setattr(provision, "http", forbidden)
    monkeypatch.setattr(provision, "build_opener", forbidden)
    monkeypatch.setattr(provision.subprocess, "run", forbidden)
    monkeypatch.setattr(provision, "attach", forbidden)
    monkeypatch.setattr(provision.time, "sleep", lambda _: None)


@pytest.fixture
def config(tmp_path):
    return {"version": 2, "account_email": "qa@example.invalid", "profile_dir": str(tmp_path / "desktop"),
            "connect_rest": provision.REST, "connect_grpc": provision.GRPC,
            "ios": {"minimum_build": 35}, "amount_sats": 10000, "fee_sats": 612}


@pytest.fixture
def runner(config, tmp_path):
    result = provision.FreshRunner(config, tmp_path / "run", deadline=0)
    yield result
    result.close()


@pytest.mark.parametrize("field,value,code", [
    ("fixture", {}, "CONFIG_SCHEMA"),
    ("version", 1, "CONFIG_SCHEMA"),
    ("connect_rest", "https://dev-api.coincube.io", "CONNECT_ENVIRONMENT"),
    ("connect_grpc", "http://127.0.0.1:50051", "CONNECT_ENVIRONMENT"),
    ("profile_dir", "relative", "ABSOLUTE_PATH"),
    ("amount_sats", True, "SPEND_LIMIT"),
    ("amount_sats", 545, "SPEND_LIMIT"),
    ("amount_sats", 999, "SPEND_LIMIT"),
    ("amount_sats", 100001, "SPEND_LIMIT"),
    ("fee_sats", 10001, "SPEND_LIMIT"),
    ("ios", {"minimum_build": 34}, "IOS_BUILD"),
    ("ios", {"minimum_build": 35, "pin_file": "/fixture/pin"}, "IOS_CONFIG"),
])
def test_fresh_config_rejects_fixtures_environment_and_limits(config, field, value, code):
    config[field] = value
    with pytest.raises(Failed) as caught:
        provision.validate_config(config)
    assert caught.value.code == code


def test_native_response_cannot_select_another_chain(tmp_path, monkeypatch):
    monkeypatch.setattr(provision.subprocess, "run", lambda *a, **k: SimpleNamespace(
        returncode=0, stdout=b'{"ok":true,"network":"bitcoin"}'))
    native = provision.Provision("/synthetic/native", tmp_path, tmp_path, "run")
    with pytest.raises(Failed) as caught:
        native.call("cube")
    assert caught.value.code == "PROVISION_CHAIN"


def faucet_stub(monkeypatch, responses, journal):
    calls = []
    outcomes = iter(responses)
    def send(request, timeout):
        # The durable attempt record must exist before a network mutation.
        assert json.loads(private_read(journal.path))["artifacts"]["funding_claim_started"] is True
        assert request.get_header("User-agent") == "COINCUBE-Testnet4-QA/1.0"
        assert request.get_header("Content-type") == "application/json"
        calls.append(json.loads(request.data))
        outcome = next(outcomes)
        if isinstance(outcome, BaseException):
            raise outcome
        return io.BytesIO(json.dumps(outcome).encode())
    monkeypatch.setattr(provision, "build_opener", lambda *a: SimpleNamespace(open=send))
    return calls


def test_faucet_lost_response_is_not_reclaimed_after_process_restart(runner, config, monkeypatch):
    directory = runner.journal.directory
    calls = faucet_stub(monkeypatch, [URLError("response lost")], runner.journal)
    with pytest.raises(Blocked) as caught:
        provision.faucet_claim(ADDRESS, runner.journal)
    assert caught.value.code == "FAUCET_UNCERTAIN"
    runner.close()
    resumed = provision.FreshRunner(config, directory)
    try:
        with pytest.raises(Blocked) as caught:
            provision.faucet_claim(ADDRESS, resumed.journal)
        assert caught.value.code == "FUNDING_PENDING"
        assert len(calls) == 1
    finally:
        resumed.close()


def test_explicit_faucet_rate_limit_waits_then_allows_one_retry(runner, monkeypatch):
    clock = [1000]
    monkeypatch.setattr(provision.time, "time", lambda: clock[0])
    rate_limit = HTTPError(provision.FAUCET, 429, "rate limit", {}, io.BytesIO(b'{"retry_after":120}'))
    calls = faucet_stub(monkeypatch, [rate_limit, {"ok": True, "network": "btc-testnet", "txid": FUNDING_TXID}], runner.journal)
    with pytest.raises(Blocked) as caught:
        provision.faucet_claim(ADDRESS, runner.journal)
    assert caught.value.code == "FAUCET_RATE_LIMIT"
    assert runner.artifacts["faucet_retry_at"] == 1120
    assert runner.artifacts["funding_claim_started"] is False
    clock[0] = 1119
    with pytest.raises(Blocked):
        provision.faucet_claim(ADDRESS, runner.journal)
    assert len(calls) == 1
    clock[0] = 1120
    provision.faucet_claim(ADDRESS, runner.journal)
    assert len(calls) == 2
    assert runner.artifacts["funding_claim_txid"] == FUNDING_TXID
    assert runner.artifacts["funding_claim_started"] is True


@pytest.mark.parametrize("response", [{"ok": False}, {"ok": True, "network": "bitcoin", "txid": FUNDING_TXID}])
def test_invalid_faucet_reply_preserves_uncertain_attempt(runner, monkeypatch, response):
    calls = faucet_stub(monkeypatch, [response], runner.journal)
    with pytest.raises(Blocked) as caught:
        provision.faucet_claim(ADDRESS, runner.journal)
    assert caught.value.code == "FAUCET_RESPONSE"
    with pytest.raises(Blocked):
        provision.faucet_claim(ADDRESS, runner.journal)
    assert len(calls) == 1


def explorer_responses(monkeypatch, *, left=None, right=None, wrong_genesis=False):
    def get(url, **kwargs):
        is_left = url.startswith(provision.EXPLORERS[0])
        if url.endswith("/block-height/0"):
            return (("0" * 64) if wrong_genesis and not is_left else provision.GENESIS).encode()
        result = left if is_left else right
        return result.encode() if isinstance(result, str) else json.dumps(result).encode()
    monkeypatch.setattr(provision, "http", get)


def test_both_explorers_must_confirm_testnet4_genesis(monkeypatch):
    explorer_responses(monkeypatch, wrong_genesis=True)
    explorer_under_test = provision.Explorer()
    with pytest.raises(Failed) as caught:
        explorer_under_test.chain()
    assert caught.value.code == "EXPLORER_CHAIN"


def test_disagreeing_funding_observations_are_not_selected(monkeypatch):
    entry = {"txid": FUNDING_TXID, "vout": 0, "value": 100000}
    explorer_responses(monkeypatch, left=[entry], right=[])
    explorer_under_test = provision.Explorer()
    with pytest.raises(Blocked) as caught:
        explorer_under_test.utxos(ADDRESS)
    assert caught.value.code == "FUNDING_PROPAGATION"


@pytest.mark.parametrize("right", ["00bb", "a", "not hex"])
def test_raw_funding_requires_identical_valid_bytes(monkeypatch, right):
    explorer_responses(monkeypatch, left="00aa", right=right)
    explorer_under_test = provision.Explorer()
    with pytest.raises(Failed) as caught:
        explorer_under_test.raw(FUNDING_TXID)
    assert caught.value.code == "FUNDING_RAW"


def test_lost_faucet_reply_reconciles_address_without_another_claim(runner):
    runner.journal.artifact("receive_address", ADDRESS)
    runner.journal.artifact("funding_claim_started", True)
    runner.core = SimpleNamespace(utxos=lambda _: [(FUNDING_TXID, 1, 100000)], raw=lambda _: "00aa")
    runner.stage_funding()
    assert runner.artifacts["funding_outpoint"] == {"txid": FUNDING_TXID, "vout": 1, "value_sats": 100000}
    assert private_read(runner.journal.directory / "funding.hex") == b"00aa"
    # A later rerun keeps the original funding identity even if unavailable.
    runner.core = SimpleNamespace()
    runner.stage_funding()


@pytest.mark.parametrize("rate_limited", [False, True])
def test_existing_external_funding_is_adopted_without_a_faucet_claim(runner, rate_limited):
    runner.journal.artifact("receive_address", ADDRESS)
    if rate_limited:
        runner.journal.artifact("funding_claim_started", False)
        runner.journal.artifact("faucet_retry_at", 9999999999)
    runner.core = SimpleNamespace(utxos=lambda _: [(FUNDING_TXID, 0, 100000)], raw=lambda _: "00aa")
    runner.stage_funding()  # External I/O fixture rejects any faucet request.
    assert runner.artifacts["funding_outpoint"] == {"txid": FUNDING_TXID, "vout": 0, "value_sats": 100000}
    assert private_read(runner.journal.directory / "funding.hex") == b"00aa"
    assert not runner.artifacts.get("funding_claim_started")
    assert "funding_claim_txid" not in runner.artifacts


def test_empty_address_is_observed_before_claim_and_claim_is_not_repeated(runner, monkeypatch):
    runner.journal.artifact("receive_address", ADDRESS)
    observations = []
    def utxos(_):
        observations.append(bool(runner.artifacts.get("funding_claim_started")))
        return []
    runner.core = SimpleNamespace(utxos=utxos)
    calls = faucet_stub(monkeypatch, [{"ok": True, "network": "btc-testnet", "txid": FUNDING_TXID}], runner.journal)
    for _ in range(2):
        with pytest.raises(Blocked) as caught:
            runner.stage_funding()
        assert caught.value.code == "FUNDING_PENDING"
    assert observations == [False, True, True]
    assert len(calls) == 1


@pytest.mark.parametrize("code,exception", [("FUNDING_PROPAGATION", Blocked), ("EXPLORER_CHAIN", Failed),
                                          ("HTTP_UNAVAILABLE", Blocked)])
def test_unverifiable_empty_address_never_requests_faucet_funds(runner, code, exception):
    runner.journal.artifact("receive_address", ADDRESS)
    def unavailable(_):
        raise exception(code, "synthetic explorer failure")
    runner.core = SimpleNamespace(utxos=unavailable)
    with pytest.raises(Blocked) as caught:
        runner.stage_funding()
    assert caught.value.code == ("FUNDING_PENDING" if code == "FUNDING_PROPAGATION" else code)
    assert "funding_claim_started" not in runner.artifacts
    assert "funding_outpoint" not in runner.artifacts


def test_unverified_raw_external_funding_never_requests_replacement(runner):
    runner.journal.artifact("receive_address", ADDRESS)
    def disagree(_):
        raise Failed("FUNDING_RAW", "synthetic disagreement")
    runner.core = SimpleNamespace(utxos=lambda _: [(FUNDING_TXID, 0, 100000)], raw=disagree)
    with pytest.raises(Failed, match="synthetic disagreement"):
        runner.stage_funding()
    assert "funding_claim_started" not in runner.artifacts
    assert "funding_outpoint" not in runner.artifacts


@pytest.mark.parametrize("outputs,code", [
    ([(FUNDING_TXID, 0, 100000), (FUNDING_TXID, 1, 100000)], "FUNDING_AMBIGUOUS"),
    ([(FUNDING_TXID, 0, 99999)], "FUNDING_UNSUITABLE"),
])
def test_unusable_existing_outputs_do_not_trigger_additional_funding(runner, outputs, code):
    runner.journal.artifact("receive_address", ADDRESS)
    runner.core = SimpleNamespace(utxos=lambda _: outputs)
    with pytest.raises(Blocked) as caught:
        runner.stage_funding()
    assert caught.value.code == code
    assert "funding_claim_started" not in runner.artifacts


def test_saved_faucet_txid_does_not_adopt_unrelated_external_funding(runner):
    runner.journal.artifact("receive_address", ADDRESS)
    runner.journal.artifact("funding_claim_started", True)
    runner.journal.artifact("funding_claim_txid", FUNDING_TXID)
    runner.core = SimpleNamespace(utxos=lambda _: [(TXID, 0, 100000)])
    with pytest.raises(Blocked) as caught:
        runner.stage_funding()
    assert caught.value.code == "FUNDING_PENDING"
    assert "funding_outpoint" not in runner.artifacts


def test_missing_funding_after_claim_blocks_without_replacement(runner):
    runner.journal.artifact("receive_address", ADDRESS)
    runner.journal.artifact("funding_claim_started", True)
    runner.core = SimpleNamespace(utxos=lambda _: [])
    with pytest.raises(Blocked) as caught:
        runner.stage_funding()
    assert caught.value.code == "FUNDING_PENDING"
    assert "funding_outpoint" not in runner.artifacts


def test_multiple_eligible_funding_outputs_never_guess(runner):
    runner.journal.artifact("receive_address", ADDRESS)
    runner.journal.artifact("funding_claim_started", True)
    runner.core = SimpleNamespace(utxos=lambda _: [(FUNDING_TXID, 0, 100000), (FUNDING_TXID, 1, 100000)])
    with pytest.raises(Failed) as caught:
        runner.stage_funding()
    assert caught.value.code == "FUNDING_AMBIGUOUS"


def phone_key(runner, **changes):
    key = {"id": 12, "name": "QA " + runner.run_id, "network": "testnet4", "cubeId": None, "status": "active",
           "xpub": "synthetic-public-key", "fingerprint": "aabbccdd", "derivationPath": "m/48h/1h/0h/2h"}
    return dict(key, **changes)


@pytest.mark.parametrize("kind,code", [("baseline", "KEY_NOT_FRESH"), ("duplicate", "KEY_AMBIGUOUS"),
                                      ("foreign_cube", "KEY_OWNERSHIP"), ("blind", "KEY_OWNERSHIP")])
def test_new_key_must_have_exact_unambiguous_identity(runner, kind, code):
    runner.journal.artifact("cube_id", 17)
    runner.journal.artifact("phone_key_creation_started", True)
    runner.journal.artifact("baseline_key_ids", [])
    key = phone_key(runner)
    if kind == "baseline":
        runner.journal.artifact("baseline_key_ids", [str(key["id"])])
    if kind == "foreign_cube":
        key["cubeId"] = 18
    if kind == "blind":
        key["xpub"] = ""
    runner.keys = lambda: [key, deepcopy(key)] if kind == "duplicate" else [key]
    with pytest.raises(Failed) as caught:
        runner.stage_phone_key()
    assert caught.value.code == code
    assert "target_key_id" not in runner.artifacts


def test_existing_matching_label_without_creation_evidence_is_not_fresh(runner):
    runner.journal.artifact("cube_id", 17)
    runner.keys = lambda: [phone_key(runner)]
    with pytest.raises(Failed) as caught:
        runner.stage_phone_key()
    assert caught.value.code == "KEY_NOT_FRESH"


def test_resumed_key_requires_durable_baseline(runner):
    runner.journal.artifact("cube_id", 17)
    runner.journal.artifact("phone_key_creation_started", True)
    runner.keys = lambda: [phone_key(runner)]
    with pytest.raises(Failed) as caught:
        runner.stage_phone_key()
    assert caught.value.code == "KEY_NOT_FRESH"


def test_registered_key_resumes_without_creating_another(runner):
    runner.journal.artifact("cube_id", 17)
    runner.journal.artifact("phone_key_creation_started", True)
    runner.journal.artifact("baseline_key_ids", ["1", "2"])
    runner.onboarding = SimpleNamespace(reconcile_pin=lambda _: None)
    runner.keys = lambda: [phone_key(runner)]
    runner.stage_phone_key()
    assert runner.artifacts["target_key_id"] == "12"
    assert runner.artifacts["target_fingerprint"] == "aabbccdd"
    saved = json.loads(private_read(runner.journal.directory / "phone-key.json"))
    assert saved["network"] == "testnet4"
    assert saved["key_id"] == "12"
    runner.stage_phone_key()


def broadcast_ready(runner):
    runner.intent = {"txid": TXID, "inputs": [FUNDING_TXID + ":1"],
                     "outputs": [{"address": "tb1recipient", "sats": 10000}, {"address": "tb1change", "sats": 89388}]}
    runner.journal.artifact("fee_sats", 612)
    runner.core = SimpleNamespace(chain=lambda: None, known=lambda _: False)
    final = {"txid": TXID, "fee_sats": 612, "raw_tx": "00aa",
             "inputs": [{"txid": FUNDING_TXID, "vout": 1}],
             "outputs": [{"address": out["address"], "value_sats": out["sats"]} for out in runner.intent["outputs"]]}
    runner.provision = SimpleNamespace(call=lambda *a, **k: final)
    return final


@pytest.mark.parametrize("field,value", [("txid", "d" * 64), ("fee_sats", 613),
                                        ("inputs", []), ("outputs", [])])
def test_final_transaction_cannot_change_pinned_intent(runner, field, value):
    final = broadcast_ready(runner)
    final[field] = value
    with pytest.raises(Failed) as caught:
        runner.stage_broadcast()
    assert caught.value.code == "FINAL_INTENT"
    assert "broadcast_hex_sha256" not in runner.artifacts


def test_final_signed_bytes_cannot_change_after_attempt(runner):
    broadcast_ready(runner)
    runner.journal.artifact("broadcast_hex_sha256", hashlib.sha256(b"00bb").hexdigest())
    with pytest.raises(Failed) as caught:
        runner.stage_broadcast()
    assert caught.value.code == "PROVISION_CHANGED"


def test_broadcast_lost_response_reconciles_before_resending(runner, monkeypatch):
    broadcast_ready(runner)
    sent = []
    def send(url, *, body, headers):
        assert private_read(runner.journal.directory / "signed.hex") == body
        assert json.loads(private_read(runner.journal.path))["artifacts"]["broadcast_hex_sha256"] == hashlib.sha256(body).hexdigest()
        sent.append(body)
        raise Blocked("SERVICE_UNREACHABLE", "Synthetic lost response")
    monkeypatch.setattr(provision, "http", send)
    with pytest.raises(Blocked):
        runner.stage_broadcast()
    runner.core.known = lambda _: True
    runner.provision = SimpleNamespace()  # Reconciliation needs no finalization.
    runner.stage_broadcast()
    assert sent == [b"00aa"]
    assert runner.artifacts["broadcast_txid"] == TXID


def test_spent_input_does_not_create_replacement_funding(runner):
    runner.intent = {"txid": TXID}
    runner.native_call = lambda _: {"fee_sats": 612}
    runner.core = SimpleNamespace(known=lambda _: False, utxos=lambda _: [])
    runner.journal.artifact("receive_address", ADDRESS)
    runner.journal.artifact("funding_outpoint", {"txid": FUNDING_TXID, "vout": 1, "value_sats": 100000})
    with pytest.raises(Blocked) as caught:
        runner.stage_connect()
    assert caught.value.code == "INPUT_SPENT"


def test_run_identity_is_stable_isolated_and_exclusively_locked(config, tmp_path):
    first = provision.FreshRunner(config, tmp_path / "first")
    try:
        with pytest.raises(Blocked) as caught:
            provision.FreshRunner(config, tmp_path / "first")
        assert caught.value.code == "RUN_BUSY"
        other = provision.FreshRunner(config, tmp_path / "second")
        try:
            assert other.run_id != first.run_id
        finally:
            other.close()
        identity = first.run_id
    finally:
        first.close()
    resumed = provision.FreshRunner(config, tmp_path / "first")
    try:
        assert resumed.run_id == identity
    finally:
        resumed.close()


@pytest.mark.parametrize('funded,accepted', [(100111, False), (100112, True)])
def test_funding_preserves_native_minimum_change(runner, funded, accepted):
    runner.config['amount_sats'] = 98500
    runner.journal.artifact('receive_address', ADDRESS)
    runner.journal.artifact('funding_claim_started', True)
    runner.core = SimpleNamespace(utxos=lambda _: [(FUNDING_TXID, 0, funded)], raw=lambda _: '00aa')
    if accepted:
        runner.stage_funding()
        assert runner.artifacts['funding_outpoint']['value_sats'] == funded
    else:
        with pytest.raises(Blocked) as caught:
            runner.stage_funding()
        assert caught.value.code == 'FUNDING_PENDING'
        assert 'funding_outpoint' not in runner.artifacts


def test_account_credential_binding_survives_new_desktop_registration(config, tmp_path, monkeypatch):
    monkeypatch.setattr(provision.SecretStore, "load", lambda _: None)
    from harness.state import atomic_write
    config['phone_state_dir'] = str(tmp_path / 'phone-state')
    account_ids, desktop_ids, credential_paths = [], [], []
    for ordinal in range(2):
        candidate = provision.FreshRunner(config, tmp_path / f'fresh-{ordinal}')
        candidate.profile.mkdir(mode=0o700)
        (candidate.profile / 'testnet4').mkdir(mode=0o700)
        account = {'email': config['account_email'], 'device_id': f'device-{ordinal}'}
        atomic_write(candidate.profile / 'testnet4/connect.json', json.dumps({'accounts': [account], 'active_email': account['email']}).encode())
        candidate.provision = SimpleNamespace(binary='/synthetic/native', call=lambda *a, **k: dict(account, user_id=42))
        candidate.phone = SimpleNamespace(device_digest='same-phone', labels=lambda: {'Use a PIN'})
        try:
            candidate.stage_authentication()
            account_ids.append(candidate.artifacts['account_identity'])
            desktop_ids.append(candidate.artifacts['desktop_identity'])
            credential_paths.append(candidate.onboarding.store.locator)
        finally:
            candidate.close()
    assert account_ids[0] == account_ids[1]
    assert desktop_ids[0] != desktop_ids[1]
    assert credential_paths[0] == credential_paths[1]


@pytest.mark.parametrize('code,category', [('desktop_session_unavailable', Blocked),
    ('desktop_session_incomplete', Blocked), ('device_registration_uncertain', Blocked),
    ('connect_account_mismatch', Failed), ('transport_invalid', Failed)])
def test_native_bootstrap_distinguishes_prerequisites_from_assertions(tmp_path, monkeypatch, code, category):
    monkeypatch.setattr(provision.subprocess, 'run', lambda *a, **k: SimpleNamespace(
        returncode=2, stdout=json.dumps({'ok': False, 'code': code}).encode()))
    native = provision.Provision('/synthetic/native', tmp_path, tmp_path, 'run')
    with pytest.raises(category) as error:
        native.call('bootstrap_auth')
    assert type(error.value) is category


def pending_phone(runner, purpose="key", *, validated=True, marker=True):
    runner.journal.artifact("account_identity", "synthetic-account")
    runner.phone = SimpleNamespace(device_digest="synthetic-phone")
    runner.onboarding = SimpleNamespace(identity="synthetic-account:synthetic-phone")
    runner.journal.artifact("cube_id", 17)
    runner.journal.artifact("cube_name", "synthetic-cube")
    runner.journal.artifact("phone_key_creation_started", True)
    runner.journal.artifact("phone_key_pin_entered", True)
    runner.journal.artifact("baseline_key_ids", ["1", "2"])
    if validated:
        runner.journal.outcome("phone_setup", "passed")
    if purpose == "sign":
        import uuid
        intent = {"txid": TXID, "inputs": [FUNDING_TXID + ":1"], "outputs": [{"address": ADDRESS, "sats": 10000}]}
        request = str(uuid.uuid5(uuid.UUID(runner.run_id), TXID))
        for key, value in {"intent": intent, "request_id": request, "session_id": "synthetic-session",
                           "approval_started": True, "target_key_id": "12", "vault_id": 19}.items():
            runner.journal.artifact(key, value)
        runner.request_id = request
        bound_purpose = "sign:" + request
    else:
        bound_purpose = "key:QA " + runner.run_id
    state = {"identity": runner.onboarding.identity, "verified": True, "pin": "000000"}
    if marker:
        state["pending_attempt"] = bound_purpose
    return state, bound_purpose


@pytest.mark.parametrize("purpose", ["key", "sign"])
def test_bound_pending_attempt_reuses_prior_validation_without_navigation(runner, purpose):
    state, expected = pending_phone(runner, purpose)
    prior = deepcopy(runner.journal.data["stages"]["phone_setup"])
    runner.defer_pending_phone_attempt(state)
    runner.stage_phone_setup()  # No phone/onboarding navigation methods supplied.
    assert runner.deferred_phone_purpose == expected
    receipt = runner.artifacts["phone_validation_reused"]
    assert receipt["purpose"] == expected
    assert receipt["validation"]["stage_outcome"] == prior
    assert receipt["validation"]["account"] == "synthetic-account"
    assert receipt["validation"]["device"] == "synthetic-phone"
    assert receipt["validation"]["network"] == "testnet4"
    assert state["pending_attempt"] == expected


@pytest.mark.parametrize("bad", ["key:another-run", "sign:another-request", "unexpected", "", 42])
def test_foreign_or_malformed_pending_intent_blocks_before_ui(runner, bad):
    state, _ = pending_phone(runner)
    state["pending_attempt"] = bad
    with pytest.raises(Blocked) as caught:
        runner.defer_pending_phone_attempt(state)
    assert caught.value.code in ("PHONE_PENDING_FOREIGN", "PHONE_PENDING_INTENT")
    assert runner.deferred_phone_purpose is None
    assert "phone_validation_reused" not in runner.artifacts


@pytest.mark.parametrize("purpose,missing", [("key", "baseline_key_ids"), ("key", "phone_key_creation_started"),
    ("sign", "session_id"), ("sign", "intent"), ("sign", "approval_started")])
def test_pending_attempt_requires_exact_run_checkpoints(runner, purpose, missing):
    state, _ = pending_phone(runner, purpose)
    runner.artifacts.pop(missing)
    with pytest.raises(Failed) as caught:
        runner.defer_pending_phone_attempt(state)
    assert caught.value.code == "PHONE_PENDING_INTENT"


@pytest.mark.parametrize("purpose", ["key", "sign"])
def test_verified_credential_alone_cannot_skip_first_run_account_network_checks(runner, purpose):
    state, _ = pending_phone(runner, purpose, validated=False)
    with pytest.raises(Blocked) as caught:
        runner.defer_pending_phone_attempt(state)
    assert caught.value.code == "PHONE_VALIDATION_REQUIRED"
    assert "phone_validation_reused" not in runner.artifacts


def test_prior_validation_cannot_move_to_another_device(runner):
    state, _ = pending_phone(runner)
    runner.journal.artifact("phone_validation", {"account": "synthetic-account", "device": "other-phone",
                                                "network": "testnet4", "source": "settings_ui"})
    with pytest.raises(Failed) as caught:
        runner.defer_pending_phone_attempt(state)
    assert caught.value.code == "PHONE_VALIDATION_CHANGED"


def test_pending_sign_skips_all_fixture_navigation_until_verified_collection(runner, monkeypatch):
    state, purpose = pending_phone(runner, "sign")
    runner.defer_pending_phone_attempt(state)
    runner.artifacts.update(target_fingerprint="aabbccdd", cube_uuid="synthetic-cube")
    runner.account = {"tokens": {"access_token": "synthetic-token"}}
    runner.keys = lambda: [phone_key(runner, cubeId=17)]
    monkeypatch.setattr(provision, "http", lambda *a, **k: b'{"data":[{"id":12,"xpubEnvelope":{"version":1}}]}')
    result = {"vault_id": 19, "descriptor_id": "descriptor", "descriptor_file": "/synthetic/descriptor",
              "receive_address": ADDRESS}
    runner.provision = SimpleNamespace(call=lambda *a, **k: result)
    runner.stage_phone_setup()
    runner.stage_vault_creation()
    runner.stage_phone()  # Phone intentionally has no navigation methods.
    runner.collect_once = lambda: False
    runner.stage_sign()  # approval_started prevents a second review/approval.
    with pytest.raises(Blocked) as caught:
        runner.stage_collect()
    assert caught.value.code == "SIGNATURE_DELIVERY"
    assert runner.deferred_phone_purpose == purpose
    actions = []
    runner.collect_once = lambda: True
    runner.onboarding.reconcile_pin = lambda p: actions.append(("reconciled", p))
    runner.stage_phone_setup = lambda: actions.append("checked-account-network")
    runner.phone.check_fixture = lambda *a: actions.append("checked-cube") or {"network": "testnet4", "cube_visible": True}
    runner.stage_collect()
    assert actions == [("reconciled", purpose), "checked-account-network", "checked-cube"]
    assert runner.deferred_phone_purpose is None
    assert runner.artifacts["phone_validation_reused"]["purpose"] == purpose


def test_pending_key_absence_keeps_prompt_and_match_releases_navigation(runner):
    state, purpose = pending_phone(runner)
    runner.phone.labels = lambda: {"Enter your PIN"}
    runner.defer_pending_phone_attempt(state)
    runner.keys = lambda: []
    with pytest.raises(Blocked) as caught:
        runner.stage_phone_key()
    assert caught.value.code == "KEY_REGISTRATION"
    assert runner.deferred_phone_purpose == purpose
    actions = []
    runner.keys = lambda: [phone_key(runner)]
    runner.onboarding.reconcile_pin = lambda p: actions.append(("reconciled", p))
    runner.stage_phone_setup = lambda: actions.append("checked-account-network")
    runner.stage_phone_key()
    assert actions == [("reconciled", purpose), "checked-account-network"]
    assert runner.deferred_phone_purpose is None
    assert runner.artifacts["target_key_id"] == "12"


def test_approval_before_pin_provider_still_preserves_the_open_prompt(runner):
    state, purpose = pending_phone(runner, "sign", marker=False)
    runner.defer_pending_phone_attempt(state)
    runner.stage_phone_setup()
    assert runner.deferred_phone_purpose == purpose
    assert "pending_attempt" not in state


def test_authentication_loads_pending_intent_before_even_reading_phone_labels(runner, tmp_path, monkeypatch):
    from harness.state import atomic_write, digest
    runner.config["phone_state_dir"] = str(tmp_path / "phone-state")
    runner.profile.mkdir(mode=0o700)
    (runner.profile / "testnet4").mkdir(mode=0o700)
    account = {"email": runner.config["account_email"], "device_id": "synthetic-desktop"}
    atomic_write(runner.profile / "testnet4/connect.json", json.dumps({"accounts": [account], "active_email": account["email"]}).encode())
    runner.provision = SimpleNamespace(binary="/synthetic/native", call=lambda *a, **k: dict(account, user_id=42))
    def no_ui():
        raise AssertionError("Pending authentication must not inspect or navigate the phone")
    runner.phone = SimpleNamespace(device_digest="synthetic-phone", labels=no_ui)
    identity = digest({"email": account["email"].casefold(), "user_id": 42}) + ":synthetic-phone"
    state = {"identity": identity, "verified": True, "pending_attempt": "key:QA " + runner.run_id}
    monkeypatch.setattr(provision.SecretStore, "load", lambda _: deepcopy(state))
    for key, value in {"cube_id": 17, "phone_key_creation_started": True, "phone_key_pin_entered": True,
                       "baseline_key_ids": []}.items():
        runner.journal.artifact(key, value)
    runner.journal.outcome("phone_setup", "passed")
    runner.stage_authentication()
    assert runner.deferred_phone_purpose == state["pending_attempt"]


def test_first_run_phone_setup_still_checks_live_account_and_network(runner):
    calls = []
    runner.journal.artifact("account_identity", "synthetic-account")
    runner.account = {"email": "qa@example.invalid"}
    runner.onboarding = SimpleNamespace(setup=lambda: calls.append("setup") or "000000")
    runner.phone = SimpleNamespace(device_digest="synthetic-phone", restore_navigation=lambda: calls.append("navigation"),
                                  click=lambda label: calls.append(label), labels=lambda: {"qa@example.invalid", "Testnet4"})
    runner.stage_phone_setup()
    assert calls == ["setup", "navigation", "Settings", "Home"]
    assert runner.artifacts["phone_validation"] == {"account": "synthetic-account", "device": "synthetic-phone",
                                                   "network": "testnet4", "source": "settings_ui"}


def attachment_state(runner):
    runner.artifacts.update(cube_id=17, cube_uuid='synthetic-cube', cube_name='QA synthetic cube',
                            target_key_id='12', target_fingerprint='aabbccdd')
    runner.account = {'tokens': {'access_token': 'synthetic-token'}}


def test_attachment_runs_phone_reseal_once_and_reconciles_after_lost_reply(runner, monkeypatch):
    attachment_state(runner)
    key = phone_key(runner)
    runner.keys = lambda: [key]
    calls = []

    def attach(name, cube, journal):
        assert name == 'QA ' + runner.run_id
        assert cube == 'QA synthetic cube'
        journal.artifact('phone_key_attachment_started', True)
        calls.append(True)
        key['cubeId'] = 17
        raise Blocked('SYNTHETIC_LOST_REPLY', 'Synthetic lost UI response')

    runner.onboarding = SimpleNamespace(attach_key=attach)
    with pytest.raises(Blocked):
        runner.ensure_phone_key_attachment()
    urls = []
    def get(url, **kwargs):
        urls.append(url)
        assert 'body' not in kwargs  # Only Keychain mutates attachment/envelope.
        return b'{"data":[{"id":12,"xpubEnvelope":{"version":1}}]}'
    monkeypatch.setattr(provision, 'http', get)
    runner.ensure_phone_key_attachment()
    assert calls == [True]
    assert runner.artifacts['phone_key_attached'] is True
    assert urls == [provision.REST + '/api/v1/connect/cubes/synthetic-cube/keys']


@pytest.mark.parametrize('change', [{'cubeId': 99}, {'status': 'revoked'}, {'network': 'bitcoin'},
                                  {'fingerprint': '11223344'}, {'name': 'unrelated'}])
def test_attachment_refuses_changed_key_before_any_phone_action(runner, change):
    attachment_state(runner)
    runner.keys = lambda: [phone_key(runner, **change)]
    runner.onboarding = SimpleNamespace()  # Any UI action would fail the test.
    with pytest.raises(Failed) as error:
        runner.ensure_phone_key_attachment()
    assert error.value.code == 'KEY_ATTACHMENT_IDENTITY'


@pytest.mark.parametrize('bound', [False, True])
def test_uncertain_attachment_never_replays_or_accepts_missing_envelope(runner, monkeypatch, bound):
    attachment_state(runner)
    runner.artifacts['phone_key_attachment_started'] = True
    runner.keys = lambda: [phone_key(runner, cubeId=17 if bound else None)]
    runner.onboarding = SimpleNamespace()
    monkeypatch.setattr(provision, 'http', lambda *a, **k: b'{"data":[{"id":12}]}')
    with pytest.raises(Blocked) as error:
        runner.ensure_phone_key_attachment()
    assert error.value.code == 'KEY_ATTACHMENT_PENDING'
    assert not runner.artifacts.get('phone_key_attached')


def test_pending_sign_cannot_navigate_to_repair_missing_attachment(runner):
    attachment_state(runner)
    runner.deferred_phone_purpose = 'sign:synthetic-request'
    runner.keys = lambda: [phone_key(runner)]
    runner.onboarding = SimpleNamespace()
    with pytest.raises(Blocked) as error:
        runner.ensure_phone_key_attachment()
    assert error.value.code == 'KEY_ATTACHMENT_PENDING'


def test_sealed_key_resume_preserves_original_public_export(runner):
    runner.artifacts.update(cube_id=17, phone_key_creation_started=True, baseline_key_ids=[])
    runner.onboarding = SimpleNamespace(reconcile_pin=lambda _: None)
    key = phone_key(runner)
    runner.keys = lambda: [key]
    runner.stage_phone_key()
    path = runner.journal.directory / 'phone-key.json'
    original = private_read(path)
    key.update(xpub='', cubeId=17)
    runner.stage_phone_key()
    assert private_read(path) == original
    key['xpub'] = 'synthetic-different-key'
    with pytest.raises(Failed) as error:
        runner.stage_phone_key()
    assert error.value.code == 'KEY_CHANGED'


def signing_recovery(runner, *, modern=False):
    state, purpose = pending_phone(runner, "sign", marker=False)
    runner.intent = deepcopy(runner.artifacts["intent"])
    runner.phone.build = 38
    runner.artifacts.update(desktop_identity="synthetic-desktop", psbt_sha256="c" * 64,
                            descriptor_id="deadbeef", target_fingerprint="aabbccdd", cube_uuid="synthetic-cube", fee_sats=612)
    runner.onboarding.store = SimpleNamespace(load=lambda: deepcopy(state))
    runner.journal.artifact("phone", {"build": 37, "network": "testnet4", "cube_visible": True})
    runner.journal.data["stages"].update(phone={"status": "passed", "code": "", "at": 10},
                                        sign={"status": "blocked", "code": "PHONE_AUTH", "at": 11})
    if modern:
        runner.phone.build = 39
        runner.journal.artifact("approval_context", {"binding": runner.approval_binding(),
            "request_id": runner.request_id, "session_id": runner.artifacts["session_id"],
            "intent_sha256": provision.digest(runner.intent), "fee_sats": 612,
            "device": runner.phone.device_digest, "build": 38,
            "process": {"pid": 123, "bundle_id": "io.coincube.keychainApp"}})
    calls = []
    runner.native_call = lambda *a, **k: {"ready": False, "status": 3, "submitted_signature_count": 0}
    def restart(journal, receipt):
        calls.append("restart")
        receipt["phase"] = "ready_for_review"
        journal.artifact("approval_upgrade_recovery", receipt)
    def approve(*args, **kwargs):
        calls.append("approve")
        assert kwargs["reapproval"] is True
        assert kwargs["session_id"] == runner.artifacts["session_id"]
        assert kwargs["before_approve"]() is True
        assert kwargs["before_pin"]() is True
        receipt = runner.artifacts["approval_upgrade_recovery"]
        receipt["phase"] = "approval_dispatched"
        runner.journal.artifact("approval_upgrade_recovery", receipt)
    runner.phone.restart_for_upgrade = restart
    runner.phone.approve = approve
    runner.stage_phone_setup = lambda: calls.append("account-network")
    runner.stage_phone = lambda: calls.append("cube-review")
    return state, calls


@pytest.mark.parametrize("modern", [False, True])
def test_upgrade_archives_original_evidence_before_stage_overwrite_and_rechecks(runner, modern):
    state, calls = signing_recovery(runner, modern=modern)
    runner.defer_pending_phone_attempt(state)
    original = deepcopy(runner.artifacts["approval_original_attempt"])
    assert original["origin"] == ("approval-context" if modern else "legacy-stage-evidence")
    if not modern:
        assert "process" not in original
        assert "context" not in original
        assert original["sign_stage"]["code"] == "PHONE_AUTH"
    runner.journal.outcome("phone", "passed")
    runner.journal.outcome("sign", "passed")
    runner.defer_pending_phone_attempt(state)
    assert runner.artifacts["approval_original_attempt"] == original
    runner.stage_sign()
    assert calls == ["restart", "account-network", "cube-review", "approve"]
    assert runner.artifacts["approval_started"] is True
    assert runner.artifacts["approval_upgrade_recovery"]["original_attempt"] == original
    calls.clear()
    if modern:
        # Phone now owns current-context validation, including the exact PID.
        runner.phone.resume_first_pin = lambda *a, **k: calls.append("resume-only")
    runner.stage_sign()
    assert calls == (["resume-only"] if modern else [])


@pytest.mark.parametrize("field", ["pending_attempt", "pin_resume_probe", "pin_dispatch_started"])
def test_signing_pin_evidence_never_allows_upgrade_generation(runner, field):
    state, calls = signing_recovery(runner)
    if field == "pending_attempt":
        state[field] = "sign:" + runner.request_id
    else:
        runner.journal.artifact(field, {"status": "started"})
    runner.defer_pending_phone_attempt(state)
    assert "approval_original_attempt" not in runner.artifacts
    runner.stage_sign()
    assert calls == []
    assert "approval_upgrade_recovery" not in runner.artifacts


@pytest.mark.parametrize("change", ["build-missing", "build36", "build38", "build-bool", "phone-not-passed", "sign-not-blocked", "not-auth", "reversed", "missing-time", "missing-hash"])
def test_legacy_adoption_requires_conclusive_ordered_original_evidence(runner, change):
    state, calls = signing_recovery(runner)
    stages = runner.journal.data["stages"]
    if change == "build-missing": runner.artifacts["phone"].pop("build")
    if change == "build36": runner.artifacts["phone"]["build"] = 36
    if change == "build38": runner.artifacts["phone"]["build"] = 38
    if change == "build-bool": runner.artifacts["phone"]["build"] = True
    if change == "phone-not-passed": stages["phone"]["status"] = "blocked"
    if change == "sign-not-blocked": stages["sign"]["status"] = "passed"
    if change == "not-auth": stages["sign"]["code"] = "SIGNING_TIMEOUT"
    if change == "reversed": stages["phone"]["at"] = 12
    if change == "missing-time": stages["sign"].pop("at")
    if change == "missing-hash": runner.artifacts.pop("psbt_sha256")
    runner.defer_pending_phone_attempt(state)
    assert "approval_original_attempt" not in runner.artifacts
    runner.stage_sign()
    assert calls == []


@pytest.mark.parametrize("build", [36, 37, True])
def test_upgrade_requires_strictly_newer_integer_build(runner, build):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    runner.phone.build = build
    runner.stage_sign()
    assert calls == []
    assert "approval_upgrade_recovery" not in runner.artifacts


@pytest.mark.parametrize("field", ["account_identity", "desktop_identity", "request_id", "session_id", "intent", "psbt_sha256",
                                   "descriptor_id", "target_key_id", "target_fingerprint", "vault_id", "fee_sats"])
def test_original_approval_binding_cannot_change(runner, field):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    runner.artifacts[field] = "d" * 64 if field == "psbt_sha256" else "changed"
    with pytest.raises(Failed) as caught:
        runner.upgrade_approval()
    assert caught.value.code == "APPROVAL_CHANGED"
    assert calls == []


@pytest.mark.parametrize("status,count", [(1, 0), (2, 0), (4, 0), (5, 0), (6, 0), (3, 1), (3, None), (3, False), (True, 0)])
def test_upgrade_refuses_anything_except_exact_viewed_and_zero_submitted(runner, status, count):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    runner.native_call = lambda *a, **k: {"ready": False, "status": status, "submitted_signature_count": count}
    with pytest.raises(Blocked) as caught:
        runner.stage_sign()
    assert caught.value.code == "APPROVAL_REMOTE_STATE"
    assert calls == []


def test_signature_arriving_after_process_stop_prevents_another_review(runner):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    responses = iter([{"ready": False, "status": 3, "submitted_signature_count": 0},
                      {"ready": False, "status": 3, "submitted_signature_count": 0},
                      {"ready": True, "status": 6, "submitted_signature_count": 1, "signature_count_min": 2}])
    runner.native_call = lambda *a, **k: next(responses)
    runner.stage_sign()
    assert calls == ["restart"]
    assert runner.artifacts["signature_count_min"] == 2
    assert runner.deferred_phone_purpose == "sign:" + runner.request_id


def test_partial_process_boundary_resumes_same_receipt_without_second_generation(runner):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    def interrupted(journal, receipt):
        receipt["phase"] = "process_stopped"
        journal.artifact("approval_upgrade_recovery", receipt)
        raise Blocked("SYNTHETIC_INTERRUPTION", "Synthetic interruption")
    normal = runner.phone.restart_for_upgrade
    runner.phone.restart_for_upgrade = interrupted
    with pytest.raises(Blocked): runner.stage_sign()
    original = deepcopy(runner.artifacts["approval_upgrade_recovery"]["original_attempt"])
    runner.phone.restart_for_upgrade = normal
    runner.stage_sign()
    assert runner.artifacts["approval_upgrade_recovery"]["original_attempt"] == original
    assert calls == ["restart", "account-network", "cube-review", "approve"]


def test_existing_pin_continuation_never_navigates_or_approves_again(runner):
    state, calls = signing_recovery(runner, modern=True)
    runner.phone.build = 38
    runner.defer_pending_phone_attempt(state)
    def resume(*args, **kwargs):
        calls.append("resume-first-pin")
        assert args[:2] == (runner.request_id, runner.artifacts["session_id"])
        assert kwargs["before_pin"]() is True
    runner.phone.resume_first_pin = resume
    runner.stage_sign()
    assert calls == ["resume-first-pin"]
    assert runner.artifacts["approval_started"] is True


def test_first_approval_keeps_review_and_remote_callbacks_mandatory(runner):
    state, calls = signing_recovery(runner)
    runner.artifacts.pop("approval_started")
    def approve(*args, **kwargs):
        assert kwargs["reapproval"] is False
        assert kwargs["session_id"] == runner.artifacts["session_id"]
        assert kwargs["before_approve"]()
        assert kwargs["before_pin"]()
        calls.append("first-review")
    runner.phone.approve = approve
    runner.stage_sign()
    assert calls == ["first-review"]
    assert runner.artifacts["approval_binding"] == runner.approval_binding()


@pytest.mark.parametrize("field,value", [("binding", {}), ("request_id", "other"), ("session_id", "other"),
    ("intent_sha256", "bad"), ("fee_sats", 613), ("device", "other"), ("build", 37),
    ("process", {"pid": 0, "bundle_id": "io.coincube.keychainApp"}),
    ("process", {"pid": 123, "bundle_id": "other.app"})])
def test_modern_context_must_authenticate_every_recorded_identity(runner, field, value):
    state, calls = signing_recovery(runner, modern=True)
    runner.artifacts["approval_context"][field] = value
    with pytest.raises(Failed) as caught:
        runner.defer_pending_phone_attempt(state)
    assert caught.value.code == "APPROVAL_CHANGED"
    assert calls == []
    assert "approval_original_attempt" not in runner.artifacts


@pytest.mark.parametrize("field,value", [("identity", "foreign"), ("verified", False),
                                        ("pending_attempt", "sign:uncertain")])
def test_credential_changes_after_archive_never_allow_recovery(runner, field, value):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    state[field] = value
    runner.stage_sign()
    assert calls == []
    assert "approval_upgrade_recovery" not in runner.artifacts


@pytest.mark.parametrize("field,value", [("phase", "unknown"), ("current_build", 39), ("original_build", 36), ("binding", {}), ("original_attempt", {})])
def test_saved_recovery_is_not_replaceable(runner, field, value):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    original = runner.artifacts["approval_original_attempt"]
    receipt = {"original_attempt": deepcopy(original), "binding": runner.approval_binding(),
               "original_build": 37, "current_build": 38, "phase": "planned"}
    receipt[field] = value
    runner.journal.artifact("approval_upgrade_recovery", receipt)
    with pytest.raises(Failed): runner.stage_sign()
    assert calls == []
    assert runner.artifacts["approval_upgrade_recovery"] == receipt


def test_account_check_failure_after_restart_cannot_dispatch_approval(runner):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    def mismatch():
        raise Failed("PHONE_ACCOUNT", "Synthetic mismatch")
    runner.stage_phone_setup = mismatch
    with pytest.raises(Failed) as caught: runner.stage_sign()
    assert caught.value.code == "PHONE_ACCOUNT"
    assert calls == ["restart"]
    assert runner.artifacts["approval_started"] is True
    assert runner.artifacts["approval_upgrade_recovery"]["phase"] == "ready_for_review"


def test_status_callback_rejects_binding_change_before_native_call(runner):
    state, calls = signing_recovery(runner)
    runner.defer_pending_phone_attempt(state)
    runner.artifacts["cube_uuid"] = "other-cube"
    runner.native_call = lambda *a, **k: pytest.fail("Binding mismatch must precede remote action")
    with pytest.raises(Failed) as caught: runner.require_viewed_unsubmitted()
    assert caught.value.code == "APPROVAL_CHANGED"
    assert calls == []


def test_pending_key_capacity_refusal_is_read_only_and_preserves_attempt(runner):
    from harness.phone import Phone, BUNDLE
    state, purpose = pending_phone(runner)
    runner.defer_pending_phone_attempt(state)
    driver = SimpleNamespace(execute_script=lambda _: {"bundleId": BUNDLE},
        page_source='<root><node visible="true" label="Key limit reached"/></root>')
    runner.phone = Phone(driver, 38)
    runner.phone.device_digest = "synthetic-phone"
    runner.keys = lambda: []
    before = deepcopy(runner.journal.data)
    with pytest.raises(Blocked) as error:
        runner.stage_phone_key()
    assert error.value.code == "SIGNER_LIMIT"
    assert runner.journal.data == before
    assert state["pending_attempt"] == purpose
    assert runner.deferred_phone_purpose == purpose


def key_generation_case(runner, tmp_path, monkeypatch):
    from harness.state import atomic_write
    state, purpose = pending_phone(runner)
    state.pop("pin", None)  # Synthetic recovery never needs a credential value.
    runner.phone.build = 38
    runner.artifacts.update(cube_uuid="synthetic-cube-uuid", baseline_key_ids=["1"])
    runner.journal.data["stages"]["phone_key"] = {"status": "blocked", "code": "SIGNER_LIMIT", "at": 30}
    runner.account = {"tokens": {"access_token": "synthetic-token"}}
    runner.account_user_id = 42
    store = SimpleNamespace(sha256="synthetic-revision", load=lambda: deepcopy(state))
    runner.onboarding.store = store
    prior = tmp_path / "prior"
    prior.mkdir(mode=0o700)
    context = {"device": runner.phone.device_digest, "build": 38, "process": {"pid": 1}}
    checkpoint = {"version": 1, "binding": runner.journal.data["binding"], "artifacts": {
        "account_identity": runner.artifacts["account_identity"], "approval_context": context,
        "pin_resume_probe": {"status": "passed"}, "pin_dispatch_started": {"context": context},
        "signature_count_min": 2, "intent": {"txid": TXID}, "broadcast_txid": TXID,
        "target_key_id": "1", "target_fingerprint": "aabbccdd"},
        "stages": {name: {"status": "passed", "at": at} for at, name in enumerate(("collect", "broadcast", "verify"), 10)}}
    import uuid
    old_artifacts = checkpoint["artifacts"]
    old_run_id = str(uuid.uuid5(uuid.NAMESPACE_URL, "synthetic-prior-run"))
    old_artifacts.update(desktop_identity="synthetic-prior-desktop", request_id=str(uuid.uuid5(uuid.UUID(old_run_id), TXID)),
        session_id="synthetic-prior-session", psbt_sha256="d" * 64, descriptor_id="deadbeef", cube_id=10,
        cube_uuid="synthetic-prior-cube", vault_id=11, fee_sats=612)
    names = ("account_identity", "desktop_identity", "request_id", "session_id", "intent", "psbt_sha256",
             "descriptor_id", "target_key_id", "target_fingerprint", "cube_id", "cube_uuid", "vault_id", "fee_sats")
    old_binding = {**{name: deepcopy(old_artifacts[name]) for name in names}, "run_id": old_run_id,
                   "journal_binding": checkpoint["binding"], "device": runner.phone.device_digest}
    old_artifacts["approval_binding"] = old_binding
    context.update(binding=old_binding, request_id=old_artifacts["request_id"], session_id=old_artifacts["session_id"],
                   intent_sha256=provision.digest(old_artifacts["intent"]), fee_sats=612)
    atomic_write(prior / "checkpoint.json", json.dumps(checkpoint).encode())
    runner.key_generation_authorization = "synthetic-explicit-user-authorization"
    runner.key_recovery_prior_run = str(prior)
    current = [{"id": 1, "name": "prior-successful-signer", "network": "testnet4", "status": "active",
                "fingerprint": "aabbccdd", "keychainId": 8, "cubeId": 10}]
    plan = {"success": True, "data": {"plan": "pro", "status": "active", "renewalAt": None, "planProvenance": None, "entitlements": {"personalKeyLimit": 3}}}
    active = {"id": 8, "userId": 42, "network": "testnet4", "noOfKeys": 1, "keyLimit": 21}
    def http(url, **kwargs):
        assert kwargs["headers"]["Authorization"] == "Bearer synthetic-token"
        if url.endswith("/connect/plan"): return json.dumps(plan).encode()
        if url.endswith("/keychains/active"): return json.dumps({"data": active}).encode()
        raise AssertionError("Unexpected recovery API endpoint")
    monkeypatch.setattr(provision, "http", http)
    runner.keys = lambda: deepcopy(current)
    runner.phone.observe_key_limit = lambda: {"build": 38, "code": "SIGNER_LIMIT",
        "process": {"pid": 9, "bundle_id": "io.coincube.keychainApp"}, "observation": "visible_capacity_modal"}
    calls = []
    def restart(journal, receipt):
        calls.append("stop-old-process")
        receipt.update(phase="ready_for_reconciliation", current_process={"pid": 10, "bundle_id": "io.coincube.keychainApp"})
        journal.artifact("key_generation_recovery", deepcopy(receipt))
    def reserve(receipt):
        calls.append("reserve")
        binding = receipt["binding"]
        state["quarantined_attempts"] = [{"recovery_id": receipt["recovery_id"], "purpose": purpose,
            "disposition": "outcome_unresolved", "binding_sha256": provision.digest(binding),
            "authorization": binding["authorization"]}]
        state.pop("pending_attempt", None)
        state["key_generation_reservation"] = {"recovery_id": receipt["recovery_id"],
            "binding_sha256": provision.digest(binding), "purpose": binding["new_purpose"], "status": "reserved"}
        return "reserved"
    def create(label, journal, receipt, *, before_create, before_pin):
        calls.append("create-replacement")
        before_create()
        receipt.update(phase="creation_dispatched", creation_started=True)
        journal.artifact("key_generation_recovery", deepcopy(receipt))
        before_pin()
        receipt["pin_entered"] = True
        state["key_generation_reservation"]["status"] = "consumed"
        state["pending_attempt"] = receipt["binding"]["new_purpose"]
        journal.artifact("key_generation_recovery", deepcopy(receipt))
        current.append(dict(phone_key(runner), id=12, name=label, keychainId=8))
    def reconcile(purpose):
        assert state["pending_attempt"] == purpose
        state.pop("pending_attempt")
        state["key_generation_reservation"]["status"] = "completed"
        calls.append("reconciled")
    runner.phone.restart_for_key_generation = restart
    runner.phone.labels = lambda: set()
    runner.onboarding.reserve_key_generation = reserve
    runner.onboarding.create_recovery_key = create
    runner.onboarding.reconcile_pin = reconcile
    runner.stage_phone_setup = lambda: calls.append("account-network")
    return state, checkpoint, current, plan, active, calls


def test_authorized_key_generation_preserves_old_history_and_creates_distinct_key(runner, tmp_path, monkeypatch):
    state, _, _, _, _, calls = key_generation_case(runner, tmp_path, monkeypatch)
    original_stage = deepcopy(runner.journal.data["stages"]["phone_key"])
    runner.prepare_key_generation_recovery(state)
    receipt = runner.artifacts["key_generation_recovery"]
    binding = deepcopy(receipt["binding"])
    assert binding["original_stage"] == original_stage
    assert binding["new_key_label"] != binding["original_key_label"]
    assert binding["new_purpose"] != binding["original_purpose"]
    assert binding["original_credential_revision"] == "synthetic-revision"
    runner.defer_pending_phone_attempt(state)
    runner.journal.outcome("phone_key", "passed")
    runner.stage_phone_key()
    assert calls == ["stop-old-process", "account-network", "reserve", "create-replacement", "reconciled"]
    assert runner.artifacts["phone_key_creation_started"] is True
    assert runner.artifacts["phone_key_pin_entered"] is True
    assert runner.artifacts["key_generation_recovery"]["binding"] == binding
    assert runner.artifacts["key_generation_recovery"]["phase"] == "completed"
    assert state["quarantined_attempts"][0]["purpose"] == binding["original_purpose"]
    assert state["quarantined_attempts"][0]["disposition"] == "outcome_unresolved"
    assert state["key_generation_reservation"]["status"] == "completed"
    assert runner.phone_key_label() == binding["new_key_label"]


@pytest.mark.parametrize("part", ["config", "account", "device", "build", "probe", "dispatch", "quorum", "txid", "verify", "ordering", "too-recent", "context-request", "context-session", "context-intent", "context-binding"])
def test_key_recovery_refuses_unproven_prior_success(runner, tmp_path, monkeypatch, part):
    from harness.state import atomic_write
    state, prior, _, _, _, calls = key_generation_case(runner, tmp_path, monkeypatch)
    artifacts = prior["artifacts"]
    if part == "config": prior["binding"] = "other"
    if part == "account": artifacts["account_identity"] = "other"
    if part == "device": artifacts["approval_context"]["device"] = "other"
    if part == "build": artifacts["approval_context"]["build"] = 37
    if part == "probe": artifacts["pin_resume_probe"]["status"] = "failed"
    if part == "dispatch": artifacts["pin_dispatch_started"] = {}
    if part == "quorum": artifacts["signature_count_min"] = 1
    if part == "txid": artifacts["broadcast_txid"] = "c" * 64
    if part == "verify": prior["stages"]["verify"]["status"] = "blocked"
    if part == "ordering": prior["stages"]["verify"]["at"] = 1
    if part == "too-recent": prior["stages"]["verify"]["at"] = 30
    if part == "context-request": artifacts["approval_context"]["request_id"] = "other"
    if part == "context-session": artifacts["approval_context"]["session_id"] = "other"
    if part == "context-intent": artifacts["approval_context"]["intent_sha256"] = "other"
    if part == "context-binding": artifacts["approval_context"]["binding"]["cube_uuid"] = "other"
    atomic_write(tmp_path / "prior/checkpoint.json", json.dumps(prior).encode())
    with pytest.raises(Failed) as error: runner.prepare_key_generation_recovery(state)
    assert error.value.code == "KEY_RECOVERY_PRIOR"
    assert "key_generation_recovery" not in runner.artifacts
    assert calls == []


@pytest.mark.parametrize("part", ["pending", "credential", "verified", "creation", "pin", "baseline", "stage", "target", "cube"])
def test_key_recovery_requires_exact_failed_original_attempt(runner, tmp_path, monkeypatch, part):
    state, _, _, _, _, calls = key_generation_case(runner, tmp_path, monkeypatch)
    if part == "pending": state["pending_attempt"] = "key:foreign"
    if part == "credential": state["identity"] = "other"
    if part == "verified": state["verified"] = False
    if part == "creation": runner.artifacts.pop("phone_key_creation_started")
    if part == "pin": runner.artifacts.pop("phone_key_pin_entered")
    if part == "baseline": runner.artifacts.pop("baseline_key_ids")
    if part == "stage": runner.journal.data["stages"]["phone_key"]["code"] = "KEY_REGISTRATION"
    if part == "target": runner.artifacts["target_key_id"] = "12"
    if part == "cube": runner.artifacts.pop("cube_uuid")
    with pytest.raises(Failed) as error: runner.prepare_key_generation_recovery(state)
    assert error.value.code == "KEY_RECOVERY_EVIDENCE"
    assert calls == []


@pytest.mark.parametrize("part", ["limit", "inactive", "keychain-full", "foreign-owner", "foreign-chain", "prior-fingerprint", "old-appeared", "new-appeared", "renewal", "campaign", "missing-renewal", "not-pro"])
def test_key_generation_guard_checks_live_capacity_identity_and_name_absence(runner, tmp_path, monkeypatch, part):
    state, _, current, plan, active, calls = key_generation_case(runner, tmp_path, monkeypatch)
    runner.prepare_key_generation_recovery(state)
    receipt = runner.artifacts["key_generation_recovery"]
    if part == "limit": plan["data"]["entitlements"]["personalKeyLimit"] = 1
    if part == "inactive": plan["data"]["status"] = "inactive"
    if part == "renewal": plan["data"]["renewalAt"] = "2000-01-01T00:00:00Z"
    if part == "campaign": plan["data"]["planProvenance"] = "campaign"
    if part == "missing-renewal": plan["data"].pop("renewalAt")
    if part == "not-pro": plan["data"]["plan"] = "free"
    if part == "keychain-full": active["noOfKeys"] = active["keyLimit"]
    if part == "foreign-owner": active["userId"] = 43
    if part == "foreign-chain": active["network"] = "bitcoin"
    if part == "prior-fingerprint": current[0]["fingerprint"] = "deadbeef"
    if part == "old-appeared": current.append(dict(current[0], id=2, name=receipt["binding"]["original_key_label"]))
    if part == "new-appeared": current.append(dict(current[0], id=2, name=receipt["binding"]["new_key_label"]))
    with pytest.raises(Blocked): runner.recover_key_generation(receipt)
    assert calls == []
    assert state["pending_attempt"] == receipt["binding"]["original_purpose"]


def test_recovery_after_reservation_crash_keeps_global_credential_nonidle(runner, tmp_path, monkeypatch):
    state, _, _, _, _, calls = key_generation_case(runner, tmp_path, monkeypatch)
    runner.prepare_key_generation_recovery(state)
    receipt = runner.artifacts["key_generation_recovery"]
    runner.onboarding.reserve_key_generation(receipt)  # Simulate CAS completed before receipt-phase save.
    receipt["phase"] = "ready_for_reconciliation"
    runner.journal.artifact("key_generation_recovery", receipt)
    runner.key_generation_authorization = runner.key_recovery_prior_run = None
    runner.prepare_key_generation_recovery(deepcopy(state))
    runner.defer_pending_phone_attempt(deepcopy(state))
    assert runner.deferred_phone_purpose == receipt["binding"]["new_purpose"]
    assert state["key_generation_reservation"]["status"] == "reserved"
    assert "pending_attempt" not in state
    assert state["quarantined_attempts"][0]["disposition"] == "outcome_unresolved"


def test_pending_replacement_never_recreates_or_navigates(runner, tmp_path, monkeypatch):
    state, _, _, _, _, calls = key_generation_case(runner, tmp_path, monkeypatch)
    runner.prepare_key_generation_recovery(state)
    receipt = runner.artifacts["key_generation_recovery"]
    runner.onboarding.reserve_key_generation(receipt)
    receipt.update(phase="creation_dispatched", creation_started=True, pin_entered=True)
    state["key_generation_reservation"]["status"] = "consumed"
    state["pending_attempt"] = receipt["binding"]["new_purpose"]
    runner.defer_pending_phone_attempt(state)
    calls.clear()
    with pytest.raises(Blocked) as error: runner.stage_phone_key()
    assert error.value.code == "KEY_REGISTRATION"
    assert calls == []
    assert state["pending_attempt"] == receipt["binding"]["new_purpose"]


def test_old_generation_appearing_is_never_adopted(runner, tmp_path, monkeypatch):
    state, _, current, _, _, calls = key_generation_case(runner, tmp_path, monkeypatch)
    runner.prepare_key_generation_recovery(state)
    current.append(dict(phone_key(runner), name="QA " + runner.run_id))
    with pytest.raises(Blocked) as error: runner.stage_phone_key()
    assert error.value.code == "KEY_RECOVERY_OLD_APPEARED"
    assert calls == []
    assert "target_key_id" not in runner.artifacts


def test_ordinary_run_has_no_implicit_generation_authorization(runner):
    state, _ = pending_phone(runner)
    runner.prepare_key_generation_recovery(state)
    assert "key_generation_recovery" not in runner.artifacts


@pytest.mark.parametrize("authorization,prior", [("grant", None), (None, "/tmp/prior"), ("grant", "relative")])
def test_recovery_options_must_be_explicit_and_paired(config, tmp_path, authorization, prior):
    with pytest.raises(Failed) as error:
        provision.FreshRunner(config, tmp_path / "unused", authorize_key_generation=authorization, key_recovery_prior_run=prior)
    assert error.value.code == "KEY_RECOVERY_OPTIONS"
    assert not (tmp_path / "unused").exists()


def test_reserved_credential_without_pending_marker_blocks_other_run_before_ui(runner):
    state, _ = pending_phone(runner)
    state.pop("pending_attempt")
    state["key_generation_reservation"] = {"recovery_id": "foreign", "status": "reserved"}
    with pytest.raises(Blocked) as error: runner.defer_pending_phone_attempt(state)
    assert error.value.code == "KEY_RECOVERY_FOREIGN"
    assert "phone_validation_reused" not in runner.artifacts


@pytest.mark.parametrize("change", ["binding", "baseline", "authorization"])
def test_recovery_resume_rejects_metadata_drift_before_any_ui(runner, tmp_path, monkeypatch, change):
    state, _, _, _, _, calls = key_generation_case(runner, tmp_path, monkeypatch)
    runner.prepare_key_generation_recovery(state)
    if change == "binding": runner.artifacts["key_generation_recovery"]["binding"]["new_purpose"] = "changed"
    if change == "baseline": runner.artifacts["baseline_key_ids"] = []
    if change == "authorization": runner.key_generation_authorization = "another-grant"
    with pytest.raises(Failed) as error: runner.prepare_key_generation_recovery(state)
    assert error.value.code == "KEY_RECOVERY_CHANGED"
    assert calls == []


def dispatched_recovery_without_pin(runner, tmp_path, monkeypatch):
    state, _, keys, plan, _, calls = key_generation_case(runner, tmp_path, monkeypatch)
    runner.prepare_key_generation_recovery(state)
    receipt = runner.artifacts["key_generation_recovery"]
    runner.onboarding.reserve_key_generation(receipt)
    receipt.update(phase="creation_dispatched", creation_started=True, pin_entered=False,
                   current_process={"pid": 10, "bundle_id": "io.coincube.keychainApp"})
    runner.journal.artifact("key_generation_recovery", receipt)
    runner.defer_pending_phone_attempt(state)
    calls.clear()
    return state, receipt, keys, plan, calls


def test_dispatched_key_generation_resumes_first_pin_without_navigation_or_creation(runner, tmp_path, monkeypatch):
    state, receipt, keys, _, calls = dispatched_recovery_without_pin(runner, tmp_path, monkeypatch)
    def resume(journal, passed, *, before_pin):
        assert passed is receipt
        assert passed["phase"] == "creation_dispatched"
        assert state["key_generation_reservation"]["status"] == "reserved"
        before_pin()
        calls.append("first-pin-only")
        passed["pin_entered"] = True
        state["key_generation_reservation"]["status"] = "consumed"
        state["pending_attempt"] = passed["binding"]["new_purpose"]
        journal.artifact("key_generation_recovery", deepcopy(passed))
        keys.append(dict(phone_key(runner), id=12, name=passed["binding"]["new_key_label"], keychainId=8))
    runner.onboarding.resume_recovery_key_pin = resume
    runner.stage_phone_key()
    assert calls == ["first-pin-only", "reconciled", "account-network"]
    assert runner.artifacts["key_generation_recovery"]["phase"] == "completed"
    assert state["key_generation_reservation"]["status"] == "completed"
    assert state["quarantined_attempts"][0]["disposition"] == "outcome_unresolved"


def test_uncertain_resumed_first_pin_is_never_dispatched_twice(runner, tmp_path, monkeypatch):
    state, receipt, _, _, calls = dispatched_recovery_without_pin(runner, tmp_path, monkeypatch)
    def interrupted(journal, passed, *, before_pin):
        before_pin()
        calls.append("first-pin-only")
        passed["pin_entered"] = True
        state["key_generation_reservation"]["status"] = "consumed"
        state["pending_attempt"] = passed["binding"]["new_purpose"]
        journal.artifact("key_generation_recovery", deepcopy(passed))
        raise Blocked("PHONE_INPUT_DISPATCH", "Synthetic lost response")
    runner.onboarding.resume_recovery_key_pin = interrupted
    with pytest.raises(Blocked) as error: runner.stage_phone_key()
    assert error.value.code == "PHONE_INPUT_DISPATCH"
    with pytest.raises(Blocked) as error: runner.stage_phone_key()
    assert error.value.code == "KEY_REGISTRATION"
    assert calls == ["first-pin-only"]
    assert receipt["creation_started"] is True
    assert receipt["pin_entered"] is True
    assert state["key_generation_reservation"]["status"] == "consumed"


def test_resume_first_key_pin_rechecks_capacity_before_touching_prompt(runner, tmp_path, monkeypatch):
    _, _, _, plan, calls = dispatched_recovery_without_pin(runner, tmp_path, monkeypatch)
    plan["data"]["entitlements"]["personalKeyLimit"] = 1
    runner.onboarding.resume_recovery_key_pin = lambda *a, **k: pytest.fail("No prompt interaction without capacity")
    with pytest.raises(Blocked) as error: runner.stage_phone_key()
    assert error.value.code == "KEY_RECOVERY_CAPACITY"
    assert calls == []


@pytest.mark.parametrize("refusal", ["KEY_GENERATION_PROCESS", "KEY_GENERATION_UNCERTAIN", "KEY_GENERATION_INPUT"])
def test_first_pin_helper_refusal_never_falls_back_to_restart_or_creation(runner, tmp_path, monkeypatch, refusal):
    _, receipt, _, _, calls = dispatched_recovery_without_pin(runner, tmp_path, monkeypatch)
    def refuse(*args, **kwargs):
        raise Blocked(refusal, "Synthetic process, reservation or field refusal")
    runner.onboarding.resume_recovery_key_pin = refuse
    with pytest.raises(Blocked) as error: runner.stage_phone_key()
    assert error.value.code == refusal
    assert calls == []
    assert receipt["pin_entered"] is False


def discovery_case(runner):
    state, calls = signing_recovery(runner)
    runner.artifacts.pop('approval_started')
    runner.artifacts['phone']['build'] = runner.phone.build
    runner.pin_artifact('approval_binding', runner.approval_binding())
    runner.journal.data['stages']['sign'] = {'status': 'blocked', 'code': 'REQUEST_DELIVERY', 'at': 20}
    runner.native_call = lambda *a, **k: {'ready': False, 'status': 1, 'submitted_signature_count': 0}
    def restart(journal, receipt, *, before_restart):
        if not before_restart():
            return False
        calls.append('discovery-restart')
        receipt.update(phase='completed', stopped_verified=True,
                       before_process={'pid': 10, 'bundle_id': 'io.coincube.keychainApp'},
                       after_process={'pid': 11, 'bundle_id': 'io.coincube.keychainApp'})
        journal.artifact('request_discovery_restart', deepcopy(receipt))
        return True
    runner.phone.restart_for_request_discovery = restart
    runner.approve_phone = lambda: calls.append('ordinary-review')
    return state, calls


def test_prior_delivery_timeout_is_archived_and_restart_occurs_once(runner):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    evidence = deepcopy(runner.artifacts['request_discovery_timeout'])
    runner.journal.outcome('sign', 'passed')
    runner.stage_sign()
    assert calls == ['discovery-restart', 'account-network', 'cube-review', 'ordinary-review']
    assert runner.artifacts['request_discovery_timeout'] == evidence
    assert 'approval_started' not in runner.artifacts
    calls.clear()
    runner.stage_sign()
    assert calls == ['ordinary-review']
    assert runner.artifacts['request_discovery_timeout'] == evidence


def test_first_delivery_timeout_gets_only_one_bounded_restart(runner):
    _, calls = discovery_case(runner)
    def absent():
        calls.append('ordinary-review')
        raise Blocked('REQUEST_DELIVERY', 'Synthetic request absence')
    runner.approve_phone = absent
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DELIVERY'
    assert calls == ['ordinary-review', 'discovery-restart', 'account-network', 'cube-review', 'ordinary-review']
    calls.clear()
    with pytest.raises(Blocked): runner.stage_sign()
    assert calls == ['ordinary-review']


@pytest.mark.parametrize('code', ['PHONE_SELECTOR', 'PHONE_AUTH', 'PHONE_REVIEW', 'SIGNER_PIN'])
def test_other_approval_failures_do_not_restart_discovery(runner, code):
    _, calls = discovery_case(runner)
    def failed(): raise Blocked(code, 'Synthetic failure')
    runner.approve_phone = failed
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == code
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts


@pytest.mark.parametrize('marker', ['approval_started', 'approval_context', 'pin_resume_probe', 'pin_dispatch_started',
                                    'pending_attempt', 'key_generation_reservation', 'unverified', 'foreign-credential'])
def test_discovery_restart_refuses_any_input_or_active_credential_evidence(runner, marker):
    state, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    if marker == 'pending_attempt': state[marker] = 'sign:' + runner.request_id
    elif marker == 'key_generation_reservation': state[marker] = {'status': 'reserved'}
    elif marker == 'unverified': state['verified'] = False
    elif marker == 'foreign-credential': state['identity'] = 'other'
    else: runner.artifacts[marker] = False  # Presence alone is enough to refuse.
    with pytest.raises(Blocked) as error: runner.restart_request_discovery()
    assert error.value.code == 'REQUEST_DISCOVERY_INPUT'
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts


@pytest.mark.parametrize('status,count', [(2, 0), (3, 0), (4, 0), (6, 0), (1, 1), (1, None), (True, 0), (1, False)])
def test_discovery_restart_requires_fresh_pending_zero_submission(runner, status, count):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.native_call = lambda *a, **k: {'ready': False, 'status': status, 'submitted_signature_count': count}
    with pytest.raises(Blocked) as error: runner.restart_request_discovery()
    assert error.value.code == 'REQUEST_DISCOVERY_ADVANCED'
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts


def test_signature_arriving_at_discovery_boundary_prevents_restart(runner):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.native_call = lambda *a, **k: {'ready': True, 'status': 6, 'submitted_signature_count': 1, 'signature_count_min': 2}
    assert runner.restart_request_discovery() is False
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts
    assert runner.artifacts['signature_count_min'] == 2


def test_uncertain_discovery_receipt_blocks_all_navigation_and_cannot_repeat(runner):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    failure = runner.artifacts['request_discovery_timeout']
    runner.journal.artifact('request_discovery_restart', {'binding': runner.discovery_binding(),
        'timeout': deepcopy(failure), 'timeout_sha256': provision.digest(failure), 'phase': 'attempted'})
    with pytest.raises(Blocked) as error: runner.validate_discovery_restart()
    assert error.value.code == 'REQUEST_DISCOVERY_UNCERTAIN'
    with pytest.raises(Blocked): runner.stage_sign()
    assert calls == []


def test_completed_discovery_receipt_allows_later_signature_reconciliation(runner):
    state, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.restart_request_discovery()
    calls.clear()
    runner.artifacts['approval_started'] = True
    runner.artifacts['pin_dispatch_started'] = {'context': 'synthetic'}
    state['pending_attempt'] = 'sign:' + runner.request_id
    runner.validate_discovery_restart()  # Validation is not an idle-credential reset.
    runner.native_call = lambda *a, **k: {'ready': True, 'status': 6, 'submitted_signature_count': 1, 'signature_count_min': 2}
    runner.stage_sign()
    assert calls == []
    assert runner.artifacts['approval_started'] is True


@pytest.mark.parametrize('field', ['session_id', 'request_id', 'psbt_sha256', 'descriptor_id', 'target_key_id',
                                   'vault_id', 'account_identity', 'desktop_identity', 'build'])
def test_discovery_receipt_cannot_move_to_changed_request_or_device_build(runner, field):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.restart_request_discovery()
    calls.clear()
    if field == 'build': runner.phone.build += 1
    else: runner.artifacts[field] = 'f' * 64
    with pytest.raises(Failed): runner.validate_discovery_restart()
    assert calls == []


def auth_discovery_case(runner, tmp_path, monkeypatch):
    from harness.state import atomic_write
    state, calls = discovery_case(runner)
    runner.config['phone_state_dir'] = str(tmp_path / 'private-phone-state')
    account = {'email': runner.config['account_email'], 'device_id': 'synthetic-desktop'}
    runner.artifacts['account_identity'] = provision.digest({'email': account['email'].casefold(), 'user_id': 42})
    runner.artifacts['desktop_identity'] = provision.digest(account['device_id'])
    runner.onboarding.identity = runner.artifacts['account_identity'] + ':' + runner.phone.device_digest
    state['identity'] = runner.onboarding.identity
    runner.artifacts['approval_binding'] = runner.approval_binding()
    runner.profile.mkdir(mode=0o700)
    (runner.profile / 'testnet4').mkdir(mode=0o700)
    atomic_write(runner.profile / 'testnet4/connect.json', json.dumps({'accounts': [account], 'active_email': account['email']}).encode())
    runner.provision = SimpleNamespace(binary='/synthetic/native', call=lambda *a, **k: dict(account, user_id=42))
    monkeypatch.setattr(provision.SecretStore, 'load', lambda _: deepcopy(state))
    return state, calls


def test_authentication_blocks_uncertain_restart_before_any_phone_navigation(runner, tmp_path, monkeypatch):
    _, calls = auth_discovery_case(runner, tmp_path, monkeypatch)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    failure = runner.artifacts['request_discovery_timeout']
    runner.journal.artifact('request_discovery_restart', {'binding': runner.discovery_binding(),
        'timeout': deepcopy(failure), 'timeout_sha256': provision.digest(failure), 'phase': 'attempted'})
    runner.phone.labels = lambda: pytest.fail('Uncertain restart must block before reading or navigating UI')
    with pytest.raises(Blocked) as error: runner.stage_authentication()
    assert error.value.code == 'REQUEST_DISCOVERY_UNCERTAIN'
    assert calls == []


def test_authentication_archives_original_timeout_before_any_phone_navigation(runner, tmp_path, monkeypatch):
    _, calls = auth_discovery_case(runner, tmp_path, monkeypatch)
    before = deepcopy(runner.journal.data['stages']['sign'])
    def labels():
        assert runner.artifacts['request_discovery_timeout']['outcome'] == before
        return {'Home', 'Cubes', 'Settings', runner.config['account_email']}
    runner.phone.labels = labels
    runner.phone.click = calls.append
    runner.stage_authentication()
    assert calls == ['Settings', 'Home']
    runner.journal.outcome('sign', 'blocked', 'OTHER')
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    assert runner.artifacts['request_discovery_timeout']['outcome'] == before


@pytest.mark.parametrize('status', [2, 3])
def test_archived_delivery_timeout_advances_to_ordinary_review_without_restart(runner, status):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    original = deepcopy(runner.artifacts['request_discovery_timeout'])
    runner.native_call = lambda *a, **k: {'ready': False, 'status': status, 'submitted_signature_count': 0}
    runner.stage_sign()
    assert calls == ['ordinary-review']
    assert 'request_discovery_restart' not in runner.artifacts
    assert runner.artifacts['request_discovery_timeout'] == original
    assert 'approval_started' not in runner.artifacts


@pytest.mark.parametrize('status', [2, 3])
def test_delivery_advancement_at_restart_guard_uses_ordinary_review_without_terminate(runner, status):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    replies = iter([{'ready': False, 'status': 1, 'submitted_signature_count': 0},
                    {'ready': False, 'status': 1, 'submitted_signature_count': 0},
                    {'ready': False, 'status': status, 'submitted_signature_count': 0}])
    runner.native_call = lambda *a, **k: next(replies)
    runner.stage_sign()
    assert calls == ['ordinary-review']
    assert 'request_discovery_restart' not in runner.artifacts


@pytest.mark.parametrize('status,count', [(0, 0), (4, 0), (5, 0), (6, 0), (7, 0), (99, 0), (2, 1), (3, 1), (3, None), (2, False)])
def test_archived_timeout_never_reviews_approved_terminal_unknown_or_submitted_state(runner, status, count):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.native_call = lambda *a, **k: {'ready': False, 'status': status, 'submitted_signature_count': count}
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DISCOVERY_ADVANCED'
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts


def test_completed_discovery_receipt_also_uses_delivered_queue_without_second_restart(runner):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.restart_request_discovery()
    receipt = deepcopy(runner.artifacts['request_discovery_restart'])
    calls.clear()
    runner.native_call = lambda *a, **k: {'ready': False, 'status': 2, 'submitted_signature_count': 0}
    runner.stage_sign()
    assert calls == ['ordinary-review']
    assert runner.artifacts['request_discovery_restart'] == receipt


def test_advanced_session_review_timeout_stops_without_a_second_discovery_pass(runner):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.native_call = lambda *a, **k: {'ready': False, 'status': 2, 'submitted_signature_count': 0}
    def absent():
        calls.append('ordinary-review')
        raise Blocked('REQUEST_DELIVERY', 'Synthetic queue absence')
    runner.approve_phone = absent
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DELIVERY'
    assert calls == ['ordinary-review']
    assert 'request_discovery_restart' not in runner.artifacts


@pytest.mark.parametrize('after_remote', [False, True])
@pytest.mark.parametrize('status', [1, 2, 3])
def test_known_incoming_refusal_routes_fresh_guarded_ordinary_review(runner, status, after_remote):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    result = {'ready': False, 'status': 1, 'submitted_signature_count': 0}
    runner.native_call = lambda *a, **k: dict(result)
    def incoming(journal, receipt, *, before_restart):
        if after_remote: assert before_restart()
        result['status'] = status
        raise Blocked('REQUEST_DISCOVERY_INCOMING', 'Synthetic exact notification')
    runner.phone.restart_for_request_discovery = incoming
    runner.stage_sign()
    assert calls == ['ordinary-review']
    assert 'request_discovery_restart' not in runner.artifacts
    assert 'approval_started' not in runner.artifacts


@pytest.mark.parametrize('mutation', ['approval_started', 'approval_context', 'pin_resume_probe', 'pin_dispatch_started',
                                     'pending_attempt', 'reservation', 'unverified', 'foreign-credential'])
def test_known_incoming_cannot_bypass_new_input_or_credential_markers(runner, mutation):
    state, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    def incoming(journal, receipt, *, before_restart):
        if mutation == 'pending_attempt': state[mutation] = 'sign:' + runner.request_id
        elif mutation == 'reservation': state['key_generation_reservation'] = {'status': 'reserved'}
        elif mutation == 'unverified': state['verified'] = False
        elif mutation == 'foreign-credential': state['identity'] = 'other'
        else: runner.artifacts[mutation] = True
        raise Blocked('REQUEST_DISCOVERY_INCOMING', 'Synthetic exact notification')
    runner.phone.restart_for_request_discovery = incoming
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DISCOVERY_INPUT'
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts


@pytest.mark.parametrize('status,count', [(4, 0), (6, 0), (99, 0), (1, 1), (3, None)])
def test_known_incoming_cannot_bypass_fresh_session_state_guard(runner, status, count):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    def incoming(journal, receipt, *, before_restart):
        runner.native_call = lambda *a, **k: {'ready': False, 'status': status, 'submitted_signature_count': count}
        raise Blocked('REQUEST_DISCOVERY_INCOMING', 'Synthetic exact notification')
    runner.phone.restart_for_request_discovery = incoming
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DISCOVERY_ADVANCED'
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts


def test_unknown_home_refusal_is_not_rerouted_to_review(runner):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    def unknown(*a, **k): raise Blocked('REQUEST_DISCOVERY_HOME', 'Synthetic unknown modal')
    runner.phone.restart_for_request_discovery = unknown
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DISCOVERY_HOME'
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts


def test_signature_arrival_after_known_incoming_moves_to_collection(runner):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    def incoming(*a, **k):
        runner.native_call = lambda *a, **k: {'ready': True, 'status': 6, 'submitted_signature_count': 1, 'signature_count_min': 2}
        raise Blocked('REQUEST_DISCOVERY_INCOMING', 'Synthetic exact notification')
    runner.phone.restart_for_request_discovery = incoming
    runner.stage_sign()
    assert calls == []
    assert runner.artifacts['signature_count_min'] == 2
    assert 'request_discovery_restart' not in runner.artifacts


def changed_discovery_build(runner):
    state, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    original = deepcopy(runner.artifacts['request_discovery_timeout'])
    runner.phone.build = 39
    runner.prepare_discovery_no_restart()
    return state, calls, original


def test_build_changed_timeout_authentication_pins_disable_before_phone_observation(runner, tmp_path, monkeypatch):
    _, calls = auth_discovery_case(runner, tmp_path, monkeypatch)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    original = deepcopy(runner.artifacts['request_discovery_timeout'])
    encoded = json.dumps(original, separators=(',', ':'))
    runner.phone.build = 39
    def labels():
        assert runner.artifacts['request_discovery_no_restart']['original_build'] == 38
        assert runner.artifacts['request_discovery_no_restart']['observed_build'] == 39
        return {'Home', 'Cubes', 'Settings', runner.config['account_email']}
    runner.phone.labels = labels
    runner.phone.click = calls.append
    runner.stage_authentication()
    decision = deepcopy(runner.artifacts['request_discovery_no_restart'])
    assert calls == ['Settings', 'Home']
    runner.artifacts['phone']['build'] = 39  # Normal stage_phone's next observation.
    runner.phone_journal.close()
    calls.clear()
    runner.stage_authentication()
    assert calls == ['Settings', 'Home']
    assert runner.artifacts['request_discovery_no_restart'] == decision
    assert json.dumps(runner.artifacts['request_discovery_timeout'], separators=(',', ':')) == encoded


def test_unarchived_original_timeout_can_disable_restart_before_build_overwrite(runner):
    _, _ = discovery_case(runner)
    old = deepcopy(runner.journal.data['stages']['sign'])
    runner.phone.build = 39
    runner.archive_discovery_timeout(old)
    runner.prepare_discovery_no_restart()
    assert runner.artifacts['request_discovery_timeout']['outcome'] == old
    assert runner.artifacts['request_discovery_no_restart']['original_build'] == 38


@pytest.mark.parametrize('status', [1, 2, 3])
def test_build_change_uses_fresh_remote_state_and_ordinary_review_without_restart(runner, status):
    _, calls, original = changed_discovery_build(runner)
    runner.artifacts['phone']['build'] = 39
    runner.native_call = lambda *a, **k: {'ready': False, 'status': status, 'submitted_signature_count': 0}
    runner.stage_sign()
    assert calls == ['ordinary-review']
    assert runner.artifacts['request_discovery_timeout'] == original
    assert 'request_discovery_restart' not in runner.artifacts


@pytest.mark.parametrize('build', [38, 39, 40])
def test_no_restart_decision_survives_timeout_again_rollback_and_further_upgrade(runner, build):
    _, calls, original = changed_discovery_build(runner)
    decision = deepcopy(runner.artifacts['request_discovery_no_restart'])
    runner.phone.build = build
    runner.artifacts['phone']['build'] = build
    runner.journal.outcome('sign', 'blocked', 'REQUEST_DELIVERY')
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.prepare_discovery_no_restart()
    def absent():
        calls.append('ordinary-review')
        raise Blocked('REQUEST_DELIVERY', 'Synthetic timeout again')
    runner.approve_phone = absent
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DELIVERY'
    with pytest.raises(Blocked) as error: runner.restart_request_discovery()
    assert error.value.code == 'REQUEST_DISCOVERY_DISABLED'
    assert calls == ['ordinary-review']
    assert runner.artifacts['request_discovery_no_restart'] == decision
    assert runner.artifacts['request_discovery_timeout'] == original
    assert 'request_discovery_restart' not in runner.artifacts


@pytest.mark.parametrize('phase', [None, {}, {'phase': 'attempted'}, {'phase': 'completed'}])
def test_any_restart_receipt_presence_excludes_build_change_decision(runner, phase):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.phone.build = 39
    runner.journal.artifact('request_discovery_restart', phase)
    with pytest.raises(Blocked): runner.prepare_discovery_no_restart()
    assert 'request_discovery_no_restart' not in runner.artifacts
    assert calls == []


@pytest.mark.parametrize('marker', ['approval_started', 'approval_context', 'pin_resume_probe', 'pin_dispatch_started',
                                   'pending_attempt', 'key_generation_reservation', 'foreign', 'unverified'])
@pytest.mark.parametrize('late', [False, True])
def test_no_restart_decision_never_bypasses_input_or_credential_guard(runner, marker, late):
    state, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.phone.build = 39
    if late: runner.prepare_discovery_no_restart()
    if marker == 'pending_attempt': state[marker] = 'sign:' + runner.request_id
    elif marker == 'key_generation_reservation': state[marker] = {'status': 'reserved'}
    elif marker == 'foreign': state['identity'] = 'other'
    elif marker == 'unverified': state['verified'] = False
    else: runner.artifacts[marker] = False
    operation_under_test = runner.continue_request_discovery if late else runner.prepare_discovery_no_restart
    with pytest.raises(Blocked) as error:
        operation_under_test()
    assert error.value.code == 'REQUEST_DISCOVERY_INPUT'
    assert calls == []
    assert 'request_discovery_restart' not in runner.artifacts
    assert ('request_discovery_no_restart' in runner.artifacts) is late


@pytest.mark.parametrize('field', ['session_id', 'target_key_id', 'account_identity', 'desktop_identity', 'psbt_sha256', 'descriptor_id'])
def test_disabled_restart_history_cannot_move_to_foreign_binding(runner, field):
    _, calls, _ = changed_discovery_build(runner)
    runner.artifacts[field] = 'f' * 64
    with pytest.raises(Failed): runner.prepare_discovery_no_restart()
    assert calls == []


@pytest.mark.parametrize('status,count', [(4, 0), (6, 0), (1, 1), (2, None), (True, 0), (3, False)])
def test_disabled_restart_still_refuses_unknown_or_submitted_remote_state(runner, status, count):
    _, calls, _ = changed_discovery_build(runner)
    runner.native_call = lambda *a, **k: {'ready': False, 'status': status, 'submitted_signature_count': count}
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DISCOVERY_ADVANCED'
    assert calls == []


def test_disabled_restart_allows_verified_signature_reconciliation_after_legitimate_input(runner):
    state, calls, original = changed_discovery_build(runner)
    runner.artifacts['approval_started'] = True
    runner.artifacts['pin_dispatch_started'] = {'context': 'saved'}
    state['pending_attempt'] = 'sign:' + runner.request_id
    runner.prepare_discovery_no_restart()  # Immutable decision validation does not reset input markers.
    runner.native_call = lambda *a, **k: {'ready': True, 'signature_count_min': 2}
    runner.stage_sign()
    assert calls == []
    assert state['pending_attempt'] == 'sign:' + runner.request_id
    assert runner.artifacts['request_discovery_timeout'] == original
    assert runner.artifacts['signature_count_min'] == 2


def test_no_restart_review_rechecks_input_after_remote_guard(runner):
    state, calls, _ = changed_discovery_build(runner)
    count = 0
    def collect(*a, **k):
        nonlocal count
        count += 1
        if count == 2:
            state['pending_attempt'] = 'another-attempt'
        return {'ready': False, 'status': 2, 'submitted_signature_count': 0}
    runner.native_call = collect
    with pytest.raises(Blocked) as error: runner.stage_sign()
    assert error.value.code == 'REQUEST_DISCOVERY_INPUT'
    assert calls == []


@pytest.mark.parametrize('marker', ['approval_context', 'pin_resume_probe', 'pending_attempt'])
def test_disabled_restart_authentication_refuses_late_input_before_settings(runner, tmp_path, monkeypatch, marker):
    state, calls = auth_discovery_case(runner, tmp_path, monkeypatch)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    runner.phone.build = 39
    runner.prepare_discovery_no_restart()
    if marker == 'pending_attempt': state[marker] = 'foreign-attempt'
    else: runner.artifacts[marker] = False
    runner.phone.labels = lambda: pytest.fail('Late input must not navigate Settings')
    with pytest.raises(Blocked) as error: runner.stage_authentication()
    assert error.value.code == 'REQUEST_DISCOVERY_INPUT'
    assert calls == []


def back_receipt(runner, phase='attempted'):
    failure = runner.artifacts['request_discovery_timeout']
    return {'binding': deepcopy(failure['binding']), 'timeout_sha256': provision.digest(failure),
            'process': {'pid': 10, 'bundle_id': 'io.coincube.keychainApp'}, 'phase': phase}


@pytest.mark.parametrize('value', [None, {}, 'attempted', 'completed'])
@pytest.mark.parametrize('changed_build', [False, True])
def test_uncertain_or_malformed_back_blocks_authentication_before_any_navigation(runner, tmp_path, monkeypatch, value, changed_build):
    _, calls = auth_discovery_case(runner, tmp_path, monkeypatch)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    saved = back_receipt(runner, value) if isinstance(value, str) else value
    if value == 'completed': saved['process']['pid'] = 0
    runner.journal.artifact('request_discovery_back', saved)
    if changed_build: runner.phone.build = 39
    runner.phone.labels = lambda: pytest.fail('No UI reading or setup navigation after uncertain Back')
    with pytest.raises(Blocked): runner.stage_authentication()
    assert calls == []
    assert 'request_discovery_no_restart' not in runner.artifacts
    assert 'request_discovery_restart' not in runner.artifacts


@pytest.mark.parametrize('field', ['timeout_sha256', 'process', 'binding'])
def test_completed_back_rejects_corrupted_immutable_evidence(runner, field):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    saved = back_receipt(runner, 'completed')
    saved[field] = 'changed'
    runner.journal.artifact('request_discovery_back', saved)
    with pytest.raises(Failed): runner.validate_discovery_back()
    assert calls == []


def test_completed_back_does_not_reenable_permanent_no_restart(runner):
    _, calls, _ = changed_discovery_build(runner)
    runner.journal.artifact('request_discovery_back', back_receipt(runner, 'completed'))
    runner.validate_discovery_back()
    with pytest.raises(Blocked) as error: runner.restart_request_discovery()
    assert error.value.code == 'REQUEST_DISCOVERY_DISABLED'
    assert calls == []


@pytest.mark.parametrize('phase', ['attempted', 'completed'])
def test_post_back_incoming_race_cannot_excuse_uncertain_navigation(runner, phase):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    def incoming(journal, receipt, *, before_restart):
        assert before_restart()
        journal.artifact('request_discovery_back', back_receipt(runner, phase))
        raise Blocked('REQUEST_DISCOVERY_INCOMING', 'Synthetic incoming after Back')
    runner.phone.restart_for_request_discovery = incoming
    if phase == 'attempted':
        with pytest.raises(Blocked) as error: runner.stage_sign()
        assert error.value.code == 'REQUEST_DISCOVERY_BACK_UNCERTAIN'
    else:
        runner.stage_sign()
    assert calls == (['ordinary-review'] if phase == 'completed' else [])
    assert 'request_discovery_restart' not in runner.artifacts


def test_exact_request_arrival_on_queue_routes_to_fresh_ordinary_review(runner):
    _, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    def arrived(*a, **k):
        raise Blocked('REQUEST_DISCOVERY_READY', 'Exact request is now visible')
    runner.phone.restart_for_request_discovery = arrived
    runner.stage_sign()
    assert calls == ['ordinary-review']
    assert 'request_discovery_back' not in runner.artifacts
    assert 'request_discovery_restart' not in runner.artifacts


def test_pending_discovery_guard_rechecks_credential_after_native_call(runner):
    state, calls = discovery_case(runner)
    runner.archive_discovery_timeout(runner.journal.data['stages']['sign'])
    def collect(*a, **k):
        state['pending_attempt'] = 'foreign'
        return {'ready': False, 'status': 1, 'submitted_signature_count': 0}
    runner.native_call = collect
    with pytest.raises(Blocked) as error: runner.require_pending_discovery()
    assert error.value.code == 'REQUEST_DISCOVERY_INPUT'
    assert calls == []
