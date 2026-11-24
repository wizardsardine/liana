use std::str::FromStr;

use bip377::{
    backend::{rust_bitcoin::RustBitcoin, Backend},
    ll::{
        self, encode_response, request, response, types::ExtendedKey, Message, Request, RequestId,
        SignResponseKind,
    },
    response::Mismatch,
};
use bwk_qr::{
    config::QrSize,
    decoder::{Decoded, Decoder},
    encoder::Encoder,
    image::Image,
};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{
        base64::{prelude::BASE64_STANDARD, Engine},
        bip32::{ChildNumber, DerivationPath, Fingerprint, Xpub},
        psbt::{Input, Output, Psbt},
        Address, Network,
    },
};
use liana_gui::airgap::device::{
    get_descriptor_keys, get_keys, Answer, Exchange, Registration, RequestBuilder,
};
use liana_ui::component::modal::LIANA_ACCOUNTS;

const ID: RequestId = RequestId([0, 1]);
const FRAME_SIZE: QrSize = QrSize::S53;
const NETWORK: Network = Network::Testnet;
const SIGNER_XPUB: &str = "tpubDFnReAwXvYd6RA46X55HuFpmvZsLanDrwHAUsdYEGEpNGTRnCdbDRXJGLTwDeqKURCPZUDgdkuuu9dYkuBNQHmSNBUu7V2CdLKwpJjx2JuC";
const SIGNER_FINGERPRINT: &str = "9f141cf0";

fn descriptor() -> LianaDescriptor {
    LianaDescriptor::from_str(
        include_str!("../test_assets/airgap/multisig-testnet.descriptor").trim(),
    )
    .unwrap()
}

fn psbt(name: &str) -> Psbt {
    let base64 = match name {
        "unsigned" => include_str!("../test_assets/airgap/unsigned.psbt.base64"),
        "partially-signed" => include_str!("../test_assets/airgap/partially-signed.psbt.base64"),
        other => panic!("unknown fixture {}", other),
    };
    Psbt::deserialize(&BASE64_STANDARD.decode(base64.trim()).unwrap()).unwrap()
}

fn builder() -> RequestBuilder {
    RequestBuilder {
        alias: "Liana".to_owned(),
        descriptor: descriptor(),
        registration: Registration::default(),
        capabilities: capabilities(),
    }
}

fn capabilities() -> response::Capabilities {
    response::Capabilities(
        response::Capability::Bip380Descriptors.bit()
            | response::Capability::Bip388WalletPolicies.bit(),
    )
}

fn exchange(body: bip377::request::Body<RustBitcoin>) -> Exchange {
    Exchange::new(body, ID)
}

fn frames(exchange: &Exchange, size: QrSize) -> Vec<Image> {
    Encoder::new(size)
        .encode_bbqr(&exchange.payload().unwrap())
        .unwrap()
}

fn scan_request(exchange: &Exchange) -> Request {
    scan(&frames(exchange, FRAME_SIZE))
}

fn scan(frames: &[Image]) -> Request {
    let mut decoder = Decoder::new();
    for frame in frames {
        if let Some(Decoded::Bytes(bytes)) = decoder.process(frame).unwrap() {
            return match ll::decode(&bytes).unwrap() {
                Message::Request(request) => request,
                Message::Response(_) => panic!("a request encoded to a response"),
            };
        }
    }
    panic!("the frames did not reassemble into a message")
}

fn answer(
    exchange: &Exchange,
    body: response::Body,
) -> Result<Answer, liana_gui::airgap::error::Error> {
    let bytes = encode_response(&ll::Response { id: ID, body }).unwrap();
    exchange.read(&bytes)
}

fn xpubs_body(count: usize) -> response::Body {
    let xpub = Xpub::from_str(SIGNER_XPUB).unwrap();
    response::Body::Keys(response::Keys {
        keys: vec![RustBitcoin::to_ll_xpub(&xpub); count],
        fingerprint: RustBitcoin::to_ll_fingerprint(
            &Fingerprint::from_str(SIGNER_FINGERPRINT).unwrap(),
        ),
        vendor: "test vendor".to_owned(),
        model: "test signer".to_owned(),
        version: response::FirmwareVersion {
            major: 1,
            minor: 2,
            patch: 3,
            flag: response::ReleaseFlag::Beta(2),
        },
        capabilities: response::Capabilities(
            response::Capability::MiniscriptSegwitV0.bit()
                | response::Capability::MiniscriptTaproot.bit(),
        ),
    })
}

#[test]
fn get_xpubs_asks_for_ten_accounts() {
    let exchange = exchange(get_keys(NETWORK));
    let request::Body::GetKeys(body) = scan_request(&exchange).body else {
        panic!("expected a Get Keys request")
    };
    assert_eq!(body.keys.len(), LIANA_ACCOUNTS as usize);
    let paths: Vec<String> = body
        .keys
        .iter()
        .map(|path| RustBitcoin::from_ll_path(path).to_string())
        .collect();
    assert_eq!(paths[0], "48'/1'/0'/2'");
    assert_eq!(paths[9], "48'/1'/9'/2'");
}

#[test]
fn get_xpubs_reads_every_account_back() {
    let exchange = exchange(get_keys(NETWORK));
    let Answer::Xpubs(signer) = answer(&exchange, xpubs_body(LIANA_ACCOUNTS as usize)).unwrap()
    else {
        panic!("expected the accounts back")
    };
    assert_eq!(signer.fingerprint.to_string(), SIGNER_FINGERPRINT);
    assert_eq!(signer.vendor, "test vendor");
    assert_eq!(signer.model, "test signer");
    assert_eq!(
        signer.version,
        response::FirmwareVersion {
            major: 1,
            minor: 2,
            patch: 3,
            flag: response::ReleaseFlag::Beta(2),
        }
    );
    assert!(signer
        .capabilities
        .supports(response::Capability::MiniscriptSegwitV0));
    assert!(signer
        .capabilities
        .supports(response::Capability::MiniscriptTaproot));
    assert_eq!(signer.keys.len(), LIANA_ACCOUNTS as usize);
    assert_eq!(
        signer.keys[0],
        (
            DerivationPath::from_str("m/48'/1'/0'/2'").unwrap(),
            Xpub::from_str(SIGNER_XPUB).unwrap()
        )
    );
    assert_eq!(
        signer.keys[9].0,
        DerivationPath::from_str("m/48'/1'/9'/2'").unwrap()
    );
}

#[test]
fn the_descriptor_key_request_asks_each_origin_path_once() {
    let bip377::request::Body::GetKeys(request) = get_descriptor_keys(&descriptor()) else {
        panic!("not a key request");
    };
    assert_eq!(
        request.keys,
        vec![DerivationPath::from_str("m/48'/1'/0'/2'").unwrap()]
    );
}

fn scanned_for_descriptor(body: response::Body) -> liana_gui::airgap::device::Signer {
    let exchange = exchange(get_descriptor_keys(&descriptor()));
    let Answer::Xpubs(signer) = answer(&exchange, body).unwrap() else {
        panic!("not a key answer");
    };
    signer
}

#[test]
fn a_device_holding_a_wallet_key_is_recognised() {
    assert!(scanned_for_descriptor(xpubs_body(1)).holds_keys_of(&descriptor()));
}

#[test]
fn a_device_returning_another_key_is_not_recognised() {
    let response::Body::Keys(mut keys) = xpubs_body(1) else {
        unreachable!();
    };
    keys.keys = vec![RustBitcoin::to_ll_xpub(&Xpub::from_str("tpubDDwKEc4i4k8rBgVLGxytHrP13VVYucUGmL2cadux7AfMwMnRHKcw1YZKt9SMB4fWut7ZAiZqPefzm3BBCNXLZMxDrWJ4Q6VA1AFB6b8GzbT").unwrap())];
    assert!(!scanned_for_descriptor(response::Body::Keys(keys)).holds_keys_of(&descriptor()));
}

#[test]
fn a_device_outside_the_wallet_is_not_recognised() {
    let response::Body::Keys(mut keys) = xpubs_body(1) else {
        unreachable!();
    };
    keys.fingerprint = RustBitcoin::to_ll_fingerprint(&Fingerprint::from_str("00000001").unwrap());
    assert!(!scanned_for_descriptor(response::Body::Keys(keys)).holds_keys_of(&descriptor()));
}

#[test]
fn a_signer_answering_fewer_accounts_than_asked_is_rejected() {
    let exchange = exchange(get_keys(NETWORK));
    assert!(matches!(
        answer(&exchange, xpubs_body(LIANA_ACCOUNTS as usize - 1)),
        Err(liana_gui::airgap::error::Error::Mismatch(
            Mismatch::KeyCount { .. }
        ))
    ));
}

#[test]
fn a_signer_answering_more_accounts_than_asked_is_rejected() {
    let exchange = exchange(get_keys(NETWORK));
    assert!(matches!(
        answer(&exchange, xpubs_body(LIANA_ACCOUNTS as usize + 1)),
        Err(liana_gui::airgap::error::Error::Mismatch(
            Mismatch::KeyCount { .. }
        ))
    ));
}

#[test]
fn testnet_keys_are_accepted_on_every_test_network() {
    for network in [
        Network::Testnet,
        Network::Testnet4,
        Network::Signet,
        Network::Regtest,
    ] {
        let exchange = exchange(get_keys(network));
        assert!(
            matches!(
                answer(&exchange, xpubs_body(LIANA_ACCOUNTS as usize)),
                Ok(Answer::Xpubs(_))
            ),
            "{} refused a testnet key",
            network
        );
    }
}

#[test]
fn a_signer_answering_no_account_is_rejected() {
    let exchange = exchange(get_keys(NETWORK));
    assert!(matches!(
        answer(&exchange, xpubs_body(0)),
        Err(liana_gui::airgap::error::Error::Mismatch(
            Mismatch::KeyCount { .. }
        ))
    ));
}

#[test]
fn register_carries_the_descriptor() {
    let exchange = exchange(builder().register().unwrap());
    let request::Body::RegisterDescriptor(body) = scan_request(&exchange).body else {
        panic!("expected a Register Descriptor request")
    };
    assert_eq!(body.descriptor_alias, "Liana");
    let Some(request::DescriptorBody::Bip388 { keys, policy }) = body.descriptor else {
        panic!("the descriptor must be sent as a wallet policy")
    };

    assert_eq!(keys.len(), 3);
    assert!(policy.starts_with("wsh(or_i("));
    assert!(
        !policy.contains("tpub"),
        "the template must only carry placeholders"
    );
    assert!(
        !policy.contains('#'),
        "the checksum has no place in a policy"
    );
    for index in 0..keys.len() {
        assert!(
            policy.contains(&format!("@{index}")),
            "@{} is unused",
            index
        );
    }

    let policy_bytes = keys.iter().map(|k| k.len()).sum::<usize>() + policy.len();
    assert!(
        policy_bytes < descriptor().to_string().len(),
        "the policy form should be the smaller one"
    );
}

#[test]
fn a_signer_without_wallet_policies_gets_the_descriptor() {
    let capabilities = response::Capabilities(response::Capability::Bip380Descriptors.bit());
    let request::Body::RegisterDescriptor(body) = scan_request(&exchange(
        RequestBuilder {
            capabilities,
            ..builder()
        }
        .register()
        .unwrap(),
    ))
    .body
    else {
        panic!("expected a Register Descriptor request")
    };
    assert_eq!(
        body.descriptor,
        Some(request::DescriptorBody::Bip380(descriptor().to_string()))
    );
}

#[test]
fn a_signer_taking_no_descriptor_form_is_not_asked() {
    let capabilities = response::Capabilities(0);
    assert!(matches!(
        RequestBuilder {
            capabilities,
            ..builder()
        }
        .register(),
        Err(liana_gui::airgap::error::Error::UnsupportedDescriptor)
    ));
}

#[test]
fn register_keeps_what_the_signer_reported() {
    let exchange = exchange(builder().register().unwrap());
    let body = response::Body::Registration(response::Registration {
        descriptor_alias: "Liana".to_owned(),
        registered: true,
        stored: Some(false),
        proof: Some(vec![7; 32]),
    });
    let Answer::Registered(registration) = answer(&exchange, body).unwrap() else {
        panic!("expected a registration")
    };
    assert_eq!(registration.stored, Some(false));
    assert_eq!(registration.proof, Some(vec![7; 32]));
}

#[test]
fn a_registration_the_signer_reports_as_not_done_is_rejected() {
    let exchange = exchange(builder().register().unwrap());
    let body = response::Body::Registration(response::Registration {
        descriptor_alias: "Liana".to_owned(),
        registered: false,
        stored: Some(true),
        proof: None,
    });
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::NotRegistered)
    ));
}

#[test]
fn a_registration_under_another_alias_is_rejected() {
    let exchange = exchange(builder().register().unwrap());
    let body = response::Body::Registration(response::Registration {
        descriptor_alias: "Other".to_owned(),
        registered: true,
        stored: Some(true),
        proof: None,
    });
    assert!(matches!(
        answer(&exchange, body).unwrap_err(),
        liana_gui::airgap::error::Error::Mismatch(Mismatch::Alias { expected, received })
            if expected == "Liana" && received == "Other"
    ));
}

#[test]
fn every_request_describes_the_wallet_the_same_way() {
    let registered = {
        let request::Body::RegisterDescriptor(body) =
            scan_request(&exchange(builder().register().unwrap())).body
        else {
            panic!("expected a registration")
        };
        body.descriptor
            .expect("a registration carries the descriptor")
    };
    let signed = {
        let request::Body::Sign(body) =
            scan_request(&exchange(builder().sign(&psbt("unsigned")).unwrap())).body
        else {
            panic!("expected a signing request")
        };
        body.descriptors[0].descriptor.clone()
    };
    assert_eq!(Some(registered), signed);
}

#[test]
fn a_stateless_signer_gets_the_descriptor_replayed() {
    let stored = RequestBuilder {
        registration: Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(true),
            proof: None,
        },
        ..builder()
    };
    let stateless = RequestBuilder {
        registration: Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(false),
            proof: Some(vec![7; 32]),
        },
        ..builder()
    };

    let verify = |builder: RequestBuilder| {
        let exchange = exchange(
            builder
                .verify_address(&address(), ChildNumber::Normal { index: 3 })
                .unwrap(),
        );
        let request::Body::VerifyAddress(body) = scan_request(&exchange).body else {
            panic!("expected an Address Verification request")
        };
        body
    };

    let body = verify(stored);
    assert_eq!(body.descriptor, None);
    assert_eq!(body.proof, None);

    let body = verify(stateless);
    assert!(matches!(
        body.descriptor,
        Some(request::DescriptorBody::Bip388 { .. })
    ));
    assert_eq!(body.proof, Some(vec![7; 32]));
}

#[test]
fn a_signer_that_stored_the_descriptor_is_asked_to_sign_by_alias() {
    let builder = RequestBuilder {
        registration: Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(true),
            proof: None,
        },
        ..builder()
    };
    let request::Body::Sign(body) =
        scan_request(&exchange(builder.sign(&psbt("unsigned")).unwrap())).body
    else {
        panic!("expected a signing request")
    };
    assert_eq!(body.descriptors[0].alias, "Liana");
    assert_eq!(body.descriptors[0].descriptor, None);
}

fn address() -> Address {
    descriptor()
        .receive_descriptor()
        .derive(ChildNumber::Normal { index: 3 }, &secp())
        .address(NETWORK)
}

fn secp() -> liana::miniscript::bitcoin::secp256k1::Secp256k1<
    liana::miniscript::bitcoin::secp256k1::VerifyOnly,
> {
    liana::miniscript::bitcoin::secp256k1::Secp256k1::verification_only()
}

#[test]
fn verify_address_names_the_branch_and_index() {
    let exchange = exchange(
        builder()
            .verify_address(&address(), ChildNumber::Normal { index: 3 })
            .unwrap(),
    );
    let request::Body::VerifyAddress(body) = scan_request(&exchange).body else {
        panic!("expected an Address Verification request")
    };
    assert_eq!(
        RustBitcoin::from_ll_path(&body.derivation_path).to_string(),
        "0/3"
    );
    assert_eq!(body.address, Some(address().to_string()));
}

#[test]
fn verify_address_accepts_the_matching_uri() {
    let exchange = exchange(
        builder()
            .verify_address(&address(), ChildNumber::Normal { index: 3 })
            .unwrap(),
    );
    let body = response::Body::AddressUri(response::AddressUri {
        uri: format!("bitcoin:{}?amount=0.1", address()),
    });
    assert!(matches!(
        answer(&exchange, body).unwrap(),
        Answer::AddressVerified
    ));
}

#[test]
fn verify_address_rejects_another_address() {
    let exchange = exchange(
        builder()
            .verify_address(&address(), ChildNumber::Normal { index: 3 })
            .unwrap(),
    );
    let other = descriptor()
        .receive_descriptor()
        .derive(ChildNumber::Normal { index: 4 }, &secp())
        .address(NETWORK);
    let body = response::Body::AddressUri(response::AddressUri {
        uri: format!("bitcoin:{other}"),
    });
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::Mismatch(
            Mismatch::Address { .. }
        ))
    ));
}

#[test]
fn verify_address_rejects_an_empty_answer() {
    let exchange = exchange(
        builder()
            .verify_address(&address(), ChildNumber::Normal { index: 3 })
            .unwrap(),
    );
    let body = response::Body::AddressUri(response::AddressUri { uri: String::new() });
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::InvalidAddressUri(_))
    ));
}

fn verify_address_answer(uri: String) -> Result<Answer, liana_gui::airgap::error::Error> {
    let exchange = exchange(
        builder()
            .verify_address(&address(), ChildNumber::Normal { index: 3 })
            .unwrap(),
    );
    answer(
        &exchange,
        response::Body::AddressUri(response::AddressUri { uri }),
    )
}

#[test]
fn verify_address_rejects_a_bare_address() {
    assert!(matches!(
        verify_address_answer(address().to_string()),
        Err(liana_gui::airgap::error::Error::InvalidAddressUri(_))
    ));
}

#[test]
fn verify_address_accepts_any_case_and_several_parameters() {
    let uri = format!("BITCOIN:{}?amount=0.1&label=Liana", address());
    assert!(matches!(
        verify_address_answer(uri),
        Ok(Answer::AddressVerified)
    ));
}

#[test]
fn signing_carries_the_psbt_across_frames() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    assert!(frames(&exchange, FRAME_SIZE).len() > 1);
    let request::Body::Sign(body) = scan_request(&exchange).body else {
        panic!("expected a Signing request")
    };
    assert_eq!(body.psbt, psbt("unsigned").serialize());
    assert_eq!(body.want_kind, Some(SignResponseKind::Signatures));
    assert!(body.accept_trimmed);
    assert_eq!(body.descriptors.len(), 1);
    assert_eq!(body.descriptors[0].alias, "Liana");
}

#[test]
fn every_frame_of_a_message_is_the_same_size() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let frames = frames(&exchange, FRAME_SIZE);
    assert!(frames.len() > 1);
    let first = &frames[0];
    for frame in &frames {
        assert_eq!(frame.side(), first.side());
    }
}

#[test]
fn reframing_keeps_the_request() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let sparse = frames(&exchange, FRAME_SIZE);
    let dense = frames(&exchange, QrSize::S97);
    assert!(dense.len() < sparse.len());
    assert_eq!(scan(&dense), scan(&sparse));
}

#[test]
fn signing_merges_the_returned_signature() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let body = response::Body::Signed(response::Signed::Psbt(psbt("partially-signed").serialize()));
    let Answer::Signed(merged) = answer(&exchange, body).unwrap() else {
        panic!("expected a signed PSBT")
    };
    assert_eq!(merged.unsigned_tx, psbt("unsigned").unsigned_tx);
    assert!(merged
        .inputs
        .iter()
        .any(|input| !input.partial_sigs.is_empty()));
}

#[test]
fn signing_merges_a_trimmed_psbt() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let mut trimmed = psbt("partially-signed");
    for input in &mut trimmed.inputs {
        *input = Input {
            partial_sigs: std::mem::take(&mut input.partial_sigs),
            tap_script_sigs: std::mem::take(&mut input.tap_script_sigs),
            ..Input::default()
        };
    }
    for output in &mut trimmed.outputs {
        *output = Output::default();
    }
    let body = response::Body::Signed(response::Signed::Psbt(trimmed.serialize()));
    let Answer::Signed(merged) = answer(&exchange, body).unwrap() else {
        panic!("expected a signed PSBT")
    };
    let unsigned = psbt("unsigned");
    assert_eq!(merged.outputs, unsigned.outputs);
    for (merged, unsigned) in merged.inputs.iter().zip(&unsigned.inputs) {
        assert_eq!(merged.witness_utxo, unsigned.witness_utxo);
        assert_eq!(merged.bip32_derivation, unsigned.bip32_derivation);
    }
    assert!(merged
        .inputs
        .iter()
        .any(|input| !input.partial_sigs.is_empty()));
}

#[test]
fn signing_rejects_unreadable_signature_bytes() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let body = response::Body::Signed(response::Signed::Psbt(vec![0; 8]));
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::InvalidPsbt(_))
    ));
}

#[test]
fn a_signature_naming_an_absent_input_is_rejected() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let body = response::Body::Signed(response::Signed::Signatures(vec![
        response::SignatureEntry::TapKey {
            input_index: 42,
            signature: vec![0; 64],
        },
    ]));
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::Mismatch(
            Mismatch::InputIndex { index: 42, .. }
        ))
    ));
}

#[test]
fn a_response_to_another_request_is_rejected() {
    let exchange = exchange(get_keys(NETWORK));
    let bytes = encode_response(&ll::Response {
        id: RequestId([9; 2]),
        body: xpubs_body(LIANA_ACCOUNTS as usize),
    })
    .unwrap();
    let error = exchange.read(&bytes).unwrap_err();
    assert!(matches!(
        error,
        liana_gui::airgap::error::Error::RequestIdMismatch
    ));
    assert!(error.to_string().contains("earlier request"));
}

#[test]
fn a_stale_answer_does_not_poison_the_exchange() {
    let exchange = exchange(get_keys(NETWORK));

    let stale = encode_response(&ll::Response {
        id: RequestId([9; 2]),
        body: xpubs_body(LIANA_ACCOUNTS as usize),
    })
    .unwrap();
    assert!(matches!(
        exchange.read(&stale),
        Err(liana_gui::airgap::error::Error::RequestIdMismatch)
    ));

    assert!(matches!(
        answer(&exchange, xpubs_body(LIANA_ACCOUNTS as usize)),
        Ok(Answer::Xpubs(_))
    ));
}

#[test]
fn a_response_of_the_wrong_type_is_rejected() {
    let exchange = exchange(get_keys(NETWORK));
    let body = response::Body::AddressUri(response::AddressUri {
        uri: format!("bitcoin:{}", address()),
    });
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::MessageTypeMismatch { .. })
    ));
}

#[test]
fn a_request_where_a_response_belongs_is_rejected() {
    let exchange = exchange(get_keys(NETWORK));
    let bytes = ll::encode_request(&Request {
        id: ID,
        body: request::Body::GetKeys(request::GetKeys { keys: Vec::new() }),
    })
    .unwrap();
    assert!(matches!(
        exchange.read(&bytes),
        Err(liana_gui::airgap::error::Error::NotAResponse)
    ));
}

#[test]
fn garbage_is_rejected() {
    let exchange = exchange(get_keys(NETWORK));
    assert!(matches!(
        exchange.read(b"not a message"),
        Err(liana_gui::airgap::error::Error::Decode(_))
    ));
}

#[test]
fn every_refusal_code_is_reported_as_a_refusal() {
    let codes = [
        response::Error::UserDeclined,
        response::Error::UnsupportedVersion,
        response::Error::MalformedRequest,
        response::Error::UnknownDescriptorAlias,
        response::Error::DescriptorRegistrationFailed,
        response::Error::UnsupportedDescriptorForm,
        response::Error::InvalidProof,
        response::Error::AddressMismatch,
        response::Error::NothingToSign,
        response::Error::InvalidPsbt,
        response::Error::InternalError,
        response::Error::MusigSession,
        response::Error::AlreadyRegistered,
        response::Error::Vendor,
    ];
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    for code in codes {
        let body = response::Body::Error(response::ErrorBody {
            message_type: ll::MessageType::Signing,
            error: code,
            message: "refused".to_owned(),
        });
        let error = answer(&exchange, body).unwrap_err();
        assert!(matches!(
            &error,
            liana_gui::airgap::error::Error::SignerRefused(refused, message)
                if *refused == code && message == "refused"
        ));
        assert_eq!(error.to_string(), "The device refused: refused");
    }
}

#[test]
fn a_refusal_of_another_request_type_is_rejected() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let body = response::Body::Error(response::ErrorBody {
        message_type: ll::MessageType::GetKeys,
        error: response::Error::UserDeclined,
        message: String::new(),
    });
    assert!(matches!(
        answer(&exchange, body).unwrap_err(),
        liana_gui::airgap::error::Error::MessageTypeMismatch {
            expected: "signing",
            received: "key export",
        }
    ));
}

#[test]
fn frames_reassemble_out_of_order_and_with_duplicates() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let mut frames = frames(&exchange, FRAME_SIZE);
    frames.reverse();
    frames.insert(0, frames[0].clone());

    let mut decoder = Decoder::new();
    let mut payload = None;
    for frame in &frames {
        if let Some(Decoded::Bytes(bytes)) = decoder.process(frame).unwrap() {
            payload = Some(bytes);
        }
    }
    let bytes = payload.expect("the frames did not reassemble");
    assert_eq!(
        ll::decode(&bytes).unwrap(),
        Message::Request(scan_request(&exchange))
    );
}

#[test]
fn a_partial_scan_reports_progress_and_yields_nothing() {
    let exchange = exchange(builder().sign(&psbt("unsigned")).unwrap());
    let frames = frames(&exchange, FRAME_SIZE);
    let mut decoder = Decoder::new();
    for frame in &frames[..frames.len() - 1] {
        assert!(!matches!(
            decoder.process(frame).unwrap(),
            Some(Decoded::Bytes(_))
        ));
    }
    let progress = decoder.progress().expect("progress should be reported");
    assert_eq!(progress.seen(), frames.len() - 1);
    assert_eq!(progress.total(), frames.len());
    assert_eq!(progress.last, frames.len() - 2);
    assert_eq!(
        progress.received.iter().position(|read| !read),
        Some(frames.len() - 1)
    );
}

#[test]
fn an_xpub_the_signer_cannot_have_derived_is_rejected() {
    let exchange = exchange(get_keys(NETWORK));
    let body = response::Body::Keys(response::Keys {
        keys: vec![ExtendedKey([0; 78]); LIANA_ACCOUNTS as usize],
        fingerprint: RustBitcoin::to_ll_fingerprint(
            &Fingerprint::from_str(SIGNER_FINGERPRINT).unwrap(),
        ),
        vendor: "test vendor".to_owned(),
        model: "test signer".to_owned(),
        version: response::FirmwareVersion {
            major: 1,
            minor: 0,
            patch: 0,
            flag: response::ReleaseFlag::Stable,
        },
        capabilities: response::Capabilities(1),
    });
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::UnknownXpub(_))
    ));
}
