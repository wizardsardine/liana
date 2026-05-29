//! Pairing-offer generation, encoding/decoding, and expiry helpers.
//!
//! Authoritative QR payload: base64url(JSON) of [`PairingOffer`]. The
//! Flutter side decodes the same shape; any field rename here must be
//! mirrored there. See
//! `plans/PLAN-local-signer-lan-interop-fixes-desktop.md` §1.2 for
//! the locked wire contract.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

use crate::phone_signer::identity::DesktopIdentity;

/// Version tag for the pairing-offer payload. Bumped whenever the
/// JSON fields below change in a non-backwards-compatible way.
pub const PAIRING_PROTOCOL_VERSION: u32 = 1;

/// Default lifetime of a pairing offer. Long enough to scan from
/// across the room; short enough that an abandoned QR can't be
/// reused hours later.
pub const PAIRING_OFFER_TTL_SECONDS: u64 = 120;

/// Payload encoded inside the QR code shown by the desktop. The
/// phone decodes (base64url → JSON), trusts the embedded `cert`
/// after verifying it hashes to `certFp`, uses `svc` to confirm the
/// QR was generated for it, and uses `wfp` + `exp` to gate the
/// pairing flow.
///
/// Authoritative shape — must stay byte-for-byte in lockstep with
/// the Flutter side. See
/// `plans/PLAN-local-signer-lan-cert-in-qr-desktop.md` §1.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingOffer {
    /// Protocol version. Pin to [`PAIRING_PROTOCOL_VERSION`] for v1.
    #[serde(rename = "v")]
    pub version: u32,

    /// Base64url(no padding) of the desktop's self-signed cert DER.
    /// The phone verifies `sha256(decoded) == cert_fp` before
    /// trusting; the desktop sources both fields from the same
    /// [`DesktopIdentity::cert_der`] so they're guaranteed to agree.
    #[serde(rename = "cert")]
    pub cert_der_b64: String,

    /// SHA-256 of the desktop's self-signed cert DER, lowercase hex
    /// (64 chars). Redundant with `cert` but kept for two reasons:
    /// (1) the phone can short-circuit if it doesn't recognise the
    /// hash from a previous pairing, (2) the QR is shorter to log.
    #[serde(rename = "certFp")]
    pub cert_fp: String,

    /// Phone's mDNS instance name (the one it advertises under
    /// `_coincube-signer._tcp.local.`). The desktop resolved this
    /// from mDNS just before generating the offer and embeds it as a
    /// UX hint so the phone can confirm "this QR was meant for me".
    #[serde(rename = "svc")]
    pub service_name: String,

    /// Master fingerprint of the wallet we want this phone to sign
    /// for. Persisted on the resulting `PairedPhone` so reconnects
    /// can match by wallet.
    #[serde(rename = "wfp")]
    pub wallet_fingerprint: Fingerprint,

    /// Pairing-offer expiry, in unix seconds. Past this point the
    /// desktop refuses to act on the offer and the phone is expected
    /// to surface "offer expired".
    #[serde(rename = "exp")]
    pub expires_at_unix: u64,
}

/// A freshly minted pairing offer. v1.1 dropped the PSK and SPK
/// fields — cert pinning already provides mutual auth — so the
/// generated artifact is just the offer itself.
#[derive(Debug)]
pub struct GeneratedOffer {
    pub offer: PairingOffer,
}

/// Generate a fresh pairing offer aimed at a specific phone.
///
/// Takes `&DesktopIdentity` directly so `cert` and `certFp` are
/// guaranteed to derive from the same `cert_der` — the phone's
/// agreement check can't fail because the desktop split the two
/// fields across separate sources.
///
/// `phone_service_name` is the mDNS instance name resolved from a
/// `_coincube-signer._tcp.local.` browse just before this call —
/// embedded so the phone can confirm receipt of the right QR.
pub fn generate_offer(
    wallet_fingerprint: Fingerprint,
    identity: &DesktopIdentity,
    phone_service_name: String,
) -> GeneratedOffer {
    let expires_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + PAIRING_OFFER_TTL_SECONDS;

    GeneratedOffer {
        offer: PairingOffer {
            version: PAIRING_PROTOCOL_VERSION,
            cert_der_b64: identity.cert_der_b64(),
            cert_fp: identity.cert_fp(),
            service_name: phone_service_name,
            wallet_fingerprint,
            expires_at_unix,
        },
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
    use crate::dir::CoincubeDirectory;
    use sha2::Digest;

    fn fresh_identity() -> DesktopIdentity {
        let mut path = std::env::temp_dir();
        path.push(format!("coincube-pairing-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("mkdir");
        let dir = CoincubeDirectory::new(path);
        crate::phone_signer::identity::load_or_create(&dir).expect("identity")
    }

    #[test]
    fn offer_roundtrips_through_base64url_json() {
        let identity = fresh_identity();
        let g = generate_offer(
            Fingerprint::default(),
            &identity,
            "keychain-12345678".into(),
        );
        let encoded = encode_offer(&g.offer).expect("encode");
        let decoded = decode_offer(&encoded).expect("decode");
        assert_eq!(g.offer.version, decoded.version);
        assert_eq!(g.offer.cert_der_b64, decoded.cert_der_b64);
        assert_eq!(g.offer.cert_fp, decoded.cert_fp);
        assert_eq!(g.offer.service_name, decoded.service_name);
        assert_eq!(g.offer.wallet_fingerprint, decoded.wallet_fingerprint);
        assert_eq!(g.offer.expires_at_unix, decoded.expires_at_unix);
    }

    #[test]
    fn generated_offer_records_phone_service_name() {
        let identity = fresh_identity();
        let g = generate_offer(
            Fingerprint::default(),
            &identity,
            "keychain-deadbeef".into(),
        );
        assert_eq!(g.offer.service_name, "keychain-deadbeef");
    }

    #[test]
    fn expires_at_is_in_the_future() {
        let identity = fresh_identity();
        let g = generate_offer(
            Fingerprint::default(),
            &identity,
            "keychain-00000000".into(),
        );
        assert!(!is_expired(&g.offer));
        assert!(seconds_remaining(&g.offer) > 0);
    }

    /// Critical invariant: `cert` and `certFp` are derived from the
    /// same `cert_der` and must agree byte-for-byte. The phone's
    /// matching check will refuse any QR where they don't.
    #[test]
    fn cert_and_cert_fp_agree() {
        let identity = fresh_identity();
        let g = generate_offer(
            Fingerprint::default(),
            &identity,
            "keychain-12345678".into(),
        );
        let raw = URL_SAFE_NO_PAD
            .decode(&g.offer.cert_der_b64)
            .expect("decode cert b64");
        let computed = sha2::Sha256::digest(&raw);
        let computed_hex: String = computed
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        assert_eq!(computed_hex, g.offer.cert_fp);
    }

    /// Sanity check on QR capacity. Ed25519 cert DER is ~250-300
    /// bytes raw, base64url-encoded ~400 bytes. The phone accepts up
    /// to ~1 KB before falling out of medium-EC QR capacity, so this
    /// catches a regression that would silently bloat the QR
    /// (e.g. switching to an RSA-2048 cert chain).
    #[test]
    fn cert_is_under_size_limit() {
        let identity = fresh_identity();
        let g = generate_offer(
            Fingerprint::default(),
            &identity,
            "keychain-12345678".into(),
        );
        assert!(
            g.offer.cert_der_b64.len() < 1024,
            "cert b64 len {} >= 1024",
            g.offer.cert_der_b64.len(),
        );
    }
}
