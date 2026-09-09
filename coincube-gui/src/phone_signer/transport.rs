//! Length-prefixed framing for the LAN signer protocol, wrapped in
//! TLS 1.3 with [`tls::PinnedVerifier`].
//!
//! Wire format on top of TLS:
//!
//! ```text
//! [4-byte big-endian length][protobuf LocalEnvelope bytes]
//! ```

use std::convert::TryFrom;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use prost::Message;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use async_hwi::Error as HwiError;

use crate::phone_signer::identity::DesktopIdentity;
use crate::phone_signer::protocol::LocalEnvelope;
use crate::phone_signer::tls::{self, client_config, CertFingerprint};

/// Maximum envelope size accepted on the wire, in either direction, across
/// pairing and steady state.
///
/// Shared cross-repo contract with Keychain's `maxFrameBytes`
/// (`lib/services/local_signer/local_envelope_framing.dart`): the two values
/// must stay identical or one side's legitimate request is the other side's
/// "hostile frame".
///
/// 4 MiB. The largest legitimate frame is a `PresentSession` carrying an
/// authenticated PSBT: since P2-A every input includes its complete previous
/// transaction, so a supported spend funded by large batch-payout transactions
/// runs to about 1.1 MiB (25 inputs funded by 1,000-output transactions:
/// 1,099,027 PSBT bytes, 1,100,096 bytes as the encrypted `LocalEnvelope`).
/// The earlier 1 MiB cap refused that request. 4 MiB matches the gRPC default
/// message limit on the Connect rail, so a request that fits Connect fits LAN.
///
/// Still a hard, finite bound: the 4-byte length prefix is validated against
/// it before any body allocation, so a hostile or corrupt length costs nothing
/// beyond the header, and the sender refuses to emit one. Frames only arrive
/// over a mutually authenticated, certificate-pinned TLS session.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Actionable user copy for a request that does not fit a LAN frame. Reports
/// sizes only; never payload, descriptor, key or session material.
pub fn frame_too_large_message(bytes: usize) -> String {
    format!(
        "This transaction is too large for local signing ({} bytes; the local limit is {} bytes). \
         Use Coincube Connect or recreate it with fewer inputs.",
        bytes, MAX_FRAME_BYTES
    )
}

/// Deterministic sender-side preflight: the exact encoded size of `envelope`
/// must fit [`MAX_FRAME_BYTES`]. Returns that size, or the actionable
/// [`frame_too_large_message`] as a device error. Run it before a session is
/// registered or any byte leaves, so an unsupported request never becomes a
/// partially started signing flow cut off by the peer's cap.
pub fn preflight_envelope(envelope: &LocalEnvelope) -> Result<usize, HwiError> {
    let len = envelope.encoded_len();
    if len > MAX_FRAME_BYTES {
        return Err(HwiError::Device(frame_too_large_message(len)));
    }
    Ok(len)
}

/// How long we wait on a TCP+TLS connect during the steady-state
/// per-tick dial. Kept tight so the discovery loop's 2s tick isn't
/// blocked when a paired phone is offline.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(750);

/// How long we wait on a TCP+TLS connect during the user-initiated
/// pairing dial. Much looser than [`CONNECT_TIMEOUT`] because:
///
///   * Pairing is one-shot, not per-tick, so we're not blocking any
///     background loop.
///   * The desktop's first dial to a phone is a cold path: ARP
///     resolution, Wi-Fi power-save wake-up, and TCP SYN retries
///     can each chew hundreds of ms on a marginal LAN. A 750ms
///     budget reliably fails on Wi-Fi that ping shows working but
///     lossy (~25% loss / ~200ms RTT), because a dropped SYN's
///     retry lands well after the deadline.
///   * The retry loop in `pairing_listener::run_pairing` already
///     caps total wall time at the offer TTL, so a longer per-dial
///     budget just shifts where the time is spent — fewer dials,
///     each more likely to succeed.
pub const PAIRING_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A live, authenticated transport to a paired phone.
pub struct PairedTransport {
    /// Remote endpoint we connected to. Useful for logs.
    pub peer: SocketAddr,

    stream: TlsStream<TcpStream>,
}

impl PairedTransport {
    /// Dial a paired phone over TLS, verifying the phone's cert pin.
    /// Steady-state path — uses the tight [`CONNECT_TIMEOUT`] so a
    /// dead phone doesn't stall the 2s discovery tick.
    pub async fn connect(
        peer: SocketAddr,
        identity: &DesktopIdentity,
        phone_cert_pin: CertFingerprint,
    ) -> Result<Self, HwiError> {
        let cfg = client_config(
            identity.cert_der.clone(),
            identity.clone_key(),
            phone_cert_pin,
        )
        .map_err(|e| HwiError::Device(format!("rustls config: {}", e)))?;

        let stream = dial_tls(peer, cfg, CONNECT_TIMEOUT).await?;
        Ok(Self { peer, stream })
    }

    /// Dial during the pairing flow when the phone's cert pin isn't
    /// known yet. The fingerprint of the phone's actual cert is
    /// captured during the TLS handshake by
    /// [`tls::CapturingServerVerifier`]; read it via
    /// [`Self::peer_cert_fingerprint`] after this returns.
    ///
    /// Uses the looser [`PAIRING_CONNECT_TIMEOUT`] because the
    /// pairing dial is a one-shot, user-initiated event over a
    /// likely-cold network path — the tight steady-state budget
    /// reliably fails on lossy Wi-Fi.
    pub async fn connect_unpinned(
        peer: SocketAddr,
        identity: &DesktopIdentity,
    ) -> Result<Self, HwiError> {
        let (cfg, _seen) =
            tls::client_config_unpinned(identity.cert_der.clone(), identity.clone_key())
                .map_err(|e| HwiError::Device(format!("rustls config: {}", e)))?;
        // We rely on `peer_cert_fingerprint()` post-connect instead
        // of the `seen` side channel — rustls populates
        // `peer_certificates()` on the connection itself.
        let stream = dial_tls(peer, cfg, PAIRING_CONNECT_TIMEOUT).await?;
        Ok(Self { peer, stream })
    }

    /// SHA-256 of the end-entity cert the peer presented during the
    /// TLS handshake. `None` if no cert was presented (shouldn't
    /// happen for our protocol — phone always presents one). Used by
    /// the pairing flow to pin the phone's cert after connection.
    pub fn peer_cert_fingerprint(&self) -> Option<CertFingerprint> {
        let (_, conn) = self.stream.get_ref();
        let cert = conn.peer_certificates()?.first()?;
        Some(tls::fingerprint_of(cert))
    }

    /// Split into independently-owned read and write halves.
    ///
    /// Sharing a single `Mutex<PairedTransport>` between the reader
    /// task and `sign_tx` deadlocks: the reader parks on
    /// `recv().await` while holding the lock, so the writer can never
    /// send `PresentSession` — and the phone never sends anything
    /// back. Splitting hands each task its own half, so reads and
    /// writes proceed concurrently.
    pub fn split(self) -> (PairedReader, PairedWriter) {
        let (read, write) = tokio::io::split(self.stream);
        (
            PairedReader { read },
            PairedWriter {
                peer: self.peer,
                write,
            },
        )
    }
}

/// Shared TCP-connect + TLS-handshake plumbing for both pinned and
/// unpinned dials. The only thing that differs between them is the
/// rustls `ClientConfig` we hand to the connector.
async fn dial_tls(
    peer: SocketAddr,
    cfg: rustls::ClientConfig,
    budget: Duration,
) -> Result<TlsStream<TcpStream>, HwiError> {
    let connector = TlsConnector::from(Arc::new(cfg));
    let tcp = match tokio::time::timeout(budget, TcpStream::connect(peer)).await {
        Ok(Ok(s)) => s,
        // Surface the underlying os error ("network is unreachable",
        // "connection refused", etc.) instead of collapsing every
        // TCP failure to a generic `DeviceNotFound`. The old
        // behaviour hid the real cause from the pairing wizard's
        // error toast and made remote bugs nearly impossible to
        // diagnose from a screenshot.
        Ok(Err(e)) => {
            return Err(HwiError::Device(format!("tcp connect {}: {}", peer, e)));
        }
        Err(_) => {
            return Err(HwiError::Device(format!(
                "tcp connect {} timed out after {:?}",
                peer, budget
            )));
        }
    };
    // SNI is required by rustls. The phone presents a cert with SAN
    // "coincube-phone.local"; pinning by cert hash makes the name
    // string itself purely cosmetic.
    let sni: ServerName<'static> = ServerName::try_from("coincube-phone.local".to_string())
        .map_err(|e| HwiError::Device(format!("sni: {}", e)))?;
    // Bound the TLS handshake on the same budget as the TCP connect.
    // A phone (or attacker) that accepts the TCP socket but stalls
    // the handshake would otherwise hang this future indefinitely —
    // blocking the discovery-loop dial's per-phone future forever
    // and preventing the cooldown from being recorded.
    match tokio::time::timeout(budget, connector.connect(sni, tcp)).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(HwiError::Device(format!("tls handshake: {}", e))),
        Err(_) => Err(HwiError::Device("tls handshake timeout".into())),
    }
}

/// Owned read half. The reader task owns one of these directly, so
/// no shared lock is needed.
pub struct PairedReader {
    read: ReadHalf<TlsStream<TcpStream>>,
}

impl PairedReader {
    /// Read one length-prefixed [`LocalEnvelope`] from the wire.
    pub async fn recv(&mut self) -> Result<LocalEnvelope, HwiError> {
        let mut len_buf = [0u8; 4];
        self.read
            .read_exact(&mut len_buf)
            .await
            .map_err(|e| HwiError::Device(format!("read len: {}", e)))?;
        let len = u32::from_be_bytes(len_buf) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(HwiError::Device(format!(
                "frame too large: {} > {}",
                len, MAX_FRAME_BYTES
            )));
        }
        let mut payload = vec![0u8; len];
        self.read
            .read_exact(&mut payload)
            .await
            .map_err(|e| HwiError::Device(format!("read body: {}", e)))?;
        LocalEnvelope::decode(payload.as_slice())
            .map_err(|e| HwiError::Device(format!("decode envelope: {}", e)))
    }
}

/// Owned write half. Wrapped in `Arc<Mutex<_>>` by the caller so
/// concurrent `sign_tx` invocations serialise their writes — but
/// never block the reader.
pub struct PairedWriter {
    /// Remote endpoint we connected to. Useful for logs.
    pub peer: SocketAddr,

    write: WriteHalf<TlsStream<TcpStream>>,
}

impl PairedWriter {
    /// Send one length-prefixed [`LocalEnvelope`] over the wire.
    pub async fn send(&mut self, envelope: &LocalEnvelope) -> Result<(), HwiError> {
        let mut payload = Vec::with_capacity(envelope.encoded_len());
        envelope
            .encode(&mut payload)
            .map_err(|e| HwiError::Device(format!("encode envelope: {}", e)))?;
        // Same bound as the receiver, checked before any byte is written.
        if payload.len() > MAX_FRAME_BYTES {
            return Err(HwiError::Device(frame_too_large_message(payload.len())));
        }
        let len = (payload.len() as u32).to_be_bytes();
        self.write
            .write_all(&len)
            .await
            .map_err(|e| HwiError::Device(format!("write len: {}", e)))?;
        self.write
            .write_all(&payload)
            .await
            .map_err(|e| HwiError::Device(format!("write body: {}", e)))?;
        self.write
            .flush()
            .await
            .map_err(|e| HwiError::Device(format!("flush: {}", e)))?;
        Ok(())
    }
}

impl std::fmt::Debug for PairedTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairedTransport")
            .field("peer", &self.peer)
            .finish()
    }
}

impl std::fmt::Debug for PairedWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairedWriter")
            .field("peer", &self.peer)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    //! Exact framing boundaries on the desktop side, over a real loopback TLS
    //! session. The cap is checked against the 4-byte length prefix before any
    //! body is allocated, in both directions.
    use super::*;
    use crate::phone_signer::protocol::local_v1;
    use rcgen::{CertificateParams, KeyPair, PKCS_ECDSA_P256_SHA256};
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    use std::net::Ipv4Addr;
    use tokio::net::TcpListener;
    use tokio_rustls::server::TlsStream as ServerTls;
    use tokio_rustls::TlsAcceptor;

    fn mint() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
        let kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("keygen");
        let cert = CertificateParams::new(vec!["coincube-phone.local".to_string()])
            .expect("params")
            .self_signed(&kp)
            .expect("self-sign");
        (
            cert.der().clone(),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(kp.serialize_der())),
        )
    }

    /// Desktop transport dialled into a loopback "phone" TLS server; returns
    /// both ends so a test can drive raw bytes on the phone side.
    async fn loopback() -> (PairedTransport, ServerTls<TcpStream>) {
        let (phone_cert, phone_key) = mint();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let cfg = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_no_client_auth()
            .with_single_cert(vec![phone_cert], phone_key)
            .expect("server cert");
        let acceptor = TlsAcceptor::from(Arc::new(cfg));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let accept = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.expect("accept");
            acceptor.accept(tcp).await.expect("tls accept")
        });
        let (desk_cert, desk_key) = mint();
        let (cfg, _seen) = tls::client_config_unpinned(desk_cert, desk_key).expect("client cfg");
        let stream = dial_tls(addr, cfg, Duration::from_secs(5))
            .await
            .expect("dial");
        (
            PairedTransport { peer: addr, stream },
            accept.await.expect("join"),
        )
    }

    /// An envelope whose exact encoded length is `target`, padded through an
    /// error message (lengths are varint-prefixed, so search for the fit).
    fn envelope_of_len(target: usize) -> LocalEnvelope {
        let mut n = target.saturating_sub(16);
        loop {
            let env = LocalEnvelope {
                payload: Some(local_v1::local_envelope::Payload::Error(
                    local_v1::ErrorEnvelope {
                        code: String::new(),
                        message: "x".repeat(n),
                        session_id: String::new(),
                    },
                )),
            };
            let len = env.encoded_len();
            if len == target {
                return env;
            }
            assert!(len < target, "overshot {} > {}", len, target);
            n += target - len;
        }
    }

    async fn read_raw_frame(tls: &mut ServerTls<TcpStream>) -> Vec<u8> {
        let mut len_buf = [0u8; 4];
        tls.read_exact(&mut len_buf).await.expect("len");
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut payload = vec![0u8; len];
        tls.read_exact(&mut payload).await.expect("body");
        payload
    }

    async fn write_raw_frame(tls: &mut ServerTls<TcpStream>, payload: &[u8]) {
        tls.write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .expect("len");
        tls.write_all(payload).await.expect("body");
        tls.flush().await.expect("flush");
    }

    #[tokio::test]
    async fn frames_at_cap_minus_one_and_at_cap_cross_both_directions() {
        for target in [MAX_FRAME_BYTES - 1, MAX_FRAME_BYTES] {
            let (transport, mut phone) = loopback().await;
            let (mut reader, mut writer) = transport.split();
            let env = envelope_of_len(target);
            assert_eq!(env.encoded_len(), target);

            // Desktop -> phone. Both ends run concurrently: a frame this size
            // fills the socket buffers long before the sender finishes.
            let (sent, raw) = tokio::join!(writer.send(&env), read_raw_frame(&mut phone));
            sent.expect("send at cap");
            assert_eq!(raw.len(), target);
            assert_eq!(LocalEnvelope::decode(raw.as_slice()).expect("decode"), env);

            // Phone -> desktop.
            let ((), got) = tokio::join!(write_raw_frame(&mut phone, &raw), reader.recv());
            assert_eq!(got.expect("recv at cap"), env);
        }
    }

    #[tokio::test]
    async fn frame_at_cap_plus_one_is_refused_by_the_sender_before_any_bytes_leave() {
        let (transport, mut phone) = loopback().await;
        let (_reader, mut writer) = transport.split();
        let env = envelope_of_len(MAX_FRAME_BYTES + 1);
        let err = writer.send(&env).await.expect_err("must refuse");
        let text = format!("{}", err);
        assert!(text.contains("too large"), "{}", text);
        assert!(text.contains(&MAX_FRAME_BYTES.to_string()), "{}", text);
        assert!(
            !text.contains("xxxx"),
            "payload must not leak into the error"
        );

        // Nothing was written: the next, small frame is the first the phone sees.
        let small = envelope_of_len(64);
        writer.send(&small).await.expect("small send");
        let raw = read_raw_frame(&mut phone).await;
        assert_eq!(
            LocalEnvelope::decode(raw.as_slice()).expect("decode"),
            small
        );
    }

    #[tokio::test]
    async fn oversized_declared_length_is_rejected_before_the_body_is_read() {
        let (transport, mut phone) = loopback().await;
        let (mut reader, _writer) = transport.split();
        // Header only: cap + 1 declared, no body ever sent. If the receiver
        // allocated or waited for the body this would hang, not error.
        phone
            .write_all(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes())
            .await
            .expect("header");
        phone.flush().await.expect("flush");
        let err = tokio::time::timeout(Duration::from_secs(5), reader.recv())
            .await
            .expect("rejected without waiting for a body")
            .expect_err("must refuse");
        assert!(format!("{}", err).contains("frame too large"), "{}", err);
    }

    #[tokio::test]
    async fn fragmented_frame_at_cap_is_reassembled() {
        let (transport, mut phone) = loopback().await;
        let (mut reader, _writer) = transport.split();
        let env = envelope_of_len(MAX_FRAME_BYTES);
        let mut payload = Vec::with_capacity(env.encoded_len());
        env.encode(&mut payload).expect("encode");
        let phone_task = tokio::spawn(async move {
            phone
                .write_all(&(payload.len() as u32).to_be_bytes())
                .await
                .expect("len");
            for chunk in payload.chunks(4096) {
                phone.write_all(chunk).await.expect("chunk");
                phone.flush().await.expect("flush");
            }
            phone
        });
        let got = reader.recv().await.expect("reassembled");
        assert_eq!(got, env);
        phone_task.await.expect("phone task");
    }
}
