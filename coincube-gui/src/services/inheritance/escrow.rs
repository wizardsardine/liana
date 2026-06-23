//! Owner-side escrow-set construction (ECIES pivot PR 2).
//!
//! When the owner turns on inheritance escrow for a Vault, the desktop seals
//! the recovery material to **each designated keyholder's** registered xpub and
//! uploads the whole set with `PUT …/vault/escrow`. This module is the pure
//! core: extract the keyholder xpubs from the Connect vault, then build the
//! [`InheritanceEnvelopeWire`] set for a chosen [`EscrowTier`]. It does no I/O —
//! the recovery-alerts card supplies the descriptor/seed plaintext and uploads.

use std::str::FromStr;

use coincube_core::miniscript::bitcoin::bip32::{DerivationPath, Xpub};

use super::ecies::{seal_to_xpub, ArtifactKind, ENCRYPTION_CHILD_DERIVATION};
use super::error::EciesError;
use super::wire::envelope_to_wire;
use crate::services::coincube::{
    ConnectVaultResponse, InheritanceEnvelopeWire, VaultMemberRole,
};

/// The owner's chosen escrow tier for a Vault (the single selector decided for
/// the ECIES pivot). Heartbeat monitoring (the server-blind release gate) is on
/// whenever the tier is on; `Off` tears the escrow down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscrowTier {
    /// No escrow — heirs cannot recover. Deletes any stored envelope set.
    Off,
    /// Encrypt the **descriptor** only. The heir recovers the watch-only Vault
    /// and sweeps via the recovery branch; never receives the seed.
    VaultOnly,
    /// Encrypt **seed + descriptor**. The heir restores the entire Cube
    /// (Liquid + Spark + Vault).
    FullCube,
}

impl EscrowTier {
    /// Whether this tier escrows the master seed (Full-Cube only).
    pub fn includes_seed(self) -> bool {
        matches!(self, Self::FullCube)
    }

    /// Whether escrow is enabled (anything to upload + heartbeat on).
    pub fn is_on(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// One keyholder we'll seal to: their `models.Key` id and parsed xpub.
#[derive(Debug, Clone)]
pub struct KeyholderXpub {
    pub key_id: u64,
    pub xpub: Xpub,
}

/// Errors from building the escrow set.
#[derive(Debug)]
pub enum EscrowError {
    /// A keyholder's registered xpub didn't parse — we refuse to upload a
    /// partial set silently, because a dropped keyholder couldn't recover.
    BadKeyholderXpub {
        key_id: u64,
        source: coincube_core::miniscript::bitcoin::bip32::Error,
    },
    /// No keyholder with a registered key was found — there is no one to
    /// escrow to, so escrow would be a no-op the owner should be told about.
    NoKeyholders,
    /// Sealing failed (e.g. a hardened derivation). Wraps the ECIES error.
    Ecies(EciesError),
}

impl std::fmt::Display for EscrowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadKeyholderXpub { key_id, source } => write!(
                f,
                "keyholder key #{} has an unreadable xpub ({}); can't set up recovery for them",
                key_id, source
            ),
            Self::NoKeyholders => write!(
                f,
                "this Vault has no keyholders with a registered key to set up recovery for"
            ),
            Self::Ecies(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for EscrowError {}

impl From<EciesError> for EscrowError {
    fn from(e: EciesError) -> Self {
        Self::Ecies(e)
    }
}

/// Extracts the designated inheritance keyholders (role == Keyholder, with a
/// registered key) and parses each xpub. A keyholder role without a registered
/// key (e.g. a pending invite) is skipped; a present-but-unparseable xpub is a
/// hard error (we never silently drop a keyholder from the set).
pub fn keyholders_from_vault(
    vault: &ConnectVaultResponse,
) -> Result<Vec<KeyholderXpub>, EscrowError> {
    let mut out = Vec::new();
    for m in &vault.members {
        if m.role != VaultMemberRole::Keyholder {
            continue;
        }
        let Some(key) = m.key.as_ref() else {
            continue; // keyholder without a registered key — nothing to seal to
        };
        let xpub = Xpub::from_str(&key.xpub).map_err(|source| EscrowError::BadKeyholderXpub {
            key_id: key.id,
            source,
        })?;
        out.push(KeyholderXpub {
            key_id: key.id,
            xpub,
        });
    }
    if out.is_empty() {
        return Err(EscrowError::NoKeyholders);
    }
    Ok(out)
}

/// Builds the full envelope set to upload: for each keyholder, one descriptor
/// envelope (always) plus, for the Full-Cube tier, one seed envelope. The
/// plaintext is the serialized `DescriptorBlob` / `SeedBlob` JSON (the same
/// blobs the Cube Recovery Kit uses), so the heir restore reuses the existing
/// blob parsing.
///
/// `seed_json` must be `Some` iff `tier.includes_seed()`. Returns
/// `2 * keyholders` envelopes for Full-Cube, `keyholders` for Vault-only.
pub fn build_escrow_set(
    keyholders: &[KeyholderXpub],
    descriptor_json: &[u8],
    seed_json: Option<&[u8]>,
) -> Result<Vec<InheritanceEnvelopeWire>, EscrowError> {
    // The constant is a fixed non-hardened path; parse once.
    let derivation = DerivationPath::from_str(ENCRYPTION_CHILD_DERIVATION)
        .expect("ENCRYPTION_CHILD_DERIVATION is a valid non-hardened path");

    let mut envelopes = Vec::with_capacity(keyholders.len() * if seed_json.is_some() { 2 } else { 1 });
    for kh in keyholders {
        let descriptor_env =
            seal_to_xpub(&kh.xpub, &derivation, ArtifactKind::Descriptor, descriptor_json)?;
        envelopes.push(envelope_to_wire(&descriptor_env, kh.key_id));

        if let Some(seed) = seed_json {
            let seed_env = seal_to_xpub(&kh.xpub, &derivation, ArtifactKind::Seed, seed)?;
            envelopes.push(envelope_to_wire(&seed_env, kh.key_id));
        }
    }
    Ok(envelopes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coincube::{VaultMemberKeySummary, VaultMemberResponse, VaultStatus};
    use crate::services::inheritance::ecies::shared_key_from_ecdh;
    use crate::services::inheritance::{open_with_shared_key, wire_to_envelope};
    use coincube_core::miniscript::bitcoin::bip32::Xpriv;
    use coincube_core::miniscript::bitcoin::secp256k1::{PublicKey, Secp256k1};
    use coincube_core::miniscript::bitcoin::Network;
    use zeroize::Zeroizing;

    /// A test keyholder: account xpub (registered) + account xpriv (Keychain).
    struct TestKeyholder {
        account_xpub: Xpub,
        account_xpriv: Xpriv,
    }

    fn keyholder(seed: &[u8]) -> TestKeyholder {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, seed).unwrap();
        let path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
        let account_xpriv = master.derive_priv(&secp, &path).unwrap();
        let account_xpub = Xpub::from_priv(&secp, &account_xpriv);
        TestKeyholder {
            account_xpub,
            account_xpriv,
        }
    }

    /// Recompute `K` the way the heir's Keychain would, to open an envelope.
    fn recover_key(kh: &TestKeyholder, wire: &InheritanceEnvelopeWire) -> Zeroizing<[u8; 32]> {
        let secp = Secp256k1::new();
        let path = DerivationPath::from_str(&wire.derivation).unwrap();
        let child_sk = kh.account_xpriv.derive_priv(&secp, &path).unwrap().private_key;
        let eph_pk = PublicKey::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(&wire.ephemeral_pubkey)
                .unwrap(),
        )
        .unwrap();
        shared_key_from_ecdh(&eph_pk, &child_sk)
    }

    fn member(role: VaultMemberRole, key: Option<VaultMemberKeySummary>) -> VaultMemberResponse {
        VaultMemberResponse {
            id: 1,
            contact_id: None,
            key_id: key.as_ref().map(|k| k.id),
            role,
            contact: None,
            key,
            created_at: "2026-06-22T00:00:00Z".into(),
        }
    }

    fn key_summary(id: u64, xpub: &Xpub) -> VaultMemberKeySummary {
        VaultMemberKeySummary {
            id,
            name: "Heir key".into(),
            xpub: xpub.to_string(),
            derivation_path: "m/48'/0'/0'/2'".into(),
        }
    }

    fn vault_with(members: Vec<VaultMemberResponse>) -> ConnectVaultResponse {
        ConnectVaultResponse {
            id: 1,
            cube_id: 1,
            timelock_days: 365,
            timelock_expires_at: "2027-06-22T00:00:00Z".into(),
            last_reset_at: "2026-06-22T00:00:00Z".into(),
            status: VaultStatus::Active,
            members,
            created_at: "2026-06-22T00:00:00Z".into(),
            updated_at: "2026-06-22T00:00:00Z".into(),
        }
    }

    use base64::Engine;

    #[test]
    fn keyholders_filters_role_and_skips_keyless() {
        let alice = keyholder(b"alice-seed-vector-000000000000000000000000");
        let bob = keyholder(b"bob-seed-vector-00000000000000000000000000");
        let vault = vault_with(vec![
            member(VaultMemberRole::Keyholder, Some(key_summary(10, &alice.account_xpub))),
            // A keyholder with no registered key yet (pending invite) — skipped.
            member(VaultMemberRole::Keyholder, None),
            // A beneficiary — not an inheritance keyholder.
            member(VaultMemberRole::Beneficiary, Some(key_summary(11, &bob.account_xpub))),
        ]);
        let khs = keyholders_from_vault(&vault).unwrap();
        assert_eq!(khs.len(), 1);
        assert_eq!(khs[0].key_id, 10);
    }

    #[test]
    fn keyholders_errors_on_unparseable_xpub() {
        let bad = VaultMemberKeySummary {
            id: 99,
            name: "Broken".into(),
            xpub: "not-an-xpub".into(),
            derivation_path: "m/0".into(),
        };
        let vault = vault_with(vec![member(VaultMemberRole::Keyholder, Some(bad))]);
        let err = keyholders_from_vault(&vault).unwrap_err();
        assert!(matches!(err, EscrowError::BadKeyholderXpub { key_id: 99, .. }));
    }

    #[test]
    fn keyholders_errors_when_none() {
        let vault = vault_with(vec![member(VaultMemberRole::Observer, None)]);
        assert!(matches!(
            keyholders_from_vault(&vault).unwrap_err(),
            EscrowError::NoKeyholders
        ));
    }

    #[test]
    fn vault_only_builds_descriptor_envelopes_only_and_roundtrips() {
        let alice = keyholder(b"vo-alice-seed-vector-0000000000000000000000");
        let bob = keyholder(b"vo-bob-seed-vector-000000000000000000000000");
        let khs = vec![
            KeyholderXpub { key_id: 10, xpub: alice.account_xpub },
            KeyholderXpub { key_id: 20, xpub: bob.account_xpub },
        ];
        let descriptor = b"wsh(or_d(multi(2,A,B),and_v(...)))#cksum";

        let set = build_escrow_set(&khs, descriptor, None).unwrap();
        // One descriptor envelope per keyholder, no seed envelopes.
        assert_eq!(set.len(), 2);
        assert!(set.iter().all(|e| e.artifact_kind == "descriptor"));
        assert_eq!(set[0].keyholder_key_id, Some(10));
        assert_eq!(set[1].keyholder_key_id, Some(20));

        // Alice's Keychain opens her descriptor envelope.
        let alice_kh = keyholder(b"vo-alice-seed-vector-0000000000000000000000");
        let k = recover_key(&alice_kh, &set[0]);
        let env = wire_to_envelope(&set[0]).unwrap();
        let pt = open_with_shared_key(&k, &env).unwrap();
        assert_eq!(pt.as_slice(), descriptor.as_slice());
    }

    #[test]
    fn full_cube_builds_descriptor_and_seed_per_keyholder() {
        let alice = keyholder(b"fc-alice-seed-vector-0000000000000000000000");
        let khs = vec![KeyholderXpub { key_id: 10, xpub: alice.account_xpub }];
        let descriptor = b"wsh(...)#ck";
        let seed = br#"{"version":1,"mnemonic":{"phrase":"abandon ... about","language":"en"}}"#;

        let set = build_escrow_set(&khs, descriptor, Some(seed)).unwrap();
        assert_eq!(set.len(), 2);
        let kinds: Vec<&str> = set.iter().map(|e| e.artifact_kind.as_str()).collect();
        assert!(kinds.contains(&"descriptor"));
        assert!(kinds.contains(&"seed"));

        // The seed envelope round-trips to the exact seed JSON.
        let alice_kh = keyholder(b"fc-alice-seed-vector-0000000000000000000000");
        let seed_wire = set.iter().find(|e| e.artifact_kind == "seed").unwrap();
        let k = recover_key(&alice_kh, seed_wire);
        let env = wire_to_envelope(seed_wire).unwrap();
        let pt = open_with_shared_key(&k, &env).unwrap();
        assert_eq!(pt.as_slice(), seed.as_slice());
    }
}
