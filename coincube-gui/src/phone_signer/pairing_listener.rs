//! Pairing dial — the desktop is now the TLS **client** during
//! pairing, matching the steady-state direction.
//!
//! Lifecycle:
//!   1. Caller has already browsed mDNS and picked a phone.
//!   2. Dial the phone's `SocketAddr` over TLS, with an
//!      accept-any-cert verifier that captures the cert hash so we
//!      can pin it post-handshake.
//!   3. Read one length-prefixed `LocalEnvelope` expecting
//!      `PairingComplete`.
//!   4. Validate the wallet fingerprint claim against the local
//!      wallet's keys.
//!   5. Persist a `PairedPhone` row.
//!   6. Drop the connection — the next 2s discovery tick redials via
//!      the steady-state path.
//!
//! See `plans/PLAN-local-signer-lan-interop-fixes-desktop.md` §1.4.

use prost::Message as _;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

use crate::dir::CoincubeDirectory;
use crate::phone_signer::errors::PairingError;
use crate::phone_signer::identity::DesktopIdentity;
use crate::phone_signer::mdns;
use crate::phone_signer::pairing::PairingOffer;
use crate::phone_signer::pairing_store::{self, PairedPhone};
use crate::phone_signer::protocol::{local_v1, LocalEnvelope};
use crate::phone_signer::transport::PairedTransport;

/// Dial the phone selected during the picker step, read its
/// `PairingComplete`, validate, persist. Returns the persisted
/// [`PairedPhone`] on success.
///
/// The caller is responsible for confirming `offer.expires_at_unix`
/// hasn't passed before invoking this; we double-check below but
/// the wizard's countdown should be doing it too.
pub async fn run_pairing(
    identity: DesktopIdentity,
    offer: PairingOffer,
    phone: mdns::DiscoveredPhone,
    wallet_fingerprints: Vec<Fingerprint>,
    dir: CoincubeDirectory,
) -> Result<PairedPhone, PairingError> {
    let now_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now_unix >= offer.expires_at_unix {
        return Err(PairingError::OfferExpired);
    }

    // Dial unpinned so we accept whatever cert the phone presents;
    // we'll capture and pin its SHA-256 right after the handshake.
    let transport = PairedTransport::connect_unpinned(phone.addr, &identity)
        .await
        .map_err(|e| PairingError::NetworkError(format!("dial phone: {}", e)))?;
    let phone_pin = transport
        .peer_cert_fingerprint()
        .ok_or_else(|| PairingError::NetworkError("phone presented no cert".into()))?;

    // Split into reader/writer so we can read PairingComplete and
    // (best-effort) send a Pong ack without one half blocking the
    // other. The connection drops when both halves go out of scope
    // at function end.
    let (mut reader, mut writer) = transport.split();
    let envelope = reader
        .recv()
        .await
        .map_err(|e| PairingError::NetworkError(format!("recv pairing_complete: {}", e)))?;
    let complete = match envelope.payload {
        Some(local_v1::local_envelope::Payload::PairingComplete(c)) => c,
        _ => {
            return Err(PairingError::InternalError(
                "expected PairingComplete envelope".into(),
            ));
        }
    };

    tracing::debug!(
        target: "phone_signer::pairing",
        "phone reported cert fp = {}",
        complete.phone_cert_fp,
    );

    // Tautological today — the phone-reported wallet fingerprint
    // isn't in the proto yet. Surface the typed variant anyway so it
    // becomes meaningful as soon as the wire format grows the field.
    let claimed_fp = offer.wallet_fingerprint;
    if !wallet_fingerprints.contains(&claimed_fp) {
        return Err(PairingError::WalletFingerprintMismatch {
            expected: wallet_fingerprints.clone(),
            claimed: claimed_fp,
        });
    }

    // Best-effort ack so the phone can render "pairing complete".
    let ack = LocalEnvelope {
        payload: Some(local_v1::local_envelope::Payload::Pong(
            crate::services::connect::grpc::connect_v1::Pong { ts_unix_ms: 0 },
        )),
    };
    let _ = writer.send(&ack).await;

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

    // Both halves of `transport` drop here; the next discovery tick
    // redials via the steady-state pinned path.
    let _ = (reader, writer);
    Ok(paired)
}

// `prost::Message` is used implicitly via the generated proto types'
// methods (encode/decode/encoded_len). Keep the import to avoid a
// future refactor accidentally dropping it.
#[allow(dead_code)]
fn _force_prost_import(env: &LocalEnvelope) -> usize {
    env.encoded_len()
}
