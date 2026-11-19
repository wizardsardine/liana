use super::*;
use coincube_core::{
    chain::ChainId,
    imported_keys::{verify_imported_account, VerifiedImportedKey},
    imported_spend::{create_imported_spend, finalize_imported_spend},
    miniscript::bitcoin::{
        self,
        absolute::LockTime,
        bip32::{ChildNumber, DerivationPath, Xpriv, Xpub},
        psbt::Psbt,
        secp256k1::Secp256k1,
        sighash::EcdsaSighashType,
        Address, Amount, Network, OutPoint, ScriptBuf, Transaction, TxIn, TxOut,
    },
    miniscript::{DescriptorPublicKey, ForEachKey},
    spend::{AddrInfo, CandidateCoin, SpendOutputAddress, SpendTxFees, TxGetter},
};
use std::str::FromStr;
use zeroize::Zeroizing;

fn account(seed: u8, path: &str) -> (String, Xpriv, String) {
    let secp = Secp256k1::new();
    let phrase = bip39::Mnemonic::from_entropy(&[seed; 16])
        .unwrap()
        .to_string();
    let root = Xpriv::new_master(
        Network::Regtest,
        &bip39::Mnemonic::parse(&phrase).unwrap().to_seed(""),
    )
    .unwrap();
    let path = DerivationPath::from_str(path).unwrap();
    let xpub = Xpub::from_priv(&secp, &root.derive_priv(&secp, &path).unwrap());
    let key = format!(
        "[{}/{}]{xpub}",
        root.fingerprint(&secp),
        path.to_string().trim_start_matches("m/")
    );
    (phrase, root, key)
}

fn verified(wallet: &ImportedWalletDescriptor, phrase: &str) -> VerifiedImportedKey {
    let mut paths = Vec::new();
    wallet.receive_descriptor().for_each_key(|key| {
        if let DescriptorPublicKey::XPub(xpub) = key {
            if let Some((_, path)) = &xpub.origin {
                paths.push(path.clone());
            }
        }
        true
    });
    paths
        .into_iter()
        .find_map(|path| {
            verify_imported_account(
                wallet,
                &path,
                Zeroizing::new(phrase.into()),
                Zeroizing::new(String::new()),
            )
            .ok()
        })
        .expect("synthetic phrase matches one exact account")
}

fn fixture(kind: &str) -> (ImportedWalletDescriptor, Vec<String>, Psbt) {
    fixture_branch(kind, false)
}

fn two_input_fixture(kind: &str) -> (ImportedWalletDescriptor, Vec<String>, Psbt) {
    let (wallet, phrases, first) = fixture(kind);
    let mut construction = first.clone();
    let mut second_prev = first.inputs[0].non_witness_utxo.clone().unwrap();
    second_prev.output[0].value += Amount::from_sat(1);
    let mut second_txin = first.unsigned_tx.input[0].clone();
    second_txin.previous_output.txid = second_prev.compute_txid();
    construction.unsigned_tx.input.push(second_txin);
    let mut second_input = first.inputs[0].clone();
    second_input.non_witness_utxo = Some(second_prev);
    second_input.witness_utxo.as_mut().unwrap().value += Amount::from_sat(1);
    construction.inputs.push(second_input);
    construction.unsigned_tx.output[1].value += Amount::from_sat(150_001);
    (wallet, phrases, construction)
}

fn fixture_branch(kind: &str, is_change: bool) -> (ImportedWalletDescriptor, Vec<String>, Psbt) {
    let multisig = kind.starts_with("wsh");
    let path = if multisig {
        "m/48h/1h/0h/2h"
    } else {
        "m/84h/1h/0h"
    };
    let accounts: Vec<_> = (1..=if multisig { 3 } else { 1 })
        .map(|seed| account(seed, path))
        .collect();
    let keys: Vec<_> = accounts.iter().map(|(_, _, key)| key.as_str()).collect();
    let render = |branch| match kind {
        "pkh" => format!("pkh({}/{branch}/*)", keys[0]),
        "wpkh" => format!("wpkh({}/{branch}/*)", keys[0]),
        "sh(wpkh)" => format!("sh(wpkh({}/{branch}/*))", keys[0]),
        "wsh(multi)" => format!(
            "wsh(multi(2,{}))",
            keys.iter()
                .map(|key| format!("{key}/{branch}/*"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        "wsh(sortedmulti)" => format!(
            "wsh(sortedmulti(2,{}))",
            keys.iter()
                .map(|key| format!("{key}/{branch}/*"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => unreachable!(),
    };
    let wallet =
        ImportedWalletDescriptor::parse(&render(0), Some(&render(1)), ChainId::Regtest).unwrap();
    let receive = (if is_change {
        wallet.change_descriptor()
    } else {
        wallet.receive_descriptor()
    })
    .at_derivation_index(0)
    .unwrap()
    .script_pubkey();
    let previous = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: Amount::from_sat(150_000),
            script_pubkey: receive,
        }],
    };
    struct OneTx(Transaction);
    impl TxGetter for OneTx {
        fn get_tx(&mut self, txid: &bitcoin::Txid) -> Option<Transaction> {
            (self.0.compute_txid() == *txid).then(|| self.0.clone())
        }
    }
    let address = |script: ScriptBuf| Address::from_script(&script, Network::Regtest).unwrap();
    let candidate = CandidateCoin {
        outpoint: OutPoint {
            txid: previous.compute_txid(),
            vout: 0,
        },
        amount: Amount::from_sat(150_000),
        deriv_index: ChildNumber::Normal { index: 0 },
        is_change,
        must_select: true,
        sequence: None,
        ancestor_info: None,
    };
    let change = SpendOutputAddress {
        addr: address(
            wallet
                .change_descriptor()
                .at_derivation_index(1)
                .unwrap()
                .script_pubkey(),
        ),
        info: Some(AddrInfo {
            index: ChildNumber::Normal { index: 1 },
            is_change: true,
        }),
    };
    let (_, recipient, _) = account(99, "m/84h/1h/0h");
    let recipient_key = Xpub::from_priv(
        &Secp256k1::new(),
        &recipient
            .derive_priv(
                &Secp256k1::new(),
                &DerivationPath::from_str("m/84h/1h/0h").unwrap(),
            )
            .unwrap(),
    );
    let recipient_script =
        coincube_core::miniscript::Descriptor::<DescriptorPublicKey>::from_str(&format!(
            "wpkh([{}/84h/1h/0h]{recipient_key}/0/*)",
            recipient.fingerprint(&Secp256k1::new())
        ))
        .unwrap()
        .at_derivation_index(0)
        .unwrap()
        .script_pubkey();
    let destination = SpendOutputAddress {
        addr: address(recipient_script),
        info: None,
    };
    let psbt = create_imported_spend(
        &wallet,
        &Secp256k1::verification_only(),
        &mut OneTx(previous),
        &[(destination, Amount::from_sat(50_000))],
        &[candidate],
        SpendTxFees::Regular(5),
        change,
        LockTime::ZERO,
    )
    .unwrap()
    .psbt;
    (
        wallet,
        accounts.into_iter().map(|(phrase, _, _)| phrase).collect(),
        psbt,
    )
}

#[test]
fn all_supported_shapes_sign_and_finalize_with_exact_wallet_keys() {
    let secp = Secp256k1::new();
    for kind in ["pkh", "wpkh", "sh(wpkh)", "wsh(multi)", "wsh(sortedmulti)"] {
        for is_change in [false, true] {
            let (wallet, roots, original) = fixture_branch(kind, is_change);
            let wallet_id = wallet.identity(wallet.chain()).unwrap();
            let first =
                sign_imported_psbt(&wallet, &original, &original, &verified(&wallet, &roots[0]))
                    .unwrap();
            let signed = if kind.starts_with("wsh") {
                sign_imported_psbt(&wallet, &original, &first, &verified(&wallet, &roots[1]))
                    .unwrap()
            } else {
                first
            };
            let finalized = finalize_imported_spend(&wallet, &original, &signed, &secp).unwrap();
            assert!(finalized.fee().to_sat() > 0, "{}", kind);
            assert_eq!(
                wallet.identity(wallet.chain()).unwrap(),
                wallet_id,
                "{kind}"
            );
            assert_eq!(original.inputs[0].partial_sigs.len(), 0, "{kind}");
            assert_eq!(
                signed.inputs[0].partial_sigs.len(),
                if kind.starts_with("wsh") { 2 } else { 1 },
                "{kind}"
            );
        }
    }
}

#[test]
fn rejects_wrong_root_path_foreign_input_and_changed_economics() {
    let (wallet, roots, original) = fixture("wpkh");
    let wrong = bip39::Mnemonic::from_entropy(&[77; 16])
        .unwrap()
        .to_string();
    assert!(matches!(
        verify_imported_account(
            &wallet,
            &DerivationPath::from_str("m/84h/1h/0h").unwrap(),
            Zeroizing::new(wrong),
            Zeroizing::new(String::new())
        ),
        Err(ImportedSignError::KeyDoesNotMatchWallet)
    ));
    let change_purpose =
        |descriptor: &coincube_core::miniscript::Descriptor<DescriptorPublicKey>| {
            descriptor
                .to_string()
                .split('#')
                .next()
                .unwrap()
                .replace("/84h", "/49h")
                .replace("/84'", "/49'")
        };
    let changed_origin_receive = change_purpose(wallet.receive_descriptor());
    let changed_origin_change = change_purpose(wallet.change_descriptor());
    assert!(changed_origin_receive.contains("/49"));
    let changed_origin = ImportedWalletDescriptor::parse(
        &changed_origin_receive,
        Some(&changed_origin_change),
        ChainId::Regtest,
    )
    .unwrap();
    assert!(matches!(
        verify_imported_account(
            &changed_origin,
            &DerivationPath::from_str("m/84h/1h/0h").unwrap(),
            Zeroizing::new(roots[0].clone()),
            Zeroizing::new(String::new())
        ),
        Err(ImportedSignError::KeyDoesNotMatchWallet)
    ));

    let mut wrong_path = original.clone();
    for (_, path) in wrong_path.inputs[0].bip32_derivation.values_mut() {
        *path = DerivationPath::from_str("m/84h/1h/0h/0/77").unwrap();
    }
    assert!(sign_imported_psbt(
        &wallet,
        &original,
        &wrong_path,
        &verified(&wallet, &roots[0])
    )
    .is_err());

    let mut foreign = original.clone();
    foreign.unsigned_tx.input[0].previous_output.vout = 99;
    assert!(
        sign_imported_psbt(&wallet, &original, &foreign, &verified(&wallet, &roots[0])).is_err()
    );

    let signed =
        sign_imported_psbt(&wallet, &original, &original, &verified(&wallet, &roots[0])).unwrap();
    for output in [0, 1] {
        let mut changed = signed.clone();
        changed.unsigned_tx.output[output].value -= Amount::from_sat(1);
        assert!(
            sign_imported_psbt(&wallet, &original, &changed, &verified(&wallet, &roots[0]))
                .is_err()
        );
    }
    let mut changed = signed.clone();
    changed.inputs[0].non_witness_utxo.as_mut().unwrap().output[0].value -= Amount::from_sat(1);
    assert!(
        sign_imported_psbt(&wallet, &original, &changed, &verified(&wallet, &roots[0])).is_err()
    );
    let mut changed = signed.clone();
    changed.inputs[0]
        .partial_sigs
        .values_mut()
        .next()
        .unwrap()
        .sighash_type = EcdsaSighashType::None;
    assert!(
        sign_imported_psbt(&wallet, &original, &changed, &verified(&wallet, &roots[0])).is_err()
    );

    let mut malformed = original.clone();
    malformed.inputs[0].non_witness_utxo = None;
    assert!(sign_imported_psbt(
        &wallet,
        &malformed,
        &malformed,
        &verified(&wallet, &roots[0])
    )
    .is_err());
    assert!(malformed.inputs[0].partial_sigs.is_empty());
}

#[test]
fn account_authority_rejects_wrong_passphrase_account_policy_and_chain() {
    let (wallet, phrases, original) = fixture("wpkh");
    let phrase = &phrases[0];
    let path = DerivationPath::from_str("m/84h/1h/0h").unwrap();
    assert!(matches!(
        verify_imported_account(
            &wallet,
            &path,
            Zeroizing::new(phrase.clone()),
            Zeroizing::new("wrong passphrase".into()),
        ),
        Err(ImportedSignError::KeyDoesNotMatchWallet)
    ));
    assert!(matches!(
        verify_imported_account(
            &wallet,
            &DerivationPath::from_str("m/84h/1h/1h").unwrap(),
            Zeroizing::new(phrase.clone()),
            Zeroizing::new(String::new()),
        ),
        Err(ImportedSignError::KeyDoesNotMatchWallet)
    ));
    let key = verified(&wallet, phrase);
    let receive = wallet.receive_descriptor().to_string();
    let change = wallet.change_descriptor().to_string();
    fn no_checksum(value: &str) -> &str {
        value.split('#').next().unwrap()
    }
    let other_policy = ImportedWalletDescriptor::parse(
        &format!("sh({})", no_checksum(&receive)),
        Some(&format!("sh({})", no_checksum(&change))),
        ChainId::Regtest,
    )
    .unwrap();
    assert!(matches!(
        sign_imported_psbt(&other_policy, &original, &original, &key),
        Err(ImportedSignError::WrongWallet)
    ));
    let other_chain = ImportedWalletDescriptor::parse(
        no_checksum(&receive),
        Some(no_checksum(&change)),
        ChainId::Testnet,
    )
    .unwrap();
    assert!(matches!(
        sign_imported_psbt(&other_chain, &original, &original, &key),
        Err(ImportedSignError::WrongWallet)
    ));
    let fork = ImportedWalletDescriptor::parse(
        no_checksum(&receive),
        Some(no_checksum(&change)),
        ChainId::BitcoinBlake2bTestnet4,
    )
    .unwrap();
    // A Bitcoin Blake2b import stores its words like any other chain
    // (Robert, 2026-11-17), but the key is scoped to that chain's wallet:
    // it cannot sign for the Bitcoin import of the same descriptors.
    let fork_key = verify_imported_account(
        &fork,
        &path,
        Zeroizing::new(phrase.clone()),
        Zeroizing::new(String::new()),
    )
    .expect("a Bitcoin Blake2b import accepts its recovery words");
    assert!(matches!(
        sign_imported_psbt(&wallet, &original, &original, &fork_key),
        Err(ImportedSignError::WrongWallet)
    ));
    assert!(original.inputs[0].partial_sigs.is_empty());
}

#[test]
fn same_account_preserves_nondefault_nonce_and_foreign_cosigner_while_signing_second_input() {
    let (wallet, phrases, original) = two_input_fixture("wsh(multi)");
    let secp = Secp256k1::new();
    let mut current = sign_imported_psbt(
        &wallet,
        &original,
        &original,
        &verified(&wallet, &phrases[1]),
    )
    .unwrap();
    let foreign_a = current.inputs[0].partial_sigs.clone();
    let foreign_b = current.inputs[1].partial_sigs.clone();
    assert_eq!(foreign_a.len(), 1);
    assert_eq!(foreign_b.len(), 1);

    let root = Xpriv::new_master(
        Network::Regtest,
        &bip39::Mnemonic::parse(&phrases[0]).unwrap().to_seed(""),
    )
    .unwrap();
    let fingerprint = root.fingerprint(&secp);
    let (_, (_, child_path)) = current.inputs[0]
        .bip32_derivation
        .iter()
        .find(|(_, (fp, _))| *fp == fingerprint)
        .unwrap();
    let private = root.derive_priv(&secp, child_path).unwrap();
    let public = private.to_priv().public_key(&secp);
    let mut cache = bitcoin::sighash::SighashCache::new(&current.unsigned_tx);
    let (message, sighash_type) = current.sighash_ecdsa(0, &mut cache).unwrap();
    let nondefault = bitcoin::ecdsa::Signature {
        signature: secp.sign_ecdsa_with_noncedata(&message, &private.private_key, &[0x7a; 32]),
        sighash_type,
    };
    let mut default_signed = current.clone();
    default_signed.sign(&root, &secp).unwrap();
    assert_ne!(
        default_signed.inputs[0].partial_sigs.get(&public),
        Some(&nondefault),
    );
    current.inputs[0].partial_sigs.insert(public, nondefault);
    let before = current.clone();
    let signed = sign_imported_psbt(
        &wallet,
        &original,
        &current,
        &verified(&wallet, &phrases[0]),
    )
    .unwrap();
    assert_eq!(
        signed.inputs[0].partial_sigs.get(&public),
        Some(&nondefault)
    );
    for (pubkey, signature) in &foreign_a {
        assert_eq!(signed.inputs[0].partial_sigs.get(pubkey), Some(signature));
    }
    for (pubkey, signature) in &foreign_b {
        assert_eq!(signed.inputs[1].partial_sigs.get(pubkey), Some(signature));
    }
    assert_eq!(signed.inputs[0].partial_sigs.len(), 2);
    assert_eq!(signed.inputs[1].partial_sigs.len(), 2);
    assert_eq!(original.inputs[0].partial_sigs.len(), 0);
    assert_eq!(original.inputs[1].partial_sigs.len(), 0);
    assert_eq!(current, before);
    coincube_core::imported_spend::verify_imported_signatures(&wallet, &original, &signed, &secp)
        .unwrap();

    let mut invalid = before.clone();
    invalid.inputs[0]
        .partial_sigs
        .insert(public, *signed.inputs[1].partial_sigs.get(&public).unwrap());
    let invalid_before = invalid.clone();
    assert!(matches!(
        sign_imported_psbt(
            &wallet,
            &original,
            &invalid,
            &verified(&wallet, &phrases[0])
        ),
        Err(ImportedSignError::InvalidConstruction),
    ));
    assert_eq!(invalid, invalid_before);
}

#[test]
fn upstream_partial_signing_error_does_not_escape_adapter() {
    let (wallet, roots, first) = fixture("wpkh");
    let mut construction = first.clone();
    let mut second_prev = first.inputs[0].non_witness_utxo.clone().unwrap();
    second_prev.output[0].value += Amount::from_sat(1);
    let mut second_txin = first.unsigned_tx.input[0].clone();
    second_txin.previous_output.txid = second_prev.compute_txid();
    construction.unsigned_tx.input.push(second_txin);
    let mut second_input = first.inputs[0].clone();
    second_input.non_witness_utxo = Some(second_prev);
    second_input.witness_utxo.as_mut().unwrap().value += Amount::from_sat(1);
    second_input.sighash_type = Some(bitcoin::psbt::PsbtSighashType::from_u32(0xdead_beef));
    construction.inputs.push(second_input);
    construction.unsigned_tx.output[1].value += Amount::from_sat(150_001);

    // Upstream mutates input zero before failing on input one. Core signs a
    // clone, so that partially mutated PSBT cannot escape on error.
    let root = Xpriv::new_master(
        Network::Regtest,
        &bip39::Mnemonic::parse(&roots[0]).unwrap().to_seed(""),
    )
    .unwrap();
    let mut upstream = construction.clone();
    assert!(upstream.sign(&root, &Secp256k1::new()).is_err());
    assert!(!upstream.inputs[0].partial_sigs.is_empty());

    assert!(construction
        .inputs
        .iter()
        .all(|input| input.partial_sigs.is_empty()));
    assert!(sign_imported_psbt(
        &wallet,
        &construction,
        &construction,
        &verified(&wallet, &roots[0])
    )
    .is_err());
    assert!(construction
        .inputs
        .iter()
        .all(|input| input.partial_sigs.is_empty()));
}

/// Proof of the v3 imported-secret payload before the Recovery Kit writer and
/// restore flow adopt it. Existing v1 Cube seeds and v2 imported descriptors
/// remain handled by their current version dispatcher.
#[test]
fn encrypted_imported_key_bundle_binds_cube_chain_wallet_and_passphrase() {
    use crate::services::recovery::{decrypt, encrypt, KdfParams};
    use serde::{Deserialize, Serialize};
    use zeroize::Zeroizing;

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LocalKey {
        mnemonic: String,
        passphrase: String,
        origin_path: String,
    }
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Bundle {
        version: u8,
        cube_uuid: String,
        network: String,
        wallet_identity: String,
        local_keys: Vec<LocalKey>,
    }
    impl Bundle {
        fn validate(&self, cube: &str, wallet: &ImportedWalletDescriptor) -> Result<(), ()> {
            if self.version != 3
                || self.cube_uuid != cube
                || self.network != wallet.chain().api_str()
                || self.wallet_identity
                    != wallet.identity(wallet.chain()).map_err(|_| ())?.to_string()
                || self.local_keys.is_empty()
            {
                return Err(());
            }
            Ok(())
        }
    }

    let (wallet, _, _) = fixture("wsh(multi)");
    let phrase = bip39::Mnemonic::from_entropy(&[0x42; 16])
        .unwrap()
        .to_string();
    let bundle = Bundle {
        version: 3,
        cube_uuid: "synthetic-cube-a".into(),
        network: wallet.chain().api_str().into(),
        wallet_identity: wallet.identity(wallet.chain()).unwrap().to_string(),
        local_keys: vec![
            LocalKey {
                mnemonic: phrase.clone(),
                passphrase: "synthetic passphrase".into(),
                origin_path: "m/48h/1h/0h/2h".into(),
            },
            LocalKey {
                mnemonic: bip39::Mnemonic::from_entropy(&[0x43; 16])
                    .unwrap()
                    .to_string(),
                passphrase: String::new(),
                origin_path: "m/48h/1h/0h/2h".into(),
            },
        ],
    };
    let password = Zeroizing::new("synthetic recovery password".to_owned());
    let encoded = serde_json::to_vec(&bundle).unwrap();
    let ciphertext = encrypt(
        &encoded,
        &password,
        KdfParams {
            memory_kib: 512,
            t_cost: 1,
            p_cost: 1,
        },
        "synthetic-cube-a",
    )
    .unwrap();
    assert!(!ciphertext.contains(&phrase));
    let plaintext = decrypt(&ciphertext, &password, "synthetic-cube-a").unwrap();
    let restored: Bundle = serde_json::from_slice(&plaintext).unwrap();
    restored.validate("synthetic-cube-a", &wallet).unwrap();
    assert_eq!(restored.local_keys[0].mnemonic, phrase);
    assert_eq!(restored.local_keys[0].passphrase, "synthetic passphrase");
    assert_eq!(restored.local_keys[0].origin_path, "m/48h/1h/0h/2h");
    assert_eq!(restored.local_keys.len(), 2);
    assert!(restored.local_keys[1].passphrase.is_empty());
    assert!(restored.validate("synthetic-cube-b", &wallet).is_err());
    let (another_wallet, _, _) = fixture("wsh(sortedmulti)");
    assert!(restored
        .validate("synthetic-cube-a", &another_wallet)
        .is_err());
    let mut unknown = restored;
    unknown.network = "bitcoin".into();
    assert!(unknown.validate("synthetic-cube-a", &wallet).is_err());
    unknown.network = wallet.chain().api_str().into();
    unknown.version = 4;
    assert!(unknown.validate("synthetic-cube-a", &wallet).is_err());
}

#[test]
fn legacy_recovery_descriptor_readers_remain_versioned() {
    use crate::services::recovery::{
        parse_wallet_descriptor_blob, ImportedDescriptorBlob, ImportedDescriptorBlobWallet,
        RecoveredWalletBlob,
    };
    let old_vault = serde_json::json!({
        "version": 1,
        "cube": { "uuid": "synthetic-cube", "network": "regtest" },
        "vault": { "name": "synthetic vault", "descriptor": "wsh(pk(test))", "signers": [] }
    });
    assert!(matches!(
        parse_wallet_descriptor_blob(&serde_json::to_vec(&old_vault).unwrap()),
        Ok(RecoveredWalletBlob::Vault(_))
    ));

    let (wallet, _, _) = fixture("wpkh");
    let old_import = ImportedDescriptorBlob::new(
        "synthetic-cube".into(),
        ChainId::Regtest,
        ImportedDescriptorBlobWallet {
            name: "synthetic imported wallet".into(),
            descriptor: wallet,
            birthday: 0,
            initial_lookahead: 100,
            receive_index: 0,
            change_index: 0,
        },
    )
    .unwrap();
    let mut value = serde_json::to_value(&old_import).unwrap();
    assert!(matches!(
        parse_wallet_descriptor_blob(&serde_json::to_vec(&value).unwrap()),
        Ok(RecoveredWalletBlob::Imported(_))
    ));
    value["version"] = 3.into();
    assert!(parse_wallet_descriptor_blob(&serde_json::to_vec(&value).unwrap()).is_err());
}
