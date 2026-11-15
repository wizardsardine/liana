"""Offline exact215 remote-only orchestration; never native lifecycle acceptance."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace
import uuid

import pytest
from test_qa_cleanup import flow
from harness import native_recovery as native, qa_cleanup, qa_funding
from harness.state import Failed, atomic_write, digest, private_read


def orphan_run(flow,tmp_path,monkeypatch):
    funding_runner,cloud,record=flow
    account={'email':'qa@example.invalid','tokens':{'access_token':'current-unit-token'}}
    account_id=digest({'email':account['email'],'user_id':5})
    old=tmp_path/'old-native';old.mkdir(mode=0o700)
    scope=str(uuid.uuid5(uuid.NAMESPACE_URL,'native-recovery/'+str(old)+'/testnet4'))
    case=old/scope;case.mkdir(mode=0o700)
    config={'version':1,'case_root':str(case),'run_id':scope,'chain':'testnet4','owner_id':5,'source_head':native.ORPHAN_SOURCE,
            'auth_account':hashlib.sha256(f'recovery-native-v1:{case}:{scope}:testnet4:auth'.encode()).hexdigest()}
    state={'config':config,'identity':{'remote_cube_id':12},'stage':'exported','pending':'wipe_original'}
    archive=tmp_path/'archive/RecoveryNative.app/Contents/MacOS/recovery_kit_native'
    archive.parent.mkdir(parents=True);archive.write_bytes(b'offline-old-helper')
    monkeypatch.setattr(native,'ORPHAN_HELPER',hashlib.sha256(archive.read_bytes()).hexdigest())
    cp={'artifacts':{'build':{'source':{'head':native.ORPHAN_SOURCE},'sha256':native.ORPHAN_HELPER},
        'testnet4':{'preflight':{},'bind_auth':{},'create_export':{}},'account_identity':account_id,
        'pending_native_action':{'chain':'testnet4','action':'wipe_original','config_sha256':digest(config)}}}
    atomic_write(old/'checkpoint.json',json.dumps(cp).encode());atomic_write(case/'state.json',json.dumps(state).encode())
    binding={'head':'a'*40};executable=tmp_path/'current-helper';executable.write_bytes(b'offline-current-helper')
    receipt={'source':binding,'sha256':hashlib.sha256(executable.read_bytes()).hexdigest()}
    cfg={'version':1,'checkout':str(tmp_path),'revision':'current'}
    monkeypatch.setattr(native,'preflight',lambda _:binding)
    monkeypatch.setattr(native,'source',lambda _:binding)
    monkeypatch.setattr(native,'build',lambda *args:(executable,receipt))
    monkeypatch.setattr(native,'signed_identity',lambda *args:None)
    monkeypatch.setattr(native.subprocess,'run',lambda *args,**kwargs:SimpleNamespace(returncode=0))
    context=SimpleNamespace(config=deepcopy(funding_runner.config),artifacts={'account_identity':account_id},account_user_id=5,
                            verify_protection_view=lambda:None)
    monkeypatch.setattr(native,'NativeCleanupContext',lambda *args,**kwargs:context)
    calls=[]
    def worker(cfg,binding,exe,receipt,request,directory,access_token=None):
        assert request['action']=='retire_orphan_215' and access_token=='current-unit-token'
        calls.append(request)
        root=Path(request['config']['case_root']);root.mkdir(mode=0o700)
        atomic_write(root/'orphan.json',json.dumps({'config':request['config'],'phase':'complete','remote_id':12,
            'old_checkpoint_sha256':request['old_checkpoint_sha256'],'old_state_sha256':request['old_state_sha256']}).encode())
        return {'pid':17,'start_id':'offline','exited':True,'returncode':0}
    monkeypatch.setattr(native,'checked_process',worker)
    suite=native.NativeRecovery(cfg,tmp_path/'new-native',{'unit':'offline-only'})
    return suite,account,old,case,archive,context,calls,record


@pytest.mark.parametrize('protection',['preserve','source','claim','external'])
def test_actual_configured_and_global_funding_protection_precedes_any_orphan_delete(flow,tmp_path,monkeypatch,protection):
    suite,account,old,case,archive,context,calls,record=orphan_run(flow,tmp_path,monkeypatch)
    try:
        if protection=='preserve':context.config['qa_cleanup']['preserve_runs']=[record['snapshot']]
        elif protection=='source':context.config['qa_funding']={'version':1,'sources':[{'source_run':record['snapshot']}]}
        else:
            with qa_funding.pool() as data:
                if protection=='claim':
                    data['claims']['unit']={'phase':'planned','target_key':'unit-target','entry':{'source_run':record['snapshot']},'identity':{'artifacts':{'cube_id':12}}}
                    data['runs']['unit-target']='unit'
                else:data['external']['unit']={'identity':{'artifacts':{'cube_id':12}}}
                qa_funding.save_pool(data)
        before=private_read(qa_funding.ROOT/'pool.json')
        with pytest.raises(Failed,match='ORPHAN_PROTECTED'):
            suite.retire_orphan_215(account,5,old,archive,'/unit/workstation')
        assert calls==[] and flow[1]['deletes']==[]
        assert private_read(qa_funding.ROOT/'pool.json')==before
    finally:suite.close()


def test_remote_only_outcome_keeps_old_pending_and_cannot_count_as_lifecycle_acceptance(flow,tmp_path,monkeypatch):
    suite,account,old,case,archive,context,calls,record=orphan_run(flow,tmp_path,monkeypatch)
    before=private_read(old/'checkpoint.json'),private_read(case/'state.json')
    try:
        outcome=suite.retire_orphan_215(account,5,old,archive,'/unit/workstation')
        assert len(calls)==1 and outcome['remote_metadata_retired'] is True
        assert outcome['old_flow']=='BLOCKED' and outcome['native_acceptance'] is False
        assert outcome['local_recovery_retained'] is True
        assert before==(private_read(old/'checkpoint.json'),private_read(case/'state.json'))
        assert 'native_recovery_acceptance' not in suite.journal.data['artifacts']
        assert json.loads(before[0])['artifacts']['pending_native_action']['action']=='wipe_original'
    finally:suite.close()
