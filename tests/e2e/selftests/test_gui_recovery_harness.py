"""GUI coordinator and public signature guard tests. No GUI acceptance is claimed."""
from copy import deepcopy
import base64
import hashlib
import json
from pathlib import Path
import struct
from types import SimpleNamespace

import pytest

from harness import gui_recovery as gui, gui_verify
from harness.state import Blocked, Failed, atomic_write, digest, private_read


@pytest.fixture
def config(tmp_path):
    return {"version": 1, "checkout": str(tmp_path / "checkout"), "revision": "a" * 40,
            "app_receipt": str(tmp_path / "app-receipt.json"), "driver": {"kind": "cua_repl", "version": 1},
            "developer_mode": True, "maximum_fee_sats": 1000,
            "funding": {chain: {"facility": "approved_disposable_qa", "chain": chain, "maximum_sats": 1000000} for chain in gui.CHAINS}}


@pytest.mark.parametrize('bad', ['driver_command', 'receipt_file', 'native_helper', 'allow_mock'])
def test_config_rejects_alternate_driver_and_receipt_import_paths(config, bad):
    config[bad] = '/untrusted/pretend-gui'
    with pytest.raises(Failed): gui.validate_config(config)


def test_live_cua_adapter_is_explicitly_unavailable_not_a_success_receipt():
    worker = gui.CuaWorker()
    with pytest.raises(Blocked) as error: worker.connect({'nonce': 'fresh'})
    assert error.value.code == 'GUI_RECOVERY_CUA_UNAVAILABLE'


def test_live_worker_override_cannot_turn_offline_test_into_acceptance(config, tmp_path):
    with pytest.raises(Failed, match='DRIVER_OVERRIDE'): gui.GuiRecovery(config, tmp_path / 'run', worker=object())


def test_missing_prerequisite_blocks_before_any_worker_or_profile(config, tmp_path, monkeypatch):
    monkeypatch.setattr(gui.subprocess, 'check_output', lambda argv, **kwargs:
        b'git@github.com:coincubetech/coincube.git\n' if 'get-url' in argv else (config['revision']+'\n').encode())
    monkeypatch.setattr(gui.subprocess, 'run', lambda *a, **k: SimpleNamespace(returncode=1))
    runner = gui.GuiRecovery(config, tmp_path / 'run')
    try:
        with pytest.raises(Blocked, match='PR #1174'): runner.run(gui.CHAINS[0], 'preflight')
        assert not (runner.root / gui.CHAINS[0]).exists()
    finally: runner.close()


@pytest.mark.parametrize('chain', gui.CHAINS)
def test_wrong_chain_funding_policy_cannot_be_reused(config, chain):
    config['funding'][chain]['chain'] = 'testnet4' if chain != 'testnet4' else 'bitcoin-blake2b-testnet4'
    with pytest.raises(Failed, match='FUNDING_CONFIG'): gui.validate_config(config)


def test_stage_order_requires_actual_prior_stages(config, tmp_path):
    runner = gui.GuiRecovery(config, tmp_path / 'run', worker=object(), offline=True)
    try:
        with pytest.raises(Failed, match='STAGE_ORDER'): runner.run(gui.CHAINS[0], 'sign_export')
    finally: runner.close()


def test_canonical_psbt_rejects_duplicate_keys_and_nonminimal_sizes():
    with pytest.raises(Failed, match='DUPLICATE_KEY'):
        gui_verify.canonical_maps(b'psbt\xff\x01\x00\x01x\x01\x00\x01y\x00')
    with pytest.raises(Failed, match='COMPACT_SIZE'):
        gui_verify.canonical_maps(b'psbt\xff\xfd\x01\x00\x00\x01x\x00')


def signature_case(chain):
    # Existing independent test-only reference dependencies; this never drives GUI.
    gui_verify.references()
    from bip32 import BIP32
    from bip380.descriptors import Descriptor
    from coincurve import PrivateKey
    from test_framework.serializations import CTransaction, CTxIn, CTxOut, COutPoint, PSBT, PSBTMap, hash160, sighash_all_witness
    from test_btcb2_unified_sighash import unified_sighash
    hot = BIP32.from_seed(b'\x01' * 32, network='test')
    recovery = BIP32.from_seed(b'\x02' * 32, network='test')
    fp = hash160(hot.get_pubkey_from_path([]))[:4].hex()
    rf = hash160(recovery.get_pubkey_from_path([]))[:4].hex()
    account = [0x80000030, 0x80000001, 0x80000000, 0x80000002]
    xpub, rxpub = hot.get_xpub_from_path(account), recovery.get_xpub_from_path(account)
    text = f"wsh(or_d(pk([{fp}/48h/1h/0h/2h]{xpub}/<0;1>/*),and_v(v:pk([{rf}/48h/1h/0h/2h]{rxpub}/<2;3>/*),older(144))))"
    desc = Descriptor.from_str(text); receive, change = desc.singlepath_descriptors()
    receive.derive(1); change.derive(1)
    parent = CTransaction(); parent.vin = [CTxIn(COutPoint(0, 0xffffffff))]
    parent.vout = [CTxOut(100000, bytes(receive.script_pubkey))]
    tx = CTransaction(); tx.vin = [CTxIn(COutPoint(int(parent.txid().hex(), 16), 0))]
    tx.vout = [CTxOut(99738, bytes(change.script_pubkey))]
    pubkey = hot.get_pubkey_from_path(account + [0, 1])
    path = bytes.fromhex(fp) + b''.join(struct.pack('<I', x) for x in account + [0, 1])
    psbt = PSBT(g=PSBTMap({0: tx.serialize()}), i=[PSBTMap({0: parent.serialize(), 1: parent.vout[0].serialize(),
        5: bytes(receive.script_sighash), 6: {pubkey: path}})], o=[PSBTMap()]); psbt.tx = tx
    unified = chain == gui.CHAINS[1]
    expected = 0x21 if unified else 1
    message = unified_sighash(tx, 0, 0x21, 1, parent.vout, bytes(receive.script_sighash)) if unified else sighash_all_witness(bytes(receive.script_sighash), psbt, 0)
    signature = PrivateKey(hot.get_privkey_from_path(account + [0, 1])).sign(message, hasher=None) + bytes([expected])
    if unified: psbt.i[0].map[0xfc] = {b'\x08coincube\x00' + pubkey: signature}
    else: psbt.i[0].map[2] = {pubkey: signature}
    manifest = {'chain': chain, 'root_fingerprint': fp, 'descriptor': text, 'receive_index': 1, 'change_index': 1,
                'broadcast': False, 'maximum_fee_sats': 1000,
                'funding': {'chain': chain, 'wallet_recognized': True, 'txid': parent.txid().hex(), 'vout': 0,
                            'value_sats': 100000, 'raw_tx': parent.serialize().hex()}}
    return psbt, manifest


@pytest.mark.parametrize('chain', gui.CHAINS)
def test_independent_public_verifier_checks_chain_specific_signature(chain):
    psbt, manifest = signature_case(chain)
    proof = gui_verify.verify_export(psbt.serialize(), manifest)
    assert proof['sighash'] == (0x21 if chain == gui.CHAINS[1] else 1)
    assert proof['broadcast'] is False
    assert proof['fee_sats'] == 262
    assert 'GUI provenance is required separately' in proof['scope']


@pytest.mark.parametrize('chain', gui.CHAINS)
@pytest.mark.parametrize('change', ['chain', 'origin', 'descriptor', 'funding', 'fee', 'sighash', 'signature', 'output'])
def test_independent_verifier_refuses_wrong_domain_key_coin_or_export(chain, change):
    psbt, manifest = signature_case(chain)
    if change == 'chain': manifest['chain'] = gui.CHAINS[1] if chain == gui.CHAINS[0] else gui.CHAINS[0]
    elif change == 'origin': manifest['root_fingerprint'] = 'ffffffff'
    elif change == 'descriptor': manifest['descriptor'] = manifest['descriptor'].replace('older(144)', 'older(145)')
    elif change == 'funding': manifest['funding']['wallet_recognized'] = False
    elif change == 'fee': manifest['maximum_fee_sats'] = 1
    elif change == 'output':
        psbt.tx.vout[0].scriptPubKey = b'\x00\x14' + b'\x00' * 20
        psbt.g.map[0] = psbt.tx.serialize()
    else:
        key = 0xfc if chain == gui.CHAINS[1] else 2
        pub = next(iter(psbt.i[0].map[key])); sig = psbt.i[0].map[key][pub]
        psbt.i[0].map[key][pub] = sig[:-1] + bytes([0x01 if sig[-1] == 0x21 else 0x21]) if change == 'sighash' else sig[:-2] + bytes([sig[-2] ^ 1]) + sig[-1:]
    serialized = psbt.serialize()
    with pytest.raises(Failed): gui_verify.verify_export(serialized, manifest)


class OfflineWorker:
    """Explicit fake for protocol tests; never selectable by the live CLI."""
    def __init__(self, mutate=None):
        self.mutate = mutate; self.actions = []; self.cases = {}

    def connect(self, challenge):
        self.scope = challenge['scope']; self.app = 'a' * 64
        return {'challenge_sha256': digest(challenge), 'driver': 'cua_repl', 'real_gui': True,
                'scope': self.scope, 'session': 'offline-test-session', 'revision': challenge['revision'],
                'secret_store': 'macos_data_protection_keychain', 'no_broadcast': True,
                'approved_funding_chains': list(gui.CHAINS), 'app_sha256': self.app,
                'cache_isolation': 'fixture_device_secret_quarantine_v1'}

    def perform(self, request):
        self.actions.append(deepcopy(request))
        chain, stage = request['chain'], request['stage']; root = Path(request['case_directory'])
        if chain not in self.cases:
            psbt, manifest = signature_case(chain)
            identity = {'cube_id': str(__import__('uuid').uuid5(__import__('uuid').NAMESPACE_URL, self.scope + chain)),
                'root_fingerprint': manifest['root_fingerprint'], 'descriptor': manifest['descriptor'], 'chain': chain}
            self.cases[chain] = psbt, manifest, identity
        psbt, manifest, identity = self.cases[chain]
        def proc(phase):
            return {'pid': {'original': 101, 'restore': 102, 'restart': 103}[phase], 'start_id': phase,
                    'profile': request['original_profile'] if phase == 'original' else request['restored_profile'], 'app_sha256': self.app}
        reply = {'request_sha256': digest(request), 'scope': self.scope, 'session': request['session'],
            'chain': chain, 'stage': stage, 'nonce': request['nonce'], 'broadcast': False, 'evidence_source': 'live_cua',
            'gui_action': True, 'sensitive_output_logged': False, 'screenshots_of_secrets': False,
            'observation_sha256': 'b' * 64}
        if stage == 'create':
            Path(request['original_profile']).mkdir(mode=0o700)
            atomic_write(root / 'original-identity.json', json.dumps(identity).encode())
            reply.update(developer_mode=True, cube_open=True, vault_open=True, process=proc('original'), credentials_saved=True, credential_namespace=self.scope)
        elif stage == 'mode_parity': reply.update(off_creation_hidden=True, off_identity=digest(identity), on_identity=digest(identity), developer_mode_restored_on=True, gate_bypassed=False)
        elif stage == 'backup':
            kit = {'encrypted_seed': base64.b64encode(b'\x02\x01' + b'\x03' * 80).decode(),
                   'encrypted_descriptor': base64.b64encode(b'\x02\x01' + b'\x04' * 80).decode(), 'cube_id': identity['cube_id'], 'chain': chain}
            atomic_write(root / 'encrypted-kit.json', json.dumps(kit).encode())
            reply.update(product_flow='full_encrypted_recovery_kit', has_encrypted_seed=True, has_encrypted_descriptor=True, identity_sha256=digest(identity))
        elif stage == 'exit_original': reply.update(process=proc('original'), exited_normally=True, process_absent=True, hot_cache_absent=True)
        elif stage == 'exclude_original_cache':
            kit_sha = digest(json.loads(private_read(root / 'encrypted-kit.json')))
            proof = {'version': 1, 'scope': self.scope, 'nonce': request['nonce'],
                     'method': 'fixture_device_secret_quarantine_v1', 'service': gui.CACHE_SERVICE,
                     'account': identity['cube_id'], 'original_record_absent': True, 'other_records_unchanged': True,
                     'identity_sha256': digest(identity), 'full_kit_sha256': kit_sha, 'original_process': proc('original'),
                     'quarantine_locator_sha256': 'c' * 64}
            raw = json.dumps(proof).encode(); atomic_write(root / 'keychain-isolation.json', raw)
            reply.update(method=proof['method'], service=gui.CACHE_SERVICE, account=identity['cube_id'],
                         original_record_absent=True, other_records_unchanged=True, fixture_owned=True, funded=False,
                         full_kit_sha256=kit_sha, isolation_receipt_sha256=hashlib.sha256(raw).hexdigest())
        elif stage == 'restore':
            Path(request['restored_profile']).mkdir(mode=0o700)
            atomic_write(root / 'restored-identity.json', json.dumps(identity).encode())
            reply.update(new_pin_distinct=True, full_kit_sha256=digest(json.loads(private_read(root / 'encrypted-kit.json'))), cube_open=True, vault_open=True, original_process_absent=True, process=proc('restore'))
        elif stage == 'restart': reply.update(process=proc('restart'), previous_process_absent=True, exited_normally=True, locked_cube_visible=True)
        elif stage == 'wrong_pin': reply.update(wrong_pin_refused=True, cube_open=False, fixture_unchanged=True, process=proc('restart'))
        elif stage == 'unlock':
            atomic_write(root / 'reopened-identity.json', json.dumps(identity).encode())
            reply.update(process=proc('restart'), new_pin_used=True, cube_open=True, vault_open=True)
        elif stage == 'funding':
            funding = {**manifest['funding'], 'chain_oracle_verified': True}
            atomic_write(root / 'funding-proof.json', json.dumps(funding).encode())
            reply.update(process=proc('restart'), wallet_recognized=True, funding_facility='approved_disposable_qa', chain_oracle_verified=True, injected_utxo=False)
        elif stage == 'sign_export':
            atomic_write(root / 'gui-signed.psbt', psbt.serialize())
            reply.update(process=proc('restart'), gui_sign_clicked=True, ready_visible=True, self_transfer=True, native_save_dialog=True, broadcast_clicked=False, receive_index=1, change_index=1)
        elif stage == 'retain': reply.update(all_owned_processes_exited=True, profiles_retained=True, funded_roots_retained=True, personal_keys_untouched=True, credential_namespace=self.scope)
        if self.mutate: self.mutate(request, reply)
        return reply


def test_offline_protocol_complete_journal_cannot_claim_live_gui(config, tmp_path):
    worker = OfflineWorker(); runner = gui.GuiRecovery(config, tmp_path / 'run', worker=worker, offline=True)
    try:
        for chain in gui.CHAINS:
            for stage in gui.STAGES: runner.run(chain, stage)
            assert runner.state(chain)['acceptance']['kind'] == 'offline_selftest'
            assert runner.state(chain)['acceptance']['broadcast'] is False
            assert (runner.directory(chain) / 'original').is_dir()
            assert (runner.directory(chain) / 'restored').is_dir()
        assert {v['status'] for v in runner.journal.data['stages'].values()} == {'offline_passed'}
    finally: runner.close()


@pytest.mark.parametrize('field', ['nonce', 'scope', 'session', 'request_sha256', 'chain'])
def test_gui_actions_reject_stale_cross_run_or_chain_receipts(config, tmp_path, field):
    def mutate(request, reply): reply[field] = 'stale-or-foreign'
    worker = OfflineWorker(mutate); runner = gui.GuiRecovery(config, tmp_path / 'run', worker=worker, offline=True)
    try:
        runner.run(gui.CHAINS[0], 'preflight')
        with pytest.raises(Failed, match='ACTION_BINDING'): runner.run(gui.CHAINS[0], 'create')
        assert runner.journal.data['artifacts']['pending_action']['request']['nonce'] == worker.actions[0]['nonce']
    finally: runner.close()


@pytest.mark.parametrize('stage,field,value', [('restore', 'new_pin_distinct', False), ('restore', 'original_process_absent', False),
    ('wrong_pin', 'wrong_pin_refused', False), ('wrong_pin', 'fixture_unchanged', False), ('funding', 'injected_utxo', True),
    ('funding', 'chain_oracle_verified', False), ('sign_export', 'gui_sign_clicked', False), ('sign_export', 'broadcast_clicked', True),
    ('exclude_original_cache', 'original_record_absent', False), ('exclude_original_cache', 'service', 'foreign-service'),
    ('exclude_original_cache', 'account', 'foreign-cube'), ('exclude_original_cache', 'other_records_unchanged', False),
    ('backup', 'has_encrypted_descriptor', False), ('backup', 'product_flow', 'mnemonic_only'), ('mode_parity', 'gate_bypassed', True)])
def test_gui_protocol_refuses_partial_native_synthetic_or_unsafe_evidence(config, tmp_path, stage, field, value):
    def mutate(request, reply):
        if request['stage'] == stage: reply[field] = value
    worker = OfflineWorker(mutate); runner = gui.GuiRecovery(config, tmp_path / 'run', worker=worker, offline=True)
    try:
        for current in gui.STAGES[:gui.STAGES.index(stage)]: runner.run(gui.CHAINS[0], current)
        with pytest.raises(Failed): runner.run(gui.CHAINS[0], stage)
        assert 'acceptance' not in runner.state(gui.CHAINS[0])
    finally: runner.close()


def test_no_secret_fields_are_accepted_or_checkpointed(config, tmp_path):
    def mutate(request, reply): reply['pin'] = 'not-a-real-secret'
    runner = gui.GuiRecovery(config, tmp_path / 'run', worker=OfflineWorker(mutate), offline=True)
    try:
        runner.run(gui.CHAINS[0], 'preflight')
        with pytest.raises(Failed, match='UNEXPECTED_REPLY_FIELDS'): runner.run(gui.CHAINS[0], 'create')
        assert b'not-a-real-secret' not in private_read(runner.journal.path)
    finally: runner.close()


def test_uncertain_gui_action_is_not_replayed_with_a_new_nonce(config, tmp_path):
    worker = OfflineWorker(); runner = gui.GuiRecovery(config, tmp_path / 'run', worker=worker, offline=True)
    try:
        runner.run(gui.CHAINS[0], 'preflight')
        runner.journal.artifact('pending_action', {'key': 'old-action', 'request': {'nonce': 'old-nonce'}})
        with pytest.raises(Blocked, match='uncertain outcome'): runner.run(gui.CHAINS[0], 'create')
        assert worker.actions == []
    finally: runner.close()


def test_new_profile_does_not_establish_keychain_cache_isolation(config, tmp_path):
    class ProfileOnlyWorker(OfflineWorker):
        def connect(self, challenge):
            value = super().connect(challenge); value['cache_isolation'] = 'profile_scoped_store'
            return value
    runner = gui.GuiRecovery(config, tmp_path / 'run', worker=ProfileOnlyWorker(), offline=True)
    try:
        with pytest.raises(Blocked, match='Cube-UUID Keychain'): runner.run(gui.CHAINS[0], 'preflight')
        assert not (runner.root / gui.CHAINS[0] / 'original').exists()
    finally: runner.close()


@pytest.mark.parametrize('chain', gui.CHAINS)
def test_valid_replacement_export_cannot_inherit_original_gui_provenance(config, tmp_path, chain):
    worker = OfflineWorker(); runner = gui.GuiRecovery(config, tmp_path / 'run', worker=worker, offline=True)
    try:
        for stage in gui.STAGES[:gui.STAGES.index('verify')]: runner.run(chain, stage)
        replacement, manifest = signature_case(chain)
        # A distinct, still valid PSBT for exactly the same transaction, input,
        # descriptor and signature: an additional global public metadata entry.
        replacement.g.map[0xfc] = {b'\x04test\x01': b'different export'}
        raw = replacement.serialize()
        gui_verify.verify_export(raw, manifest)  # Crypto alone would accept it.
        atomic_write(runner.directory(chain) / 'gui-signed.psbt', raw)
        with pytest.raises(Failed, match='EXPORT_CHANGED'): runner.run(chain, 'verify')
        assert 'signature_proof' not in runner.state(chain)
        assert 'acceptance' not in runner.state(chain)
    finally: runner.close()


@pytest.mark.parametrize('field,value', [
    ('vout', None), ('vout', False), ('vout', True), ('vout', -1), ('vout', 1),
    ('value_sats', False), ('value_sats', True), ('value_sats', -1), ('value_sats', 0), ('value_sats', 1000001),
    ('txid', None), ('txid', 'a' * 63), ('txid', 'A' * 64), ('txid', 'z' * 64)])
def test_funding_schema_rejects_bool_indexes_values_and_malformed_ids(field, value):
    psbt, manifest = signature_case(gui.CHAINS[0]); manifest['funding'][field] = value
    serialized = psbt.serialize()
    with pytest.raises(Failed, match='FUNDING_SCHEMA'): gui_verify.verify_export(serialized, manifest)


@pytest.mark.parametrize('value', [-1, 0, 100001])
def test_invalid_output_value_is_rejected_before_fee_or_signature(value):
    psbt, manifest = signature_case(gui.CHAINS[0])
    psbt.tx.vout[0].nValue = value; psbt.g.map[0] = psbt.tx.serialize()
    serialized = psbt.serialize()
    with pytest.raises(Failed, match='OUTPUT_VALUE'): gui_verify.verify_export(serialized, manifest)


@pytest.mark.parametrize('field,value', [('vout', False), ('vout', None), ('vout', -1), ('vout', 1), ('value_sats', True), ('value_sats', -1), ('txid', 'A'*64)])
def test_funding_stage_requires_typed_public_coin_metadata(config, tmp_path, field, value):
    def mutate(request, reply):
        if request['stage'] == 'funding':
            path = Path(request['case_directory']) / 'funding-proof.json'
            proof = json.loads(private_read(path)); proof[field] = value
            atomic_write(path, json.dumps(proof).encode())
    runner = gui.GuiRecovery(config, tmp_path/'run', worker=OfflineWorker(mutate), offline=True)
    try:
        for stage in gui.STAGES[:gui.STAGES.index('funding')]: runner.run(gui.CHAINS[0], stage)
        with pytest.raises(Failed, match='FUNDING'): runner.run(gui.CHAINS[0], 'funding')
        assert 'funding' not in runner.state(gui.CHAINS[0])
    finally: runner.close()
