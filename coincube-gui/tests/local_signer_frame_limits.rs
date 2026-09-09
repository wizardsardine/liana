//! LAN frame ceiling for authenticated (P2-A) requests.
//!
//! Since every input carries its complete previous transaction, a supported
//! spend funded by large batch-payout transactions produces a `PresentSession`
//! of about 1.1 MiB. This suite measures the exact production payload at each
//! boundary (raw PSBT, encrypted payload, encoded `LocalEnvelope`, Connect
//! `CreateSigningSessionRequest`), drives that request through the real
//! `PhoneSigner::sign_tx` framing path to a loopback phone, and pins the
//! deterministic sender-side preflight for a request that does not fit.

#[path = "common/lan_binding.rs"]
mod lan_binding;
#[path = "common/large_psbt.rs"]
mod large_psbt;

use std::net::Ipv4Addr;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use prost::Message as _;
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::ServerConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use coincube_gui::dir::NetworkDirectory;
use coincube_gui::phone_signer::{
    identity::DesktopIdentity,
    pairing_store::PairedPhone,
    protocol::{local_v1, present_session_envelope, LocalEnvelope},
    tls,
    transport::{PairedTransport, MAX_FRAME_BYTES},
    PhoneSigner,
};
use coincube_gui::services::connect::crypto::{seal_to_device, DeviceTransportKey};
use coincube_gui::services::connect::grpc::connect_v1 as cv1;

/// The paired phone's exact signer binding: one of the Taproot fixture's
/// three keys (the shared `lan_binding::binding` helper only handles
/// single-key descriptors).
fn binding_for_tr_desc() -> coincube_gui::phone_signer::pairing_store::SignerBinding {
    use coincube_core::{descriptors::CoincubeDescriptor, miniscript::DescriptorPublicKey};
    use sha2::{Digest, Sha256};
    use std::str::FromStr;
    let keys = CoincubeDescriptor::from_str(large_psbt::TR_DESC)
        .expect("descriptor")
        .spendable_keys();
    let key = keys
        .iter()
        .find_map(|k| match k {
            DescriptorPublicKey::XPub(x)
                if x.origin.as_ref().map(|o| o.0.to_string()).as_deref() == Some("ffd63c8d") =>
            {
                Some(x.clone())
            }
            _ => None,
        })
        .expect("ffd63c8d key");
    coincube_gui::phone_signer::pairing_store::SignerBinding {
        key_id: "10".into(),
        xpub: key.xkey.to_string(),
        fingerprint: key.origin.as_ref().expect("origin").0,
        descriptor_sha256: Sha256::digest(large_psbt::TR_DESC.as_bytes()).to_vec(),
    }
}

/// gRPC default maximum message size (tonic client and grpc-go server).
const GRPC_DEFAULT_MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

fn fresh_transport_key(tag: &str) -> DeviceTransportKey {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "coincube-frame-limits-{}-{}-{}",
        tag,
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("temp transport key dir");
    DeviceTransportKey::load_or_create(&NetworkDirectory::new(path)).expect("mint transport key")
}

fn mint_cert() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key_pair = KeyPair::generate_for(&PKCS_ED25519).expect("ed25519 keygen");
    let cert = CertificateParams::new(vec!["test.local".to_string()])
        .expect("params")
        .self_signed(&key_pair)
        .expect("self-sign");
    (
        cert.der().clone(),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der())),
    )
}

/// Exact production request sizes for one PSBT: (raw PSBT, ciphertext,
/// encoded LocalEnvelope, encoded Connect CreateSigningSessionRequest).
fn measure(
    psbt_bytes: &[u8],
    phone_pub: &[u8],
    desktop_pub: &[u8],
) -> (usize, usize, usize, usize) {
    let request_id = "measure-request";
    let psbt_env = seal_to_device(phone_pub, request_id, psbt_bytes).expect("seal psbt");
    let desc_env =
        seal_to_device(phone_pub, request_id, large_psbt::TR_DESC.as_bytes()).expect("seal desc");
    let to_proto = |s: coincube_gui::services::connect::crypto::transport::SealedPayload| {
        cv1::PayloadEnvelope {
            device_id: String::new(),
            ephemeral_pubkey: s.ephemeral_pubkey,
            nonce: s.nonce,
            ciphertext: s.ciphertext,
        }
    };
    let ciphertext_len = psbt_env.ciphertext.len();
    let target = cv1::SignerTarget {
        device_id: String::new(),
        key_fingerprint: "ffd63c8d".into(),
        key_id: "10".into(),
        transport_pubkey: phone_pub.to_vec(),
    };
    let psbt_env = to_proto(psbt_env);
    let desc_env = to_proto(desc_env);
    let connect = cv1::CreateSigningSessionRequest {
        request_id: request_id.into(),
        vault_id: "vault-00000000-0000-0000-0000-000000000000".into(),
        descriptor_id: "ffd63c8d".into(),
        psbt: Vec::new(),
        targets: vec![target.clone()],
        note: String::new(),
        ttl: Some(prost_types::Duration {
            seconds: 900,
            nanos: 0,
        }),
        require_user_presence: true,
        is_recovery_spend: false,
        payload_scheme: cv1::PayloadScheme::EciesV1 as i32,
        psbt_envelopes: vec![psbt_env.clone()],
        descriptor_envelopes: vec![desc_env.clone()],
    };
    let session = cv1::SigningSession {
        session_id: "11111111-2222-3333-4444-555555555555".into(),
        request_id: request_id.into(),
        user_id: String::new(),
        vault_id: String::new(),
        descriptor_id: "ffd63c8d".into(),
        psbt: Vec::new(),
        tx_summary: None,
        policy_summary: None,
        targets: vec![target],
        payload_scheme: cv1::PayloadScheme::EciesV1 as i32,
        psbt_envelopes: vec![psbt_env],
        descriptor_envelopes: vec![desc_env],
        creator_transport_pubkey: desktop_pub.to_vec(),
        status: cv1::SessionStatus::Pending as i32,
        created_at: None,
        expires_at: None,
        created_by_device_id: String::new(),
        note: String::new(),
        submitted_signatures: Vec::new(),
        is_recovery_spend: false,
    };
    let envelope = present_session_envelope(session);
    (
        psbt_bytes.len(),
        ciphertext_len,
        envelope.encoded_len(),
        connect.encoded_len(),
    )
}

/// The P2-A size matrix, now measured as complete production messages and
/// asserted against the LAN frame cap and the gRPC default. Run with
/// `--nocapture` for the table.
#[test]
fn authenticated_production_envelopes_fit_lan_and_connect_limits() {
    let phone = fresh_transport_key("measure-phone");
    let desktop = fresh_transport_key("measure-desktop");
    println!("inputs prev_outputs psbt ciphertext local_envelope connect_request");
    for (inputs, prev_outputs) in [(1, 2), (10, 2), (50, 2), (200, 2), (50, 100), (25, 1000)] {
        let psbt = large_psbt::authenticated_taproot_psbt(inputs, prev_outputs).serialize();
        let (raw, ct, env, connect) = measure(&psbt, &phone.public_key(), &desktop.public_key());
        println!("{inputs:>6} {prev_outputs:>12} {raw:>9} {ct:>10} {env:>14} {connect:>15}");
        assert_eq!(ct, raw + 16, "AES-GCM tag only");
        assert!(
            env <= MAX_FRAME_BYTES,
            "{} inputs: LocalEnvelope {} > {}",
            inputs,
            env,
            MAX_FRAME_BYTES
        );
        assert!(
            connect <= GRPC_DEFAULT_MAX_MESSAGE_BYTES,
            "{} inputs: Connect {} > {}",
            inputs,
            connect,
            GRPC_DEFAULT_MAX_MESSAGE_BYTES
        );
        if (inputs, prev_outputs) == (25, 1000) {
            // The case the 1 MiB cap refused: the raw PSBT alone exceeds it.
            assert!(raw > 1024 * 1024, "measured {}", raw);
        }
    }
}

/// Loopback phone: reads exactly one framed `PresentSession` (any declared
/// length, so the test observes what the desktop actually sent), decrypts the
/// PSBT, and echoes it back sealed to the desktop inside a `PartialSignature`
/// of the same order of size. Returns the decrypted PSBT length, or `None` if
/// the desktop closed without sending a frame.
async fn echo_phone(
    listener: TcpListener,
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    phone_transport: DeviceTransportKey,
) -> Option<usize> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("cert");
    let (tcp, _) = listener.accept().await.expect("accept");
    let mut tls = TlsAcceptor::from(Arc::new(cfg))
        .accept(tcp)
        .await
        .expect("tls");
    let mut len_buf = [0u8; 4];
    if tls.read_exact(&mut len_buf).await.is_err() {
        return None;
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    tls.read_exact(&mut payload).await.expect("body");
    let envelope = LocalEnvelope::decode(payload.as_slice()).expect("decode");
    let Some(local_v1::local_envelope::Payload::PresentSession(p)) = envelope.payload else {
        panic!("expected PresentSession");
    };
    let s = p.session.expect("session");
    let pe = s.psbt_envelopes.first().expect("psbt envelope");
    let psbt = phone_transport
        .open(
            &pe.ephemeral_pubkey,
            &pe.nonce,
            &pe.ciphertext,
            &s.request_id,
        )
        .expect("open");
    let sealed = seal_to_device(&s.creator_transport_pubkey, &s.request_id, &psbt).expect("seal");
    let reply = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::Partial(
            local_v1::PartialSignature {
                session_id: s.session_id,
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
    };
    let mut buf = Vec::with_capacity(reply.encoded_len());
    reply.encode(&mut buf).expect("encode");
    tls.write_all(&(buf.len() as u32).to_be_bytes())
        .await
        .expect("len");
    tls.write_all(&buf).await.expect("body");
    tls.flush().await.expect("flush");
    Some(psbt.len())
}

async fn signer_against_echo_phone() -> (PhoneSigner, tokio::task::JoinHandle<Option<usize>>) {
    let (desk_cert, desk_key) = mint_cert();
    let (phone_cert, phone_key) = mint_cert();
    let phone_pin = tls::fingerprint_of(&phone_cert);
    let desktop = DesktopIdentity {
        cert_der: desk_cert,
        key_der: desk_key,
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind");
    let phone_addr = listener.local_addr().expect("addr");
    let phone_transport = fresh_transport_key("phone");
    let phone_pubkey = phone_transport.public_key().to_vec();
    let handle = tokio::spawn(echo_phone(listener, phone_cert, phone_key, phone_transport));
    let transport = PairedTransport::connect(phone_addr, &desktop, phone_pin)
        .await
        .expect("dial");
    let paired = PairedPhone {
        signer_binding: Some(binding_for_tr_desc()),
        cert_pin: phone_pin,
        name: "Test phone".into(),
        paired_at_unix: 0,
        wallet_fingerprints: vec![Fingerprint::default()],
        vault_fingerprint: lan_binding::vault(large_psbt::TR_DESC),
        transport_pubkey: phone_pubkey,
        fallback_addr: None,
    };
    let signer = PhoneSigner::new(
        transport,
        Fingerprint::default(),
        None,
        paired,
        large_psbt::TR_DESC.to_string(),
        Some(Arc::new(fresh_transport_key("desktop"))),
    );
    (signer, handle)
}

/// The previously refused ~1.1 MiB authenticated request crosses the real
/// `sign_tx` framing path: the phone receives and decrypts the whole PSBT, and
/// its ~1.1 MiB reply crosses back. The echo carries no signature, so the
/// desktop's signature-delta check rejects it; that rejection proves the reply
/// frame was received and decoded, and is the only error allowed here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_request_above_the_old_cap_crosses_lan_framing() {
    let mut psbt = large_psbt::authenticated_taproot_psbt(25, 1000);
    let raw_len = psbt.serialize().len();
    assert!(raw_len > 1024 * 1024, "measured {}", raw_len);
    let (signer, phone) = signer_against_echo_phone().await;
    let err = async_hwi::HWI::sign_tx(&signer, &mut psbt)
        .await
        .expect_err("an unsigned echo is refused after transport");
    let text = format!("{}", err);
    assert!(
        text.contains("invalid or unauthorized signature delta"),
        "expected the post-transport signature check, got: {}",
        text
    );
    assert_eq!(
        phone.await.expect("phone"),
        Some(raw_len),
        "phone decrypted the full PSBT"
    );
}

/// A request that cannot fit any LAN frame fails deterministically before the
/// session is registered or a byte leaves, with actionable copy and no payload
/// material in the message. The phone sees the connection close without a frame.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_request_fails_preflight_with_actionable_copy() {
    // 100 inputs funded by 1,000-output transactions: ~4.4 MB of PSBT.
    let mut psbt = large_psbt::authenticated_taproot_psbt(100, 1000);
    assert!(psbt.serialize().len() > MAX_FRAME_BYTES);
    let (signer, phone) = signer_against_echo_phone().await;
    let err = async_hwi::HWI::sign_tx(&signer, &mut psbt)
        .await
        .expect_err("must fail preflight");
    let text = format!("{}", err);
    assert!(text.contains("too large for local signing"), "{}", text);
    assert!(text.contains("Coincube Connect"), "{}", text);
    assert!(text.contains("fewer inputs"), "{}", text);
    assert!(
        !text.contains("tpub") && !text.contains("psbt"),
        "no payload material: {}",
        text
    );
    drop(signer);
    assert_eq!(
        phone.await.expect("phone"),
        None,
        "no frame reached the phone"
    );
}
