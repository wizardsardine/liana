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
use rand::RngCore as _;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq as _;

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

use crate::phone_signer::identity::DesktopIdentity;

/// Version tag for the pairing-offer payload. Bumped whenever the
/// JSON fields below change in a non-backwards-compatible way.
///
/// v2 added the per-offer `psk` and made the phone's
/// `pairing_proof` (a channel-bound HMAC of that psk) mandatory, so
/// the desktop can authenticate the phone it dialed instead of
/// trusting whatever cert answered on the LAN. See
/// `plans/PLAN-local-signer-pairing-phone-auth.md`.
pub const PAIRING_PROTOCOL_VERSION: u32 = 2;

/// Length of the per-offer pairing secret, in bytes. 128-bit: the
/// secret is online, single-use, and TTL-bounded, so this is well
/// past any brute-force concern (22 base64url chars in the QR).
///
/// **Locked at 16 to match the keychain-app**, whose v2 QR parser
/// rejects any psk that doesn't decode to exactly this many bytes.
/// Changing it is a wire-breaking change — update both sides together.
pub const PAIRING_PSK_LEN: usize = 16;

/// Domain-separation prefix for the proof-of-QR-scan HMAC. The `-v2`
/// suffix is part of the signed message: bump it (in lockstep with
/// the phone) if the construction in [`pairing_proof`] ever changes,
/// so a v2 proof can never be mistaken for a future v3 one.
const PAIRING_PROOF_DOMAIN: &[u8] = b"coincube-pair-v2";

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

    /// Per-offer pairing secret: [`PAIRING_PSK_LEN`] CSPRNG bytes,
    /// base64url(no pad). Generated fresh per offer, never persisted,
    /// never logged. The phone learns it only by scanning the QR and
    /// returns proof of knowledge via `PairingComplete.pairing_proof`
    /// (see [`pairing_proof`]). This is the desktop's sole evidence
    /// that the peer it dialed is the device that scanned the QR —
    /// without it the unpinned dial trusts whatever answered on the
    /// LAN. Added in protocol v2.
    #[serde(rename = "psk")]
    pub psk_b64: String,
}

/// A freshly minted pairing offer. The offer carries a per-offer
/// `psk` (protocol v2) that the phone proves knowledge of, so the
/// generated artifact is just the offer itself — the secret travels
/// inside it.
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

    // Fresh CSPRNG secret per offer. `OsRng` pulls straight from the
    // platform entropy source — never seed this from anything
    // predictable, the whole proof rests on the phone being the only
    // other party that learns it.
    let mut psk = [0u8; PAIRING_PSK_LEN];
    rand::rngs::OsRng.fill_bytes(&mut psk);

    GeneratedOffer {
        offer: PairingOffer {
            version: PAIRING_PROTOCOL_VERSION,
            cert_der_b64: identity.cert_der_b64(),
            cert_fp: identity.cert_fp(),
            service_name: phone_service_name,
            wallet_fingerprint,
            expires_at_unix,
            psk_b64: URL_SAFE_NO_PAD.encode(psk),
        },
    }
}

/// Compute the proof-of-QR-scan: lowercase-hex `HMAC-SHA256` over the
/// domain tag and BOTH cert fingerprints, keyed by the offer's
/// per-offer secret. The phone computes this after scanning the QR
/// and returns it in `PairingComplete.pairing_proof`; the desktop
/// recomputes it to authenticate the phone before pinning.
///
/// Construction (MUST stay byte-for-byte in lockstep with the phone —
/// see `grpc/local_envelope.proto` `PairingComplete.pairing_proof`):
///
/// ```text
/// key = base64url_no_pad_decode(psk_b64)
/// msg = "coincube-pair-v2" || desktop_cert_fp || phone_cert_fp   (raw bytes, no separators)
/// proof = lowercase_hex(HMAC-SHA256(key, msg))
/// ```
///
/// Both fingerprints are the 64-char lowercase-hex SHA-256 of the
/// respective self-signed cert DER. `desktop_cert_fp` is the value
/// embedded in the QR (`PairingOffer.cert_fp`); `phone_cert_fp` is
/// the phone's own cert fp. Binding the MAC to both pins is what
/// defeats a relay/substitution attacker: a peer that terminates TLS
/// with a different cert produces a proof over a different
/// `phone_cert_fp` than the desktop expects.
///
/// Returns `Err` only when `psk_b64` isn't valid base64url.
pub fn pairing_proof(
    psk_b64: &str,
    desktop_cert_fp: &str,
    phone_cert_fp: &str,
) -> Result<String, String> {
    let psk = URL_SAFE_NO_PAD
        .decode(psk_b64)
        .map_err(|e| format!("decode psk: {}", e))?;
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &psk);
    let mut msg = Vec::with_capacity(
        PAIRING_PROOF_DOMAIN.len() + desktop_cert_fp.len() + phone_cert_fp.len(),
    );
    msg.extend_from_slice(PAIRING_PROOF_DOMAIN);
    msg.extend_from_slice(desktop_cert_fp.as_bytes());
    msg.extend_from_slice(phone_cert_fp.as_bytes());
    let tag = ring::hmac::sign(&key, &msg);
    Ok(tag.as_ref().iter().map(|b| format!("{:02x}", b)).collect())
}

/// Verify a phone-reported `pairing_proof` in constant time. Returns
/// `true` only when the recomputed proof matches `reported_proof`
/// exactly; `false` on any mismatch, an empty/short report (e.g. a v1
/// phone that doesn't send the field), or a malformed psk.
///
/// `phone_cert_fp` MUST be the fingerprint the desktop captured from
/// the live TLS handshake (the cert it is about to pin), not the
/// phone-reported `phone_cert_fp` string — otherwise the binding is
/// to a value the peer controls and the check proves nothing.
pub fn verify_pairing_proof(
    psk_b64: &str,
    desktop_cert_fp: &str,
    phone_cert_fp: &str,
    reported_proof: &str,
) -> bool {
    let Ok(expected) = pairing_proof(psk_b64, desktop_cert_fp, phone_cert_fp) else {
        return false;
    };
    // Constant-time over the hex strings. A length difference (e.g. an
    // empty report) compares unequal without leaking timing on the
    // matching prefix — and length alone is not secret.
    expected.as_bytes().ct_eq(reported_proof.as_bytes()).into()
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
        assert_eq!(g.offer.psk_b64, decoded.psk_b64);
    }

    #[test]
    fn generated_offer_psk_is_fresh_and_correct_length() {
        let identity = fresh_identity();
        let a = generate_offer(Fingerprint::default(), &identity, "x".into()).offer;
        let b = generate_offer(Fingerprint::default(), &identity, "x".into()).offer;
        // Distinct per offer (a reused psk would let a captured proof
        // replay across pairings).
        assert_ne!(a.psk_b64, b.psk_b64, "psk must be fresh per offer");
        let raw = URL_SAFE_NO_PAD
            .decode(&a.psk_b64)
            .expect("psk is base64url");
        assert_eq!(
            raw.len(),
            PAIRING_PSK_LEN,
            "psk must be {PAIRING_PSK_LEN} bytes"
        );
    }

    /// The whole encoded QR (not just the cert field) must stay inside
    /// the phone's ~1 KB QR budget after adding the psk.
    #[test]
    fn encoded_offer_stays_within_qr_budget() {
        let identity = fresh_identity();
        let g = generate_offer(
            Fingerprint::default(),
            &identity,
            "keychain-12345678".into(),
        );
        let encoded = encode_offer(&g.offer).expect("encode");
        assert!(
            encoded.len() < 1024,
            "encoded QR len {} >= 1024",
            encoded.len()
        );
    }

    #[test]
    fn pairing_proof_verifies_for_matching_inputs() {
        let g = generate_offer(Fingerprint::default(), &fresh_identity(), "x".into()).offer;
        let desktop_fp = &g.cert_fp;
        let phone_fp = "ab".repeat(32); // any 64-hex string
        let proof = pairing_proof(&g.psk_b64, desktop_fp, &phone_fp).expect("compute");
        assert_eq!(proof.len(), 64, "proof is 64 lowercase-hex chars");
        assert!(verify_pairing_proof(
            &g.psk_b64, desktop_fp, &phone_fp, &proof
        ));
    }

    #[test]
    fn pairing_proof_is_bound_to_both_fingerprints() {
        let g = generate_offer(Fingerprint::default(), &fresh_identity(), "x".into()).offer;
        let desktop_fp = &g.cert_fp;
        let phone_fp = "ab".repeat(32);
        let proof = pairing_proof(&g.psk_b64, desktop_fp, &phone_fp).expect("compute");

        // Wrong phone fp (the relay/substitution case): a peer that
        // terminates TLS with a different cert produces a proof over a
        // different phone fp than the desktop expects → rejected.
        let other_phone_fp = "cd".repeat(32);
        assert!(
            !verify_pairing_proof(&g.psk_b64, desktop_fp, &other_phone_fp, &proof),
            "proof must not verify against a different phone cert fp",
        );
        // Wrong desktop fp.
        let other_desktop_fp = "ef".repeat(32);
        assert!(
            !verify_pairing_proof(&g.psk_b64, &other_desktop_fp, &phone_fp, &proof),
            "proof must not verify against a different desktop cert fp",
        );
    }

    /// Shared cross-implementation known-answer vector. This is the
    /// SAME tuple the keychain-app (Flutter) asserts on its side, so a
    /// drift in either codebase's HMAC construction trips both vector
    /// tests in lockstep. Do not change without changing the phone
    /// side and bumping the domain tag — it's the wire protocol. See
    /// plans/PLAN-local-signer-pairing-phone-auth.md.
    #[test]
    fn pairing_proof_known_answer_vector() {
        // psk = 16 bytes 0x00..0x0f; fps = "aa".. and "bb"..
        let psk_b64 = "AAECAwQFBgcICQoLDA0ODw";
        let desktop_fp = "aa".repeat(32);
        let phone_fp = "bb".repeat(32);
        let proof = pairing_proof(psk_b64, &desktop_fp, &phone_fp).expect("compute");
        assert_eq!(
            proof,
            "87f118c74fd540810db273e2ff4c00ee805831dce43aa649553d636e795239b9",
        );
        assert!(verify_pairing_proof(
            psk_b64,
            &desktop_fp,
            &phone_fp,
            &proof
        ));
    }

    #[test]
    fn pairing_proof_rejects_wrong_psk_tamper_and_empty() {
        let g = generate_offer(Fingerprint::default(), &fresh_identity(), "x".into()).offer;
        let other = generate_offer(Fingerprint::default(), &fresh_identity(), "x".into()).offer;
        let desktop_fp = &g.cert_fp;
        let phone_fp = "ab".repeat(32);
        let proof = pairing_proof(&g.psk_b64, desktop_fp, &phone_fp).expect("compute");

        // A different psk (didn't scan THIS QR).
        assert!(!verify_pairing_proof(
            &other.psk_b64,
            desktop_fp,
            &phone_fp,
            &proof
        ));
        // Tampered proof.
        let mut tampered = proof.clone();
        tampered.replace_range(0..1, if proof.starts_with('0') { "1" } else { "0" });
        assert!(!verify_pairing_proof(
            &g.psk_b64, desktop_fp, &phone_fp, &tampered
        ));
        // Empty / missing (e.g. a pre-v2 phone).
        assert!(!verify_pairing_proof(&g.psk_b64, desktop_fp, &phone_fp, ""));
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
        let computed_hex: String = computed.iter().map(|b| format!("{:02x}", b)).collect();
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
