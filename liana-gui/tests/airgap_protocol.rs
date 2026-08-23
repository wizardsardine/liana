//! The wallet half of the signing-flow protocol, driven through real QR frames.
//!
//! Every test renders the request Liana would show, scans those frames back with
//! the same decoder the camera thread runs, and answers as a signer would. So a
//! break in the framing, the codec or the response checking fails here.

use std::str::FromStr;

use bip377::{
    backend::{Active, Backend},
    ll::{
        self, encode_response, request, response, types::ExtendedKey, Message, Request, RequestId,
        SignResponseKind,
    },
};
use bwk_qr::{
    config::QrSize,
    decoder::{Decoded, Decoder},
};
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{
        base64::{prelude::BASE64_STANDARD, Engine},
        bip32::{ChildNumber, Fingerprint, Xpub},
        psbt::Psbt,
        Address, Network,
    },
};
use liana_gui::airgap::{
    device::{Capabilities, Registration},
    exchange::{Answer, Ask, Exchange, Wallet, ACCOUNTS},
};

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

fn wallet() -> Wallet {
    Wallet {
        alias: "Liana".to_owned(),
        descriptor: descriptor(),
        registration: Registration::default(),
        capabilities: Capabilities(
            response::Capability::Bip380Descriptors.bit()
                | response::Capability::Bip388WalletPolicies.bit(),
        ),
    }
}

fn exchange(ask: Ask) -> Exchange {
    Exchange::new(ask, ID, FRAME_SIZE).unwrap()
}

/// Renders the request the way Liana shows it and reads it back the way a signer
/// would, so the frames are proven decodable rather than assumed so.
fn scan_request(exchange: &Exchange) -> Request {
    let mut decoder = Decoder::new();
    for frame in exchange.frames() {
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
        keys: vec![Active::to_ll_xpub(&xpub); count],
        fingerprint: Active::to_ll_fingerprint(&Fingerprint::from_str(SIGNER_FINGERPRINT).unwrap()),
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
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    let request::Body::GetKeys(body) = scan_request(&exchange).body else {
        panic!("expected a Get Keys request")
    };
    assert_eq!(body.keys.len(), ACCOUNTS as usize);
    let paths: Vec<String> = body
        .keys
        .iter()
        .map(|path| Active::from_ll_path(path).to_string())
        .collect();
    assert_eq!(paths[0], "48'/1'/0'/2'");
    assert_eq!(paths[9], "48'/1'/9'/2'");
}

#[test]
fn get_xpubs_reads_every_account_back() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    let Answer::Xpubs(signer) = answer(&exchange, xpubs_body(ACCOUNTS as usize)).unwrap() else {
        panic!("expected the accounts back")
    };
    assert_eq!(signer.fingerprint.to_string(), SIGNER_FINGERPRINT);
    assert_eq!(signer.vendor, "test vendor");
    assert_eq!(signer.model, "test signer");
    assert_eq!(signer.version.to_string(), "1.2.3-beta.2");
    assert!(signer
        .capabilities
        .supports(response::Capability::MiniscriptSegwitV0));
    assert!(signer
        .capabilities
        .supports(response::Capability::MiniscriptTaproot));
    assert_eq!(signer.accounts.len(), ACCOUNTS as usize);
    assert_eq!(signer.accounts[0].0, ChildNumber::Hardened { index: 0 });
    assert_eq!(
        signer.accounts[0].1.to_string(),
        format!("[{SIGNER_FINGERPRINT}/48'/1'/0'/2']{SIGNER_XPUB}")
    );
    assert_eq!(signer.accounts[9].0, ChildNumber::Hardened { index: 9 });
}

/// The signer answers one key per requested path, in order, so a short answer
/// cannot be matched to its accounts.
#[test]
fn a_signer_answering_fewer_accounts_than_asked_is_rejected() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    assert!(matches!(
        answer(&exchange, xpubs_body(ACCOUNTS as usize - 1)),
        Err(liana_gui::airgap::error::Error::UnknownXpub(_))
    ));
}

#[test]
fn a_signer_answering_more_accounts_than_asked_is_rejected() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    assert!(matches!(
        answer(&exchange, xpubs_body(ACCOUNTS as usize + 1)),
        Err(liana_gui::airgap::error::Error::UnknownXpub(_))
    ));
}

/// A signer left on another network answers with keys that parse but belong to
/// the wrong chain, and a descriptor built from them could never be spent.
#[test]
fn keys_for_another_network_are_refused() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    let mainnet = Xpub::from_str("xpub6ERApfZwUNrhLCkDtcHTcxd75RbzS1ed54G1LkBUHQVHQKqhMkhgbmJbZRkrgZw4koxb5JaHWkY4ALHY2grBGRjaDMzQLcgJvLJuZZvRcEL").unwrap();
    let body = response::Body::Keys(response::Keys {
        keys: vec![Active::to_ll_xpub(&mainnet); ACCOUNTS as usize],
        fingerprint: Active::to_ll_fingerprint(&Fingerprint::from_str(SIGNER_FINGERPRINT).unwrap()),
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
        Err(liana_gui::airgap::error::Error::WrongNetwork)
    ));
}

/// The signer this suite speaks to is on testnet, so its keys must be accepted
/// on every network Liana treats as testnet.
#[test]
fn testnet_keys_are_accepted_on_every_test_network() {
    for network in [
        Network::Testnet,
        Network::Testnet4,
        Network::Signet,
        Network::Regtest,
    ] {
        let exchange = exchange(Ask::Xpubs { network });
        assert!(
            matches!(
                answer(&exchange, xpubs_body(ACCOUNTS as usize)),
                Ok(Answer::Xpubs(_))
            ),
            "{} refused a testnet key",
            network
        );
    }
}

#[test]
fn a_signer_answering_no_account_is_rejected() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    assert!(matches!(
        answer(&exchange, xpubs_body(0)),
        Err(liana_gui::airgap::error::Error::UnknownXpub(_))
    ));
}

#[test]
fn register_carries_the_descriptor() {
    let exchange = exchange(Ask::Register { wallet: wallet() });
    let request::Body::RegisterDescriptor(body) = scan_request(&exchange).body else {
        panic!("expected a Register Descriptor request")
    };
    assert_eq!(body.descriptor_alias, "Liana");
    let Some(request::DescriptorBody::Bip388 { keys, policy }) = body.descriptor else {
        panic!("the descriptor must be sent as a wallet policy")
    };

    // three distinct keys, though the descriptor names two of them twice
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

    // and it is smaller than sending the descriptor whole
    let policy_bytes = keys.iter().map(|k| k.len()).sum::<usize>() + policy.len();
    assert!(
        policy_bytes < descriptor().to_string().len(),
        "the policy form should be the smaller one"
    );
}

#[test]
fn a_signer_without_wallet_policies_gets_the_descriptor() {
    let wallet = Wallet {
        capabilities: Capabilities(response::Capability::Bip380Descriptors.bit()),
        ..wallet()
    };
    let request::Body::RegisterDescriptor(body) =
        scan_request(&exchange(Ask::Register { wallet })).body
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
    let wallet = Wallet {
        capabilities: Capabilities(0),
        ..wallet()
    };
    assert!(matches!(
        Exchange::new(Ask::Register { wallet }, ID, FRAME_SIZE),
        Err(liana_gui::airgap::error::Error::UnsupportedDescriptor)
    ));
}

#[test]
fn register_keeps_what_the_signer_reported() {
    let exchange = exchange(Ask::Register { wallet: wallet() });
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
    let exchange = exchange(Ask::Register { wallet: wallet() });
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

/// Later requests name the wallet by the alias Liana sent, so a registration
/// under another one would leave them unknown to the signer.
#[test]
fn a_registration_under_another_alias_is_rejected() {
    let exchange = exchange(Ask::Register { wallet: wallet() });
    let body = response::Body::Registration(response::Registration {
        descriptor_alias: "Other".to_owned(),
        registered: true,
        stored: Some(true),
        proof: None,
    });
    assert_eq!(
        answer(&exchange, body).unwrap_err(),
        liana_gui::airgap::error::Error::AliasMismatch {
            expected: "Liana".to_owned(),
            received: "Other".to_owned(),
        }
    );
}

/// A signer that stored the descriptor is not sent it again; a stateless one is,
/// together with the proof it issued.
/// A stateless signer checks its proof of registration against the descriptor it
/// was registered with, so every request has to describe the wallet the same way.
#[test]
fn every_request_describes_the_wallet_the_same_way() {
    let registered = {
        let request::Body::RegisterDescriptor(body) =
            scan_request(&exchange(Ask::Register { wallet: wallet() })).body
        else {
            panic!("expected a registration")
        };
        body.descriptor
            .expect("a registration carries the descriptor")
    };
    let signed = {
        let request::Body::Sign(body) = scan_request(&exchange(Ask::Sign {
            wallet: wallet(),
            psbt: psbt("unsigned"),
        }))
        .body
        else {
            panic!("expected a signing request")
        };
        body.descriptors[0].descriptor.clone()
    };
    assert_eq!(Some(registered), signed);
}

#[test]
fn a_stateless_signer_gets_the_descriptor_replayed() {
    let stored = Wallet {
        registration: Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(true),
            proof: None,
        },
        ..wallet()
    };
    let stateless = Wallet {
        registration: Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(false),
            proof: Some(vec![7; 32]),
        },
        ..wallet()
    };

    let verify = |wallet: Wallet| {
        let exchange = exchange(Ask::VerifyAddress {
            wallet,
            address: address(),
            change: false,
            index: ChildNumber::Normal { index: 3 },
        });
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
    let wallet = Wallet {
        registration: Registration {
            descriptor_checksum: Some("u768v50p".to_owned()),
            stored: Some(true),
            proof: None,
        },
        ..wallet()
    };
    let request::Body::Sign(body) = scan_request(&exchange(Ask::Sign {
        wallet,
        psbt: psbt("unsigned"),
    }))
    .body
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
    let exchange = exchange(Ask::VerifyAddress {
        wallet: wallet(),
        address: address(),
        change: false,
        index: ChildNumber::Normal { index: 3 },
    });
    let request::Body::VerifyAddress(body) = scan_request(&exchange).body else {
        panic!("expected an Address Verification request")
    };
    assert_eq!(
        Active::from_ll_path(&body.derivation_path).to_string(),
        "0/3"
    );
    assert_eq!(body.address, Some(address().to_string()));
}

#[test]
fn verify_address_accepts_the_matching_uri() {
    let exchange = exchange(Ask::VerifyAddress {
        wallet: wallet(),
        address: address(),
        change: false,
        index: ChildNumber::Normal { index: 3 },
    });
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
    let exchange = exchange(Ask::VerifyAddress {
        wallet: wallet(),
        address: address(),
        change: false,
        index: ChildNumber::Normal { index: 3 },
    });
    let other = descriptor()
        .receive_descriptor()
        .derive(ChildNumber::Normal { index: 4 }, &secp())
        .address(NETWORK);
    let body = response::Body::AddressUri(response::AddressUri {
        uri: format!("bitcoin:{other}"),
    });
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::AddressMismatch { .. })
    ));
}

#[test]
fn verify_address_rejects_an_empty_answer() {
    let exchange = exchange(Ask::VerifyAddress {
        wallet: wallet(),
        address: address(),
        change: false,
        index: ChildNumber::Normal { index: 3 },
    });
    let body = response::Body::AddressUri(response::AddressUri { uri: String::new() });
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::InvalidAddressUri(_))
    ));
}

fn verify_address_answer(uri: String) -> Result<Answer, liana_gui::airgap::error::Error> {
    let exchange = exchange(Ask::VerifyAddress {
        wallet: wallet(),
        address: address(),
        change: false,
        index: ChildNumber::Normal { index: 3 },
    });
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

/// Parameters only start after a `?`, so this names another address.
#[test]
fn verify_address_rejects_parameters_without_a_query() {
    assert!(matches!(
        verify_address_answer(format!("bitcoin:{}&x", address())),
        Err(liana_gui::airgap::error::Error::AddressMismatch { .. })
    ));
}

#[test]
fn verify_address_rejects_a_malformed_query() {
    for query in ["foo", "", "=1", "amount=0.1&"] {
        assert!(
            matches!(
                verify_address_answer(format!("bitcoin:{}?{query}", address())),
                Err(liana_gui::airgap::error::Error::InvalidAddressUri(_))
            ),
            "?{} was accepted",
            query
        );
    }
}

#[test]
fn verify_address_accepts_any_case_and_several_parameters() {
    let uri = format!("BITCOIN:{}?amount=0.1&label=Liana", address());
    assert!(matches!(
        verify_address_answer(uri),
        Ok(Answer::AddressVerified)
    ));
}

/// The signing request is the only one large enough to need several frames.
#[test]
fn signing_carries_the_psbt_across_frames() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    assert!(exchange.frames().len() > 1);
    let request::Body::Sign(body) = scan_request(&exchange).body else {
        panic!("expected a Signing request")
    };
    assert_eq!(body.psbt, psbt("unsigned").serialize());
    assert_eq!(body.want_kind, Some(SignResponseKind::Signatures));
    assert_eq!(body.descriptors.len(), 1);
    assert_eq!(body.descriptors[0].alias, "Liana");
}

/// Every frame is pinned to the same QR version, so a message is carried by a
/// predictable number of equally sized codes rather than one dense outlier.
#[test]
fn every_frame_of_a_message_is_the_same_size() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let frames = exchange.frames();
    assert!(frames.len() > 1);
    let first = &frames[0];
    for frame in frames {
        assert_eq!(frame.side(), first.side());
    }
}

/// A denser setting carries the same request, under the same id, in fewer frames.
#[test]
fn reframing_keeps_the_request() {
    let mut exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let sparse = exchange.frames().len();
    let request = scan_request(&exchange);
    exchange.reframe(QrSize::S97).unwrap();
    assert!(exchange.frames().len() < sparse);
    assert_eq!(scan_request(&exchange), request);
}

#[test]
fn signing_merges_the_returned_signature() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
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
fn signing_rejects_a_different_transaction() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let mut tampered = psbt("partially-signed");
    tampered.unsigned_tx.output[0].value += liana::miniscript::bitcoin::Amount::from_sat(1);
    let body = response::Body::Signed(response::Signed::Psbt(tampered.serialize()));
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::InvalidPsbt(_))
    ));
}

#[test]
fn signing_rejects_an_answer_that_added_nothing() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let body = response::Body::Signed(response::Signed::Psbt(psbt("unsigned").serialize()));
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::InvalidPsbt(_))
    ));
}

#[test]
fn signing_rejects_unreadable_signature_bytes() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let body = response::Body::Signed(response::Signed::Psbt(vec![0; 8]));
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::InvalidPsbt(_))
    ));
}

#[test]
fn a_signature_naming_an_absent_input_is_rejected() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let body = response::Body::Signed(response::Signed::Signatures(vec![
        response::SignatureEntry::TapKey {
            input_index: 42,
            signature: vec![0; 64],
        },
    ]));
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::InvalidPsbt(_))
    ));
}

#[test]
fn an_empty_signature_list_is_rejected() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let body = response::Body::Signed(response::Signed::Signatures(Vec::new()));
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::InvalidPsbt(_))
    ));
}

#[test]
fn a_response_to_another_request_is_rejected() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    let bytes = encode_response(&ll::Response {
        id: RequestId([9; 2]),
        body: xpubs_body(ACCOUNTS as usize),
    })
    .unwrap();
    let error = exchange.read(&bytes).unwrap_err();
    assert!(matches!(
        error,
        liana_gui::airgap::error::Error::RequestIdMismatch
    ));
    // the common cause is a stale answer still on screen, so the message has to
    // say so rather than read as a misbehaving device
    assert!(error.to_string().contains("earlier request"));
}

/// A signer usually still shows its previous answer when the scan starts, so a
/// rejected one must leave the exchange able to take the real one straight
/// after, without the user restarting anything.
#[test]
fn a_stale_answer_does_not_poison_the_exchange() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });

    let stale = encode_response(&ll::Response {
        id: RequestId([9; 2]),
        body: xpubs_body(ACCOUNTS as usize),
    })
    .unwrap();
    assert!(matches!(
        exchange.read(&stale),
        Err(liana_gui::airgap::error::Error::RequestIdMismatch)
    ));

    // the very next message, this time addressed to us, is accepted
    assert!(matches!(
        answer(&exchange, xpubs_body(ACCOUNTS as usize)),
        Ok(Answer::Xpubs(_))
    ));
}

#[test]
fn a_response_of_the_wrong_type_is_rejected() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    let body = response::Body::AddressUri(response::AddressUri { uri: String::new() });
    assert!(matches!(
        answer(&exchange, body),
        Err(liana_gui::airgap::error::Error::MessageTypeMismatch { .. })
    ));
}

#[test]
fn a_request_where_a_response_belongs_is_rejected() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
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
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    assert!(matches!(
        exchange.read(b"not a message"),
        Err(liana_gui::airgap::error::Error::Decode(_))
    ));
}

/// Every standard refusal reaches the user as a refusal, not as a protocol bug.
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
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    for code in codes {
        let body = response::Body::Error(response::ErrorBody {
            message_type: ll::MessageType::Signing,
            error: code,
            message: "refused".to_owned(),
        });
        let error = answer(&exchange, body).unwrap_err();
        assert_eq!(
            error,
            liana_gui::airgap::error::Error::SignerRefused(code, "refused".to_owned())
        );
        assert_eq!(error.to_string(), "The device refused: refused");
    }
}

/// A refusal names the request it answers, so one naming another is not ours.
#[test]
fn a_refusal_of_another_request_type_is_rejected() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let body = response::Body::Error(response::ErrorBody {
        message_type: ll::MessageType::GetKeys,
        error: response::Error::UserDeclined,
        message: String::new(),
    });
    assert_eq!(
        answer(&exchange, body).unwrap_err(),
        liana_gui::airgap::error::Error::MessageTypeMismatch {
            expected: "signing",
            received: "key export",
        }
    );
}

#[test]
fn frames_reassemble_out_of_order_and_with_duplicates() {
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let mut frames: Vec<_> = exchange.frames().to_vec();
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
    let exchange = exchange(Ask::Sign {
        wallet: wallet(),
        psbt: psbt("unsigned"),
    });
    let frames = exchange.frames();
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
    // the scan can tell which frame it still lacks
    assert_eq!(
        progress.received.iter().position(|read| !read),
        Some(frames.len() - 1)
    );
}

#[test]
fn an_xpub_the_signer_cannot_have_derived_is_rejected() {
    let exchange = exchange(Ask::Xpubs { network: NETWORK });
    let body = response::Body::Keys(response::Keys {
        keys: vec![ExtendedKey([0; 78]); ACCOUNTS as usize],
        fingerprint: Active::to_ll_fingerprint(&Fingerprint::from_str(SIGNER_FINGERPRINT).unwrap()),
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
