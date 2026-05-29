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

use std::net::Ipv4Addr;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{
    bip32::Fingerprint, psbt::Psbt, transaction::Version, Transaction,
};
use prost::Message as _;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::ServerConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use coincube_gui::phone_signer::{
    identity::{fingerprint_hex8, DesktopIdentity},
    pairing_store::PairedPhone,
    protocol::{local_v1, LocalEnvelope},
    tls,
    transport::PairedTransport,
    PhoneSigner,
};

/// Self-signed Ed25519 cert + key, returned as the rustls-pki-types
/// `Der` newtypes the runtime expects.
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

/// How the fake phone responds after reading the `PresentSession`.
/// Lets the same harness drive happy-path, error-envelope, and
/// "close without responding" scenarios.
enum FakeResponse {
    /// Echo the PSBT bytes back inside a `PartialSignature`.
    EchoPartial,
    /// Send back an `ErrorEnvelope` with the given code/message.
    Error { code: String, message: String },
    /// Drop the connection right after reading the request without
    /// sending anything back. Exercises the reader-side
    /// `Disconnected` path.
    Disconnect,
    /// Read the request and then sleep on the virtual clock so the
    /// desktop's `sign_tx` timeout fires. Used with
    /// `#[tokio::test(start_paused = true)]` so the wall-clock wait
    /// collapses to instant via auto-advance.
    HangForever,
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
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier_pinning(desktop_cert_pin))
        .with_single_cert(vec![cert], key)?;

    let (tcp, _peer) = listener.accept().await?;
    let acceptor = TlsAcceptor::from(Arc::new(cfg));
    let mut tls = acceptor.accept(tcp).await?;

    // Read one length-prefixed PresentSession.
    let mut len_buf = [0u8; 4];
    tls.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    tls.read_exact(&mut payload).await?;
    let envelope = LocalEnvelope::decode(payload.as_slice())?;
    let (session_id, psbt_bytes) = match envelope.payload {
        Some(local_v1::local_envelope::Payload::PresentSession(p)) => {
            let s = p.session.expect("session present");
            (s.session_id, s.psbt)
        }
        other => panic!("expected PresentSession, got {:?}", other),
    };

    let reply = match response {
        FakeResponse::EchoPartial => Some(LocalEnvelope {
            payload: Some(local_v1::local_envelope::Payload::Partial(
                local_v1::PartialSignature {
                    session_id,
                    signed_psbt: psbt_bytes,
                    signed_key_ids: Vec::new(),
                },
            )),
        }),
        FakeResponse::Error { code, message } => Some(LocalEnvelope {
            payload: Some(local_v1::local_envelope::Payload::Error(
                local_v1::ErrorEnvelope { code, message },
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
fn verifier_pinning(_pin: tls::CertFingerprint) -> Arc<dyn rustls::server::danger::ClientCertVerifier> {
    WebPkiClientVerifier::no_client_auth()
}

#[tokio::test]
async fn sign_tx_round_trips_through_fake_phone() {
    // 1. Mint desktop and phone identities.
    let (desk_cert, desk_key) = mint_ed25519_cert("Coincube Desktop (test)");
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");

    let desktop_pin = tls::fingerprint_of(&desk_cert);
    let phone_pin = tls::fingerprint_of(&phone_cert);

    let desktop = DesktopIdentity {
        cert_der: desk_cert.clone(),
        key_der: desk_key,
        // Test-only — pubkey field unused by sign_tx itself.
        pubkey: [0u8; 32],
    };

    // 2. Bind the fake phone listener, capture its addr, hand the
    //    bound listener to the phone task — no rebind race.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind fake phone");
    let phone_addr = listener.local_addr().expect("local_addr");
    let phone_handle = tokio::spawn(async move {
        fake_phone(
            listener,
            phone_cert,
            phone_key,
            desktop_pin,
            FakeResponse::EchoPartial,
        )
        .await
        .expect("fake phone");
    });

    // 3. Dial via PairedTransport, build a PhoneSigner, sign.
    let transport = PairedTransport::connect(phone_addr, &desktop, phone_pin)
        .await
        .expect("dial fake phone");
    let paired = PairedPhone {
        identity_pubkey: phone_pin,
        name: "Test phone".into(),
        paired_at_unix: 0,
        wallet_fingerprints: vec![Fingerprint::default()],
        fallback_addr: None,
    };
    let signer = PhoneSigner::new(transport, Fingerprint::default(), None, paired);
    let mut psbt = empty_psbt();
    let original = psbt.serialize();
    async_hwi::HWI::sign_tx(&signer, &mut psbt)
        .await
        .expect("sign_tx ok");
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
    let _hex = fingerprint_hex8(&phone_pin);
}

/// Shared scaffolding for the unhappy-path tests: spin up a fake
/// phone with the given response policy, build a `PhoneSigner`
/// pointed at it, and return the signer so the caller can drive
/// `sign_tx` and assert on the error variant.
async fn signer_against_response(
    response: FakeResponse,
) -> (PhoneSigner, tokio::task::JoinHandle<()>) {
    let (desk_cert, desk_key) = mint_ed25519_cert("Coincube Desktop (test)");
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let desktop_pin = tls::fingerprint_of(&desk_cert);
    let phone_pin = tls::fingerprint_of(&phone_cert);

    let desktop = DesktopIdentity {
        cert_der: desk_cert.clone(),
        key_der: desk_key,
        pubkey: [0u8; 32],
    };

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind fake phone");
    let phone_addr = listener.local_addr().expect("local_addr");
    let handle = tokio::spawn(async move {
        fake_phone(listener, phone_cert, phone_key, desktop_pin, response)
            .await
            .expect("fake phone");
    });

    let transport = PairedTransport::connect(phone_addr, &desktop, phone_pin)
        .await
        .expect("dial fake phone");
    let paired = PairedPhone {
        identity_pubkey: phone_pin,
        name: "Test phone".into(),
        paired_at_unix: 0,
        wallet_fingerprints: vec![Fingerprint::default()],
        fallback_addr: None,
    };
    let signer = PhoneSigner::new(transport, Fingerprint::default(), None, paired);
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

#[tokio::test(start_paused = true)]
async fn sign_tx_returns_timeout_error_when_phone_hangs() {
    // With paused time, the runtime auto-advances the virtual
    // clock whenever every task is blocked on a timer. The fake
    // phone sleeps for a virtual day; the desktop's `sign_tx`
    // times out at `SIGN_RESPONSE_TIMEOUT` (5 min today). The
    // 5 min deadline is earlier, so auto-advance fires it first
    // and we get back an HwiError::Device("sign_tx timeout").
    let (signer, _handle) = signer_against_response(FakeResponse::HangForever).await;
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
