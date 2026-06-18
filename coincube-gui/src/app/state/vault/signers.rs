//! Signer classification for the spend flow.
//!
//! Given a PSBT and the wallet's descriptor, produce the list of signers
//! still required to push the PSBT past its spending-path threshold,
//! tagging each one as either a `Local` signer (HW, master signer,
//! border-wallet, …) or a `Keychain` signer (a Connect-registered key on
//! a contact's or the owner's phone).
//!
//! The Keychain classification depends on the per-vault `ConnectVaultMember`
//! list and the cube's Keychain key list — both fetched from the API just
//! before this is called. Wiring those fetches lives in
//! `KeychainSignModal::launch`; this module is pure logic so it can be
//! unit-tested.

use std::collections::HashMap;

use coincube_core::{
    descriptors::{CoincubeDescriptor, CoincubePolicy},
    miniscript::bitcoin::{bip32::Fingerprint, psbt::Psbt},
};

use crate::services::coincube::{CubeKeyRaw, VaultMemberResponse};

/// One signer that the user still has to bring to the PSBT to advance
/// the active spending path past its threshold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequiredSigner {
    /// A signer the desktop can collect locally — hardware wallet,
    /// master signer, border-wallet recovery, or paste-an-xpub flow.
    /// The existing `SignModal` handles these.
    Local {
        fingerprint: Fingerprint,
        /// Display alias from `wallet.keys_aliases`, if known.
        name: Option<String>,
    },
    /// A signer that lives on a Keychain-registered phone. The desktop
    /// must open a `SigningSession` against the API and wait for the
    /// owner to approve / sign on their device.
    Keychain {
        fingerprint: Fingerprint,
        /// Backend `keys.id` — used to address the session's `target_key_id`.
        key_id: u64,
        /// Backend `users.id` of the owner — used by `ResolveSigners` to
        /// pick the right `SignerDevice`.
        owner_user_id: u64,
        /// Display name from the cube key list.
        name: String,
        /// `Some(email)` when the signer is a contact; `None` when the
        /// signer is the current user themselves.
        owner_email: Option<String>,
        /// Backend `contacts.id` when the signer is a contact; `None`
        /// for self-signers.
        contact_id: Option<u64>,
    },
}

impl RequiredSigner {
    pub fn fingerprint(&self) -> Fingerprint {
        match self {
            Self::Local { fingerprint, .. } => *fingerprint,
            Self::Keychain { fingerprint, .. } => *fingerprint,
        }
    }

    pub fn is_keychain(&self) -> bool {
        matches!(self, Self::Keychain { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassifyError {
    /// `CoincubeDescriptor::partial_spend_info` rejected the PSBT
    /// (malformed input/output counts, etc.).
    PsbtAnalysis(String),
    /// Every spending path is over-threshold or unreachable — the user
    /// has no work left to do, but the modal should still close cleanly
    /// instead of producing a "no signers required" mystery.
    NoSpendablePath,
}

impl std::fmt::Display for ClassifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PsbtAnalysis(s) => write!(f, "PSBT analysis failed: {}", s),
            Self::NoSpendablePath => {
                write!(f, "No spendable path available for this transaction")
            }
        }
    }
}

/// Pre-resolved mapping from descriptor fingerprint to the Keychain key
/// metadata needed to open a signing session. The caller builds this by
/// joining `GET /connect/cubes/{id}/vault.members` (key_id, contact_id)
/// against `GET /connect/cubes/{id}/keys` (fingerprint, owner_user_id).
///
/// Implemented as a hashmap so the classifier just looks up each
/// fingerprint encountered during descriptor traversal.
pub type KeychainSignerIndex = HashMap<Fingerprint, KeychainSignerInfo>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainSignerInfo {
    pub key_id: u64,
    pub owner_user_id: u64,
    pub name: String,
    pub owner_email: Option<String>,
    pub contact_id: Option<u64>,
}

/// Build a `KeychainSignerIndex` from the API-returned vault members and
/// cube keys. Members without a `key_id` (contact-only members) are
/// silently skipped — they don't correspond to a descriptor signer.
///
/// `self_user_id` is the authenticated viewer's user id (from
/// `client.get_user().id`) — used to distinguish self-signers
/// (`contact_id = None`) from contact-signers.
pub fn build_keychain_index(
    members: &[VaultMemberResponse],
    cube_keys: &[CubeKeyRaw],
    self_user_id: u64,
) -> KeychainSignerIndex {
    let mut index = HashMap::new();
    for member in members {
        let Some(key_id) = member.key_id else {
            continue;
        };
        // Join member.key_id → cube_keys[i].id to get fingerprint + owner.
        let Some(cube_key) = cube_keys.iter().find(|k| k.id == key_id) else {
            tracing::warn!(
                "Vault member {} references key_id {} that's not in the cube's key list — \
                 skipping (will be classified as Local; signing will likely fail).",
                member.id,
                key_id,
            );
            continue;
        };
        let Ok(fingerprint) = cube_key.fingerprint.parse::<Fingerprint>() else {
            tracing::warn!(
                "Cube key {} has unparseable fingerprint {:?} — skipping.",
                cube_key.id,
                cube_key.fingerprint,
            );
            continue;
        };
        let owner_user_id = cube_key.effective_owner_user_id();
        let (owner_email, contact_id) = if owner_user_id == self_user_id {
            (None, None)
        } else {
            // Contact: prefer the contact summary email when the API sent
            // it, else the owner_email field on the key itself.
            let email = member
                .contact
                .as_ref()
                .and_then(|c| c.contact_user.as_ref())
                .map(|u| u.email.clone())
                .or_else(|| {
                    (!cube_key.owner_email.is_empty()).then(|| cube_key.owner_email.clone())
                });
            (email, member.contact_id)
        };
        index.insert(
            fingerprint,
            KeychainSignerInfo {
                key_id,
                owner_user_id,
                name: cube_key.name.clone(),
                owner_email,
                contact_id,
            },
        );
    }
    // Diagnostic summary of the join: when a descriptor signer ends up
    // classified `local` despite being expected on a phone, this shows
    // whether the key was absent from the cube key list, present but
    // unreferenced by any vault member, or referenced by a member that
    // carries no `key_id`. Built lazily so it costs nothing unless the
    // `coincube_gui::signing` target is at DEBUG.
    if tracing::enabled!(target: "coincube_gui::signing", tracing::Level::DEBUG) {
        let cube_keys_summary = cube_keys
            .iter()
            .map(|k| format!("{}#{}", k.fingerprint, k.id))
            .collect::<Vec<_>>()
            .join(",");
        let members_summary = members
            .iter()
            .map(|m| {
                format!(
                    "m{}(key_id={:?},contact_id={:?})",
                    m.id, m.key_id, m.contact_id
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let index_summary = index
            .keys()
            .map(|fp| fp.to_string())
            .collect::<Vec<_>>()
            .join(",");
        tracing::debug!(
            target: "coincube_gui::signing",
            cube_keys = %cube_keys_summary,
            vault_members = %members_summary,
            keychain_index = %index_summary,
            "Built keychain signer index"
        );
    }
    index
}

/// Classify the still-required signers for the active spending path.
///
/// Strategy:
/// 1. Compute `PartialSpendInfo` on the PSBT. The primary path is always
///    available; recovery paths only show up when the input nSequence
///    matches their CSV timelock.
/// 2. Pick the path the transaction was built to spend through: an available
///    recovery path (under threshold, timelock-asc order) wins, since its
///    presence means the PSBT's nSequence was set for recovery; otherwise
///    fall back to the primary path. Preferring the always-available primary
///    path would misclassify every recovery spend.
/// 3. Enumerate the descriptor's path-info fingerprints, subtract the
///    set that already signed, classify each survivor against the
///    `keychain_index`.
pub fn classify_signers(
    psbt: &Psbt,
    descriptor: &CoincubeDescriptor,
    keychain_index: &KeychainSignerIndex,
    keys_aliases: &HashMap<Fingerprint, String>,
) -> Result<Vec<RequiredSigner>, ClassifyError> {
    let info = descriptor
        .partial_spend_info(psbt)
        .map_err(|e| ClassifyError::PsbtAnalysis(e.to_string()))?;

    let policy: CoincubePolicy = descriptor.policy();

    // Pick the path the user is actively trying to spend through.
    //
    // A recovery path appears in `PartialSpendInfo` only when the PSBT's
    // input nSequence satisfies its CSV timelock — i.e. the transaction was
    // deliberately built as a recovery spend (see `partial_spend_info_txin`).
    // When one is present it is the route the user is taking, so it wins over
    // the primary path even though the primary is still under threshold: the
    // primary path is *always* "available" (its branch carries no timelock),
    // so a "prefer primary while under threshold" rule would misclassify
    // every recovery spend against the primary signers and wrongly report
    // "no Keychain signers required" for a recovery that needs a Keychain
    // signer. Recovery paths are tried in ascending timelock order (BTreeMap
    // iteration); fall back to the primary path for ordinary spends, where no
    // recovery path is available.
    let recovery_choice = info
        .recovery_paths()
        .iter()
        .find(|(_, spend)| spend.sigs_count < spend.threshold);
    let primary_under_threshold = info.primary_path().sigs_count < info.primary_path().threshold;
    let (path_info, path_spend_info) = match recovery_choice {
        Some((timelock, spend)) => {
            let path = policy
                .recovery_paths()
                .get(timelock)
                .ok_or_else(|| {
                    ClassifyError::PsbtAnalysis(format!(
                        "Recovery path with timelock {} present in PSBT analysis but not in descriptor policy",
                        timelock,
                    ))
                })?;
            (path, spend)
        }
        None if primary_under_threshold => (policy.primary_path(), info.primary_path()),
        None => return Err(ClassifyError::NoSpendablePath),
    };

    let (threshold, origins) = path_info.thresh_origins();
    let already_signed = &path_spend_info.signed_pubkeys;

    // An M-of-N path only needs `remaining` more signatures, not every
    // unsigned key. Returning all unsigned origins makes the Keychain
    // flow open a session per extra signer and then block forever in
    // `check_all_done`, waiting on signatures the policy never needs.
    // `sigs_count` is the canonical "collected toward this path" count
    // used elsewhere (cf. the missing-signatures calc in view::psbt).
    let remaining = threshold.saturating_sub(path_spend_info.sigs_count);

    // Deterministically pick the `remaining` still-unsigned
    // fingerprints to address. `classify_signers` only runs from the
    // Keychain flow, so the user has already chosen to sign via
    // Keychain: when the path needs fewer signatures than there are
    // unsigned keys, prefer keeping Keychain signers in the kept set.
    // A blind fingerprint sort could otherwise truncate the only
    // Keychain signer away and make the modal falsely claim "no
    // Keychain signers required". Fingerprint-ascending is the
    // tiebreak within each class — cheap (32-bit) and intuitive in
    // the UI ("smallest" first); `truncate` caps the set.
    let mut unsigned: Vec<Fingerprint> = origins
        .into_keys()
        .filter(|fg| !already_signed.contains_key(fg))
        .collect();
    unsigned.sort_by(|a, b| {
        let a_kc = keychain_index.contains_key(a);
        let b_kc = keychain_index.contains_key(b);
        // `bool` orders false < true, so compare reversed to put
        // Keychain signers (true) first.
        b_kc.cmp(&a_kc).then_with(|| a.cmp(b))
    });
    unsigned.truncate(remaining);

    let mut required: Vec<RequiredSigner> = unsigned
        .into_iter()
        .map(|fg| {
            if let Some(info) = keychain_index.get(&fg) {
                RequiredSigner::Keychain {
                    fingerprint: fg,
                    key_id: info.key_id,
                    owner_user_id: info.owner_user_id,
                    name: info.name.clone(),
                    owner_email: info.owner_email.clone(),
                    contact_id: info.contact_id,
                }
            } else {
                RequiredSigner::Local {
                    fingerprint: fg,
                    name: keys_aliases.get(&fg).cloned(),
                }
            }
        })
        .collect();

    // Stable ordering — already fingerprint-sorted above; keep this so
    // the returned `RequiredSigner` order is guaranteed regardless of
    // the intermediate map.
    required.sort_by_key(|r| r.fingerprint());
    Ok(required)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::coincube::{
        VaultMemberContactSummary, VaultMemberContactUserSummary, VaultMemberKeySummary,
        VaultMemberResponse, VaultMemberRole,
    };

    fn cube_key(id: u64, fp: &str, owner_user_id: u64, name: &str) -> CubeKeyRaw {
        CubeKeyRaw {
            id,
            name: name.to_string(),
            xpub: String::new(),
            fingerprint: fp.to_string(),
            derivation_path: String::new(),
            network: "bitcoin".to_string(),
            status: "active".to_string(),
            primary_owner_id: owner_user_id,
            keychain_id: None,
            curve: String::new(),
            taproot: false,
            cube_id: 0,
            created_at: String::new(),
            updated_at: String::new(),
            owner_user_id,
            owner_email: String::new(),
            is_own_key: false,
            used_by_vault: false,
        }
    }

    fn vault_member(
        id: u64,
        key_id: Option<u64>,
        contact_id: Option<u64>,
        contact_email: Option<&str>,
    ) -> VaultMemberResponse {
        VaultMemberResponse {
            id,
            contact_id,
            key_id,
            role: VaultMemberRole::Keyholder,
            contact: contact_email.map(|e| VaultMemberContactSummary {
                id: contact_id.unwrap_or(0),
                contact_user: Some(VaultMemberContactUserSummary {
                    id: 0,
                    email: e.to_string(),
                }),
            }),
            key: key_id.map(|_| VaultMemberKeySummary {
                id: key_id.unwrap_or(0),
                name: String::new(),
                xpub: String::new(),
                derivation_path: String::new(),
            }),
            created_at: String::new(),
        }
    }

    #[test]
    fn build_index_classifies_self_vs_contact() {
        // Self: viewer.id == owner_user_id and contact_id is None.
        let cube_keys = vec![
            cube_key(1, "deadbeef", 100, "My HW"),
            cube_key(2, "cafef00d", 200, "Alice's HW"),
        ];
        let members = vec![
            vault_member(11, Some(1), None, None),
            vault_member(12, Some(2), Some(42), Some("alice@example.com")),
        ];
        let index = build_keychain_index(&members, &cube_keys, /* self_user_id = */ 100);
        assert_eq!(index.len(), 2);
        let self_entry = index
            .get(&"deadbeef".parse::<Fingerprint>().unwrap())
            .unwrap();
        assert_eq!(self_entry.owner_user_id, 100);
        assert!(self_entry.contact_id.is_none());
        assert!(self_entry.owner_email.is_none());

        let cafe: Fingerprint = "cafef00d".parse().unwrap();
        let contact_entry = index.get(&cafe).unwrap();
        assert_eq!(contact_entry.owner_user_id, 200);
        assert_eq!(contact_entry.contact_id, Some(42));
        assert_eq!(
            contact_entry.owner_email.as_deref(),
            Some("alice@example.com")
        );
    }

    #[test]
    fn build_index_skips_contact_only_members() {
        // A member with `key_id = None` is a contact-only attachment —
        // not a signer. The classifier should skip silently.
        let cube_keys = vec![cube_key(1, "deadbeef", 100, "My HW")];
        let members = vec![
            vault_member(11, Some(1), None, None),
            vault_member(12, None, Some(99), Some("bob@example.com")),
        ];
        let index = build_keychain_index(&members, &cube_keys, 100);
        assert_eq!(index.len(), 1);
        assert!(index.contains_key(&"deadbeef".parse::<Fingerprint>().unwrap()));
    }

    #[test]
    fn build_index_skips_dangling_member_key_id() {
        // Member references a key_id that isn't in cube_keys — log and skip.
        let cube_keys = vec![cube_key(1, "deadbeef", 100, "My HW")];
        let members = vec![
            vault_member(11, Some(1), None, None),
            vault_member(12, Some(999), Some(42), Some("alice@example.com")),
        ];
        let index = build_keychain_index(&members, &cube_keys, 100);
        assert_eq!(index.len(), 1);
    }

    // `or_d(pk(primary), and_v(v:pkh(recovery), older(10)))`: primary path is
    // the single key `f5acc2fd`; the recovery path is the single key
    // `8a64f2a9` behind a 10-block CSV. Reused from
    // `coincube_core::descriptors` analysis fixtures.
    const RECOVERY_DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";
    // Unsigned single-input/single-output PSBT for the descriptor above, with
    // a default (non-recovery) nSequence.
    const UNSIGNED_PSBT_B64: &str = "cHNidP8BAHECAAAAAUSHuliRtuCX1S6JxRuDRqDCKkWfKmWL5sV9ukZ/wzvfAAAAAAD9////AogTAAAAAAAAFgAUIxe7UY6LJ6y5mFBoWTOoVispDmdwFwAAAAAAABYAFKqO83TK+t/KdpAt21z2HGC7/Z2FAAAAAAABASsQJwAAAAAAACIAIIIySQjGCTeyx/rKUQx8qobjhJeNCiVCliBJPdyRX6XKAQVBIQI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDKxzZHapFNYASzIYkEdH9bJz6nnqUG3uBB8kiK1asmgiBgI2cqWpc9UAW2gZt2WkKjvi8KoMCui00pRlL6wG32uKDAz1rML9AAAAAG8AAAAiBgMLcbOxsfLe6+3r1UcjQo77HY0As8OKE4l37yj0/qhIyQyKZPKpAAAAAG8AAAAAAAA=";

    fn primary_fg() -> Fingerprint {
        "f5acc2fd".parse().unwrap()
    }
    fn recovery_fg() -> Fingerprint {
        "8a64f2a9".parse().unwrap()
    }

    /// Index marking the recovery key `8a64f2a9` as a Keychain signer.
    fn recovery_keychain_index() -> KeychainSignerIndex {
        let mut index = KeychainSignerIndex::new();
        index.insert(
            recovery_fg(),
            KeychainSignerInfo {
                key_id: 7,
                owner_user_id: 100,
                name: "testnet4 keychain".to_string(),
                owner_email: None,
                contact_id: None,
            },
        );
        index
    }

    #[test]
    fn classify_targets_recovery_keychain_for_recovery_spend() {
        use coincube_core::miniscript::bitcoin::Sequence;
        use std::str::FromStr;

        let desc = CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap();
        let mut psbt = Psbt::from_str(UNSIGNED_PSBT_B64).unwrap();
        // Build it as a recovery spend: nSequence satisfies the older(10) CSV,
        // so `partial_spend_info` surfaces the recovery path.
        psbt.unsigned_tx.input[0].sequence = Sequence::from_height(10);

        let required =
            classify_signers(&psbt, &desc, &recovery_keychain_index(), &HashMap::new()).unwrap();

        // Regression: the primary path is always "available", so the old
        // "prefer primary while under threshold" rule classified this against
        // `f5acc2fd` and reported no Keychain signers. The recovery keychain
        // signer is the one actually required.
        assert_eq!(required.len(), 1);
        assert!(required[0].is_keychain());
        assert_eq!(required[0].fingerprint(), recovery_fg());
    }

    #[test]
    fn classify_targets_primary_for_ordinary_spend() {
        use std::str::FromStr;

        let desc = CoincubeDescriptor::from_str(RECOVERY_DESC).unwrap();
        // Default nSequence — no recovery path is available, so the primary
        // path is the spend route.
        let psbt = Psbt::from_str(UNSIGNED_PSBT_B64).unwrap();

        let required =
            classify_signers(&psbt, &desc, &recovery_keychain_index(), &HashMap::new()).unwrap();

        assert_eq!(required.len(), 1);
        assert!(!required[0].is_keychain());
        assert_eq!(required[0].fingerprint(), primary_fg());
    }
}
