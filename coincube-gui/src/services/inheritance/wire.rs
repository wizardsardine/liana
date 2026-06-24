//! Conversion between the in-memory [`Envelope`] and the **hex** JSON wire DTO
//! [`InheritanceEnvelopeWire`] that the Connect client uploads / downloads.
//!
//! The byte fields (`ephemeralPubkey`, `nonce`, `ciphertext`) are **lowercase
//! hex** per `SPEC-ecies-v1.md` §5 — `coincube-api` `hex.DecodeString`s them on
//! `PUT …/vault/escrow` and `hex.EncodeToString`s them on
//! `GET …/vault/recovery-envelope`, so the desktop MUST match (base64 here
//! silently breaks *both* directions: the API rejects the upload, and the heir
//! mis-decodes the release). The gRPC `wrapped_shared_key` is raw `bytes` and is
//! unaffected — only this REST envelope JSON is hex-encoded.
//!
//! Kept next to the crypto so the byte<->hex boundary lives with the type that
//! understands the bytes; the Connect DTO stays a plain serde struct.

use super::ecies::{ArtifactKind, Envelope};
use super::error::EciesError;
use crate::services::coincube::InheritanceEnvelopeWire;

/// Hex-decode a wire field, mapping any malformed input to a fail-closed
/// [`EciesError::MalformedEnvelope`].
fn hex_decode(s: &str, field: &'static str) -> Result<Vec<u8>, EciesError> {
    hex::decode(s).map_err(|_| EciesError::MalformedEnvelope(field))
}

/// Serialises a sealed envelope to the wire DTO for upload, tagging it with the
/// recipient keyholder's `models.Key` id so the server can validate membership.
/// Byte fields are lowercase hex (SPEC §5), matching `coincube-api`.
pub fn envelope_to_wire(env: &Envelope, keyholder_key_id: u64) -> InheritanceEnvelopeWire {
    InheritanceEnvelopeWire {
        keyholder_key_id: Some(keyholder_key_id),
        artifact_kind: env.artifact_kind.as_wire().to_string(),
        scheme: env.scheme.clone(),
        ephemeral_pubkey: hex::encode(&env.ephemeral_pubkey),
        ciphertext: hex::encode(&env.ciphertext),
        nonce: hex::encode(&env.nonce),
        derivation: env.derivation.clone(),
    }
}

/// Parses a released wire DTO back into an [`Envelope`]. Hex-decodes the byte
/// fields (SPEC §5) and validates the `artifactKind`; a bad kind or non-hex
/// field is a malformed envelope (fail-closed). The `scheme` is carried through
/// verbatim — [`super::open_with_shared_key`] rejects an unsupported one.
pub fn wire_to_envelope(w: &InheritanceEnvelopeWire) -> Result<Envelope, EciesError> {
    let artifact_kind = ArtifactKind::from_wire(&w.artifact_kind)
        .ok_or(EciesError::MalformedEnvelope("artifact_kind"))?;
    Ok(Envelope {
        artifact_kind,
        scheme: w.scheme.clone(),
        ephemeral_pubkey: hex_decode(&w.ephemeral_pubkey, "ephemeral_pubkey")?,
        ciphertext: hex_decode(&w.ciphertext, "ciphertext")?,
        nonce: hex_decode(&w.nonce, "nonce")?,
        derivation: w.derivation.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::inheritance::ecies::SCHEME;

    fn sample_envelope() -> Envelope {
        Envelope {
            artifact_kind: ArtifactKind::Seed,
            scheme: SCHEME.to_string(),
            ephemeral_pubkey: vec![0x02; 33],
            ciphertext: vec![0xAB; 48],
            nonce: vec![0x11; 12],
            derivation: "m/48h/1h/0h/2h/7000".to_string(),
        }
    }

    #[test]
    fn wire_roundtrip_preserves_all_fields() {
        let env = sample_envelope();
        let wire = envelope_to_wire(&env, 77);
        assert_eq!(wire.keyholder_key_id, Some(77));
        assert_eq!(wire.artifact_kind, "seed");

        let back = wire_to_envelope(&wire).unwrap();
        assert_eq!(back.artifact_kind, env.artifact_kind);
        assert_eq!(back.scheme, env.scheme);
        assert_eq!(back.ephemeral_pubkey, env.ephemeral_pubkey);
        assert_eq!(back.ciphertext, env.ciphertext);
        assert_eq!(back.nonce, env.nonce);
        assert_eq!(back.derivation, env.derivation);
    }

    // SPEC-ecies-v1 §5/§7.1: byte fields are **lowercase hex**. Pinning the
    // exact strings for the §7.1 known-answer bytes locks the desktop wire
    // encoding to the same strings `coincube-api` produces/consumes — the
    // cross-repo boundary the earlier base64 self-round-trip test never crossed.
    // These bytes are the §7.1 vector shared by `ecies.rs`, `SPEC-ecies-v1.md`,
    // and the API's KAT.
    const KAT_E_HEX: &str = "034f355bdcb7cc0af728ef3cceb9615d90684bb5b2ca5f859ab0f0b704075871aa";
    const KAT_NONCE_HEX: &str = "0000000000000000deadbeef";
    const KAT_CT_HEX: &str = "2e283e30ebac64ec0741b8f0281b3ae458196e5563bf95ac308414d1a457e261c15b99ed0606c4ccd7d44645c52ad3874cf6030efacb5891b7df4c98d426e7cda4ee173ecb5334bd8bae6dea9f2428a5d8920f5b4c8779db83baf40e8ad890ca7465a4964f6ed2d4fc";

    #[test]
    fn wire_encodes_bytes_as_lowercase_hex_spec_v1() {
        let env = Envelope {
            artifact_kind: ArtifactKind::Descriptor,
            scheme: SCHEME.to_string(),
            ephemeral_pubkey: hex::decode(KAT_E_HEX).unwrap(),
            ciphertext: hex::decode(KAT_CT_HEX).unwrap(),
            nonce: hex::decode(KAT_NONCE_HEX).unwrap(),
            derivation: "m/48h/1h/0h/2h/7000".to_string(),
        };
        let wire = envelope_to_wire(&env, 7);
        // Exact lowercase-hex strings — must match coincube-api byte-for-byte.
        assert_eq!(wire.ephemeral_pubkey, KAT_E_HEX);
        assert_eq!(wire.nonce, KAT_NONCE_HEX);
        assert_eq!(wire.ciphertext, KAT_CT_HEX);

        // And hex round-trips back to the exact bytes.
        let back = wire_to_envelope(&wire).unwrap();
        assert_eq!(back.ephemeral_pubkey, env.ephemeral_pubkey);
        assert_eq!(back.ciphertext, env.ciphertext);
        assert_eq!(back.nonce, env.nonce);
    }

    #[test]
    fn unknown_artifact_kind_is_malformed() {
        let mut wire = envelope_to_wire(&sample_envelope(), 1);
        wire.artifact_kind = "private_key".to_string();
        assert!(matches!(
            wire_to_envelope(&wire),
            Err(EciesError::MalformedEnvelope("artifact_kind"))
        ));
    }

    #[test]
    fn non_hex_ciphertext_is_malformed() {
        let mut wire = envelope_to_wire(&sample_envelope(), 1);
        wire.ciphertext = "not hex zz".to_string();
        assert!(matches!(
            wire_to_envelope(&wire),
            Err(EciesError::MalformedEnvelope("ciphertext"))
        ));
    }
}
