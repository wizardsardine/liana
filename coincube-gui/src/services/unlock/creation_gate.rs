//! "This Cube's backup is done" — the gate on Cube creation.
//!
//! # Why creation has to block on it now
//!
//! Before Tier 1, copying `<datadir>` copied the wallet: the seed file opened
//! anywhere with the PIN. That was bad security and a decent accidental
//! backup. Sealing the seed to an OS-keystore device secret (`ENCRYPTED_V3`)
//! removes the accidental backup along with the weakness — **a copied datadir
//! no longer opens**. Same for a passkey Cube, which has no seed file at all:
//! its seed is re-derived from the credential's PRF output, so the datadir
//! alone is inert without the credential.
//!
//! The passkey credential itself is Apple-ID-bound, not machine-bound
//! (decision 2026-08-04), so a passkey user's other Macs can re-derive the
//! seed. That widens recovery but does not soften this gate: iCloud Keychain
//! can be off, and the user is not asked.
//!
//! So the Recovery Kit stops being advisory. A user who creates a Cube, skips
//! the backup, and then loses the machine has lost the funds, and no support
//! action recovers them. That is only acceptable if creation makes it
//! impossible to skip by accident.
//!
//! # Reusing the duress gate's rule
//!
//! The "is this Cube's kit complete *for its shape*" question is already
//! answered by [`crate::app::state::connect::cube_backup_completeness`] for the
//! duress Vault gate: seed-only kits satisfy vaultless mnemonic Cubes, passkey
//! Cubes are descriptor-only, unknown status is never coerced to complete. This
//! module reuses that verbatim rather than growing a second, subtly different
//! rule that would drift.
//!
//! # The compound risk, for the support runbook
//!
//! A user can now lose access two ways: lose the machine, **or** lose the
//! Recovery Kit's own recovery password (already documented as
//! unrecoverable-by-design). Both need to be in the support copy.

use serde::{Deserialize, Serialize};

use crate::app::state::connect::{cube_backup_completeness, CubeBackupCompleteness};

/// Why a Cube's creation was allowed to complete without a demonstrated
/// backup. Persisted on the Cube so support can identify these Cubes later —
/// "did this user bypass the gate?" must be answerable from the datadir, not
/// from someone's memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationBackupBypass {
    /// Unix seconds when the user bypassed.
    pub at: i64,
    /// What the user was told they were accepting, verbatim, so a later
    /// support conversation is about the same thing they agreed to.
    pub acknowledged: String,
}

/// A Cube Recovery Kit created **during** creation, recorded on the Cube.
///
/// The creation gate is evaluated at open time with no server status (see
/// `gui::tab`: an offline user or an expired session must not be locked out of
/// a wallet whose material is right here). So a Kit that exists only on Connect
/// is invisible to the gate exactly when it matters. This is the local half:
/// proof that this client uploaded a Kit for this Cube before the Cube was
/// written, which is checkable with no network at all.
///
/// It records an **act**, not a live guarantee — same as
/// [`CreationBackupBypass`] and same as `CubeSettings::backed_up`, which
/// records that a phrase was shown and verified, not that the paper still
/// exists. A Kit deleted from Connect later is surfaced by the online
/// completeness check (`cube_backup_completeness`), which is where live truth
/// belongs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreationRecoveryKit {
    /// Unix seconds when the Kit was uploaded.
    pub at: i64,
    /// Connect's numeric id for this Cube — what a support conversation or a
    /// restore needs to find the Kit.
    pub cube_id: u64,
    /// Whether the uploaded Kit carried the encrypted seed half. A seed-only
    /// Kit is a complete backup for a Cube with no Vault, which every Cube is
    /// at creation.
    pub has_seed: bool,
}

/// Text the user must be shown — and must actively accept — to bypass.
pub const BYPASS_ACKNOWLEDGEMENT: &str =
    "I understand that without a backup, losing this computer means losing the \
     bitcoin in this Cube, and that nobody — including COINCUBE — can recover it.";

/// The creation-copy line that matters most, and the one users get wrong —
/// for a **PIN** Cube.
///
/// Every clause here is a statement about the `ENCRYPTED_V3` device secret, so
/// it is true for a PIN Cube and false for a passkey one. Use
/// [`not_a_backup_copy`] rather than reaching for this constant directly unless
/// the Cube's shape is known to be PIN.
pub const NOT_A_BACKUP_COPY: &str =
    "Copying the Coincube folder is not a backup. Part of this Cube's encryption key \
     is stored in this computer's system keychain and never leaves it, so the folder \
     will not open on another machine. Your Recovery Kit and written seed phrase are \
     the only way to regain access to this Cube on another computer.";

/// The same line for a **passkey** Cube.
///
/// The PIN version's reasoning does not transfer. A passkey Cube has no seed
/// file and no device secret, so "part of the encryption key is in this
/// computer's keychain" is simply not true of it — and the conclusion it
/// supports is wrong in the other direction too: the passkey *is* synced, so
/// the same Cube does open on another Mac signed into the same Apple ID
/// (`company-brain/decisions/2026-08-04-tenshu-passkey-apple-id-bound.md`).
///
/// What stays true is the part that matters: the folder is not the backup, and
/// iCloud Keychain sync is not a promise we are in a position to make — it can
/// be off, and we can neither require nor detect that. The Recovery Kit is the
/// one recovery a passkey user is owed (**I11**), so it is the one named here.
///
/// It must not overclaim in the *other* direction either. A passkey Cube has no
/// device secret, so folder plus Apple ID is genuinely sufficient to open it —
/// telling someone a copy "will not open anywhere" would invite them to treat
/// the folder as safe to hand around, leave on a shared machine, or sync to
/// somewhere public. Both halves have to be said: the copy is not a backup,
/// *and* it is not harmless.
pub const NOT_A_BACKUP_COPY_PASSKEY: &str =
    "Copying the Coincube folder is not a backup. The folder doesn't hold your passkey \
     — that lives in your Apple ID — so on its own it opens nothing. It will open on \
     another Mac signed in to the same Apple ID once iCloud Keychain has synced your \
     passkey there, which COINCUBE can't confirm for you, so treat the folder as \
     sensitive rather than safe to pass around. Your Recovery Kit is the backup that \
     works regardless: it carries this Cube's master seed, encrypted with a recovery \
     password only you know.";

/// The right "a folder copy is not a backup" line for this Cube's shape.
pub const fn not_a_backup_copy(is_passkey: bool) -> &'static str {
    if is_passkey {
        NOT_A_BACKUP_COPY_PASSKEY
    } else {
        NOT_A_BACKUP_COPY
    }
}

/// Verdict for the creation gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationGate {
    /// Backup demonstrated; creation may complete.
    Satisfied,
    /// Blocked, with a reason to show the user.
    Blocked(String),
    /// The user explicitly bypassed. Creation completes and the bypass is
    /// recorded on the Cube.
    Bypassed,
}

/// Whether this Cube may finish creation.
///
/// - `local_backup` is evidence, checkable **without a network**, that this
///   Cube can be recovered: `CubeSettings::backed_up` (the user wrote the seed
///   phrase down and confirmed it), or a [`CreationRecoveryKit`] uploaded
///   during creation. Either alone satisfies the gate — a written mnemonic
///   recovers a Cube just as well as a server kit does, and demanding a Connect
///   account to create a local wallet would be wrong.
/// - `kit` is the server-side Recovery Kit completeness for this Cube's shape,
///   `None` when the Cube isn't registered with Connect at all.
///
/// Fails **closed** on `Unknown`: a status probe that didn't answer is not
/// evidence of a backup.
pub fn evaluate(
    local_backup: bool,
    kit: Option<CubeBackupCompleteness>,
    bypass: Option<&CreationBackupBypass>,
) -> CreationGate {
    if bypass.is_some() {
        return CreationGate::Bypassed;
    }
    if local_backup {
        return CreationGate::Satisfied;
    }
    match kit {
        Some(CubeBackupCompleteness::Complete) => CreationGate::Satisfied,
        Some(CubeBackupCompleteness::MissingDescriptor) => CreationGate::Blocked(
            "Your seed phrase is backed up, but this Cube's Vault descriptor isn't. \
             Without it, the seed alone can't rebuild the Vault."
                .to_string(),
        ),
        Some(CubeBackupCompleteness::MissingSeed) => CreationGate::Blocked(
            "This Cube's Vault descriptor is backed up, but its seed phrase isn't.".to_string(),
        ),
        Some(CubeBackupCompleteness::NoKit) | None => CreationGate::Blocked(
            "Back up this Cube before you finish setting it up. Write down your seed \
             phrase, or save a Recovery Kit to COINCUBE Connect."
                .to_string(),
        ),
        Some(CubeBackupCompleteness::Unknown) => CreationGate::Blocked(
            "Couldn't confirm this Cube's backup. Retry, or write down your seed phrase \
             to continue."
                .to_string(),
        ),
    }
}

/// Convenience wrapper for a Cube whose shape is known locally.
pub fn evaluate_for_cube(
    cube: &crate::app::settings::CubeSettings,
    kit_halves: Option<(bool, bool)>,
) -> CreationGate {
    // Cubes created before the gate existed are never held to it — see
    // `CubeSettings::creation_backup_required`.
    if !cube.creation_backup_required {
        return CreationGate::Satisfied;
    }
    let kit = Some(cube_backup_completeness(
        Some(cube.vault_wallet_id.is_some()),
        kit_halves,
    ));
    // Local, offline-checkable evidence: a written-down phrase, or a Recovery
    // Kit this client uploaded during creation. `kit_halves` is `None` at open
    // time by design, so a Kit that exists only on the server cannot answer
    // this question — see [`CreationRecoveryKit`].
    let local_backup = cube.backed_up || cube.creation_recovery_kit.is_some();
    evaluate(local_backup, kit, cube.creation_backup_bypass.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use CubeBackupCompleteness as C;

    fn bypass() -> CreationBackupBypass {
        CreationBackupBypass {
            at: 1_700_000_000,
            acknowledged: BYPASS_ACKNOWLEDGEMENT.to_string(),
        }
    }

    /// A Cube that has just been created must be openable.
    ///
    /// The regression guard for a bricking bug: creation armed the gate while
    /// no backup step ran, so every new Cube was `Blocked` at open — and the
    /// only way to satisfy the gate (Settings → Backup Master Seed) is behind
    /// the Cube being open. Create a Cube, lose it forever.
    #[test]
    fn a_cube_straight_out_of_creation_can_be_opened() {
        use crate::app::settings::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        // Exactly what `home.rs::finalize_cube_creation` persists now that the
        // gate is armed: master signer set, gate armed, and the backup the
        // wizard just demonstrated. No kit — a local Cube need not have one.
        let mut cube = CubeSettings::new("Fresh".to_string(), Network::Bitcoin);
        cube.creation_backup_required = true;
        cube.backed_up = true;
        assert_eq!(
            evaluate_for_cube(&cube, None),
            CreationGate::Satisfied,
            "a newly created Cube cannot be opened — it is bricked at birth"
        );

        // …and the restore paths, which reconstruct through `new_*` and are
        // never gated.
        let restored = CubeSettings::new("Restored".to_string(), Network::Bitcoin);
        assert!(!restored.creation_backup_required);
        assert_eq!(evaluate_for_cube(&restored, None), CreationGate::Satisfied);
    }

    /// The creation-time backup step exists, which is what makes arming the
    /// gate safe.
    ///
    /// **This test used to be its own inverse.** As
    /// `arming_the_gate_without_a_creation_backup_step_would_brick_new_cubes`
    /// it asserted `Blocked` and said *"if this no longer blocks, a
    /// creation-time backup path exists and `home.rs` should arm the flag"* —
    /// the deadlock written down so nobody armed the flag before there was a
    /// way to satisfy it. Unit 3a built that path and 3b armed the flag, so it
    /// is **inverted here, not deleted**: it now pins the property that made
    /// arming safe.
    ///
    /// That property is not "the flag is set". It is that
    /// `home.rs::finalize_cube_creation` sets the flag and one of the two
    /// pieces of evidence in the **same** write, so no Cube can reach disk
    /// armed-but-unsatisfiable. Both exits from the backup step are checked
    /// below, and so is the shape that would brick — which stays `Blocked`,
    /// because that is precisely what creation must never write.
    #[test]
    fn arming_the_gate_is_safe_because_creation_always_produces_evidence() {
        use crate::app::settings::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        // Exit 1: the user typed the challenge words back correctly.
        let mut verified = CubeSettings::new("Verified".to_string(), Network::Bitcoin);
        verified.creation_backup_required = true;
        verified.backed_up = true;
        assert_eq!(
            evaluate_for_cube(&verified, None),
            CreationGate::Satisfied,
            "a Cube that completed the creation backup step must open"
        );

        // Exit 2: the user accepted the bypass acknowledgement.
        let mut bypassed = CubeSettings::new("Bypassed".to_string(), Network::Bitcoin);
        bypassed.creation_backup_required = true;
        bypassed.creation_backup_bypass = Some(bypass());
        assert_eq!(
            evaluate_for_cube(&bypassed, None),
            CreationGate::Bypassed,
            "a Cube whose owner accepted the bypass must still open"
        );

        // The shape creation must never write: armed, with neither piece of
        // evidence. Still blocked — arming alone is not what makes a Cube
        // openable, and if this ever stops blocking the gate has become
        // decorative.
        let mut armed_only = CubeSettings::new("Armed only".to_string(), Network::Bitcoin);
        armed_only.creation_backup_required = true;
        assert!(
            matches!(
                evaluate_for_cube(&armed_only, None),
                CreationGate::Blocked(_)
            ),
            "the gate must still block a Cube with no backup evidence — \
             `finalize_cube_creation` is what guarantees this shape is never persisted"
        );
    }

    #[test]
    fn creation_cannot_complete_without_a_kit() {
        assert!(matches!(
            evaluate(false, Some(C::NoKit), None),
            CreationGate::Blocked(_)
        ));
        assert!(matches!(
            evaluate(false, None, None),
            CreationGate::Blocked(_)
        ));
    }

    #[test]
    fn a_written_seed_phrase_is_a_backup() {
        // The gate must not require a Connect account to create a local wallet.
        assert_eq!(evaluate(true, None, None), CreationGate::Satisfied);
        assert_eq!(
            evaluate(true, Some(C::NoKit), None),
            CreationGate::Satisfied
        );
    }

    #[test]
    fn a_seed_only_kit_satisfies_a_seed_only_cube() {
        // Partial kits stay valid — `cube_backup_completeness` already resolves
        // a vaultless Cube with a seed half to `Complete`, and the gate must
        // not second-guess it.
        let verdict = cube_backup_completeness(Some(false), Some((true, false)));
        assert_eq!(verdict, C::Complete);
        assert_eq!(
            evaluate(false, Some(verdict), None),
            CreationGate::Satisfied
        );
    }

    #[test]
    fn unknown_status_fails_closed() {
        // A probe that didn't answer is not evidence of a backup.
        assert!(matches!(
            evaluate(false, Some(C::Unknown), None),
            CreationGate::Blocked(_)
        ));
    }

    #[test]
    fn a_bypass_is_recorded_and_lets_creation_through() {
        let b = bypass();
        assert_eq!(
            evaluate(false, Some(C::NoKit), Some(&b)),
            CreationGate::Bypassed
        );
        // What support needs: the acknowledgement is stored verbatim.
        assert_eq!(b.acknowledged, BYPASS_ACKNOWLEDGEMENT);
    }

    #[test]
    fn partial_kits_get_a_reason_that_names_the_missing_half() {
        // A blocked user has to be told what to go and do.
        let CreationGate::Blocked(msg) = evaluate(false, Some(C::MissingDescriptor), None) else {
            panic!("expected Blocked");
        };
        assert!(msg.contains("descriptor"), "{}", msg);

        let CreationGate::Blocked(msg) = evaluate(false, Some(C::MissingSeed), None) else {
            panic!("expected Blocked");
        };
        assert!(msg.contains("seed phrase"), "{}", msg);
    }

    #[test]
    fn cubes_that_predate_the_gate_are_never_blocked() {
        // The gate must not retroactively lock people out of Cubes they have
        // been using. Only Cubes created under it are held to it.
        use crate::app::settings::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        let legacy = CubeSettings::new("Legacy".to_string(), Network::Bitcoin);
        assert!(!legacy.creation_backup_required);
        assert_eq!(evaluate_for_cube(&legacy, None), CreationGate::Satisfied);

        let mut fresh = CubeSettings::new("Fresh".to_string(), Network::Bitcoin);
        fresh.creation_backup_required = true;
        assert!(matches!(
            evaluate_for_cube(&fresh, None),
            CreationGate::Blocked(_)
        ));
    }

    #[test]
    fn a_post_creation_vault_addition_does_not_re_gate_the_cube() {
        // Matches the duress-gate precedent: adding a Vault later surfaces an
        // incomplete-kit row, it does not block the Cube. Once the seed is
        // backed up, the gate stays satisfied.
        use crate::app::settings::CubeSettings;
        use crate::app::settings::WalletId;
        use coincube_core::miniscript::bitcoin::Network;

        let mut cube = CubeSettings::new("With Vault".to_string(), Network::Bitcoin);
        cube.creation_backup_required = true;
        cube.backed_up = true;
        assert_eq!(evaluate_for_cube(&cube, None), CreationGate::Satisfied);

        cube.vault_wallet_id = Some(WalletId::new("desc".to_string(), Some(1)));
        assert_eq!(evaluate_for_cube(&cube, None), CreationGate::Satisfied);
    }

    #[test]
    fn the_copy_says_a_folder_copy_is_not_a_backup() {
        // This is the sentence that decides whether the support queue fills up.
        for copy in [NOT_A_BACKUP_COPY, NOT_A_BACKUP_COPY_PASSKEY] {
            assert!(copy.contains("not a backup"));
            assert!(copy.contains("Recovery Kit"));
        }
        assert!(BYPASS_ACKNOWLEDGEMENT.contains("nobody"));
    }

    /// The passkey line must not inherit the PIN line's device-secret
    /// reasoning, and must not claim the passkey stays on this machine — the
    /// 2026-08-01 device-bound decision is superseded and the code never
    /// implemented it.
    #[test]
    fn the_passkey_copy_makes_no_device_bound_claim() {
        let copy = NOT_A_BACKUP_COPY_PASSKEY.to_lowercase();
        for false_claim in [
            "system keychain",
            "never leaves",
            "written seed phrase",
            "will not open on another machine",
        ] {
            assert!(
                !copy.contains(false_claim),
                "the passkey copy still says {:?}, which is not true of a passkey Cube",
                false_claim
            );
        }
        // And it must say the thing that *is* the promise.
        assert!(copy.contains("icloud keychain"));
        assert!(copy.contains("master seed"));
    }

    /// The mirror of the test above. Correcting a device-bound overclaim is
    /// easy to overcorrect into "a folder copy opens nothing", which is just as
    /// false and more dangerous: a passkey Cube has no device secret, so folder
    /// plus Apple ID really does open it. A user who believes the copy is inert
    /// will treat it as safe to share.
    #[test]
    fn the_passkey_copy_does_not_call_a_folder_copy_harmless() {
        let copy = NOT_A_BACKUP_COPY_PASSKEY.to_lowercase();
        for overclaim in [
            "will not open anywhere",
            "opens nothing anywhere",
            "cannot be opened",
            "useless to anyone",
        ] {
            assert!(
                !copy.contains(overclaim),
                "the passkey copy says {:?}, which invites a user to hand the folder \
                 around — folder plus Apple ID is enough to open a passkey Cube",
                overclaim
            );
        }
        // It has to name the case where the copy *does* open, and say the folder
        // is still worth protecting.
        assert!(
            copy.contains("same apple id"),
            "the copy never admits the folder opens on a same-Apple-ID Mac: {}",
            copy
        );
        assert!(
            copy.contains("sensitive"),
            "the copy never tells the user to protect the folder: {}",
            copy
        );
    }

    /// A Recovery Kit made during creation is a **backup**, and satisfies the
    /// gate on its own — including with no server status at all, which is the
    /// only way the gate is ever evaluated at open time (`gui::tab` passes
    /// `None` so an offline user is never locked out).
    #[test]
    fn a_creation_recovery_kit_satisfies_the_gate_offline() {
        use crate::app::settings::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        let mut cube = CubeSettings::new("Kitted".to_string(), Network::Bitcoin);
        cube.creation_backup_required = true;
        // The shape the Kit branch writes: no written phrase, no bypass.
        cube.backed_up = false;
        cube.creation_recovery_kit = Some(CreationRecoveryKit {
            at: 1_700_000_000,
            cube_id: 42,
            has_seed: true,
        });

        assert_eq!(
            evaluate_for_cube(&cube, None),
            CreationGate::Satisfied,
            "a Cube whose Kit was made at creation must open with no network"
        );
        // And it is not a bypass — the distinction is what support reads.
        assert!(cube.creation_backup_bypass.is_none());
        assert_ne!(evaluate_for_cube(&cube, None), CreationGate::Bypassed);
    }

    /// The three creation exits stay distinguishable on the Cube itself.
    #[test]
    fn the_three_creation_exits_are_told_apart() {
        use crate::app::settings::CubeSettings;
        use coincube_core::miniscript::bitcoin::Network;

        let armed = |name: &str| {
            let mut c = CubeSettings::new(name.to_string(), Network::Bitcoin);
            c.creation_backup_required = true;
            c
        };

        // 1. Wrote the phrase down.
        let mut phrase = armed("Phrase");
        phrase.backed_up = true;
        assert_eq!(evaluate_for_cube(&phrase, None), CreationGate::Satisfied);

        // 2. Made a Recovery Kit.
        let mut kit = armed("Kit");
        kit.creation_recovery_kit = Some(CreationRecoveryKit {
            at: 1,
            cube_id: 7,
            has_seed: true,
        });
        assert_eq!(evaluate_for_cube(&kit, None), CreationGate::Satisfied);

        // 3. Took the acknowledgement.
        let mut bypassed = armed("Bypassed");
        bypassed.creation_backup_bypass = Some(bypass());
        assert_eq!(evaluate_for_cube(&bypassed, None), CreationGate::Bypassed);

        // None of the three — still blocked. Creation must never write this.
        assert!(matches!(
            evaluate_for_cube(&armed("Nothing"), None),
            CreationGate::Blocked(_)
        ));
    }

    #[test]
    fn the_selector_matches_the_shape() {
        assert_eq!(not_a_backup_copy(false), NOT_A_BACKUP_COPY);
        assert_eq!(not_a_backup_copy(true), NOT_A_BACKUP_COPY_PASSKEY);
    }
}
