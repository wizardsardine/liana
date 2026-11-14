"""Offline sponsor boundaries. No Keychain, live files, or network."""
from copy import deepcopy
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from harness import sponsor
from harness.state import Failed, Blocked, atomic_write, private_read


def address(byte):
    # Fixture encoder; intentionally not imported from the implementation.
    alphabet = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"
    bits = ''.join(f'{byte:08b}' for _ in range(32)) + '0000'
    payload = [0] + [int(bits[i:i+5], 2) for i in range(0, len(bits), 5)]
    chk = 1
    for d in [3, 3, 0, 20, 2] + payload + [0] * 6:
        top = chk >> 25
        chk = ((chk & 0x1ffffff) << 5) ^ d
        for i, generator in enumerate([0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3]):
            if top & (1 << i):
                chk ^= generator
    chk ^= 1
    return 'tb1' + ''.join(alphabet[d] for d in payload + [(chk >> (5 * i)) & 31 for i in range(5, -1, -1)])


def raw_transaction(txid, outputs, *, sequence=0xffffffff, witness=True):
    body = b'\x01' + bytes.fromhex(txid)[::-1] + (1).to_bytes(4, 'little') + b'\x00' + sequence.to_bytes(4, 'little')
    body += bytes([len(outputs)])
    for value, script in outputs:
        script = bytes.fromhex(script)
        body += value.to_bytes(8, 'little') + bytes([len(script)]) + script
    return (b'\x02\x00\x00\x00' + (b'\x00\x01' if witness else b'') + body
            + (b'\x03\x00\x01\x01\x01\x02' if witness else b'') + b'\x00' * 4).hex()


@pytest.fixture(autouse=True)
def no_external(monkeypatch):
    def forbidden(*a, **k):
        raise AssertionError('No live I/O permitted')
    monkeypatch.setattr(sponsor, 'http', forbidden)
    monkeypatch.setattr(sponsor.subprocess, 'run', forbidden)


@pytest.fixture
def scenario(tmp_path, monkeypatch):
    source, target = tmp_path / 'source', tmp_path / 'target'
    for directory in (source, target):
        directory.mkdir(mode=0o700)
    old_raw = raw_transaction('b' * 64, [(10000, '0014' + '11' * 20), (989000, '0020' + '22' * 32)])
    old_txid = sponsor.transaction(old_raw)['txid']
    for i, directory in enumerate((source, target)):
        state = {'run_id': str(i + 1), 'profile_dir': str(directory / 'profile'), 'vault_id': i + 10, 'owner_user_id': 5, 'descriptor_id': 'descriptor-' + str(i),
                 'spend': {'txid': old_txid, 'change_sats': 989000, 'change_address': address(34)}}
        artifacts = {'account_identity': 'a' * 64, 'vault_id': str(state['vault_id']), 'descriptor_id': state['descriptor_id'], 'public_acceptance': {'txid': old_txid}, 'broadcast_txid': old_txid, 'receive_address': address(51)}
        atomic_write(directory / 'provision.json', sponsor.encoded(state))
        atomic_write(directory / 'checkpoint.json', sponsor.encoded({'artifacts': artifacts}))
        atomic_write(directory / 'descriptor.txt', b'synthetic-public-descriptor')
    evidence = [{'endpoint': endpoint, 'genesis': sponsor.GENESIS, 'observed_at': 1000, 'txid': old_txid,
                 'vout': 1, 'raw_tx': old_raw, 'spent': False} for endpoint in sponsor.EXPLORERS]
    monkeypatch.setattr(sponsor, 'observe', lambda *a: deepcopy(evidence))
    args = SimpleNamespace(source_run=str(source), target_run=str(target), output_dir=str(tmp_path / 'sponsor'),
                           amount_sats=500000, fee_sats=1000, native_binary=str(tmp_path / 'native'), sign_intent_sha256=None)
    atomic_write(Path(args.native_binary), b"synthetic native candidate")
    return args, evidence


def native_reply(request):
    intent = json.loads(private_read(request['sponsor_intent_file']))
    raw = raw_transaction(intent['previous_txid'], [(intent['amount_sats'], intent['target_script']),
                                                   (intent['change_sats'], intent['source_script'])])
    return {'ok': True, 'action': 'sponsor', 'network': 'testnet4', 'raw_tx': raw,
            'txid': sponsor.transaction(raw)['txid'], 'intent_sha256': request['sponsor_intent_sha256'],
            'amount_sats': intent['amount_sats'], 'fee_sats': intent['fee_sats'], 'change_sats': intent['change_sats']}


@pytest.fixture
def hop(scenario, tmp_path, monkeypatch):
    import shutil
    args, _ = scenario
    prepared = sponsor.execute(args)
    prior_dir = Path(args.output_dir)
    receipt = native_reply({'sponsor_intent_file': str(prior_dir / 'sponsor-intent.json'),
                            'sponsor_intent_sha256': prepared['intent_sha256']})
    atomic_write(prior_dir / 'sponsor-receipt.json', sponsor.encoded(receipt))
    previous_target = Path(args.target_run)
    # The historical target progressed after receiving its payout. Its old
    # snapshot stays in the parent intent; the new intent binds current state.
    checkpoint = json.loads(private_read(previous_target / 'checkpoint.json'))
    checkpoint['artifacts']['later_progress'] = True
    atomic_write(previous_target / 'checkpoint.json', sponsor.encoded(checkpoint))
    target = tmp_path / 'next-target'
    shutil.copytree(previous_target, target)
    state = json.loads(private_read(target / 'provision.json'))
    state.update(run_id='next-target', vault_id=99, profile_dir=str(target / 'profile'))
    checkpoint['artifacts']['vault_id'] = '99'
    atomic_write(target / 'provision.json', sponsor.encoded(state))
    atomic_write(target / 'checkpoint.json', sponsor.encoded(checkpoint))
    args.source_sponsor_dir = str(prior_dir)
    args.target_run = str(target)
    args.output_dir = str(tmp_path / 'next-sponsor')
    args.amount_sats = 100000
    evidence = [{'endpoint': ep, 'genesis': sponsor.GENESIS, 'observed_at': 1000,
                 'txid': receipt['txid'], 'vout': 1, 'raw_tx': receipt['raw_tx'], 'spent': False}
                for ep in sponsor.EXPLORERS]
    monkeypatch.setattr(sponsor, 'observe', lambda *a: deepcopy(evidence))
    return args, receipt, evidence, previous_target


def test_one_hop_prepare_and_sign_keep_original_runs_and_parent_bytes(hop, monkeypatch):
    args, parent_receipt, _, previous_target = hop
    before = {str(p): p.read_bytes() for d in (args.source_run, args.target_run, args.source_sponsor_dir, previous_target)
              for p in Path(d).iterdir() if p.is_file()}
    prepared = sponsor.execute(args)
    intent = json.loads(private_read(Path(args.output_dir) / 'sponsor-intent.json'))
    assert intent['version'] == 2
    assert intent['previous_txid'] == parent_receipt['txid']
    assert (intent['previous_vout'], intent['previous_value_sats'], intent['amount_sats'], intent['change_sats']) == (1, 488000, 100000, 387000)
    assert intent['prior_sponsor']['intent_json'].encode() == private_read(Path(args.source_sponsor_dir) / 'sponsor-intent.json')
    assert intent['prior_sponsor']['receipt_json'].encode() == private_read(Path(args.source_sponsor_dir) / 'sponsor-receipt.json')
    assert intent['prior_sponsor']['target']['checkpoint_sha256'] != json.loads(intent['prior_sponsor']['intent_json'])['target']['checkpoint_sha256']
    args.sign_intent_sha256 = prepared['intent_sha256']
    stub_native(monkeypatch)
    result = sponsor.execute(args)
    assert result['signed']
    assert result['broadcast'] is False
    assert sponsor.execute(args) == result
    assert before == {str(p): p.read_bytes() for d in (args.source_run, args.target_run, args.source_sponsor_dir, previous_target)
                      for p in Path(d).iterdir() if p.is_file()}


@pytest.mark.parametrize('change', ['receipt', 'source', 'nested', 'descriptor', 'account', 'identity', 'witness'])
def test_one_hop_refuses_unbound_or_changed_parent_before_native(hop, change):
    args, _, evidence, previous_target = hop
    parent_path = Path(args.source_sponsor_dir) / 'sponsor-intent.json'
    if change in ('source', 'nested'):
        parent = json.loads(private_read(parent_path))
        if change == 'source': parent['source']['state_sha256'] = 'changed'
        else: parent['prior_sponsor'] = {}
        atomic_write(parent_path, sponsor.encoded(parent))
    elif change == 'receipt':
        path = Path(args.source_sponsor_dir) / 'sponsor-receipt.json'
        receipt = json.loads(private_read(path)); receipt['fee_sats'] += 1
        atomic_write(path, sponsor.encoded(receipt))
    elif change == 'descriptor':
        atomic_write(previous_target / 'descriptor.txt', b'other descriptor')
    elif change == 'account':
        path = previous_target / 'checkpoint.json'; checkpoint = json.loads(private_read(path))
        checkpoint['artifacts']['account_identity'] = 'b' * 64
        atomic_write(path, sponsor.encoded(checkpoint))
    elif change == 'identity':
        path = previous_target / 'provision.json'; state = json.loads(private_read(path)); state['run_id'] = 'other'
        atomic_write(path, sponsor.encoded(state))
    else:
        # Same unsigned txid, different observed witness cannot substitute for
        # the immutable parent receipt, even before native crypto checks.
        for item in evidence: item['raw_tx'] = item['raw_tx'].replace('03000101010200000000', '03000101010300000000')
    with pytest.raises(Failed):
        sponsor.execute(args)


def test_one_hop_freezes_parent_receipt_and_current_snapshot(hop, monkeypatch):
    args, _, _, previous_target = hop
    args.sign_intent_sha256 = sponsor.execute(args)['intent_sha256']
    def mutate():
        path = previous_target / 'checkpoint.json'
        path.write_bytes(path.read_bytes() + b' ')
    stub_native(monkeypatch, mutate)
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == 'PARENT_SNAPSHOT_CHANGED'
    assert not (Path(args.output_dir) / 'sponsor-receipt.json').exists()


def test_one_hop_refuses_changed_parent_bytes_after_prepare(hop):
    args, _, _, _ = hop
    args.sign_intent_sha256 = sponsor.execute(args)['intent_sha256']
    path = Path(args.source_sponsor_dir) / 'sponsor-receipt.json'
    atomic_write(path, private_read(path) + b' ')
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == 'SPONSOR_INTENT_CHANGED'


def test_one_hop_output_cannot_overwrite_parent_evidence(hop):
    args, _, _, _ = hop
    args.output_dir = args.source_sponsor_dir
    with pytest.raises(Failed, match='OUTPUT_ISOLATION'):
        sponsor.execute(args)


def test_prepare_does_not_invoke_native_or_modify_runs(scenario):
    args, _ = scenario
    before = {str(p): p.read_bytes() for d in (args.source_run, args.target_run) for p in Path(d).iterdir()}
    result = sponsor.execute(args)
    assert result['prepared'] is True
    intent_file = Path(args.output_dir) / 'sponsor-intent.json'
    assert result['intent_sha256'] == hashlib.sha256(private_read(intent_file)).hexdigest()
    intent = json.loads(private_read(intent_file))
    assert (intent['previous_vout'], intent['previous_value_sats'], intent['change_sats']) == (1, 989000, 488000)
    assert not (Path(args.output_dir) / 'sponsor-receipt.json').exists()
    assert before == {str(p): p.read_bytes() for d in (args.source_run, args.target_run) for p in Path(d).iterdir()}


def test_sign_requires_prepare_and_exact_hash(scenario):
    args, _ = scenario
    args.sign_intent_sha256 = '0' * 64
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == 'PREPARE_FIRST'
    args.sign_intent_sha256 = None
    sponsor.execute(args)
    args.sign_intent_sha256 = '0' * 64
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == 'SIGN_INTENT_BINDING'


def test_changed_amount_cannot_replace_prepared_intent(scenario):
    args, _ = scenario
    sponsor.execute(args)
    args.amount_sats = 400000
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == 'SPONSOR_INTENT_CHANGED'


@pytest.mark.parametrize('field,value', [('amount_sats', 99999), ('amount_sats', 500001), ('fee_sats', 1001)])
def test_limit_refusal_happens_before_observation(scenario, monkeypatch, field, value):
    args, _ = scenario
    setattr(args, field, value)
    monkeypatch.setattr(sponsor, 'observe', lambda *a: pytest.fail('must refuse before network'))
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == 'AMOUNT_BOUNDS'


@pytest.mark.parametrize('destination', ['source', 'target', 'ancestor'])
def test_output_cannot_touch_either_run_or_ancestor(scenario, destination):
    args, _ = scenario
    args.output_dir = args.source_run if destination == 'source' else args.target_run if destination == 'target' else str(Path(args.source_run).parent)
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == 'OUTPUT_ISOLATION'


def test_output_lock_is_exclusive(tmp_path):
    directory = tmp_path / 'output'
    with sponsor.output_lock(directory, []):
        context_under_test = sponsor.output_lock(directory, [])
        with pytest.raises(Blocked) as caught:
            with context_under_test:
                pytest.fail('concurrent sponsor admitted')
        assert caught.value.code == 'SPONSOR_BUSY'


@pytest.mark.parametrize('mutation', ['amount', 'sequence', 'input', 'fee', 'chain'])
def test_signed_result_independently_checks_intent(scenario, mutation):
    args, _ = scenario
    prepared = sponsor.execute(args)
    request = {'sponsor_intent_file': str(Path(args.output_dir) / 'sponsor-intent.json'), 'sponsor_intent_sha256': prepared['intent_sha256']}
    reply = native_reply(request)
    intent = json.loads(private_read(request['sponsor_intent_file']))
    sponsor.validate_receipt(reply, intent, prepared['intent_sha256'])
    if mutation in ('amount', 'sequence', 'input'):
        reply['raw_tx'] = raw_transaction('c' * 64 if mutation == 'input' else intent['previous_txid'],
            [(500001 if mutation == 'amount' else 500000, intent['target_script']), (488000, intent['source_script'])],
            sequence=0xfffffffd if mutation == 'sequence' else 0xffffffff)
    elif mutation == 'fee':
        reply['fee_sats'] = 999
    else:
        reply['network'] = 'bitcoin'
    with pytest.raises(Failed):
        sponsor.validate_receipt(reply, intent, prepared['intent_sha256'])


def test_parser_rejects_trailing_bytes_and_txid_ignores_witness():
    raw = raw_transaction('b' * 64, [(500000, '0020' + '22' * 32)])
    bare = raw_transaction('b' * 64, [(500000, '0020' + '22' * 32)], witness=False)
    assert sponsor.transaction(raw)['txid'] == sponsor.transaction(bare)['txid']
    with pytest.raises(Failed):
        sponsor.transaction(raw + '00')


def test_p2wsh_address_requires_checksum_network_and_32bytes():
    assert sponsor.p2wsh_script(address(34)) == '0020' + '22' * 32
    for wrong in [address(34).upper(), address(34).replace('tb1', 'bc1'), address(34)[:-1] + ('q' if address(34)[-1] != 'q' else 'p')]:
        with pytest.raises(Failed):
            sponsor.p2wsh_script(wrong)


def test_observation_requires_two_matching_chains_and_unspent(monkeypatch):
    raw = raw_transaction('b' * 64, [(989000, '0020' + '22' * 32)])
    txid = sponsor.transaction(raw)['txid']
    spent = [False]
    def read(url):
        if url.endswith('/block-height/0'):
            return sponsor.GENESIS.encode()
        if url.endswith('/hex'):
            return raw.encode()
        return json.dumps({'spent': spent[0]}).encode()
    monkeypatch.setattr(sponsor, 'http', read)
    assert len(sponsor.observe(txid, 1)) == 2
    spent[0] = True
    with pytest.raises(Failed) as caught:
        sponsor.observe(txid, 1)
    assert caught.value.code == 'SOURCE_SPENT'


@pytest.mark.parametrize('mutation', ['missing_account', 'different_account', 'vault', 'descriptor'])
def test_checkpoint_identity_is_required_before_network(scenario, monkeypatch, mutation):
    args, _ = scenario
    path = Path(args.target_run) / 'checkpoint.json'
    value = json.loads(private_read(path))
    if mutation == 'missing_account':
        del value['artifacts']['account_identity']
    elif mutation == 'different_account':
        value['artifacts']['account_identity'] = 'b' * 64
    else:
        value['artifacts']['vault_id' if mutation == 'vault' else 'descriptor_id'] = 999
    atomic_write(path, sponsor.encoded(value))
    monkeypatch.setattr(sponsor, 'observe', lambda *a: pytest.fail('must reject before network'))
    with pytest.raises(Failed):
        sponsor.execute(args)


def stub_native(monkeypatch, mutation=lambda: None):
    def run(argv, **kwargs):
        request = json.loads(kwargs['input'])
        reply = native_reply(request)
        mutation()
        return SimpleNamespace(returncode=0, stdout=sponsor.encoded(reply))
    monkeypatch.setattr(sponsor.subprocess, 'run', run)


@pytest.mark.parametrize('mutation', ['source', 'target', 'binary'])
def test_native_cannot_change_bound_snapshot_or_binary_before_publication(scenario, monkeypatch, mutation):
    args, _ = scenario
    args.sign_intent_sha256 = sponsor.execute(args)['intent_sha256']
    def change():
        path = Path(args.native_binary) if mutation == 'binary' else Path(args.source_run if mutation == 'source' else args.target_run) / 'checkpoint.json'
        path.write_bytes(path.read_bytes() + b' ')
    stub_native(monkeypatch, change)
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == ('NATIVE_BINARY_CHANGED' if mutation == 'binary' else 'RUN_SNAPSHOT_CHANGED')
    assert not (Path(args.output_dir) / 'sponsor-receipt.json').exists()


def test_sign_and_replay_preserve_immutable_receipt_and_runs(scenario, monkeypatch):
    args, _ = scenario
    args.sign_intent_sha256 = sponsor.execute(args)['intent_sha256']
    stub_native(monkeypatch)
    before = {str(p): p.read_bytes() for d in (args.source_run, args.target_run) for p in Path(d).iterdir()}
    result = sponsor.execute(args)
    receipt_path = Path(args.output_dir) / 'sponsor-receipt.json'
    receipt = private_read(receipt_path)
    assert result['signed']
    assert not result['broadcast']
    assert json.loads(receipt)['native_binary_sha256'] == sponsor.sha(Path(args.native_binary).read_bytes())
    assert sponsor.execute(args) == result
    assert private_read(receipt_path) == receipt
    assert before == {str(p): p.read_bytes() for d in (args.source_run, args.target_run) for p in Path(d).iterdir()}


@pytest.mark.parametrize('bad', [11, True, '011', '11 ', '0', '-11'])
def test_checkpoint_vault_id_requires_actual_canonical_string(scenario, bad):
    args, _ = scenario
    path = Path(args.target_run) / 'checkpoint.json'
    value = json.loads(private_read(path))
    value['artifacts']['vault_id'] = bad
    atomic_write(path, sponsor.encoded(value))
    with pytest.raises(Failed) as caught:
        sponsor.execute(args)
    assert caught.value.code == 'ACCOUNT_BINDING'
