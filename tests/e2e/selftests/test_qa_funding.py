"""Offline funding registry boundaries; no device, keys or network."""
import pytest

from harness import qa_funding
from harness.state import Blocked, Failed


def test_pool_excludes_concurrent_owner(tmp_path, monkeypatch):
    monkeypatch.setattr(qa_funding, 'ROOT', tmp_path / 'registry')
    with qa_funding.pool():
        pool_context = qa_funding.pool()
        with pytest.raises(Blocked) as caught:
            with pool_context:
                pytest.fail('second owner admitted')
        assert caught.value.code == 'QA_FUNDING_BUSY'


def test_pool_rejects_orphan_claim(tmp_path, monkeypatch):
    monkeypatch.setattr(qa_funding, 'ROOT', tmp_path / 'registry')
    with qa_funding.pool() as data:
        data['claims']['tx:1'] = {'target_key': 'missing'}
        qa_funding.save_pool(data)
    pool_context = qa_funding.pool()
    with pytest.raises(Failed) as caught:
        with pool_context:
            pytest.fail('orphan claim admitted')
    assert caught.value.code == 'QA_FUNDING_REGISTRY_INDEX'


def test_empty_explicit_allowlist_does_not_authorize_unrelated_completed_run():
    config = qa_funding.options({'version': 1, 'sources': []})
    unrelated = {'kind': 'completed', 'entry': {'source_run': '/unrelated/qa/run'}}
    data = {'accounts': {'same-account': [unrelated]}}
    assert list(qa_funding.entries(data, 'same-account', config)) == []


def test_disabled_registration_does_not_import_managed_sources():
    config = qa_funding.options({'version': 1, 'sources': [], 'register_completed': False})
    data = {'accounts': {'same-account': [
        {'kind': 'completed', 'entry': {'source_run': '/unrelated/qa/run'}}]}}
    assert list(qa_funding.entries(data, 'same-account', config)) == []


# Use the native sponsor protocol's synthetic transaction fixtures. The
# orchestrator still executes its real intent/receipt validation and file I/O.
from copy import deepcopy
import json
from pathlib import Path
from types import SimpleNamespace
from test_sponsor import scenario, no_external, address, native_reply
from harness import sponsor
from harness.state import atomic_write, digest, private_read


@pytest.fixture
def flow(scenario, monkeypatch, tmp_path):
    args, evidence = scenario
    monkeypatch.setattr(qa_funding, 'ROOT', tmp_path / 'registry')
    config = {'amount_sats': 10000, 'fee_sats': 1000,
              'qa_funding': {'version': 1, 'sources': [{'source_run': args.source_run}],
                             'faucet_wait_seconds': 0, 'register_completed': True}}
    live = Path(args.target_run)
    state = json.loads(private_read(live / 'provision.json'))
    state.update(version=1, cube_id=12, cube_uuid=state['run_id'], spend=None)
    atomic_write(live / 'provision.json', sponsor.encoded(state))
    cp = json.loads(private_read(live / 'checkpoint.json'))
    cp.update(version=1, binding=digest(config), stages={})
    cp['artifacts'].update(desktop_identity='d' * 64, cube_id=12, cube_uuid=state['run_id'],
        target_key_id='synthetic-phone-key', target_fingerprint='1234abcd', strict_account='a' * 64,
        strict_acceptance={'version': 1, 'config_binding': digest(config)},
        strict_process={'device': 'e' * 64, 'build': 41,
                        'process': {'bundle_id': 'io.coincube.keychainApp', 'pid': 42}},
        qa_build={'define': 'COINCUBE_QA_HOME_REFRESH=true', 'app': {'build': 41}})
    atomic_write(live / 'checkpoint.json', sponsor.encoded(cp))
    def artifact(k, v):
        cp['artifacts'][k] = v
        atomic_write(live / 'checkpoint.json', sponsor.encoded(cp))
    journal = SimpleNamespace(directory=live, data=cp, artifact=artifact)
    wire = {'published': False, 'posts': [], 'native': 0, 'late': None}
    def native(command, *, input, **kwargs):
        wire['native'] += 1
        request = json.loads(input)
        receipt = native_reply(request)
        wire['receipt'] = receipt
        return SimpleNamespace(returncode=0, stdout=sponsor.encoded(receipt))
    monkeypatch.setattr(sponsor.subprocess, 'run', native)
    def http(url, *, body=None, **kwargs):
        if url.endswith('/block-height/0'):
            return qa_funding.GENESIS.encode()
        if body is not None:
            # Durable attempt and signed receipt must precede network mutation.
            data = json.loads(private_read(qa_funding.ROOT / 'pool.json'))
            claim = next(iter(data['claims'].values()))
            assert claim['phase'] == 'broadcasting'
            assert claim['receipt_sha256']
            wire['posts'].append(body)
            wire['published'] = True
            return wire['receipt']['txid'].encode()
        if url.endswith('/hex'):
            return wire['receipt']['raw_tx'].encode() if wire['published'] else None
        if url.endswith('/outspend/0'):
            return b'{"spent":false}'
        raise AssertionError(url)
    monkeypatch.setattr(qa_funding, 'http', http)
    runner = SimpleNamespace(config=config, journal=journal, run_id=state['run_id'],
        artifacts=cp['artifacts'], strict=SimpleNamespace(requested=False, observe=lambda: None),
        provision=SimpleNamespace(binary=args.native_binary), deadline=0,
        core=SimpleNamespace(utxos=lambda _: [wire['late']] if wire['late'] else [],
                             raw=lambda _: wire.get('receipt', {}).get('raw_tx', evidence[0]['raw_tx'])),
        pin_artifact=artifact)
    return runner, wire


def test_automatic_funding_signs_broadcasts_and_adopts_exact_outpoint(flow):
    runner, wire = flow
    qa_funding.Funding(runner).execute()
    point = runner.artifacts['funding_outpoint']
    assert point == {'txid': wire['receipt']['txid'], 'vout': 0, 'value_sats': 500000}
    assert len(wire['posts']) == 1
    assert wire['native'] == 1
    assert wire['posts'] == [wire['receipt']['raw_tx'].encode()]
    assert private_read(runner.journal.directory / 'funding.hex') == wire['posts'][0]
    with qa_funding.pool() as data:
        assert next(iter(data['claims'].values()))['phase'] == 'accepted'
        enrolled = data['accounts']['a' * 64][0]
        assert enrolled['authorization_roots'] == sorted(qa_funding.root_keys(qa_funding.options(runner.config['qa_funding'])))


@pytest.mark.parametrize('phase', ['reserved', 'signed', 'broadcasting'])
def test_crash_recovery_reuses_one_reserved_plan(flow, monkeypatch, phase):
    runner, wire = flow
    save = qa_funding.save_pool
    crashed = []
    def fault(data):
        save(data)
        if not crashed and any(c['phase'] == phase for c in data['claims'].values()):
            crashed.append(True)
            raise RuntimeError('synthetic crash after durable write')
    monkeypatch.setattr(qa_funding, 'save_pool', fault)
    funding = qa_funding.Funding(runner)
    with pytest.raises(RuntimeError):
        funding.execute()
    with qa_funding.pool() as data:
        before = deepcopy(data['claims'])
    monkeypatch.setattr(qa_funding, 'save_pool', save)
    qa_funding.Funding(runner).execute()
    with qa_funding.pool() as data:
        assert list(data['claims']) == list(before)
        assert next(iter(data['claims'].values()))['intent_sha256'] == next(iter(before.values()))['intent_sha256']
    assert len(wire['posts']) == 1
    assert wire['native'] == 1


def test_lost_post_response_reconciles_without_second_payment(flow, monkeypatch):
    runner, wire = flow
    http = qa_funding.http
    def lost(url, **kwargs):
        result = http(url, **kwargs)
        if kwargs.get('body'):
            raise Blocked('HTTP_UNCERTAIN', 'synthetic lost response')
        return result
    monkeypatch.setattr(qa_funding, 'http', lost)
    qa_funding.Funding(runner).execute()
    assert len(wire['posts']) == 1
    assert runner.artifacts['funding_outpoint']['txid'] == wire['receipt']['txid']


def test_partial_propagation_does_not_post_or_select_another_source(flow, monkeypatch):
    runner, wire = flow
    monkeypatch.setattr(qa_funding.Funding, 'observed', lambda *a: 'partial')
    funding = qa_funding.Funding(runner)
    with pytest.raises(Blocked) as caught:
        funding.execute()
    assert caught.value.code == 'QA_FUNDING_PROPAGATION'
    assert wire['posts'] == []
    assert wire['native'] == 1


def test_late_faucet_is_adopted_without_signing(flow):
    runner, wire = flow
    wire['late'] = ('f' * 64, 0, 100000)
    qa_funding.Funding(runner).execute()
    assert runner.artifacts['funding_outpoint']['txid'] == 'f' * 64
    assert wire['native'] == 0
    assert wire['posts'] == []
    with qa_funding.pool() as data:
        assert data['claims'] == {}


@pytest.mark.parametrize('field,value', [('strict_process', None), ('target_fingerprint', ''), ('qa_build', {})])
def test_missing_target_identity_prevents_key_lookup(flow, field, value):
    runner, wire = flow
    runner.journal.artifact(field, value)
    with pytest.raises(Failed):
        qa_funding.Funding(runner)
    assert wire['native'] == 0
    assert wire['posts'] == []


@pytest.mark.parametrize('changed_field', ['phone', 'native'])
def test_target_process_mutation_after_prepare_prevents_signature(flow, monkeypatch, changed_field):
    runner, wire = flow
    execute = sponsor.execute
    def changed(args):
        result = execute(args)
        if changed_field == 'phone':
            process = deepcopy(runner.artifacts['strict_process'])
            process['process']['pid'] += 1
            runner.journal.artifact('strict_process', process)
        else:
            atomic_write(Path(runner.provision.binary), b'replaced native candidate')
        return result
    monkeypatch.setattr(sponsor, 'execute', changed)
    funding = qa_funding.Funding(runner)
    with pytest.raises(Failed) as caught:
        funding.execute()
    assert caught.value.code == ('QA_FUNDING_TARGET_CHANGED' if changed_field == 'phone' else 'QA_FUNDING_NATIVE_CHANGED')
    assert wire['native'] == 0
    assert wire['posts'] == []


def test_signed_receipt_tamper_never_posts(flow, monkeypatch):
    runner, wire = flow
    monkeypatch.setattr(qa_funding.Funding, 'observed', lambda *a: 'partial')
    funding = qa_funding.Funding(runner)
    with pytest.raises(Blocked):
        funding.execute()
    output = runner.journal.directory.with_name(runner.journal.directory.name + '-funding')
    raw = json.loads(private_read(output / 'sponsor-receipt.json'))
    raw['raw_tx'] += '00'
    atomic_write(output / 'sponsor-receipt.json', sponsor.encoded(raw))
    funding = qa_funding.Funding(runner)
    with pytest.raises(Failed) as caught:
        funding.execute()
    assert caught.value.code == 'QA_FUNDING_RECEIPT_CHANGED'
    assert wire['posts'] == []


@pytest.mark.parametrize('change', [{'maximum_amount_sats': 500001}, {'fee_sats': 1001},
                                    {'maximum_amount_sats': True}, {'faucet_wait_seconds': -1},
                                    {'version': True}, {'sources': [{'source_run': 'relative'}]}])
def test_automatic_pool_limits(change):
    with pytest.raises(Failed):
        qa_funding.options({'version': 1, 'sources': [], **change})


def test_global_source_reservation_cannot_be_claimed_by_another_run(flow):
    runner, wire = flow
    source = Path(runner.config['qa_funding']['sources'][0]['source_run'])
    txid = json.loads(private_read(source / 'provision.json'))['spend']['txid']
    with qa_funding.pool() as data:
        data['claims'][txid + ':1'] = {'target_key': 'other-config-and-run', 'phase': 'reserved'}
        data['runs']['other-config-and-run'] = txid + ':1'
        qa_funding.save_pool(data)
    funding = qa_funding.Funding(runner)
    with pytest.raises(Blocked) as caught:
        funding.execute()
    assert caught.value.code == 'QA_FUNDING_EMPTY'
    assert wire['native'] == 0
    assert wire['posts'] == []


def test_foreign_account_source_never_reaches_native_signer(flow):
    runner, wire = flow
    source = Path(runner.config['qa_funding']['sources'][0]['source_run'])
    cp = json.loads(private_read(source / 'checkpoint.json'))
    cp['artifacts']['account_identity'] = 'b' * 64
    atomic_write(source / 'checkpoint.json', sponsor.encoded(cp))
    funding = qa_funding.Funding(runner)
    with pytest.raises(Failed) as caught:
        funding.execute()
    assert caught.value.code == 'QA_FUNDING_ACCOUNT'
    assert wire['native'] == 0
    assert wire['posts'] == []


def test_exhausted_source_pool_blocks_without_faucet_or_signer_retry(flow, monkeypatch):
    runner, wire = flow
    def spent(*args):
        raise Failed('SOURCE_SPENT', 'synthetic spent source')
    monkeypatch.setattr(sponsor, 'observe', spent)
    funding = qa_funding.Funding(runner)
    with pytest.raises(Blocked) as caught:
        funding.execute()
    assert caught.value.code == 'QA_FUNDING_EMPTY'
    assert wire['native'] == 0
    assert wire['posts'] == []


def test_automatic_source_descendants_are_scoped_to_explicit_roots():
    config = qa_funding.options({'version': 1, 'sources': [{'source_run': '/pool/seed'}]})
    root = next(iter(qa_funding.root_keys(config)))
    own = {'kind': 'completed', 'entry': {'source_run': '/pool/descendant'}, 'authorization_roots': [root]}
    unrelated = {'kind': 'completed', 'entry': {'source_run': '/other/descendant'}, 'authorization_roots': ['other']}
    data = {'accounts': {'account': [own, unrelated]}}
    assert [e for e, _ in qa_funding.entries(data, 'account', config)] == [own['entry'], {'source_run': '/pool/seed'}]


@pytest.mark.parametrize('crash', [False, True])
def test_completed_registration_snapshot_survives_live_checkpoint_replay(flow, monkeypatch, crash):
    from harness.provisioning import STAGES
    runner, wire = flow
    qa_funding.Funding(runner).execute()
    live = runner.journal.directory
    raw = wire['receipt']['raw_tx']
    state = json.loads(private_read(live / 'provision.json'))
    state['spend'] = {'txid': wire['receipt']['txid'], 'change_sats': 488000, 'change_address': address(34)}
    atomic_write(live / 'provision.json', sponsor.encoded(state))
    cp = runner.journal.data
    cp['stages'] = {k: {'status': 'passed', 'at': 1} for k in STAGES}
    cp['artifacts'].update(signature_count_min=2, strict_pin_input={'completed': True},
        strict_home_refresh={'home_refresh_proven': True}, request_id='request', session_id='session',
        broadcast_txid=wire['receipt']['txid'], public_acceptance={'txid': wire['receipt']['txid']},
        broadcast_hex_sha256=sponsor.sha(raw.encode()), fee_sats=1000)
    atomic_write(live / 'checkpoint.json', sponsor.encoded(cp))
    atomic_write(live / 'signed.hex', raw.encode())
    calls = []
    runner.strict.require_completion = lambda *args: calls.append(args)
    monkeypatch.setattr(qa_funding, 'public_transaction', lambda _: {'fee': 1000})
    if crash:
        save = qa_funding.save_pool
        monkeypatch.setattr(qa_funding, 'save_pool', lambda _: (_ for _ in ()).throw(RuntimeError('crash before enrollment')))
        with pytest.raises(RuntimeError):
            qa_funding.register_completed(runner)
        monkeypatch.setattr(qa_funding, 'save_pool', save)
    qa_funding.register_completed(runner)
    with qa_funding.pool() as data:
        item = deepcopy(data['accounts']['a' * 64][0])
    snapshot = Path(item['entry']['source_run'])
    original = {n: private_read(snapshot / n) for n in qa_funding.FILES}
    cp['stages']['verify']['at'] = 2
    cp['artifacts']['public_acceptance']['confirmed'] = True
    atomic_write(live / 'checkpoint.json', sponsor.encoded(cp))
    qa_funding.register_completed(runner)
    with qa_funding.pool() as data:
        assert data['accounts']['a' * 64][0] == item
        assert len([e for e in data['accounts']['a' * 64] if e['kind'] == 'completed']) == 1
    assert {n: private_read(snapshot / n) for n in qa_funding.FILES} == original
    assert json.loads(original['provision.json'])['profile_dir'] == str(live / 'profile')
    assert calls == [('request', 'session')] * (3 if crash else 2)
    # The donor snapshot remains compatible with the native sponsor protocol,
    # including the original secret locator's profile and run identity.
    future = live.with_name('future')
    future.mkdir(mode=0o700)
    future_state = deepcopy(state)
    future_state.update(run_id='future', cube_uuid='future', vault_id=99, profile_dir=str(future / 'profile'), spend=None)
    future_cp = deepcopy(cp)
    future_cp['artifacts'].update(vault_id='99', receive_address=address(68))
    atomic_write(future / 'provision.json', sponsor.encoded(future_state))
    atomic_write(future / 'checkpoint.json', sponsor.encoded(future_cp))
    atomic_write(future / 'descriptor.txt', b'future-public-descriptor')
    monkeypatch.setattr(sponsor, 'observe', lambda txid, vout: [{'raw_tx': raw}])
    plan, _, donor = sponsor.make_intent(snapshot, future, 400000, 1000)
    assert plan['source'] == item['source_binding']
    assert donor['profile_dir'] == str(live / 'profile')
    assert donor['run_id'] == runner.run_id
    runner.config['qa_funding']['sources'].append({'source_run': '/unapproved/new-root'})
    with pytest.raises(Failed) as caught:
        qa_funding.register_completed(runner)
    assert caught.value.code == 'QA_FUNDING_COMPLETION'


def test_faucet_failure_automatically_enters_fallback(flow, monkeypatch):
    from harness import provisioning
    runner, wire = flow
    runner.config['qa_funding']['register_completed'] = False  # no registry enrollment in this integration stub
    def unavailable(*args):
        raise Blocked('FAUCET_UNCERTAIN', 'synthetic 503')
    monkeypatch.setattr(provisioning, 'faucet_claim', unavailable)
    calls = []
    monkeypatch.setattr(qa_funding, 'Funding', lambda r: SimpleNamespace(execute=lambda: calls.append(r)))
    provisioning.FreshRunner.stage_funding(runner)
    assert calls == [runner]
    assert runner.artifacts['qa_faucet_failure']['code'] == 'FAUCET_UNCERTAIN'
    assert wire['native'] == 0


def configure_large_spend(runner):
    runner.config.update(amount_sats=100000, fee_sats=10000)
    binding = digest(runner.config)
    runner.journal.data['binding'] = binding
    runner.journal.artifact('strict_acceptance', {'version': 1, 'config_binding': binding})


def test_pool_cap_must_cover_configured_spend_fee_and_change(flow):
    runner, wire = flow
    runner.config['qa_funding']['maximum_amount_sats'] = 100000
    configure_large_spend(runner)
    with pytest.raises(Failed) as caught:
        qa_funding.Funding(runner)
    assert caught.value.code == 'QA_FUNDING_CAPACITY'
    assert wire['native'] == 0
    assert wire['posts'] == []


def test_depleted_donor_is_skipped_before_constructing_insufficient_payment(flow, monkeypatch):
    runner, wire = flow
    configure_large_spend(runner)
    metadata = qa_funding.source_metadata
    def depleted(*args):
        source, prior, _, binding = metadata(*args)
        return source, prior, 110000, binding  # Only108k after funding fee+source change, below111k.
    monkeypatch.setattr(qa_funding, 'source_metadata', depleted)
    monkeypatch.setattr(sponsor, 'make_intent', lambda *a: pytest.fail('insufficient donor reached transaction construction'))
    funding = qa_funding.Funding(runner)
    with pytest.raises(Blocked) as caught:
        funding.execute()
    assert caught.value.code == 'QA_FUNDING_EMPTY'
    assert wire['native'] == 0
    assert wire['posts'] == []


def test_impossible_pool_cap_fails_workstation_validation_before_phone_setup():
    from harness import provisioning
    config = {'version': 2, 'account_email': 'qa@example.invalid', 'profile_dir': '/synthetic/desktop',
              'connect_rest': provisioning.REST, 'connect_grpc': provisioning.GRPC,
              'strict_acceptance': {'version': 1},
              'ios': {'minimum_build': 41, 'qa_build': {'checkout': '/synthetic/keychain',
                       'revision': '0' * 40, 'flutter': '/synthetic/flutter'}},
              'amount_sats': 100000, 'fee_sats': 10000,
              'qa_funding': {'version': 1, 'sources': [], 'maximum_amount_sats': 100000}}
    with pytest.raises(Failed) as caught:
        provisioning.validate_config(config)
    assert caught.value.code == 'QA_FUNDING_CAPACITY'


def test_explorer_disagreement_waits_then_adopts_late_faucet(flow, monkeypatch):
    runner, wire = flow
    runner.deadline = 3
    clock = [0]
    monkeypatch.setattr(qa_funding.time, 'monotonic', lambda: clock[0])
    monkeypatch.setattr(qa_funding.time, 'sleep', lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    responses = iter([Blocked('FUNDING_PROPAGATION', 'one explorer is behind'), [('f' * 64, 0, 100000)]])
    def utxos(_):
        value = next(responses)
        if isinstance(value, Exception):
            raise value
        return value
    runner.core.utxos = utxos
    qa_funding.Funding(runner).execute()
    assert clock[0] == 1
    assert runner.artifacts['funding_outpoint']['txid'] == 'f' * 64
    assert wire['native'] == 0
    assert wire['posts'] == []
    with qa_funding.pool() as data:
        assert data['claims'] == {}


@pytest.mark.parametrize('integrity_failure', [False, True])
def test_persistent_or_invalid_explorer_disagreement_never_funds(flow, monkeypatch, integrity_failure):
    runner, wire = flow
    runner.deadline = 2
    clock, calls = [0], []
    monkeypatch.setattr(qa_funding.time, 'monotonic', lambda: clock[0])
    monkeypatch.setattr(qa_funding.time, 'sleep', lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    def utxos(_):
        calls.append(True)
        error = Failed if integrity_failure else Blocked
        raise error('FUNDING_PROPAGATION', 'synthetic disagreement')
    runner.core.utxos = utxos
    funding = qa_funding.Funding(runner)
    with pytest.raises(Blocked) as caught:
        funding.execute()
    assert caught.value.code == 'FUNDING_PROPAGATION'
    assert len(calls) == (1 if integrity_failure else 3)
    assert clock[0] == (0 if integrity_failure else 2)
    assert wire['native'] == 0
    assert wire['posts'] == []


def test_late_faucet_releases_only_fresh_unsigned_reservation(flow, monkeypatch):
    runner, wire = flow
    save = qa_funding.save_pool
    released = []
    def faucet_arrives(data):
        save(data)
        if any(c['phase'] == 'reserved' for c in data['claims'].values()):
            wire['late'] = ('f' * 64, 0, 100000)
        if data['external']:
            released.append(deepcopy(data))
    monkeypatch.setattr(qa_funding, 'save_pool', faucet_arrives)
    qa_funding.Funding(runner).execute()
    assert wire['native'] == 0
    assert wire['posts'] == []
    assert runner.artifacts['funding_outpoint']['txid'] == 'f' * 64
    assert released
    assert released[-1]['claims'] == {}
    assert released[-1]['runs'] == {}
    # A separate fresh target can use the released source, not the cached
    # external target's outcome. Preserve the original source secret locator.
    old = runner.journal.directory
    live = old.with_name('second-target')
    live.mkdir(mode=0o700)
    state = json.loads(private_read(old / 'provision.json'))
    state.update(run_id='second', cube_uuid='second', cube_id=13, vault_id=13,
                 descriptor_id='descriptor-second', profile_dir=str(live / 'profile'))
    cp = deepcopy(runner.journal.data)
    cp['artifacts'].update(cube_uuid='second', cube_id=13, vault_id='13', descriptor_id='descriptor-second',
                           receive_address=address(68))
    cp['artifacts'].pop('funding_outpoint')
    atomic_write(live / 'provision.json', sponsor.encoded(state))
    atomic_write(live / 'checkpoint.json', sponsor.encoded(cp))
    atomic_write(live / 'descriptor.txt', b'second-public-descriptor')
    other = SimpleNamespace(**vars(runner))
    def artifact(k, v):
        cp['artifacts'][k] = v
        atomic_write(live / 'checkpoint.json', sponsor.encoded(cp))
    other.journal = SimpleNamespace(directory=live, data=cp, artifact=artifact)
    other.artifacts, other.pin_artifact, other.run_id = cp['artifacts'], artifact, 'second'
    wire['late'] = None
    monkeypatch.setattr(qa_funding, 'save_pool', save)
    qa_funding.Funding(other).execute()
    assert wire['native'] == 1
    assert len(wire['posts']) == 1
    with qa_funding.pool() as data:
        assert len(data['external']) == 1
        assert len(data['claims']) == 1
        assert next(iter(data['claims'].values()))['phase'] == 'accepted'


def test_released_target_crash_resumes_external_outcome_without_reacquiring(flow, monkeypatch):
    from harness import provisioning
    runner, wire = flow
    save, adopt = qa_funding.save_pool, qa_funding.Funding.adopt
    def faucet_arrives(data):
        save(data)
        if any(c['phase'] == 'reserved' for c in data['claims'].values()):
            wire['late'] = ('f' * 64, 0, 100000)
    monkeypatch.setattr(qa_funding, 'save_pool', faucet_arrives)
    monkeypatch.setattr(qa_funding.Funding, 'adopt', lambda *a: (_ for _ in ()).throw(RuntimeError('crash after release')))
    funding = qa_funding.Funding(runner)
    with pytest.raises(RuntimeError):
        funding.execute()
    monkeypatch.setattr(qa_funding.Funding, 'adopt', adopt)
    monkeypatch.setattr(provisioning, 'faucet_claim', lambda *a: pytest.fail('cached external outcome retried faucet'))
    provisioning.FreshRunner.stage_funding(runner)
    assert runner.artifacts['funding_outpoint']['txid'] == 'f' * 64
    assert wire['native'] == 0
    assert wire['posts'] == []
    with qa_funding.pool() as data:
        assert data['claims'] == {}
        assert data['runs'] == {}
        assert len(data['external']) == 1


@pytest.mark.parametrize('hidden_receipt', [False, True])
def test_resumed_reservation_is_not_released_even_before_broadcast(flow, monkeypatch, hidden_receipt):
    runner, wire = flow
    save = qa_funding.save_pool
    def crash(data):
        phase = next(iter(data['claims'].values()))['phase'] if data['claims'] else None
        if hidden_receipt and phase == 'signed':
            raise RuntimeError('crash before signed registry write')
        save(data)
        if not hidden_receipt and phase == 'reserved':
            raise RuntimeError('crash after reservation')
    monkeypatch.setattr(qa_funding, 'save_pool', crash)
    funding = qa_funding.Funding(runner)
    with pytest.raises(RuntimeError):
        funding.execute()
    monkeypatch.setattr(qa_funding, 'save_pool', save)
    wire['late'] = ('f' * 64, 0, 100000)
    qa_funding.Funding(runner).execute()
    with qa_funding.pool() as data:
        assert len(data['claims']) == 1
        assert len(data['runs']) == 1
        assert data['external'] == {}
        assert next(iter(data['claims'].values()))['phase'] == 'external'
    assert wire['native'] == (1 if hidden_receipt else 0)
    assert wire['posts'] == []


def test_signed_late_faucet_keeps_valid_signature_source_quarantined(flow, monkeypatch):
    runner, wire = flow
    save = qa_funding.save_pool
    def faucet_arrives(data):
        save(data)
        if any(c['phase'] == 'signed' for c in data['claims'].values()):
            wire['late'] = ('f' * 64, 0, 100000)
    monkeypatch.setattr(qa_funding, 'save_pool', faucet_arrives)
    qa_funding.Funding(runner).execute()
    with qa_funding.pool() as data:
        assert len(data['claims']) == len(data['runs']) == 1
        assert data['external'] == {}
        record = next(iter(data['claims'].values()))
        assert record['phase'] == 'external'
        assert record['receipt_sha256']
    assert wire['native'] == 1
    assert wire['posts'] == []


def test_explorer_disagreement_then_empty_reserves_only_one_donor(flow, monkeypatch):
    runner, wire = flow
    runner.deadline = 2
    clock, calls = [0], []
    monkeypatch.setattr(qa_funding.time, 'monotonic', lambda: clock[0])
    monkeypatch.setattr(qa_funding.time, 'sleep', lambda seconds: clock.__setitem__(0, clock[0] + seconds))
    def utxos(_):
        calls.append(True)
        if len(calls) == 1:
            raise Blocked('FUNDING_PROPAGATION', 'one explorer is behind')
        return []
    runner.core.utxos = utxos
    qa_funding.Funding(runner).execute()
    assert clock[0] == 1
    assert wire['native'] == 1
    assert len(wire['posts']) == 1
    with qa_funding.pool() as data:
        assert len(data['claims']) == len(data['runs']) == 1


def test_mutated_enrolled_source_binding_stops_before_keys_or_donor_selection(flow, monkeypatch):
    runner, wire = flow
    config = qa_funding.options(runner.config['qa_funding'])
    entry = config['sources'][0]
    source = Path(entry['source_run'])
    with qa_funding.pool() as data:
        qa_funding.add_source(data, 'a' * 64, {'kind': 'completed', 'entry': entry,
            'source_binding': sponsor.read_run(source)[2], 'authorization_roots': sorted(qa_funding.root_keys(config))})
    cp = json.loads(private_read(source / 'checkpoint.json'))
    cp['stages'] = {'verify': {'status': 'passed', 'at': 2}}
    atomic_write(source / 'checkpoint.json', sponsor.encoded(cp))
    runner.account_user_id = 5
    monkeypatch.setattr(sponsor, 'make_intent', lambda *a: pytest.fail('changed donor reached construction'))
    for action in (lambda: qa_funding.preflight(runner), lambda: qa_funding.Funding(runner).execute()):
        with pytest.raises(Failed) as caught:
            action()
        assert caught.value.code == 'QA_FUNDING_SOURCE_CHANGED'
    assert wire['native'] == 0
    assert wire['posts'] == []


@pytest.mark.parametrize('kind', ['existing_output', 'broken_symlink'])
def test_existing_helper_output_or_link_prevents_unsigned_release(flow, monkeypatch, kind):
    runner, wire = flow
    output = runner.journal.directory.with_name(runner.journal.directory.name + '-funding')
    if kind == 'existing_output':
        output.mkdir(mode=0o700)
        atomic_write(output / 'sponsor-receipt.json', b'uncertain previous helper output')
    else:
        output.symlink_to(output.with_name('missing-output'))
    save = qa_funding.save_pool
    def faucet_arrives(data):
        save(data)
        if any(c['phase'] == 'reserved' for c in data['claims'].values()):
            wire['late'] = ('f' * 64, 0, 100000)
    monkeypatch.setattr(qa_funding, 'save_pool', faucet_arrives)
    qa_funding.Funding(runner).execute()
    with qa_funding.pool() as data:
        assert len(data['claims']) == len(data['runs']) == 1
        assert data['external'] == {}
    assert wire['native'] == 0
    assert wire['posts'] == []
