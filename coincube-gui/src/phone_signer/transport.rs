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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tokio_rustls::TlsConnector;

use async_hwi::Error as HwiError;

use crate::phone_signer::identity::DesktopIdentity;
use crate::phone_signer::protocol::LocalEnvelope;
use crate::phone_signer::tls::{client_config, CertFingerprint};

/// Maximum envelope size accepted on the wire. Sized for a generous
/// PSBT round-trip; rejects framing-length runaways before we
/// allocate.
const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

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

        let connector = TlsConnector::from(Arc::new(cfg));
        let tcp = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(peer)).await {
            Ok(Ok(s)) => s,
            Ok(Err(_)) => return Err(HwiError::DeviceNotFound),
            Err(_) => return Err(HwiError::DeviceNotFound),
        };
        // SNI is required by rustls. Phones present a cert with SAN
        // "coincube-phone.local"; we pin by cert hash so the name
        // string itself is purely cosmetic.
        let sni: ServerName<'static> = ServerName::try_from("coincube-phone.local".to_string())
            .map_err(|e| HwiError::Device(format!("sni: {}", e)))?;
        let stream = connector
            .connect(sni, tcp)
            .await
            .map_err(|e| HwiError::Device(format!("tls handshake: {}", e)))?;
        Ok(Self { peer, stream })
    }

    /// Read one length-prefixed [`LocalEnvelope`] from the wire.
    pub async fn recv(&mut self) -> Result<LocalEnvelope, HwiError> {
        let mut len_buf = [0u8; 4];
        self.stream
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
        self.stream
            .read_exact(&mut payload)
            .await
            .map_err(|e| HwiError::Device(format!("read body: {}", e)))?;
        LocalEnvelope::decode(payload.as_slice())
            .map_err(|e| HwiError::Device(format!("decode envelope: {}", e)))
    }

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
        self.stream
            .write_all(&len)
            .await
            .map_err(|e| HwiError::Device(format!("write len: {}", e)))?;
        self.stream
            .write_all(&payload)
            .await
            .map_err(|e| HwiError::Device(format!("write body: {}", e)))?;
        self.stream
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
