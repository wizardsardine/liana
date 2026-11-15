"""Offline abort boundaries; no real account, PIN, network, or phone access."""
from copy import deepcopy
import json
from types import SimpleNamespace
import uuid
import pytest
from harness import sign_abort as abort
from harness.state import Failed, atomic_write, digest, private_read


class Store:
    def __init__(self, value):
        self.value = deepcopy(value)
        self.sha256 = digest(value)
        self.writes = 0
        self.race = False

    def load(self):
        self.sha256 = digest(self.value)
        return deepcopy(self.value)

    def save(self, value):
        if self.race:
            self.value['pending_attempt'] = 'sign:newer'
        if digest(self.value) != self.sha256:
            raise Failed('native_cas_conflict', 'CAS conflict')
        self.writes += 1
        self.value = deepcopy(value)
        self.load()


def material():
    binding = dict(request_id='request', session_id='session', descriptor_id='descriptor',
                   target_key_id='key', target_fingerprint='abcd1234', desktop_identity=digest('creator'),
                   intent={'txid': 'a' * 64}, fee_sats=612)
    store = Store({'identity': 'account:device', 'verified': True, 'pin': 'synthetic',
                   'pending_attempt': 'sign:request', 'quarantined_attempts': [{'old': 'keygen'}]})
    intent = dict(binding=binding, owner_user_id=7, credential_identity='account:device',
                  credential_revision=store.sha256, purpose='sign:request', authorization_ref='test instruction')
    terminal = dict(ok=True, action='cancel', network='testnet4', cancelled=True, status=8,
                    submitted_signature_count=0, ready=False, signature_count_min=1, owner_user_id=7,
                    txid='a' * 64, fee_sats=612, creator_device_id='creator', target_device_id='phone',
                    **{k: binding[k] for k in ('request_id', 'session_id', 'descriptor_id', 'target_key_id', 'target_fingerprint')})
    receipt = dict(phase='terminal_verified', intent_sha256=digest(intent), terminal=terminal)
    return store, intent, receipt


def test_quarantine_preserves_unknown_pin_outcome_and_all_other_credentials():
    store, intent, receipt = material()
    before = deepcopy(store.value)
    assert abort.quarantine(store, intent, receipt) == 'quarantined'
    audit = store.value.pop('cancelled_signing_attempts')
    del before['pending_attempt']
    assert store.value == before
    assert audit[0]['local_pin_outcome'] == 'unresolved'
    assert audit[0]['terminal_receipt_sha256'] == digest(receipt)


def test_replay_leaves_newer_attempt_untouched():
    store, intent, receipt = material()
    abort.quarantine(store, intent, receipt)
    store.value['pending_attempt'] = 'sign:newer'
    before = deepcopy(store.value)
    assert abort.quarantine(store, intent, receipt) == 'already_quarantined'
    assert store.value == before
    assert store.writes == 1


@pytest.mark.parametrize('change', ['revision', 'pending', 'identity', 'verified', 'reservation', 'race'])
def test_changed_credentials_never_cleared(change):
    store, intent, receipt = material()
    if change == 'revision': store.value['new'] = 1
    if change == 'pending': store.value['pending_attempt'] = 'sign:newer'
    if change == 'identity': store.value['identity'] = 'other'
    if change == 'verified': store.value['verified'] = False
    if change == 'reservation': store.value['key_generation_reservation'] = {'status': 'pending'}
    if change == 'race': store.race = True
    with pytest.raises(Failed): abort.quarantine(store, intent, receipt)
    assert store.writes == 0
    assert store.value['pending_attempt'].startswith('sign:')


@pytest.mark.parametrize('field,value', [('status', 3), ('status', 7), ('status', 9), ('status', 10),
    ('status', 0), ('submitted_signature_count', 1), ('ready', True), ('request_id', 'other'),
    ('session_id', 'other'), ('owner_user_id', 8), ('creator_device_id', 'other'), ('target_key_id', 'other'),
    ('descriptor_id', 'other'), ('txid', 'b'*64), ('network', 'bitcoin')])
def test_wrong_or_raced_terminal_receipt_never_releases(field, value):
    store, intent, receipt = material()
    receipt['terminal'][field] = value
    with pytest.raises(Failed): abort.quarantine(store, intent, receipt)
    assert store.writes == 0


def test_replay_cannot_clear_reintroduced_old_attempt():
    store, intent, receipt = material()
    abort.quarantine(store, intent, receipt)
    store.value['pending_attempt'] = intent['purpose']
    with pytest.raises(Failed): abort.quarantine(store, intent, receipt)
    assert store.writes == 1


@pytest.fixture(params=['direct', 'symlinked-parent', 'default-store'])
def scenario(tmp_path, monkeypatch, request):
    parent = tmp_path / 'parent'
    parent.mkdir(mode=0o700)
    if request.param == 'symlinked-parent':
        alias = tmp_path / 'parent-alias'
        alias.symlink_to(parent, target_is_directory=True)
        parent = alias
    run = parent / 'original'
    run.mkdir(mode=0o700)
    config = {'account_email': 'fixture@example.invalid', 'profile_dir': str(tmp_path / 'desktop'),
              'phone_state_dir': str(tmp_path / 'phone')}
    if request.param == 'default-store':
        from harness import provisioning
        del config['phone_state_dir']
        monkeypatch.setattr(provisioning, '__file__', str(tmp_path / 'repo/tests/e2e/harness/provisioning.py'))
    run_id = str(uuid.uuid5(uuid.NAMESPACE_URL, 'coincube-fresh-testnet4/' + str(run)))
    txid = 'a' * 64
    state = dict(run_id=run_id, profile_dir=str(run / 'profile'), owner_user_id=7, cube_id=3,
                 cube_uuid='cube', vault_id=4, descriptor_id='descriptor', target_key_id='8',
                 target_fingerprint='abcd1234', spend={'txid': txid, 'fee_sats': 612})
    artifacts = {k: state[k] for k in ('cube_id', 'cube_uuid', 'descriptor_id', 'target_key_id', 'target_fingerprint')}
    artifacts.update(vault_id='4', account_identity=digest({'email': config['account_email'], 'user_id': 7}),
                     desktop_identity=digest('creator'), request_id=str(uuid.uuid5(uuid.UUID(run_id), txid)),
                     session_id='session', intent={'txid': txid}, psbt_sha256=abort.sha(b'synthetic psbt'),
                     fee_sats=612, descriptor_file=str(run / 'descriptor.txt'))
    binding = {k: v for k, v in artifacts.items() if k != 'descriptor_file'}
    binding.update(run_id=run_id, journal_binding=digest(config), device='d' * 64)
    context = dict(binding=deepcopy(binding), request_id=binding['request_id'], session_id='session',
                   device='d' * 64, intent_sha256=digest(binding['intent']), fee_sats=612)
    artifacts.update(approval_binding=binding, approval_context=context, pin_dispatch_started={'context': context})
    for name, value in [('provision.json', state), ('checkpoint.json', {'binding': digest(config), 'artifacts': artifacts})]:
        atomic_write(run / name, json.dumps(value).encode())
    for name, value in [('descriptor.txt', b'synthetic descriptor'), ('local-signed.psbt', b'synthetic psbt'), ('run.lock', b'')]:
        atomic_write(run / name, value)
    config_path = tmp_path / 'config.json'
    atomic_write(config_path, json.dumps(config).encode())
    phone_dir = abort.phone_state_root(config) / artifacts['account_identity']
    phone_dir.mkdir(parents=True, mode=0o700)
    atomic_write(phone_dir / 'phone-secrets.keychain.json', b'{}')
    store = Store(dict(identity=binding['account_identity'] + ':' + binding['device'], verified=True,
                       pin='synthetic', pending_attempt='sign:' + binding['request_id']))
    args = SimpleNamespace(source_run=str(run), config=str(config_path), output_dir=str(tmp_path / 'abort'),
                           native_binary='/synthetic/native', signing_binary='/synthetic/connect', authorize_abort='test instruction')
    calls = []
    monkeypatch.setattr(abort, 'validate_config', lambda x: x)
    monkeypatch.setattr(abort, 'binary_digest', lambda x: digest(x))
    monkeypatch.setattr(abort, 'SecretStore', lambda *a: store)
    monkeypatch.setattr(abort, 'Provision', lambda *a: None)
    class Native:
        def __init__(self, *a): pass
        def call(self, action, **kwargs):
            assert action == 'cancel'
            calls.append(kwargs)
            _, _, receipt = material()
            receipt['terminal'].update(request_id=binding['request_id'], target_key_id='8')
            return receipt['terminal']
    monkeypatch.setattr(abort, 'Native', Native)
    return args, run, config, store, calls


def test_durable_terminal_precedes_cas_and_original_run_unchanged(scenario):
    args, run, config, store, calls = scenario
    before = {p.name: private_read(p) for p in run.iterdir()}
    save = store.save
    def checked_save(value):
        from pathlib import Path
        receipt = json.loads(private_read(Path(args.output_dir) / 'abort-terminal.json'))
        assert receipt['phase'] == 'terminal_verified'
        assert receipt['terminal']['status'] == 8
        save(value)
    store.save = checked_save
    assert abort.execute(args)['phase'] == 'quarantined'
    assert {p.name: private_read(p) for p in run.iterdir()} == before
    store.value['pending_attempt'] = 'sign:newer'
    assert abort.execute(args)['phase'] == 'already_quarantined'
    assert store.value['pending_attempt'] == 'sign:newer'
    assert store.writes == 1
    assert len(calls) == 2


def test_output_cannot_overlap_source_through_parent_alias(scenario):
    args, run, config, store, calls = scenario
    args.output_dir = str(run.resolve() / 'abort')
    with pytest.raises(Failed, match='OUTPUT_ISOLATION'):
        abort.execute(args)
    assert not calls
    assert store.writes == 0
    assert not (run / 'abort').exists()


@pytest.mark.parametrize('file', ['checkpoint.json', 'local-signed.psbt', 'provision.json'])
def test_source_change_blocks_before_cancel(scenario, file):
    args, run, config, store, calls = scenario
    if file.endswith('json'):
        value = json.loads(private_read(run / file))
        if file == 'checkpoint.json': value['artifacts']['session_id'] = 'other'
        else: value['owner_user_id'] = 8
        atomic_write(run / file, json.dumps(value).encode())
    else: atomic_write(run / file, b'changed')
    with pytest.raises(Failed): abort.execute(args)
    assert not calls
    assert store.writes == 0


@pytest.mark.parametrize('field,value', [('status', 3), ('submitted_signature_count', 1)])
def test_rpc_success_with_nonterminal_or_signature_race_never_cas(scenario, monkeypatch, field, value):
    args, run, config, store, calls = scenario
    original = abort.Native
    class Native(original):
        def call(self, *a, **kw):
            result = super().call(*a, **kw)
            result[field] = value
            return result
    monkeypatch.setattr(abort, 'Native', Native)
    with pytest.raises(Failed): abort.execute(args)
    assert len(calls) == 1
    assert store.writes == 0


def test_binary_replacement_after_rpc_prevents_cas(scenario, monkeypatch):
    args, run, config, store, calls = scenario
    monkeypatch.setattr(abort, 'binary_digest', lambda path: digest(path) if not calls else 'changed')
    with pytest.raises(Failed): abort.execute(args)
    assert len(calls) == 1
    assert store.writes == 0
