"""Disposable retirement boundaries; every cloud/native/chain interaction is fake."""
from copy import deepcopy
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from harness import qa_cleanup as cleanup, qa_funding, sponsor
from harness.provisioning import FreshRunner, STAGES
from harness.state import Blocked, Failed, atomic_write, digest, private_read
from test_sponsor import address, raw_transaction


@pytest.fixture
def flow(tmp_path, monkeypatch):
    monkeypatch.setattr(qa_funding, 'ROOT', tmp_path / 'registry')
    directory = tmp_path / 'completed'
    snapshot = tmp_path / 'completed-completed-source'
    directory.mkdir(mode=0o700); snapshot.mkdir(mode=0o700)
    run_id = '12345678-1234-5678-abcd-123456789012'
    raw = raw_transaction('b' * 64, [(10000, '0014' + '11' * 20), (89000, '0020' + '22' * 32)])
    txid = sponsor.transaction(raw)['txid']
    state = {'version': 1, 'run_id': run_id, 'cube_uuid': run_id, 'cube_name': 'Testnet4 QA ' + run_id,
             'cube_id': 12, 'vault_id': 23, 'target_key_id': '34', 'target_fingerprint': 'aabbccdd',
             'descriptor_id': '1234abcd', 'owner_user_id': 5, 'registration_started': True,
             'profile_dir': str(directory / 'profile'),
             'spend': {'txid': txid, 'change_sats': 89000, 'change_address': address(34)}}
    artifacts = {'account_identity': 'a' * 64, 'cube_id': 12, 'cube_uuid': run_id, 'vault_id': '23',
                 'descriptor_id': '1234abcd', 'target_key_id': '34', 'target_fingerprint': 'aabbccdd',
                 'receive_address': address(51), 'broadcast_txid': txid, 'public_acceptance': {'txid': txid}, 'request_id': run_id}
    cp = {'artifacts': artifacts, 'binding': 'binding', 'stages': {k: {'status': 'passed'} for k in STAGES}}
    for target in (directory, snapshot):
        atomic_write(target / 'provision.json', sponsor.encoded(state))
        atomic_write(target / 'checkpoint.json', sponsor.encoded(cp))
        atomic_write(target / 'descriptor.txt', b'synthetic-public-descriptor')
    for name, value in {'signed.hex': raw.encode(), 'local-signed.psbt': b'synthetic-public-psbt',
                        'provision-secrets.keychain.json': b'{"locator":"native-only"}'}.items():
        atomic_write(directory / name, value)
    (directory / 'profile').mkdir(mode=0o700)
    atomic_write(directory / 'profile/encrypted-backup.bin', b'synthetic-encrypted-profile-backup')
    atomic_write(directory / 'funding.hex', b'synthetic-public-funding')
    atomic_write(directory / 'phone-key.json', b'{"public_key":"synthetic"}')
    atomic_write(directory / 'signed.psbt', b'synthetic-public-fully-signed-psbt')
    atomic_write(directory / 'run.lock', b'')
    xml = '<testsuites><testsuite>' + ''.join(
        '<testcase name="' + stage + '"><properties><property name="stage" value="' + stage + '"/>'
        '<property name="outcome" value="passed"/></properties></testcase>' for stage in STAGES) + '</testsuite></testsuites>'
    atomic_write(directory / 'results.xml', xml.encode())
    record = {'version': 1, 'account': 'a' * 64, 'owner': 5, 'run_id': run_id,
              'directory': str(directory), 'snapshot': str(snapshot), 'source_binding': sponsor.read_run(snapshot)[2],
              'cube_id': 12, 'cube_name': state['cube_name'], 'vault_id': 23, 'descriptor_id': '1234abcd',
              'key_id': 34, 'fingerprint': 'aabbccdd', 'keychain_id': 45, 'phase': 'enrolled',
              'encryption_pubkey': '02' + 'ab' * 32, 'sealed': True,
              'retained_files': cleanup.retained_tree(directory)}
    runner = SimpleNamespace(config={'amount_sats': 10000, 'fee_sats': 1000,
                                    'qa_cleanup': {'version': 1, 'preserve_runs': []}},
        strict=SimpleNamespace(requested=False, observe=lambda: None), account_user_id=5,
        artifacts={'account_identity': 'a' * 64}, run_id='new-run', profile=tmp_path / 'current-profile',
        signing_binary='/synthetic/signing', provision=SimpleNamespace(binary='/synthetic/provision'),
        journal=SimpleNamespace(data={'binding': 'new-binding'}, artifact=lambda k, v: runner.artifacts.update({k: v})))
    key = {'id': 34, 'cubeId': 12, 'keychainId': 45, 'network': 'testnet4',
           'fingerprint': 'aabbccdd', 'status': 'active', 'vaultId': 23}
    cube = {'id': 12, 'userId': 5, 'uuid': run_id, 'name': state['cube_name'], 'network': 'testnet4',
            'status': 'active', 'members': [], 'pendingInvites': [], 'lightningAddress': None,
            'encryptionPubkey': '02' + 'ab' * 32,
            'sparkStableBalance': None, 'hasRecoveryKit': False,
            'keys': [{'id': 34, 'primaryOwnerId': 5}], 'vault': {'id': 23}}
    vault = {'id': 23, 'cubeId': 12, 'fingerprint': '1234abcd', 'status': 'active',
             'members': [{'keyId': 34, 'role': 'keyholder'}]}
    cloud = {'cube': cube, 'vault': vault, 'key': key, 'other': [], 'deletes': [],
             'fail_delete': None, 'pending': [], 'decrypt': [], 'residual': 89000,
             'shared': [], 'other_details': {}, 'other_keys': {}, 'other_vaults': {}, 'reads': []}
    def api(_, path, *, delete=False, body=None, missing=False):
        if delete:
            import fcntl, os
            fd = os.open(directory / 'run.lock', os.O_RDWR)
            try:
                with pytest.raises(BlockingIOError): fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            finally: os.close(fd)
            with open(qa_funding.ROOT / 'pool.json') as f: saved = json.load(f)
            r = saved['retirements']['fixture']
            phase = {'/connect/cubes/12/vault': 'vault_intent', '/keys/delete': 'key_intent', '/connect/cubes/12': 'cube_intent'}[path]
            assert r['phase'] == phase  # fsynced before the destructive API call
            cloud['deletes'].append(path)
            if cloud['fail_delete'] == path:
                raise Blocked('SYNTHETIC_LOST_DELETE', 'synthetic transport failure')
            if path.endswith('/vault'):
                cloud['vault'] = None; cloud['cube']['vault'] = None
            elif path == '/keys/delete':
                assert body == {'keyId': 34, 'keychainId': 45}
                cloud['key'] = None; cloud['cube']['keys'] = []
            else: cloud['cube'] = None
            return None
        cloud['reads'].append(path)
        if path == '/connect/cubes/shared-with-me': return deepcopy(cloud['shared'])
        if path == '/keys': return {'keys': [cloud['key']] if cloud['key'] else []}
        if path == '/connect/cubes': return ([cloud['cube']] if cloud['cube'] else []) + cloud['other']
        if path == '/connect/cubes/12': return deepcopy(cloud['cube'])
        if path == '/connect/cubes/12/vault': return deepcopy(cloud['vault'])
        if path == '/connect/cubes/12/keys': return ([{'id': 34}] if cloud['key'] else []) if cloud['cube'] else None
        if path.startswith('/connect/cubes/'):
            parts = path.split('/')
            cube_id = int(parts[3])
            rows = cloud['other'] + cloud['shared']
            row = next((row for row in rows if row.get('id') == cube_id), None)
            if row is not None:
                detail = cloud['other_details'].get(cube_id, row)
                if path.endswith('/keys'): return deepcopy(cloud['other_keys'].get(cube_id, detail.get('keys', [])))
                if path.endswith('/vault'): return deepcopy(cloud['other_vaults'].get(cube_id, detail.get('vault')))
                return deepcopy(detail)
        raise AssertionError(path)
    def http(url, **kwargs):
        if url.endswith('/block-height/0'): return cleanup.GENESIS.encode()
        if url.endswith('/hex'): return raw.encode()
        if url.endswith('/outspend/1'): return b'{"spent":false}'
        if url.endswith('/address/' + address(51) + '/utxo'): return b'[]'
        if url.endswith('/address/' + address(34) + '/utxo'):
            return sponsor.encoded([{'txid': txid, 'vout': 1, 'value': cloud['residual']}])
        raise AssertionError(url)
    def native_call(self, action, **kwargs):
        assert action == 'cleanup_audit'
        return {'owner_user_id': 5, 'sessions': cloud['pending'], 'decrypt_cubes': cloud['decrypt']}
    from harness.provisioning import Provision
    monkeypatch.setattr(Provision, 'call', lambda self, action: {'backup_verified': True, 'run_id': run_id})
    monkeypatch.setattr(cleanup.Native, 'call', native_call)
    monkeypatch.setattr(cleanup, 'api', api)
    monkeypatch.setattr(cleanup, 'http', http)
    with qa_funding.pool() as data:
        data['retirements'] = {'fixture': deepcopy(record)}
        qa_funding.save_pool(data)
    return runner, cloud, record


def test_cleanup_retires_exact_cloud_graph_and_keeps_recovery_and_residual(flow):
    runner, cloud, record = flow
    before = {p: p.read_bytes() for p in Path(record['directory']).rglob('*') if p.is_file()}
    cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == ['/connect/cubes/12/vault', '/keys/delete', '/connect/cubes/12']
    assert before == {p: p.read_bytes() for p in Path(record['directory']).rglob('*') if p.is_file()}
    assert runner.artifacts['qa_cleanup']['residual_sats'] == 89000
    with qa_funding.pool() as data:
        assert cleanup.retired(data, {'source_run': record['snapshot']})
        assert data['retirements']['fixture']['phase'] == 'complete'
    cleanup.ensure_capacity(runner)
    assert len(cloud['deletes']) == 3  # a second candidate can never be selected


@pytest.mark.parametrize('change', ['owner', 'network', 'members', 'pending_invites', 'shared_key', 'foreign_key', 'vault_members', 'descriptor', 'recovery_kit', 'lightning'])
def test_cleanup_refuses_foreign_shared_or_changed_graph(flow, change):
    runner, cloud, _ = flow
    if change == 'owner': cloud['cube']['userId'] = 99
    elif change == 'network': cloud['key']['network'] = 'mainnet'
    elif change == 'members': cloud['cube']['members'] = [{'id': 99}]
    elif change == 'pending_invites': cloud['cube']['pendingInvites'] = [{'id': 99}]
    elif change == 'shared_key': cloud['other'] = [{'id': 99, 'keys': [{'id': 34}]}]
    elif change == 'foreign_key': cloud['cube']['keys'].append({'id': 99, 'primaryOwnerId': 99})
    elif change == 'vault_members': cloud['vault']['members'].append({'keyId': 99, 'role': 'heir'})
    elif change == 'descriptor': cloud['vault']['fingerprint'] = 'changed'
    elif change == 'recovery_kit': cloud['cube']['hasRecoveryKit'] = True
    else: cloud['cube']['lightningAddress'] = 'personal@example.invalid'
    with pytest.raises(Failed): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('pending', ['vault', 'key', 'decrypt_numeric', 'decrypt_uuid'])
def test_active_signing_or_recovery_request_prevents_retirement(flow, pending):
    runner, cloud, record = flow
    if pending == 'vault': cloud['pending'] = [{'vault_id': '23', 'key_ids': []}]
    elif pending == 'key': cloud['pending'] = [{'vault_id': 'other', 'key_ids': ['34']}]
    elif pending == 'decrypt_numeric': cloud['decrypt'] = ['12']
    else: cloud['decrypt'] = [record['run_id']]
    with pytest.raises(Failed, match='ACTIVE_REQUEST'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('path', ['/connect/cubes/12/vault', '/keys/delete', '/connect/cubes/12'])
def test_crash_after_delete_reconciles_exact_ids_without_second_delete(flow, monkeypatch, path):
    runner, cloud, _ = flow
    api = cleanup.api
    crashed = []
    def lost(*args, **kwargs):
        result = api(*args, **kwargs)
        if kwargs.get('delete') and args[1] == path and not crashed:
            crashed.append(True)
            raise Blocked('LOST_RESPONSE', 'synthetic response lost after server deletion')
        return result
    monkeypatch.setattr(cleanup, 'api', lost)
    with pytest.raises(Blocked): cleanup.ensure_capacity(runner)
    cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == ['/connect/cubes/12/vault', '/keys/delete', '/connect/cubes/12']


def test_key_delete_failure_does_not_cascade_cube(flow):
    runner, cloud, _ = flow
    cloud['fail_delete'] = '/keys/delete'
    with pytest.raises(Blocked): cleanup.ensure_capacity(runner)
    assert cloud['cube'] is not None
    assert '/connect/cubes/12' not in cloud['deletes']


def test_personal_qa_label_is_never_cleanup_authority(flow):
    runner, cloud, _ = flow
    with qa_funding.pool() as data:
        data['retirements'] = {}
        qa_funding.save_pool(data)
    cloud['key']['name'] = 'QA looks-disposable'
    with pytest.raises(Blocked, match='No safely enrolled'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('field', ['account', 'owner'])
def test_foreign_enrollment_does_not_authorize_cleanup(flow, field):
    runner, cloud, _ = flow
    with qa_funding.pool() as data:
        data['retirements']['fixture'][field] = 'foreign' if field == 'account' else 99
        qa_funding.save_pool(data)
    with pytest.raises(Blocked): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


def test_preserved_funding_root_uses_ids_even_when_recovery_label_differs(flow):
    runner, cloud, record = flow
    runner.config['qa_cleanup']['preserve_runs'] = [record['directory']]
    cloud['key']['name'] = 'QA recovery-label-from-another-attempt'
    with pytest.raises(Blocked, match='No safely enrolled'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('name', ['provision.json', 'descriptor.txt', 'provision-secrets.keychain.json', 'signed.hex'])
def test_changed_recovery_or_history_stops_before_cloud_mutation(flow, name):
    runner, cloud, record = flow
    p = Path(record['directory']) / name
    atomic_write(p, p.read_bytes() + b' ')
    with pytest.raises(Failed): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


def test_unexpected_incoming_funds_stop_retirement(flow):
    runner, cloud, _ = flow
    cloud['residual'] = 90000
    with pytest.raises(Failed, match='BALANCE_INDEX'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


def test_capacity_available_performs_no_cleanup(monkeypatch):
    runner = SimpleNamespace(keys=lambda: [], phone_key_label=lambda: 'QA current', artifacts={},
        account={'tokens': {'access_token': 'synthetic'}}, account_user_id=5, config={'qa_cleanup': {'version': 1, 'preserve_runs': []}})
    monkeypatch.setattr('harness.provisioning.http', lambda url, **kwargs: sponsor.encoded(
        {'success': True, 'data': {'entitlements': {'personalKeyLimit': 7}}} if url.endswith('/plan') else
        {'success': True, 'data': {'userId': 5, 'network': 'testnet4', 'noOfKeys': 0, 'keyLimit': 21}}))
    monkeypatch.setattr(cleanup, 'ensure_capacity', lambda _: pytest.fail('cleanup without a capacity refusal'))
    monkeypatch.setattr(cleanup, 'reconcile_capacity', lambda _: False)
    FreshRunner.preflight_capacity(runner)


def test_current_key_resume_performs_no_cleanup(monkeypatch):
    runner = SimpleNamespace(keys=lambda: [{'name': 'QA current', 'network': 'testnet4'}],
        phone_key_label=lambda: 'QA current', artifacts={'phone_key_creation_started': True}, config={})
    monkeypatch.setattr(cleanup, 'ensure_capacity', lambda _: pytest.fail('cleanup during current-key resume'))
    FreshRunner.preflight_capacity(runner)


def test_cleanup_after_strict_request_is_refused(flow):
    runner, cloud, _ = flow
    runner.strict.requested = True
    with pytest.raises(Failed, match='MODE'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('phase', ['reserved', 'signed', 'broadcasting', 'external', 'quarantined'])
def test_uncertain_funding_claim_protects_source_and_target(flow, phase):
    runner, _, record = flow
    data = {'claims': {'source:1': {'phase': phase, 'entry': {'source_run': record['directory']},
                                  'identity': {'artifacts': {'cube_id': 999}}}}, 'accounts': {}, 'external': {}}
    assert cleanup.protected_ids(runner, data) == {12, 999}


def test_eligible_managed_donor_is_protected_until_change_is_spent(flow, monkeypatch):
    runner, _, record = flow
    runner.config['qa_funding'] = {'version': 1, 'sources': []}
    data = {'claims': {}, 'external': {}, 'accounts': {'a' * 64: [
        {'kind': 'completed', 'entry': {'source_run': record['snapshot']}, 'source_binding': record['source_binding']}]}}
    monkeypatch.setattr(qa_funding, 'source_metadata', lambda *a: (Path(record['snapshot']), None, 500000, {}))
    monkeypatch.setattr(sponsor, 'observe', lambda *a: [{'spent': False}])
    assert cleanup.protected_ids(runner, data) == {12}
    def spent(*a): raise Failed('SOURCE_SPENT', 'synthetic spent donor')
    monkeypatch.setattr(sponsor, 'observe', spent)
    assert cleanup.protected_ids(runner, data) == set()


def test_cloud_tombstone_skips_managed_source_but_not_explicit_protected_root(flow):
    runner, _, record = flow
    cleanup.ensure_capacity(runner)
    with qa_funding.pool() as data:
        config = qa_funding.options({'version': 1, 'sources': [{'source_run': record['directory']}]})
        entry = {'source_run': record['snapshot']}
        data['accounts']['a' * 64] = [{'kind': 'completed', 'entry': entry,
            'source_binding': record['source_binding'], 'authorization_roots': sorted(qa_funding.root_keys(config))}]
        assert list(qa_funding.entries(data, 'a' * 64, config)) == [(config['sources'][0], None)]
    assert Path(record['snapshot']).is_dir()
    assert Path(record['directory'], 'provision-secrets.keychain.json').exists()


def test_no_second_fixture_for_one_consumer_even_when_first_did_not_free_capacity(flow):
    runner, cloud, record = flow
    cleanup.ensure_capacity(runner)
    with qa_funding.pool() as data:
        second = deepcopy(record); second['cube_id'] = 99
        data['retirements']['unrelated-second'] = second
        qa_funding.save_pool(data)
    cleanup.ensure_capacity(runner)
    assert len(cloud['deletes']) == 3


def test_completed_strict_run_auto_enrolls_without_enabling_funding(flow, monkeypatch):
    from harness.provisioning import STAGES
    runner, _, record = flow
    runner.stop = False
    runner.run_id = record['run_id']
    runner.journal.directory = Path(record['directory'])
    cp = json.loads(private_read(runner.journal.directory / 'checkpoint.json'))
    cp.update(version=1, binding=digest(runner.config), stages={k: {'status': 'passed'} for k in STAGES})
    raw = private_read(runner.journal.directory / 'signed.hex').decode()
    cp['artifacts'].update(signature_count_min=2, strict_pin_input={'completed': True},
        strict_home_refresh={'home_refresh_proven': True}, session_id='complete-session', fee_sats=1000,
        broadcast_hex_sha256=sponsor.sha(raw.encode()))
    for directory in (Path(record['directory']), Path(record['snapshot'])):
        atomic_write(directory / 'checkpoint.json', sponsor.encoded(cp))
    runner.journal.data = cp
    runner.strict.require_completion = lambda request_id, session_id: None
    monkeypatch.setattr(qa_funding, 'public_transaction', lambda _: {'fee': 1000})
    monkeypatch.setattr(qa_funding, 'http', lambda *a, **k: raw.encode())
    with qa_funding.pool() as data:
        data['retirements'] = {}
        qa_funding.save_pool(data)
    cleanup.register_completed(runner)
    with qa_funding.pool() as data:
        assert data['accounts'] == {}  # cleanup authorization does not enroll a funding source
        enrolled = next(iter(data['retirements'].values()))
        assert enrolled['run_id'] == record['run_id']
        assert enrolled['key_id'] == 34
        assert enrolled['phase'] == 'enrolled'
    cleanup.register_completed(runner)
    with qa_funding.pool() as data:
        assert len(data['retirements']) == 1


def test_incomplete_run_cannot_auto_enroll_cleanup(flow):
    runner, _, record = flow
    runner.stop = False
    runner.run_id = record['run_id']
    runner.journal.directory = Path(record['directory'])
    with pytest.raises((Failed, KeyError)):
        cleanup.register_completed(runner)


@pytest.mark.parametrize('field', ['members', 'pendingInvites', 'keys', 'vault'])
def test_missing_sharing_inventory_is_not_treated_as_empty(flow, field):
    runner, cloud, _ = flow
    del cloud['cube'][field]
    with pytest.raises(Failed, match='SCHEMA'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


def test_candidate_run_lease_busy_prevents_every_delete(flow):
    import fcntl, os
    runner, cloud, record = flow
    fd = os.open(Path(record['directory']) / 'run.lock', os.O_RDWR)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with pytest.raises(Blocked, match='still owned'): cleanup.ensure_capacity(runner)
    finally:
        os.close(fd)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('path', ['funding.hex', 'phone-key.json', 'signed.psbt', 'results.xml', 'profile/encrypted-backup.bin'])
def test_every_retained_history_and_profile_file_is_required(flow, path):
    runner, cloud, record = flow
    Path(record['directory'], path).unlink()
    with pytest.raises(Failed, match='RECOVERY_CHANGED'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('change', ['phase', 'snapshot', 'account', 'run_id', 'cube_id', 'binding', 'history', 'forged'])
def test_tombstone_cannot_mask_wrong_retirement_or_changed_history(flow, change):
    runner, _, record = flow
    cleanup.ensure_capacity(runner)
    with qa_funding.pool() as data:
        tombstone = data['retired_cloud_sources'][record['snapshot']]
        if change == 'phase': data['retirements']['fixture']['phase'] = 'key_intent'
        elif change == 'snapshot': data['retirements']['fixture']['snapshot'] = '/foreign/snapshot'
        elif change in ('account', 'run_id'): tombstone[change] = 'foreign'
        elif change == 'cube_id': tombstone['cube_id'] = 99
        elif change == 'binding': tombstone['source_binding']['state_sha256'] = 'changed'
        elif change == 'history': Path(record['directory'], 'funding.hex').unlink()
        else: tombstone['retirement'] = 'missing-record'
        with pytest.raises(Failed): cleanup.retired(data, {'source_run': record['snapshot']})


def test_empty_utxo_index_without_spent_proof_is_not_zero_balance(flow, monkeypatch):
    runner, cloud, _ = flow
    original = cleanup.http
    monkeypatch.setattr(cleanup, 'http', lambda url, **kwargs: b'[]' if url.endswith('/utxo') else original(url, **kwargs))
    with pytest.raises(Failed, match='BALANCE_INDEX'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


def test_capacity_resume_finishes_cube_after_key_success_even_with_free_slot(flow, monkeypatch):
    from types import MethodType
    runner, cloud, _ = flow
    original = cleanup.api
    lost = []
    def api(*args, **kwargs):
        result = original(*args, **kwargs)
        if kwargs.get('delete') and args[1] == '/keys/delete' and not lost:
            lost.append(True)
            raise Blocked('LOST_DELETE', 'synthetic key deleted before response')
        return result
    monkeypatch.setattr(cleanup, 'api', api)
    with pytest.raises(Blocked): cleanup.ensure_capacity(runner)
    assert cloud['key'] is None
    assert cloud['cube'] is not None
    runner.account = {'tokens': {'access_token': 'synthetic'}}
    runner.keys = lambda: [{'id': 100 + i, 'name': 'other', 'network': 'testnet4'} for i in range(6)]
    runner.phone_key_label = lambda: 'QA new'
    runner.preflight_capacity = MethodType(FreshRunner.preflight_capacity, runner)
    monkeypatch.setattr('harness.provisioning.http', lambda url, **kwargs: sponsor.encoded(
        {'success': True, 'data': {'entitlements': {'personalKeyLimit': 7}}} if url.endswith('/plan') else
        {'success': True, 'data': {'userId': 5, 'network': 'testnet4', 'noOfKeys': 6, 'keyLimit': 21}}))
    runner.preflight_capacity()
    assert cloud['cube'] is None
    assert cloud['deletes'] == ['/connect/cubes/12/vault', '/keys/delete', '/connect/cubes/12']


def test_unsealed_completion_can_be_sealed_once_after_lost_callback(flow):
    runner, _, record = flow
    with qa_funding.pool() as data:
        saved = data['retirements']['fixture']
        saved['sealed'] = False
        saved['retained_files'].pop('results.xml')
        qa_funding.save_pool(data)
    cleanup.ensure_capacity(runner)
    with qa_funding.pool() as data:
        assert data['retirements']['fixture']['sealed'] is True
        assert data['retirements']['fixture']['phase'] == 'complete'


def test_actual_pytest_junit_is_written_before_completion_seals(flow, monkeypatch, tmp_path):
    import os, subprocess, sys
    runner, _, record = flow
    directory = Path(record['directory'])
    with qa_funding.pool() as data:
        saved = data['retirements'].pop('fixture')
        saved['sealed'] = False
        saved['retained_files'].pop('results.xml')
        key = digest({'account': saved['account'], 'run_id': saved['run_id'], 'directory': saved['directory']})
        data['retirements'][key] = saved
        qa_funding.save_pool(data)
    (directory / 'results.xml').unlink()
    child = tmp_path / 'child-pytest'; child.mkdir(mode=0o700)
    original = Path(__file__).resolve().parents[1] / 'conftest.py'
    (child / 'conftest.py').write_text(f'''
import os
os.umask(0o077)
import importlib.util
from pathlib import Path
from types import SimpleNamespace
from harness import qa_funding
qa_funding.ROOT = Path({str(qa_funding.ROOT)!r})
spec = importlib.util.spec_from_file_location('live_qa_hooks', {str(original)!r})
hooks = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hooks)
pytest_addoption = hooks.pytest_addoption
pytest_sessionfinish = hooks.pytest_sessionfinish
pytest_terminal_summary = hooks.pytest_terminal_summary
def pytest_configure(config):
    hooks.pytest_configure(config)
    config._testnet4_runner = SimpleNamespace(stop=False, config={{'qa_cleanup': {{'version':1,'preserve_runs':[]}}}},
        artifacts={{'account_identity': {record['account']!r}}}, account_user_id={record['owner']!r},
        run_id={record['run_id']!r}, journal=SimpleNamespace(directory=Path({record['directory']!r})))
''')
    (child / 'test_completed.py').write_text('''
import pytest
from harness.provisioning import STAGES
@pytest.mark.parametrize('stage', STAGES)
def test_completed(stage, record_property):
    record_property('stage', stage)
    record_property('outcome', 'passed')
''')
    env = dict(os.environ, PYTEST_DISABLE_PLUGIN_AUTOLOAD='1', PYTHONDONTWRITEBYTECODE='1',
               PYTHONPATH=str(original.parent))
    result = subprocess.run([sys.executable, '-m', 'pytest', str(child), '-q', '--junitxml',
                             str(directory / 'results.xml'), '-o', 'junit_family=xunit1'],
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
    assert result.returncode == 0, result.stdout.decode()
    with qa_funding.pool() as data:
        assert data['retirements'][key]['sealed'] is True
        assert 'results.xml' in data['retirements'][key]['retained_files']
    runner.run_id = record['run_id']
    runner.journal.directory = directory
    # Lost callback after its durable save can call the same seal idempotently.
    cleanup.seal_completed(runner)


def test_missing_cube_keys_after_delete_requires_explicit_empty_key_endpoint(flow, monkeypatch):
    runner, cloud, _ = flow
    original = cleanup.api
    def api(*args, **kwargs):
        result = original(*args, **kwargs)
        if not kwargs.get('delete') and args[1] == '/connect/cubes/12' and not cloud['key'] and result:
            result.pop('keys', None)
        return result
    monkeypatch.setattr(cleanup, 'api', api)
    cleanup.ensure_capacity(runner)
    assert cloud['cube'] is None


def test_foreign_account_cannot_use_tombstone_to_skip_a_source(flow):
    runner, _, record = flow
    cleanup.ensure_capacity(runner)
    with qa_funding.pool() as data:
        with pytest.raises(Failed, match='TOMBSTONE'):
            cleanup.retired(data, {'source_run': record['snapshot']}, account='foreign-account')


@pytest.mark.parametrize('change', ['missing', 'failure', 'duplicate'])
def test_unsealed_completion_requires_final_complete_passed_junit(flow, change):
    runner, cloud, record = flow
    with qa_funding.pool() as data:
        data['retirements']['fixture']['sealed'] = False
        data['retirements']['fixture']['retained_files'].pop('results.xml')
        qa_funding.save_pool(data)
    path = Path(record['directory']) / 'results.xml'
    if change == 'missing': path.unlink()
    elif change == 'failure': atomic_write(path, path.read_bytes().replace(b'</testcase>', b'<failure/></testcase>', 1))
    else: atomic_write(path, path.read_bytes().replace(b'value="environment"', b'value="phone_setup"', 1))
    with pytest.raises(Blocked): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('proof', ['different', 'missing', 'unknown'])
def test_spent_change_never_means_zero_without_known_matching_spender(flow, monkeypatch, proof):
    runner, cloud, _ = flow
    original = cleanup.http
    def http(url, **kwargs):
        if url.endswith('/outspend/1'):
            value = {'spent': True, 'txid': 'f' * 64, 'vin': 0}
            if proof == 'different' and url.startswith(cleanup.EXPLORERS[1]): value['txid'] = 'e' * 64
            if proof == 'missing': value.pop('txid')
            return sponsor.encoded(value)
        return original(url, **kwargs)
    monkeypatch.setattr(cleanup, 'http', http)
    with pytest.raises(Blocked): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


def accepted_sponsors(flow, monkeypatch, tmp_path, hops=1):
    """Construct only public accepted v1/v2 protocol evidence, no native keys."""
    from test_sponsor import native_reply
    runner, cloud, record = flow
    source = Path(record['snapshot']); live = Path(record['directory'])
    state = json.loads(private_read(live / 'provision.json'))
    cp = json.loads(private_read(live / 'checkpoint.json'))
    amount = 102000 if hops == 1 else 203000
    original_raw = raw_transaction('b' * 64, [(10000, '0014' + '11' * 20), (amount, '0020' + '22' * 32)])
    original_txid = sponsor.transaction(original_raw)['txid']
    state['spend'].update(txid=original_txid, change_sats=amount)
    cp['artifacts'].update(broadcast_txid=original_txid, public_acceptance={'txid': original_txid})
    for directory in (source, live):
        atomic_write(directory / 'provision.json', sponsor.encoded(state))
        atomic_write(directory / 'checkpoint.json', sponsor.encoded(cp))
    atomic_write(live / 'signed.hex', original_raw.encode())
    record['source_binding'] = sponsor.read_run(source)[2]
    record['retained_files'] = cleanup.retained_tree(live)
    root = 'c' * 64
    previous_txid, previous_raw, previous_value = original_txid, original_raw, amount
    parent, claims, entries, raws, spends = None, {}, [], {original_txid: original_raw}, {}
    source_entry = {'source_run': str(source)}
    entries.append({'kind': 'completed', 'entry': source_entry,
                    'source_binding': record['source_binding'], 'authorization_roots': [root]})
    for hop in range(1, hops + 1):
        target = tmp_path / ('target-snapshot-' + str(hop)); target.mkdir(mode=0o700)
        target_live = tmp_path / ('target-live-' + str(hop))
        target_state = deepcopy(state)
        target_state.update(run_id='target-' + str(hop), cube_id=100 + hop, vault_id=100 + hop,
                            spend=None, profile_dir=str(target_live / 'profile'))
        target_cp = deepcopy(cp)
        target_cp['artifacts'].update(cube_id=target_state['cube_id'], vault_id=str(target_state['vault_id']),
                                      receive_address=address(60 + hop))
        for name, content in {'provision.json': sponsor.encoded(target_state),
                              'checkpoint.json': sponsor.encoded(target_cp), 'descriptor.txt': b'synthetic-target-descriptor'}.items():
            atomic_write(target / name, content)
        binding = sponsor.read_run(target)[2]
        identity = {'run_id': target_state['run_id'], 'binding': 'synthetic-target-config',
                    'artifacts': {'cube_id': target_state['cube_id'], 'receive_address': address(60 + hop)}}
        manifest = {'version': 1, 'directory': str(target_live), 'identity': identity,
                    'hashes': {n: sponsor.sha(private_read(target / n)) for n in qa_funding.FILES}}
        atomic_write(target / 'manifest.json', sponsor.encoded(manifest))
        intent = {'version': hop, 'network': 'testnet4', 'authorization_ref': 'coincube#1137:testnet4-sponsor',
            'source': record['source_binding'], 'target': binding, 'previous_txid': previous_txid,
            'previous_vout': 1, 'previous_value_sats': previous_value,
            'previous_raw_sha256': sponsor.sha(previous_raw.encode()), 'source_script': '0020' + '22' * 32,
            'target_address': address(60 + hop), 'target_script': sponsor.p2wsh_script(address(60 + hop)),
            'amount_sats': 100000, 'fee_sats': 1000, 'change_sats': previous_value - 101000}
        if parent:
            intent['prior_sponsor'] = {'intent_json': parent[0].decode(), 'receipt_json': parent[1].decode(), 'target': parent[2]}
        out = tmp_path / ('sponsor-' + str(hop)); out.mkdir(mode=0o700)
        raw_intent = sponsor.encoded(intent)
        atomic_write(out / 'sponsor-intent.json', raw_intent)
        receipt = native_reply({'sponsor_intent_file': str(out / 'sponsor-intent.json'),
                                'sponsor_intent_sha256': sponsor.sha(raw_intent)})
        receipt['native_binary_sha256'] = 'd' * 64
        raw_receipt = sponsor.encoded(receipt)
        atomic_write(out / 'sponsor-receipt.json', raw_receipt)
        source_entry = {'source_run': str(source)}
        if hop == 2: source_entry['source_sponsor_dir'] = str(parent[3])
        claim = {'phase': 'accepted', 'account': record['account'], 'entry': source_entry,
            'authorization_roots': [root], 'output': str(out), 'intent_json': raw_intent.decode(),
            'intent_sha256': sponsor.sha(raw_intent), 'receipt_sha256': sponsor.sha(raw_receipt),
            'native_sha256': 'd' * 64, 'amount_sats': 100000, 'fee_sats': 1000,
            'snapshot': str(target), 'live': str(target_live), 'identity': identity,
            'manifest_sha256': sponsor.sha(sponsor.encoded(manifest)), 'target_key': 'target-' + str(hop)}
        claims[previous_txid + ':1'] = claim
        raws[receipt['txid']] = receipt['raw_tx']
        spends[previous_txid] = {'spent': True, 'txid': receipt['txid'], 'vin': 0}
        if hop == 1:
            entries.append({'kind': 'sponsor', 'entry': {'source_run': str(source), 'source_sponsor_dir': str(out)},
                            'source_binding': record['source_binding'], 'authorization_roots': [root],
                            'receipt_sha256': sponsor.sha(raw_receipt)})
        parent = (raw_intent, raw_receipt, binding, out)
        previous_txid, previous_raw, previous_value = receipt['txid'], receipt['raw_tx'], receipt['change_sats']
    spends[previous_txid] = {'spent': False}
    def http(url, **kwargs):
        if url.endswith('/block-height/0'): return cleanup.GENESIS.encode()
        if '/tx/' in url:
            txid = url.split('/tx/')[1].split('/')[0]
            if url.endswith('/hex'): return raws[txid].encode()
            if url.endswith('/outspend/1'): return sponsor.encoded(spends[txid])
        if url.endswith('/address/' + address(51) + '/utxo'): return b'[]'
        if url.endswith('/address/' + address(34) + '/utxo'):
            return sponsor.encoded([{'txid': previous_txid, 'vout': 1, 'value': previous_value}])
        raise AssertionError(url)
    def spent(*args): raise Failed('SOURCE_SPENT', 'synthetic managed donor has been consumed')
    monkeypatch.setattr(sponsor, 'observe', spent)
    monkeypatch.setattr(cleanup, 'http', http)
    with qa_funding.pool() as data:
        data['retirements']['fixture'] = deepcopy(record)
        data['claims'] = claims
        data['runs'] = {claim['target_key']: k for k, claim in claims.items()}
        data['accounts'][record['account']] = entries
        qa_funding.save_pool(data)
    return runner, cloud, record, claims, spends, raws, previous_txid


@pytest.mark.parametrize('hops', [1, 2])
def test_known_accepted_qa_sponsorship_retains_exact_small_child_change(flow, monkeypatch, tmp_path, hops):
    runner, cloud, _, _, _, _, _ = accepted_sponsors(flow, monkeypatch, tmp_path, hops)
    cleanup.ensure_capacity(runner)
    assert runner.artifacts['qa_cleanup']['residual_sats'] == 1000
    assert cloud['deletes'] == ['/connect/cubes/12/vault', '/keys/delete', '/connect/cubes/12']


@pytest.mark.parametrize('change', ['receipt_hash', 'source_binding', 'intent_hash', 'spender_raw', 'input_link', 'late_index'])
def test_known_qa_child_requires_all_frozen_receipt_and_chain_evidence(flow, monkeypatch, tmp_path, change):
    runner, cloud, _, claims, spends, raws, child_txid = accepted_sponsors(flow, monkeypatch, tmp_path)
    if change in ('receipt_hash', 'source_binding', 'intent_hash'):
        with qa_funding.pool() as data:
            claim = next(iter(data['claims'].values()))
            if change == 'receipt_hash': claim['receipt_sha256'] = 'wrong'
            elif change == 'intent_hash': claim['intent_sha256'] = 'wrong'
            else: data['accounts']['a' * 64][0]['source_binding']['state_sha256'] = 'wrong'
            qa_funding.save_pool(data)
    elif change == 'spender_raw': raws[child_txid] = raws[child_txid] + '00'
    elif change == 'input_link': next(s for s in spends.values() if s['spent'])['vin'] = 1
    else:
        original = cleanup.http
        monkeypatch.setattr(cleanup, 'http', lambda url, **kwargs: b'[]' if url.endswith('/utxo') else original(url, **kwargs))
    with pytest.raises(Blocked): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


def test_unknown_spend_after_last_supported_qa_hop_is_quarantined(flow, monkeypatch, tmp_path):
    runner, cloud, _, _, spends, _, child = accepted_sponsors(flow, monkeypatch, tmp_path, 2)
    spends[child] = {'spent': True, 'txid': 'e' * 64, 'vin': 0}
    with pytest.raises(Blocked, match='outside the bounded'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('flag', [False, None, 'missing'])
def test_other_authoritative_key_inventory_protects_signer_without_has_vault_assertion(flow, flag):
    runner, cloud, _ = flow
    row = {'id': 99}
    if flag != 'missing': row['hasVault'] = flag
    cloud['other'] = [row]
    cloud['other_keys'][99] = [{'id': 34}]
    with pytest.raises(Failed, match='SHARED_KEY'): cleanup.ensure_capacity(runner)
    assert '/connect/cubes/99/keys' in cloud['reads']
    assert cloud['deletes'] == []


def visible_other(cloud, *, shared=False, network='bitcoin'):
    row = {'id': 99, 'hasVault': False}
    detail = {'id': 99, 'userId': 77 if shared else 5, 'network': network,
              'members': [], 'pendingInvites': [], 'vault': None}
    cloud['shared' if shared else 'other'] = [row]
    cloud['other_details'][99] = detail
    cloud['other_keys'][99] = []
    return row, detail


@pytest.mark.parametrize('shared', [False, True])
def test_owned_and_shared_discovery_scan_all_networks_before_deleting(flow, shared):
    runner, cloud, _ = flow
    visible_other(cloud, shared=shared, network='bitcoin')
    cloud['other_keys'][99] = [{'id': 34}]
    with pytest.raises(Failed, match='SHARED_KEY'): cleanup.ensure_capacity(runner)
    assert '/connect/cubes' in cloud['reads']
    assert '/connect/cubes/shared-with-me' in cloud['reads']
    assert '/connect/cubes/99/keys' in cloud['reads']
    assert cloud['deletes'] == []


@pytest.mark.parametrize('flag', [False, None, 'missing'])
@pytest.mark.parametrize('shared', [False, True])
def test_unrelated_complete_inventories_allow_retirement_without_has_vault_assertion(flow, flag, shared):
    runner, cloud, _ = flow
    row, _ = visible_other(cloud, shared=shared)
    if flag == 'missing': row.pop('hasVault')
    else: row['hasVault'] = flag
    cleanup.ensure_capacity(runner)
    assert '/connect/cubes/99/keys' in cloud['reads']
    assert '/connect/cubes/99' in cloud['reads']
    assert ('/connect/cubes/99/vault' in cloud['reads']) is (not shared)
    assert cloud['deletes'] == ['/connect/cubes/12/vault', '/keys/delete', '/connect/cubes/12']


@pytest.mark.parametrize('shared', [False, True])
@pytest.mark.parametrize('origin', ['detail_keys', 'detail_vault', 'listed_keys', 'listed_vault', 'owned_vault'])
def test_stale_or_contradictory_other_membership_never_erases_a_reference(flow, shared, origin):
    runner, cloud, _ = flow
    row, detail = visible_other(cloud, shared=shared)
    vault = {'id': 88, 'cubeId': 99, 'members': [{'id': 1, 'keyId': 34, 'role': 'keyholder'}]}
    if origin == 'detail_keys': detail['keys'] = [{'id': 34}]
    elif origin == 'detail_vault': detail['vault'] = vault
    elif origin == 'listed_keys': row['keys'] = [{'id': 34}]
    elif origin == 'listed_vault': row['vault'] = vault
    elif shared:
        # Shared /vault is owner-only; membership must ride the member-visible detail.
        detail['vault'] = vault
    else: cloud['other_vaults'][99] = vault
    with pytest.raises(Failed, match='SHARED_KEY'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('value', [None, {}, [{'id': True}], [{'id': 0}], [None], [{'name': 'omitted ID'}]])
def test_malformed_authoritative_other_keys_fail_before_deletion(flow, value):
    runner, cloud, _ = flow
    visible_other(cloud)
    cloud['other_keys'][99] = value
    with pytest.raises(Failed, match='OTHER_KEY_SCHEMA'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('field', ['members', 'pendingInvites', 'vault', 'userId', 'network'])
def test_missing_other_detail_inventory_is_not_absence(flow, field):
    runner, cloud, _ = flow
    _, detail = visible_other(cloud)
    detail.pop(field)
    with pytest.raises(Failed, match='OTHER_CUBE_SCHEMA'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('path', ['/connect/cubes/shared-with-me', '/connect/cubes/99/keys',
                                   '/connect/cubes/99', '/connect/cubes/99/vault'])
def test_unavailable_other_inventory_stops_before_first_delete(flow, monkeypatch, path):
    runner, cloud, _ = flow
    visible_other(cloud)
    original = cleanup.api
    def unavailable(*args, **kwargs):
        if args[1] == path: raise Blocked('SYNTHETIC_INVENTORY_UNAVAILABLE', 'synthetic inventory service failure')
        return original(*args, **kwargs)
    monkeypatch.setattr(cleanup, 'api', unavailable)
    with pytest.raises(Blocked): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


@pytest.mark.parametrize('shared', [False, True])
def test_authoritative_other_keys_must_account_for_complete_detail_membership(flow, shared):
    runner, cloud, _ = flow
    _, detail = visible_other(cloud, shared=shared)
    cloud['other_keys'][99] = [{'id': 77}]
    # A positive authoritative ID with omitted attached and Vault data is not empty.
    with pytest.raises(Failed, match='OTHER_KEY_GRAPH'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []


def test_shared_vault_inventory_never_uses_owner_only_404_as_absence(flow, monkeypatch):
    runner, cloud, _ = flow
    _, detail = visible_other(cloud, shared=True)
    detail['vault'] = {'id': 88, 'cubeId': 99, 'members': [{'id': 1, 'keyId': 77, 'role': 'keyholder'}]}
    cloud['other_keys'][99] = [{'id': 77}]
    original = cleanup.api
    def api(*args, **kwargs):
        assert args[1] != '/connect/cubes/99/vault', 'shared owner-only 404 cannot prove Vault absence'
        return original(*args, **kwargs)
    monkeypatch.setattr(cleanup, 'api', api)
    cleanup.ensure_capacity(runner)
    assert len(cloud['deletes']) == 3


@pytest.mark.parametrize('inventory', ['owned', 'shared'])
def test_malformed_discovery_cannot_silently_omit_foreign_references(flow, monkeypatch, inventory):
    runner, cloud, _ = flow
    original = cleanup.api
    path = '/connect/cubes' if inventory == 'owned' else '/connect/cubes/shared-with-me'
    def api(*args, **kwargs):
        if args[1] == path: return {'cubes': []}
        return original(*args, **kwargs)
    monkeypatch.setattr(cleanup, 'api', api)
    with pytest.raises(Failed, match='CUBE_DISCOVERY'): cleanup.ensure_capacity(runner)
    assert cloud['deletes'] == []
