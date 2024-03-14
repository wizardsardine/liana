import logging
import os
import subprocess

from bip32 import BIP32
from bip32.utils import coincurve
from test_framework.serializations import (
    PSBT,
    sighash_all_witness,
    PSBT_IN_BIP32_DERIVATION,
    PSBT_IN_WITNESS_SCRIPT,
    PSBT_IN_PARTIAL_SIG,
    PSBT_IN_TAP_KEY_SIG,
    PSBT_IN_TAP_SCRIPT_SIG,
    PSBT_IN_TAP_LEAF_SCRIPT,
    PSBT_IN_TAP_BIP32_DERIVATION,
    PSBT_IN_TAP_INTERNAL_KEY,
    PSBT_IN_TAP_MERKLE_ROOT,
)


def sign_psbt_wsh(psbt, hds):
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


def sign_psbt_taproot(psbt, hds):
    """Sign a transaction.

    This will fill the 'tap_script_sig' / 'tap_key_sig' field of all inputs.

    :param psbt: PSBT of the transaction to be signed.
    :param hds: the BIP32 objects to sign the transaction with.
    :returns: PSBT with a signature in each input for the given keys.
    """
    assert isinstance(psbt, PSBT)

    # This file is under tests/test_framework/ and we want tests/tools/taproot_signer/target/release/taproot_signer.
    bin_path = os.path.join(
        os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
        "tools",
        "taproot_signer",
        "target",
        "release",
        "taproot_signer",
    )
    if not os.path.exists(bin_path):
        raise Exception(
            "Please compile the Taproot signer under tests/tools using 'cargo bin --release'."
        )

    psbt_str = psbt.to_base64()
    for hd in hds:
        xprv = hd.get_xpriv()
        proc = subprocess.run([bin_path, psbt_str, xprv], capture_output=True, check=True)
        psbt_str = proc.stdout.decode("utf-8")

    return PSBT.from_base64(psbt_str)


class SingleSigner:
    """Assumes a simple 1-primary path 1-recovery path Liana descriptor."""

    def __init__(self, is_taproot):
        self.primary_hd = BIP32.from_seed(os.urandom(32), network="test")
        self.recovery_hd = BIP32.from_seed(os.urandom(32), network="test")
        self.is_taproot = is_taproot

    def sign_psbt(self, psbt, recovery=False):
        """Sign a transaction.

        This will fill the 'partial_sigs' field of all inputs. Uses either the 'primary'
        'recovery' key as specified.

        :param psbt: PSBT of the transaction to be signed.
        :returns: PSBT with a signature in each input for the specified key.
        """
        assert isinstance(recovery, bool)
        if self.is_taproot:
            return sign_psbt_taproot(
                psbt, [self.recovery_hd if recovery else self.primary_hd]
            )
        return sign_psbt_wsh(psbt, [self.recovery_hd if recovery else self.primary_hd])


class MultiSigner:
    """A signer that has multiple keys and may have multiple recovery path."""

    def __init__(self, primary_hds_count, recovery_hds_counts, is_taproot):
        self.prim_hds = [
            BIP32.from_seed(os.urandom(32), network="test")
            for _ in range(primary_hds_count)
        ]
        self.recov_hds = {}
        for timelock, count in recovery_hds_counts.items():
            self.recov_hds[timelock] = [
                BIP32.from_seed(os.urandom(32), network="test") for _ in range(count)
            ]
        self.is_taproot = is_taproot

    def sign_psbt(self, psbt, key_indices):
        """Sign a transaction with the keys at the specified indices.

        The key indices may be specified as a mapping from timelock value to list of
        indices to sign with the keys of a specific recovery path.
        """
        if isinstance(key_indices, dict):
            hds = [
                self.recov_hds[timelock][i]
                for timelock, indices in key_indices.items()
                for i in indices
            ]
        else:
            hds = [self.prim_hds[i] for i in key_indices]
        if self.is_taproot:
            return sign_psbt_taproot(psbt, hds)
        return sign_psbt_wsh(psbt, hds)
