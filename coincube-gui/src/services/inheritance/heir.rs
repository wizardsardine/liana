//! Heir-side decrypt (ECIES pivot PR 3).
//!
//! Given the ciphertext envelope set the release endpoint returns, the heir's
//! Keychain derives the per-envelope symmetric key `K` (ECDH + HKDF on the
//! recovery private child), and the desktop opens the AES-256-GCM ciphertext
//! here and parses it into the same [`SeedBlob`] / [`DescriptorBlob`] the Cube
//! Recovery Kit uses — so the restore reuses the existing installer machinery.
//!
//! The seed (if any) is reconstructed only here, transiently, on the heir's
//! desktop. Keychain never returns the seed or the recovery private key.

use std::time::Duration;

use super::ecies::ArtifactKind;
use super::error::EciesError;
use super::wire::wire_to_envelope;
use super::{open_with_shared_key, transport_keypair, unwrap_shared_key, SCHEME};
use crate::services::coincube::InheritanceEnvelopeWire;
use crate::services::connect::grpc::session::{DecryptOutcome, GrpcSessionClient};
use crate::services::recovery::{DecryptedKit, DescriptorBlob, SeedBlob, BLOB_VERSION};

/// Length of the HKDF-derived symmetric key Keychain returns.
const SHARED_KEY_LEN: usize = 32;

/// Errors from the heir decrypt path. `Clone` so it can ride Iced messages;
/// no variant carries secrets (only metadata / display copy).
#[derive(Debug, Clone)]
pub enum HeirDecryptError {
    /// The Keychain decrypt call (relayed via the API) failed — declined,
    /// offline, timed out, or returned a malformed key. Display-safe.
    Keychain(String),
    /// The envelope was malformed or used an unsupported scheme.
    Envelope(String),
    /// The ciphertext failed to open (wrong key / tampered) — fail-closed.
    BadKeyOrCorrupt,
    /// Decrypted cleanly but the plaintext JSON wasn't a blob we understand
    /// (or a newer blob version this client must refuse).
    BlobParse(String),
    /// The heir's Keychain declined the decrypt — approval denied, or the
    /// heir's own account is under duress.
    Rejected,
    /// The decrypt request expired before the Keychain answered (TTL elapsed,
    /// Keychain offline, or the local wait timed out).
    Expired,
    /// The release returned no envelopes at all, or none of the kind a given
    /// restore needs (e.g. Full-Cube with no seed envelope).
    MissingMaterial,
}

impl std::fmt::Display for HeirDecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keychain(m) => write!(f, "Keychain couldn't complete recovery: {}", m),
            Self::Envelope(m) => write!(f, "The recovery data was malformed: {}", m),
            Self::BadKeyOrCorrupt => {
                write!(f, "The recovery data couldn't be decrypted on this device.")
            }
            Self::BlobParse(m) => write!(f, "The recovery data wasn't recognised: {}", m),
            Self::Rejected => write!(f, "The recovery was declined on the Keychain."),
            Self::Expired => write!(
                f,
                "The recovery request expired before it was approved. Please try again."
            ),
            Self::MissingMaterial => write!(f, "There's nothing escrowed for you to recover here."),
        }
    }
}

impl std::error::Error for HeirDecryptError {}

impl From<EciesError> for HeirDecryptError {
    fn from(e: EciesError) -> Self {
        match e {
            EciesError::BadKeyOrCorrupt => Self::BadKeyOrCorrupt,
            EciesError::UnsupportedScheme(s) => {
                Self::Envelope(format!("unsupported scheme '{}'", s))
            }
            EciesError::MalformedEnvelope(field) => Self::Envelope(field.to_string()),
            other => Self::Envelope(other.to_string()),
        }
    }
}

/// One decrypted, parsed artifact.
#[derive(Debug)]
pub enum OpenedArtifact {
    Seed(SeedBlob),
    Descriptor(DescriptorBlob),
}

fn check_blob_version(kind: &str, seen: u8) -> Result<(), HeirDecryptError> {
    if seen == BLOB_VERSION {
        return Ok(());
    }
    Err(HeirDecryptError::BlobParse(format!(
        "{} version {} not supported by this client (expected {}). Update your Cube app.",
        kind, seen, BLOB_VERSION
    )))
}

/// Opens one envelope with the Keychain-derived key `K` and parses its blob.
/// Pure (no I/O) — the [`super::decrypt_envelopes`] orchestration calls
/// Keychain for `K` and then this.
pub fn open_blob(
    wire: &InheritanceEnvelopeWire,
    shared_key: &[u8; SHARED_KEY_LEN],
    cube_id: u64,
) -> Result<OpenedArtifact, HeirDecryptError> {
    // The AAD binds the envelope to (kind, cube_id, keyholder_key_id) — SPEC §1.
    // The server must have stamped the keyholder key id on the released row; a
    // missing one means we can't rebuild the AAD, so fail closed.
    let keyholder_key_id = wire.keyholder_key_id.ok_or_else(|| {
        HeirDecryptError::Envelope("recovery envelope is missing keyholderKeyId".to_string())
    })?;
    let env = wire_to_envelope(wire)?;
    let pt = open_with_shared_key(shared_key, &env, cube_id, keyholder_key_id)?;
    match env.artifact_kind {
        ArtifactKind::Seed => {
            let blob: SeedBlob = serde_json::from_slice(&pt)
                .map_err(|e| HeirDecryptError::BlobParse(format!("seed blob: {}", e)))?;
            check_blob_version("seed blob", blob.version)?;
            Ok(OpenedArtifact::Seed(blob))
        }
        ArtifactKind::Descriptor => {
            let blob: DescriptorBlob = serde_json::from_slice(&pt)
                .map_err(|e| HeirDecryptError::BlobParse(format!("descriptor blob: {}", e)))?;
            check_blob_version("descriptor blob", blob.version)?;
            Ok(OpenedArtifact::Descriptor(blob))
        }
    }
}

/// Collapses the opened artifacts into the same [`DecryptedKit`] the Cube
/// Recovery Kit restore consumes. The last of each kind wins (the set has at
/// most one of each for the caller).
pub fn assemble(artifacts: Vec<OpenedArtifact>) -> DecryptedKit {
    let mut seed = None;
    let mut descriptor = None;
    for a in artifacts {
        match a {
            OpenedArtifact::Seed(b) => seed = Some(b),
            OpenedArtifact::Descriptor(b) => descriptor = Some(b),
        }
    }
    DecryptedKit { seed, descriptor }
}

/// Full heir decrypt over the async Connect relay (SPEC-ecies-v1 §4 + §4b).
/// Generates a per-recovery transport keypair, then for each envelope brokers a
/// decrypt to the heir's Keychain (which wraps the symmetric key `K` to our
/// transport pubkey), unwraps `K` locally, and opens + parses the envelope.
/// Returns the assembled [`DecryptedKit`] for the existing restore machinery.
/// `grpc` is the authenticated Connect SessionService client; `cube_id` selects
/// the recovery key on the heir's Keychain and is bound into each envelope's AAD.
pub async fn decrypt_envelopes(
    grpc: &mut GrpcSessionClient,
    cube_id: u64,
    wires: &[InheritanceEnvelopeWire],
) -> Result<DecryptedKit, HeirDecryptError> {
    if wires.is_empty() {
        return Err(HeirDecryptError::MissingMaterial);
    }
    // Fresh transport keypair for this recovery — in memory only, never
    // persisted. The Keychain wraps each `K` to its pubkey; we unwrap with the
    // private scalar. Reused across the descriptor + seed requests; the
    // per-request `request_id` keeps each wrap bound to its own request (§4b).
    let transport = transport_keypair();
    let cube_id_str = cube_id.to_string();
    let mut artifacts = Vec::with_capacity(wires.len());
    for wire in wires {
        // Refuse a scheme we can't open before round-tripping to Keychain.
        if wire.scheme != SCHEME {
            return Err(HeirDecryptError::Envelope(format!(
                "unsupported scheme '{}'",
                wire.scheme
            )));
        }
        // 1. Broker the decrypt; 2–4. wait for the Keychain's wrapped key.
        let request_id = uuid::Uuid::new_v4().to_string();
        grpc.create_decrypt_request(
            request_id.clone(),
            cube_id_str.clone(),
            wire.artifact_kind.clone(),
            transport.public_key().to_vec(),
        )
        .await
        .map_err(|s| HeirDecryptError::Keychain(s.message().to_string()))?;
        let wrapped = await_wrapped_key(grpc, &request_id).await?;

        // 5. Unwrap `K` with the transport key, then open + parse the envelope.
        let k = unwrap_shared_key(&transport, &request_id, &wrapped)?;
        artifacts.push(open_blob(wire, &k, cube_id)?);
    }
    Ok(assemble(artifacts))
}

/// Polls `GetDecryptResult` until the Keychain answers or the wait elapses. The
/// Keychain step needs a human (biometric/PIN) approval, so the deadline is
/// generous; the best-effort `decrypt_result` stream push (not yet wired into
/// the realtime handler) will later let this resolve promptly instead of at the
/// poll cadence.
async fn await_wrapped_key(
    grpc: &mut GrpcSessionClient,
    request_id: &str,
) -> Result<Vec<u8>, HeirDecryptError> {
    const POLL_INTERVAL: Duration = Duration::from_secs(2);
    const MAX_ATTEMPTS: u32 = 150; // ~5 minutes of human-approval headroom
    for _ in 0..MAX_ATTEMPTS {
        match grpc
            .get_decrypt_result(request_id.to_string())
            .await
            .map_err(|s| HeirDecryptError::Keychain(s.message().to_string()))?
        {
            DecryptOutcome::Completed(wrapped) => return Ok(wrapped),
            DecryptOutcome::Rejected => return Err(HeirDecryptError::Rejected),
            DecryptOutcome::Expired => return Err(HeirDecryptError::Expired),
            DecryptOutcome::Pending => tokio::time::sleep(POLL_INTERVAL).await,
        }
    }
    Err(HeirDecryptError::Expired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::inheritance::ecies::{keychain_shared_key, ENCRYPTION_CHILD_INDEX};
    use crate::services::inheritance::{build_escrow_set, KeyholderXpub};
    use crate::services::recovery::{
        DescriptorBlobCube, DescriptorBlobVault, SeedBlobCube, SeedBlobMnemonic,
    };
    use base64::Engine;
    use coincube_core::miniscript::bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv, Xpub};
    use coincube_core::miniscript::bitcoin::secp256k1::{PublicKey, Secp256k1};
    use coincube_core::miniscript::bitcoin::Network;
    use std::str::FromStr;
    use zeroize::Zeroizing;

    // Connect cube id bound into the AAD; the open side must match the seal.
    const CUBE: u64 = 1;

    struct Kh {
        xpub: Xpub,
        xpriv: Xpriv,
    }

    fn kh(seed: &[u8]) -> Kh {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, seed).unwrap();
        let path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
        let xpriv = master.derive_priv(&secp, &path).unwrap();
        let xpub = Xpub::from_priv(&secp, &xpriv);
        Kh { xpub, xpriv }
    }

    /// Stands in for Keychain. The test xpriv is at the account level, so the
    /// dedicated enc child is the single relative step `/7000`.
    fn recover_key(k: &Kh, wire: &InheritanceEnvelopeWire) -> Zeroizing<[u8; 32]> {
        let secp = Secp256k1::new();
        let child = ChildNumber::from_normal_idx(ENCRYPTION_CHILD_INDEX).unwrap();
        let child_sk = k.xpriv.derive_priv(&secp, &[child]).unwrap().private_key;
        let eph_pk = PublicKey::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(&wire.ephemeral_pubkey)
                .unwrap(),
        )
        .unwrap();
        keychain_shared_key(&child_sk, &eph_pk)
    }

    fn sample_seed_blob() -> SeedBlob {
        SeedBlob {
            version: BLOB_VERSION,
            cube: SeedBlobCube {
                uuid: "cube-uuid".into(),
                name: "My Cube".into(),
                network: "bitcoin".into(),
                created_at: "2026-06-22T00:00:00Z".into(),
                lightning_address: None,
            },
            mnemonic: SeedBlobMnemonic {
                phrase: "abandon abandon abandon abandon abandon abandon abandon abandon \
                         abandon abandon abandon about"
                    .into(),
                language: "en".into(),
            },
        }
    }

    fn sample_descriptor_blob() -> DescriptorBlob {
        DescriptorBlob {
            version: BLOB_VERSION,
            cube: DescriptorBlobCube {
                uuid: "cube-uuid".into(),
                network: "bitcoin".into(),
            },
            vault: DescriptorBlobVault {
                name: "My Vault".into(),
                descriptor: "wsh(multi(2,xpubA,xpubB))#cksum".into(),
                change_descriptor: None,
                signers: vec![],
            },
        }
    }

    /// Full-Cube round-trip: owner seals seed+descriptor → heir opens both →
    /// assembled DecryptedKit carries both, parsed correctly.
    #[test]
    fn full_cube_open_and_assemble() {
        let heir = kh(b"heir-full-cube-seed-vector-0000000000000000");
        let khs = vec![KeyholderXpub {
            key_id: 5,
            xpub: heir.xpub,
            account_derivation: "m/48'/0'/0'/2'".to_string(),
        }];
        let descriptor_json = serde_json::to_vec(&sample_descriptor_blob()).unwrap();
        let seed_json = serde_json::to_vec(&sample_seed_blob()).unwrap();

        let set = build_escrow_set(&khs, CUBE, &descriptor_json, Some(&seed_json)).unwrap();

        let opened: Vec<OpenedArtifact> = set
            .iter()
            .map(|w| {
                let heir2 = kh(b"heir-full-cube-seed-vector-0000000000000000");
                let k = recover_key(&heir2, w);
                open_blob(w, &k, CUBE).unwrap()
            })
            .collect();

        let kit = assemble(opened);
        assert!(kit.seed.is_some(), "seed half present");
        assert!(kit.descriptor.is_some(), "descriptor half present");
        assert_eq!(kit.seed.unwrap().mnemonic.language, "en");
        assert_eq!(kit.descriptor.unwrap().vault.name, "My Vault");
    }

    /// Vault-only: a single descriptor envelope → descriptor present, no seed.
    #[test]
    fn vault_only_open_has_descriptor_no_seed() {
        let heir = kh(b"heir-vault-only-seed-vector-000000000000000");
        let khs = vec![KeyholderXpub {
            key_id: 7,
            xpub: heir.xpub,
            account_derivation: "m/48'/0'/0'/2'".to_string(),
        }];
        let descriptor_json = serde_json::to_vec(&sample_descriptor_blob()).unwrap();
        let set = build_escrow_set(&khs, CUBE, &descriptor_json, None).unwrap();

        let heir2 = kh(b"heir-vault-only-seed-vector-000000000000000");
        let k = recover_key(&heir2, &set[0]);
        let kit = assemble(vec![open_blob(&set[0], &k, CUBE).unwrap()]);
        assert!(kit.descriptor.is_some());
        assert!(kit.seed.is_none());
    }

    /// A wrong key fails closed as BadKeyOrCorrupt, never a parse error.
    #[test]
    fn wrong_key_fails_closed() {
        let owner_heir = kh(b"correct-heir-seed-vector-00000000000000000");
        let attacker = kh(b"attacker-heir-seed-vector-0000000000000000");
        let khs = vec![KeyholderXpub {
            key_id: 1,
            xpub: owner_heir.xpub,
            account_derivation: "m/48'/0'/0'/2'".to_string(),
        }];
        let descriptor_json = serde_json::to_vec(&sample_descriptor_blob()).unwrap();
        let set = build_escrow_set(&khs, CUBE, &descriptor_json, None).unwrap();

        let wrong = recover_key(&attacker, &set[0]);
        assert!(matches!(
            open_blob(&set[0], &wrong, CUBE),
            Err(HeirDecryptError::BadKeyOrCorrupt)
        ));
    }

    /// A newer blob version is refused rather than mis-parsed into v1 shape.
    #[test]
    fn newer_blob_version_refused() {
        let heir = kh(b"version-heir-seed-vector-00000000000000000");
        let khs = vec![KeyholderXpub {
            key_id: 1,
            xpub: heir.xpub,
            account_derivation: "m/48'/0'/0'/2'".to_string(),
        }];
        let mut blob = sample_descriptor_blob();
        blob.version = BLOB_VERSION + 1;
        let descriptor_json = serde_json::to_vec(&blob).unwrap();
        let set = build_escrow_set(&khs, CUBE, &descriptor_json, None).unwrap();

        let heir2 = kh(b"version-heir-seed-vector-00000000000000000");
        let k = recover_key(&heir2, &set[0]);
        assert!(matches!(
            open_blob(&set[0], &k, CUBE),
            Err(HeirDecryptError::BlobParse(_))
        ));
    }
}
