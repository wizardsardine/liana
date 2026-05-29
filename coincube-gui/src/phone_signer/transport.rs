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

/// Maximum envelope size accepted on the wire. Locked to 1 MiB
/// across pairing and steady state per the cross-repo contract.
/// Rejects framing-length runaways before we allocate.
const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// How long we wait on a TCP+TLS connect to a paired phone before
/// declaring it unreachable. Kept tight so the discovery loop's 2s
/// tick isn't blocked.
pub const CONNECT_TIMEOUT: Duration = Duration::from_millis(750);

/// A live, authenticated transport to a paired phone.
pub struct PairedTransport {
    /// Remote endpoint we connected to. Useful for logs.
    pub peer: SocketAddr,

    stream: TlsStream<TcpStream>,
}

impl PairedTransport {
    /// Dial a paired phone over TLS, verifying the phone's cert pin.
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

        let stream = dial_tls(peer, cfg).await?;
        Ok(Self { peer, stream })
    }

    /// Dial during the pairing flow when the phone's cert pin isn't
    /// known yet. The fingerprint of the phone's actual cert is
    /// captured during the TLS handshake by
    /// [`tls::CapturingServerVerifier`]; read it via
    /// [`Self::peer_cert_fingerprint`] after this returns.
    pub async fn connect_unpinned(
        peer: SocketAddr,
        identity: &DesktopIdentity,
    ) -> Result<Self, HwiError> {
        let (cfg, _seen) = tls::client_config_unpinned(
            identity.cert_der.clone(),
            identity.clone_key(),
        )
        .map_err(|e| HwiError::Device(format!("rustls config: {}", e)))?;
        // We rely on `peer_cert_fingerprint()` post-connect instead
        // of the `seen` side channel — rustls populates
        // `peer_certificates()` on the connection itself.
        let stream = dial_tls(peer, cfg).await?;
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
) -> Result<TlsStream<TcpStream>, HwiError> {
    let connector = TlsConnector::from(Arc::new(cfg));
    let tcp = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(peer)).await {
        Ok(Ok(s)) => s,
        Ok(Err(_)) => return Err(HwiError::DeviceNotFound),
        Err(_) => return Err(HwiError::DeviceNotFound),
    };
    // SNI is required by rustls. The phone presents a cert with SAN
    // "coincube-phone.local"; pinning by cert hash makes the name
    // string itself purely cosmetic.
    let sni: ServerName<'static> = ServerName::try_from("coincube-phone.local".to_string())
        .map_err(|e| HwiError::Device(format!("sni: {}", e)))?;
    connector
        .connect(sni, tcp)
        .await
        .map_err(|e| HwiError::Device(format!("tls handshake: {}", e)))
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
        if payload.len() > MAX_FRAME_BYTES {
            return Err(HwiError::Device(format!(
                "frame too large to send: {} > {}",
                payload.len(),
                MAX_FRAME_BYTES
            )));
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
