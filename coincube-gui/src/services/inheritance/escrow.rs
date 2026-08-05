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

use coincube_core::miniscript::bitcoin::Network;

use super::ecies::{seal_to_xpub, ArtifactKind, ENCRYPTION_CHILD_INDEX};
use super::error::EciesError;
use super::wire::envelope_to_wire;
use crate::services::coincube::{ConnectVaultResponse, InheritanceEnvelopeWire, VaultMemberRole};
use crate::services::connect::crypto::{resolve_key_xpub, CubeEncryptionKey};

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
    /// The keyholder's registered account derivation path (`models.Key
    /// .DerivationPath`). The envelope's full enc-child path is this + `/7000`
    /// (SPEC §2), so Keychain can derive the matching private child from root.
    pub account_derivation: String,
}

/// Errors from building the escrow set.
#[derive(Debug)]
pub enum EscrowError {
    /// A keyholder's registered key couldn't be turned into a usable xpub —
    /// the blinding envelope wouldn't open, the plaintext didn't parse, or the
    /// decrypted key failed validation. We refuse to upload a partial set
    /// silently, because a dropped keyholder couldn't recover.
    UnreadableKeyholderKey {
        key_id: u64,
        source: crate::services::connect::crypto::KeyResolveError,
    },
    /// No keyholder with a registered key was found — there is no one to
    /// escrow to, so escrow would be a no-op the owner should be told about.
    NoKeyholders,
    /// A keyholder's registered account derivation path (from the Connect vault
    /// response) does not parse as a BIP-32 derivation path. Building the enc
    /// child path (`account_derivation + /7000`) from it would produce an
    /// envelope the heir's phone can never derive `d` for, so it would silently
    /// fail to open at recovery time (CC-DESK-002). Fail closed at seal instead.
    BadKeyholderDerivation { key_id: u64, path: String },
    /// Sealing failed (e.g. a hardened derivation). Wraps the ECIES error.
    Ecies(EciesError),
}

impl std::fmt::Display for EscrowError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnreadableKeyholderKey { key_id, source } => write!(
                f,
                "keyholder key #{} has an unreadable xpub ({}); can't set up recovery for them",
                key_id, source
            ),
            Self::NoKeyholders => write!(
                f,
                "this Vault has no keyholders with a registered key to set up recovery for"
            ),
            Self::BadKeyholderDerivation { key_id, path } => write!(
                f,
                "keyholder key #{} has an unreadable derivation path ({:?}); can't set up recovery for them",
                key_id, path
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

/// Validates that a keyholder's registered account derivation path parses as a
/// BIP-32 [`DerivationPath`]. Accepts both the bare form (`84'/0'/0'`) and the
/// `m/`-prefixed form (`m/84'/0'/0'`). The combined enc-child path used for
/// sealing is `account_derivation + /7000`; if the account path itself parses,
/// appending a valid non-hardened index keeps it valid. Returns `Ok(())` on
/// success and `Err(())` on a malformed path.
fn validate_account_derivation(path: &str) -> Result<(), ()> {
    let trimmed = path.trim();
    let normalized = trimmed
        .strip_prefix("m/")
        .or_else(|| trimmed.strip_prefix("M/"))
        .unwrap_or(trimmed);
    if normalized.is_empty() {
        return Err(());
    }
    DerivationPath::from_str(normalized)
        .map(|_| ())
        .map_err(|_| ())
}

/// Extracts the designated inheritance keyholders (role == Keyholder, with a
/// registered key) and resolves each xpub. A keyholder role without a registered
/// key (e.g. a pending invite) is skipped; a present-but-unreadable key is a
/// hard error (we never silently drop a keyholder from the set — a dropped
/// keyholder simply couldn't recover, and they'd never find out).
///
/// Under Connect blinding (`PLAN-connect-blinding` PR D3) the recovery-recipient
/// list serves **envelopes** rather than plaintext xpubs, so this goes through
/// [`resolve_key_xpub`]: it opens the envelope with the Cube's seed-derived
/// encryption key, or accepts a legacy plaintext column, and validates the
/// result either way. `cube_enc_key` is `None` only where the Cube's seed isn't
/// available (watch-only restores), which surfaces as a hard error here — the
/// right outcome, since sealing an escrow set you can't verify would be worse.
///
/// The sealing semantics below are unchanged: the same envelope set is built
/// from the same xpubs, just resolved differently.
pub fn keyholders_from_vault(
    vault: &ConnectVaultResponse,
    cube_enc_key: Option<&CubeEncryptionKey>,
    cube_id: u64,
    network: Network,
) -> Result<Vec<KeyholderXpub>, EscrowError> {
    let mut out = Vec::new();
    for m in &vault.members {
        if m.role != VaultMemberRole::Keyholder {
            continue;
        }
        let Some(key) = m.key.as_ref() else {
            continue; // keyholder without a registered key — nothing to seal to
        };
        let xpub = resolve_key_xpub(key, cube_enc_key, cube_id, network).map_err(|source| {
            EscrowError::UnreadableKeyholderKey {
                key_id: key.id,
                source,
            }
        })?;
        // CC-DESK-002: validate the account derivation path parses before we
        // build the enc-child path from it and seal an envelope. A malformed
        // path from the server would otherwise yield an envelope the heir can
        // never open, discoverable only at recovery time. Fail closed here.
        validate_account_derivation(&key.derivation_path).map_err(|_| {
            EscrowError::BadKeyholderDerivation {
                key_id: key.id,
                path: key.derivation_path.clone(),
            }
        })?;
        out.push(KeyholderXpub {
            key_id: key.id,
            xpub,
            account_derivation: key.derivation_path.clone(),
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
/// `cube_id` is the Connect vault's cube id, bound into each envelope's AAD
/// (SPEC §1) so a relayed envelope can't be re-targeted at another cube.
pub fn build_escrow_set(
    keyholders: &[KeyholderXpub],
    cube_id: u64,
    descriptor_json: &[u8],
    seed_json: Option<&[u8]>,
) -> Result<Vec<InheritanceEnvelopeWire>, EscrowError> {
    let mut envelopes =
        Vec::with_capacity(keyholders.len() * if seed_json.is_some() { 2 } else { 1 });
    for kh in keyholders {
        // Full path from the seed root to the dedicated enc child (SPEC §2),
        // stored in the envelope so Keychain derives the matching `d`.
        let full_derivation = format!("{}/{}", kh.account_derivation, ENCRYPTION_CHILD_INDEX);
        let descriptor_env = seal_to_xpub(
            &kh.xpub,
            &full_derivation,
            ArtifactKind::Descriptor,
            cube_id,
            kh.key_id,
            descriptor_json,
        )?;
        envelopes.push(envelope_to_wire(&descriptor_env, kh.key_id));

        if let Some(seed) = seed_json {
            let seed_env = seal_to_xpub(
                &kh.xpub,
                &full_derivation,
                ArtifactKind::Seed,
                cube_id,
                kh.key_id,
                seed,
            )?;
            envelopes.push(envelope_to_wire(&seed_env, kh.key_id));
        }
    }
    Ok(envelopes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coincube::{VaultMemberKeySummary, VaultMemberResponse, VaultStatus};
    use crate::services::inheritance::ecies::keychain_shared_key;
    use crate::services::inheritance::{open_with_shared_key, wire_to_envelope};
    use coincube_core::miniscript::bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
    use coincube_core::miniscript::bitcoin::secp256k1::{PublicKey, Secp256k1};
    use coincube_core::miniscript::bitcoin::Network;
    use zeroize::Zeroizing;

    // Connect cube id bound into the AAD; the open side must match the seal.
    const CUBE: u64 = 1;

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
    /// The test keyholder's xpriv is at the account level, so it derives the
    /// dedicated enc child by the single relative step `/7000`.
    fn recover_key(kh: &TestKeyholder, wire: &InheritanceEnvelopeWire) -> Zeroizing<[u8; 32]> {
        let secp = Secp256k1::new();
        let child = ChildNumber::from_normal_idx(ENCRYPTION_CHILD_INDEX).unwrap();
        let child_sk = kh
            .account_xpriv
            .derive_priv(&secp, &[child])
            .unwrap()
            .private_key;
        // The wire encodes byte fields as lowercase hex (SPEC §5), matching
        // production / `coincube-api` — decode the same way the heir does.
        let eph_pk = PublicKey::from_slice(&hex::decode(&wire.ephemeral_pubkey).unwrap()).unwrap();
        keychain_shared_key(&child_sk, &eph_pk)
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
            xpub_envelope: None,
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

    #[test]
    fn keyholders_filters_role_and_skips_keyless() {
        let alice = keyholder(b"alice-seed-vector-000000000000000000000000");
        let bob = keyholder(b"bob-seed-vector-00000000000000000000000000");
        let vault = vault_with(vec![
            member(
                VaultMemberRole::Keyholder,
                Some(key_summary(10, &alice.account_xpub)),
            ),
            // A keyholder with no registered key yet (pending invite) — skipped.
            member(VaultMemberRole::Keyholder, None),
            // A beneficiary — not an inheritance keyholder.
            member(
                VaultMemberRole::Beneficiary,
                Some(key_summary(11, &bob.account_xpub)),
            ),
        ]);
        let khs = keyholders_from_vault(&vault, None, CUBE, Network::Bitcoin).unwrap();
        assert_eq!(khs.len(), 1);
        assert_eq!(khs[0].key_id, 10);
    }

    #[test]
    fn keyholders_errors_on_unparseable_xpub() {
        let bad = VaultMemberKeySummary {
            xpub_envelope: None,
            id: 99,
            name: "Broken".into(),
            xpub: "not-an-xpub".into(),
            derivation_path: "m/0".into(),
        };
        let vault = vault_with(vec![member(VaultMemberRole::Keyholder, Some(bad))]);
        let err = keyholders_from_vault(&vault, None, CUBE, Network::Bitcoin).unwrap_err();
        assert!(matches!(
            err,
            EscrowError::UnreadableKeyholderKey { key_id: 99, .. }
        ));
    }

    /// Connect blinding (PR D3): the recovery-recipient list serves the
    /// keyholder's xpub as an envelope. Sealing must resolve it through the
    /// Cube's encryption key and otherwise behave exactly as before.
    #[test]
    fn keyholders_resolves_a_blinded_keyholder_key() {
        use crate::services::connect::crypto::{CubeEncryptionKey, XpubEnvelope};
        use coincube_core::signer::MasterSigner;

        const MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        // The SPEC-cube-xpub-envelope-v1 §8 vector: a testnet BIP-48 account
        // xpub sealed to the Cube key derived from the same mnemonic, cube 42.
        const KAT_XPUB: &str = "tpubDFH9dgzveyD8zTbPUFuLrGmCydNvxehyNdUXKJAQN8x4aZ4j6UZqGfnqFrD4NqyaTVGKbvEW54tsvPTK2UoSbCC1PJY8iCNiwTL3RWZEheQ";
        const KAT_E: &str = "032c0b7cf95324a07d05398b240174dc0c2be444d96b159aa6c7f7b1e668680991";
        const KAT_NONCE: &str = "0000000000000000cafebabe";
        const KAT_CT: &str = "fc13b1b9639e00e163b3664b62f516ad49d7f19c5383a758706ca813fa8e236cf14a4189aa61ee94801d31cb26a14a999eb5ea2c90a53bc704c5b262ff2b4cf984e97d7c92d13069b829b972c501190db9eaba00b8df84a25c78125e602cff3b037c7db65974b063084596a64667d5f92d647067c3c5453237d7e9e3573a57";
        const KAT_CUBE: u64 = 42;
        // The AAD binds the key id too, so the fixture row must BE key 7.
        const KAT_KEY: u64 = 7;

        let blinded = VaultMemberKeySummary {
            id: KAT_KEY,
            name: "Kenji's phone".into(),
            xpub: String::new(),
            xpub_envelope: Some(XpubEnvelope {
                scheme: crate::services::connect::crypto::XPUB_ENVELOPE_SCHEME.to_string(),
                recipient: crate::services::connect::crypto::RECIPIENT_CUBE_OWNER.to_string(),
                aad_key_id_bound: true,
                ephemeral_pubkey: KAT_E.to_string(),
                nonce: KAT_NONCE.to_string(),
                ciphertext: KAT_CT.to_string(),
            }),
            derivation_path: "m/48'/1'/0'/2'".into(),
        };
        let vault = vault_with(vec![member(VaultMemberRole::Keyholder, Some(blinded))]);

        let signer = MasterSigner::from_str(Network::Testnet, MNEMONIC).unwrap();
        let cube_key = CubeEncryptionKey::derive(&signer, Network::Testnet);

        let khs =
            keyholders_from_vault(&vault, Some(&cube_key), KAT_CUBE, Network::Testnet).unwrap();
        assert_eq!(khs.len(), 1);
        assert_eq!(khs[0].key_id, KAT_KEY);
        assert_eq!(khs[0].xpub.to_string(), KAT_XPUB);

        // Without the Cube key the set can't be built — better to fail loudly
        // than to upload an escrow set that silently drops a keyholder.
        let err = keyholders_from_vault(&vault, None, KAT_CUBE, Network::Testnet).unwrap_err();
        assert!(matches!(
            err,
            EscrowError::UnreadableKeyholderKey {
                key_id: KAT_KEY,
                ..
            }
        ));
    }

    #[test]
    fn keyholders_errors_when_none() {
        let vault = vault_with(vec![member(VaultMemberRole::Observer, None)]);
        assert!(matches!(
            keyholders_from_vault(&vault, None, CUBE, Network::Bitcoin).unwrap_err(),
            EscrowError::NoKeyholders
        ));
    }

    #[test]
    fn vault_only_builds_descriptor_envelopes_only_and_roundtrips() {
        let alice = keyholder(b"vo-alice-seed-vector-0000000000000000000000");
        let bob = keyholder(b"vo-bob-seed-vector-000000000000000000000000");
        let khs = vec![
            KeyholderXpub {
                key_id: 10,
                xpub: alice.account_xpub,
                account_derivation: "m/48'/0'/0'/2'".to_string(),
            },
            KeyholderXpub {
                key_id: 20,
                xpub: bob.account_xpub,
                account_derivation: "m/48'/0'/0'/2'".to_string(),
            },
        ];
        let descriptor = b"wsh(or_d(multi(2,A,B),and_v(...)))#cksum";

        let set = build_escrow_set(&khs, CUBE, descriptor, None).unwrap();
        // One descriptor envelope per keyholder, no seed envelopes.
        assert_eq!(set.len(), 2);
        assert!(set.iter().all(|e| e.artifact_kind == "descriptor"));
        assert_eq!(set[0].keyholder_key_id, Some(10));
        assert_eq!(set[1].keyholder_key_id, Some(20));

        // Alice's Keychain opens her descriptor envelope (AAD = CUBE + key 10).
        let alice_kh = keyholder(b"vo-alice-seed-vector-0000000000000000000000");
        let k = recover_key(&alice_kh, &set[0]);
        let env = wire_to_envelope(&set[0]).unwrap();
        let pt = open_with_shared_key(&k, &env, CUBE, 10).unwrap();
        assert_eq!(pt.as_slice(), descriptor.as_slice());
    }

    #[test]
    fn full_cube_builds_descriptor_and_seed_per_keyholder() {
        let alice = keyholder(b"fc-alice-seed-vector-0000000000000000000000");
        let khs = vec![KeyholderXpub {
            key_id: 10,
            xpub: alice.account_xpub,
            account_derivation: "m/48'/0'/0'/2'".to_string(),
        }];
        let descriptor = b"wsh(...)#ck";
        let seed = br#"{"version":1,"mnemonic":{"phrase":"abandon ... about","language":"en"}}"#;

        let set = build_escrow_set(&khs, CUBE, descriptor, Some(seed)).unwrap();
        assert_eq!(set.len(), 2);
        let kinds: Vec<&str> = set.iter().map(|e| e.artifact_kind.as_str()).collect();
        assert!(kinds.contains(&"descriptor"));
        assert!(kinds.contains(&"seed"));

        // The seed envelope round-trips to the exact seed JSON (AAD = CUBE + 10).
        let alice_kh = keyholder(b"fc-alice-seed-vector-0000000000000000000000");
        let seed_wire = set.iter().find(|e| e.artifact_kind == "seed").unwrap();
        let k = recover_key(&alice_kh, seed_wire);
        let env = wire_to_envelope(seed_wire).unwrap();
        let pt = open_with_shared_key(&k, &env, CUBE, 10).unwrap();
        assert_eq!(pt.as_slice(), seed.as_slice());
    }
}
