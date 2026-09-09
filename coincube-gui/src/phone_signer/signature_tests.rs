use super::*;
use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::bitcoin::{
        self, bip32::Xpriv, secp256k1::Secp256k1, Amount, Network, Transaction, TxIn, TxOut,
    },
};
use std::str::FromStr;

fn fixture(tap: bool) -> (String, Psbt, [Xpriv; 2]) {
    let secp = Secp256k1::new();
    let roots = [
        Xpriv::new_master(Network::Testnet, &[1; 32]).unwrap(),
        Xpriv::new_master(Network::Testnet, &[2; 32]).unwrap(),
    ];
    let keys: Vec<_> = roots
        .iter()
        .map(|k| {
            format!(
                "[{}]{}/<0;1>/*",
                k.fingerprint(&secp),
                Xpub::from_priv(&secp, k)
            )
        })
        .collect();
    let desc = if tap {
        format!("tr({},and_v(v:pk({}),older(52560)))", keys[0], keys[1])
    } else {
        format!(
            "wsh(or_d(pk({}),and_v(v:pkh({}),older(52560))))",
            keys[0], keys[1]
        )
    };
    let descriptor = CoincubeDescriptor::from_str(&desc).unwrap();
    let derived = descriptor.receive_descriptor().derive(0.into(), &secp);
    let tx = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: Amount::from_sat(195000),
            script_pubkey: derived.script_pubkey(),
        }],
    };
    let previous = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: Amount::from_sat(200000),
            script_pubkey: derived.script_pubkey(),
        }],
    };
    let mut tx = tx;
    tx.input[0].previous_output = bitcoin::OutPoint::new(previous.compute_txid(), 0);
    tx.input[0].sequence = bitcoin::Sequence(52560);
    let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
    if !tap {
        psbt.inputs[0].non_witness_utxo = Some(previous);
    }
    psbt.inputs[0].witness_utxo = Some(TxOut {
        value: Amount::from_sat(200000),
        script_pubkey: derived.script_pubkey(),
    });
    derived.update_psbt_in(&mut psbt.inputs[0]);
    (descriptor.to_string(), psbt, roots)
}

#[test]
fn extra_valid_second_key_signature_is_rejected() {
    let (descriptor, original, roots) = fixture(true);
    let mut signed = original.clone();
    signed.sign(&roots[0], &Secp256k1::new()).unwrap();
    signed.sign(&roots[1], &Secp256k1::new()).unwrap();
    assert!(signed.inputs[0].tap_key_sig.is_some());
    assert_eq!(signed.inputs[0].tap_script_sigs.len(), 1);
    let mut target = original.clone();
    assert!(signatures::merge_verified(
        &mut target,
        &signed,
        &descriptor,
        &binding(&descriptor, &roots[0])
    )
    .is_err());
    assert_eq!(
        target, original,
        "response claiming selected key ID must not merge a valid second-key signature"
    );
}

fn binding(descriptor: &str, root: &Xpriv) -> pairing_store::SignerBinding {
    use sha2::{Digest, Sha256};
    pairing_store::SignerBinding {
        key_id: "10".into(),
        xpub: Xpub::from_priv(&Secp256k1::new(), root).to_string(),
        fingerprint: root.fingerprint(&Secp256k1::new()),
        descriptor_sha256: Sha256::digest(descriptor.as_bytes()).to_vec(),
    }
}

#[test]
fn owner_and_recovery_signatures_are_verified_for_tr_and_wsh() {
    for tap in [true, false] {
        let (descriptor, original, roots) = fixture(tap);
        for root in &roots {
            let mut signed = original.clone();
            signed.sign(root, &Secp256k1::new()).unwrap();
            let mut target = original.clone();
            signatures::merge_verified(
                &mut target,
                &signed,
                &descriptor,
                &binding(&descriptor, root),
            )
            .unwrap();
            assert_eq!(target, signed);
        }
    }
}

#[test]
fn wrong_path_invalid_signature_and_empty_delta_are_refused() {
    let (descriptor, original, roots) = fixture(true);
    let mut owner = original.clone();
    owner.sign(&roots[0], &Secp256k1::new()).unwrap();
    let mut leaf = original.clone();
    leaf.sign(&roots[1], &Secp256k1::new()).unwrap();
    let mut invalid_owner = owner.clone();
    invalid_owner.inputs[0]
        .tap_key_sig
        .as_mut()
        .unwrap()
        .signature = bitcoin::secp256k1::schnorr::Signature::from_slice(&[1; 64]).unwrap();
    let mut invalid_leaf = leaf.clone();
    invalid_leaf.inputs[0]
        .tap_script_sigs
        .values_mut()
        .next()
        .unwrap()
        .signature = bitcoin::secp256k1::schnorr::Signature::from_slice(&[1; 64]).unwrap();
    for (label, response, selected) in [
        ("leaf-only key returning keypath", &owner, 1),
        ("internal key returning unrelated leaf", &leaf, 0),
        ("invalid Schnorr keypath", &invalid_owner, 0),
        ("invalid Schnorr leaf", &invalid_leaf, 1),
        ("no signature", &original, 0),
    ] {
        let mut target = original.clone();
        assert!(
            signatures::merge_verified(
                &mut target,
                response,
                &descriptor,
                &binding(&descriptor, &roots[selected])
            )
            .is_err(),
            "{}",
            label
        );
        assert_eq!(target, original, "{label}: rejection must be atomic");
    }
}

#[test]
fn preexisting_signatures_cannot_be_removed_or_replaced() {
    for tap in [true, false] {
        let (descriptor, mut original, roots) = fixture(tap);
        original.sign(&roots[0], &Secp256k1::new()).unwrap();
        let mut complete = original.clone();
        complete.sign(&roots[1], &Secp256k1::new()).unwrap();
        let selected = binding(&descriptor, &roots[1]);
        let mut target = original.clone();
        signatures::merge_verified(&mut target, &complete, &descriptor, &selected).unwrap();
        for replace in [false, true] {
            let mut bad = complete.clone();
            if tap {
                bad.inputs[0].tap_key_sig = if replace {
                    Some(bitcoin::taproot::Signature {
                        signature: bitcoin::secp256k1::schnorr::Signature::from_slice(&[1; 64])
                            .unwrap(),
                        sighash_type: bitcoin::sighash::TapSighashType::Default,
                    })
                } else {
                    None
                };
            } else {
                let pk = *original.inputs[0].partial_sigs.keys().next().unwrap();
                if replace {
                    bad.inputs[0]
                        .partial_sigs
                        .get_mut(&pk)
                        .unwrap()
                        .sighash_type = bitcoin::sighash::EcdsaSighashType::None;
                } else {
                    bad.inputs[0].partial_sigs.remove(&pk);
                }
            }
            assert!(signatures::merge_verified(
                &mut original.clone(),
                &bad,
                &descriptor,
                &selected
            )
            .is_err());
        }
    }
}

#[test]
fn invalid_ecdsa_and_forged_derivation_are_refused() {
    let (descriptor, original, roots) = fixture(false);
    let mut signed = original.clone();
    signed.sign(&roots[0], &Secp256k1::new()).unwrap();
    let selected = binding(&descriptor, &roots[0]);
    let pk = *signed.inputs[0].partial_sigs.keys().next().unwrap();
    let mut bad = signed.clone();
    bad.inputs[0].partial_sigs.get_mut(&pk).unwrap().signature = Secp256k1::new().sign_ecdsa(
        &bitcoin::secp256k1::Message::from_digest([7; 32]),
        &roots[0].private_key,
    );
    assert!(
        signatures::merge_verified(&mut original.clone(), &bad, &descriptor, &selected).is_err()
    );
    let mut forged = original.clone();
    forged.inputs[0]
        .bip32_derivation
        .get_mut(&pk.inner)
        .unwrap()
        .0 = Fingerprint::default();
    assert!(signatures::merge_verified(&mut forged, &signed, &descriptor, &selected).is_err());
}

#[test]
fn valid_signature_is_bound_to_request_aad() {
    use crate::dir::NetworkDirectory;
    use crate::services::connect::crypto::{transport::seal_to_device, DeviceTransportKey};
    let dir = std::env::temp_dir().join(format!("lan-signature-aad-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let key = DeviceTransportKey::load_or_create(&NetworkDirectory::new(dir.clone())).unwrap();
    let (descriptor, mut original, roots) = fixture(true);
    let mut signed = original.clone();
    signed.sign(&roots[0], &Secp256k1::new()).unwrap();
    let sealed = seal_to_device(&key.public_key(), "request-A", &signed.serialize()).unwrap();
    assert!(key
        .open(
            &sealed.ephemeral_pubkey,
            &sealed.nonce,
            &sealed.ciphertext,
            "request-B"
        )
        .is_err());
    let opened = key
        .open(
            &sealed.ephemeral_pubkey,
            &sealed.nonce,
            &sealed.ciphertext,
            "request-A",
        )
        .unwrap();
    signatures::merge_verified(
        &mut original,
        &Psbt::deserialize(&opened).unwrap(),
        &descriptor,
        &binding(&descriptor, &roots[0]),
    )
    .unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn preexisting_leaf_signature_and_non_default_sighash_are_protected() {
    let (descriptor, mut original, roots) = fixture(true);
    original.sign(&roots[1], &Secp256k1::new()).unwrap();
    let mut signed = original.clone();
    signed.sign(&roots[0], &Secp256k1::new()).unwrap();
    let selected = binding(&descriptor, &roots[0]);
    signatures::merge_verified(&mut original.clone(), &signed, &descriptor, &selected).unwrap();
    let mut removed = signed.clone();
    removed.inputs[0].tap_script_sigs.clear();
    assert!(
        signatures::merge_verified(&mut original.clone(), &removed, &descriptor, &selected)
            .is_err()
    );
    signed.inputs[0]
        .tap_script_sigs
        .values_mut()
        .next()
        .unwrap()
        .signature = bitcoin::secp256k1::schnorr::Signature::from_slice(&[1; 64]).unwrap();
    assert!(
        signatures::merge_verified(&mut original.clone(), &signed, &descriptor, &selected).is_err()
    );
    let (_, mut unsigned, _) = fixture(true);
    unsigned.inputs[0].sighash_type = Some(bitcoin::sighash::TapSighashType::None.into());
    let mut non_default = unsigned.clone();
    non_default.sign(&roots[0], &Secp256k1::new()).unwrap();
    assert!(
        signatures::merge_verified(&mut unsigned, &non_default, &descriptor, &selected).is_err()
    );
}

#[test]
fn selected_key_without_descriptor_origin_is_refused() {
    let (descriptor, mut original, roots) = fixture(true);
    let mut signed = original.clone();
    signed.sign(&roots[0], &Secp256k1::new()).unwrap();
    let no_origin = descriptor.split('#').next().unwrap().replace(
        &format!("[{}]", roots[0].fingerprint(&Secp256k1::new())),
        "",
    );
    assert!(signatures::merge_verified(
        &mut original,
        &signed,
        &no_origin,
        &binding(&descriptor, &roots[0])
    )
    .is_err());
}
