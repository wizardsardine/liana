//! "This Cube's backup is done" — the gate on Cube creation.
//!
//! # Why creation has to block on it now
//!
//! Before Tier 1, copying `<datadir>` copied the wallet: the seed file opened
//! anywhere with the PIN. That was bad security and a decent accidental
//! backup. Sealing the seed to an OS-keystore device secret (`ENCRYPTED_V3`)
//! removes the accidental backup along with the weakness — **a copied datadir
//! no longer opens**. Same for a device-bound passkey Cube, whose credential
//! never leaves the machine by design.
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

/// Text the user must be shown — and must actively accept — to bypass.
pub const BYPASS_ACKNOWLEDGEMENT: &str =
    "I understand that without a backup, losing this computer means losing the \
     bitcoin in this Cube, and that nobody — including COINCUBE — can recover it.";

/// The creation-copy line that matters most, and the one users get wrong.
pub const NOT_A_BACKUP_COPY: &str =
    "Copying the Coincube folder is not a backup. Part of this Cube's encryption key \
     is stored in this computer's system keychain and never leaves it, so the folder \
     will not open on another machine. Your Recovery Kit — or your written seed \
     phrase — is the only way back in.";

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
/// - `local_seed_backed_up` is `CubeSettings::backed_up` — the user wrote the
///   seed phrase down and confirmed it. That alone satisfies the gate: a
///   written mnemonic recovers a Cube just as well as a server kit does, and
///   demanding a Connect account to create a local wallet would be wrong.
/// - `kit` is the server-side Recovery Kit completeness for this Cube's shape,
///   `None` when the Cube isn't registered with Connect at all.
///
/// Fails **closed** on `Unknown`: a status probe that didn't answer is not
/// evidence of a backup.
pub fn evaluate(
    local_seed_backed_up: bool,
    kit: Option<CubeBackupCompleteness>,
    bypass: Option<&CreationBackupBypass>,
) -> CreationGate {
    if bypass.is_some() {
        return CreationGate::Bypassed;
    }
    if local_seed_backed_up {
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
            "This Cube's Vault descriptor is backed up, but its seed phrase isn't."
                .to_string(),
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
        cube.is_passkey_cube(),
        kit_halves,
    ));
    evaluate(cube.backed_up, kit, cube.creation_backup_bypass.as_ref())
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

    #[test]
    fn creation_cannot_complete_without_a_kit() {
        assert!(matches!(
            evaluate(false, Some(C::NoKit), None),
            CreationGate::Blocked(_)
        ));
        assert!(matches!(evaluate(false, None, None), CreationGate::Blocked(_)));
    }

    #[test]
    fn a_written_seed_phrase_is_a_backup() {
        // The gate must not require a Connect account to create a local wallet.
        assert_eq!(evaluate(true, None, None), CreationGate::Satisfied);
        assert_eq!(evaluate(true, Some(C::NoKit), None), CreationGate::Satisfied);
    }

    #[test]
    fn a_seed_only_kit_satisfies_a_seed_only_cube() {
        // Partial kits stay valid — `cube_backup_completeness` already resolves
        // a vaultless mnemonic Cube with a seed half to `Complete`, and the
        // gate must not second-guess it.
        let verdict = cube_backup_completeness(Some(false), false, Some((true, false)));
        assert_eq!(verdict, C::Complete);
        assert_eq!(evaluate(false, Some(verdict), None), CreationGate::Satisfied);
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
        assert_eq!(evaluate(false, Some(C::NoKit), Some(&b)), CreationGate::Bypassed);
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
        use coincube_core::miniscript::bitcoin::Network;
        use crate::app::settings::WalletId;

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
        assert!(NOT_A_BACKUP_COPY.contains("not a backup"));
        assert!(NOT_A_BACKUP_COPY.contains("Recovery Kit"));
        assert!(BYPASS_ACKNOWLEDGEMENT.contains("nobody"));
    }
}
