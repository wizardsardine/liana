//! Pairing-offer generation, encoding/decoding, and expiry helpers.
//!
//! Authoritative QR payload: base64url(JSON) of [`PairingOffer`]. The
//! Flutter side decodes the same shape; any field rename here must be
//! mirrored there.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

/// Version tag for the pairing-offer payload. Bumped whenever the
/// JSON fields below change in a non-backwards-compatible way.
pub const PAIRING_PROTOCOL_VERSION: u32 = 1;

/// Default lifetime of a pairing offer. Long enough to scan from
/// across the room; short enough that an abandoned QR can't be
/// reused hours later.
pub const PAIRING_OFFER_TTL_SECONDS: u64 = 120;

/// Payload encoded inside the QR code shown by the desktop. The
/// phone decodes (base64url → JSON) and uses the fields to verify the
/// desktop's TLS cert at first connect (`spk` matches the cert's
/// SubjectPublicKeyInfo Ed25519 raw pubkey) and to derive the
/// short-lived ephemeral PSK used as an out-of-band shared secret.
///
/// Authoritative shape — must stay in lockstep with the Flutter side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingOffer {
    /// Protocol version. Pin to [`PAIRING_PROTOCOL_VERSION`] for v1.
    #[serde(rename = "v")]
    pub version: u32,

    /// Ephemeral pre-shared key for this pairing attempt. 32 bytes,
    /// base64url-encoded (no padding). Used as proof-of-QR-scan: the
    /// phone echoes a HMAC over the desktop pubkey using this PSK
    /// inside the first encrypted frame, so a passive observer on the
    /// LAN can't impersonate the phone even if cert pinning weren't
    /// enforced. Dropped after the pairing handshake completes.
    #[serde(rename = "psk")]
    pub psk_b64: String,

    /// Desktop's long-lived Ed25519 identity pubkey, 32 bytes
    /// base64url-encoded. This is the bytes of the cert's
    /// SubjectPublicKeyInfo that rustls will present during TLS;
    /// the phone pins it on first connect.
    #[serde(rename = "spk")]
    pub session_pubkey_b64: String,

    /// Service instance name (UUID v4 in v1) — the desktop publishes
    /// this same name in mDNS during the pairing window so the phone
    /// can find which port to dial.
    #[serde(rename = "svc")]
    pub service_name: String,

    /// Master fingerprint of the wallet we want this phone to sign
    /// for. The phone refuses to pair if it can't satisfy this.
    #[serde(rename = "wfp")]
    pub wallet_fingerprint: Fingerprint,

    /// Pairing-offer expiry, in unix seconds. Past this point the
    /// desktop closes the listening socket and the phone is expected
    /// to surface "offer expired".
    #[serde(rename = "exp")]
    pub expires_at_unix: u64,
}

/// A freshly minted pairing offer plus the short-lived PSK the
/// desktop must keep around to verify the phone's first handshake.
/// The desktop's long-lived identity pubkey is implicitly part of the
/// offer (it's whatever the persisted [`identity::DesktopIdentity`]
/// holds — we don't duplicate it here).
#[derive(Debug)]
pub struct GeneratedOffer {
    pub offer: PairingOffer,
    /// The 32-byte PSK in the clear, to verify the phone's
    /// proof-of-QR-scan once it connects. Drop after pairing succeeds.
    pub psk: [u8; 32],
}

/// Generate a fresh pairing offer for the given wallet fingerprint
/// and desktop identity pubkey.
///
/// The desktop is expected to:
///   1. open a TLS listener bound to a free ephemeral port
///   2. advertise mDNS `_coincube-signer._tcp.local.` with
///      `service_name` instance and the bound port
///   3. render the returned offer as a QR
///   4. wait for the phone to dial in within `expires_at_unix`
pub fn generate_offer(
    wallet_fingerprint: Fingerprint,
    desktop_identity_pubkey: &[u8; 32],
) -> GeneratedOffer {
    let mut psk = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut psk);

    let psk_b64 = URL_SAFE_NO_PAD.encode(psk);
    let spk_b64 = URL_SAFE_NO_PAD.encode(desktop_identity_pubkey);
    let service_name = format!("coincube-{}", uuid::Uuid::new_v4().simple());

    let expires_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + PAIRING_OFFER_TTL_SECONDS;

    GeneratedOffer {
        offer: PairingOffer {
            version: PAIRING_PROTOCOL_VERSION,
            psk_b64,
            session_pubkey_b64: spk_b64,
            service_name,
            wallet_fingerprint,
            expires_at_unix,
        },
        psk,
    }
}

/// Encode a pairing offer to the base64url(JSON) form the phone
/// expects inside the QR code.
pub fn encode_offer(offer: &PairingOffer) -> Result<String, String> {
    let json = serde_json::to_vec(offer).map_err(|e| format!("encode offer json: {}", e))?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

/// Decode a pairing offer from the base64url(JSON) form. Useful for
/// tests and for the Phase-3 "Connect by IP" fallback.
pub fn decode_offer(payload: &str) -> Result<PairingOffer, String> {
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|e| format!("decode base64url: {}", e))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("decode offer json: {}", e))
}

/// Whether this offer is still within its validity window.
pub fn is_expired(offer: &PairingOffer) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now >= offer.expires_at_unix
}

/// Seconds remaining until the offer expires (`0` if already past).
pub fn seconds_remaining(offer: &PairingOffer) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    offer.expires_at_unix.saturating_sub(now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_roundtrips_through_base64url_json() {
        let pubkey = [0x42u8; 32];
        let g = generate_offer(Fingerprint::default(), &pubkey);
        let encoded = encode_offer(&g.offer).expect("encode");
        let decoded = decode_offer(&encoded).expect("decode");
        assert_eq!(g.offer.version, decoded.version);
        assert_eq!(g.offer.psk_b64, decoded.psk_b64);
        assert_eq!(g.offer.session_pubkey_b64, decoded.session_pubkey_b64);
        assert_eq!(g.offer.service_name, decoded.service_name);
        assert_eq!(g.offer.expires_at_unix, decoded.expires_at_unix);
    }

    #[test]
    fn psk_is_distinct_per_offer() {
        let pubkey = [0x42u8; 32];
        let g1 = generate_offer(Fingerprint::default(), &pubkey);
        let g2 = generate_offer(Fingerprint::default(), &pubkey);
        assert_ne!(g1.psk, g2.psk);
        assert_ne!(g1.offer.service_name, g2.offer.service_name);
    }
}
