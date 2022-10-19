#!/usr/bin/env python3
#
# Taken then adapted from:
#   - Initially https://github.com/achow101/psbt-simple-signer/blob/5def3622a09f5bcb76ae79707f0790d050291474/serializations.py
#   - Then from the October 2022 Bitcoin Core functional test for the new PSBTMap class
#
# Copyright (c) 2010 ArtForz -- public domain half-a-node
# Copyright (c) 2012 Jeff Garzik
# Copyright (c) 2010-2016 The Bitcoin Core developers
# Copyright (c) 2022 The Bitcoin Core developers
# Distributed under the MIT software license, see the accompanying
# file COPYING or http://www.opensource.org/licenses/mit-license.php.
"""Bitcoin Object Python Serializations

Modified from the test/test_framework/mininode.py file from the
Bitcoin repository

CTransaction,CTxIn, CTxOut, etc....:
    data structures that should map to corresponding structures in
    bitcoin/primitives for transactions only
ser_*, deser_*: functions that handle serialization/deserialization
"""

from io import BytesIO
from codecs import encode
import struct
import binascii
import hashlib
import copy
import base64


def sha256(s):
    return hashlib.new("sha256", s).digest()


def ripemd160(s):
    return hashlib.new("ripemd160", s).digest()


def hash256(s):
    return sha256(sha256(s))


def hash160(s):
    return ripemd160(sha256(s))


# Serialization/deserialization tools
def ser_compact_size(l):
    r = b""
    if l < 253:
        r = struct.pack("B", l)
    elif l < 0x10000:
        r = struct.pack("<BH", 253, l)
    elif l < 0x100000000:
        r = struct.pack("<BI", 254, l)
    else:
        r = struct.pack("<BQ", 255, l)
    return r


def deser_compact_size(f):
    nit = struct.unpack("<B", f.read(1))[0]
    if nit == 253:
        nit = struct.unpack("<H", f.read(2))[0]
    elif nit == 254:
        nit = struct.unpack("<I", f.read(4))[0]
    elif nit == 255:
        nit = struct.unpack("<Q", f.read(8))[0]
    return nit


def deser_string(f):
    nit = deser_compact_size(f)
    return f.read(nit)


def ser_string(s):
    return ser_compact_size(len(s)) + s


def deser_uint256(f):
    r = 0
    for i in range(8):
        t = struct.unpack("<I", f.read(4))[0]
        r += t << (i * 32)
    return r


def ser_uint256(u):
    rs = b""
    for i in range(8):
        rs += struct.pack("<I", u & 0xFFFFFFFF)
        u >>= 32
    return rs


def uint256_from_str(s):
    r = 0
    t = struct.unpack("<IIIIIIII", s[:32])
    for i in range(8):
        r += t[i] << (i * 32)
    return r


def uint256_from_compact(c):
    nbytes = (c >> 24) & 0xFF
    v = (c & 0xFFFFFF) << (8 * (nbytes - 3))
    return v


def deser_vector(f, c):
    nit = deser_compact_size(f)
    r = []
    for i in range(nit):
        t = c()
        t.deserialize(f)
        r.append(t)
    return r


# ser_function_name: Allow for an alternate serialization function on the
# entries in the vector (we use this for serializing the vector of transactions
# for a witness block).
def ser_vector(l, ser_function_name=None):
    r = ser_compact_size(len(l))
    for i in l:
        if ser_function_name:
            r += getattr(i, ser_function_name)()
        else:
            r += i.serialize()
    return r


def deser_uint256_vector(f):
    nit = deser_compact_size(f)
    r = []
    for i in range(nit):
        t = deser_uint256(f)
        r.append(t)
    return r


def ser_uint256_vector(l):
    r = ser_compact_size(len(l))
    for i in l:
        r += ser_uint256(i)
    return r


def deser_string_vector(f):
    nit = deser_compact_size(f)
    r = []
    for i in range(nit):
        t = deser_string(f)
        r.append(t)
    return r


def ser_string_vector(l):
    r = ser_compact_size(len(l))
    for sv in l:
        r += ser_string(sv)
    return r


def deser_int_vector(f):
    nit = deser_compact_size(f)
    r = []
    for i in range(nit):
        t = struct.unpack("<i", f.read(4))[0]
        r.append(t)
    return r


def ser_int_vector(l):
    r = ser_compact_size(len(l))
    for i in l:
        r += struct.pack("<i", i)
    return r


def hex_str_to_bytes(s):
    return binascii.unhexlify(s)


def bytes_to_hex_str(s):
    return binascii.hexlify(s)


# like from_hex, but without the hex part
def from_binary(cls, stream):
    """deserialize a binary stream (or bytes object) into an object"""
    # handle bytes object by turning it into a stream
    was_bytes = isinstance(stream, bytes)
    if was_bytes:
        stream = BytesIO(stream)
    obj = cls()
    obj.deserialize(stream)
    if was_bytes:
        assert len(stream.read()) == 0
    return obj


def ser_sig_der(r, s):
    sig = b"\x30"

    # Make r and s as short as possible
    ri = 0
    for b in r:
        if b == "\x00":
            ri += 1
        else:
            break
    r = r[ri:]
    si = 0
    for b in s:
        if b == "\x00":
            si += 1
        else:
            break
    s = s[si:]

    # Make positive of neg
    first = r[0]
    if first & (1 << 7) != 0:
        r = b"\x00" + r
    first = s[0]
    if first & (1 << 7) != 0:
        s = b"\x00" + s

    # Write total length
    total_len = len(r) + len(s) + 4
    sig += struct.pack("B", total_len)

    # write r
    sig += b"\x02"
    sig += struct.pack("B", len(r))
    sig += r

    # write s
    sig += b"\x02"
    sig += struct.pack("B", len(s))
    sig += s

    sig += b"\x01"
    return sig


def ser_sig_compact(r, s, recid):
    rec = struct.unpack("B", recid)[0]
    prefix = struct.pack("B", 27 + 4 + rec)

    sig = b""
    sig += prefix
    sig += r + s

    return sig


# Script helper functions
def make_p2sh(redeem_script):
    # Get the hash160 of the redeem_script
    h160 = hash160(redeem_script)

    # Build spk
    return b"\xa9\x14" + h160 + b"\x87"


def make_p2pkh(h160):
    assert len(h160) == 20
    return b"\x76\xa9\x14" + h160 + b"\x88\xac"


def make_p2wsh(witness_script):
    # Get the sha256 of the witness_script
    s256 = sha256(witness_script)

    # Build spk
    return b"\x00\x20" + s256


def is_witness(script):
    if len(script) < 4 or len(script) > 42:
        return (False, None, None)

    if script[0] != 0 and (script[0] < 81 or script[0] > 96):
        return (False, None, None)

    if script[1] + 2 == len(script):
        return (True, script[0] - 0x50 if script[0] else 0, script[2:])

    return (False, None, None)


# Objects that map to bitcoind objects, which can be serialized/deserialized

MSG_WITNESS_FLAG = 1 << 30


class COutPoint(object):
    def __init__(self, hash=0, n=0xFFFFFFFF):
        self.hash = hash
        self.n = n

    def deserialize(self, f):
        self.hash = deser_uint256(f)
        self.n = struct.unpack("<I", f.read(4))[0]

    def serialize(self):
        r = b""
        r += ser_uint256(self.hash)
        r += struct.pack("<I", self.n)
        return r

    def __repr__(self):
        return "COutPoint(hash=%064x n=%i)" % (self.hash, self.n)


class CTxIn(object):
    def __init__(self, outpoint=None, scriptSig=b"", nSequence=0):
        if outpoint is None:
            self.prevout = COutPoint()
        else:
            self.prevout = outpoint
        self.scriptSig = scriptSig
        self.nSequence = nSequence

    def deserialize(self, f):
        self.prevout = COutPoint()
        self.prevout.deserialize(f)
        self.scriptSig = deser_string(f)
        self.nSequence = struct.unpack("<I", f.read(4))[0]

    def serialize(self):
        r = b""
        r += self.prevout.serialize()
        r += ser_string(self.scriptSig)
        r += struct.pack("<I", self.nSequence)
        return r

    def __repr__(self):
        return "CTxIn(prevout=%s scriptSig=%s nSequence=%i)" % (
            repr(self.prevout),
            bytes_to_hex_str(self.scriptSig),
            self.nSequence,
        )


class CTxOut(object):
    def __init__(self, nValue=0, scriptPubKey=b""):
        self.nValue = nValue
        self.scriptPubKey = scriptPubKey

    def deserialize(self, f):
        self.nValue = struct.unpack("<q", f.read(8))[0]
        self.scriptPubKey = deser_string(f)

    def serialize(self):
        r = b""
        r += struct.pack("<q", self.nValue)
        r += ser_string(self.scriptPubKey)
        return r

    def is_p2sh(self):
        return (
            len(self.scriptPubKey) == 23
            and self.scriptPubKey[0] == 0xA9
            and self.scriptPubKey[1] == 0x14
            and self.scriptPubKey[22] == 0x87
        )

    def is_p2pkh(self):
        return (
            len(self.scriptPubKey) == 25
            and self.scriptPubKey[0] == 0x76
            and self.scriptPubKey[1] == 0xA9
            and self.scriptPubKey[2] == 0x14
            and self.scriptPubKey[23] == 0x88
            and self.scriptPubKey[24] == 0xAC
        )

    def is_p2pk(self):
        return (
            (len(self.scriptPubKey) == 35 or len(self.scriptPubKey) == 67)
            and (self.scriptPubKey[0] == 0x21 or self.scriptPubKey[0] == 0x41)
            and self.scriptPubKey[-1] == 0xAC
        )

    def is_witness(self):
        return is_witness(self.scriptPubKey)

    def __repr__(self):
        return "CTxOut(nValue=%i.%08i scriptPubKey=%s)" % (
            self.nValue,
            self.nValue,
            binascii.hexlify(self.scriptPubKey),
        )


class CScriptWitness(object):
    def __init__(self, stack=[]):
        # stack is a vector of strings
        self.stack = stack

    def __repr__(self):
        return "CScriptWitness(%s)" % (
            ",".join([bytes_to_hex_str(x) for x in self.stack])
        )

    def is_null(self):
        if self.stack:
            return False
        return True


class CTxInWitness(object):
    def __init__(self, script=CScriptWitness()):
        self.scriptWitness = script

    def deserialize(self, f):
        self.scriptWitness.stack = deser_string_vector(f)

    def serialize(self):
        return ser_string_vector(self.scriptWitness.stack)

    def __repr__(self):
        return repr(self.scriptWitness)

    def is_null(self):
        return self.scriptWitness.is_null()


class CTxWitness(object):
    def __init__(self):
        self.vtxinwit = []

    def deserialize(self, f):
        for i in range(len(self.vtxinwit)):
            self.vtxinwit[i].deserialize(f)

    def serialize(self):
        r = b""
        # This is different than the usual vector serialization --
        # we omit the length of the vector, which is required to be
        # the same length as the transaction's vin vector.
        for x in self.vtxinwit:
            r += x.serialize()
        return r

    def __repr__(self):
        return "CTxWitness(%s)" % (";".join([repr(x) for x in self.vtxinwit]))

    def is_null(self):
        for x in self.vtxinwit:
            if not x.is_null():
                return False
        return True


class CTransaction(object):
    def __init__(self, tx=None):
        if tx is None:
            self.nVersion = 1
            self.vin = []
            self.vout = []
            self.wit = CTxWitness()
            self.nLockTime = 0
            self.sha256 = None
            self.hash = None
        else:
            self.nVersion = tx.nVersion
            self.vin = copy.deepcopy(tx.vin)
            self.vout = copy.deepcopy(tx.vout)
            self.nLockTime = tx.nLockTime
            self.sha256 = tx.sha256
            self.hash = tx.hash
            self.wit = copy.deepcopy(tx.wit)

    def deserialize(self, f):
        self.nVersion = struct.unpack("<i", f.read(4))[0]
        self.vin = deser_vector(f, CTxIn)
        flags = 0
        if len(self.vin) == 0:
            flags = struct.unpack("<B", f.read(1))[0]
            # Not sure why flags can't be zero, but this
            # matches the implementation in bitcoind
            if flags != 0:
                self.vin = deser_vector(f, CTxIn)
                self.vout = deser_vector(f, CTxOut)
        else:
            self.vout = deser_vector(f, CTxOut)
        if flags != 0:
            self.wit.vtxinwit = [CTxInWitness() for i in range(len(self.vin))]
            self.wit.deserialize(f)
        self.nLockTime = struct.unpack("<I", f.read(4))[0]
        self.sha256 = None
        self.hash = None

    def serialize_without_witness(self):
        r = b""
        r += struct.pack("<i", self.nVersion)
        r += ser_vector(self.vin)
        r += ser_vector(self.vout)
        r += struct.pack("<I", self.nLockTime)
        return r

    # Only serialize with witness when explicitly called for
    def serialize_with_witness(self):
        flags = 0
        if not self.wit.is_null():
            flags |= 1
        r = b""
        r += struct.pack("<i", self.nVersion)
        if flags:
            dummy = []
            r += ser_vector(dummy)
            r += struct.pack("<B", flags)
        r += ser_vector(self.vin)
        r += ser_vector(self.vout)
        if flags & 1:
            if len(self.wit.vtxinwit) != len(self.vin):
                # vtxinwit must have the same length as vin
                self.wit.vtxinwit = self.wit.vtxinwit[: len(self.vin)]
                for i in range(len(self.wit.vtxinwit), len(self.vin)):
                    self.wit.vtxinwit.append(CTxInWitness())
            r += self.wit.serialize()
        r += struct.pack("<I", self.nLockTime)
        return r

    # Regular serialization is without witness -- must explicitly
    # call serialize_with_witness to include witness data.
    def serialize(self):
        return self.serialize_without_witness()

    # Recalculate the txid (transaction hash without witness)
    def rehash(self):
        self.sha256 = None
        self.calc_sha256()

    # We will only cache the serialization without witness in
    # self.sha256 and self.hash -- those are expected to be the txid.
    def calc_sha256(self, with_witness=False):
        if with_witness:
            # Don't cache the result, just return it
            return uint256_from_str(hash256(self.serialize_with_witness()))

        if self.sha256 is None:
            self.sha256 = uint256_from_str(hash256(self.serialize_without_witness()))
        self.hash = encode(hash256(self.serialize())[::-1], "hex_codec").decode("ascii")

    def txid(self):
        if self.sha256 is None:
            self.calc_sha256()
        return ser_uint256(self.sha256)[::-1]

    def is_valid(self):
        self.calc_sha256()
        for tout in self.vout:
            if tout.nValue < 0 or tout.nValue > 21000000 * COIN:
                return False
        return True

    def is_null(self):
        return len(self.vin) == 0 and len(self.vout) == 0

    def __repr__(self):
        return "CTransaction(nVersion=%i vin=%s vout=%s wit=%s nLockTime=%i)" % (
            self.nVersion,
            repr(self.vin),
            repr(self.vout),
            repr(self.wit),
            self.nLockTime,
        )


# global types
PSBT_GLOBAL_UNSIGNED_TX = 0x00
PSBT_GLOBAL_XPUB = 0x01
PSBT_GLOBAL_TX_VERSION = 0x02
PSBT_GLOBAL_FALLBACK_LOCKTIME = 0x03
PSBT_GLOBAL_INPUT_COUNT = 0x04
PSBT_GLOBAL_OUTPUT_COUNT = 0x05
PSBT_GLOBAL_TX_MODIFIABLE = 0x06
PSBT_GLOBAL_VERSION = 0xFB
PSBT_GLOBAL_PROPRIETARY = 0xFC

# per-input types
PSBT_IN_NON_WITNESS_UTXO = 0x00
PSBT_IN_WITNESS_UTXO = 0x01
PSBT_IN_PARTIAL_SIG = 0x02
PSBT_IN_SIGHASH_TYPE = 0x03
PSBT_IN_REDEEM_SCRIPT = 0x04
PSBT_IN_WITNESS_SCRIPT = 0x05
PSBT_IN_BIP32_DERIVATION = 0x06
PSBT_IN_FINAL_SCRIPTSIG = 0x07
PSBT_IN_FINAL_SCRIPTWITNESS = 0x08
PSBT_IN_POR_COMMITMENT = 0x09
PSBT_IN_RIPEMD160 = 0x0A
PSBT_IN_SHA256 = 0x0B
PSBT_IN_HASH160 = 0x0C
PSBT_IN_HASH256 = 0x0D
PSBT_IN_PREVIOUS_TXID = 0x0E
PSBT_IN_OUTPUT_INDEX = 0x0F
PSBT_IN_SEQUENCE = 0x10
PSBT_IN_REQUIRED_TIME_LOCKTIME = 0x11
PSBT_IN_REQUIRED_HEIGHT_LOCKTIME = 0x12
PSBT_IN_TAP_KEY_SIG = 0x13
PSBT_IN_TAP_SCRIPT_SIG = 0x14
PSBT_IN_TAP_LEAF_SCRIPT = 0x15
PSBT_IN_TAP_BIP32_DERIVATION = 0x16
PSBT_IN_TAP_INTERNAL_KEY = 0x17
PSBT_IN_TAP_MERKLE_ROOT = 0x18
PSBT_IN_PROPRIETARY = 0xFC

# per-output types
PSBT_OUT_REDEEM_SCRIPT = 0x00
PSBT_OUT_WITNESS_SCRIPT = 0x01
PSBT_OUT_BIP32_DERIVATION = 0x02
PSBT_OUT_AMOUNT = 0x03
PSBT_OUT_SCRIPT = 0x04
PSBT_OUT_TAP_INTERNAL_KEY = 0x05
PSBT_OUT_TAP_TREE = 0x06
PSBT_OUT_TAP_BIP32_DERIVATION = 0x07
PSBT_OUT_PROPRIETARY = 0xFC


class PSBTMap:
    """Class for serializing and deserializing PSBT maps"""

    def __init__(self, map=None):
        self.map = map if map is not None else {}

    # NOTE: this implementation assumes that the keytype from bip174 is always 1 byte,
    # as it detects mappings (like bip32 derivations, partial sigs, ..) based on this.
    def deserialize(self, f):
        m = {}
        while True:
            k = deser_string(f)
            if len(k) == 0:
                break
            v = deser_string(f)
            if len(k) == 1:
                k = k[0]
                assert k not in m
                m[k] = v
            else:
                typ, k = k[0], k[1:]
                if typ not in m:
                    m[typ] = {k: v}
                else:
                    m[typ][k] = v
        self.map = m

    def serialize(self):
        m = b""
        for key_type in sorted(self.map):
            psbt_val = self.map[key_type]
            if isinstance(key_type, int) and 0 <= key_type and key_type <= 255:
                key_type = bytes([key_type])
            if isinstance(psbt_val, dict):
                for key_data, val_data in psbt_val.items():
                    k = key_type + key_data
                    m += ser_compact_size(len(k)) + k
                    m += ser_compact_size(len(val_data)) + val_data
            else:
                m += ser_compact_size(len(key_type)) + key_type
                m += ser_compact_size(len(psbt_val)) + psbt_val
        m += b"\x00"
        return m


class PSBT:
    """Class for serializing and deserializing PSBTs"""

    def __init__(self, *, g=None, i=None, o=None):
        self.g = g if g is not None else PSBTMap()
        self.i = i if i is not None else []
        self.o = o if o is not None else []
        self.tx = None

    def deserialize(self, f):
        assert f.read(5) == b"psbt\xff"
        self.g = from_binary(PSBTMap, f)
        assert 0 in self.g.map
        self.tx = from_binary(CTransaction, self.g.map[0])
        self.i = [from_binary(PSBTMap, f) for _ in self.tx.vin]
        self.o = [from_binary(PSBTMap, f) for _ in self.tx.vout]
        return self

    def serialize(self):
        assert isinstance(self.g, PSBTMap)
        assert isinstance(self.i, list) and all(isinstance(x, PSBTMap) for x in self.i)
        assert isinstance(self.o, list) and all(isinstance(x, PSBTMap) for x in self.o)
        assert 0 in self.g.map
        tx = from_binary(CTransaction, self.g.map[0])
        assert len(tx.vin) == len(self.i)
        assert len(tx.vout) == len(self.o)

        psbt = [x.serialize() for x in [self.g] + self.i + self.o]
        return b"psbt\xff" + b"".join(psbt)

    def make_blank(self):
        """
        Remove all fields except for PSBT_GLOBAL_UNSIGNED_TX
        """
        for m in self.i + self.o:
            m.map.clear()

        self.g = PSBTMap(map={0: self.g.map[0]})

    def to_base64(self):
        return base64.b64encode(self.serialize()).decode("utf8")

    @classmethod
    def from_base64(cls, b64psbt):
        return from_binary(cls, base64.b64decode(b64psbt))


# Sighash serializations
def sighash_all_witness(script_code, psbt, i, acp=False):
    """
    Compute the ALL signature hash of the {psbt} 's input {i}.

    :param acp: if True, use ALL | ANYONECANPAY behaviour.
    """
    # Calculate hashPrevouts and hashSequence
    if not acp:
        prevouts_preimage = b""
        sequence_preimage = b""
        for inputs in psbt.tx.vin:
            prevouts_preimage += inputs.prevout.serialize()
            sequence_preimage += struct.pack("<I", inputs.nSequence)
        hashPrevouts = hash256(prevouts_preimage)
        hashSequence = hash256(sequence_preimage)
    else:
        hashPrevouts = b"\x00" * 32
        hashSequence = b"\x00" * 32

    # Calculate hashOutputs
    outputs_preimage = b""
    for output in psbt.tx.vout:
        outputs_preimage += output.serialize()
    hashOutputs = hash256(outputs_preimage)

    sighash_type = b"\x01\x00\x00\x00" if not acp else b"\x81\x00\x00\x00"

    # Make sighash preimage
    prev_txo = from_binary(CTxOut, psbt.i[i].map[PSBT_IN_WITNESS_UTXO])
    preimage = b""
    preimage += struct.pack("<i", psbt.tx.nVersion)
    preimage += hashPrevouts
    preimage += hashSequence
    preimage += psbt.tx.vin[i].prevout.serialize()
    preimage += ser_string(script_code)
    preimage += struct.pack("<q", prev_txo.nValue)
    preimage += struct.pack("<I", psbt.tx.vin[i].nSequence)
    preimage += hashOutputs
    preimage += struct.pack("<I", psbt.tx.nLockTime)
    preimage += sighash_type

    # hash it
    return hash256(preimage)
