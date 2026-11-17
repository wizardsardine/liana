//! Whether a *confirmed* Bitcoin Blake2b transaction could also be valid on
//! Bitcoin, judged from the witness its block commits to.
//!
//! Split coins' signature method (`coincube-gui/src/services/split_coins.rs`)
//! spends both-chain coins on Bitcoin Blake2b with `ALL|UNIFIED` (`0x21`)
//! signatures. Bitcoin hashes a `0x21` signature with the BIP 143 digest, not
//! the unified one, so it fails there, and the spend can't be replayed onto
//! Bitcoin. A txid commits to no witness, though, and the same Vault can sign
//! the same transaction the legacy way too; only the witness the confirming
//! block commits to says which one the chain holds
//! ([`crate::btcb2_block::confirmed_transaction`]).
//!
//! [`verify_replay_protected`] accepts a transaction only if, on every input:
//!
//! - the input is native P2WSH and its witness ends with the witness script
//!   the spent output commits to;
//! - that script is sane Miniscript whose every satisfaction needs a
//!   signature (Miniscript's *safe* property, which every Vault path has);
//! - every witness element shaped like a signature carries `0x21` and is a
//!   valid ECDSA signature over the unified digest by a key of the script;
//! - and there is at least one.
//!
//! On Bitcoin every one of those signatures fails, and no other element can
//! pass a signature check, so no branch of the script can be satisfied: the
//! input, and with it the transaction, is invalid there. A witness that mixes
//! in a legacy signature is refused even where it might still be
//! protected: this is evidence, and the rule is kept simple enough to check.
//!
//! Nothing here establishes that the transaction is confirmed, or where:
//! the caller authenticates the block and its depth, and supplies the spent
//! outputs from transactions it has authenticated by txid.

use std::{error, fmt};

use miniscript::{
    bitcoin::{secp256k1, PublicKey, ScriptBuf, Transaction, TxOut},
    ExtParams, Miniscript, Segwitv0,
};

use crate::{
    unified_finalize::looks_like_der_signature,
    unified_sighash::{UnifiedSighashCache, SCRIPT_TYPE_WITNESS_V0},
    unified_signing::script_uses_key,
};

/// `SIGHASH_ALL | SIGHASH_UNIFIED`.
const UNIFIED_SIGHASH_ALL: u8 = 0x21;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmedUnifiedError {
    /// `spent_outputs` does not list one output per input.
    PrevoutsLengthMismatch,
    UnsupportedPrevoutScript {
        input: usize,
    },
    EmptyWitness {
        input: usize,
    },
    WitnessScriptMismatch {
        input: usize,
    },
    UnsupportedWitnessScript {
        input: usize,
        reason: String,
    },
    /// Some satisfaction of the script needs no signature.
    UnsafeScript {
        input: usize,
    },
    /// A signature with a sighash other than `ALL|UNIFIED`.
    NotUnified {
        input: usize,
        sighash: u8,
    },
    /// A `0x21` signature that no key of the script made over this input's
    /// unified digest.
    InvalidSignature {
        input: usize,
    },
    /// The input carries no signature at all.
    Unsigned {
        input: usize,
    },
}

impl fmt::Display for ConfirmedUnifiedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PrevoutsLengthMismatch => write!(f, "one spent output per input is required"),
            Self::UnsupportedPrevoutScript { input } => {
                write!(f, "input {} does not spend native P2WSH", input)
            }
            Self::EmptyWitness { input } => write!(f, "input {} has no witness", input),
            Self::WitnessScriptMismatch { input } => {
                write!(
                    f,
                    "input {} witness script is not the spent output's",
                    input
                )
            }
            Self::UnsupportedWitnessScript { input, reason } => {
                write!(
                    f,
                    "input {} witness script is unsupported: {}",
                    input, reason
                )
            }
            Self::UnsafeScript { input } => {
                write!(f, "input {} can be satisfied without a signature", input)
            }
            Self::NotUnified { input, sighash } => {
                write!(
                    f,
                    "input {} has a signature with sighash {:#04x}",
                    input, sighash
                )
            }
            Self::InvalidSignature { input } => {
                write!(f, "input {} has an invalid unified signature", input)
            }
            Self::Unsigned { input } => write!(f, "input {} has no signature", input),
        }
    }
}

impl error::Error for ConfirmedUnifiedError {}

/// Check that every input of `transaction` is signed only with valid
/// `ALL|UNIFIED` signatures (see the module docs), so that Bitcoin rejects
/// it. `spent_outputs` are the outputs it spends, in input order. Returns
/// how many signatures were checked.
pub fn verify_replay_protected<C: secp256k1::Verification>(
    transaction: &Transaction,
    spent_outputs: &[TxOut],
    secp: &secp256k1::Secp256k1<C>,
) -> Result<usize, ConfirmedUnifiedError> {
    if spent_outputs.len() != transaction.input.len() {
        return Err(ConfirmedUnifiedError::PrevoutsLengthMismatch);
    }
    let cache = UnifiedSighashCache::new(transaction, spent_outputs)
        .map_err(|_| ConfirmedUnifiedError::PrevoutsLengthMismatch)?;
    let mut checked = 0;
    for (input, (txin, spent)) in transaction.input.iter().zip(spent_outputs).enumerate() {
        if !spent.script_pubkey.is_p2wsh() {
            return Err(ConfirmedUnifiedError::UnsupportedPrevoutScript { input });
        }
        let elements: Vec<&[u8]> = txin.witness.iter().collect();
        let (script, stack) = elements
            .split_last()
            .ok_or(ConfirmedUnifiedError::EmptyWitness { input })?;
        let witness_script = ScriptBuf::from(script.to_vec());
        if witness_script.to_p2wsh() != spent.script_pubkey {
            return Err(ConfirmedUnifiedError::WitnessScriptMismatch { input });
        }
        let miniscript = Miniscript::<PublicKey, Segwitv0>::parse_with_ext(
            &witness_script,
            &ExtParams::sane().raw_pkh(),
        )
        .map_err(|err| ConfirmedUnifiedError::UnsupportedWitnessScript {
            input,
            reason: err.to_string(),
        })?;
        // Sane parsing already refuses a sigless branch; this stands behind
        // it should the parameters ever change.
        if !miniscript.ty.mall.safe {
            return Err(ConfirmedUnifiedError::UnsafeScript { input });
        }
        // Script keys, plus keys the witness reveals for raw `pkh` leaves.
        let mut keys: Vec<PublicKey> = miniscript.iter_pk().collect();
        keys.extend(
            stack
                .iter()
                .filter_map(|element| PublicKey::from_slice(element).ok())
                .filter(|key| script_uses_key(&miniscript, key)),
        );
        let digest = cache
            .signature_hash(
                input,
                UNIFIED_SIGHASH_ALL,
                SCRIPT_TYPE_WITNESS_V0,
                &witness_script,
            )
            .map_err(|_| ConfirmedUnifiedError::PrevoutsLengthMismatch)?;
        let message = secp256k1::Message::from_digest(digest);
        let signatures = verify_witness_signatures(stack, &keys, &message, secp, input)?;
        checked += signatures;
    }
    Ok(checked)
}

fn verify_witness_signatures<C: secp256k1::Verification>(
    stack: &[&[u8]],
    keys: &[PublicKey],
    message: &secp256k1::Message,
    secp: &secp256k1::Secp256k1<C>,
    input: usize,
) -> Result<usize, ConfirmedUnifiedError> {
    let mut signatures = 0;
    for element in stack
        .iter()
        .filter(|element| looks_like_der_signature(element))
    {
        let (sighash, der) = element.split_last().expect("not empty");
        if *sighash != UNIFIED_SIGHASH_ALL {
            return Err(ConfirmedUnifiedError::NotUnified {
                input,
                sighash: *sighash,
            });
        }
        let mut signature = secp256k1::ecdsa::Signature::from_der(der)
            .map_err(|_| ConfirmedUnifiedError::InvalidSignature { input })?;
        // Consensus does not require low S; the digest is what matters.
        signature.normalize_s();
        if !keys
            .iter()
            .any(|key| secp.verify_ecdsa(message, &signature, &key.inner).is_ok())
        {
            return Err(ConfirmedUnifiedError::InvalidSignature { input });
        }
        signatures += 1;
    }
    if signatures == 0 {
        return Err(ConfirmedUnifiedError::Unsigned { input });
    }
    Ok(signatures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniscript::bitcoin::{
        absolute,
        hashes::{sha256d, Hash},
        secp256k1::{Secp256k1, SecretKey},
        sighash::{EcdsaSighashType, SighashCache},
        transaction, Amount, OutPoint, Sequence, TxIn, Txid, Witness,
    };

    struct Fixture {
        secp: Secp256k1<secp256k1::All>,
        keys: Vec<SecretKey>,
        script: ScriptBuf,
        tx: Transaction,
        spent: Vec<TxOut>,
    }

    fn public(secp: &Secp256k1<secp256k1::All>, key: &SecretKey) -> PublicKey {
        PublicKey::new(secp256k1::PublicKey::from_secret_key(secp, key))
    }

    /// A two-input spend of `policy`, with `@0`, `@1`… standing for keys.
    fn fixture(policy: &str) -> Fixture {
        fixture_with(policy, &ExtParams::sane())
    }

    fn fixture_with(policy: &str, params: &ExtParams) -> Fixture {
        let secp = Secp256k1::new();
        let keys: Vec<SecretKey> = (1..=4u8)
            .map(|n| SecretKey::from_slice(&[n; 32]).unwrap())
            .collect();
        let mut text = policy.to_string();
        for (index, key) in keys.iter().enumerate() {
            text = text.replace(&format!("@{}", index), &public(&secp, key).to_string());
        }
        let script = Miniscript::<PublicKey, Segwitv0>::from_str_ext(&text, params)
            .unwrap()
            .encode();
        let spent = vec![
            TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: script.to_p2wsh(),
            };
            2
        ];
        let tx = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: (0..2u32)
                .map(|vout| TxIn {
                    previous_output: OutPoint::new(Txid::all_zeros(), vout),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    ..TxIn::default()
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(99_000),
                script_pubkey: script.to_p2wsh(),
            }],
        };
        Fixture {
            secp,
            keys,
            script,
            tx,
            spent,
        }
    }

    impl Fixture {
        fn unified(&self, input: usize, key: usize) -> Vec<u8> {
            let digest = UnifiedSighashCache::new(&self.tx, &self.spent)
                .unwrap()
                .signature_hash(
                    input,
                    UNIFIED_SIGHASH_ALL,
                    SCRIPT_TYPE_WITNESS_V0,
                    &self.script,
                )
                .unwrap();
            self.sign(digest, key, UNIFIED_SIGHASH_ALL)
        }
        /// BIP 143 `SIGHASH_ALL`, what Bitcoin accepts.
        fn legacy(&self, input: usize, key: usize) -> Vec<u8> {
            let digest = SighashCache::new(&self.tx)
                .p2wsh_signature_hash(
                    input,
                    &self.script,
                    self.spent[input].value,
                    EcdsaSighashType::All,
                )
                .unwrap();
            self.sign(digest.to_byte_array(), key, 0x01)
        }
        /// BIP 143 with hash type `0x21`: what Bitcoin computes for a `0x21`
        /// signature, so a signature over it would pass there.
        fn legacy_0x21(&self, input: usize, key: usize) -> Vec<u8> {
            // rust-bitcoin normalizes a non-standard type to `ALL`, so encode
            // as `ALL` and put the real 4-byte type back, as Core hashes it.
            let mut data = Vec::new();
            SighashCache::new(&self.tx)
                .segwit_v0_encode_signing_data_to(
                    &mut data,
                    input,
                    &self.script,
                    self.spent[input].value,
                    EcdsaSighashType::All,
                )
                .unwrap();
            // The encoding is the one Bitcoin signs for `ALL`…
            let all = SighashCache::new(&self.tx)
                .p2wsh_signature_hash(
                    input,
                    &self.script,
                    self.spent[input].value,
                    EcdsaSighashType::All,
                )
                .unwrap();
            assert_eq!(
                sha256d::Hash::hash(&data).to_byte_array(),
                all.to_byte_array()
            );
            // …but for its last four bytes.
            let at = data.len() - 4;
            data[at..].copy_from_slice(&u32::from(UNIFIED_SIGHASH_ALL).to_le_bytes());
            let digest = sha256d::Hash::hash(&data).to_byte_array();
            self.sign(digest, key, UNIFIED_SIGHASH_ALL)
        }
        fn sign(&self, digest: [u8; 32], key: usize, sighash: u8) -> Vec<u8> {
            let message = secp256k1::Message::from_digest(digest);
            let mut bytes = self
                .secp
                .sign_ecdsa(&message, &self.keys[key])
                .serialize_der()
                .to_vec();
            bytes.push(sighash);
            bytes
        }
        fn key(&self, key: usize) -> Vec<u8> {
            public(&self.secp, &self.keys[key]).to_bytes()
        }
        fn set(&mut self, input: usize, stack: Vec<Vec<u8>>) {
            let mut elements = stack;
            elements.push(self.script.to_bytes());
            self.tx.input[input].witness = Witness::from_slice(&elements);
        }
        fn verify(&self) -> Result<usize, ConfirmedUnifiedError> {
            verify_replay_protected(&self.tx, &self.spent, &self.secp)
        }
    }

    const MULTI: &str = "multi(2,@0,@1,@2)";
    const VAULT: &str = "or_d(multi(2,@0,@1),and_v(v:pkh(@2),older(52560)))";

    fn signed_multi() -> Fixture {
        let mut f = fixture(MULTI);
        for input in 0..2 {
            let stack = vec![vec![], f.unified(input, 0), f.unified(input, 2)];
            f.set(input, stack);
        }
        f
    }

    #[test]
    fn a_fully_unified_spend_is_replay_protected() {
        assert_eq!(signed_multi().verify(), Ok(4));
    }

    #[test]
    fn a_recovery_path_signature_counts() {
        // Signatures are only made after the witness scripts are fixed, so the
        // digest commits to the parsed-back raw `pkh` leaf.
        let mut f = fixture(VAULT);
        f.tx.input[1].sequence = Sequence::from_height(52560);
        f.set(1, vec![vec![]; 3]);
        let stack = vec![vec![], f.unified(0, 0), f.unified(0, 1)];
        f.set(0, stack);
        let stack = vec![f.unified(1, 2), f.key(2), vec![], vec![], vec![]];
        f.set(1, stack);
        assert_eq!(f.verify(), Ok(3));
    }

    #[test]
    fn a_legacy_signature_anywhere_refuses() {
        let mut f = signed_multi();
        let stack = vec![vec![], f.unified(1, 0), f.legacy(1, 2)];
        f.set(1, stack);
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::NotUnified {
                input: 1,
                sighash: 0x01
            })
        );
    }

    #[test]
    fn a_unified_byte_on_the_wrong_digest_refuses() {
        // `0x21` on a signature over the BIP 143 digest: what Bitcoin would
        // check, so it must not pass as unified.
        let mut f = signed_multi();
        let mut forged = f.legacy(0, 0);
        *forged.last_mut().unwrap() = UNIFIED_SIGHASH_ALL;
        let stack = vec![vec![], forged, f.unified(0, 2)];
        f.set(0, stack);
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::InvalidSignature { input: 0 })
        );
        // Bitcoin's own digest for hash type `0x21`.
        let mut f = signed_multi();
        let stack = vec![vec![], f.legacy_0x21(0, 0), f.unified(0, 2)];
        f.set(0, stack);
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::InvalidSignature { input: 0 })
        );
        // Another input's signature, and a key outside the script.
        let mut f = signed_multi();
        let stack = vec![vec![], f.unified(1, 0), f.unified(0, 2)];
        f.set(0, stack);
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::InvalidSignature { input: 0 })
        );
        let mut f = signed_multi();
        let stack = vec![vec![], f.unified(0, 0), f.unified(0, 3)];
        f.set(0, stack);
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::InvalidSignature { input: 0 })
        );
    }

    #[test]
    fn the_spent_outputs_are_committed_to() {
        let mut f = signed_multi();
        f.spent[1].value = Amount::from_sat(50_001);
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::InvalidSignature { input: 0 })
        );
        let mut f = signed_multi();
        f.spent[0].script_pubkey = fixture(VAULT).script.to_p2wsh();
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::WitnessScriptMismatch { input: 0 })
        );
        let f = signed_multi();
        assert_eq!(
            verify_replay_protected(&f.tx, &f.spent[..1], &f.secp),
            Err(ConfirmedUnifiedError::PrevoutsLengthMismatch)
        );
    }

    #[test]
    fn an_input_without_a_signature_refuses() {
        let mut f = signed_multi();
        f.tx.input[1].witness = Witness::new();
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::EmptyWitness { input: 1 })
        );
        let mut f = signed_multi();
        f.set(1, vec![vec![], vec![], vec![]]);
        assert_eq!(
            f.verify(),
            Err(ConfirmedUnifiedError::Unsigned { input: 1 })
        );
    }

    #[test]
    fn a_script_satisfiable_without_a_signature_refuses() {
        // Sane Miniscript already refuses a sigless branch; the safe check
        // stands behind it.
        let mut f = fixture_with("or_d(multi(2,@0,@1),older(52560))", &ExtParams::insane());
        for input in 0..2 {
            let stack = vec![vec![], f.unified(input, 0), f.unified(input, 1)];
            f.set(input, stack);
        }
        assert!(matches!(
            f.verify(),
            Err(ConfirmedUnifiedError::UnsupportedWitnessScript { input: 0, .. })
        ));
    }
}
