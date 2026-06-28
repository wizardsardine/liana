//! Owner self-recovery escrow — "protect with my phone"
//! (PLAN-owner-keychain-recovery PR 1 + PR 2).
//!
//! The owner-side analogue of [`super::owner`]. Where the heir-escrow path seals
//! to designated heirs' xpubs, this seals the owner's own recovery material to
//! their **`owner-self`** Keychain key. It reuses the same ECIES machinery
//! ([`super::escrow::build_escrow_set`], `seal_to_xpub`) unchanged — no new
//! crypto: the owner-self recipient is just a one-element keyholder set.
//!
//! Two phases:
//!   * PR 1 — **detect** (provisioning is phone-initiated, COIN-390): the
//!     Keychain app mints + attaches the `owner-self` key and registers the
//!     recovery recipient itself (tier `full_cube`). The desktop **never mints
//!     or registers** — it only *detects* the registered recipient via
//!     [`find_owner_self_recipient`]; a `404`/absent row maps to
//!     [`OwnerSelfError::NoRecipient`], the "set this up on your phone first"
//!     affordance.
//!   * PR 2 — seal: read the registered recipient's xpub, seal the seed /
//!     descriptor to it, and upload the envelope set. Owner-side desktop crypto
//!     only — the Keychain is **not** involved in sealing (public-key encryption
//!     to the recipient's xpub, exactly like heir escrow).
//!
//! Invariant I2: the `owner-self` key is a **recovery key, not a Vault signer**.
//! The desktop never routes it through the Vault keyholder chooser; the server
//! rejects a non-`owner-self` role.

use std::str::FromStr;

use coincube_core::miniscript::bitcoin::bip32::Xpub;
use zeroize::Zeroizing;

use super::escrow::{build_escrow_set, EscrowError, KeyholderXpub};
use crate::services::coincube::{
    CoincubeClient, CoincubeError, InheritanceEnvelopeWire, RecoveryKitRecipient,
};

/// Errors from owner self-recovery detection + sealing.
#[derive(Debug)]
pub enum OwnerSelfError {
    /// No `owner-self` recovery recipient is registered for this Cube yet — the
    /// owner must create the recovery key in their Keychain app first
    /// (provisioning is phone-initiated; the desktop only detects it).
    NoRecipient,
    /// The registered recipient row carried no key (xpub) to seal to — a server
    /// that dropped the join. Fail closed rather than guess an xpub.
    RecipientMissingKey,
    /// The recipient's registered xpub didn't parse.
    BadRecipientXpub(coincube_core::miniscript::bitcoin::bip32::Error),
    /// A `seed_json`/tier mismatch: the recipient's tier wants the seed but the
    /// caller didn't supply it (or vice-versa).
    TierMismatch,
    /// Building the envelope set failed (a seal error).
    Escrow(EscrowError),
    /// A Connect call failed (register / read recipients / upload).
    Connect(CoincubeError),
}

impl std::fmt::Display for OwnerSelfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoRecipient => write!(
                f,
                "This Cube isn't set up for phone recovery yet. Create a recovery key in your \
                 Keychain app first, then check again."
            ),
            Self::RecipientMissingKey => write!(
                f,
                "Your phone recovery key is registered but its details are missing — re-run \
                 “Protect with my phone”."
            ),
            Self::BadRecipientXpub(e) => {
                write!(f, "Your phone recovery key is unreadable ({}).", e)
            }
            Self::TierMismatch => write!(
                f,
                "The recovery material doesn't match what your phone key is set up to protect."
            ),
            Self::Escrow(e) => write!(f, "{}", e),
            Self::Connect(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for OwnerSelfError {}

impl From<EscrowError> for OwnerSelfError {
    fn from(e: EscrowError) -> Self {
        Self::Escrow(e)
    }
}

impl From<CoincubeError> for OwnerSelfError {
    fn from(e: CoincubeError) -> Self {
        Self::Connect(e)
    }
}

/// Find the cube's registered `owner-self` recipient (the one we seal to) —
/// the PR 1 "detect" step. Provisioning is phone-initiated: the Keychain app
/// mints + registers the recipient, and the desktop only reads it back here.
/// Maps a `404`/absent row (no recipient yet) to [`OwnerSelfError::NoRecipient`]
/// — the "set this up on your phone first" affordance.
pub async fn find_owner_self_recipient(
    client: &CoincubeClient,
    cube_id: u64,
) -> Result<RecoveryKitRecipient, OwnerSelfError> {
    let rows = match client.list_recovery_kit_recipients(cube_id).await {
        Ok(rows) => rows,
        Err(CoincubeError::NotFound) => return Err(OwnerSelfError::NoRecipient),
        Err(e) => return Err(OwnerSelfError::Connect(e)),
    };
    rows.into_iter()
        .find(|r| r.is_owner_self())
        .ok_or(OwnerSelfError::NoRecipient)
}

/// Build the owner-self envelope set by reusing the heir escrow builder with a
/// **single keyholder** — the owner's own key. `seed_json` must be `Some` iff
/// the Full-Cube tier (the recipient's `tier`, when known, is the authority).
/// Returns one descriptor envelope (+ one seed envelope for Full-Cube).
pub fn build_owner_self_envelope_set(
    recipient: &RecoveryKitRecipient,
    cube_id: u64,
    descriptor_json: &[u8],
    seed_json: Option<&[u8]>,
) -> Result<Vec<InheritanceEnvelopeWire>, OwnerSelfError> {
    // Defense in depth: only ever seal the owner's recovery material — which
    // includes the master seed — to the cube's own `owner-self` recovery key.
    // The production path filters upstream (`find_owner_self_recipient`), but
    // this is a `pub` helper; refuse a mis-roled recipient before touching its
    // key so a wrong caller can't escrow the seed to a non-owner-self party.
    if !recipient.is_owner_self() {
        return Err(OwnerSelfError::NoRecipient);
    }
    // The registered tier (when the server reports it) is the authority on
    // whether the seed is escrowed; refuse a mismatch so we never silently seal
    // a seed the owner didn't intend (or omit one they did).
    if let Some(tier) = recipient.tier {
        if tier.includes_seed() != seed_json.is_some() {
            return Err(OwnerSelfError::TierMismatch);
        }
    }
    let key = recipient
        .key
        .as_ref()
        .ok_or(OwnerSelfError::RecipientMissingKey)?;
    let xpub = Xpub::from_str(&key.xpub).map_err(OwnerSelfError::BadRecipientXpub)?;
    let khs = [KeyholderXpub {
        key_id: key.id,
        xpub,
        account_derivation: key.derivation_path.clone(),
    }];
    build_escrow_set(&khs, cube_id, descriptor_json, seed_json).map_err(OwnerSelfError::Escrow)
}

/// Seal the owner's recovery material to their `owner-self` key and upload it
/// (PR 2). Owner-side desktop crypto only — the Keychain isn't involved in
/// sealing (public-key encryption to the recipient's xpub). The `Zeroizing`
/// seed buffer is owned here and wiped the instant it's sealed — before the
/// upload await — so the seed plaintext never lingers across the network
/// round-trip; only ciphertext crosses the wire.
pub async fn seal_and_upload_owner_self(
    client: &CoincubeClient,
    cube_id: u64,
    recipient: &RecoveryKitRecipient,
    descriptor_json: &[u8],
    seed_json: Option<Zeroizing<Vec<u8>>>,
) -> Result<(), OwnerSelfError> {
    let set = build_owner_self_envelope_set(
        recipient,
        cube_id,
        descriptor_json,
        seed_json.as_ref().map(|s| s.as_slice()),
    )?;
    // The seed plaintext is now sealed into ciphertext in `set`; wipe it
    // (Zeroizing's Drop) immediately rather than holding it alive across the
    // upload's network await.
    drop(seed_json);
    client
        .put_recovery_kit_envelope(cube_id, set)
        .await
        .map_err(OwnerSelfError::Connect)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coincube::{OwnerRecoveryTier, RecoveryRecipientKey};
    use crate::services::inheritance::ecies::{keychain_shared_key, ENCRYPTION_CHILD_INDEX};
    use crate::services::inheritance::{open_with_shared_key, wire_to_envelope};
    use coincube_core::miniscript::bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
    use coincube_core::miniscript::bitcoin::secp256k1::{PublicKey, Secp256k1};
    use coincube_core::miniscript::bitcoin::Network;

    const CUBE: u64 = 7;

    /// A deterministic owner-self key: account xpub (registered) + xpriv (the
    /// Keychain's private side, stood in for here to open envelopes).
    struct OwnerKey {
        xpub: Xpub,
        xpriv: Xpriv,
    }

    fn owner_key(seed: &[u8]) -> OwnerKey {
        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, seed).unwrap();
        let path = DerivationPath::from_str("m/48'/0'/0'/2'").unwrap();
        let xpriv = master.derive_priv(&secp, &path).unwrap();
        let xpub = Xpub::from_priv(&secp, &xpriv);
        OwnerKey { xpub, xpriv }
    }

    fn recipient(key: &OwnerKey, tier: Option<OwnerRecoveryTier>) -> RecoveryKitRecipient {
        RecoveryKitRecipient {
            id: 1,
            key_id: 77,
            role: "owner-self".to_string(),
            tier,
            key: Some(RecoveryRecipientKey {
                id: 77,
                xpub: key.xpub.to_string(),
                derivation_path: "m/48'/0'/0'/2'".to_string(),
            }),
        }
    }

    /// Stand in for the owner's Keychain: derive the `/7000` child priv and
    /// ECDH+HKDF to `K`, then open the wire envelope.
    fn open(key: &OwnerKey, wire: &InheritanceEnvelopeWire) -> Zeroizing<Vec<u8>> {
        let secp = Secp256k1::new();
        let child = ChildNumber::from_normal_idx(ENCRYPTION_CHILD_INDEX).unwrap();
        let child_sk = key.xpriv.derive_priv(&secp, &[child]).unwrap().private_key;
        let eph_pk = PublicKey::from_slice(&hex::decode(&wire.ephemeral_pubkey).unwrap()).unwrap();
        let k = keychain_shared_key(&child_sk, &eph_pk);
        let env = wire_to_envelope(wire).unwrap();
        open_with_shared_key(&k, &env, CUBE, 77).unwrap()
    }

    /// Detect-then-seal (PR 1 "detect" + PR 2): the Keychain app has already
    /// minted + registered the `owner-self` recipient (tier `full_cube`); the
    /// desktop reads it back via `find_owner_self_recipient`, then seals the
    /// seed + descriptor to it. No desktop mint/register anywhere in this path.
    #[tokio::test]
    async fn detect_then_seal_full_cube() {
        use httpmock::{Method, MockServer};
        use serde_json::json;

        let key = owner_key(b"owner-self-detect-then-seal-vector-0000000000");
        let server = MockServer::start();
        // Detect: the phone-registered recipient row, full-cube, with its key.
        let list = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/7/recovery-kit/recipients");
            then.status(200).json_body(json!({
                "success": true,
                "data": [{
                    "id": 1,
                    "keyId": 77,
                    "role": "owner-self",
                    "tier": "full_cube",
                    "key": {
                        "id": 77,
                        "xpub": key.xpub.to_string(),
                        "derivationPath": "m/48'/0'/0'/2'"
                    }
                }]
            }));
        });
        // Seal: full-cube → seed + descriptor uploaded (the seed-half inclusion
        // itself is covered by `full_cube_builds_descriptor_and_seed_that_round_trip`;
        // here we just confirm the detected recipient drives a successful upload).
        let put = server.mock(|when, then| {
            when.method(Method::PUT)
                .path("/api/v1/connect/cubes/7/recovery-kit/envelope");
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let recipient = find_owner_self_recipient(&client, CUBE)
            .await
            .expect("the phone-registered recipient should be detected");
        assert!(recipient.is_owner_self());
        assert_eq!(recipient.tier, Some(OwnerRecoveryTier::FullCube));

        // Full-cube seals both halves; build the set directly to assert the seed
        // half is present, then upload it through the detected recipient.
        let seed = Zeroizing::new(
            br#"{"version":1,"cube":{},"mnemonic":{"phrase":"abandon about","language":"en"}}"#
                .to_vec(),
        );
        let set =
            build_owner_self_envelope_set(&recipient, CUBE, b"wsh(desc)#ck", Some(seed.as_slice()))
                .unwrap();
        assert!(
            set.iter().any(|e| e.artifact_kind == "seed"),
            "full-cube detect-then-seal must include the seed envelope"
        );
        seal_and_upload_owner_self(&client, CUBE, &recipient, b"wsh(desc)#ck", Some(seed))
            .await
            .expect("seal+upload should succeed");
        list.assert();
        put.assert();
    }

    #[test]
    fn vault_only_builds_descriptor_envelope_that_round_trips() {
        let key = owner_key(b"owner-self-vault-only-seed-vector-000000000");
        let r = recipient(&key, Some(OwnerRecoveryTier::VaultOnly));
        let descriptor = b"wsh(or_d(multi(2,A,B),and_v(...)))#cksum";

        let set = build_owner_self_envelope_set(&r, CUBE, descriptor, None).unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].artifact_kind, "descriptor");
        assert_eq!(set[0].keyholder_key_id, Some(77));
        assert_eq!(open(&key, &set[0]).as_slice(), descriptor.as_slice());
    }

    #[test]
    fn full_cube_builds_descriptor_and_seed_that_round_trip() {
        let key = owner_key(b"owner-self-full-cube-seed-vector-0000000000");
        let r = recipient(&key, Some(OwnerRecoveryTier::FullCube));
        let descriptor = b"wsh(...)#ck";
        let seed = br#"{"version":1,"mnemonic":{"phrase":"abandon ... about","language":"en"}}"#;

        let set = build_owner_self_envelope_set(&r, CUBE, descriptor, Some(seed)).unwrap();
        assert_eq!(set.len(), 2);
        let seed_wire = set.iter().find(|e| e.artifact_kind == "seed").unwrap();
        assert_eq!(open(&key, seed_wire).as_slice(), seed.as_slice());
    }

    #[test]
    fn tier_mismatch_is_rejected() {
        let key = owner_key(b"owner-self-mismatch-seed-vector-00000000000");
        // Recipient says vault-only but caller supplied a seed → reject.
        let r = recipient(&key, Some(OwnerRecoveryTier::VaultOnly));
        assert!(matches!(
            build_owner_self_envelope_set(&r, CUBE, b"d", Some(b"seed")),
            Err(OwnerSelfError::TierMismatch)
        ));
        // Recipient says full-cube but caller omitted the seed → reject.
        let r2 = recipient(&key, Some(OwnerRecoveryTier::FullCube));
        assert!(matches!(
            build_owner_self_envelope_set(&r2, CUBE, b"d", None),
            Err(OwnerSelfError::TierMismatch)
        ));
    }

    #[test]
    fn missing_key_fails_closed() {
        let mut r = recipient(
            &owner_key(b"owner-self-nokey-seed-vector-0000000000000"),
            Some(OwnerRecoveryTier::VaultOnly),
        );
        r.key = None;
        assert!(matches!(
            build_owner_self_envelope_set(&r, CUBE, b"d", None),
            Err(OwnerSelfError::RecipientMissingKey)
        ));
    }

    #[test]
    fn non_owner_self_recipient_is_refused() {
        // Defense in depth: never seal the owner's seed material to a recipient
        // whose role isn't the cube's own `owner-self` recovery key. The role
        // guard fires before the tier/key checks (here the row is otherwise
        // well-formed) so a mis-roled recipient can't escrow the seed.
        let key = owner_key(b"owner-self-wrong-role-seed-vector-000000000");
        let mut r = recipient(&key, Some(OwnerRecoveryTier::VaultOnly));
        r.role = "heir".to_string();
        assert!(matches!(
            build_owner_self_envelope_set(&r, CUBE, b"d", None),
            Err(OwnerSelfError::NoRecipient)
        ));
    }

    #[tokio::test]
    async fn seal_and_upload_puts_the_set() {
        use httpmock::{Method, MockServer};
        use serde_json::json;

        let key = owner_key(b"owner-self-upload-seed-vector-00000000000000");
        let r = recipient(&key, Some(OwnerRecoveryTier::VaultOnly));
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::PUT)
                .path("/api/v1/connect/cubes/7/recovery-kit/envelope")
                .json_body_partial(
                    r#"{ "envelopes": [ { "artifactKind": "descriptor", "keyholderKeyId": 77 } ] }"#,
                );
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        seal_and_upload_owner_self(&client, CUBE, &r, b"wsh(desc)#ck", None)
            .await
            .expect("seal+upload should succeed");
        mock.assert();
    }

    #[tokio::test]
    async fn find_owner_self_recipient_maps_404_to_no_recipient() {
        use httpmock::{Method, MockServer};

        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/7/recovery-kit/recipients");
            then.status(404);
        });
        let client = CoincubeClient::for_test(server.base_url());
        assert!(matches!(
            find_owner_self_recipient(&client, CUBE).await,
            Err(OwnerSelfError::NoRecipient)
        ));
        mock.assert();
    }
}
