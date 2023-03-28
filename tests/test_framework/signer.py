import logging
import os

from bip32 import BIP32
from bip32.utils import coincurve
from test_framework.serializations import (
    PSBT,
    sighash_all_witness,
    PSBT_IN_BIP32_DERIVATION,
    PSBT_IN_WITNESS_SCRIPT,
    PSBT_IN_PARTIAL_SIG,
)


def sign_psbt(psbt, hds):
    """Sign a transaction.

    This will fill the 'partial_sigs' field of all inputs.

    :param psbt: PSBT of the transaction to be signed.
    :param hds: the BIP32 objects to sign the transaction with.
    :returns: PSBT with a signature in each input for the given keys.
    """
    assert isinstance(psbt, PSBT)

    # Sign each input.
    for i, psbt_in in enumerate(psbt.i):
        # First, gather the needed information from the PSBT input.
        # 'hd_keypaths' is of the form {pubkey: (fingerprint (4 bytes), derivation path (n * 4 bytes))}
        fing_der = next(iter(psbt_in.map[PSBT_IN_BIP32_DERIVATION].values()))
        raw_der_path = fing_der[4:]
        der_path = [
            int.from_bytes(raw_der_path[i : i + 4], byteorder="little", signed=True)
            for i in range(0, len(raw_der_path), 4)
        ]
        script_code = psbt_in.map[PSBT_IN_WITNESS_SCRIPT]

        # Now sign the transaction for all the given keys.
        for hd in hds:
            sighash = sighash_all_witness(script_code, psbt, i)
            privkey = coincurve.PrivateKey(hd.get_privkey_from_path(der_path))
            pubkey = privkey.public_key.format()
            assert pubkey in psbt_in.map[PSBT_IN_BIP32_DERIVATION].keys(), (
                der_path,
                fing_der,
                pubkey,
                psbt_in.map[PSBT_IN_BIP32_DERIVATION].keys(),
            )
            sig = privkey.sign(sighash, hasher=None) + b"\x01"
            logging.debug(
                f"Adding signature {sig.hex()} for pubkey {pubkey.hex()} (path {der_path})"
            )
            if PSBT_IN_PARTIAL_SIG not in psbt_in.map:
                psbt_in.map[PSBT_IN_PARTIAL_SIG] = {pubkey: sig}
            else:
                psbt_in.map[PSBT_IN_PARTIAL_SIG][pubkey] = sig

    return psbt


class SingleSigner:
    def __init__(self):
        self.primary_hd = BIP32.from_seed(os.urandom(32), network="test")
        self.recovery_hd = BIP32.from_seed(os.urandom(32), network="test")

    def sign_psbt(self, psbt, recovery=False):
        """Sign a transaction.

        This will fill the 'partial_sigs' field of all inputs. Uses either the 'primary'
        'recovery' key as specified.

        :param psbt: PSBT of the transaction to be signed.
        :returns: PSBT with a signature in each input for the specified key.
        """
        return sign_psbt(psbt, [self.recovery_hd if recovery else self.primary_hd])


class MultiSigner:
    def __init__(
        self, primary_hds_count, recovery_hds_count
    ):
        self.prim_hds = [
            BIP32.from_seed(os.urandom(32), network="test")
            for _ in range(primary_hds_count)
        ]
        self.recov_hds = [
            BIP32.from_seed(os.urandom(32), network="test")
            for _ in range(recovery_hds_count)
        ]

    def sign_psbt(self, psbt, key_indices, recovery=False):
        """Sign a transaction with the keys at the specified indices."""
        hds = self.recov_hds if recovery else self.prim_hds
        hds = [hds[i] for i in key_indices]
        return sign_psbt(psbt, hds)
