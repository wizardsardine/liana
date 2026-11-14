"""Offline native recovery guards. No live fixture or GUI acceptance claimed."""
from copy import deepcopy
import io
import json
from pathlib import Path
from types import SimpleNamespace
import struct
import sys

import pytest
from harness import native_recovery as native, native_recovery_verify as public
from harness.provisioning import FreshRunner
from harness.state import Blocked, Failed


@pytest.fixture
def config(tmp_path):
    return {'version': 1, 'checkout': str(tmp_path/'checkout'), 'revision': 'current'}


@pytest.mark.parametrize('field', ['binary', 'receipt_file', 'seed', 'pin', 'factor', 'original_profile', 'driver'])
def test_external_secret_binary_or_receipt_injection_is_not_a_native_config(config, field):
    config[field] = '/untrusted/input'
    with pytest.raises(Failed): native.options(config)


def test_missing_prerequisite_blocks_before_credentials_or_run_creation(config, tmp_path, monkeypatch):
    def output(argv, **kw):
        if 'get-url' in argv: return b'git@github.com:coincubetech/coincube.git\n'
        if 'status' in argv: return b''
        return b'a'*40+b'\n'
    monkeypatch.setattr(native.subprocess, 'check_output', output)
    monkeypatch.setattr(native.subprocess, 'run', lambda *a, **k: SimpleNamespace(returncode=1))
    with pytest.raises(Blocked, match='prerequisite'): native.NativeRecovery(config, tmp_path/'run', {})
    assert not (tmp_path/'run').exists()


def test_missing_verifier_dependency_blocks_before_helper_or_credentials(config, tmp_path, monkeypatch):
    monkeypatch.setattr(native, 'source', lambda _: {'head': 'a'*40})
    def missing(): raise Blocked('NATIVE_RECOVERY_VERIFY_DEPENDENCIES', 'missing')
    monkeypatch.setattr(native, 'dependencies', missing)
    with pytest.raises(Blocked, match='missing'): native.NativeRecovery(config, tmp_path/'run', {})
    assert not (tmp_path/'run').exists()


def test_environment_preflight_blocks_before_mobile_or_native_fixtures(config, monkeypatch):
    calls = []
    runner = FreshRunner.__new__(FreshRunner); runner.config = {'native_recovery': config}
    runner.core = SimpleNamespace(chain=lambda: calls.append('mobile'))
    def missing(_): raise Blocked('NATIVE_RECOVERY_PR1174', 'missing prerequisite')
    monkeypatch.setattr(native, 'preflight', missing)
    with pytest.raises(Blocked): runner.stage_environment()
    assert calls == []


def test_funding_gate_runs_before_saved_funding_or_sponsor_faucet(config, monkeypatch):
    runner = FreshRunner.__new__(FreshRunner); runner.config = {'native_recovery': config}
    def missing(_): raise Blocked('NATIVE_RECOVERY_PR1174', 'missing prerequisite')
    monkeypatch.setattr(native, 'gate', missing)
    # No artifacts/provision/phone/funding client installed: any use before the
    # native gate would raise instead of the required prerequisite block.
    with pytest.raises(Blocked): runner.stage_funding()


def test_old_config_is_explicitly_legacy_mobile_only():
    evidence = {}
    runner = SimpleNamespace(config={}, journal=SimpleNamespace(artifact=lambda k,v:evidence.update({k:v})))
    native.gate(runner)
    assert evidence == {'recovery_coverage': 'legacy_mobile_only'}


def fixture(chain):
    public.dependencies()
    from bip32 import BIP32
    from bip380.descriptors import Descriptor
    from coincurve import PrivateKey
    from test_framework.serializations import CTransaction, CTxIn, CTxOut, COutPoint, PSBT, PSBTMap, hash160, sighash_all_witness
    from test_btcb2_unified_sighash import unified_sighash
    root = BIP32.from_seed(b'\x17'*32, network='test')
    fp = hash160(root.get_pubkey_from_path([]))[:4].hex()
    path = [0x80000030,0x80000001,0x80000000,0x80000002]
    xpub = root.get_xpub_from_path(path)
    descriptor = f'wsh(or_d(pk([{fp}/48h/1h/0h/2h]{xpub}/<0;1>/*),and_v(v:pk([{fp}/48h/1h/0h/2h]{xpub}/<2;3>/*),older(144))))'
    receive, change = Descriptor.from_str(descriptor).singlepath_descriptors(); receive.derive(0); change.derive(0)
    parent = CTransaction();parent.vin=[CTxIn(COutPoint(0,0xffffffff))];parent.vout=[CTxOut(100000,bytes(receive.script_pubkey))]
    tx=CTransaction();tx.vin=[CTxIn(COutPoint(int(parent.txid().hex(),16),0))];tx.vout=[CTxOut(99738,bytes(change.script_pubkey))]
    origins={root.get_pubkey_from_path(path+[branch,0]):bytes.fromhex(fp)+b''.join(struct.pack('<I',v) for v in path+[branch,0]) for branch in (0,2)}
    psbt=PSBT(g=PSBTMap({0:tx.serialize()}),i=[PSBTMap({0:parent.serialize(),1:parent.vout[0].serialize(),5:bytes(receive.script_sighash),6:origins})],o=[PSBTMap()]);psbt.tx=tx
    message=unified_sighash(tx,0,0x21,1,parent.vout,bytes(receive.script_sighash)) if chain==public.CHAINS[1] else sighash_all_witness(bytes(receive.script_sighash),psbt,0)
    signatures={root.get_pubkey_from_path(path+[branch,0]):PrivateKey(root.get_privkey_from_path(path+[branch,0])).sign(message,hasher=None)+bytes([0x21 if chain==public.CHAINS[1] else 1]) for branch in (0,2)}
    psbt.i[0].map[0xfc if chain==public.CHAINS[1] else 2] = {b'\x08coincube\x00'+k:v for k,v in signatures.items()} if chain==public.CHAINS[1] else signatures
    identity={'cube':{'network':chain},'descriptor':descriptor,'fingerprint':fp}
    return psbt,identity


@pytest.mark.parametrize('chain', public.CHAINS)
def test_independent_unfunded_verifier_requires_exact_primary_and_recovery_signatures(chain):
    psbt,identity=fixture(chain);proof=public.verify(psbt.serialize(),identity,chain)
    assert proof['verified_key_count']==2 and proof['verified_paths']==[[0,0],[2,0]]
    assert proof['sighash']==(1 if chain==public.CHAINS[0] else 0x21)
    assert proof['funded'] is proof['recognized_utxo'] is proof['gui'] is proof['broadcast'] is False


@pytest.mark.parametrize('chain',public.CHAINS)
@pytest.mark.parametrize('change',['chain','fingerprint','path','pubkey','output','utxo','drop_signature','wrong_storage','alter_signature'])
def test_independent_verifier_refuses_bad_domain_origins_outputs_and_signature_sets(chain,change):
    psbt,identity=fixture(chain)
    field=0xfc if chain==public.CHAINS[1] else 2
    if change=='chain': identity['cube']['network']=public.CHAINS[1] if chain==public.CHAINS[0] else public.CHAINS[0]
    elif change=='fingerprint': identity['fingerprint']='00000000'
    elif change=='path':
        key=next(iter(psbt.i[0].map[6]));psbt.i[0].map[6][key]=psbt.i[0].map[6][key][:-4]+struct.pack('<I',99)
    elif change=='pubkey':
        key=next(iter(psbt.i[0].map[6]));psbt.i[0].map[6][b'\x02'+b'\x01'*32]=psbt.i[0].map[6].pop(key)
    elif change=='output': psbt.tx.vout[0].nValue-=1;psbt.g.map[0]=psbt.tx.serialize()
    elif change=='utxo': psbt.i[0].map[1]=psbt.i[0].map[1]+b'\0'
    elif change=='drop_signature': psbt.i[0].map[field].pop(next(iter(psbt.i[0].map[field])))
    elif change=='wrong_storage':
        sigs=psbt.i[0].map.pop(field);psbt.i[0].map[2 if field==0xfc else 0xfc]={k[10:] if field==0xfc else b'\x08coincube\x00'+k:v for k,v in sigs.items()}
    else:
        key=next(iter(psbt.i[0].map[field]));value=psbt.i[0].map[field][key];psbt.i[0].map[field][key]=value[:-2]+bytes([value[-2]^1])+value[-1:]
    with pytest.raises(Failed): public.verify(psbt.serialize(),identity,chain)


@pytest.mark.parametrize('raw',[b'psbt\xff\x01\x00\x01x\x01\x00\x01y\x00', b'psbt\xff\xfd\x01\x00\x00\x01x\x00'])
def test_noncanonical_or_duplicate_psbt_maps_are_refused(raw):
    with pytest.raises(Failed): public.canonical_maps(raw)


def offline_protocol(config, tmp_path, monkeypatch, crash_retirement=False):
    """Unit-only fake worker; never injectable through a live CLI/config."""
    import hashlib
    from harness.state import atomic_write
    binding={'head':'a'*40,'tree':'b'*40,'prerequisite':native.PREREQUISITE}
    executable=tmp_path/'test-only-binary';executable.write_bytes(b'not an executable')
    receipt={'source':binding,'sha256':hashlib.sha256(executable.read_bytes()).hexdigest()}
    monkeypatch.setattr(native,'preflight',lambda _:binding)
    monkeypatch.setattr(native,'source',lambda _:binding)
    monkeypatch.setattr(native,'build',lambda *args:(executable,receipt))
    monkeypatch.setattr(native,'signed_identity',lambda *args:None)
    calls=[]; interrupted=[False]
    def worker(executable,request,directory,access_token=None):
        action=request['action'];cfg=request['config'];calls.append((cfg['chain'],action))
        case=Path(cfg['case_root']);state_path=case/'state.json'
        pid={'refresh_auth':12,'admit_create':9,'preflight':10,'bind_auth':11,'create_export':100,'wipe_original':150,'restore':200,'reopen_sign':300,'retire_remote':400}[action]
        record={'pid':pid,'start_id':str(pid),'exited':True,'returncode':0}
        if action in ('admit_create','refresh_auth','preflight','bind_auth'):return record
        if action=='create_export':
            case.mkdir(mode=0o700);(case/'original').mkdir(mode=0o700)
            psbt,identity=fixture(cfg['chain'])
            state={'config':cfg,'pending':None,'stage':'exported','creator':{'pid':100,'start_id':'100'},'restorer':None,
                   'identity':identity,'original_factor':'old','restored_factor':None,'proof_sha256':None}
        else:state=json.loads(public_state_read(state_path))
        if action=='wipe_original':(case/'original').rename(case/'quarantined-original');state['stage']='wiped'
        if action=='restore':
            (case/'restored').mkdir(mode=0o700);state.update(stage='restored',restorer={'pid':200,'start_id':'200'},restored_factor='new')
        if action=='reopen_sign':
            psbt,identity=fixture(cfg['chain']);raw=psbt.serialize();atomic_write(case/'unfunded-key-control.psbt',raw)
            state.update(stage='signed_unfunded',proof_sha256=hashlib.sha256(raw).hexdigest())
        if action=='retire_remote':state['stage']='retired_unfunded'
        atomic_write(state_path,json.dumps(state).encode())
        if action=='retire_remote' and crash_retirement and not interrupted[0]:
            interrupted[0]=True;raise Blocked('TEST_LOST_RETIREMENT_RESPONSE','unit-only interruption')
        return record
    monkeypatch.setattr(native,'process',worker)
    return calls


def public_state_read(path):
    from harness.state import private_read
    return private_read(path)


def test_offline_orchestration_orders_both_chains_and_retirement_without_gui_claim(config,tmp_path,monkeypatch):
    calls=offline_protocol(config,tmp_path,monkeypatch)
    suite=native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    try:
        result=suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1)
        assert list(result['chains'])==list(public.CHAINS)
        assert result['gui'] is result['funded'] is result['broadcast'] is False
        assert all(result['chains'][c]['verified_key_count']==2 for c in public.CHAINS)
        assert calls==[(c,a) for c in public.CHAINS for a in ('admit_create','preflight','bind_auth','create_export','refresh_auth','wipe_original','refresh_auth','restore','refresh_auth','reopen_sign','refresh_auth','retire_remote')]
        before=list(calls);suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1)
        assert calls==before  # Same consumer resumes retained proof, not a new run.
    finally:suite.close()


def test_lost_retirement_response_reconciles_only_exact_completed_fixture(config,tmp_path,monkeypatch):
    calls=offline_protocol(config,tmp_path,monkeypatch,crash_retirement=True)
    suite=native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    try:
        with pytest.raises(Blocked,match='unit-only'):suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1)
        result=suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1)
        assert result['remote_retirement']=='verified_native_fixture_only'
        assert calls.count((public.CHAINS[0],'create_export'))==1
        assert calls.count((public.CHAINS[0],'retire_remote'))==2
    finally:suite.close()


def test_completed_consumer_refuses_replaced_retained_state(config,tmp_path,monkeypatch):
    calls=offline_protocol(config,tmp_path,monkeypatch)
    suite=native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    try:
        suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1)
        case=next(p for p in suite.directory.iterdir() if p.is_dir())
        from harness.state import atomic_write
        value=json.loads(public_state_read(case/'state.json'));value['untrusted_extra']='replaced'
        atomic_write(case/'state.json',json.dumps(value).encode());before=list(calls)
        with pytest.raises(Failed,match='RETAINED_STATE_CHANGED'):suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1)
        assert calls==before
    finally:suite.close()


def test_replaced_helper_during_public_verification_never_reaches_retirement(config,tmp_path,monkeypatch):
    calls=offline_protocol(config,tmp_path,monkeypatch)
    original=native.verify
    def replace(raw,identity,chain):
        result=original(raw,identity,chain)
        (tmp_path/'test-only-binary').write_bytes(b'replaced after verified signing')
        return result
    monkeypatch.setattr(native,'verify',replace)
    suite=native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    try:
        with pytest.raises(Failed,match='BUILD_CHANGED'):suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1)
        assert not any(action=='retire_remote' for _,action in calls)
    finally:suite.close()


def test_checked_process_revalidates_source_hash_and_identity_before_and_after(config,tmp_path,monkeypatch):
    executable=tmp_path/'fixture';executable.write_bytes(b'owned binary')
    import hashlib
    binding={'head':'a'*40};receipt={'source':binding,'sha256':hashlib.sha256(executable.read_bytes()).hexdigest()}
    checks=[]
    monkeypatch.setattr(native,'source',lambda _:checks.append('source') or binding)
    monkeypatch.setattr(native,'signed_identity',lambda *args:checks.append('identity'))
    monkeypatch.setattr(native,'process',lambda *args,**kwargs:checks.append('process') or {})
    native.checked_process(config,binding,executable,receipt,{},tmp_path)
    assert checks==['source','identity','process','source','identity']
    def changed(*args,**kwargs):executable.write_bytes(b'replaced during process');return {}
    monkeypatch.setattr(native,'process',changed)
    with pytest.raises(Failed,match='BUILD_CHANGED'):native.checked_process(config,binding,executable,receipt,{},tmp_path)


def test_completed_resume_revalidates_signer_identity_instead_of_returning_cached_acceptance(config,tmp_path,monkeypatch):
    calls=offline_protocol(config,tmp_path,monkeypatch)
    suite=native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    try:
        suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1);before=list(calls)
        def rejected(*args):raise Failed('NATIVE_RECOVERY_HELPER_IDENTITY','identity changed')
        monkeypatch.setattr(native,'signed_identity',rejected)
        with pytest.raises(Failed,match='identity changed'):suite.execute({'tokens':{'access_token':'unit-not-a-credential'}},1)
        assert calls==before
    finally:suite.close()


def test_capacity_refusal_precedes_every_secret_producing_action(config, tmp_path, monkeypatch):
    calls = offline_protocol(config, tmp_path, monkeypatch)
    original = native.process
    def worker(executable, request, directory, access_token=None):
        if request['action'] == 'admit_create':
            calls.append((request['config']['chain'], 'admit_create'))
            raise native.NativeActionBlocked('native_capacity_blocked')
        return original(executable, request, directory, access_token=access_token)
    monkeypatch.setattr(native, 'process', worker)
    suite = native.NativeRecovery(config, tmp_path/'run', {'consumer': 'offline-capacity-test'})
    try:
        with pytest.raises(native.NativeActionBlocked):
            suite.execute({'tokens': {'access_token': 'unit-not-a-credential'}}, 1)
        assert calls == [(public.CHAINS[0], 'admit_create')]
        assert 'pending_native_action' not in suite.journal.data['artifacts']
        assert not any(p.is_dir() for p in suite.directory.iterdir())
    finally:
        suite.close()


def test_mobile_cube_creation_gate_precedes_any_provision_call(monkeypatch):
    calls = []
    runner = FreshRunner.__new__(FreshRunner)
    runner.provision = SimpleNamespace(call=lambda _: calls.append('mobile-create'))
    def stop(_):
        calls.append('native-gate')
        raise Blocked('NATIVE_RECOVERY_ACTION', 'capacity blocked')
    monkeypatch.setattr(native, 'gate', stop)
    with pytest.raises(Blocked):
        runner.stage_cube_creation()
    assert calls == ['native-gate']


def test_completed_native_proof_revalidation_needs_no_capacity_or_cleanup_context(config, tmp_path, monkeypatch):
    calls = offline_protocol(config, tmp_path, monkeypatch)
    suite = native.NativeRecovery(config, tmp_path/'run', {'consumer': 'offline-only-test'})
    try:
        suite.execute({'tokens': {'access_token': 'unit-not-a-credential'}}, 1)
        before = list(calls)
        def forbidden(*args, **kwargs):
            pytest.fail('Completed proof must not build cleanup tools or query full capacity')
        monkeypatch.setattr(native, 'NativeCleanupContext', forbidden)
        suite.execute({'tokens': {'access_token': 'unit-not-a-credential'}}, 1, cleanup_runner=object())
        assert calls == before
    finally:
        suite.close()


def test_cleanup_rejects_untyped_noop_admission():
    from harness import qa_cleanup
    runner = SimpleNamespace()
    with pytest.raises(Failed, match='ADMISSION'):
        qa_cleanup.cleanup_admission(runner, SimpleNamespace(runner=runner, observe=lambda: None))


def test_native_legacy_operation_has_immediate_before_after_identity_guards(monkeypatch):
    from harness import qa_cleanup
    calls = []
    runner = SimpleNamespace(observe=lambda: calls.append('identity'))
    admission = native.NativeCleanupConsumerV1.__new__(native.NativeCleanupConsumerV1)
    admission.runner = runner
    admission.observe = lambda: runner.observe()
    def operation(actual, record):
        assert actual is runner and record == {'fixture': 'exact'}
        calls.append('backup')
    qa_cleanup.cleanup_local(admission, operation, runner, {'fixture': 'exact'})
    assert calls == ['identity', 'backup', 'identity']
    calls.clear()
    def replaced():
        calls.append('replacement-refused')
        raise Failed('NATIVE_RECOVERY_CLEANUP_BUILD_CHANGED', 'replaced binary')
    runner.observe = replaced
    with pytest.raises(Failed):
        qa_cleanup.cleanup_local(admission, operation, runner, {'fixture': 'exact'})
    assert calls == ['replacement-refused']


def abandoned_capacity_fixture(config, tmp_path, monkeypatch):
    import hashlib
    import uuid
    from harness.state import atomic_write, digest
    calls = offline_protocol(config, tmp_path, monkeypatch)
    old = tmp_path/'old-capacity-run'; old.mkdir(mode=0o700)
    scope = str(uuid.uuid5(uuid.NAMESPACE_URL, 'native-recovery/' + str(old) + '/testnet4'))
    case = old/scope; case.mkdir(mode=0o700)
    old_config = {'version':1,'case_root':str(case),'run_id':scope,'chain':'testnet4','owner_id':5,
        'source_head':native.LEGACY_CAPACITY_SOURCE,
        'auth_account':hashlib.sha256(f'recovery-native-v1:{case}:{scope}:testnet4:auth'.encode()).hexdigest()}
    archive = tmp_path/'archive'/'RecoveryNative.app'/'Contents'/'MacOS'/'recovery_kit_native'
    archive.parent.mkdir(parents=True); archive.write_bytes(b'offline-reviewed-legacy-artifact')
    monkeypatch.setattr(native, 'LEGACY_CAPACITY_HELPER', hashlib.sha256(archive.read_bytes()).hexdigest())
    cp = {'artifacts':{'build':{'sha256':native.LEGACY_CAPACITY_HELPER,'source':{'head':native.LEGACY_CAPACITY_SOURCE}},
        'testnet4':{'preflight':{},'bind_auth':{}},
        'pending_native_action':{'chain':'testnet4','action':'create_export','config_sha256':digest(old_config)}}}
    atomic_write(old/'checkpoint.json', json.dumps(cp).encode())
    atomic_write(case/'state.json', json.dumps({'config':old_config}).encode())
    suite = native.NativeRecovery(config, tmp_path/'new-capacity-run', {'unit':'offline-handoff'})
    observed = []
    def audit(cfg, binding, executable, receipt, request, directory, access_token=None):
        assert request['action'] == 'audit_capacity_abandonment'
        assert request['old_directory'] == str(old)
        observed.append(request)
        return {'pid':99,'start_id':'offline-only','exited':True,'returncode':0}
    monkeypatch.setattr(native, 'checked_process', audit)
    return suite, old, case, archive, observed


def test_narrow_abandonment_requires_live_observer_and_never_claims_native_acceptance(config,tmp_path,monkeypatch):
    suite, old, case, archive, observed = abandoned_capacity_fixture(config,tmp_path,monkeypatch)
    before = (old/'checkpoint.json').read_bytes(), (case/'state.json').read_bytes()
    try:
        marker = suite.abandon_capacity({'email':'qa@example.invalid','tokens':{'access_token':'offline-only'}},5,old,archive)
        assert len(observed) == 1
        assert marker['native_acceptance'] is False and marker['all_old_material_retained'] is True
        assert before == ((old/'checkpoint.json').read_bytes(), (case/'state.json').read_bytes())
        first = suite.journal.data['artifacts']['capacity_abandonment']
        assert suite.abandon_capacity({'email':'qa@example.invalid','tokens':{'access_token':'offline-only'}},5,old,archive) == marker
        assert suite.journal.data['artifacts']['capacity_abandonment'] == first
    finally:
        suite.close()


@pytest.mark.parametrize('change', ['helper','source','owner','scope','pending'])
def test_unreviewed_or_changed_abandonment_cannot_reach_observer(config,tmp_path,monkeypatch,change):
    from harness.state import atomic_write
    suite, old, case, archive, observed = abandoned_capacity_fixture(config,tmp_path,monkeypatch)
    try:
        if change == 'helper': archive.write_bytes(b'different')
        elif change in ('owner','scope'):
            state = json.loads((case/'state.json').read_bytes())
            state['config']['owner_id' if change == 'owner' else 'run_id'] = 99 if change == 'owner' else 'wrong'
            atomic_write(case/'state.json',json.dumps(state).encode())
        else:
            cp = json.loads((old/'checkpoint.json').read_bytes())
            if change == 'source': cp['artifacts']['build']['source']['head'] = 'b'*40
            else: cp['artifacts']['pending_native_action']['action'] = 'wipe_original'
            atomic_write(old/'checkpoint.json',json.dumps(cp).encode())
        with pytest.raises(Failed):
            suite.abandon_capacity({'email':'qa@example.invalid','tokens':{'access_token':'offline-only'}},5,old,archive)
        assert observed == []
        assert 'capacity_abandonment' not in suite.journal.data['artifacts']
    finally:
        suite.close()


def test_actual_legacy_binary_hash_guard_stops_replacement_before_backup_or_delete(tmp_path, monkeypatch):
    import hashlib
    from harness import qa_cleanup
    from harness.state import atomic_write, digest
    context = native.NativeCleanupContext.__new__(native.NativeCleanupContext)
    binding = {'head':'a'*40}
    context.suite = SimpleNamespace(config={}, binding=binding)
    context.parent = None
    context.config = {}; context.config_binding = digest({})
    context.workstation = tmp_path/'workstation.json'; context.workstation_raw = b'{}'
    atomic_write(context.workstation, context.workstation_raw)
    context.profile = tmp_path/'profile'; (context.profile/'testnet4').mkdir(parents=True)
    context.account = {'email':'qa@example.invalid','tokens':{'access_token':'offline-only'}}
    atomic_write(context.profile/'testnet4/connect.json',json.dumps({'active_email':context.account['email'],'accounts':[context.account]}).encode())
    context.provision_binary = tmp_path/'provision'; context.provision_binary.write_bytes(b'original')
    context.signing_binary = tmp_path/'signing'; context.signing_binary.write_bytes(b'signing')
    context.tools = {'head':binding['head'],'provision_sha256':hashlib.sha256(b'original').hexdigest(),
                     'signing_sha256':hashlib.sha256(b'signing').hexdigest()}
    monkeypatch.setattr(native,'source',lambda _:binding)
    monkeypatch.setattr(native,'signed_identity',lambda *args:None)
    context.suite.config = {'checkout':str(tmp_path)}
    context.observe()  # Real source/hash guard with synthetic files, not keys.
    context.provision_binary.write_bytes(b'replacement')
    admission = native.NativeCleanupConsumerV1.__new__(native.NativeCleanupConsumerV1)
    admission.runner = context
    admission.observe = context.observe
    called = []
    with pytest.raises(Failed, match='CLEANUP_BUILD_CHANGED'):
        qa_cleanup.cleanup_local(admission, lambda *args:called.append('private-backup'), context, {})
    assert called == []


def test_available_blake2b_creation_needs_no_ordinary_testnet4_cleanup_context(config,tmp_path,monkeypatch):
    import shutil
    import uuid
    calls = offline_protocol(config,tmp_path,monkeypatch)
    suite = native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    account = {'tokens':{'access_token':'unit-not-a-credential'}}
    try:
        suite.execute(account,1)
        chain = public.CHAINS[1]
        scope = str(uuid.uuid5(uuid.NAMESPACE_URL,'native-recovery/'+str(suite.directory.resolve())+'/'+chain))
        # Recreate only this test's synthetic external worker state to exercise
        # the next chain independently. This is not a live resume mechanism.
        shutil.rmtree(suite.directory/scope)
        del suite.journal.data['artifacts'][chain]
        suite.journal.save(); calls.clear()
        def forbidden(*args,**kwargs):
            pytest.fail('B2 available capacity must not construct ordinary-T4 cleanup admission')
        monkeypatch.setattr(native,'NativeCleanupContext',forbidden)
        suite.execute(account,1,cleanup_runner=object())
        assert calls == [(chain,a) for a in ('admit_create','preflight','bind_auth','create_export','refresh_auth','wipe_original','refresh_auth','restore','refresh_auth','reopen_sign','refresh_auth','retire_remote')]
    finally:
        suite.close()


def test_delayed_retirement_refreshes_current_token_without_resetting_completed_actions(config,tmp_path,monkeypatch):
    calls=offline_protocol(config,tmp_path,monkeypatch,crash_retirement=True)
    original=native.process;tokens=[]
    def worker(executable,request,directory,access_token=None):
        if request['action']=='refresh_auth':tokens.append(access_token)
        return original(executable,request,directory,access_token=access_token)
    monkeypatch.setattr(native,'process',worker)
    suite=native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    try:
        with pytest.raises(Blocked):suite.execute({'tokens':{'access_token':'old-unit-token'}},1)
        saved=deepcopy(suite.journal.data['artifacts']['testnet4']['bind_auth'])
        before=list(calls);tokens.clear()
        suite.execute({'tokens':{'access_token':'current-unit-token'}},1)
        assert tokens and all(token=='current-unit-token' for token in tokens)
        assert suite.journal.data['artifacts']['testnet4']['bind_auth']==saved
        assert ('testnet4','create_export') not in calls[len(before):]
        assert ('testnet4','refresh_auth') in calls[len(before):]
    finally:suite.close()


def test_failed_child_identity_is_durable_before_wait_and_never_contains_token(tmp_path):
    import os
    if os.name!='posix':pytest.skip('Native process supervision is POSIX-only')
    executable=tmp_path/'offline-child'
    executable.write_text('#!/usr/bin/env python3\nimport sys,json,pathlib\njson.load(sys.stdin)\nroot=pathlib.Path('+repr(str(tmp_path))+')\nrows=list((root/"native-processes").glob("*.json"))\nassert rows and json.loads(rows[0].read_text())["payload_sent"]\nprint(json.dumps({"ok":False,"code":"native_unit_failure"}))\nsys.exit(2)\n')
    executable.chmod(0o700)
    with pytest.raises(native.NativeActionBlocked):
        native.process(executable,{'action':'refresh_auth','config':{'unit':'offline-only'}},tmp_path,access_token='unit-token-must-not-persist')
    rows=list((tmp_path/'native-processes').glob('*.json'));assert len(rows)==1
    raw=rows[0].read_text();identity=json.loads(raw)
    assert identity['pid']>0 and identity['start_id'] and identity['exited'] is True and identity['returncode']==2
    assert identity['code']=='native_unit_failure' and 'unit-token-must-not-persist' not in raw
    assert rows[0].stat().st_mode&0o077==0


def test_invalid_process_directory_never_starts_child(tmp_path,monkeypatch):
    (tmp_path/'native-processes').mkdir(mode=0o755)
    (tmp_path/'native-processes').chmod(0o755)
    children=[]
    monkeypatch.setattr(native.subprocess,'Popen',lambda *args,**kwargs:children.append(args))
    with pytest.raises(Blocked,match='PROCESS_DIRECTORY'):
        native.process(tmp_path/'unused',{'action':'reopen_sign','config':{}},tmp_path)
    assert children==[]


def test_initial_identity_write_failure_reaps_child_without_action_payload(tmp_path,monkeypatch):
    import os
    if os.name!='posix':pytest.skip('Native process supervision is POSIX-only')
    executable=tmp_path/'offline-waiting-child'
    marker=tmp_path/'payload-received'
    executable.write_text('#!/usr/bin/env python3\nimport sys,pathlib\npayload=sys.stdin.read()\nif payload:pathlib.Path('+repr(str(marker))+').write_text("action")\n')
    executable.chmod(0o700)
    popen=native.subprocess.Popen;children=[]
    def capture(*args,**kwargs):
        child=popen(*args,**kwargs);children.append(child);return child
    def fail_write(*args,**kwargs):raise OSError('unit identity write failure')
    monkeypatch.setattr(native.subprocess,'Popen',capture)
    monkeypatch.setattr(native,'atomic_write',fail_write)
    with pytest.raises(Blocked,match='Native process unavailable'):
        native.process(executable,{'action':'reopen_sign','config':{}},tmp_path)
    assert len(children)==1 and children[0].poll() is not None
    with pytest.raises(ChildProcessError):os.waitpid(children[0].pid,os.WNOHANG)
    assert not marker.exists()


@pytest.mark.parametrize('target',['wipe_original','restore','reopen_sign','retire_remote'])
def test_auth_refresh_failure_does_not_create_lifecycle_intent_and_retry_runs_once(config,tmp_path,monkeypatch,target):
    calls=offline_protocol(config,tmp_path,monkeypatch)
    original=native.process;failure=[True];suite=None
    def worker(executable,request,directory,access_token=None):
        if request['action']=='refresh_auth' and request['config']['chain']=='testnet4' and failure[0]:
            records=suite.journal.data['artifacts']['testnet4']
            next_action=next((action for action in (*native.ACTIONS,'retire_remote') if action not in records),None)
            if next_action==target:
                failure[0]=False
                raise Blocked('UNIT_AUTH_FAILURE','unit auth refresh failed')
        return original(executable,request,directory,access_token=access_token)
    monkeypatch.setattr(native,'process',worker)
    suite=native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    try:
        with pytest.raises(Blocked,match='unit auth refresh failed'):
            suite.execute({'tokens':{'access_token':'expired-unit-token'}},1)
        assert 'pending_native_action' not in suite.journal.data['artifacts']
        assert ('testnet4',target) not in calls
        suite.execute({'tokens':{'access_token':'current-unit-token'}},1)
        assert calls.count(('testnet4',target))==1
    finally:suite.close()


def test_refresh_failure_preserves_real_pending_retirement(config,tmp_path,monkeypatch):
    offline_protocol(config,tmp_path,monkeypatch,crash_retirement=True)
    suite=native.NativeRecovery(config,tmp_path/'run',{'consumer':'offline-only-test'})
    account={'tokens':{'access_token':'unit-token'}}
    try:
        with pytest.raises(Blocked):suite.execute(account,1)
        pending=deepcopy(suite.journal.data['artifacts']['pending_native_action'])
        original=native.process
        def fail_refresh(executable,request,directory,access_token=None):
            if request['action']=='refresh_auth':raise Blocked('UNIT_AUTH_FAILURE','unit auth refresh failed')
            return original(executable,request,directory,access_token=access_token)
        monkeypatch.setattr(native,'process',fail_refresh)
        with pytest.raises(Blocked,match='unit auth refresh failed'):suite.execute(account,1)
        assert suite.journal.data['artifacts']['pending_native_action']==pending
        monkeypatch.setattr(native,'process',original)
        suite.execute(account,1)
        assert 'pending_native_action' not in suite.journal.data['artifacts']
    finally:suite.close()


@pytest.mark.parametrize('failure',[False,True])
def test_owned_awake_guard_is_finite_parent_bound_and_reaped(tmp_path,monkeypatch,failure):
    monkeypatch.setattr(native.sys,'platform','darwin')
    calls=[]
    class Guard:
        def poll(self):return None
        def terminate(self):calls.append('terminate')
        def wait(self,timeout=None):calls.append(('wait',timeout))
    def launch(argv,**kwargs):
        calls.append((argv,kwargs));return Guard()
    monkeypatch.setattr(native.subprocess,'Popen',launch)
    try:
        with native.keep_awake():
            if failure:raise ValueError('unit suite failure')
    except ValueError:
        assert failure
    assert calls[0][0]==['/usr/bin/caffeinate','-diu','-t','7200','-w',str(native.os.getpid())]
    assert calls[0][1]==dict(stdin=native.subprocess.DEVNULL,stdout=native.subprocess.DEVNULL,stderr=native.subprocess.DEVNULL,start_new_session=True)
    assert calls[1:]==['terminate',('wait',5)]


def test_owned_awake_startup_failure_blocks_before_suite(monkeypatch):
    monkeypatch.setattr(native.sys,'platform','darwin')
    def unavailable(*args,**kwargs):raise OSError('unit failure')
    monkeypatch.setattr(native.subprocess,'Popen',unavailable)
    with pytest.raises(Blocked):
        with native.keep_awake():pytest.fail('must not enter suite')


@pytest.mark.parametrize('action,expected',[('wipe_original',1380),('retire_remote',1380),('retire_orphan_215',1380),('restore',660),('reopen_sign',660)])
def test_extended_timeout_is_limited_to_oracle_actions(tmp_path,monkeypatch,action,expected):
    observed=[]
    class Child:
        pid=777
        returncode=0
        def communicate(self,payload,timeout):
            observed.append(timeout)
            return json.dumps({'ok':True,'provenance':'native_product_integration','funded':False,'broadcast':False}).encode(),None
    monkeypatch.setattr(native.subprocess,'Popen',lambda *args,**kwargs:Child())
    monkeypatch.setattr(native.subprocess,'check_output',lambda *args,**kwargs:b'unit-kernel-start')
    native.process(tmp_path/'offline',{'action':action,'config':{}},tmp_path)
    assert observed==[expected]
