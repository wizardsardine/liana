//! Integration tests for [`pairing_listener::run_pairing`].
//!
//! Inverted from the v1 shape: the desktop is now the TLS **client**
//! during pairing. The harness spins up a fake-phone TLS server on
//! loopback, hands the desktop a `DiscoveredPhone` pointing at it,
//! and drives `run_pairing` to completion.
//!
//! `run_pairing` owns durable completion. Fake peers cover TLS/frame failures;
//! the cross-language native driver verifies both real application stores.
//!
//! Three scenarios:
//!   1. Happy path — desktop dials, fake phone sends
//!      `PairingComplete`, listener returns `Ok(PairedPhone)` with
//!      the phone's cert pin captured from the TLS handshake.
//!   2. Offer expired — `run_pairing` is called with an offer whose
//!      `expires_at_unix` is in the past; returns `OfferExpired`
//!      without dialing.
//!   3. Wallet fingerprint mismatch — desktop's `wallet_fingerprints`
//!      doesn't contain `offer.wallet_fingerprint`; returns
//!      `WalletFingerprintMismatch`.

#[path = "common/pairing_completion.rs"]
mod pairing_completion;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use prost::Message as _;
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::ServerConfig;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use coincube_gui::phone_signer::{
    errors::PairingError,
    identity::DesktopIdentity,
    mdns::DiscoveredPhone,
    pairing::{self, PairingOffer, PAIRING_PROTOCOL_VERSION},
    pairing_listener,
    protocol::{local_v1, LocalEnvelope},
    tls,
};

/// Compute the v2 proof-of-QR-scan the fake phone must return so the
/// desktop will pin its cert. Mirrors what the keychain-app does after
/// scanning the QR. See
/// `coincube_gui::phone_signer::pairing::pairing_proof`.
fn proof_for(offer: &PairingOffer, phone_cert_fp_hex: &str) -> String {
    pairing::pairing_proof(&offer.psk_b64, &offer.cert_fp, phone_cert_fp_hex).expect("proof")
}

/// A valid compressed secp256k1 point for `PairingComplete.transport_pubkey`.
///
/// Pairing now refuses a phone that reports no usable ECIES transport key, so
/// every fake phone that expects to pair must send one. The value is the
/// generator point — any on-curve point works; these tests never seal to it.
fn valid_transport_pubkey() -> Vec<u8> {
    hex::decode("0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
        .expect("static generator point")
}

fn mint_ed25519_cert(common_name: &str) -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key_pair = KeyPair::generate_for(&PKCS_ED25519).expect("ed25519 keygen");
    let mut params = CertificateParams::new(vec!["test.local".to_string()]).expect("params");
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(rcgen::DnType::CommonName, common_name);
    params.distinguished_name = dn;
    let cert = params.self_signed(&key_pair).expect("self-sign");
    let cert_der = cert.der().clone();
    let key_pkcs8 = key_pair.serialize_der();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pkcs8));
    (cert_der, key_der)
}

fn fresh_offer(wallet_fp: Fingerprint, cert_fp: String, ttl_secs: u64) -> PairingOffer {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + ttl_secs;
    PairingOffer {
        signer_xpub: "selected-test-xpub".into(),
        descriptor_sha256: format!("{}{}", wallet_fp, "00".repeat(28)),
        version: PAIRING_PROTOCOL_VERSION,
        // These tests don't exercise the cert-trust path (a separate
        // pair-then-sign integration test does), so any well-formed
        // string here is fine — the cert isn't decoded.
        cert_der_b64: "AAAA".to_string(),
        cert_fp,
        service_name: "keychain-test".to_string(),
        wallet_fingerprint: wallet_fp,
        expires_at_unix: exp,
        // Borrow a well-formed psk from a throwaway generated offer so
        // the test crate doesn't need base64 directly. The value is
        // arbitrary; what matters is that the fake phone's proof is
        // computed over this same psk (via `proof_for`).
        psk_b64: pairing::generate_offer(
            wallet_fp,
            &fresh_desktop_identity(),
            "x".into(),
            pairing::OfferedKey::default(),
        )
        .offer
        .psk_b64,
    }
}

fn fresh_desktop_identity() -> DesktopIdentity {
    let (cert, key) = mint_ed25519_cert("Coincube Desktop (test)");
    DesktopIdentity {
        cert_der: cert,
        key_der: key,
    }
}

/// Run a one-shot fake-phone TLS server on the supplied `listener`
/// that, on accept, sends a `PairingComplete` envelope with the
/// given `device_name` and drains whatever the desktop writes back.
async fn fake_phone_server(
    listener: TcpListener,
    phone_cert: CertificateDer<'static>,
    phone_key: PrivateKeyDer<'static>,
    device_name: String,
    phone_cert_fp_hex: String,
    pairing_proof: String,
) {
    fake_phone_server_with_transport_key(
        listener,
        phone_cert,
        phone_key,
        device_name,
        phone_cert_fp_hex,
        pairing_proof,
        valid_transport_pubkey(),
    )
    .await
}

/// As [`fake_phone_server`], but the caller chooses what goes in
/// `PairingComplete.transport_pubkey` — including nothing, to exercise the
/// refusal path.
#[allow(clippy::too_many_arguments)]
async fn fake_phone_server_with_transport_key(
    listener: TcpListener,
    phone_cert: CertificateDer<'static>,
    phone_key: PrivateKeyDer<'static>,
    device_name: String,
    phone_cert_fp_hex: String,
    pairing_proof: String,
    transport_pubkey: Vec<u8>,
) {
    fake_phone_server_with_identity(
        listener,
        phone_cert,
        phone_key,
        device_name,
        phone_cert_fp_hex,
        pairing_proof,
        transport_pubkey,
        local_v1::SignerBinding {
            key_id: "10".into(),
            xpub: "selected-test-xpub".into(),
            fingerprint: "01020304".into(),
            descriptor_sha256: [&[1u8, 2, 3, 4][..], &[0u8; 28][..]].concat(),
        },
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn fake_phone_server_with_identity(
    listener: TcpListener,
    phone_cert: CertificateDer<'static>,
    phone_key: PrivateKeyDer<'static>,
    device_name: String,
    phone_cert_fp_hex: String,
    pairing_proof: String,
    transport_pubkey: Vec<u8>,
    binding: local_v1::SignerBinding,
) {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        // Pairing dial uses an unpinned client verifier on the
        // desktop side, but it still presents the desktop's cert.
        // The fake phone accepts that cert unconditionally so we
        // exercise the desktop's outbound TLS, not phone-side auth.
        .with_client_cert_verifier(WebPkiClientVerifier::no_client_auth())
        .with_single_cert(vec![phone_cert], phone_key)
        .expect("single cert");
    let acceptor = TlsAcceptor::from(Arc::new(cfg));

    let (tcp, _peer) = listener.accept().await.expect("accept");
    let mut tls = acceptor.accept(tcp).await.expect("tls handshake");

    let fault_name = device_name.clone();
    let envelope = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::PairingComplete(
            local_v1::PairingComplete {
                completion_protocol: 1,
                signer_binding: Some(binding),
                phone_cert_fp: phone_cert_fp_hex,
                device_name,
                app_version: "test-1.0".into(),
                capabilities: vec!["sign-psbt".into()],
                pairing_proof,
                transport_pubkey,
            },
        )),
    };
    let mut buf = Vec::with_capacity(envelope.encoded_len());
    envelope.encode(&mut buf).expect("encode");
    tls.write_all(&(buf.len() as u32).to_be_bytes())
        .await
        .expect("write len");
    tls.write_all(&buf).await.expect("write body");
    tls.flush().await.expect("flush");

    if let Some(fault) = fault_name.strip_prefix("fault:") {
        let (phase, kind) = fault.split_once(':').unwrap();
        let _ =
            pairing_completion::complete_with_fault(&mut tls, Some((phase.parse().unwrap(), kind)))
                .await;
    } else {
        let _ = pairing_completion::complete(&mut tls).await;
    }
}

#[tokio::test]
async fn run_pairing_happy_path_returns_paired_phone() {
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let phone_cert_fp_hex = phone_pin
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let offer = fresh_offer(wallet_fp, identity.cert_fp(), 30);
    let proof = proof_for(&offer, &phone_cert_fp_hex);

    let phone_cert_for_server = phone_cert.clone();
    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert_for_server,
        phone_key,
        "Test Pixel".into(),
        phone_cert_fp_hex.clone(),
        proof,
    ));

    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let paired = pairing_listener::run_pairing(
        identity,
        offer,
        phone,
        wallet_fp,
        vec![wallet_fp],
        wallet_fp,
        &durable_pairing_test_dir(),
        &Default::default(),
    )
    .await
    .expect("run_pairing ok");

    assert_eq!(paired.name, "Test Pixel");
    assert_eq!(paired.wallet_fingerprints, vec![wallet_fp]);
    assert_eq!(paired.cert_pin, phone_pin);
    // The validated vault id is recorded so the hw refresh loop can
    // scope this phone to the vault it was paired with.
    assert_eq!(paired.vault_fingerprint, wallet_fp);

    let _ = phone_handle.await;
}

#[tokio::test]
async fn run_pairing_returns_offer_expired_when_ttl_in_past() {
    let identity = fresh_desktop_identity();
    let mut offer = fresh_offer(Fingerprint::default(), identity.cert_fp(), 10);
    offer.expires_at_unix = 1; // far in the past
    let phone = DiscoveredPhone {
        cert_fp8: "deadbeef".into(),
        addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), // never dialed
        instance_name: "keychain-test".into(),
    };

    let result = pairing_listener::run_pairing(
        identity,
        offer,
        phone,
        Fingerprint::default(),
        vec![Fingerprint::default()],
        Fingerprint::default(),
        &durable_pairing_test_dir(),
        &Default::default(),
    )
    .await;
    assert!(
        matches!(result, Err(PairingError::OfferExpired)),
        "expected OfferExpired, got {:?}",
        result
    );
}

#[tokio::test]
async fn run_pairing_returns_wallet_fingerprint_mismatch() {
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let phone_cert_fp_hex = phone_pin
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    // Offer is for `wanted`; desktop's local wallet only contains
    // `actual`. The post-handshake fingerprint check should reject.
    let wanted = Fingerprint::from([9, 9, 9, 9]);
    let actual = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let offer = fresh_offer(wanted, identity.cert_fp(), 30);
    // Valid proof so we get PAST the phone-auth check and actually
    // exercise the wallet-fingerprint mismatch this test is about.
    let proof = proof_for(&offer, &phone_cert_fp_hex);

    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert,
        phone_key,
        "Wrong-wallet phone".into(),
        phone_cert_fp_hex.clone(),
        proof,
    ));

    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    // expected_vault_id = `actual`; offer.wallet_fingerprint = `wanted`.
    // The listener compares them as scalars and surfaces the typed
    // mismatch.
    let result = pairing_listener::run_pairing(
        identity,
        offer,
        phone,
        actual,
        vec![actual],
        actual,
        &durable_pairing_test_dir(),
        &Default::default(),
    )
    .await;
    match result {
        Err(PairingError::WalletFingerprintMismatch { expected, claimed }) => {
            assert_eq!(expected, vec![actual]);
            assert_eq!(claimed, wanted);
        }
        other => panic!("expected WalletFingerprintMismatch, got {:?}", other),
    }

    let _ = phone_handle.await;
}

#[tokio::test]
async fn run_pairing_rejects_phone_reporting_mismatched_cert_fp() {
    // The proto requires `PairingComplete.phone_cert_fp` to match the
    // SHA-256 of the cert the phone presented during TLS. A
    // misconfigured phone that reports a different value must fail
    // pairing rather than silently persisting the live pin.
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    // Deliberately wrong: 64 hex chars of zeros, not the real
    // cert's SHA-256.
    let bogus_fp = "0".repeat(64);
    // The reported-cert-fp equality check runs before the proof check,
    // so this never reaches proof verification — an empty proof is
    // fine.
    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert,
        phone_key,
        "BogusPhone".into(),
        bogus_fp,
        String::new(),
    ));

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let offer = fresh_offer(wallet_fp, identity.cert_fp(), 30);
    let phone = DiscoveredPhone {
        cert_fp8: "00000000".into(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let result = pairing_listener::run_pairing(
        identity,
        offer,
        phone,
        wallet_fp,
        vec![wallet_fp],
        wallet_fp,
        &durable_pairing_test_dir(),
        &Default::default(),
    )
    .await;

    match result {
        Err(PairingError::InternalError(msg)) => {
            assert!(
                msg.contains("doesn't match TLS handshake"),
                "expected mismatch error, got: {}",
                msg
            );
        }
        other => panic!("expected InternalError(mismatch), got {:?}", other),
    }

    let _ = phone_handle.await;
}

#[tokio::test]
async fn run_pairing_rejects_invalid_pairing_proof() {
    // Security regression: the desktop dials unpinned and pins
    // whatever cert answers, so the proof-of-QR-scan is the ONLY thing
    // tying that cert to the device that scanned the QR. A peer that
    // reports a correct cert fp (passes the equality check) but a
    // wrong/absent proof must be refused, not pinned — this is the
    // active-LAN-attacker / wrong-psk case. See
    // plans/PLAN-local-signer-pairing-phone-auth.md.
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let phone_cert_fp_hex = phone_pin
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    // Correct cert fp (equality check passes), but a proof that
    // doesn't match the offer's psk — 64 hex chars of zeros.
    let bad_proof = "0".repeat(64);
    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert,
        phone_key,
        "Impostor".into(),
        phone_cert_fp_hex.clone(),
        bad_proof,
    ));

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let offer = fresh_offer(wallet_fp, identity.cert_fp(), 30);
    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let result = pairing_listener::run_pairing(
        identity,
        offer,
        phone,
        wallet_fp,
        vec![wallet_fp],
        wallet_fp,
        &durable_pairing_test_dir(),
        &Default::default(),
    )
    .await;

    assert!(
        matches!(result, Err(PairingError::PhoneVerificationFailed)),
        "expected PhoneVerificationFailed, got {:?}",
        result,
    );

    let _ = phone_handle.await;
}

/// Fake phone that completes TLS but never sends the
/// `PairingComplete` envelope. Used to exercise the recv-side TTL
/// bound in [`pairing_listener::run_pairing`].
async fn fake_phone_silent_after_tls(
    listener: TcpListener,
    phone_cert: CertificateDer<'static>,
    phone_key: PrivateKeyDer<'static>,
) {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_client_cert_verifier(WebPkiClientVerifier::no_client_auth())
        .with_single_cert(vec![phone_cert], phone_key)
        .expect("single cert");
    let acceptor = TlsAcceptor::from(Arc::new(cfg));

    let (tcp, _peer) = listener.accept().await.expect("accept");
    let _tls = acceptor.accept(tcp).await.expect("tls handshake");
    // Hold the TLS connection open without sending PairingComplete.
    // 30 s is well past the 2 s offer TTL used in the test.
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
}

#[tokio::test]
async fn run_pairing_returns_offer_expired_when_phone_stalls_after_tls() {
    // Real regression for the recv-side TTL bound: phone completes
    // TLS (so the dial succeeds and we get past the pre-dial expiry
    // check), then never sends `PairingComplete`. Before the bound,
    // `reader.recv().await` would hang indefinitely. With the bound,
    // the recv is wrapped in `tokio::time::timeout(remaining_ttl,
    // ...)` and returns `Err(OfferExpired)` when the TTL elapses.
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let phone_handle = tokio::spawn(fake_phone_silent_after_tls(listener, phone_cert, phone_key));

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    // Why 2 s, not 1 s: `fresh_offer` computes `expires_at_unix =
    // now_seconds + ttl`. After the TLS handshake the listener
    // re-reads the wall clock with second granularity and computes
    // `remaining = expires_at_unix - now`. With ttl=1, a test
    // started near the end of a wall-clock second can see the next
    // second tick before the handshake completes, leaving
    // `remaining == 0` and triggering the **pre-recv** OfferExpired
    // branch — the same one already covered by
    // `run_pairing_returns_offer_expired_when_ttl_in_past`. ttl=2
    // guarantees the handshake-completion timestamp sees ≥ 1 s
    // remaining so the recv-side timeout is the only branch that
    // can fire, which is what this test is meant to exercise.
    let offer = fresh_offer(wallet_fp, identity.cert_fp(), 2);
    let phone = DiscoveredPhone {
        cert_fp8: "deadbeef".into(),
        addr,
        instance_name: "keychain-test".into(),
    };

    // Outer cap fails the test fast on regression instead of hanging
    // CI; ~5 s is plenty given the 2 s offer TTL.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pairing_listener::run_pairing(
            identity,
            offer,
            phone,
            wallet_fp,
            vec![wallet_fp],
            wallet_fp,
            &durable_pairing_test_dir(),
            &Default::default(),
        ),
    )
    .await
    .expect("run_pairing must return within the outer cap");

    assert!(
        matches!(result, Err(PairingError::OfferExpired)),
        "expected OfferExpired, got {:?}",
        result,
    );

    // Cancel the stalled phone task so it doesn't outlive the test;
    // its 30 s sleep would otherwise sit in the runtime past the
    // assertion. `abort()` signals cancellation and the JoinHandle
    // then drops naturally without tripping clippy's
    // `let_underscore_future`.
    phone_handle.abort();
}

/// Regression for the "Vault ID breaks phone signer" finding: the
/// offer's `wallet_fingerprint` is the vault id (a descriptor-hash
/// id_fingerprint), which is **not** one of `descriptor_keys()`. The
/// listener must surface `signer_fingerprints` (the real BIP-32
/// master fps) on `PairedPhone.wallet_fingerprints`, not the vault
/// id — otherwise the hw refresh tick's `descriptor_keys()` filter
/// would downgrade the phone to `Unsupported(NotPartOfWallet)` on
/// every tick.
#[tokio::test]
async fn run_pairing_returns_signer_fps_not_vault_id() {
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let phone_cert_fp_hex = phone_pin
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    // Vault id and signer fps are deliberately disjoint — this is
    // the realistic shape: `id_fingerprint` is sha256(descriptor)[..4]
    // and won't accidentally collide with any BIP-32 master fp.
    let vault_id = Fingerprint::from([0xaa, 0xbb, 0xcc, 0xdd]);
    let signer_fps = vec![
        Fingerprint::from([1, 2, 3, 4]),
        Fingerprint::from([5, 6, 7, 8]),
    ];

    let identity = fresh_desktop_identity();
    let offer = fresh_offer(vault_id, identity.cert_fp(), 30);
    let proof = proof_for(&offer, &phone_cert_fp_hex);
    let phone_handle = tokio::spawn(fake_phone_server_with_identity(
        listener,
        phone_cert,
        phone_key,
        "Test Pixel".into(),
        phone_cert_fp_hex.clone(),
        proof,
        valid_transport_pubkey(),
        local_v1::SignerBinding {
            key_id: "11".into(),
            xpub: offer.signer_xpub.clone(),
            fingerprint: signer_fps[1].to_string(),
            descriptor_sha256: hex::decode(&offer.descriptor_sha256).unwrap(),
        },
    ));

    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let paired = pairing_listener::run_pairing(
        identity,
        offer,
        phone,
        vault_id,
        signer_fps.clone(),
        signer_fps[1],
        &durable_pairing_test_dir(),
        &Default::default(),
    )
    .await
    .expect("run_pairing ok");

    assert_eq!(
        paired.wallet_fingerprints, vec![signer_fps[1]],
        "only the exact selected phone key is advertised, not all descriptor fingerprints or the vault id",
    );
    assert!(
        !paired.wallet_fingerprints.contains(&vault_id),
        "vault id must NOT leak into the returned signer-fp list",
    );

    let _ = phone_handle.await;
}

/// A legacy offer — one with no `signer_xpub`, as generated before
/// exact-key pairing existed — must say so, not blame the phone.
///
/// Both failures used to share "Exact pairing identity mismatch; pair
/// again.", which sends the user to re-pick a key on their handset when the
/// QR on their screen is what's stale. Only regenerating the offer fixes it.
#[tokio::test]
async fn run_pairing_reports_a_legacy_offer_rather_than_blaming_the_phone() {
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let phone_cert_fp_hex = phone_pin
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let mut offer = fresh_offer(wallet_fp, identity.cert_fp(), 30);
    // Exactly what a pre-exact-key desktop would have put on screen.
    offer.signer_xpub = String::new();
    let proof = proof_for(&offer, &phone_cert_fp_hex);
    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert,
        phone_key,
        "Test Pixel".into(),
        phone_cert_fp_hex.clone(),
        proof,
    ));

    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let result = pairing_listener::run_pairing(
        identity,
        offer,
        phone,
        wallet_fp,
        vec![wallet_fp],
        wallet_fp,
        &durable_pairing_test_dir(),
        &Default::default(),
    )
    .await;

    match result {
        Err(PairingError::InternalError(msg)) => {
            assert!(
                msg.contains("Start pairing again"),
                "expected the regenerate-the-QR error, got: {}",
                msg,
            );
            assert!(
                !msg.contains("Exact pairing identity mismatch"),
                "a stale offer must not be reported as a phone-side mismatch: {}",
                msg,
            );
        }
        other => panic!(
            "an offer with no signer_xpub must be refused; got {:?}",
            other.map(|p| p.wallet_fingerprints),
        ),
    }

    phone_handle.abort();
}

/// A phone that reports the QR-selected xpub alongside a *different*
/// vault signer's fingerprint must be refused.
///
/// Vault membership alone doesn't bind the two halves of the reported
/// identity: both keys here are legitimately in the descriptor, so the
/// membership check passes and only the xpub/fingerprint binding can
/// reject this. Without it the desktop would persist a `SignerBinding`
/// whose xpub and fingerprint name different keys, and advertise the
/// phone under `wallet_fingerprints` for a key it cannot sign for.
#[tokio::test]
async fn run_pairing_rejects_fingerprint_of_a_different_vault_key() {
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let phone_cert_fp_hex = phone_pin
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    let vault_id = Fingerprint::from([0xaa, 0xbb, 0xcc, 0xdd]);
    let selected = Fingerprint::from([1, 2, 3, 4]);
    let other = Fingerprint::from([5, 6, 7, 8]);
    let signer_fps = vec![selected, other];

    let identity = fresh_desktop_identity();
    let offer = fresh_offer(vault_id, identity.cert_fp(), 30);
    let proof = proof_for(&offer, &phone_cert_fp_hex);
    let phone_handle = tokio::spawn(fake_phone_server_with_identity(
        listener,
        phone_cert,
        phone_key,
        "Test Pixel".into(),
        phone_cert_fp_hex.clone(),
        proof,
        valid_transport_pubkey(),
        local_v1::SignerBinding {
            key_id: "11".into(),
            // The selected xpub, so every other identity check passes...
            xpub: offer.signer_xpub.clone(),
            // ...but a fingerprint belonging to the vault's *other* key.
            fingerprint: other.to_string(),
            descriptor_sha256: hex::decode(&offer.descriptor_sha256).unwrap(),
        },
    ));

    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let result = pairing_listener::run_pairing(
        identity,
        offer,
        phone,
        vault_id,
        signer_fps,
        selected,
        &durable_pairing_test_dir(),
        &Default::default(),
    )
    .await;

    match result {
        Err(PairingError::InternalError(msg)) => assert!(
            msg.contains("doesn't match the selected key"),
            "expected the xpub/fingerprint binding error, got: {}",
            msg,
        ),
        other => panic!(
            "pairing must fail when the reported fingerprint is a different vault key; got {:?}",
            other.map(|p| p.wallet_fingerprints),
        ),
    }

    phone_handle.abort();
}

/// Two-shot fake phone: completes TLS on the first inbound
/// connection and immediately drops it (mimicking a phone whose
/// pairing handler hasn't seen the QR scan yet), then serves a
/// real `PairingComplete` on the second connection. Used to
/// exercise the redial loop in [`pairing_listener::run_pairing`].
async fn fake_phone_close_then_serve(
    listener: TcpListener,
    phone_cert: CertificateDer<'static>,
    phone_key: PrivateKeyDer<'static>,
    device_name: String,
    phone_cert_fp_hex: String,
    pairing_proof: String,
) {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_client_cert_verifier(WebPkiClientVerifier::no_client_auth())
        .with_single_cert(vec![phone_cert], phone_key)
        .expect("single cert");
    let acceptor = TlsAcceptor::from(Arc::new(cfg));

    // Connection #1: complete handshake, drop without writing.
    let (tcp, _peer) = listener.accept().await.expect("accept #1");
    let tls = acceptor.accept(tcp).await.expect("tls handshake #1");
    drop(tls);

    // Connection #2: real pairing serve.
    let (tcp, _peer) = listener.accept().await.expect("accept #2");
    let mut tls = acceptor.accept(tcp).await.expect("tls handshake #2");
    let envelope = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::PairingComplete(
            local_v1::PairingComplete {
                completion_protocol: 1,
                signer_binding: Some(local_v1::SignerBinding {
                    key_id: "10".into(),
                    xpub: "selected-test-xpub".into(),
                    fingerprint: "01020304".into(),
                    descriptor_sha256: [&[1u8, 2, 3, 4][..], &[0u8; 28][..]].concat(),
                }),
                phone_cert_fp: phone_cert_fp_hex,
                device_name,
                app_version: "test-1.0".into(),
                capabilities: vec!["sign-psbt".into()],
                pairing_proof,
                transport_pubkey: valid_transport_pubkey(),
            },
        )),
    };
    let mut buf = Vec::with_capacity(envelope.encoded_len());
    envelope.encode(&mut buf).expect("encode");
    tls.write_all(&(buf.len() as u32).to_be_bytes())
        .await
        .expect("write len");
    tls.write_all(&buf).await.expect("write body");
    tls.flush().await.expect("flush");

    let _ = pairing_completion::complete(&mut tls).await;
}

/// Regression: when the retry loop has accumulated `NetworkError`s
/// from failed dials and the offer TTL then runs out, the wizard
/// must surface `OfferExpired` (so the user sees "Offer expired —
/// generate a new offer") rather than the last `NetworkError`
/// (which would route them to the network-error toast with a Try
/// Again that does nothing useful for a dead QR).
#[tokio::test]
async fn run_pairing_returns_offer_expired_when_retries_exhaust_ttl() {
    // Bind, capture the address, then drop the listener. Any TCP
    // connect to this address now gets RST → `Connection refused`,
    // a `NetworkError` the retry loop treats as retriable.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    drop(listener);

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    // 2 s budget — long enough for two or three retry cycles
    // (REDIAL_BACKOFF is 750 ms), short enough not to slow CI.
    let offer = fresh_offer(wallet_fp, identity.cert_fp(), 2);
    let phone = DiscoveredPhone {
        cert_fp8: "deadbeef".into(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pairing_listener::run_pairing(
            identity,
            offer,
            phone,
            wallet_fp,
            vec![wallet_fp],
            wallet_fp,
            &durable_pairing_test_dir(),
            &Default::default(),
        ),
    )
    .await
    .expect("run_pairing must complete within outer cap");

    assert!(
        matches!(result, Err(PairingError::OfferExpired)),
        "expected OfferExpired after TTL elapses through retries, got {:?}",
        result,
    );
}

/// Regression: the phone closes inbound TLS sessions before it has
/// seen the QR scan, so the user's very first dial after clicking
/// "Pair" almost always fails. The desktop must redial within the
/// offer TTL so the user has time to scan; without it, the QR
/// vanishes on the first failure and the user never gets a chance.
#[tokio::test]
async fn run_pairing_redials_after_phone_closes_early() {
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let phone_cert_fp_hex = phone_pin
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let offer = fresh_offer(wallet_fp, identity.cert_fp(), 30);
    let proof = proof_for(&offer, &phone_cert_fp_hex);

    let phone_handle = tokio::spawn(fake_phone_close_then_serve(
        listener,
        phone_cert,
        phone_key,
        "Pixel".into(),
        phone_cert_fp_hex.clone(),
        proof,
    ));

    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    // Outer cap so a regression fails CI fast instead of hanging.
    // 5s is plenty: one failed dial + REDIAL_BACKOFF (750ms) + one
    // successful dial should land inside a second or two.
    let paired = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pairing_listener::run_pairing(
            identity,
            offer,
            phone,
            wallet_fp,
            vec![wallet_fp],
            wallet_fp,
            &durable_pairing_test_dir(),
            &Default::default(),
        ),
    )
    .await
    .expect("run_pairing must complete within cap")
    .expect("run_pairing ok after redial");

    assert_eq!(paired.cert_pin, phone_pin);
    assert_eq!(paired.name, "Pixel");

    let _ = phone_handle.await;
}

/// Pairing fails closed when the phone reports no usable ECIES transport key.
///
/// This is the gate that keeps the LAN rail sealed. The desktop seals the PSBT
/// and the full descriptor to this key, so a phone we cannot seal to could only
/// ever sign over a plaintext path — and a LAN peer is exactly the untrusted
/// party such a fallback would empower. Refusing at pairing turns that into a
/// "update the Keychain app" prompt instead of a phone that pairs cleanly and
/// then fails, or worse, silently downgrades.
#[tokio::test]
async fn run_pairing_refuses_a_phone_with_no_transport_key() {
    // x = 2^256-1 exceeds the secp256k1 field prime, so this is the right
    // length and a legal prefix but not a point at all. (Note `[0x02; 33]`
    // would NOT work here: that x is genuinely on the curve.)
    let off_curve = {
        let mut v = Vec::with_capacity(33);
        v.push(0x02);
        v.extend_from_slice(&[0xff; 32]);
        v
    };
    // A real point in the UNCOMPRESSED encoding. `PublicKey::from_slice`
    // accepts it, so a parse-only gate would pair this phone and then fail
    // every sign against the 33-byte requirement in `seal_to_device`.
    let uncompressed = {
        use coincube_core::miniscript::bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
        let secp = Secp256k1::new();
        let sk = SecretKey::from_slice(&[0x11; 32]).expect("static secret");
        PublicKey::from_secret_key(&secp, &sk)
            .serialize_uncompressed()
            .to_vec()
    };
    for bad_key in [
        Vec::new(),     // pre-blinding Keychain: field absent
        vec![0x02; 10], // truncated
        off_curve,
        uncompressed,
    ] {
        let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
        let phone_pin = tls::fingerprint_of(&phone_cert);
        let phone_cert_fp_hex = phone_pin
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");

        let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
        let identity = fresh_desktop_identity();
        let offer = fresh_offer(wallet_fp, identity.cert_fp(), 30);
        let proof = proof_for(&offer, &phone_cert_fp_hex);

        let handle = tokio::spawn(fake_phone_server_with_transport_key(
            listener,
            phone_cert.clone(),
            phone_key,
            "Test Pixel".into(),
            phone_cert_fp_hex.clone(),
            proof,
            bad_key.clone(),
        ));

        let phone = DiscoveredPhone {
            cert_fp8: phone_cert_fp_hex[..8].to_string(),
            addr,
            instance_name: "keychain-test".into(),
        };
        let res = pairing_listener::run_pairing(
            identity,
            offer,
            phone,
            wallet_fp,
            vec![wallet_fp],
            wallet_fp,
            &durable_pairing_test_dir(),
            &Default::default(),
        )
        .await;

        assert!(
            matches!(res, Err(PairingError::TransportKeyMissing)),
            "a phone reporting transport_pubkey {:?} must be refused, got {:?}",
            bad_key,
            res.map(|p| p.name),
        );
        let _ = handle.await;
    }
}

fn durable_pairing_test_dir() -> coincube_gui::dir::CoincubeDirectory {
    let p = std::env::temp_dir().join(format!("pairing-protocol-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&p).unwrap();
    coincube_gui::dir::CoincubeDirectory::new(p)
}

/// Real TLS checkpoints, not just a stale UI completion assertion. The peer
/// stops at an observed frame boundary and revokes the actual run authority.
#[tokio::test]
async fn cancellation_at_tls_identity_acceptance_and_pre_persistence_leaves_no_row() {
    use coincube_gui::phone_signer::{pairing_run::PairingRun, pairing_store};
    use tokio::io::AsyncReadExt;
    for stop in 0..6 {
        let dir = durable_pairing_test_dir();
        let identity = fresh_desktop_identity();
        let fp = Fingerprint::from([1, 2, 3, 4]);
        let offer = fresh_offer(fp, hex::encode(tls::fingerprint_of(&identity.cert_der)), 30);
        let (cert, key) = mint_ed25519_cert("cancel test");
        let phone_fp = hex::encode(tls::fingerprint_of(&cert));
        let complete = LocalEnvelope {
            payload: Some(local_v1::local_envelope::Payload::PairingComplete(
                local_v1::PairingComplete {
                    completion_protocol: 1,
                    phone_cert_fp: phone_fp.clone(),
                    device_name: "phone".into(),
                    app_version: "test".into(),
                    capabilities: vec![],
                    pairing_proof: proof_for(&offer, &phone_fp),
                    transport_pubkey: valid_transport_pubkey(),
                    signer_binding: Some(local_v1::SignerBinding {
                        key_id: "10".into(),
                        xpub: offer.signer_xpub.clone(),
                        fingerprint: fp.to_string(),
                        descriptor_sha256: hex::decode(&offer.descriptor_sha256).unwrap(),
                    }),
                },
            )),
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let run = PairingRun::default();
        let peer_run = run.clone();
        let peer_dir = dir.clone();
        let server = tokio::spawn(async move {
            let cfg = ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .unwrap();
            let (tcp, _) = listener.accept().await.unwrap();
            let mut stream = TlsAcceptor::from(Arc::new(cfg)).accept(tcp).await.unwrap();
            if stop == 0 {
                peer_run.cancel();
                return;
            }
            let bytes = complete.encode_to_vec();
            stream.write_u32(bytes.len() as u32).await.unwrap();
            stream.write_all(&bytes).await.unwrap();
            stream.flush().await.unwrap();
            if stop == 1 {
                peer_run.cancel();
                return;
            }
            let len = stream.read_u32().await.unwrap();
            let mut bytes = vec![0; len as usize];
            stream.read_exact(&mut bytes).await.unwrap();
            let env = LocalEnvelope::decode(&*bytes).unwrap();
            let Some(local_v1::local_envelope::Payload::PairingStep(step)) = env.payload else {
                panic!("no acceptance")
            };
            assert_eq!(step.phase, local_v1::pairing_step::Phase::Accept as i32);
            if stop >= 4 {
                use local_v1::{local_envelope::Payload, pairing_step::Phase};
                // The peer has not acknowledged its durable write. A late
                // COMMITTED response must not override Cancel.
                for (reply, expected) in [
                    (Phase::Prepared, Phase::Commit),
                    (Phase::Committed, Phase::Finish),
                ] {
                    if reply == Phase::Committed && stop == 4 {
                        peer_run.cancel();
                    }
                    let bytes = LocalEnvelope {
                        payload: Some(Payload::PairingStep(local_v1::PairingStep {
                            transaction_id: step.transaction_id.clone(),
                            phase: reply as i32,
                        })),
                    }
                    .encode_to_vec();
                    if stream.write_u32(bytes.len() as u32).await.is_err() {
                        return;
                    }
                    if stream.write_all(&bytes).await.is_err() {
                        return;
                    }
                    let Ok(len) = stream.read_u32().await else {
                        return;
                    };
                    let mut bytes = vec![0; len as usize];
                    if stream.read_exact(&mut bytes).await.is_err() {
                        return;
                    }
                    let env = LocalEnvelope::decode(&*bytes).unwrap();
                    let Some(Payload::PairingStep(received)) = env.payload else {
                        return;
                    };
                    assert_eq!(received.phase, expected as i32);
                    if expected == Phase::Finish {
                        assert!(pairing_store::load(&peer_dir).unwrap().phones.is_empty());
                        peer_run.cancel(); // Too late: durable hidden decision wins.
                        peer_run.check().unwrap();
                    }
                }
                let bytes = LocalEnvelope {
                    payload: Some(Payload::PairingStep(local_v1::PairingStep {
                        transaction_id: step.transaction_id,
                        phase: Phase::Finished as i32,
                    })),
                }
                .encode_to_vec();
                let _ = stream.write_u32(bytes.len() as u32).await;
                let _ = stream.write_all(&bytes).await;
                return;
            }
            // Both remaining checkpoints are before PREPARED permits the
            // desktop's durable decision; cancellation wins that race.
            peer_run.cancel();
            if stop == 3 {
                let bytes = LocalEnvelope {
                    payload: Some(local_v1::local_envelope::Payload::PairingStep(
                        local_v1::PairingStep {
                            transaction_id: step.transaction_id,
                            phase: local_v1::pairing_step::Phase::Prepared as i32,
                        },
                    )),
                }
                .encode_to_vec();
                let _ = stream.write_u32(bytes.len() as u32).await;
                let _ = stream.write_all(&bytes).await;
            }
        });
        let result = pairing_listener::run_pairing(
            identity,
            offer,
            DiscoveredPhone {
                cert_fp8: "test".into(),
                addr,
                instance_name: "test".into(),
            },
            fp,
            vec![fp],
            fp,
            &dir,
            &run,
        )
        .await;
        server.await.unwrap();
        if stop == 5 {
            assert!(
                result.is_ok(),
                "postdecision cancellation reversed completion"
            );
            assert_eq!(pairing_store::load(&dir).unwrap().phones.len(), 1);
        } else {
            assert!(result.is_err(), "checkpoint {} accepted", stop);
            assert!(
                pairing_store::load(&dir).unwrap().phones.is_empty(),
                "checkpoint {} persisted",
                stop
            );
        }
    }
}

#[tokio::test]
async fn cancellation_stops_redial_and_late_persistence() {
    use coincube_gui::phone_signer::{pairing_run::PairingRun, pairing_store};
    let dir = durable_pairing_test_dir();
    let identity = fresh_desktop_identity();
    let fp = Fingerprint::from([1, 2, 3, 4]);
    let offer = fresh_offer(fp, hex::encode(tls::fingerprint_of(&identity.cert_der)), 30);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let run = PairingRun::default();
    let peer_run = run.clone();
    let peer = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        drop(tcp);
        peer_run.cancel();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    assert!(pairing_listener::run_pairing(
        identity,
        offer,
        DiscoveredPhone {
            cert_fp8: "test".into(),
            addr,
            instance_name: "test".into()
        },
        fp,
        vec![fp],
        fp,
        &dir,
        &run
    )
    .await
    .is_err());
    peer.await.unwrap();
    assert!(pairing_store::load(&dir).unwrap().phones.is_empty());
}

#[tokio::test]
async fn eof_timeout_and_unexpected_frames_at_each_desktop_boundary_leave_no_trusted_row() {
    for boundary in 0..3 {
        for fault in ["eof", "unexpected", "timeout"] {
            let dir = durable_pairing_test_dir();
            let identity = fresh_desktop_identity();
            let fp = Fingerprint::from([1, 2, 3, 4]);
            let offer = fresh_offer(fp, hex::encode(tls::fingerprint_of(&identity.cert_der)), 60);
            let (cert, key) = mint_ed25519_cert("fault test");
            let pin = hex::encode(tls::fingerprint_of(&cert));
            let proof = proof_for(&offer, &pin);
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let addr = listener.local_addr().unwrap();
            let peer = tokio::spawn(fake_phone_server(
                listener,
                cert,
                key,
                format!("fault:{}:{}", boundary, fault),
                pin,
                proof,
            ));
            let result = pairing_listener::run_pairing(
                identity,
                offer,
                DiscoveredPhone {
                    cert_fp8: "test".into(),
                    addr,
                    instance_name: "test".into(),
                },
                fp,
                vec![fp],
                fp,
                &dir,
                &Default::default(),
            )
            .await;
            assert!(result.is_err(), "{} {}", boundary, fault);
            peer.await.unwrap();
            let journal =
                std::fs::read_to_string(dir.path().join("pairing-transactions.json")).unwrap();
            if boundary == 2 {
                assert!(
                    journal.contains("decided"),
                    "postdecision {} must retain hidden recovery",
                    fault
                );
                assert!(!journal.contains("completed"));
            } else {
                assert_eq!(journal, "{}");
            }
            assert!(
                coincube_gui::phone_signer::pairing_store::load(&dir)
                    .unwrap()
                    .phones
                    .is_empty(),
                "{} {}",
                boundary,
                fault
            );
        }
    }
}
