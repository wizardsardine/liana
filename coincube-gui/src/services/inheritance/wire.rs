//! Conversion between the in-memory [`Envelope`] and the base64 JSON wire DTO
//! [`InheritanceEnvelopeWire`] that the Connect client uploads / downloads.
//!
//! Kept here (next to the crypto) rather than in `services::coincube` so the
//! byte<->base64 boundary lives with the type that understands the bytes. The
//! Connect DTO stays a plain serde struct.

use base64::Engine;

use super::ecies::{ArtifactKind, Envelope};
use super::error::EciesError;
use crate::services::coincube::InheritanceEnvelopeWire;

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(s: &str, field: &'static str) -> Result<Vec<u8>, EciesError> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|_| EciesError::MalformedEnvelope(field))
}

/// Serialises a sealed envelope to the wire DTO for upload, tagging it with the
/// recipient keyholder's `models.Key` id so the server can validate membership.
pub fn envelope_to_wire(env: &Envelope, keyholder_key_id: u64) -> InheritanceEnvelopeWire {
    InheritanceEnvelopeWire {
        keyholder_key_id: Some(keyholder_key_id),
        artifact_kind: env.artifact_kind.as_wire().to_string(),
        scheme: env.scheme.clone(),
        ephemeral_pubkey: b64(&env.ephemeral_pubkey),
        ciphertext: b64(&env.ciphertext),
        nonce: b64(&env.nonce),
        derivation: env.derivation.clone(),
    }
}

/// Parses a released wire DTO back into an [`Envelope`]. base64-decodes the
/// byte fields and validates the `artifactKind`; a bad kind or non-base64 field
/// is a malformed envelope (fail-closed). The `scheme` is carried through
/// verbatim — [`super::open_with_shared_key`] rejects an unsupported one.
pub fn wire_to_envelope(w: &InheritanceEnvelopeWire) -> Result<Envelope, EciesError> {
    let artifact_kind = ArtifactKind::from_wire(&w.artifact_kind)
        .ok_or(EciesError::MalformedEnvelope("artifact_kind"))?;
    Ok(Envelope {
        artifact_kind,
        scheme: w.scheme.clone(),
        ephemeral_pubkey: unb64(&w.ephemeral_pubkey, "ephemeral_pubkey")?,
        ciphertext: unb64(&w.ciphertext, "ciphertext")?,
        nonce: unb64(&w.nonce, "nonce")?,
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
            derivation: "9/0".to_string(),
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
    fn non_base64_ciphertext_is_malformed() {
        let mut wire = envelope_to_wire(&sample_envelope(), 1);
        wire.ciphertext = "not base64 !!!".to_string();
        assert!(matches!(
            wire_to_envelope(&wire),
            Err(EciesError::MalformedEnvelope("ciphertext"))
        ));
    }
}
