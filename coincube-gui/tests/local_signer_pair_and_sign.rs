//! Phase 5 pair-then-sign integration, extended in the cert-in-QR
//! round to exercise the production trust path:
//!
//!   1. The desktop builds the offer via `generate_offer(&identity)`
//!      so `cert` and `certFp` come from the same source.
//!   2. The offer is encoded to base64url(JSON) and decoded back —
//!      i.e. the in-memory struct doesn't shortcut the wire format.
//!   3. The fake phone re-derives `sha256(cert)` and checks it
//!      against `certFp` — the same check the real phone does.
//!   4. The fake phone uses a `PinnedVerifier` keyed on the cert
//!      from the QR as its `ClientCertVerifier`. The TLS handshake
//!      therefore succeeds **only because** the fake phone trusted
//!      the cert pulled from the offer.
//!   5. A second test pins the fake phone to a wrong cert and
//!      asserts the handshake fails — proving the trust path is
//!      doing the work.

use std::net::Ipv4Addr;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{
    bip32::Fingerprint, psbt::Psbt, transaction::Version as TxVersion, Transaction,
};
use prost::Message as _;
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;

use coincube_gui::dir::CoincubeDirectory;
use coincube_gui::phone_signer::{
    identity::DesktopIdentity,
    mdns::DiscoveredPhone,
    pairing::{decode_offer, encode_offer, generate_offer, PairingOffer},
    pairing_listener,
    pairing_store::PairedPhone,
    protocol::{local_v1, LocalEnvelope},
    tls::{self, CertFingerprint, PinnedVerifier},
    transport::PairedTransport,
    PhoneSigner,
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

fn fresh_dir() -> CoincubeDirectory {
    let mut path = std::env::temp_dir();
    path.push(format!("coincube-pair-then-sign-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).expect("mkdir tempdir");
    CoincubeDirectory::new(path)
}

fn fresh_desktop_identity() -> DesktopIdentity {
    let dir = fresh_dir();
    coincube_gui::phone_signer::identity::load_or_create(&dir).expect("identity")
}

fn empty_psbt() -> Psbt {
    let tx = Transaction {
        version: TxVersion::TWO,
        lock_time: coincube_core::miniscript::bitcoin::absolute::LockTime::ZERO,
        input: Vec::new(),
        output: Vec::new(),
    };
    Psbt::from_unsigned_tx(tx).expect("empty psbt")
}

/// Validate `cert` and `certFp` agree, then extract the desktop's
/// cert fingerprint from the decoded offer. Mirrors what the real
/// phone-side handler does before trusting the cert.
fn extract_trusted_desktop_pin(offer: &PairingOffer) -> CertFingerprint {
    let cert_bytes = URL_SAFE_NO_PAD
        .decode(&offer.cert_der_b64)
        .expect("decode cert b64");
    let digest = Sha256::digest(&cert_bytes);
    let computed_hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    assert_eq!(computed_hex, offer.cert_fp, "cert/certFp must agree");
    let mut pin = [0u8; 32];
    pin.copy_from_slice(&digest);
    pin
}

/// Build a TLS `ServerConfig` for the fake phone that pins the
/// desktop's cert via `PinnedVerifier`. Passing the **wrong** pin
/// here causes the TLS handshake to fail with a cert-pin-mismatch
/// error.
fn fake_phone_server_config(
    phone_cert: CertificateDer<'static>,
    phone_key: PrivateKeyDer<'static>,
    trust_desktop_pin: CertFingerprint,
) -> ServerConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_client_cert_verifier(PinnedVerifier::new(trust_desktop_pin))
        .with_single_cert(vec![phone_cert], phone_key)
        .expect("single cert")
}

/// Fake phone harness that accepts the pairing connection first and
/// then a second steady-state connection on the same listener. Both
/// accepts use the same `ServerConfig`, so a wrong `trust_desktop_pin`
/// fails the first handshake and the function returns early.
async fn fake_phone_pair_then_sign(
    listener: TcpListener,
    phone_cert: CertificateDer<'static>,
    phone_key: PrivateKeyDer<'static>,
    phone_cert_fp_hex: String,
    trust_desktop_pin: CertFingerprint,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cfg = fake_phone_server_config(phone_cert, phone_key, trust_desktop_pin);
    let acceptor = TlsAcceptor::from(Arc::new(cfg));

    // ── Accept #1: pairing.
    {
        let (tcp, _peer) = listener.accept().await?;
        let mut tls = acceptor.accept(tcp).await?;

        let env = LocalEnvelope {
            payload: Some(local_v1::local_envelope::Payload::PairingComplete(
                local_v1::PairingComplete {
                    phone_cert_fp: phone_cert_fp_hex.clone(),
                    device_name: "TestPhone".into(),
                    app_version: "test-1.0".into(),
                    capabilities: vec!["sign-psbt".into()],
                },
            )),
        };
        let mut buf = Vec::with_capacity(env.encoded_len());
        env.encode(&mut buf)?;
        tls.write_all(&(buf.len() as u32).to_be_bytes()).await?;
        tls.write_all(&buf).await?;
        tls.flush().await?;

        // Best-effort drain of the desktop's Pong ack.
        let mut len_buf = [0u8; 4];
        let _ = tls.read_exact(&mut len_buf).await;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > 0 && len < 16 * 1024 {
            let mut body = vec![0u8; len];
            let _ = tls.read_exact(&mut body).await;
        }
    }

    // ── Accept #2: steady-state signing.
    {
        let (tcp, _peer) = listener.accept().await?;
        let mut tls = acceptor.accept(tcp).await?;

        let mut len_buf = [0u8; 4];
        tls.read_exact(&mut len_buf).await?;
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut payload = vec![0u8; len];
        tls.read_exact(&mut payload).await?;
        let env = LocalEnvelope::decode(payload.as_slice())?;
        let (session_id, psbt_bytes) = match env.payload {
            Some(local_v1::local_envelope::Payload::PresentSession(p)) => {
                let s = p.session.expect("session");
                (s.session_id, s.psbt)
            }
            other => panic!("expected PresentSession, got {:?}", other),
        };

        let reply = LocalEnvelope {
            payload: Some(local_v1::local_envelope::Payload::Partial(
                local_v1::PartialSignature {
                    session_id,
                    signed_psbt: psbt_bytes,
                    signed_key_ids: Vec::new(),
                },
            )),
        };
        let mut buf = Vec::with_capacity(reply.encoded_len());
        reply.encode(&mut buf)?;
        tls.write_all(&(buf.len() as u32).to_be_bytes()).await?;
        tls.write_all(&buf).await?;
        tls.flush().await?;
    }
    Ok(())
}

#[tokio::test]
async fn full_pair_then_sign_flow_via_offer_trust_path() {
    // ── Desktop side: mint identity, build offer, encode→decode.
    let identity = fresh_desktop_identity();
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
    let g = generate_offer(wallet_fp, &identity, "keychain-test".into());
    let encoded = encode_offer(&g.offer).expect("encode offer");
    let decoded = decode_offer(&encoded).expect("decode offer");

    // ── Phone side: validate cert/certFp agreement; pin desktop's
    // cert into the TLS trust slot.
    let trust_desktop_pin = extract_trusted_desktop_pin(&decoded);
    let phone_cert_for_server = phone_cert.clone();
    let phone_handle = tokio::spawn(fake_phone_pair_then_sign(
        listener,
        phone_cert_for_server,
        phone_key,
        phone_cert_fp_hex.clone(),
        trust_desktop_pin,
    ));

    // ── Phase 1: pair. Use the decoded offer to prove the wire
    // format round-trip is what authorised the dial.
    let phone_discovered = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };
    let identity_for_pair = DesktopIdentity {
        cert_der: identity.cert_der.clone(),
        key_der: identity.clone_key(),
        pubkey: identity.pubkey,
    };
    let paired = pairing_listener::run_pairing(
        identity_for_pair,
        decoded,
        phone_discovered,
        wallet_fp,
        vec![wallet_fp],
    )
    .await
    .expect("pairing ok");
    assert_eq!(paired.cert_pin, phone_pin);

    // ── Phase 2: steady-state pinned dial + sign_tx round-trip.
    let transport = PairedTransport::connect(addr, &identity, phone_pin)
        .await
        .expect("steady-state dial");
    let paired_clone = PairedPhone {
        cert_pin: paired.cert_pin,
        name: paired.name.clone(),
        paired_at_unix: paired.paired_at_unix,
        wallet_fingerprints: paired.wallet_fingerprints.clone(),
        fallback_addr: paired.fallback_addr.clone(),
    };
    let signer = PhoneSigner::new(transport, wallet_fp, None, paired_clone);

    let mut psbt = empty_psbt();
    let original = psbt.serialize();
    async_hwi::HWI::sign_tx(&signer, &mut psbt)
        .await
        .expect("sign_tx ok");
    assert_eq!(
        psbt.serialize(),
        original,
        "echo-back PSBT bytes should be byte-identical",
    );

    drop(signer);
    let _ = phone_handle.await;
}

#[tokio::test]
async fn handshake_fails_when_phone_pins_a_different_cert() {
    // Same harness, but the fake phone is pinned to a DIFFERENT
    // desktop cert. The desktop's TLS client cert won't match the
    // phone's trust pin, so the TLS handshake fails — proving the
    // trust path is doing real work, not just being ignored.
    let identity = fresh_desktop_identity();
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

    // Mint a *different* desktop cert and pin the phone to that.
    // The real desktop will present its own cert, which the phone
    // refuses.
    let (other_desk_cert, _other_desk_key) = mint_ed25519_cert("Some other desktop");
    let wrong_pin = tls::fingerprint_of(&other_desk_cert);

    let phone_handle = tokio::spawn(fake_phone_pair_then_sign(
        listener,
        phone_cert,
        phone_key,
        phone_cert_fp_hex.clone(),
        wrong_pin,
    ));

    let phone_discovered = DiscoveredPhone {
        cert_fp8: phone_cert_fp_hex[..8].to_string(),
        addr,
        instance_name: "keychain-test".into(),
    };
    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let g = generate_offer(wallet_fp, &identity, "keychain-test".into());

    let result = pairing_listener::run_pairing(
        identity,
        g.offer,
        phone_discovered,
        wallet_fp,
        vec![wallet_fp],
    )
    .await;

    assert!(
        result.is_err(),
        "pairing must fail when the phone pins the wrong desktop cert; got Ok({:?})",
        result.ok(),
    );

    // The fake phone task should also surface an error from
    // `accept(tcp)` — the TLS handshake fails server-side.
    let phone_outcome = phone_handle.await.expect("join");
    assert!(
        phone_outcome.is_err(),
        "fake phone should have surfaced a TLS handshake error",
    );
}
