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
//!   * PR 1 — provision: mint + attach an `owner-self` key on the Keychain, then
//!     register it as a recovery recipient. **The mint has no desktop rail yet**
//!     (it lands in the keychain-app plan), so [`mint_owner_self_key`] is a stub
//!     that fails closed; the registration ([`register_owner_self_recipient`])
//!     is wired and tested.
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
    CoincubeClient, CoincubeError, InheritanceEnvelopeWire, OwnerRecoveryTier, RecoveryKitRecipient,
};

/// Errors from owner self-recovery provisioning + sealing.
#[derive(Debug)]
pub enum OwnerSelfError {
    /// The Keychain couldn't mint/attach the `owner-self` recovery key. Fires on
    /// every call today: the desktop has no rail to ask the Keychain to mint a
    /// recovery key (it lands in the keychain-app plan). Display-safe.
    KeychainMintUnavailable,
    /// No `owner-self` recovery recipient is registered for this Cube yet — the
    /// owner must provision phone protection first.
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
            Self::KeychainMintUnavailable => write!(
                f,
                "Setting up phone recovery isn't available in this build yet. Update your \
                 Keychain app, then try again."
            ),
            Self::NoRecipient => write!(
                f,
                "This Cube isn't set up for phone recovery yet. Choose “Protect with my phone” \
                 first."
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

/// **Stub.** Mint + attach an `owner-self` recovery key on the owner's Keychain
/// and return its `models.Key` id (PR 1).
///
/// Always returns [`OwnerSelfError::KeychainMintUnavailable`] today: the desktop
/// has no rail to ask the Keychain to mint a key (keychain-app plan PR 1). When
/// that lands, this asks the existing Keychain key-management client to mint the
/// key **as a recovery key, not a Vault signer** (invariant I2) and returns its
/// id for [`register_owner_self_recipient`].
pub async fn mint_owner_self_key(_cube_id: u64) -> Result<u64, OwnerSelfError> {
    // TODO(keychain-app PR 1): once the desktop has a Keychain key-mint rail,
    // call it here to mint + attach an `owner-self` key (role MUST be a recovery
    // key, never a Vault signer — invariant I2) and return its `models.Key` id.
    Err(OwnerSelfError::KeychainMintUnavailable)
}

/// Register a freshly-minted `owner-self` key as the cube's recovery recipient
/// (PR 1). `coincube-api` validates the role and refuses to treat the key as a
/// Vault signer.
pub async fn register_owner_self_recipient(
    client: &CoincubeClient,
    cube_id: u64,
    key_id: u64,
    tier: OwnerRecoveryTier,
) -> Result<RecoveryKitRecipient, OwnerSelfError> {
    client
        .register_recovery_kit_recipient(cube_id, key_id, tier)
        .await
        .map_err(OwnerSelfError::Connect)
}

/// Find the cube's registered `owner-self` recipient (the one we seal to). Maps
/// a `404` (no recipients yet) to [`OwnerSelfError::NoRecipient`].
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
/// seed buffer is owned here so it's wiped when this returns; only ciphertext
/// crosses the wire.
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
    client
        .put_recovery_kit_envelope(cube_id, set)
        .await
        .map_err(OwnerSelfError::Connect)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coincube::RecoveryRecipientKey;
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

    #[tokio::test]
    async fn mint_is_stubbed_unavailable() {
        assert!(matches!(
            mint_owner_self_key(CUBE).await,
            Err(OwnerSelfError::KeychainMintUnavailable)
        ));
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
