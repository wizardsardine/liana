"""Independent PUBLIC, UNFUNDED restored-key-control PSBT verification.

Never establishes GUI provenance, coin recognition, broadcast or funded signing.
"""
import hashlib
import io
from pathlib import Path
import struct
import sys

from .state import Blocked, Failed

CHAINS = ('testnet4', 'bitcoin-blake2b-testnet4')


def require(ok, code):
    if not ok:
        raise Failed('NATIVE_RECOVERY_VERIFY_' + code, 'Native unfunded public proof rejected: ' + code)


def canonical_maps(raw):
    stream = io.BytesIO(raw)
    require(stream.read(5) == b'psbt\xff', 'PSBT')
    def size():
        head = stream.read(1); require(len(head) == 1, 'TRUNCATED')
        first = head[0]
        if first < 253: return first
        count = {253: 2, 254: 4, 255: 8}[first]; value = stream.read(count)
        require(len(value) == count, 'TRUNCATED'); number = int.from_bytes(value, 'little')
        require(number >= {253: 253, 254: 65536, 255: 4294967296}[first], 'COMPACT_SIZE')
        return number
    def take(length):
        require(length <= len(raw), 'SIZE'); value = stream.read(length)
        require(len(value) == length, 'TRUNCATED'); return value
    maps = []
    while stream.tell() < len(raw):
        keys = set()
        while True:
            length = size()
            if length == 0: break
            key = take(length); require(key not in keys, 'DUPLICATE_KEY'); keys.add(key)
            take(size())
        maps.append(keys)
    require(len(maps) == 3 and b'\x00' in maps[0], 'MAPS')


def dependencies():
    require(__debug__, "PYTHON_OPTIMIZED")
    tests = str(Path(__file__).resolve().parents[2])
    if tests not in sys.path:
        sys.path.insert(0, tests)
    try:
        from bip380.descriptors import Descriptor
        from coincurve import PublicKey
        from test_framework.serializations import PSBT, CTransaction, CTxOut, from_binary, sighash_all_witness
        from test_btcb2_unified_sighash import unified_sighash
    except ImportError:
        raise Blocked('NATIVE_RECOVERY_VERIFY_DEPENDENCIES', 'Independent verifier dependencies are unavailable.') from None
    return Descriptor, PublicKey, PSBT, CTransaction, CTxOut, from_binary, sighash_all_witness, unified_sighash


def verify(raw, identity, chain):
    require(chain in CHAINS and identity['cube']['network'] == chain, 'CHAIN')
    require(isinstance(raw, bytes) and raw.startswith(b'psbt\xff') and len(raw) < 1_000_000, 'PSBT')
    canonical_maps(raw)
    Descriptor, PublicKey, PSBT, CTransaction, CTxOut, from_binary, sighash_all_witness, unified_sighash = dependencies()
    try:
        require(__debug__, 'PYTHON_OPTIMIZED')
        psbt = PSBT(); stream = io.BytesIO(raw); psbt.deserialize(stream)
        require(not stream.read() and len(psbt.i) == len(psbt.o) == 1, 'SHAPE')
        receive, change = Descriptor.from_str(identity['descriptor']).singlepath_descriptors()
        receive.derive(0); change.derive(0)
        parent = CTransaction(); parent_stream = io.BytesIO(psbt.i[0].map[0]); parent.deserialize(parent_stream)
        witness = from_binary(CTxOut, psbt.i[0].map[1])
        require(not parent_stream.read() and len(parent.vin) == len(parent.vout) == 1
                and parent.vin[0].prevout.hash == 0 and parent.vin[0].prevout.n == 0xffffffff, 'EXPLICIT_UNFUNDED_PARENT')
        require(psbt.tx.vin[0].prevout.hash == int(parent.txid().hex(), 16) and psbt.tx.vin[0].prevout.n == 0
                and witness.serialize() == parent.vout[0].serialize() and witness.serialize() == psbt.i[0].map[1]
                and parent.serialize() == psbt.i[0].map[0] and psbt.tx.serialize() == psbt.g.map[0]
                and witness.nValue == 100000 and witness.scriptPubKey == bytes(receive.script_pubkey)
                and psbt.i[0].map[5] == bytes(receive.script_sighash), 'INPUT')
        require(psbt.tx.vout[0].nValue == 99738 and psbt.tx.vout[0].scriptPubKey == bytes(change.script_pubkey), 'OUTPUT')
        signatures = [(pubkey, signature, False) for pubkey, signature in psbt.i[0].map.get(2, {}).items()]
        signatures += [(key[10:], value, True) for key, value in psbt.i[0].map.get(0xfc, {}).items()
                       if key.startswith(b'\x08coincube\x00')]
        require(len(signatures) == 2 and len({item[0] for item in signatures}) == 2, 'SIGNATURE_COUNT')
        expected = 0x21 if chain == CHAINS[1] else 1
        require(set(psbt.i[0].map[6]) == {item[0] for item in signatures}, 'ORIGIN_KEYS')
        script = bytes(receive.script_sighash)
        digest = unified_sighash(psbt.tx, 0, 0x21, 1, [witness], script) if expected == 0x21 else sighash_all_witness(script, psbt, 0)
        verified_paths = []
        for pubkey, signature, proprietary in signatures:
            require(signature[-1] == expected and proprietary == (expected == 0x21), 'DOMAIN')
            origin = psbt.i[0].map[6][pubkey]
            require(origin[:4].hex() == identity['fingerprint'] and len(origin) >= 12 and (len(origin)-4) % 4 == 0, 'ORIGIN')
            path = list(struct.unpack('<' + 'I'*((len(origin)-4)//4), origin[4:]))
            candidates = []
            for key in Descriptor.from_str(identity['descriptor']).keys:
                if key.origin and key.origin.fingerprint == origin[:4] and key.path:
                    prefix = key.origin.path; suffix = path[len(prefix):]
                    if path[:len(prefix)] == prefix and suffix in ([0, 0], [2, 0]) and suffix[:-1] in key.path.paths:
                        candidates.append((key.key.get_pubkey_from_path(suffix), suffix))
            require(len(candidates) == 1 and candidates[0][0] == pubkey, 'DESCRIPTOR_KEY')
            verified_paths.append(candidates[0][1])
            verifier = PublicKey(pubkey)
            require(verifier.verify(signature[:-1], digest, hasher=None), 'ECDSA')
            altered = bytes([digest[0]^1]) + digest[1:]
            require(not verifier.verify(signature[:-1], altered, hasher=None), 'ALTERED_DIGEST')
            if expected == 0x21:
                require(not verifier.verify(signature[:-1], sighash_all_witness(script, psbt, 0), hasher=None), 'BITCOIN_REPLAY')
        require(sorted(verified_paths) == [[0, 0], [2, 0]], 'EXACT_PRIMARY_RECOVERY_KEYS')
    except (KeyError, ValueError, AssertionError, AttributeError, IndexError, TypeError, struct.error):
        raise Failed('NATIVE_RECOVERY_VERIFY_ARTIFACT', 'Invalid public unfunded proof artifact.') from None
    return {'version': 1, 'provenance': 'native_product_integration', 'chain': chain,
            'verified_key_count': 2, 'verified_paths': sorted(verified_paths),
            'psbt_sha256': hashlib.sha256(raw).hexdigest(), 'signature_digest': digest.hex(), 'sighash': expected,
            'funded': False, 'recognized_utxo': False, 'gui': False, 'broadcast': False,
            'scope': 'Unfunded restored-key control only; no funded transaction or GUI acceptance.'}
