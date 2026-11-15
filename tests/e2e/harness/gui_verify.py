"""Public-artifact verifier, independent of Tenshu's Rust signing implementation.

Uses the existing Python descriptor/BIP143/Knots-vector reference code and
OpenSSL's ECDSA verifier. Never loads a seed, PIN, hot cache or private key.
"""
import base64
import hashlib
import io
import re
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

from .state import Blocked, Failed

CHAINS = ("testnet4", "bitcoin-blake2b-testnet4")
MAX = 4_000_000


def require(value, code):
    if not value:
        raise Failed("GUI_VERIFY_" + code, "GUI exported-artifact verification failed: " + code)


def canonical_maps(raw):
    require(isinstance(raw, bytes) and 5 <= len(raw) <= MAX and raw.startswith(b"psbt\xff"), "PSBT")
    offset = 5
    def take(size):
        nonlocal offset
        require(0 <= size <= len(raw) - offset, "TRUNCATED")
        value = raw[offset:offset + size]; offset += size
        return value
    def size():
        lead = take(1)[0]
        if lead < 253: return lead
        n = {253: 2, 254: 4, 255: 8}[lead]
        value = int.from_bytes(take(n), "little")
        require(value >= {253: 253, 254: 65536, 255: 4294967296}[lead], "COMPACT_SIZE")
        return value
    maps = []
    while offset < len(raw):
        items = {}
        while True:
            length = size()
            if not length: break
            key = take(length)
            require(key not in items, "DUPLICATE_KEY")
            items[key] = take(size())
        maps.append(items)
    require(maps and b"\x00" in maps[0] and maps[0].get(b"\xfb", b"\0" * 4) == b"\0" * 4, "GLOBAL")
    return maps


def references():
    tests = Path(__file__).resolve().parents[2]
    sys.path.insert(0, str(tests)) if str(tests) not in sys.path else None
    try:
        from bip380.descriptors import Descriptor
        from test_framework.serializations import PSBT, CTransaction, CTxOut, from_binary, sighash_all_witness
        from test_btcb2_unified_sighash import unified_sighash
    except ImportError:
        raise Blocked("GUI_VERIFY_DEPENDENCIES", "Install the repository's pinned tests/requirements.txt for the independent verifier.") from None
    return Descriptor, PSBT, CTransaction, CTxOut, from_binary, sighash_all_witness, unified_sighash


def canonical_der(signature):
    require(8 <= len(signature) <= 72 and signature[0] == 0x30 and signature[1] == len(signature) - 2, "DER")
    offset = 2; values = []
    for _ in range(2):
        require(offset + 2 <= len(signature) and signature[offset] == 2, "DER")
        size = signature[offset + 1]; offset += 2
        require(1 <= size <= 33 and offset + size <= len(signature), "DER")
        integer = signature[offset:offset + size]; offset += size
        require(not integer[0] & 0x80 and (size == 1 or integer[0] != 0 or integer[1] & 0x80), "DER")
        values.append(int.from_bytes(integer, "big"))
    order = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141
    require(offset == len(signature) and 0 < values[0] < order and 0 < values[1] <= order // 2, "DER_LOW_S")


def openssl_verify(pubkey, signature, message):
    # secp256k1 SubjectPublicKeyInfo, compressed point. No private material.
    require(len(pubkey) == 33 and pubkey[0] in (2, 3) and len(message) == 32, "PUBLIC_KEY")
    spki = bytes.fromhex("3036301006072a8648ce3d020106052b8104000a032200") + pubkey
    with tempfile.TemporaryDirectory(prefix="coincube-gui-public-verify-") as folder:
        root = Path(folder)
        for name, value in (("public.der", spki), ("signature.der", signature), ("digest.bin", message)):
            (root / name).write_bytes(value)
        try:
            result = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-keyform", "DER", "-inkey", str(root / "public.der"),
                "-sigfile", str(root / "signature.der"), "-in", str(root / "digest.bin"), "-pkeyopt", "digest:sha256"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        except (OSError, subprocess.TimeoutExpired):
            raise Blocked("GUI_VERIFY_OPENSSL", "OpenSSL public-key verification is unavailable.") from None
    return result.returncode == 0


def funding_parent(funding, transaction_type, maximum=1000000):
    require(type(funding.get("vout")) is int and funding["vout"] >= 0
            and type(funding.get("value_sats")) is int and 0 < funding["value_sats"] <= maximum
            and isinstance(funding.get("txid"), str) and re.fullmatch(r"[0-9a-f]{64}", funding["txid"]), "FUNDING_SCHEMA")
    try:
        raw = bytes.fromhex(funding["raw_tx"])
        stream = io.BytesIO(raw); parent = transaction_type(); parent.deserialize(stream)
        require(stream.read() == b"" and parent.txid().hex() == funding["txid"], "PARENT")
        require(funding["vout"] < len(parent.vout), "FUNDING_SCHEMA")
        require(parent.vout[funding["vout"]].nValue == funding["value_sats"], "FUNDING_VALUE")
    except (AssertionError, KeyError, ValueError, IndexError, AttributeError, TypeError):
        raise Failed("GUI_VERIFY_FUNDING_SCHEMA", "Invalid funding parent metadata.") from None
    return parent, raw


def verify_export(content, manifest):
    require(manifest.get("chain") in CHAINS and manifest.get("broadcast") is False, "CHAIN")
    if not content.startswith(b"psbt\xff"):
        try: content = base64.b64decode(content.strip(), validate=True)
        except ValueError: raise Failed("GUI_VERIFY_BASE64", "Export is not canonical base64 or PSBT bytes.") from None
    maps = canonical_maps(content)
    Descriptor, PSBT, Tx, TxOut, parse, bip143, unified = references()
    require(__debug__, "PYTHON_OPTIMIZED")  # The existing independent reference uses assertions.
    stream = io.BytesIO(content)
    try:
        psbt = PSBT().deserialize(stream)
        require(stream.read() == b"" and len(psbt.i) == len(psbt.o) == 1 and len(maps) == 3, "SHAPE")
        desc = Descriptor.from_str(manifest["descriptor"])
        receive, change = desc.singlepath_descriptors()
        ri, ci = manifest["receive_index"], manifest["change_index"]
        require(type(ri) is int and type(ci) is int and 0 <= ri <= 10000 and 0 <= ci <= 10000, "DERIVATION")
        receive.derive(ri); change.derive(ci)
        funding = manifest["funding"]
        require(funding["chain"] == manifest["chain"] and funding["wallet_recognized"] is True, "FUNDING_CHAIN")
        parent, parent_raw = funding_parent(funding, Tx)
        txin = psbt.tx.vin[0]
        require(txin.prevout.hash == int(funding["txid"], 16) and txin.prevout.n == funding["vout"], "OUTPOINT")
        previous = parent.vout[funding["vout"]]
        witness = parse(TxOut, psbt.i[0].map[1])
        require(previous.serialize() == witness.serialize() and witness.nValue == funding["value_sats"]
                and witness.scriptPubKey == bytes(receive.script_pubkey), "RECOGNIZED_COIN")
        require(psbt.i[0].map.get(0) == parent_raw and psbt.i[0].map.get(5) == bytes(receive.script_sighash), "WITNESS_SCRIPT")
        require(not any(k in psbt.i[0].map for k in (7, 8, 0x13, 0x14)), "FINAL_OR_TAPROOT")
        require(psbt.tx.vout[0].scriptPubKey == bytes(change.script_pubkey), "SELF_TRANSFER")
        require(type(psbt.tx.vout[0].nValue) is int and 0 < psbt.tx.vout[0].nValue <= witness.nValue, "OUTPUT_VALUE")
        fee = witness.nValue - psbt.tx.vout[0].nValue
        require(type(manifest["maximum_fee_sats"]) is int and 0 < fee <= manifest["maximum_fee_sats"], "FEE")
        signatures = []
        for key, value in maps[1].items():
            if key[0] == 2:
                signatures.append((key[1:], value, "standard"))
            elif key[0] == 0xfc and key[1:11] == b"\x08coincube\x00":
                signatures.append((key[11:], value, "unified"))
        require(len(signatures) == 1, "SIGNATURE_COUNT")
        pubkey, signature, representation = signatures[0]
        canonical_der(signature[:-1])
        expected = 0x21 if manifest["chain"] == CHAINS[1] else 1
        require(signature[-1] == expected and (expected == 0x21 or representation == "standard"), "SIGHASH")
        if 3 in psbt.i[0].map:
            require(psbt.i[0].map[3] == struct.pack("<I", expected), "SIGHASH")
        origin = psbt.i[0].map[6][pubkey]
        require(len(origin) >= 12 and (len(origin) - 4) % 4 == 0
                and origin[:4].hex() == manifest["root_fingerprint"], "ORIGIN")
        path = list(struct.unpack("<" + "I" * ((len(origin) - 4) // 4), origin[4:]))
        candidates = []
        for key in Descriptor.from_str(manifest["descriptor"]).keys:
            if key.origin and key.origin.fingerprint == origin[:4] and key.path:
                prefix = key.origin.path
                suffix = path[len(prefix):]
                if (path[:len(prefix)] == prefix and len(suffix) >= 2 and suffix[-1] == ri
                        and suffix[:-1] in key.path.paths and suffix[0] == 0):
                    candidates.append(key.key.get_pubkey_from_path(suffix))
        require(candidates.count(pubkey) == 1, "DESCRIPTOR_SIGNER")
        script = bytes(receive.script_sighash)
        digest = unified(psbt.tx, 0, 0x21, 1, [witness], script) if expected == 0x21 else bip143(script, psbt, 0)
        require(openssl_verify(pubkey, signature[:-1], digest), "ECDSA")
        altered = bytes([digest[0] ^ 1]) + digest[1:]
        require(not openssl_verify(pubkey, signature[:-1], altered), "ALTERED_DIGEST")
        if expected == 0x21:
            require(not openssl_verify(pubkey, signature[:-1], bip143(script, psbt, 0)), "REPLAY_DOMAIN")
    except (AssertionError, KeyError, ValueError, IndexError, struct.error, AttributeError, TypeError):
        raise Failed("GUI_VERIFY_ARTIFACT", "The GUI export or independent public descriptor metadata is invalid.") from None
    return {"chain": manifest["chain"], "txid": psbt.tx.txid().hex(), "psbt_sha256": hashlib.sha256(content).hexdigest(),
            "signer_fingerprint": manifest["root_fingerprint"], "sighash": expected, "digest": digest.hex(),
            "fee_sats": fee, "broadcast": False, "scope": "Public PSBT signature verification only; GUI provenance is required separately; not broadcast acceptance"}
