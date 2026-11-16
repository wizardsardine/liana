use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use coincube_core::{
    imported_spend::verify_imported_signatures,
    miniscript::bitcoin::{bip32::DerivationPath, psbt::Psbt, secp256k1::Secp256k1},
    wallet_descriptor::ImportedWalletDescriptor,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::str::FromStr;

const INPUT: &str = include_str!("fixtures/imported_signing.json");
const SIGNED: &str = include_str!("fixtures/imported_signing_signed.json");
const INPUT_SHA256: &str = "ef14b201a02d3c0f2f21e9a8ef98b88251a478863d9e63982654ec1798585b30";
const SIGNED_SHA256: &str = "075e261fa5433603746c7462890c14ea99ac7309da06e288449b46c54e20146b";
const CANONICAL_REQUEST: &str = include_str!("fixtures/imported_canonical_requests.json");
const CANONICAL_SIGNED: &str = include_str!("fixtures/imported_canonical_signed.json");
const CANONICAL_REQUEST_SHA256: &str =
    "71bf8c42cce0c9725b641da66d321a9ea6894b5a783ea5725c2100bfdd57c23f";
const CANONICAL_SIGNED_SHA256: &str =
    "944753d442cad41affbddf424069b08f2a51990ab6148ec645f0d42fa7fb9a31";

fn fixture() -> (Value, Value) {
    assert_eq!(hex::encode(Sha256::digest(INPUT.as_bytes())), INPUT_SHA256);
    assert_eq!(
        hex::encode(Sha256::digest(SIGNED.as_bytes())),
        SIGNED_SHA256
    );
    (
        serde_json::from_str(INPUT).unwrap(),
        serde_json::from_str(SIGNED).unwrap(),
    )
}

fn psbt(value: &Value) -> Psbt {
    Psbt::deserialize(&STANDARD.decode(value.as_str().unwrap()).unwrap()).unwrap()
}

fn case<'a>(input: &'a Value, name: &str) -> &'a Value {
    input["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["name"] == name)
        .unwrap()
}

fn policy(vector: &Value) -> PhonePolicy {
    let text = vector["descriptor"].as_str().unwrap();
    let imported = ImportedWalletDescriptor::parse(text, None, crate::chain::ChainId::Testnet)
        .expect("Mobile fixture must pass desktop imported policy");
    let wallet_id =
        crate::app::settings::WalletId::for_imported(&imported, crate::chain::ChainId::Testnet)
            .unwrap();
    PhonePolicy::imported(imported, crate::chain::ChainId::Testnet, wallet_id).unwrap()
}

fn phone(input: &Value, policy: &PhonePolicy) -> pairing_store::PairedPhone {
    phone_for_key(input, policy, 0)
}

fn phone_for_key(input: &Value, policy: &PhonePolicy, index: usize) -> pairing_store::PairedPhone {
    let policy::PhonePolicyKind::Imported { wallet, wallet_id } = policy.kind() else {
        panic!("imported fixture");
    };
    let key = &input["keys"][index];
    let fingerprint = key["fingerprint"].as_str().unwrap().parse().unwrap();
    let binding = pairing_store::SignerBinding {
        key_id: key["backend_key_id"].as_u64().unwrap().to_string(),
        xpub: key["xpub"].as_str().unwrap().to_owned(),
        fingerprint,
        descriptor_sha256: Sha256::digest(policy.descriptor().as_bytes()).to_vec(),
        imported_scope: Some(
            pairing_store::ImportedPairingScope::for_account(
                wallet,
                wallet_id,
                key["xpub"].as_str().unwrap(),
                fingerprint,
            )
            .unwrap(),
        ),
    };
    pairing_store::PairedPhone {
        signer_binding: Some(binding),
        cert_pin: [7; 32],
        name: "Synthetic Mobile".into(),
        paired_at_unix: 1,
        wallet_fingerprints: vec![fingerprint],
        vault_fingerprint: policy.transport_id(),
        transport_pubkey: vec![2; 33],
        fallback_addr: None,
    }
}

/// Public-only export from the real desktop policy constructor. The frozen M1
/// inputs are never rewritten.
fn canonical_sortedmulti_requests() -> Value {
    let (input, _) = fixture();
    let vectors: Vec<_> = ["sortedmulti_first", "sortedmulti_nonfirst"]
        .iter()
        .map(|name| {
            let original = case(&input, name);
            let policy = policy(original);
            let descriptor = policy.descriptor();
            let hash = hex::encode(Sha256::digest(descriptor.as_bytes()));
            serde_json::json!({
                "name": name,
                "descriptor": descriptor,
                "descriptor_sha256": hash,
                "descriptor_id": policy.transport_id().to_string(),
                "branch": original["branch"],
                "index": original["index"],
                "psbt": original["psbt"],
                "previous_tx": original["previous_tx"],
                "prevout_script": original["prevout_script"],
                "prevout_value_sats": original["prevout_value_sats"],
                "scripts": original["scripts"],
                "signer_key_id": original["signer_key_id"],
                "signer_key_index": original["signer_key_index"],
            })
        })
        .collect();
    let keys: Vec<_> = input["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| {
            serde_json::json!({
                "backend_key_id": key["backend_key_id"],
                "fingerprint": key["fingerprint"],
                "path": key["path"],
                "xpub": key["xpub"],
            })
        })
        .collect();
    let export =
        serde_json::json!({"schema": 1, "keys": keys, "network": "testnet", "vectors": vectors});
    export
}

#[test]
fn export_canonical_sortedmulti_requests() {
    let export = canonical_sortedmulti_requests();
    if let Ok(path) = std::env::var("COINCUBE_CANONICAL_IMPORT_EXPORT_PATH") {
        std::fs::write(path, serde_json::to_vec_pretty(&export).unwrap()).unwrap();
    }
    assert_eq!(export["vectors"].as_array().unwrap().len(), 2);
}

#[test]
fn wallet_policy_entry_preserves_both_frozen_imported_requests_and_vault_control() {
    use coincube_core::{
        descriptors::CoincubeDescriptor,
        miniscript::bitcoin::{
            bip32::{Xpriv, Xpub},
            Network,
        },
    };

    let frozen: Value = serde_json::from_str(CANONICAL_REQUEST).unwrap();
    assert_eq!(
        serde_json::to_vec_pretty(&canonical_sortedmulti_requests()).unwrap(),
        CANONICAL_REQUEST.as_bytes()
    );
    for name in ["sortedmulti_first", "sortedmulti_nonfirst"] {
        let vector = case(&frozen, name);
        let exact = vector["descriptor"].as_str().unwrap();
        let imported =
            ImportedWalletDescriptor::parse(exact, None, crate::chain::ChainId::Testnet).unwrap();
        let loaded = crate::app::wallet::Wallet::new(imported.clone());
        let entry = PhonePolicy::from_wallet(&loaded).unwrap();
        assert_eq!(entry.descriptor().as_bytes(), exact.as_bytes(), "{name}");
        assert!(
            entry.descriptor().contains("48'/1'/0'/2'"),
            "{name}",
            name = name
        );
        assert!(
            entry.descriptor().ends_with("#slsxmyf5"),
            "{name}",
            name = name
        );
        assert_eq!(
            hex::encode(Sha256::digest(entry.descriptor().as_bytes())),
            vector["descriptor_sha256"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(
            entry.transport_id().to_string(),
            vector["descriptor_id"].as_str().unwrap(),
            "{name}"
        );
        let policy::PhonePolicyKind::Imported { wallet, wallet_id } = entry.kind() else {
            panic!("imported Wallet must yield imported PhonePolicy")
        };
        assert_eq!(wallet, &imported, "{name}");
        assert_eq!(wallet_id, &loaded.id(), "{name}");
        assert_eq!(
            wallet_id,
            &crate::app::settings::WalletId::for_imported(
                &imported,
                crate::chain::ChainId::Testnet
            )
            .unwrap(),
            "{name}"
        );
    }

    let secp = Secp256k1::new();
    let roots = [
        Xpriv::new_master(Network::Bitcoin, &[5; 32]).unwrap(),
        Xpriv::new_master(Network::Bitcoin, &[6; 32]).unwrap(),
    ];
    let keys: Vec<_> = roots
        .iter()
        .map(|root| {
            format!(
                "[{}]{}/<0;1>/*",
                root.fingerprint(&secp),
                Xpub::from_priv(&secp, root)
            )
        })
        .collect();
    let vault = CoincubeDescriptor::from_str(&format!(
        "wsh(or_d(pk({}),and_v(v:pkh({}),older(52560))))",
        keys[0], keys[1]
    ))
    .unwrap();
    let loaded = crate::app::wallet::Wallet::new(vault.clone());
    let entry = PhonePolicy::from_wallet(&loaded).unwrap();
    assert!(matches!(entry.kind(), policy::PhonePolicyKind::Vault(_)));
    assert_eq!(entry.descriptor(), vault.to_string());
    assert_eq!(entry.chain(), loaded.chain);
    assert_eq!(
        entry.transport_id().to_string(),
        hex::encode(&Sha256::digest(entry.descriptor().as_bytes())[..4])
    );
}

#[test]
fn actual_canonical_phone_results_verify_against_unchanged_tenshu_request() {
    assert_eq!(
        hex::encode(Sha256::digest(CANONICAL_REQUEST.as_bytes())),
        CANONICAL_REQUEST_SHA256
    );
    assert_eq!(
        hex::encode(Sha256::digest(CANONICAL_SIGNED.as_bytes())),
        CANONICAL_SIGNED_SHA256
    );
    let export = canonical_sortedmulti_requests();
    assert_eq!(
        serde_json::to_vec_pretty(&export).unwrap(),
        CANONICAL_REQUEST.as_bytes(),
        "published mobile request bytes must be the current Tenshu serializer export"
    );
    let signed: Value = serde_json::from_str(CANONICAL_SIGNED).unwrap();
    let (raw, _) = fixture();
    assert_eq!(signed["vectors"].as_array().unwrap().len(), 2);
    for request in export["vectors"].as_array().unwrap() {
        let name = request["name"].as_str().unwrap();
        let original_vector = case(&raw, name);
        let policy = policy(original_vector);
        assert_eq!(request["descriptor"], policy.descriptor());
        let original = psbt(&request["psbt"]);
        assert_eq!(original, psbt(&original_vector["psbt"]));
        let returned = psbt(&case(&signed, name)["signed_psbt"]);
        let phone = phone(&raw, &policy);
        let binding = phone.exact_signer_for(&policy).unwrap();
        let policy::PhonePolicyKind::Imported { wallet, .. } = policy.kind() else {
            unreachable!()
        };
        let mut merged = original.clone();
        signatures::merge_imported_verified(&mut merged, &returned, wallet, binding).unwrap();
        assert_eq!(merged.unsigned_tx, original.unsigned_tx);
        assert_eq!(merged.outputs, original.outputs);
        assert_eq!(merged.inputs[0].partial_sigs.len(), 1);
        assert_eq!(
            merged.inputs[0].final_script_sig,
            original.inputs[0].final_script_sig
        );
        assert_eq!(
            merged.inputs[0].final_script_witness,
            original.inputs[0].final_script_witness
        );
        assert_eq!(
            verify_imported_signatures(wallet, &original, &merged, &Secp256k1::verification_only())
                .unwrap(),
            vec![1]
        );
        let mut tampered = returned.clone();
        tampered.unsigned_tx.output[0].value =
            coincube_core::miniscript::bitcoin::Amount::from_sat(1);
        let before = original.serialize();
        let mut rejected = original.clone();
        assert!(
            signatures::merge_imported_verified(&mut rejected, &tampered, wallet, binding).is_err()
        );
        assert_eq!(
            rejected.serialize(),
            before,
            "{name}: failed merge mutated original"
        );
    }
}

#[test]
fn actual_phone_vectors_match_desktop_canonical_policy_and_verify_all_signatures() {
    let (input, signed) = fixture();
    assert_eq!(input["schema"], 1);
    assert_eq!(signed["schema"], 1);
    assert_eq!(input["vectors"].as_array().unwrap().len(), 10);
    assert_eq!(signed["vectors"].as_array().unwrap().len(), 10);
    for vector in input["vectors"].as_array().unwrap() {
        let name = vector["name"].as_str().unwrap();
        let policy = policy(vector);
        let policy::PhonePolicyKind::Imported { wallet, .. } = policy.kind() else {
            unreachable!()
        };
        let expected = crate::split_hardware::policy::imported_descriptor(wallet)
            .unwrap()
            .to_string();
        assert_eq!(policy.descriptor(), expected, "{}", name);
        let digest = hex::encode(Sha256::digest(policy.descriptor().as_bytes()));
        assert_eq!(policy.transport_id().to_string(), digest[..8], "{}", name);
        if name.starts_with("sortedmulti_") {
            assert_ne!(digest, vector["descriptor_sha256"], "{}", name);
            assert_eq!(
                digest, "6c2d38811a23ff93af86b81c84667591e674884d804843ddf35c5b9995cc1219",
                "{}",
                name
            );
        }
        for scripts in vector["scripts"].as_array().unwrap() {
            let index = scripts["index"].as_u64().unwrap() as u32;
            let receive = wallet
                .receive_descriptor()
                .at_derivation_index(index)
                .unwrap();
            let change = wallet
                .change_descriptor()
                .at_derivation_index(index)
                .unwrap();
            assert_eq!(
                hex::encode(receive.script_pubkey().as_bytes()),
                scripts["receive"],
                "{name}: receive {index}"
            );
            assert_eq!(
                hex::encode(change.script_pubkey().as_bytes()),
                scripts["change"],
                "{name}: change {index}"
            );
        }
        if name.starts_with("sortedmulti_") {
            let branch = vector["branch"].as_u64().unwrap();
            let index = vector["index"].as_u64().unwrap();
            let scripts = vector["scripts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|script| script["index"] == index)
                .unwrap();
            let rank = if branch == 0 {
                scripts["phone_sorted_receive_rank"].as_u64().unwrap()
            } else {
                scripts["phone_sorted_change_rank"].as_u64().unwrap()
            };
            assert_eq!(rank, if name == "sortedmulti_first" { 0 } else { 1 });
        }
        let signed_vector = case(&signed, name);
        let original = psbt(&vector["psbt"]);
        let returned = psbt(&signed_vector["signed_psbt"]);
        let prevout = signatures::authenticated_prevouts(&original).unwrap();
        assert_eq!(
            hex::encode(prevout[0].script_pubkey.as_bytes()),
            vector["prevout_script"],
            "{name}"
        );
        assert_eq!(
            prevout[0].value.to_sat(),
            vector["prevout_value_sats"].as_u64().unwrap()
        );
        let phone = phone(&input, &policy);
        let binding = phone.exact_signer_for(&policy).unwrap();
        let mut target = original.clone();
        signatures::merge_imported_verified(&mut target, &returned, wallet, binding)
            .unwrap_or_else(|reason| panic!("{}: {}", name, reason));
        assert_eq!(target.unsigned_tx, original.unsigned_tx, "{name}");
        assert_eq!(target.outputs, original.outputs, "{name}");
        assert_eq!(target.inputs[0].partial_sigs.len(), 1, "{name}");
        assert_eq!(
            verify_imported_signatures(wallet, &original, &target, &Secp256k1::verification_only())
                .unwrap(),
            vec![1],
            "{name}"
        );
        // Native singlesig final fields are disposable, never copied.
        assert_eq!(
            target.inputs[0].final_script_sig,
            original.inputs[0].final_script_sig
        );
        assert_eq!(
            target.inputs[0].final_script_witness,
            original.inputs[0].final_script_witness
        );
    }
}

#[test]
fn imported_pairing_requires_exact_account_origin_chain_wallet_and_full_hash() {
    let (input, _) = fixture();
    let vector = case(&input, "sortedmulti_nonfirst");
    let policy = policy(vector);
    let policy::PhonePolicyKind::Imported { wallet, wallet_id } = policy.kind() else {
        unreachable!()
    };
    assert!(PhonePolicy::imported(
        wallet.clone(),
        crate::chain::ChainId::Signet,
        wallet_id.clone(),
    )
    .is_err());
    let phone = phone(&input, &policy);
    let saved = serde_json::to_string(&phone).unwrap();
    let restored: pairing_store::PairedPhone = serde_json::from_str(&saved).unwrap();
    assert!(restored.exact_signer_for(&policy).is_ok());
    assert!(phone.exact_signer_for(&policy).is_ok());
    let duplicate = pairing_store::PairingStoreFile {
        phones: vec![phone.clone(), phone.clone()],
        pending_unpairs: Vec::new(),
    };
    assert!(duplicate.unique_signer_for(&phone, &policy).is_err());
    let mut second_phone = phone.clone();
    second_phone.cert_pin = [8; 32];
    let duplicate_key_id = pairing_store::PairingStoreFile {
        phones: vec![phone.clone(), second_phone],
        pending_unpairs: Vec::new(),
    };
    assert!(duplicate_key_id.unique_signer_for(&phone, &policy).is_err());
    assert!(pairing_store::PairingStoreFile::default()
        .unique_signer_for(&phone, &policy)
        .is_err());
    let mut replaced = pairing_store::PairingStoreFile {
        phones: vec![phone.clone()],
        pending_unpairs: Vec::new(),
    };
    replaced.phones[0].signer_binding.as_mut().unwrap().key_id = "99".into();
    assert!(replaced.unique_signer_for(&phone, &policy).is_err());
    let key = &input["keys"][3];
    let mut sibling = phone.clone();
    let binding = sibling.signer_binding.as_mut().unwrap();
    binding.xpub = key["xpub"].as_str().unwrap().to_owned();
    assert!(sibling.exact_signer_for(&policy).is_err());
    let mut wrong_origin = phone.clone();
    wrong_origin
        .signer_binding
        .as_mut()
        .unwrap()
        .imported_scope
        .as_mut()
        .unwrap()
        .account_origin = DerivationPath::from_str("m/48'/1'/1'/2'").unwrap();
    assert!(wrong_origin.exact_signer_for(&policy).is_err());
    let mut wrong_hash = phone.clone();
    wrong_hash
        .signer_binding
        .as_mut()
        .unwrap()
        .descriptor_sha256[8] ^= 1;
    assert_eq!(wrong_hash.vault_fingerprint, policy.transport_id());
    assert!(wrong_hash.exact_signer_for(&policy).is_err());
    let mut wrong_chain = phone.clone();
    wrong_chain
        .signer_binding
        .as_mut()
        .unwrap()
        .imported_scope
        .as_mut()
        .unwrap()
        .chain = crate::chain::ChainId::Signet;
    assert!(wrong_chain.exact_signer_for(&policy).is_err());
    let mut wrong_wallet = phone.clone();
    wrong_wallet
        .signer_binding
        .as_mut()
        .unwrap()
        .imported_scope
        .as_mut()
        .unwrap()
        .wallet_id
        .descriptor_checksum
        .push('0');
    assert!(wrong_wallet.exact_signer_for(&policy).is_err());
    let mut legacy = phone.clone();
    legacy.signer_binding.as_mut().unwrap().imported_scope = None;
    let encoded = serde_json::to_string(&legacy).unwrap();
    assert!(!encoded.contains("imported_scope"));
    let read: pairing_store::PairedPhone = serde_json::from_str(&encoded).unwrap();
    assert!(read.exact_signer_for(&policy).is_err());
    // Existing store JSON without the new optional field remains readable.
    let old_store = serde_json::to_string(&pairing_store::PairingStoreFile {
        phones: vec![legacy],
        pending_unpairs: Vec::new(),
    })
    .unwrap();
    let migrated: pairing_store::PairingStoreFile = serde_json::from_str(&old_store).unwrap();
    assert!(migrated.phones[0]
        .signer_binding
        .as_ref()
        .unwrap()
        .imported_scope
        .is_none());
    let mut zero = phone;
    zero.signer_binding.as_mut().unwrap().key_id = "0".into();
    assert!(zero.exact_signer_for(&policy).is_err());
}

#[test]
fn imported_pairing_revision_revokes_same_second_pending_response_and_cached_handle() {
    use super::pairing_transaction::PairingTransaction;
    let (input, signed) = fixture();
    let vector = case(&input, "sortedmulti_first");
    let policy = policy(vector);
    let first = phone(&input, &policy);
    let dir = crate::dir::CoincubeDirectory::new(
        std::env::temp_dir().join(format!("imported-revision-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(dir.path()).unwrap();
    let first_id = uuid::Uuid::new_v4().to_string();
    let transaction = PairingTransaction::prepare(&dir, first_id.clone(), first.clone()).unwrap();
    transaction.write_candidate().unwrap();
    transaction.decide().unwrap();
    transaction.finish().unwrap();
    let store = pairing_store::load(&dir).unwrap();
    let captured = store.phones[0].clone();
    let revision = |row: &pairing_store::PairedPhone| {
        row.signer_binding
            .as_ref()
            .unwrap()
            .imported_scope
            .as_ref()
            .unwrap()
            .pairing_revision
            .as_deref()
            .unwrap()
            .to_owned()
    };
    assert_eq!(revision(&captured), first_id);
    assert!(store.unique_signer_for(&captured, &policy).is_ok());
    assert!(store.cached_imported_matches(&captured, &policy, &policy));

    // Rename and fallback edits do not create a new admission or retire a
    // connected handle. Removal does, even before a replacement appears.
    pairing_store::update_user_fields(
        &dir,
        &captured.cert_pin,
        "Renamed".into(),
        Some("127.0.0.1:1234".into()),
    )
    .unwrap();
    assert!(pairing_store::load(&dir)
        .unwrap()
        .cached_imported_matches(&captured, &policy, &policy));
    pairing_store::unpair(&dir, &captured.cert_pin).unwrap();
    assert!(pairing_store::load(&dir)
        .unwrap()
        .unique_signer_for(&captured, &policy)
        .is_err());

    let second_id = uuid::Uuid::new_v4().to_string();
    let mut re_pair = first;
    re_pair.paired_at_unix = captured.paired_at_unix;
    let transaction = PairingTransaction::prepare(&dir, second_id.clone(), re_pair).unwrap();
    transaction.write_candidate().unwrap();
    transaction.decide().unwrap();
    transaction.finish().unwrap();
    let store = pairing_store::load(&dir).unwrap();
    let current = &store.phones[0];
    assert_eq!(captured.paired_at_unix, current.paired_at_unix);
    assert_eq!(revision(current), second_id);
    assert_ne!(revision(&captured), revision(current));
    let persisted = serde_json::to_vec(&store).unwrap();
    let restored: pairing_store::PairingStoreFile = serde_json::from_slice(&persisted).unwrap();
    assert_eq!(revision(&restored.phones[0]), second_id);
    assert!(!pairing_store::remove_unpaired_by_peer(
        &dir,
        &captured.cert_pin,
        captured.paired_at_unix,
        captured.imported_pairing_revision(),
    )
    .unwrap());
    assert!(store.unique_signer_for(&captured, &policy).is_err());
    assert!(!store.cached_imported_matches(&captured, &policy, &policy));
    assert!(store.cached_imported_matches(current, &policy, &policy));

    // The late response from the old review cannot reach the PSBT merge.
    let mut pending = psbt(&vector["psbt"]);
    let before = pending.serialize();
    let returned = psbt(&case(&signed, "sortedmulti_first")["signed_psbt"]);
    let policy::PhonePolicyKind::Imported { wallet, .. } = policy.kind() else {
        unreachable!()
    };
    let result = store
        .unique_signer_for(&captured, &policy)
        .and_then(|binding| {
            signatures::merge_imported_verified(&mut pending, &returned, wallet, binding)
        });
    assert!(result.is_err());
    assert_eq!(pending.serialize(), before);
    let current_binding = store.unique_signer_for(current, &policy).unwrap();
    signatures::merge_imported_verified(&mut pending, &returned, wallet, current_binding).unwrap();
    assert_eq!(pending.inputs[0].partial_sigs.len(), 1);

    let mut legacy = current.clone();
    legacy
        .signer_binding
        .as_mut()
        .unwrap()
        .imported_scope
        .as_mut()
        .unwrap()
        .pairing_revision = None;
    let legacy_store = pairing_store::PairingStoreFile {
        phones: vec![legacy.clone()],
        pending_unpairs: Vec::new(),
    };
    let legacy_bytes = serde_json::to_vec(&legacy_store).unwrap();
    assert!(!String::from_utf8_lossy(&legacy_bytes).contains("pairing_revision"));
    let restored_legacy: pairing_store::PairingStoreFile =
        serde_json::from_slice(&legacy_bytes).unwrap();
    assert!(restored_legacy.unique_signer_for(&legacy, &policy).is_err());
}

#[test]
fn late_imported_unpair_cannot_remove_same_second_vault_replacement() {
    let (input, _) = fixture();
    let policy = policy(case(&input, "sortedmulti_first"));
    let imported = phone(&input, &policy);
    let dir = crate::dir::CoincubeDirectory::new(
        std::env::temp_dir().join(format!("imported-to-vault-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir_all(dir.path()).unwrap();
    pairing_store::upsert(&dir, imported.clone()).unwrap();

    // Same TLS identity and display second, but a freshly admitted Vault
    // pairing has no imported revision. The old imported reader must not
    // revoke this replacement; the Vault reader still may remove its own row.
    let mut vault = imported.clone();
    vault.signer_binding.as_mut().unwrap().imported_scope = None;
    pairing_store::upsert(&dir, vault.clone()).unwrap();
    assert!(!pairing_store::remove_unpaired_by_peer(
        &dir,
        &imported.cert_pin,
        imported.paired_at_unix,
        imported.imported_pairing_revision(),
    )
    .unwrap());
    assert_eq!(
        serde_json::to_value(&pairing_store::load(&dir).unwrap().phones).unwrap(),
        serde_json::to_value(vec![vault.clone()]).unwrap()
    );
    assert!(pairing_store::remove_unpaired_by_peer(
        &dir,
        &vault.cert_pin,
        vault.paired_at_unix,
        None,
    )
    .unwrap());
    assert!(pairing_store::load(&dir).unwrap().phones.is_empty());
}

#[test]
fn cached_live_imported_handle_is_replaced_for_another_valid_account() {
    let (input, _) = fixture();
    let vector = case(&input, "sortedmulti_first");
    let policy = policy(vector);
    let captured = phone(&input, &policy);
    let unchanged = pairing_store::PairingStoreFile {
        phones: vec![captured.clone()],
        pending_unpairs: Vec::new(),
    };
    assert!(unchanged.cached_imported_matches(&captured, &policy, &policy));

    let mut replacement = phone_for_key(&input, &policy, 1);
    replacement.cert_pin = captured.cert_pin;
    replacement.paired_at_unix = captured.paired_at_unix;
    replacement.transport_pubkey = captured.transport_pubkey.clone();
    let replacement_store = pairing_store::PairingStoreFile {
        phones: vec![replacement.clone()],
        pending_unpairs: Vec::new(),
    };
    assert!(replacement_store
        .unique_signer_for(&replacement, &policy)
        .is_ok());
    assert!(!replacement_store.cached_imported_matches(&captured, &policy, &policy));
    assert!(replacement_store.cached_imported_matches(&replacement, &policy, &policy));

    let wrong_policy = self::policy(case(&input, "wpkh_receive"));
    assert!(!replacement_store.cached_imported_matches(&replacement, &policy, &wrong_policy));
    assert!(
        !pairing_store::PairingStoreFile::default().cached_imported_matches(
            &replacement,
            &policy,
            &policy
        )
    );
    let ambiguous = pairing_store::PairingStoreFile {
        phones: vec![replacement.clone(), replacement],
        pending_unpairs: Vec::new(),
    };
    assert!(!ambiguous.cached_imported_matches(&captured, &policy, &policy));
}

#[test]
fn imported_signature_delta_rejects_tampering_and_preserves_original() {
    let (input, signed) = fixture();
    let vector = case(&input, "multi_first");
    let policy = policy(vector);
    let policy::PhonePolicyKind::Imported { wallet, .. } = policy.kind() else {
        unreachable!()
    };
    let binding = phone(&input, &policy).signer_binding.unwrap();
    let original = psbt(&vector["psbt"]);
    let returned = psbt(&case(&signed, "multi_first")["signed_psbt"]);
    let mut changed = returned.clone();
    changed.unsigned_tx.output[0].value = coincube_core::miniscript::bitcoin::Amount::from_sat(1);
    let mut changed_output_metadata = returned.clone();
    changed_output_metadata.outputs[0].redeem_script =
        Some(coincube_core::miniscript::bitcoin::ScriptBuf::new());
    let mut wrong_prevout = returned.clone();
    wrong_prevout.inputs[0].witness_utxo.as_mut().unwrap().value =
        coincube_core::miniscript::bitcoin::Amount::from_sat(1);
    let mut wrong_script = returned.clone();
    wrong_script.inputs[0]
        .witness_utxo
        .as_mut()
        .unwrap()
        .script_pubkey = coincube_core::miniscript::bitcoin::ScriptBuf::new();
    let mut changed_prev_tx = returned.clone();
    changed_prev_tx.inputs[0]
        .non_witness_utxo
        .as_mut()
        .unwrap()
        .output[0]
        .value = coincube_core::miniscript::bitcoin::Amount::from_sat(1);
    let mut wrong_path = returned.clone();
    wrong_path.inputs[0]
        .bip32_derivation
        .values_mut()
        .next()
        .unwrap()
        .1 = DerivationPath::from_str("m/48'/1'/0'/2'/1/99").unwrap();
    let mut wrong_sig = returned.clone();
    wrong_sig.inputs[0]
        .partial_sigs
        .values_mut()
        .next()
        .unwrap()
        .sighash_type = coincube_core::miniscript::bitcoin::sighash::EcdsaSighashType::None;
    let mut final_only = returned.clone();
    final_only.inputs[0].partial_sigs.clear();
    let mut malformed_native = returned.clone();
    malformed_native.inputs[0].witness_script = None;
    let mut extra = returned.clone();
    let prior = psbt(&vector["mutations"]["preexisting_partial"]);
    extra.inputs[0]
        .partial_sigs
        .extend(prior.inputs[0].partial_sigs.clone());
    let mut invalid_crypto = returned.clone();
    invalid_crypto.inputs[0]
        .partial_sigs
        .values_mut()
        .next()
        .unwrap()
        .signature = prior.inputs[0]
        .partial_sigs
        .values()
        .next()
        .unwrap()
        .signature;
    for (label, response) in [
        ("changed unsigned transaction", changed),
        ("changed output metadata", changed_output_metadata),
        ("changed authenticated amount", wrong_prevout),
        ("changed authenticated script", wrong_script),
        ("changed previous transaction", changed_prev_tx),
        ("changed derivation", wrong_path),
        ("wrong sighash", wrong_sig),
        ("final-only response", final_only),
        ("malformed native script metadata", malformed_native),
        ("unauthorized extra signer", extra),
        ("cryptographically invalid signature", invalid_crypto),
    ] {
        let mut target = original.clone();
        assert!(
            signatures::merge_imported_verified(&mut target, &response, wallet, &binding).is_err(),
            "{}",
            label
        );
        assert_eq!(target, original, "{label} must be atomic");
    }
    let mut with_prior = original.clone();
    with_prior.inputs[0].partial_sigs = prior.inputs[0].partial_sigs.clone();
    let mut missing = returned.clone();
    assert!(
        signatures::merge_imported_verified(&mut with_prior, &missing, wallet, &binding).is_err()
    );
    assert_eq!(
        with_prior.inputs[0].partial_sigs,
        prior.inputs[0].partial_sigs
    );
    let mut conflicting = returned.clone();
    conflicting.inputs[0]
        .partial_sigs
        .extend(prior.inputs[0].partial_sigs.clone());
    conflicting.inputs[0]
        .partial_sigs
        .get_mut(prior.inputs[0].partial_sigs.keys().next().unwrap())
        .unwrap()
        .sighash_type = coincube_core::miniscript::bitcoin::sighash::EcdsaSighashType::None;
    assert!(
        signatures::merge_imported_verified(&mut with_prior, &conflicting, wallet, &binding)
            .is_err()
    );
    assert_eq!(
        with_prior.inputs[0].partial_sigs,
        prior.inputs[0].partial_sigs
    );
    missing.inputs[0]
        .partial_sigs
        .extend(prior.inputs[0].partial_sigs.clone());
    let before = with_prior.clone();
    signatures::merge_imported_verified(&mut with_prior, &missing, wallet, &binding).unwrap();
    assert_eq!(with_prior.inputs[0].partial_sigs.len(), 2);
    assert_eq!(
        with_prior.inputs[0].non_witness_utxo,
        before.inputs[0].non_witness_utxo
    );
}
