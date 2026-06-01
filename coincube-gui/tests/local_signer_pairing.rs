//! Integration tests for [`pairing_listener::run_pairing`].
//!
//! Inverted from the v1 shape: the desktop is now the TLS **client**
//! during pairing. The harness spins up a fake-phone TLS server on
//! loopback, hands the desktop a `DiscoveredPhone` pointing at it,
//! and drives `run_pairing` to completion.
//!
//! `run_pairing` no longer writes to disk; persistence is the
//! caller's job (see
//! `LocalSigningState::apply_pairing_completed`). These tests assert
//! on the returned `PairedPhone` only.
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

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use prost::Message as _;
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::ServerConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use coincube_gui::phone_signer::{
    errors::PairingError,
    identity::DesktopIdentity,
    mdns::DiscoveredPhone,
    pairing::{PairingOffer, PAIRING_PROTOCOL_VERSION},
    pairing_listener,
    protocol::{local_v1, LocalEnvelope},
    tls,
};

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
        version: PAIRING_PROTOCOL_VERSION,
        // These tests don't exercise the cert-trust path (a separate
        // pair-then-sign integration test does), so any well-formed
        // string here is fine — the cert isn't decoded.
        cert_der_b64: "AAAA".to_string(),
        cert_fp,
        service_name: "keychain-test".to_string(),
        wallet_fingerprint: wallet_fp,
        expires_at_unix: exp,
    }
}

fn fresh_desktop_identity() -> DesktopIdentity {
    let (cert, key) = mint_ed25519_cert("Coincube Desktop (test)");
    DesktopIdentity {
        cert_der: cert,
        key_der: key,
        pubkey: [0u8; 32],
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

    let envelope = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::PairingComplete(
            local_v1::PairingComplete {
                phone_cert_fp: phone_cert_fp_hex,
                device_name,
                app_version: "test-1.0".into(),
                capabilities: vec!["sign-psbt".into()],
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

    // Best-effort read of the desktop's Pong ack. Tolerate EOF.
    let mut len_buf = [0u8; 4];
    let _ = tls.read_exact(&mut len_buf).await;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 0 && len < 16 * 1024 {
        let mut body = vec![0u8; len];
        let _ = tls.read_exact(&mut body).await;
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

    let phone_cert_for_server = phone_cert.clone();
    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert_for_server,
        phone_key,
        "Test Pixel".into(),
        phone_cert_fp_hex.clone(),
    ));

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let offer = fresh_offer(wallet_fp, identity.cert_fp(), 30);
    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let paired = pairing_listener::run_pairing(identity, offer, phone, wallet_fp, vec![wallet_fp])
        .await
        .expect("run_pairing ok");

    assert_eq!(paired.name, "Test Pixel");
    assert_eq!(paired.wallet_fingerprints, vec![wallet_fp]);
    assert_eq!(paired.cert_pin, phone_pin);

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

    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert,
        phone_key,
        "Wrong-wallet phone".into(),
        phone_cert_fp_hex.clone(),
    ));

    // Offer is for `wanted`; desktop's local wallet only contains
    // `actual`. The post-handshake fingerprint check should reject.
    let wanted = Fingerprint::from([9, 9, 9, 9]);
    let actual = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let offer = fresh_offer(wanted, identity.cert_fp(), 30);
    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    // expected_vault_id = `actual`; offer.wallet_fingerprint = `wanted`.
    // The listener compares them as scalars and surfaces the typed
    // mismatch.
    let result = pairing_listener::run_pairing(identity, offer, phone, actual, vec![actual]).await;
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
    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert,
        phone_key,
        "BogusPhone".into(),
        bogus_fp,
    ));

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let identity = fresh_desktop_identity();
    let offer = fresh_offer(wallet_fp, identity.cert_fp(), 30);
    let phone = DiscoveredPhone {
        cert_fp8: "00000000".into(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let result =
        pairing_listener::run_pairing(identity, offer, phone, wallet_fp, vec![wallet_fp]).await;

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
        pairing_listener::run_pairing(identity, offer, phone, wallet_fp, vec![wallet_fp]),
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
    let phone_handle = tokio::spawn(fake_phone_server(
        listener,
        phone_cert,
        phone_key,
        "Test Pixel".into(),
        phone_cert_fp_hex.clone(),
    ));

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
    let phone = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };

    let paired =
        pairing_listener::run_pairing(identity, offer, phone, vault_id, signer_fps.clone())
            .await
            .expect("run_pairing ok");

    assert_eq!(
        paired.wallet_fingerprints, signer_fps,
        "returned fps must be the real signer fps, not the vault id",
    );
    assert!(
        !paired.wallet_fingerprints.contains(&vault_id),
        "vault id must NOT leak into the returned signer-fp list",
    );

    let _ = phone_handle.await;
}
