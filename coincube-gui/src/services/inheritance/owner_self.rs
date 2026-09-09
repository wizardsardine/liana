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
//!
//! Under Connect blinding (`PLAN-connect-blinding` A3) the recipients list
//! serves the phone key's xpub as an envelope sealed to **this Cube's** own
//! encryption key, with the plaintext `xpub` column empty. The recipient key
//! therefore goes through [`resolve_key_xpub`] — the same one place every
//! other Connect-served key becomes an xpub — before anything is sealed to it.

use coincube_core::miniscript::bitcoin::Network;
use zeroize::Zeroizing;

use super::escrow::{
    build_escrow_set_parts, validate_account_derivation, EscrowError, KeyholderXpub,
};
use crate::services::coincube::{
    CoincubeClient, CoincubeError, InheritanceEnvelopeWire, RecoveryKitRecipient,
};
use crate::services::connect::crypto::{resolve_key_xpub, CubeEncryptionKey, KeyResolveError};

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
    /// The recipient's registered key couldn't be turned into a usable xpub:
    /// its envelope wouldn't open (or this device has no Cube encryption key
    /// to open it with — [`KeyResolveError::Locked`]), the row carried neither
    /// shape, or the plaintext failed validation.
    UnreadableRecipientKey(KeyResolveError),
    /// A `seed_json`/tier mismatch: the recipient's tier wants the seed but the
    /// caller didn't supply it (or vice-versa).
    TierMismatch,
    /// There is nothing to seal: the Cube has no Vault (so no descriptor) and
    /// the recipient's tier is Vault-only (so no seed either). The owner needs
    /// a Vault, or a Full-Cube phone key, before phone backup can hold anything.
    NothingToSeal,
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
            Self::UnreadableRecipientKey(KeyResolveError::Locked) => write!(
                f,
                "Your phone recovery key is encrypted to this Cube's key, which isn't available \
                 on this device. Restore this Cube from its seed, then try again."
            ),
            Self::UnreadableRecipientKey(e) => write!(
                f,
                "Your phone recovery key is unreadable ({}). Create the recovery key again in \
                 your Keychain app, then check again.",
                e
            ),
            Self::TierMismatch => write!(
                f,
                "The recovery material doesn't match what your phone key is set up to protect."
            ),
            Self::NothingToSeal => write!(
                f,
                "Nothing to back up to your phone yet: this Cube has no Vault, and your phone \
                 key is set up for Vault-only recovery. Create a Vault, or set up Full-Cube \
                 recovery in your Keychain app, then try again."
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
/// Returns one descriptor envelope (when `descriptor_json` is `Some`) plus one
/// seed envelope (for Full-Cube).
///
/// `cube_enc_key` is this Cube's seed-derived encryption key, needed to open
/// the recipient's blinded xpub envelope (see the module docs); `None` only on
/// a Cube with no on-disk seed, where a blinded key fails closed as
/// [`OwnerSelfError::UnreadableRecipientKey`]`(Locked)` rather than sealing to
/// a key we can't read. `network` is the Cube's, checked against the resolved
/// xpub exactly as the Vault keyholder path does.
///
/// `descriptor_json` is optional because a Cube backs up its Master Seed Phrase
/// the moment it is created — before it has a Vault — and adds the Wallet
/// Descriptor later by re-sealing (the card's "Finish backing up" → Rotate).
/// A seed-only set is a legitimate kit; an *empty* set is refused with
/// [`OwnerSelfError::NothingToSeal`] so a Vault-only phone key on a vaultless
/// Cube can never upload nothing and report "backed up".
pub fn build_owner_self_envelope_set(
    recipient: &RecoveryKitRecipient,
    cube_id: u64,
    cube_enc_key: Option<&CubeEncryptionKey>,
    network: Network,
    descriptor_json: Option<&[u8]>,
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
    // Checked here (not only in `build_escrow_set_parts`) so the user-facing
    // message can name the actual situation — no Vault + Vault-only key —
    // rather than the generic "nothing supplied".
    if descriptor_json.is_none() && seed_json.is_none() {
        return Err(OwnerSelfError::NothingToSeal);
    }
    let key = recipient
        .key
        .as_ref()
        .ok_or(OwnerSelfError::RecipientMissingKey)?;
    // CC-DESK-002 fail-fast, before any envelope is opened: a malformed
    // derivation path is the *row's* problem and is reported as such
    // (`build_escrow_set_parts` is the authoritative gate; this mirrors
    // `keyholders_from_vault`). Running it first also keeps the resolve step
    // from mis-reporting an empty path as an xpub depth mismatch.
    validate_account_derivation(&key.derivation_path).map_err(|_| {
        OwnerSelfError::Escrow(EscrowError::BadKeyholderDerivation {
            key_id: key.id,
            path: key.derivation_path.clone(),
        })
    })?;
    // Blinding-agnostic resolve: opens the envelope with the Cube key (or
    // accepts a legacy plaintext column) and validates the result either way.
    // Reading `key.xpub` directly here is exactly the bug this replaces — under
    // envelope-only serving it is empty, and `Xpub::from_str("")` surfaced as
    // a bare "base58 encoding error".
    let xpub = resolve_key_xpub(key, cube_enc_key, cube_id, network)
        .map_err(OwnerSelfError::UnreadableRecipientKey)?;
    let khs = [KeyholderXpub {
        key_id: key.id,
        xpub,
        account_derivation: key.derivation_path.clone(),
    }];
    build_escrow_set_parts(&khs, cube_id, descriptor_json, seed_json)
        .map_err(OwnerSelfError::Escrow)
}

/// Seal the owner's recovery material to their `owner-self` key and upload it
/// (PR 2). Owner-side desktop crypto only — the Keychain isn't involved in
/// sealing (public-key encryption to the recipient's xpub). The `Zeroizing`
/// seed buffer is owned here and wiped the instant it's sealed — before the
/// upload await — so the seed plaintext never lingers across the network
/// round-trip; only ciphertext crosses the wire.
///
/// `cube_enc_key` / `network` are forwarded to
/// [`build_owner_self_envelope_set`] to open the recipient's blinded xpub.
#[allow(clippy::too_many_arguments)]
pub async fn seal_and_upload_owner_self(
    client: &CoincubeClient,
    cube_id: u64,
    cube_enc_key: Option<&CubeEncryptionKey>,
    network: Network,
    recipient: &RecoveryKitRecipient,
    descriptor_json: Option<&[u8]>,
    seed_json: Option<Zeroizing<Vec<u8>>>,
) -> Result<(), OwnerSelfError> {
    let set = build_owner_self_envelope_set(
        recipient,
        cube_id,
        cube_enc_key,
        network,
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
    use crate::services::connect::crypto::XpubEnvelope;
    use crate::services::inheritance::ecies::{keychain_shared_key, ENCRYPTION_CHILD_INDEX};
    use crate::services::inheritance::{open_with_shared_key, wire_to_envelope};
    use coincube_core::miniscript::bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv, Xpub};
    use coincube_core::miniscript::bitcoin::secp256k1::{PublicKey, Secp256k1};
    use coincube_core::signer::MasterSigner;
    use std::str::FromStr;

    const CUBE: u64 = 7;
    /// The fixture keys below are mainnet (`Network::Bitcoin` master) — the
    /// resolve step checks the xpub's network against the Cube's.
    const NET: Network = Network::Bitcoin;

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
                xpub_envelope: None,
                derivation_path: "m/48'/0'/0'/2'".to_string(),
            }),
        }
    }

    /// Stand in for the owner's Keychain: derive the `/7000` child priv and
    /// ECDH+HKDF to `K`, then open the wire envelope.
    fn open(key: &OwnerKey, wire: &InheritanceEnvelopeWire) -> Zeroizing<Vec<u8>> {
        open_as(key, wire, CUBE, 77)
    }

    fn open_as(
        key: &OwnerKey,
        wire: &InheritanceEnvelopeWire,
        cube_id: u64,
        key_id: u64,
    ) -> Zeroizing<Vec<u8>> {
        let secp = Secp256k1::new();
        let child = ChildNumber::from_normal_idx(ENCRYPTION_CHILD_INDEX).unwrap();
        let child_sk = key.xpriv.derive_priv(&secp, &[child]).unwrap().private_key;
        let eph_pk = PublicKey::from_slice(&hex::decode(&wire.ephemeral_pubkey).unwrap()).unwrap();
        let k = keychain_shared_key(&child_sk, &eph_pk);
        let env = wire_to_envelope(wire).unwrap();
        open_with_shared_key(&k, &env, cube_id, key_id).unwrap()
    }

    /// The gap this closes. `build_owner_self_envelope_set` builds its own
    /// `KeyholderXpub` and therefore never passed through
    /// `keyholders_from_vault`'s CC-DESK-002 check: a malformed
    /// `derivationPath` from the server was sealed verbatim, the upload
    /// reported success, and the Recovery card showed phone recovery as on. The
    /// failure surfaced only when recovery was attempted — and re-running
    /// "Protect with my phone" would re-seal the same bad path.
    ///
    /// `build_escrow_set` now validates every keyholder before sealing any, so
    /// this path fails closed at seal time instead.
    #[test]
    fn owner_self_refuses_a_malformed_recipient_derivation() {
        let key = owner_key(b"owner-self-bad-derivation-vector-000000000000");
        for bad in ["", "   ", "m/", "not-a-path", "m/48'/0'/0'/2'/x"] {
            let mut r = recipient(&key, None);
            r.key.as_mut().expect("recipient key").derivation_path = bad.to_string();

            let err =
                build_owner_self_envelope_set(&r, CUBE, None, NET, Some(&b"wsh(...)#ck"[..]), None)
                    .expect_err("a malformed derivation path must never be sealed");
            assert!(
                matches!(
                    err,
                    OwnerSelfError::Escrow(EscrowError::BadKeyholderDerivation { .. })
                ),
                "{:?} gave {:?}",
                bad,
                err,
            );
        }

        // The registered form still seals — the gate refuses the malformed
        // path, not the flow.
        let good = recipient(&key, None);
        assert!(build_owner_self_envelope_set(
            &good,
            CUBE,
            None,
            NET,
            Some(&b"wsh(...)#ck"[..]),
            None
        )
        .is_ok());
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
        let set = build_owner_self_envelope_set(
            &recipient,
            CUBE,
            None,
            NET,
            Some(&b"wsh(desc)#ck"[..]),
            Some(seed.as_slice()),
        )
        .unwrap();
        assert!(
            set.iter().any(|e| e.artifact_kind == "seed"),
            "full-cube detect-then-seal must include the seed envelope"
        );
        seal_and_upload_owner_self(
            &client,
            CUBE,
            None,
            NET,
            &recipient,
            Some(&b"wsh(desc)#ck"[..]),
            Some(seed),
        )
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

        let set =
            build_owner_self_envelope_set(&r, CUBE, None, NET, Some(descriptor), None).unwrap();
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

        let set = build_owner_self_envelope_set(&r, CUBE, None, NET, Some(descriptor), Some(seed))
            .unwrap();
        assert_eq!(set.len(), 2);
        let seed_wire = set.iter().find(|e| e.artifact_kind == "seed").unwrap();
        assert_eq!(open(&key, seed_wire).as_slice(), seed.as_slice());
    }

    /// The vaultless-Cube case: a Cube backs up its Master Seed Phrase at
    /// creation, before any Vault exists, so the phone seal must accept a
    /// seed with no descriptor. The set is a single seed envelope that the
    /// owner's Keychain can open; the descriptor is added later by re-sealing.
    #[test]
    fn full_cube_without_vault_builds_seed_only_set_that_round_trips() {
        let key = owner_key(b"owner-self-seed-only-seed-vector-0000000000");
        let r = recipient(&key, Some(OwnerRecoveryTier::FullCube));
        let seed = br#"{"version":1,"mnemonic":{"phrase":"abandon ... about","language":"en"}}"#;

        let set = build_owner_self_envelope_set(&r, CUBE, None, NET, None, Some(seed)).unwrap();
        assert_eq!(set.len(), 1, "seed-only seals exactly one envelope");
        assert_eq!(set[0].artifact_kind, "seed");
        assert_eq!(set[0].keyholder_key_id, Some(77));
        assert_eq!(open(&key, &set[0]).as_slice(), seed.as_slice());
    }

    /// No Vault *and* a Vault-only phone key leaves nothing to seal. Refuse
    /// with the specific variant (not the generic escrow error) so the wizard
    /// can tell the owner what to do — an empty upload would otherwise read as
    /// "backed up" while restoring nothing.
    #[test]
    fn vault_only_without_vault_has_nothing_to_seal() {
        let key = owner_key(b"owner-self-nothing-seed-vector-000000000000");
        let r = recipient(&key, Some(OwnerRecoveryTier::VaultOnly));
        assert!(matches!(
            build_owner_self_envelope_set(&r, CUBE, None, NET, None, None),
            Err(OwnerSelfError::NothingToSeal)
        ));
        // Unknown tier (older API) with nothing supplied is the same refusal.
        let r_unknown = recipient(&key, None);
        assert!(matches!(
            build_owner_self_envelope_set(&r_unknown, CUBE, None, NET, None, None),
            Err(OwnerSelfError::NothingToSeal)
        ));
    }

    #[test]
    fn tier_mismatch_is_rejected() {
        let key = owner_key(b"owner-self-mismatch-seed-vector-00000000000");
        // Recipient says vault-only but caller supplied a seed → reject.
        let r = recipient(&key, Some(OwnerRecoveryTier::VaultOnly));
        assert!(matches!(
            build_owner_self_envelope_set(&r, CUBE, None, NET, Some(&b"d"[..]), Some(&b"seed"[..])),
            Err(OwnerSelfError::TierMismatch)
        ));
        // Recipient says full-cube but caller omitted the seed → reject.
        let r2 = recipient(&key, Some(OwnerRecoveryTier::FullCube));
        assert!(matches!(
            build_owner_self_envelope_set(&r2, CUBE, None, NET, Some(&b"d"[..]), None),
            Err(OwnerSelfError::TierMismatch)
        ));
    }

    /// Both guards apply when a Full-Cube recipient is given neither half:
    /// the missing seed violates the registered tier and the set would also be
    /// empty. The tier check deliberately wins so callers are told that their
    /// recovery material does not match the phone key configuration.
    #[test]
    fn full_cube_with_no_artifacts_reports_tier_mismatch_first() {
        let key = owner_key(b"owner-self-empty-full-seed-vector-00000000000");
        let r = recipient(&key, Some(OwnerRecoveryTier::FullCube));

        assert!(matches!(
            build_owner_self_envelope_set(&r, CUBE, None, NET, None, None),
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
            build_owner_self_envelope_set(&r, CUBE, None, NET, Some(&b"d"[..]), None),
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
            build_owner_self_envelope_set(&r, CUBE, None, NET, Some(&b"d"[..]), None),
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
        seal_and_upload_owner_self(
            &client,
            CUBE,
            None,
            NET,
            &r,
            Some(&b"wsh(desc)#ck"[..]),
            None,
        )
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

    /// The `SPEC-cube-xpub-envelope-v1` §8 vector, shared with
    /// `key_resolve.rs` / `escrow.rs`: a testnet BIP-48 account xpub sealed to
    /// the Cube key derived from the same mnemonic, bound to cube 42 / key 7.
    const KAT_MNEMONIC: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const KAT_XPUB: &str = "tpubDFH9dgzveyD8zTbPUFuLrGmCydNvxehyNdUXKJAQN8x4aZ4j6UZqGfnqFrD4NqyaTVGKbvEW54tsvPTK2UoSbCC1PJY8iCNiwTL3RWZEheQ";
    const KAT_E: &str = "032c0b7cf95324a07d05398b240174dc0c2be444d96b159aa6c7f7b1e668680991";
    const KAT_NONCE: &str = "0000000000000000cafebabe";
    const KAT_CT: &str = "fc13b1b9639e00e163b3664b62f516ad49d7f19c5383a758706ca813fa8e236cf14a4189aa61ee94801d31cb26a14a999eb5ea2c90a53bc704c5b262ff2b4cf984e97d7c92d13069b829b972c501190db9eaba00b8df84a25c78125e602cff3b037c7db65974b063084596a64667d5f92d647067c3c5453237d7e9e3573a57";
    const KAT_CUBE: u64 = 42;
    const KAT_KEY: u64 = 7;
    const KAT_PATH: &str = "m/48'/1'/0'/2'";

    /// A recipient row exactly as an envelope-only server serves it: empty
    /// plaintext `xpub`, the KAT envelope in `xpubEnvelope`.
    fn blinded_recipient(tier: Option<OwnerRecoveryTier>) -> RecoveryKitRecipient {
        RecoveryKitRecipient {
            id: 1,
            key_id: KAT_KEY,
            role: "owner-self".to_string(),
            tier,
            key: Some(RecoveryRecipientKey {
                id: KAT_KEY,
                xpub: String::new(),
                xpub_envelope: Some(XpubEnvelope {
                    scheme: crate::services::connect::crypto::XPUB_ENVELOPE_SCHEME.to_string(),
                    recipient: crate::services::connect::crypto::RECIPIENT_CUBE_OWNER.to_string(),
                    aad_key_id_bound: true,
                    ephemeral_pubkey: KAT_E.to_string(),
                    nonce: KAT_NONCE.to_string(),
                    ciphertext: KAT_CT.to_string(),
                }),
                derivation_path: KAT_PATH.to_string(),
            }),
        }
    }

    /// The regression this file's blinding support closes. Once the API serves
    /// envelope-only recipients (`XPUB_ENVELOPE_ONLY`), `key.xpub` is `""` and
    /// the old `Xpub::from_str(&key.xpub)` surfaced in the wizard as
    /// "Your phone recovery key is unreadable (base58 encoding error)". The
    /// recipient must resolve through the Cube's encryption key — and the
    /// envelope it seals must open under the phone's real private key, proving
    /// the *decrypted* xpub (not some fallback) is what was sealed to.
    #[test]
    fn blinded_recipient_resolves_through_the_cube_key_and_seals_to_the_decrypted_xpub() {
        let signer = MasterSigner::from_str(Network::Testnet, KAT_MNEMONIC).unwrap();
        let cube_key = CubeEncryptionKey::derive(&signer, Network::Testnet);
        let r = blinded_recipient(Some(OwnerRecoveryTier::VaultOnly));
        let descriptor = b"wsh(blinded)#ck";

        let set = build_owner_self_envelope_set(
            &r,
            KAT_CUBE,
            Some(&cube_key),
            Network::Testnet,
            Some(descriptor),
            None,
        )
        .expect("an envelope-only recipient must resolve and seal");
        assert_eq!(set.len(), 1);
        assert_eq!(set[0].artifact_kind, "descriptor");
        assert_eq!(set[0].keyholder_key_id, Some(KAT_KEY));

        // The phone's side: the KAT xpub's private half, from the same mnemonic.
        let secp = Secp256k1::new();
        let path = DerivationPath::from_str("48'/1'/0'/2'").unwrap();
        let xpriv = signer.xpriv_at(&path, &secp);
        let phone = OwnerKey {
            xpub: Xpub::from_priv(&secp, &xpriv),
            xpriv,
        };
        assert_eq!(phone.xpub.to_string(), KAT_XPUB, "fixture drift");
        assert_eq!(
            open_as(&phone, &set[0], KAT_CUBE, KAT_KEY).as_slice(),
            descriptor.as_slice()
        );
    }

    /// No Cube encryption key on this device (watch-only / passkey restore):
    /// a blinded recipient must fail closed with the *local* `Locked` reason —
    /// copy that tells the owner to restore from seed, not to re-create the
    /// phone key — rather than silently downgrading to the empty plaintext.
    #[test]
    fn blinded_recipient_without_cube_key_is_locked_not_unreadable() {
        let r = blinded_recipient(Some(OwnerRecoveryTier::VaultOnly));
        let err = build_owner_self_envelope_set(
            &r,
            KAT_CUBE,
            None,
            Network::Testnet,
            Some(&b"d"[..]),
            None,
        )
        .expect_err("no Cube key → cannot open the envelope");
        assert!(
            matches!(
                err,
                OwnerSelfError::UnreadableRecipientKey(KeyResolveError::Locked)
            ),
            "{:?}",
            err
        );
        assert!(err.to_string().contains("isn't available on this device"));
    }

    /// A blinded recipient opened with the *wrong* Cube key must not seal to
    /// anything: the tag fails and the row is reported unreadable.
    #[test]
    fn blinded_recipient_with_wrong_cube_key_is_unreadable() {
        let other = MasterSigner::from_str(
            Network::Testnet,
            "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
        )
        .unwrap();
        let wrong_key = CubeEncryptionKey::derive(&other, Network::Testnet);
        let r = blinded_recipient(Some(OwnerRecoveryTier::VaultOnly));
        let err = build_owner_self_envelope_set(
            &r,
            KAT_CUBE,
            Some(&wrong_key),
            Network::Testnet,
            Some(&b"d"[..]),
            None,
        )
        .expect_err("a foreign Cube key cannot open the envelope");
        assert!(
            matches!(
                err,
                OwnerSelfError::UnreadableRecipientKey(KeyResolveError::Envelope(_))
            ),
            "{:?}",
            err
        );
    }

    /// An envelope-only server row that somehow carries neither shape (empty
    /// `xpub`, no `xpubEnvelope`) is `Missing`, reported as unreadable with the
    /// re-create-on-phone guidance — never the bare base58 parse error.
    #[test]
    fn recipient_with_neither_xpub_nor_envelope_is_reported_unreadable() {
        let key = owner_key(b"owner-self-empty-row-seed-vector-000000000000");
        let mut r = recipient(&key, Some(OwnerRecoveryTier::VaultOnly));
        r.key.as_mut().unwrap().xpub = String::new();
        let err = build_owner_self_envelope_set(&r, CUBE, None, NET, Some(&b"d"[..]), None)
            .expect_err("an empty row has nothing to seal to");
        assert!(
            matches!(
                err,
                OwnerSelfError::UnreadableRecipientKey(KeyResolveError::Missing)
            ),
            "{:?}",
            err
        );
        let msg = err.to_string();
        assert!(msg.contains("Keychain app"), "{}", msg);
        assert!(!msg.contains("base58"), "{}", msg);
    }

    /// Detect-then-seal against an envelope-only server end to end: the
    /// recipients list carries an empty `xpub` and the envelope, and the
    /// upload still goes out (sealed to the decrypted key).
    #[tokio::test]
    async fn detect_then_seal_blinded_recipient() {
        use httpmock::{Method, MockServer};
        use serde_json::json;

        let signer = MasterSigner::from_str(Network::Testnet, KAT_MNEMONIC).unwrap();
        let cube_key = CubeEncryptionKey::derive(&signer, Network::Testnet);
        let server = MockServer::start();
        let list = server.mock(|when, then| {
            when.method(Method::GET)
                .path("/api/v1/connect/cubes/42/recovery-kit/recipients");
            then.status(200).json_body(json!({
                "success": true,
                "data": [{
                    "id": 1,
                    "keyId": KAT_KEY,
                    "role": "owner-self",
                    "tier": "vault_only",
                    "key": {
                        "id": KAT_KEY,
                        "xpub": "",
                        "xpubEnvelope": {
                            "scheme": crate::services::connect::crypto::XPUB_ENVELOPE_SCHEME,
                            "recipient": crate::services::connect::crypto::RECIPIENT_CUBE_OWNER,
                            "aadKeyIdBound": true,
                            "ephemeralPubkey": KAT_E,
                            "nonce": KAT_NONCE,
                            "ciphertext": KAT_CT
                        },
                        "derivationPath": KAT_PATH
                    }
                }]
            }));
        });
        let put = server.mock(|when, then| {
            when.method(Method::PUT)
                .path("/api/v1/connect/cubes/42/recovery-kit/envelope")
                .json_body_partial(
                    r#"{ "envelopes": [ { "artifactKind": "descriptor", "keyholderKeyId": 7 } ] }"#,
                );
            then.status(200)
                .json_body(json!({ "success": true, "data": {} }));
        });

        let client = CoincubeClient::for_test(server.base_url());
        let recipient = find_owner_self_recipient(&client, KAT_CUBE).await.unwrap();
        assert!(recipient.key.as_ref().unwrap().xpub_envelope.is_some());
        seal_and_upload_owner_self(
            &client,
            KAT_CUBE,
            Some(&cube_key),
            Network::Testnet,
            &recipient,
            Some(&b"wsh(desc)#ck"[..]),
            None,
        )
        .await
        .expect("a blinded recipient must seal and upload");
        list.assert();
        put.assert();
    }
}
