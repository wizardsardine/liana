//! Loopback round-trip test for the local LAN signer.
//!
//! Pattern: spin up a fake phone TLS server on `127.0.0.1:<random>`
//! that speaks the `LocalEnvelope` protocol. On `PresentSession`, it
//! decodes the embedded PSBT, deserialises it, re-serialises it
//! unchanged (we don't have a real signing path in the test), and
//! replies with a matching `PartialSignature`. Then construct a
//! [`PhoneSigner`] aimed at that endpoint and call `sign_tx`, which
//! must return Ok and leave the PSBT structurally intact.
//!
//! Side benefit: this exercises the desktop's TLS pinning path on a
//! self-signed phone cert that the test mints inline, end-to-end.

#[path = "common/lan_binding.rs"]
mod lan_binding;
use std::net::Ipv4Addr;
use std::{str::FromStr, sync::Arc};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use coincube_core::miniscript::bitcoin::{
    bip32::Fingerprint, psbt::Psbt, secp256k1::Secp256k1, transaction::Version, Transaction,
};
use coincube_core::{
    imported_spend::verify_imported_signatures, wallet_descriptor::ImportedWalletDescriptor,
};
use prost::Message as _;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::ServerConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;

use coincube_gui::dir::NetworkDirectory;
use coincube_gui::phone_signer::{
    identity::{pin_hex8, DesktopIdentity},
    pairing_store::{self, ImportedPairingScope, PairedPhone, SignerBinding},
    pairing_transaction::PairingTransaction,
    policy::PhonePolicy,
    protocol::{local_v1, LocalEnvelope},
    tls,
    transport::PairedTransport,
    PhoneSigner,
};
use coincube_gui::services::connect::crypto::{seal_to_device, DeviceTransportKey};
use coincube_gui::services::connect::grpc::connect_v1 as cv1;
use serde_json::Value;
use sha2::{Digest, Sha256};

const IMPORTED_REQUESTS: &str =
    include_str!("../src/phone_signer/fixtures/imported_canonical_requests.json");
const IMPORTED_SIGNED: &str =
    include_str!("../src/phone_signer/fixtures/imported_canonical_signed.json");

/// Self-signed Ed25519 cert + key, returned as the rustls-pki-types
/// `Der` newtypes the runtime expects.
/// A real ECIES transport keypair in a throwaway directory.
///
/// The LAN rail is ECIES_V1 now: the desktop seals the PSBT and descriptor to
/// the phone's key and the phone seals the signature back to the desktop's, so
/// both sides of these loopback tests need real keys rather than placeholders.
fn fresh_transport_key(tag: &str) -> DeviceTransportKey {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "coincube-loopback-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("temp transport key dir");
    DeviceTransportKey::load_or_create(&NetworkDirectory::new(path)).expect("mint transport key")
}

/// A descriptor for the signer to seal. These tests assert on transport
/// behaviour, not descriptor content, but it must be real bytes.
const TEST_DESCRIPTOR: &str = "wsh(or_d(pk([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<0;1>/*),and_v(v:pkh([8a550171/48'/1'/0'/2']tpubDFnCs5ZaCqopaNhgLCiXAwbkaBdcnuMt1VFoPsRpUrpidyvzG67MYjkfxw6HnTBhHqeU3xw2ioNBVcWY3jXwGhSyppEQvtn38GsL7RH1eef/<2;3>/*),older(52596))))#jz5sm0xn";

fn mint_ed25519_cert(common_name: &str) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key_pair = KeyPair::generate_for(&PKCS_ED25519).expect("ed25519 keygen");
    let mut params = CertificateParams::new(vec!["test.local".to_string()]).expect("params");
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;
    let cert = params.self_signed(&key_pair).expect("self-sign");
    let cert_der = cert.der().clone();
    let key_pkcs8 = key_pair.serialize_der();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pkcs8));
    (cert_der, key_der)
}

/// Empty-but-valid PSBT (one input, zero outputs) we can feed through
/// `sign_tx`. The fake phone server doesn't actually sign — it just
/// echoes the PSBT bytes back inside a `PartialSignature` envelope.
fn empty_psbt() -> Psbt {
    let tx = Transaction {
        version: Version::TWO,
        lock_time: coincube_core::miniscript::bitcoin::absolute::LockTime::ZERO,
        input: Vec::new(),
        output: Vec::new(),
    };
    Psbt::from_unsigned_tx(tx).expect("empty psbt")
}

fn imported_case(name: &str) -> (Value, Psbt, Psbt, ImportedWalletDescriptor, PhonePolicy) {
    let requests: Value = serde_json::from_str(IMPORTED_REQUESTS).expect("frozen desktop requests");
    let signed: Value = serde_json::from_str(IMPORTED_SIGNED).expect("native signed results");
    let request = requests["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["name"] == name)
        .unwrap()
        .clone();
    let result = signed["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|vector| vector["name"] == name)
        .unwrap();
    let original =
        Psbt::deserialize(&STANDARD.decode(request["psbt"].as_str().unwrap()).unwrap()).unwrap();
    let returned = Psbt::deserialize(
        &STANDARD
            .decode(result["signed_psbt"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let wallet = ImportedWalletDescriptor::parse(
        request["descriptor"].as_str().unwrap(),
        None,
        coincube_core::chain::ChainId::Testnet,
    )
    .unwrap();
    let loaded = coincube_gui::app::wallet::Wallet::new(wallet.clone());
    let policy = PhonePolicy::from_wallet(&loaded).unwrap();
    assert_eq!(policy.descriptor(), request["descriptor"].as_str().unwrap());
    assert_eq!(
        policy.transport_id().to_string(),
        request["descriptor_id"].as_str().unwrap()
    );
    (request, original, returned, wallet, policy)
}

fn imported_pairing(
    request: &Value,
    policy: &PhonePolicy,
    cert_pin: tls::CertFingerprint,
    transport_pubkey: Vec<u8>,
) -> PairedPhone {
    let key: Value = serde_json::from_str::<Value>(IMPORTED_REQUESTS).unwrap()["keys"][0].clone();
    let fingerprint = Fingerprint::from_str(key["fingerprint"].as_str().unwrap()).unwrap();
    let coincube_gui::phone_signer::policy::PhonePolicyKind::Imported { wallet, wallet_id } =
        policy.kind()
    else {
        panic!("expected imported policy")
    };
    assert_eq!(request["signer_key_id"], key["backend_key_id"]);
    PairedPhone {
        signer_binding: Some(SignerBinding {
            key_id: key["backend_key_id"].as_u64().unwrap().to_string(),
            xpub: key["xpub"].as_str().unwrap().to_owned(),
            fingerprint,
            descriptor_sha256: Sha256::digest(policy.descriptor().as_bytes()).to_vec(),
            imported_scope: Some(
                ImportedPairingScope::for_account(
                    wallet,
                    wallet_id,
                    key["xpub"].as_str().unwrap(),
                    fingerprint,
                )
                .unwrap(),
            ),
        }),
        cert_pin,
        name: "Synthetic imported phone".into(),
        paired_at_unix: 7,
        wallet_fingerprints: vec![fingerprint],
        vault_fingerprint: policy.transport_id(),
        transport_pubkey,
        fallback_addr: None,
    }
}

fn persist_imported_pairing(
    dir: &coincube_gui::dir::CoincubeDirectory,
    candidate: PairedPhone,
    revision: uuid::Uuid,
) -> PairedPhone {
    let transaction = PairingTransaction::prepare(dir, revision.to_string(), candidate).unwrap();
    transaction.write_candidate().unwrap();
    transaction.decide().unwrap();
    transaction.finish().unwrap()
}

async fn imported_signer_against(
    request: &Value,
    policy: PhonePolicy,
    response: FakeResponse,
) -> (
    PhoneSigner,
    tokio::task::JoinHandle<()>,
    coincube_gui::dir::CoincubeDirectory,
    PairedPhone,
) {
    let (desk_cert, desk_key) = mint_ed25519_cert("Imported desktop (test)");
    let (phone_cert, phone_key) = mint_ed25519_cert("Imported phone (test)");
    let desktop_pin = tls::fingerprint_of(&desk_cert);
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let desktop = DesktopIdentity {
        cert_der: desk_cert,
        key_der: desk_key,
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let phone_transport = fresh_transport_key("imported-phone");
    let candidate = imported_pairing(
        request,
        &policy,
        phone_pin,
        phone_transport.public_key().to_vec(),
    );
    let phone_handle = tokio::spawn(async move {
        fake_phone(
            listener,
            phone_cert,
            phone_key,
            desktop_pin,
            response,
            phone_transport,
        )
        .await
        .expect("imported fake phone");
    });
    let transport = PairedTransport::connect(addr, &desktop, phone_pin)
        .await
        .expect("dial imported fake phone");
    let path = std::env::temp_dir().join(format!(
        "coincube-imported-sign-tx-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&path).unwrap();
    let dir = coincube_gui::dir::CoincubeDirectory::new(path);
    let paired = persist_imported_pairing(&dir, candidate, uuid::Uuid::new_v4());
    let fingerprint = paired.signer_binding.as_ref().unwrap().fingerprint;
    let signer = PhoneSigner::new(
        transport,
        fingerprint,
        None,
        paired.clone(),
        policy,
        dir.clone(),
        Some(Arc::new(fresh_transport_key("imported-desktop"))),
    );
    (signer, phone_handle, dir, paired)
}

/// How the fake phone responds after reading the `PresentSession`.
/// Lets the same harness drive happy-path, error-envelope, and
/// "close without responding" scenarios.
enum FakeResponse {
    /// Echo the PSBT bytes back inside a `PartialSignature`.
    EchoPartial,
    WrongRequest,
    WrongKey,
    Plaintext,
    /// Send back an `ErrorEnvelope` with the given code/message.
    Error {
        code: String,
        message: String,
    },
    /// Drop the connection right after reading the request without
    /// sending anything back. Exercises the reader-side
    /// `Disconnected` path.
    Disconnect,
    /// Read the request and then sleep on the virtual clock so the
    /// desktop's `sign_tx` timeout fires. Used with
    /// `#[tokio::test(start_paused = true)]` so the wall-clock wait
    /// collapses to instant via auto-advance.
    HangForever,
    /// Native BDK signed fixture, delayed at the real phone-review boundary.
    Imported {
        expected_descriptor: String,
        expected_psbt: Vec<u8>,
        signed_psbt: Vec<u8>,
        review_started: oneshot::Sender<()>,
        release_review: oneshot::Receiver<()>,
    },
    /// A pre-send refusal must close the transport without a frame.
    NoPresentSession,
}

/// Run a fake phone server on the supplied `listener` that presents
/// `(cert, key)` to clients pinned to `desktop_cert_pin`, then
/// responds per [`FakeResponse`]. Caller owns the listener so the
/// port is already bound when the desktop side dials — no
/// `tokio::time::sleep`-based synchronisation race.
async fn fake_phone(
    listener: TcpListener,
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    desktop_cert_pin: tls::CertFingerprint,
    response: FakeResponse,
    phone_transport: DeviceTransportKey,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier_pinning(desktop_cert_pin))
        .with_single_cert(vec![cert], key)?;

    let (tcp, _peer) = listener.accept().await?;
    let acceptor = TlsAcceptor::from(Arc::new(cfg));
    let mut tls = acceptor.accept(tcp).await?;

    // Read one length-prefixed PresentSession. For a pre-send refusal the
    // signer is dropped after sign_tx returns; even one received byte means
    // it began emitting a frame.
    let mut len_buf = [0u8; 4];
    if matches!(&response, FakeResponse::NoPresentSession) {
        let mut first_byte = [0u8; 1];
        match tls.read(&mut first_byte).await {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                ) => {}
            Ok(n) => panic!(
                "revoked cached signer emitted {n} plaintext byte(s), starting with {:02x}",
                first_byte[0]
            ),
            Err(error) => panic!(
                "expected transport close before PresentSession, got {error:?}",
                error = error
            ),
        }
        return Ok(());
    }
    tls.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    tls.read_exact(&mut payload).await?;
    let envelope = LocalEnvelope::decode(payload.as_slice())?;
    let (session_id, request_id, psbt_bytes, descriptor_bytes, creator_pub) = match envelope.payload
    {
        Some(local_v1::local_envelope::Payload::PresentSession(p)) => {
            let s = p.session.expect("session present");
            assert_eq!(
                s.payload_scheme,
                cv1::PayloadScheme::EciesV1 as i32,
                "LAN sessions must be ECIES_V1",
            );
            assert!(s.psbt.is_empty(), "ECIES_V1 carries no plaintext PSBT");
            assert_eq!(
                s.descriptor_envelopes.len(),
                1,
                "the descriptor must be sealed to this phone",
            );
            let pe = s.psbt_envelopes.first().expect("psbt envelope").clone();
            let psbt = phone_transport
                .open(
                    &pe.ephemeral_pubkey,
                    &pe.nonce,
                    &pe.ciphertext,
                    &s.request_id,
                )
                .expect("open sealed PSBT");
            let de = s.descriptor_envelopes.first().expect("descriptor envelope");
            let descriptor = phone_transport
                .open(
                    &de.ephemeral_pubkey,
                    &de.nonce,
                    &de.ciphertext,
                    &s.request_id,
                )
                .expect("open sealed descriptor");
            (
                s.session_id,
                s.request_id,
                psbt.to_vec(),
                descriptor.to_vec(),
                s.creator_transport_pubkey,
            )
        }
        other => panic!("expected PresentSession, got {:?}", other),
    };

    let reply = match response {
        FakeResponse::EchoPartial
        | FakeResponse::WrongRequest
        | FakeResponse::WrongKey
        | FakeResponse::Plaintext => {
            let wrong_request = matches!(response, FakeResponse::WrongRequest);
            let wrong_key = matches!(response, FakeResponse::WrongKey);
            let plaintext = matches!(response, FakeResponse::Plaintext);
            let sealed = seal_to_device(
                &creator_pub,
                if wrong_request {
                    "another-request"
                } else {
                    &request_id
                },
                &psbt_bytes,
            )
            .expect("seal the signature back to the desktop");
            Some(LocalEnvelope {
                payload: Some(local_v1::local_envelope::Payload::Partial(
                    local_v1::PartialSignature {
                        session_id,
                        // Empty under ECIES_V1.
                        signed_psbt: if plaintext { psbt_bytes } else { Vec::new() },
                        signed_key_ids: vec![if wrong_key { "999" } else { "10" }.into()],
                        signature_envelope: Some(cv1::PayloadEnvelope {
                            device_id: String::new(),
                            ephemeral_pubkey: sealed.ephemeral_pubkey,
                            nonce: sealed.nonce,
                            ciphertext: sealed.ciphertext,
                        }),
                    },
                )),
            })
        }
        FakeResponse::Error { code, message } => Some(LocalEnvelope {
            payload: Some(local_v1::local_envelope::Payload::Error(
                local_v1::ErrorEnvelope {
                    code,
                    message,
                    // Tag the error with the session_id the desktop
                    // just sent. This exercises the production
                    // TargetedError dispatch path (per-session
                    // routing) end-to-end through the reader task.
                    // A real phone would do the same on a per-session
                    // user reject.
                    session_id,
                },
            )),
        }),
        FakeResponse::Disconnect => None,
        FakeResponse::HangForever => {
            // Hold the connection open so the desktop side can fire
            // its `sign_tx` timeout. Sleep on the virtual clock so
            // paused-time auto-advance collapses to instant.
            tokio::time::sleep(std::time::Duration::from_secs(60 * 60 * 24)).await;
            None
        }
        FakeResponse::Imported {
            expected_descriptor,
            expected_psbt,
            signed_psbt,
            review_started,
            release_review,
        } => {
            assert_eq!(descriptor_bytes, expected_descriptor.as_bytes());
            assert_eq!(psbt_bytes, expected_psbt);
            review_started
                .send(())
                .expect("test waits for phone review");
            release_review.await.expect("test releases phone review");
            let sealed = seal_to_device(&creator_pub, &request_id, &signed_psbt)
                .expect("seal native signed PSBT to desktop");
            Some(LocalEnvelope {
                payload: Some(local_v1::local_envelope::Payload::Partial(
                    local_v1::PartialSignature {
                        session_id,
                        signed_psbt: Vec::new(),
                        signed_key_ids: vec!["10".into()],
                        signature_envelope: Some(cv1::PayloadEnvelope {
                            device_id: String::new(),
                            ephemeral_pubkey: sealed.ephemeral_pubkey,
                            nonce: sealed.nonce,
                            ciphertext: sealed.ciphertext,
                        }),
                    },
                )),
            })
        }
        FakeResponse::NoPresentSession => unreachable!(),
    };
    if let Some(envelope) = reply {
        let mut buf = Vec::with_capacity(envelope.encoded_len());
        envelope.encode(&mut buf)?;
        tls.write_all(&(buf.len() as u32).to_be_bytes()).await?;
        tls.write_all(&buf).await?;
        tls.flush().await?;
    }
    Ok(())
}

/// Don't pin the desktop's cert from the phone side in this test —
/// the production code exercises the pinning verifier on the
/// **outbound** direction (PhoneSigner → phone), which is what the
/// `assert_eq!` round-trip validates. The phone's
/// `ClientCertVerifier` is therefore configured as "no client auth"
/// so the desktop's TLS client cert isn't required to chain to
/// anything.
fn verifier_pinning(
    _pin: tls::CertFingerprint,
) -> Arc<dyn rustls::server::danger::ClientCertVerifier> {
    WebPkiClientVerifier::no_client_auth()
}

// Multi-threaded so a regression that holds the transport mutex
// across `recv().await` would actually deadlock instead of being
// papered over by single-threaded cooperative scheduling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sign_tx_rejects_unsigned_echo_through_fake_phone() {
    // 1. Mint desktop and phone identities.
    let (desk_cert, desk_key) = mint_ed25519_cert("Coincube Desktop (test)");
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");

    let desktop_pin = tls::fingerprint_of(&desk_cert);
    let phone_pin = tls::fingerprint_of(&phone_cert);

    let desktop = DesktopIdentity {
        cert_der: desk_cert.clone(),
        key_der: desk_key,
    };

    // 2. Bind the fake phone listener, capture its addr, hand the
    //    bound listener to the phone task — no rebind race.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind fake phone");
    let phone_addr = listener.local_addr().expect("local_addr");
    let phone_transport = fresh_transport_key("phone");
    let phone_pubkey = phone_transport.public_key().to_vec();
    let phone_handle = tokio::spawn(async move {
        fake_phone(
            listener,
            phone_cert,
            phone_key,
            desktop_pin,
            FakeResponse::EchoPartial,
            phone_transport,
        )
        .await
        .expect("fake phone");
    });

    // 3. Dial via PairedTransport, build a PhoneSigner, sign.
    let transport = PairedTransport::connect(phone_addr, &desktop, phone_pin)
        .await
        .expect("dial fake phone");
    let paired = PairedPhone {
        signer_binding: Some(lan_binding::binding(TEST_DESCRIPTOR)),
        cert_pin: phone_pin,
        name: "Test phone".into(),
        paired_at_unix: 0,
        wallet_fingerprints: vec![Fingerprint::default()],
        vault_fingerprint: lan_binding::vault(TEST_DESCRIPTOR),
        transport_pubkey: phone_pubkey,
        fallback_addr: None,
    };
    let signer = PhoneSigner::new(
        transport,
        Fingerprint::default(),
        None,
        paired,
        coincube_gui::phone_signer::policy::PhonePolicy::vault(
            TEST_DESCRIPTOR,
            coincube_core::chain::ChainId::Regtest,
        )
        .expect("vault policy"),
        coincube_gui::dir::CoincubeDirectory::new(std::env::temp_dir()),
        Some(Arc::new(fresh_transport_key("desktop"))),
    );
    let mut psbt = empty_psbt();
    let original = psbt.serialize();
    async_hwi::HWI::sign_tx(&signer, &mut psbt)
        .await
        .expect_err("echo without a selected-key signature must be rejected");
    // Round-trip: the fake phone echoed back unchanged, so the
    // PSBT serialises to the same bytes.
    let returned = psbt.serialize();
    assert_eq!(
        original, returned,
        "echo-back PSBT bytes should be byte-identical"
    );

    // Drain the server task. It returns once the single envelope
    // round-trip is done.
    let _ = phone_handle.await;

    // Sanity: the test also exercises the fingerprint helper.
    let _hex = pin_hex8(&phone_pin);
}

/// Shared scaffolding for the unhappy-path tests: spin up a fake
/// phone with the given response policy, build a `PhoneSigner`
/// pointed at it, and return the signer so the caller can drive
/// `sign_tx` and assert on the error variant.
async fn signer_against_response(
    response: FakeResponse,
) -> (PhoneSigner, tokio::task::JoinHandle<()>) {
    signer_against_chain_response(coincube_core::chain::ChainId::Regtest, response).await
}

async fn signer_against_chain_response(
    chain: coincube_core::chain::ChainId,
    response: FakeResponse,
) -> (PhoneSigner, tokio::task::JoinHandle<()>) {
    let (desk_cert, desk_key) = mint_ed25519_cert("Coincube Desktop (test)");
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let desktop_pin = tls::fingerprint_of(&desk_cert);
    let phone_pin = tls::fingerprint_of(&phone_cert);

    let desktop = DesktopIdentity {
        cert_der: desk_cert.clone(),
        key_der: desk_key,
    };

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind fake phone");
    let phone_addr = listener.local_addr().expect("local_addr");
    let phone_transport = fresh_transport_key("phone");
    let phone_pubkey = phone_transport.public_key().to_vec();
    let handle = tokio::spawn(async move {
        fake_phone(
            listener,
            phone_cert,
            phone_key,
            desktop_pin,
            response,
            phone_transport,
        )
        .await
        .expect("fake phone");
    });

    let transport = PairedTransport::connect(phone_addr, &desktop, phone_pin)
        .await
        .expect("dial fake phone");
    let paired = PairedPhone {
        signer_binding: Some(lan_binding::binding(TEST_DESCRIPTOR)),
        cert_pin: phone_pin,
        name: "Test phone".into(),
        paired_at_unix: 0,
        wallet_fingerprints: vec![Fingerprint::default()],
        vault_fingerprint: lan_binding::vault(TEST_DESCRIPTOR),
        transport_pubkey: phone_pubkey,
        fallback_addr: None,
    };
    let signer = PhoneSigner::new(
        transport,
        Fingerprint::default(),
        None,
        paired,
        coincube_gui::phone_signer::policy::PhonePolicy::vault(TEST_DESCRIPTOR, chain)
            .expect("vault policy"),
        coincube_gui::dir::CoincubeDirectory::new(std::env::temp_dir()),
        Some(Arc::new(fresh_transport_key("desktop"))),
    );
    (signer, handle)
}

#[tokio::test]
async fn sign_tx_surfaces_phone_error_envelope_as_device_error() {
    let (signer, handle) = signer_against_response(FakeResponse::Error {
        code: "USER_DECLINED".into(),
        message: "user pressed reject".into(),
    })
    .await;
    let mut psbt = empty_psbt();
    let err = async_hwi::HWI::sign_tx(&signer, &mut psbt)
        .await
        .expect_err("expected Err");
    let msg = match err {
        async_hwi::Error::Device(m) => m,
        other => panic!("expected Device, got {:?}", other),
    };
    assert!(
        msg.contains("USER_DECLINED") && msg.contains("user pressed reject"),
        "error message should include code and text: {}",
        msg
    );
    let _ = handle.await;
}

#[tokio::test]
async fn sign_tx_surfaces_phone_disconnect_as_device_disconnected() {
    let (signer, handle) = signer_against_response(FakeResponse::Disconnect).await;
    let mut psbt = empty_psbt();
    let err = async_hwi::HWI::sign_tx(&signer, &mut psbt)
        .await
        .expect_err("expected Err");
    assert!(
        matches!(err, async_hwi::Error::DeviceDisconnected),
        "expected DeviceDisconnected, got {:?}",
        err
    );
    let _ = handle.await;
}

#[tokio::test]
async fn sign_tx_returns_timeout_error_when_phone_hangs() {
    // The fake phone sleeps for a virtual day; the desktop's
    // `sign_tx` times out at `SIGN_RESPONSE_TIMEOUT` (5 min today).
    // We pause time **after** the dial completes so the TLS
    // handshake's real I/O doesn't race the (now bounded) handshake
    // timeout under auto-advance — only the sign_tx round-trip is
    // virtualised. With paused time, auto-advance fires the 5 min
    // deadline first and we get back HwiError::Device("sign_tx
    // timeout").
    let (signer, _handle) = signer_against_response(FakeResponse::HangForever).await;
    tokio::time::pause();
    let mut psbt = empty_psbt();
    let err = async_hwi::HWI::sign_tx(&signer, &mut psbt)
        .await
        .expect_err("expected Err");
    let msg = match err {
        async_hwi::Error::Device(m) => m,
        other => panic!("expected Device(timeout), got {:?}", other),
    };
    assert!(
        msg.to_lowercase().contains("timeout"),
        "error should mention timeout, got: {}",
        msg
    );
    // Detach the hanging phone task: we don't await its
    // completion because it's deliberately sleeping past the
    // virtual end of the test.
}

/// Regression for the "stale phone skips redial" finding: once the
/// underlying TLS session goes away, `PhoneSigner::is_alive()` must
/// flip to `false` so the hw refresh tick redials instead of leaving
/// a dead handle listed as Supported (which would silently fail every
/// sign attempt until mDNS times out).
#[tokio::test]
async fn is_alive_flips_false_after_phone_disconnect() {
    let (signer, handle) = signer_against_response(FakeResponse::Disconnect).await;
    assert!(
        signer.is_alive(),
        "freshly-dialled PhoneSigner should report alive",
    );
    // Drive a sign so the fake phone reads PresentSession and then
    // closes — that closes the read half, the reader task observes
    // Err, and Correlator's task exits.
    let mut psbt = empty_psbt();
    let _ = async_hwi::HWI::sign_tx(&signer, &mut psbt).await;
    let _ = handle.await;
    // The reader task may take a tick to fully exit after the
    // sign_tx future resolves on `Disconnected`. Yield until
    // `is_finished()` observes the join, with a generous cap so a
    // regression that never flips fails fast instead of hanging.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while signer.is_alive() && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(
        !signer.is_alive(),
        "PhoneSigner::is_alive() should flip to false once the reader task exits",
    );
}

#[tokio::test]
async fn response_identity_aad_and_plaintext_downgrades_are_refused() {
    for response in [
        FakeResponse::WrongRequest,
        FakeResponse::WrongKey,
        FakeResponse::Plaintext,
    ] {
        let (signer, handle) = signer_against_response(response).await;
        let mut psbt = empty_psbt();
        let before = psbt.serialize();
        assert!(async_hwi::HWI::sign_tx(&signer, &mut psbt).await.is_err());
        assert_eq!(psbt.serialize(), before);
        handle.await.unwrap();
    }
}

#[test]
fn stale_or_wrong_exact_pairing_identity_is_not_a_capability() {
    let mut phone = PairedPhone {
        signer_binding: Some(lan_binding::binding(TEST_DESCRIPTOR)),
        cert_pin: [1; 32],
        name: "fixture".into(),
        paired_at_unix: 0,
        wallet_fingerprints: Vec::new(),
        vault_fingerprint: lan_binding::vault(TEST_DESCRIPTOR),
        transport_pubkey: Vec::new(),
        fallback_addr: None,
    };
    assert!(phone.exact_signer(TEST_DESCRIPTOR).is_ok());
    let valid = phone.signer_binding.clone();
    phone.signer_binding = None;
    assert!(phone
        .exact_signer(TEST_DESCRIPTOR)
        .unwrap_err()
        .contains("again"));
    phone.signer_binding = valid.clone();
    phone.signer_binding.as_mut().unwrap().xpub = "another-key".into();
    assert!(phone.exact_signer(TEST_DESCRIPTOR).is_err());
    phone.signer_binding = valid.clone();
    phone.signer_binding.as_mut().unwrap().descriptor_sha256[0] ^= 1;
    assert!(phone.exact_signer(TEST_DESCRIPTOR).is_err());
    phone.signer_binding = valid;
    phone.vault_fingerprint = Fingerprint::default();
    assert!(phone.exact_signer(TEST_DESCRIPTOR).is_err());
}

#[tokio::test]
async fn btcb2_lan_refuses_before_a_session_is_sent() {
    use coincube_core::chain::ChainId;
    for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
        let (signer, mut phone) = signer_against_chain_response(
            chain,
            FakeResponse::Error {
                code: "UNEXPECTED_SESSION".into(),
                message: "A BTCB2 session reached LAN".into(),
            },
        )
        .await;
        let mut psbt = empty_psbt();
        let before = psbt.serialize();
        let error = async_hwi::HWI::sign_tx(&signer, &mut psbt)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("chain-bound pairing"),
            "{}",
            error
        );
        assert_eq!(psbt.serialize(), before);
        // The fake phone completes immediately after processing PresentSession.
        // It must remain waiting for a frame; the refusal happens in the caller.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), &mut phone)
                .await
                .is_err()
        );
        phone.abort();
        drop(signer);
    }
}

#[derive(Clone, Copy)]
enum ImportedReviewChange {
    None,
    Remove,
    Replace,
}

/// Drive the actual HWI entry point with an authenticated, native-BDK signed
/// result. The phone holds the TLS session open at review until the test
/// changes the durable pairing row, so the production post-review read decides
/// whether an otherwise valid signature may reach the caller's PSBT.
async fn imported_sign_tx_case(name: &str, change: ImportedReviewChange) {
    let (request, original, returned, wallet, policy) = imported_case(name);
    let before = original.serialize();
    let (review_started_tx, review_started_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let response = FakeResponse::Imported {
        expected_descriptor: policy.descriptor().to_owned(),
        expected_psbt: before.clone(),
        signed_psbt: returned.serialize(),
        review_started: review_started_tx,
        release_review: release_rx,
    };
    let (signer, phone_handle, dir, paired) =
        imported_signer_against(&request, policy, response).await;
    let signing = tokio::spawn(async move {
        let mut psbt = original;
        let result = async_hwi::HWI::sign_tx(&signer, &mut psbt).await;
        (result, psbt)
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), review_started_rx)
        .await
        .expect("phone reached review")
        .expect("review signal");
    let replacement = match change {
        ImportedReviewChange::None => None,
        ImportedReviewChange::Remove => {
            pairing_store::unpair(&dir, &paired.cert_pin).unwrap();
            assert!(pairing_store::load(&dir).unwrap().phones.is_empty());
            None
        }
        ImportedReviewChange::Replace => {
            let current = persist_imported_pairing(&dir, paired.clone(), uuid::Uuid::new_v4());
            assert_eq!(current.cert_pin, paired.cert_pin);
            assert_eq!(current.paired_at_unix, paired.paired_at_unix);
            assert_ne!(
                current.imported_pairing_revision(),
                paired.imported_pairing_revision()
            );
            Some(current)
        }
    };
    release_tx.send(()).unwrap();
    let (result, signed) = tokio::time::timeout(std::time::Duration::from_secs(10), signing)
        .await
        .expect("sign_tx completed")
        .unwrap();
    phone_handle.await.unwrap();
    match change {
        ImportedReviewChange::None => {
            result.expect("current imported pairing accepts native signature");
            let original = Psbt::deserialize(&before).unwrap();
            assert_eq!(signed.unsigned_tx, original.unsigned_tx);
            assert_eq!(signed.outputs, original.outputs);
            assert_eq!(signed.inputs[0].partial_sigs.len(), 1);
            let mut without_added_signatures = signed.clone();
            for (current, prior) in without_added_signatures
                .inputs
                .iter_mut()
                .zip(&original.inputs)
            {
                current.partial_sigs = prior.partial_sigs.clone();
            }
            assert_eq!(
                without_added_signatures.serialize(),
                before,
                "sign_tx changed a non-signature PSBT field"
            );
            assert_eq!(
                verify_imported_signatures(
                    &wallet,
                    &original,
                    &signed,
                    &Secp256k1::verification_only()
                )
                .unwrap(),
                vec![1]
            );
        }
        ImportedReviewChange::Remove | ImportedReviewChange::Replace => {
            result.expect_err("revoked imported pairing refuses late valid response");
            assert_eq!(signed.serialize(), before, "caller PSBT mutated on refusal");
            if let Some(current) = replacement {
                let store = pairing_store::load(&dir).unwrap();
                assert_eq!(store.phones.len(), 1);
                assert_eq!(
                    store.phones[0].imported_pairing_revision(),
                    current.imported_pairing_revision(),
                    "late old response displaced the new pairing"
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imported_sign_tx_accepts_native_phone_result() {
    imported_sign_tx_case("sortedmulti_first", ImportedReviewChange::None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imported_sign_tx_rechecks_pairing_after_valid_phone_review() {
    for (name, change) in [
        ("sortedmulti_nonfirst", ImportedReviewChange::Remove),
        ("sortedmulti_first", ImportedReviewChange::Replace),
    ] {
        imported_sign_tx_case(name, change).await;
    }
}

/// A cached handle may stay alive after local unpair/re-pair, but it must not
/// emit even a PresentSession once its exact persisted authority is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn imported_cached_signer_refuses_before_present_session_after_revocation() {
    for replace in [false, true] {
        let (request, mut psbt, _, _, policy) = imported_case("sortedmulti_first");
        let (signer, phone_handle, dir, paired) =
            imported_signer_against(&request, policy, FakeResponse::NoPresentSession).await;
        let before = psbt.serialize();
        if replace {
            let current = persist_imported_pairing(&dir, paired.clone(), uuid::Uuid::new_v4());
            assert_ne!(
                current.imported_pairing_revision(),
                paired.imported_pairing_revision()
            );
        } else {
            pairing_store::unpair(&dir, &paired.cert_pin).unwrap();
        }
        let error = async_hwi::HWI::sign_tx(&signer, &mut psbt)
            .await
            .expect_err("cached imported handle lost its authority");
        let expected = if replace {
            "This phone pairing is ambiguous; pair it again."
        } else {
            "This phone is no longer paired; pair it again."
        };
        match error {
            async_hwi::Error::Device(message) => assert_eq!(message, expected),
            other => panic!(
                "expected exact-pairing refusal, got {other:?}",
                other = other
            ),
        }
        assert_eq!(psbt.serialize(), before);
        drop(signer);
        tokio::time::timeout(std::time::Duration::from_secs(10), phone_handle)
            .await
            .expect("phone saw transport close without PresentSession")
            .unwrap();
    }
}
