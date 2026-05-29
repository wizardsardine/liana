//! One-shot TLS listener for the pairing window.
//!
//! Lifecycle:
//!   1. Bind a free ephemeral TCP port.
//!   2. Advertise the offer's `service_name` over mDNS so the phone
//!      can resolve which port to dial.
//!   3. Accept the first inbound TCP connection.
//!   4. Run a TLS handshake using `pairing_server_config`, which
//!      accepts any client cert at this stage (we don't yet know the
//!      phone's pin).
//!   5. Read a single length-prefixed `LocalEnvelope`, expecting
//!      `PairingComplete`. Snapshot the phone's end-entity cert
//!      hash (it's the "pin" for future reconnects).
//!   6. Validate that the phone's reported wallet fingerprint matches
//!      one of `wallet_fingerprints`.
//!   7. Persist a `PairedPhone` row.
//!   8. Drop the mDNS advertisement and the TLS listener.
//!
//! Errors propagate as `String` so the settings UI can surface them
//! verbatim in the pairing wizard's status line.

use std::sync::Arc;
use std::time::Duration;

use prost::Message as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

use crate::dir::CoincubeDirectory;
use crate::phone_signer::errors::PairingError;
use crate::phone_signer::identity::DesktopIdentity;
use crate::phone_signer::mdns;
use crate::phone_signer::pairing::PairingOffer;
use crate::phone_signer::pairing_store::{self, PairedPhone};
use crate::phone_signer::protocol::{local_v1, LocalEnvelope};
use crate::phone_signer::tls;

const PAIRING_FRAME_LIMIT: usize = 64 * 1024;

/// Run the pairing flow to completion. Returns the persisted
/// [`PairedPhone`] on success, or a typed [`PairingError`] keyed off
/// the failure category so the wizard can render specific copy.
pub async fn run(
    identity: DesktopIdentity,
    offer: PairingOffer,
    psk: [u8; 32],
    wallet_fingerprints: Vec<Fingerprint>,
    dir: CoincubeDirectory,
) -> Result<PairedPhone, PairingError> {
    let listener = TcpListener::bind("0.0.0.0:0")
        .await
        .map_err(|e| PairingError::NetworkError(format!("bind: {}", e)))?;
    let port = listener
        .local_addr()
        .map_err(|e| PairingError::NetworkError(format!("local_addr: {}", e)))?
        .port();

    // mDNS advertisement scoped to this function; dropped on return.
    let _adv = mdns::advertise_pairing_target(&offer.service_name, port, &identity.fingerprint_hex8())
        .map_err(|e| PairingError::NetworkError(format!("mdns advertise: {}", e)))?;

    run_with_listener(identity, offer, psk, wallet_fingerprints, dir, listener).await
}

/// Pairing flow against a caller-supplied listener, skipping the
/// mDNS register step. The production [`run`] binds `0.0.0.0:0` and
/// advertises before calling this; tests bind a loopback listener
/// and have the fake phone dial it directly. Doing the split keeps
/// the network plumbing isolated from the protocol logic the tests
/// want to cover.
pub async fn run_with_listener(
    identity: DesktopIdentity,
    offer: PairingOffer,
    _psk: [u8; 32],
    wallet_fingerprints: Vec<Fingerprint>,
    dir: CoincubeDirectory,
    listener: TcpListener,
) -> Result<PairedPhone, PairingError> {
    let server_cfg = tls::pairing_server_config(identity.cert_der.clone(), identity.clone_key())
        .map_err(|e| PairingError::InternalError(format!("rustls config: {}", e)))?;
    let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

    // Deadline derived from the offer expiry; accept the first
    // inbound connection within that window.
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let remaining = offer.expires_at_unix.saturating_sub(now_unix);
    if remaining == 0 {
        return Err(PairingError::OfferExpired);
    }
    let (tcp, _peer) = tokio::time::timeout(Duration::from_secs(remaining), listener.accept())
        .await
        .map_err(|_| PairingError::OfferExpired)?
        .map_err(|e| PairingError::NetworkError(format!("accept: {}", e)))?;

    let tls_stream = acceptor
        .accept(tcp)
        .await
        .map_err(|e| PairingError::NetworkError(format!("tls handshake: {}", e)))?;

    // Snapshot the phone's end-entity cert from the just-completed
    // handshake — this is the pin we'll use on every reconnect.
    let (_, server_conn) = tls_stream.get_ref();
    let phone_cert = server_conn
        .peer_certificates()
        .and_then(|chain| chain.first().cloned())
        .ok_or_else(|| PairingError::NetworkError("phone presented no cert".into()))?;
    let phone_pin = tls::fingerprint_of(&phone_cert);

    // Single envelope expected: PairingComplete.
    let mut stream = tls_stream;
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| PairingError::NetworkError(format!("read len: {}", e)))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > PAIRING_FRAME_LIMIT {
        return Err(PairingError::NetworkError(format!(
            "pairing frame too large: {}",
            len
        )));
    }
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .await
        .map_err(|e| PairingError::NetworkError(format!("read body: {}", e)))?;
    let envelope = LocalEnvelope::decode(payload.as_slice())
        .map_err(|e| PairingError::NetworkError(format!("decode envelope: {}", e)))?;
    let complete = match envelope.payload {
        Some(local_v1::local_envelope::Payload::PairingComplete(c)) => c,
        _ => {
            return Err(PairingError::InternalError(
                "expected PairingComplete envelope".into(),
            ))
        }
    };

    // The phone's claim of which wallet it can sign for. v1 has no
    // dedicated field for this (proto only carries identity_pubkey,
    // device_name, app_version, capabilities), so today the check
    // degenerates to "offer.wallet_fingerprint is in
    // wallet_fingerprints" which is tautological. Surface a typed
    // mismatch anyway so the variant is ready when the proto grows
    // a wallet_fingerprint field on PairingComplete.
    let claimed_fp = offer.wallet_fingerprint;
    if !wallet_fingerprints.contains(&claimed_fp) {
        return Err(PairingError::WalletFingerprintMismatch {
            expected: wallet_fingerprints.clone(),
            claimed: claimed_fp,
        });
    }

    // Send an empty ack so the phone knows pairing succeeded
    // server-side. Best-effort; we still persist regardless.
    let ack = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::Pong(
            crate::services::connect::grpc::connect_v1::Pong { ts_unix_ms: 0 },
        )),
    };
    let mut ack_buf = Vec::with_capacity(ack.encoded_len());
    let _ = ack.encode(&mut ack_buf);
    let _ = stream
        .write_all(&(ack_buf.len() as u32).to_be_bytes())
        .await;
    let _ = stream.write_all(&ack_buf).await;
    let _ = stream.flush().await;

    let name = if complete.device_name.is_empty() {
        "Keychain phone".to_string()
    } else {
        complete.device_name
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let paired = PairedPhone {
        identity_pubkey: phone_pin,
        name,
        paired_at_unix: now,
        wallet_fingerprints,
        fallback_addr: None,
    };

    pairing_store::upsert(&dir, paired.clone())
        .map_err(|e| PairingError::InternalError(format!("persist: {}", e)))?;

    Ok(paired)
}
