//! Integration tests for [`pairing_listener::run_with_listener`].
//!
//! The same loopback-TLS pattern as `local_signer_loopback.rs`, but
//! exercising the *other* direction: a fake phone CLIENT dialing
//! the desktop's pairing listener and sending `PairingComplete`.
//!
//! Three scenarios:
//!   1. Happy path — PairingComplete arrives within the offer
//!      window, listener returns `Ok(PairedPhone)` with the cert
//!      pin populated, and the store on disk reflects the entry.
//!   2. Offer expired — kick off the listener with an offer whose
//!      `expires_at_unix` is already in the past; listener returns
//!      `Err(OfferExpired)` immediately, no connect required.
//!   3. Wallet fingerprint mismatch — fake phone connects and sends
//!      `PairingComplete`, but the desktop's `wallet_fingerprints`
//!      doesn't include the offer's fingerprint; listener returns
//!      `Err(WalletFingerprintMismatch{..})`.

use std::convert::TryFrom;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use prost::Message as _;
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;

use coincube_gui::dir::CoincubeDirectory;
use coincube_gui::phone_signer::{
    errors::PairingError,
    identity::DesktopIdentity,
    pairing::{PairingOffer, PAIRING_PROTOCOL_VERSION},
    pairing_listener,
    pairing_store,
    protocol::{local_v1, LocalEnvelope},
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
    path.push(format!("coincube-pairing-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).expect("mkdir tempdir");
    CoincubeDirectory::new(path)
}

/// Accept-any-cert verifier used by the fake phone — pinning the
/// desktop's cert in the test would duplicate production logic
/// without adding coverage, and the listener's behaviour against a
/// non-pinning client is the part we care about.
#[derive(Debug)]
struct AcceptAnyServer;

impl ServerCertVerifier for AcceptAnyServer {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ED25519,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

fn fake_phone_client_config(
    phone_cert: CertificateDer<'static>,
    phone_key: PrivateKeyDer<'static>,
) -> ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServer))
        .with_client_auth_cert(vec![phone_cert], phone_key)
        .expect("with_client_auth_cert")
}

async fn send_pairing_complete(
    stream: &mut tokio_rustls::client::TlsStream<TcpStream>,
    device_name: &str,
) {
    let envelope = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::PairingComplete(
            local_v1::PairingComplete {
                // 32 bytes — exact contents don't matter for the
                // listener: it pins the phone's *cert* hash, not
                // this proto-level pubkey field, in v1.
                phone_identity_pubkey: vec![0u8; 32],
                device_name: device_name.to_string(),
                app_version: "test-1.0".to_string(),
                capabilities: vec!["sign-psbt".to_string()],
            },
        )),
    };
    let mut buf = Vec::with_capacity(envelope.encoded_len());
    envelope.encode(&mut buf).expect("encode");
    stream
        .write_all(&(buf.len() as u32).to_be_bytes())
        .await
        .expect("write len");
    stream.write_all(&buf).await.expect("write body");
    stream.flush().await.expect("flush");
}

fn fresh_offer(wallet_fp: Fingerprint, ttl_secs: u64) -> PairingOffer {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + ttl_secs;
    PairingOffer {
        version: PAIRING_PROTOCOL_VERSION,
        psk_b64: "AAA".into(),
        session_pubkey_b64: "BBB".into(),
        service_name: format!("coincube-test-{}", uuid::Uuid::new_v4().simple()),
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

#[tokio::test]
async fn pairing_listener_happy_path_persists_paired_phone() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));

    let wallet_fp = Fingerprint::from([1, 2, 3, 4]);
    let offer = fresh_offer(wallet_fp, 30);
    let identity = fresh_desktop_identity();
    let dir = fresh_dir();

    // Spawn the desktop pairing listener.
    let dir_for_listener = dir.clone();
    let listener_task = tokio::spawn(async move {
        pairing_listener::run_with_listener(
            identity,
            offer,
            [0u8; 32],
            vec![wallet_fp],
            dir_for_listener,
            listener,
        )
        .await
    });

    // Spawn the fake phone client. Give the listener a beat to
    // configure its acceptor.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let client_cfg = fake_phone_client_config(phone_cert, phone_key);
    let connector = TlsConnector::from(Arc::new(client_cfg));
    let tcp = TcpStream::connect(target).await.expect("tcp");
    let sni: ServerName<'static> =
        ServerName::try_from("coincube-desktop.local".to_string()).expect("sni");
    let mut tls = connector.connect(sni, tcp).await.expect("tls handshake");
    send_pairing_complete(&mut tls, "Test Pixel").await;

    // Drain the ack envelope (Pong wrapped in LocalEnvelope) so the
    // listener's flush returns before we drop the socket.
    let mut len_buf = [0u8; 4];
    tls.read_exact(&mut len_buf).await.expect("ack len");
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    tls.read_exact(&mut payload).await.expect("ack body");

    let paired = listener_task.await.expect("join").expect("listener ok");
    assert_eq!(paired.name, "Test Pixel");
    assert_eq!(paired.wallet_fingerprints, vec![wallet_fp]);

    // The store on disk must reflect the entry.
    let on_disk = pairing_store::load(&dir).expect("load store");
    assert_eq!(on_disk.phones.len(), 1);
    assert_eq!(on_disk.phones[0].name, "Test Pixel");
    assert_eq!(on_disk.phones[0].identity_pubkey, paired.identity_pubkey);
}

#[tokio::test]
async fn pairing_listener_returns_offer_expired_when_ttl_in_past() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    // Offer expired 10 seconds ago.
    let mut offer = fresh_offer(Fingerprint::default(), 10);
    offer.expires_at_unix = 1; // far in the past
    let identity = fresh_desktop_identity();
    let dir = fresh_dir();

    let result = pairing_listener::run_with_listener(
        identity,
        offer,
        [0u8; 32],
        vec![Fingerprint::default()],
        dir,
        listener,
    )
    .await;
    assert!(matches!(result, Err(PairingError::OfferExpired)));
}

#[tokio::test]
async fn pairing_listener_returns_wallet_fingerprint_mismatch() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let target = SocketAddr::from((Ipv4Addr::LOCALHOST, port));

    // Offer is for `wanted`, but we tell the listener the wallet
    // only contains `actual`. The listener's post-handshake check
    // should reject.
    let wanted = Fingerprint::from([9, 9, 9, 9]);
    let actual = Fingerprint::from([1, 2, 3, 4]);
    let offer = fresh_offer(wanted, 30);
    let identity = fresh_desktop_identity();
    let dir = fresh_dir();

    let listener_task = tokio::spawn(async move {
        pairing_listener::run_with_listener(
            identity,
            offer,
            [0u8; 32],
            vec![actual],
            dir,
            listener,
        )
        .await
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let (phone_cert, phone_key) = mint_ed25519_cert("Coincube Phone (test)");
    let client_cfg = fake_phone_client_config(phone_cert, phone_key);
    let connector = TlsConnector::from(Arc::new(client_cfg));
    let tcp = TcpStream::connect(target).await.expect("tcp");
    let sni: ServerName<'static> =
        ServerName::try_from("coincube-desktop.local".to_string()).expect("sni");
    let mut tls = connector.connect(sni, tcp).await.expect("tls handshake");
    send_pairing_complete(&mut tls, "Wrong-wallet phone").await;

    let result = listener_task.await.expect("join");
    match result {
        Err(PairingError::WalletFingerprintMismatch { expected, claimed }) => {
            assert_eq!(expected, vec![actual]);
            assert_eq!(claimed, wanted);
        }
        other => panic!("expected WalletFingerprintMismatch, got {:?}", other),
    }
}
