"""Offline native admission + real retirement saga; external services are fakes."""
from types import SimpleNamespace

import pytest
from test_qa_cleanup import flow  # Reuse audited fixtures; do not edit their owner.
from harness import native_recovery as native, qa_cleanup as cleanup
from harness.state import Blocked, Journal


def admission_for(flow, tmp_path, monkeypatch):
    original, cloud, record = flow
    runner = native.NativeCleanupContext.__new__(native.NativeCleanupContext)
    runner.__dict__.update(original.__dict__)
    runner.account = {'email': 'qa@example.invalid', 'tokens': {'access_token': 'offline-only'}}
    # Native admission never consults the synthetic mobile policy.
    del runner.strict
    observations = []
    runner.observe = lambda: observations.append('legacy-identities')
    journal = Journal(tmp_path/'native-consumer', {'unit': 'offline-only'})
    suite = SimpleNamespace(journal=journal, consumer={'unit': 'offline-only'}, directory=journal.directory,
                            config={}, binding={})
    config = {'chain': 'testnet4', 'owner_id': 5, 'case_root': str(journal.directory/'not-created')}
    original_digest = native.digest
    monkeypatch.setattr(native, 'digest', lambda value: 'a'*64 if value == {'email':'qa@example.invalid','user_id':5} else original_digest(value))
    calls = []
    def worker(cfg, binding, executable, receipt, request, directory, access_token=None):
        action = request['action']; calls.append(action)
        if action == 'begin_cleanup':
            assert cloud['cube'] is not None  # Actual initial FULL quota observation.
        assert action in ('observe_cleanup', 'begin_cleanup')
        assert not __import__('pathlib').Path(config['case_root']).exists()
    monkeypatch.setattr(native, 'checked_process', worker)
    admission = native.NativeCleanupConsumerV1(suite, runner, config, runner.account, 5, None, {})
    return runner, admission, calls, observations, journal


def test_native_full_quota_admits_only_once_and_finishes_after_capacity_is_available(flow, tmp_path, monkeypatch):
    runner, admission, calls, observations, journal = admission_for(flow, tmp_path, monkeypatch)
    try:
        cleanup.ensure_capacity(runner, admission=admission)
        assert calls.count('begin_cleanup') == 1
        assert flow[1]['cube'] is None
        cleanup.ensure_capacity(runner, admission=admission)
        assert calls.count('begin_cleanup') == 1
        assert len(flow[1]['deletes']) == 3
        assert observations  # Native contexts pin real backup/audit identities.
    finally:
        journal.close()


def test_lost_cube_delete_finishes_exact_saga_even_when_slot_is_now_available(flow, tmp_path, monkeypatch):
    runner, admission, calls, _, journal = admission_for(flow, tmp_path, monkeypatch)
    original_api = cleanup.api
    lost = [False]
    def api(*args, **kwargs):
        result = original_api(*args, **kwargs)
        if args[1] == '/connect/cubes/12' and kwargs.get('delete') and not lost[0]:
            lost[0] = True
            raise Blocked('OFFLINE_LOST_RESPONSE', 'offline synthetic lost response')
        return result
    monkeypatch.setattr(cleanup, 'api', api)
    try:
        with pytest.raises(Blocked):
            cleanup.ensure_capacity(runner, admission=admission)
        assert flow[1]['cube'] is None
        assert cleanup.reconcile_capacity(runner, admission=admission)
        assert calls.count('begin_cleanup') == 1
        assert len(flow[1]['deletes']) == 3
        assert runner.artifacts['qa_cleanup']['phase'] == 'complete'
    finally:
        journal.close()


def test_blake2b_full_quota_cannot_select_or_delete_ordinary_testnet4_fixture(flow, tmp_path, monkeypatch):
    from harness import qa_funding
    from harness.state import Failed, private_read
    runner, ordinary, calls, _, journal = admission_for(flow, tmp_path, monkeypatch)
    before = private_read(qa_funding.ROOT/'pool.json')
    try:
        config = dict(ordinary.config, chain='bitcoin-blake2b-testnet4')
        with pytest.raises(Failed, match='CLEANUP_CHAIN'):
            admission = native.NativeCleanupConsumerV1(ordinary.suite, runner, config, runner.account, 5, None, {})
            cleanup.ensure_capacity(runner, admission=admission)
        assert flow[1]['deletes'] == []
        assert private_read(qa_funding.ROOT/'pool.json') == before
        assert calls == []
    finally:
        journal.close()
