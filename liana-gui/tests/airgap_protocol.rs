use std::str::FromStr;

use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{secp256k1::Secp256k1, Network},
};
use liana_gui::airgap::{
    encode_ur, validate_and_merge_psbt, AddressVerificationRequest, AirgappedResponse,
    DecodeProgress, Error, ExpectedResponse, PassportAccount, PolicyRegistration, ScanLimits,
    UrDecodeSession, UrPayload, UrType, VerifiedAddress,
};

const POLICY: &[u8] = include_bytes!("../test_assets/passport/policy-registration-mainnet.json");
const ADDRESS_REQUEST: &[u8] =
    include_bytes!("../test_assets/passport/address-request-mainnet.json");
const ADDRESS_RESPONSE: &[u8] =
    include_bytes!("../test_assets/passport/address-response-mainnet.json");
const SINGLE_UR: &str = include_str!("../test_assets/passport/ur-single-bytes.txt");
const MULTIPART_UR: &str = include_str!("../test_assets/passport/ur-multipart-bytes.txt");

fn strip_fixture_line_ending(bytes: &[u8]) -> &[u8] {
    bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes)
}

#[test]
fn policy_fixture_matches_passport_core_identity_and_liana_checksum() {
    let policy = PolicyRegistration::from_json(POLICY).unwrap();
    assert_eq!(
        policy.policy_id,
        "506b3dd1ce28b757cde12e2977c483b0afb518de9ad8edbdfbc01e5d9763dd9f"
    );
    assert_eq!(policy.descriptor_checksum().unwrap(), "y7qrgwup");
    assert_eq!(policy.to_json().unwrap(), strip_fixture_line_ending(POLICY));
}

#[test]
fn liana_multisig_descriptor_preserves_paths_key_order_and_timelock() {
    let source = include_str!("../test_assets/passport/liana-multisig-testnet.descriptor").trim();
    let descriptor = LianaDescriptor::from_str(source).unwrap();
    let policy =
        PolicyRegistration::from_descriptor("Family Vault", Network::Testnet4, &descriptor)
            .unwrap();
    assert_eq!(policy.keys.len(), 3);
    assert_eq!(
        policy.template,
        "wsh(or_i(and_v(v:thresh(2,pkh(@0/<2;3>/*),a:pkh(@1/<2;3>/*),a:pkh(@2/<0;1>/*)),older(52596)),and_v(v:pk(@0/<0;1>/*),pk(@1/<0;1>/*))))"
    );
    assert_eq!(policy.full_descriptor(), source.rsplit_once('#').unwrap().0);
    assert_eq!(policy.descriptor_checksum().unwrap(), "u768v50p");
    // Generated independently by Passport Core's MiniscriptPolicy v1.
    assert_eq!(
        policy.policy_id,
        "54c9de390dd71ce7f500cac1b20b3ec2bbea26fd31dea892f936e78b61833151"
    );
}

#[test]
fn address_is_bound_to_the_active_policy() {
    let policy = PolicyRegistration::from_json(POLICY).unwrap();
    let request: AddressVerificationRequest = serde_json::from_slice(ADDRESS_REQUEST).unwrap();
    let response = match ExpectedResponse::VerifiedAddress
        .decode(UrPayload::bytes(
            strip_fixture_line_ending(ADDRESS_RESPONSE).to_vec(),
        ))
        .unwrap()
    {
        AirgappedResponse::VerifiedAddress(value) => value,
        _ => unreachable!(),
    };
    let descriptor = LianaDescriptor::from_str(&policy.full_descriptor()).unwrap();
    let address = descriptor
        .receive_descriptor()
        .derive(7.into(), &Secp256k1::verification_only())
        .address(Network::Bitcoin)
        .to_string();
    response
        .validate_for(&request, &address, "abcdef01")
        .unwrap();

    let stale = AddressVerificationRequest {
        index: 8,
        ..request
    };
    assert_eq!(
        response.validate_for(&stale, &address, "abcdef01"),
        Err(Error::WrongResponseType)
    );
}

#[test]
fn account_key_file_vectors_enforce_network() {
    let mainnet = include_str!("../test_assets/passport/account-mainnet.txt");
    let testnet = include_str!("../test_assets/passport/account-testnet.txt");
    let mainnet = PassportAccount::from_descriptor_key(mainnet, Network::Bitcoin).unwrap();
    let testnet = PassportAccount::from_descriptor_key(testnet, Network::Testnet4).unwrap();
    assert_eq!(mainnet.fingerprint.to_string(), "aabb0011");
    assert_eq!(testnet.fingerprint.to_string(), "9f141cf0");
    assert_eq!(
        PassportAccount::from_descriptor_key(
            include_str!("../test_assets/passport/account-testnet.txt"),
            Network::Bitcoin,
        ),
        Err(Error::InvalidNetwork)
    );
}

#[test]
fn passport_core_crypto_account_vector_is_accepted() {
    let encoded = hex::decode(concat!(
        "a2011aa1b2c3d40281d90134d90191d9019ad9012fa602f40358210279be667e",
        "f9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f8179804582000",
        "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f05",
        "d99d71a20100020006d99d70a301881830f500f507f502f5021aa1b2c3d40304",
        "081a11223344"
    ))
    .unwrap();
    let account = PassportAccount::from_crypto_account_cbor(&encoded, Network::Bitcoin).unwrap();
    assert_eq!(account.fingerprint.to_string(), "a1b2c3d4");
    assert_eq!(account.account_number().unwrap().to_string(), "7'");
    assert!(account
        .account
        .to_string()
        .starts_with("[a1b2c3d4/48'/0'/7'/2']xpub"));
}

#[test]
fn single_and_multipart_ur_vectors_are_stable() {
    let single = encode_ur(&UrPayload::bytes(b"Passport v1".to_vec()), 200).unwrap();
    assert_eq!(single.frames, [SINGLE_UR.trim()]);

    let expected: Vec<_> = MULTIPART_UR.lines().collect();
    let encoded = encode_ur(
        &UrPayload::bytes(b"Passport multipart protocol fixture".repeat(8)),
        80,
    )
    .unwrap();
    assert_eq!(encoded.frames, expected);

    let mut decoder = UrDecodeSession::new(UrType::Bytes, ScanLimits::default());
    let mut frames = expected;
    frames.reverse();
    frames.insert(1, frames[0]);
    let mut decoded = None;
    for frame in frames {
        if let DecodeProgress::Complete(payload) = decoder.receive(frame).unwrap() {
            decoded = Some(payload.data);
            break;
        }
    }
    assert_eq!(
        decoded.unwrap(),
        b"Passport multipart protocol fixture".repeat(8)
    );
}

#[test]
fn malformed_wrong_type_and_missing_fragments_fail_safely() {
    let mut wrong_type = UrDecodeSession::new(UrType::CryptoPsbt, ScanLimits::default());
    assert!(matches!(
        wrong_type.receive(SINGLE_UR.trim()),
        Err(Error::WrongUrType { .. })
    ));

    let mut corrupt = SINGLE_UR.trim().to_owned();
    corrupt.replace_range(corrupt.len() - 1.., "a");
    let mut decoder = UrDecodeSession::new(UrType::Bytes, ScanLimits::default());
    assert!(matches!(
        decoder.receive(&corrupt),
        Err(Error::InvalidUr(_))
    ));

    let mut incomplete = UrDecodeSession::new(UrType::Bytes, ScanLimits::default());
    for frame in MULTIPART_UR.lines().take(3) {
        assert!(matches!(
            incomplete.receive(frame),
            Ok(DecodeProgress::Incomplete { .. })
        ));
    }
}

#[test]
fn mixed_and_oversized_multipart_sessions_are_rejected_before_allocation() {
    let first = encode_ur(&UrPayload::bytes(vec![1; 500]), 100).unwrap();
    let second = encode_ur(&UrPayload::bytes(vec![2; 500]), 100).unwrap();
    let mut decoder = UrDecodeSession::new(UrType::Bytes, ScanLimits::default());
    decoder.receive(&first.frames[0]).unwrap();
    assert_eq!(decoder.receive(&second.frames[1]), Err(Error::MixedSession));

    let limits = ScanLimits {
        maximum_decoded_bytes: 128,
        ..ScanLimits::default()
    };
    let mut oversized = UrDecodeSession::new(UrType::Bytes, limits);
    assert!(matches!(
        oversized.receive(&first.frames[0]),
        Err(Error::PayloadTooLarge { .. })
    ));

    // Build an externally supplied sequence that exceeds Liana's encoder cap
    // to verify the decoder independently rejects the declared geometry.
    let mut cbor = minicbor::Encoder::new(Vec::new());
    cbor.bytes(&vec![3; 500]).unwrap();
    let cbor = cbor.into_writer();
    let mut encoder = foundation_ur::Encoder::new();
    encoder.start("bytes", &cbor, 1);
    assert!(encoder.sequence_count() > 128);
    let first_oversized_frame = encoder.next_part().to_string();
    let mut bounded = UrDecodeSession::new(UrType::Bytes, ScanLimits::default());
    assert!(matches!(
        bounded.receive(&first_oversized_frame),
        Err(Error::TooManyFragments { .. })
    ));
}

#[test]
fn response_decoder_rejects_an_unexpected_json_envelope() {
    assert!(matches!(
        ExpectedResponse::VerifiedAddress.decode(UrPayload::bytes(POLICY.to_vec())),
        Err(Error::InvalidJson(_))
    ));

    // Compile-time use of the public response type also locks the v1 API.
    let _: fn(&[u8]) -> Result<VerifiedAddress, Error> = VerifiedAddress::from_json;
}

#[test]
fn psbt_vectors_roundtrip_and_only_signatures_are_merged() {
    use liana::miniscript::bitcoin::{
        ecdsa,
        psbt::{raw, Psbt},
        secp256k1::{Message, SecretKey},
    };

    let mut unsigned =
        Psbt::from_str(include_str!("../test_assets/passport/unsigned.psbt.base64").trim())
            .unwrap();
    let mut signed =
        Psbt::from_str(include_str!("../test_assets/passport/partially-signed.psbt.base64").trim())
            .unwrap();
    assert_eq!(unsigned.unsigned_tx, signed.unsigned_tx);
    assert_eq!(unsigned.inputs[0].partial_sigs.len(), 0);
    assert_eq!(signed.inputs[0].partial_sigs.len(), 1);

    let proprietary = raw::ProprietaryKey {
        prefix: b"liana-test".to_vec(),
        subtype: 7,
        key: vec![1, 2, 3],
    };
    unsigned
        .proprietary
        .insert(proprietary.clone(), vec![4, 5, 6]);
    signed
        .proprietary
        .insert(proprietary.clone(), vec![4, 5, 6]);
    let merged = validate_and_merge_psbt(&unsigned, &signed).unwrap();
    assert_eq!(merged.inputs[0].partial_sigs.len(), 1);
    assert_eq!(merged.proprietary.get(&proprietary), Some(&vec![4, 5, 6]));

    let mut invalid_signature = signed.clone();
    let (public_key, signature) = invalid_signature.inputs[0]
        .partial_sigs
        .iter()
        .next()
        .map(|(public_key, signature)| (*public_key, *signature))
        .unwrap();
    let secp = Secp256k1::signing_only();
    let wrong_signature = secp.sign_ecdsa(
        &Message::from_digest([42; 32]),
        &SecretKey::from_slice(&[7; 32]).unwrap(),
    );
    invalid_signature.inputs[0].partial_sigs.insert(
        public_key,
        ecdsa::Signature {
            signature: wrong_signature,
            sighash_type: signature.sighash_type,
        },
    );
    assert!(matches!(
        validate_and_merge_psbt(&unsigned, &invalid_signature),
        Err(Error::InvalidPsbt(_))
    ));

    let encoded = encode_ur(&UrPayload::psbt(&signed), 100).unwrap();
    let mut decoder = UrDecodeSession::new(UrType::CryptoPsbt, ScanLimits::default());
    let mut decoded = None;
    for frame in encoded.frames {
        if let DecodeProgress::Complete(payload) = decoder.receive(&frame).unwrap() {
            decoded = Some(payload);
            break;
        }
    }
    match ExpectedResponse::SignedPsbt
        .decode(decoded.unwrap())
        .unwrap()
    {
        AirgappedResponse::SignedPsbt(value) => assert_eq!(value, signed),
        _ => unreachable!(),
    }

    let mut wrong_transaction = signed.clone();
    wrong_transaction.unsigned_tx.output[0].value = liana::miniscript::bitcoin::Amount::from_sat(1);
    assert!(matches!(
        validate_and_merge_psbt(&unsigned, &wrong_transaction),
        Err(Error::InvalidPsbt(_))
    ));

    let mut mutated_metadata = signed.clone();
    mutated_metadata
        .proprietary
        .insert(proprietary.clone(), vec![7]);
    let merged = validate_and_merge_psbt(&unsigned, &mutated_metadata).unwrap();
    assert_eq!(merged.proprietary.get(&proprietary), Some(&vec![4, 5, 6]));
}

#[test]
fn taproot_key_path_signature_is_verified_and_merged() {
    use liana::{
        miniscript::bitcoin::{
            absolute,
            bip32::{ChildNumber, DerivationPath},
            psbt::{Input, Output, Psbt},
            transaction, Address, Amount, OutPoint, Sequence, Transaction, TxIn, TxOut,
        },
        signer::HotSigner,
    };

    let secp = Secp256k1::new();
    let signer = HotSigner::from_str(
        Network::Bitcoin,
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )
    .unwrap();
    let account_path = DerivationPath::from_str("m/86'/0'/0'").unwrap();
    let relative = [
        ChildNumber::from_normal_idx(0).unwrap(),
        ChildNumber::from_normal_idx(0).unwrap(),
    ];
    let account = signer.xpub_at(&account_path, &secp);
    let child = account.derive_pub(&secp, &relative).unwrap();
    let internal_key = child.public_key.x_only_public_key().0;
    let mut input = Input {
        witness_utxo: Some(TxOut {
            value: Amount::from_sat(10_000),
            script_pubkey: Address::p2tr(&secp, internal_key, None, Network::Bitcoin)
                .script_pubkey(),
        }),
        tap_internal_key: Some(internal_key),
        ..Input::default()
    };
    input.tap_key_origins.insert(
        internal_key,
        (
            vec![],
            (signer.fingerprint(&secp), account_path.extend(relative)),
        ),
    );
    let unsigned = Psbt {
        unsigned_tx: Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                ..TxIn::default()
            }],
            output: vec![TxOut::NULL],
        },
        version: 0,
        xpub: Default::default(),
        proprietary: Default::default(),
        unknown: Default::default(),
        inputs: vec![input],
        outputs: vec![Output::default()],
    };
    let signed = signer.sign_psbt(unsigned.clone(), &secp).unwrap();
    assert!(signed.inputs[0].tap_key_sig.is_some());
    let merged = validate_and_merge_psbt(&unsigned, &signed).unwrap();
    assert_eq!(merged.inputs[0].tap_key_sig, signed.inputs[0].tap_key_sig);
}

#[test]
fn repeated_multisig_rounds_preserve_and_verify_each_signature() {
    use std::collections::BTreeMap;

    use liana::{
        descriptors::{LianaPolicy, PathInfo},
        miniscript::{
            bitcoin::{
                absolute,
                bip32::DerivationPath,
                psbt::{Input, Output, Psbt},
                transaction, Amount, OutPoint, Sequence, Transaction, TxIn, TxOut,
            },
            descriptor::DescriptorPublicKey,
        },
        signer::HotSigner,
    };

    let secp = Secp256k1::new();
    let signer_a = HotSigner::from_str(
        Network::Bitcoin,
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )
    .unwrap();
    let signer_b = HotSigner::from_str(
        Network::Bitcoin,
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
    )
    .unwrap();
    let signer_c = HotSigner::from_str(
        Network::Bitcoin,
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
    )
    .unwrap();
    let account_path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
    let key = |signer: &HotSigner, branches: &str| {
        DescriptorPublicKey::from_str(&format!(
            "[{}/48'/0'/0'/2']{}/{branches}/*",
            signer.fingerprint(&secp),
            signer.xpub_at(&account_path, &secp),
        ))
        .unwrap()
    };
    let primary = PathInfo::Multi(2, vec![key(&signer_a, "<0;1>"), key(&signer_b, "<0;1>")]);
    let recovery = PathInfo::Single(key(&signer_c, "<2;3>"));
    let descriptor = LianaDescriptor::new(
        LianaPolicy::new_legacy(primary, BTreeMap::from([(10, recovery)])).unwrap(),
    );
    let coin = descriptor.receive_descriptor().derive(0.into(), &secp);
    let mut input = Input::default();
    coin.update_psbt_in(&mut input);
    input.witness_utxo = Some(TxOut {
        value: Amount::from_sat(10_000),
        script_pubkey: coin.script_pubkey(),
    });
    let unsigned = Psbt {
        unsigned_tx: Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                ..TxIn::default()
            }],
            output: vec![TxOut::NULL],
        },
        version: 0,
        xpub: BTreeMap::new(),
        proprietary: BTreeMap::new(),
        unknown: BTreeMap::new(),
        inputs: vec![input],
        outputs: vec![Output::default()],
    };

    let returned_a = signer_a.sign_psbt(unsigned.clone(), &secp).unwrap();
    let round_one = validate_and_merge_psbt(&unsigned, &returned_a).unwrap();
    assert_eq!(round_one.inputs[0].partial_sigs.len(), 1);

    let returned_b = signer_b.sign_psbt(round_one.clone(), &secp).unwrap();
    let round_two = validate_and_merge_psbt(&round_one, &returned_b).unwrap();
    assert_eq!(round_two.inputs[0].partial_sigs.len(), 2);
    assert!(round_one.inputs[0]
        .partial_sigs
        .keys()
        .all(|key| round_two.inputs[0].partial_sigs.contains_key(key)));
}
