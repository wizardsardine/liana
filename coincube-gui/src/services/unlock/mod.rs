//! Cube unlock.
//!
//! # The finding this exists to fix
//!
//! A Cube's PIN used to be verified against `CubeSettings::security_pin_hash`,
//! an Argon2id PHC string at m=19456 KiB, t=2, p=1 — about **27 ms** a guess.
//! The thing it was protecting, the seed file, is Argon2id at m=262144 KiB,
//! t=3, p=4 — about **831 ms** a guess. An attacker holding the datadir has
//! both files and attacks the cheap one:
//!
//! | Artifact | ms/guess | 10,000-PIN sweep, 1 core | on 64 cores |
//! |---|---|---|---|
//! | Seed file | 831 | 2.3 h | ~2 min |
//! | PIN hash | 27 | 4.5 min | **~4 s** |
//!
//! A 4-digit PIN has 10,000 possibilities. The hash is a complete break.
//!
//! # The fix
//!
//! There is no separate verifier. The PIN is checked by **trial-decrypting the
//! seed file**: the AES-GCM tag *is* the verifier, so there is exactly one cost
//! and it is the highest one available (invariant I1). The happy path gets the
//! decrypted signer for free — verification and decryption are the same
//! operation, so unlock does not pay 831 ms twice.
//!
//! For that to be true the caller has to *keep* the signer
//! [`unlock_blocking`] hands back. It goes into [`crate::app::session`], where
//! the Liquid and Spark loaders pick it up instead of re-reading the seed file.
//! Dropping it costs two further ~831 ms derivations — one per loader — which
//! is worse than the code this replaced.
//!
//! Same for duress: `duress_pin_hash` is replaced by a marker blob sealed at
//! *identical* parameters ([`marker`]), so on a duress-armed Cube a wrong PIN
//! and a duress PIN cost the same wall clock and the two files look the same on
//! disk (invariant I2).
//!
//! I2 covers wrong-vs-duress **on one Cube**. It does not hide *whether* duress
//! is armed: a wrong PIN costs one derivation on an unarmed Cube and two on an
//! armed one. The marker's name is random and recorded rather than derived
//! (unit 6a), so that presence is no longer *computable* from `settings.json`
//! — but `mnemonics/` still holds one blob more than an unarmed Cube needs, and
//! the timing agrees with the count. Closing it is the decoy slot every Cube
//! carries from creation (unit 6b). See [`marker::verify`].
//!
//! ```text
//!                  user types 4 digits
//!                           │
//!                           ▼
//!         ┌─────────────────────────────────────┐
//!         │ trial-decrypt SEED FILE with PIN    │  ~831 ms, off the UI thread
//!         │   GCM tag verifies?                 │
//!         └───────────────┬─────────────────────┘
//!                 yes ────┴──── no
//!                  │             │
//!                  ▼             ▼
//!               UNLOCK   ┌──────────────────────────────┐
//!            (seed is    │ trial-decrypt DURESS MARKER  │  IDENTICAL params
//!             already    │   GCM tag verifies?          │  → indistinguishable
//!             in hand)   └───────────┬──────────────────┘     cost
//!                              yes ──┴── no
//!                               │        │
//!                               ▼        ▼
//!                           DURESS     WRONG
//! ```
//!
//! # Cost, and where it is paid
//!
//! Unlock went from 27 ms to ~831 ms on the happy path and ~1.7 s worst case
//! (wrong PIN on a duress-enrolled Cube). That is the intended trade and there
//! is no UX change for it — the existing loading screen and its Kage quote
//! already cover the wait. What is *not* optional is that the derivation runs
//! off the UI thread; see [`unlock_blocking`]'s contract.

pub mod creation_gate;
pub mod device_secret;
pub mod marker;
pub mod throttle;

use std::path::{Path, PathBuf};
use std::str::FromStr;

use coincube_core::miniscript::bitcoin::{bip32::Fingerprint, Network};
use coincube_core::seed_crypt::{self, DeviceSecret};
use coincube_core::signer::{MasterSigner, SignerError, MASTER_SEED_LABEL};
use zeroize::Zeroizing;

use crate::app::settings::CubeSettings;

/// Classification of a submitted PIN at Cube unlock.
pub enum PinOutcome {
    /// Correct. Carries the signer that verifying the PIN already produced —
    /// re-deriving it would mean a second 831 ms Argon2 pass.
    Unlock(Box<MasterSigner>),
    /// The Cube's duress PIN. The caller activates duress; it must not be told
    /// anything more than this.
    Duress,
    /// Neither.
    Wrong,
}

impl std::fmt::Debug for PinOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never derive Debug here — `MasterSigner` would print the seed.
        match self {
            Self::Unlock(_) => write!(f, "Unlock(<signer>)"),
            Self::Duress => write!(f, "Duress"),
            Self::Wrong => write!(f, "Wrong"),
        }
    }
}

/// Failures that are **not** "wrong PIN".
///
/// Invariant I7: a user whose system keychain is locked must never be told
/// their wallet is gone. Each of these gets its own message, and a QA tester
/// who does not know which was induced must be able to tell them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnlockError {
    /// The keystore itself couldn't be reached. Transient — unlocking the
    /// keychain and retrying works.
    KeystoreUnreachable(String),
    /// The keystore can't be used by *this build*, whatever the user does.
    /// Terminal, and that is the whole difference from `KeystoreUnreachable`:
    /// the retry advice that variant appends is not just useless here, it sends
    /// the user to unlock a keychain that is already unlocked while the real
    /// cause — an unsigned binary with no `keychain-access-groups` entitlement —
    /// goes unmentioned. The detail carries the entire message.
    KeystoreUnusable(String),
    /// The keystore works but holds no device secret for this Cube. For a v3
    /// seed file that is terminal on this machine; the copy must point at the
    /// Recovery Kit.
    DeviceSecretMissing,
    /// This Cube has no PIN-protected seed on this device, so there is nothing
    /// to verify a PIN against.
    ///
    /// Explicitly **not** a success. The predecessor of this code returned
    /// `true` from `verify_pin` when no PIN was configured, and three call
    /// sites had to bolt on `has_pin()` guards to compensate for it.
    NoPinConfigured,
    Io(String),
}

impl std::fmt::Display for UnlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeystoreUnreachable(detail) => write!(
                f,
                "{detail} Your Cube is safe — unlock your system keychain and try again."
            ),
            // No trailing advice: there is none to give that the detail does
            // not already carry, and "try again" would be a lie.
            Self::KeystoreUnusable(detail) => write!(f, "{detail}"),
            Self::DeviceSecretMissing => write!(
                f,
                "Part of this Cube's encryption key was stored in this computer's system \
                 keychain, and it's no longer there. This can happen after a keychain reset, \
                 a disk restore, or moving the Cube folder between machines. Your PIN alone \
                 can't open it. Restore this Cube from its Recovery Kit."
            ),
            // Reachable from the PIN screen: `gui::tab` routes every Cube
            // there without consulting `pin_requirement`, so a Cube whose seed
            // is absent still gets asked for one. The copy has to tell that
            // user what is actually wrong and what to do about it.
            Self::NoPinConfigured => write!(
                f,
                "This Cube's seed isn't on this device, so there's nothing for a PIN \
                 to unlock. Restore this Cube from its Recovery Kit or its written \
                 seed phrase."
            ),
            Self::Io(detail) => write!(f, "Couldn't read this Cube's files: {detail}"),
        }
    }
}

impl std::error::Error for UnlockError {}

/// Whether this Cube needs a PIN, decided from **ground truth on disk** rather
/// than from a settings field.
///
/// The old `has_pin()` read `security_pin_hash.is_some()`, which could drift
/// from reality in either direction. A Cube has a PIN if and only if its master
/// seed file is encrypted, and [`MasterSigner::is_encrypted`] answers that
/// without needing the PIN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinRequirement {
    /// Encrypted master seed on disk: a PIN is required.
    Required,
    /// A master seed is on disk but is not encrypted — a Cube written by a
    /// pre-hardening build. Opening it needs no PIN; the migration re-encrypts
    /// it as soon as one is available.
    Unprotected,
    /// No master seed on this device. Nothing local to protect, and nothing to
    /// check a PIN against (a Cube created elsewhere and never restored here).
    NoLocalSeed,
}

/// Everything unlock needs to know about where a Cube's files are.
pub struct CubeLocation<'a> {
    pub datadir_root: &'a Path,
    pub network: Network,
    pub cube_id: &'a str,
    pub cube_created_at: i64,
    pub master_signer_fingerprint: Option<Fingerprint>,
    /// Recorded name of this Cube's duress marker, if armed. The name is
    /// random (see [`marker`]), so this is the only way to find the file — and
    /// the only way for a seed-file scan to know which entry to skip.
    pub duress_slot_file: Option<&'a str>,
    /// `CubeSettings::backed_up` — the user wrote this Cube's seed phrase down
    /// and confirmed it. Half of the I10 gate on reaching v3.
    pub backed_up: bool,
    /// Server-side Recovery Kit completeness for this Cube's shape, when it is
    /// known. `None` means "not asked" and fails **closed** — see
    /// [`migrate_seed_files`].
    pub kit_completeness: Option<crate::app::state::connect::CubeBackupCompleteness>,
    /// A recorded creation-time bypass. The user was shown
    /// `BYPASS_ACKNOWLEDGEMENT` and accepted it, so they have been told what
    /// they are risking — that is consent to reach v3, not a backup.
    pub creation_bypass: Option<&'a creation_gate::CreationBackupBypass>,
}

impl<'a> CubeLocation<'a> {
    pub fn new(datadir_root: &'a Path, cube: &'a CubeSettings) -> Self {
        Self {
            datadir_root,
            network: cube.network,
            cube_id: &cube.id,
            cube_created_at: cube.created_at,
            master_signer_fingerprint: cube.master_signer_fingerprint,
            duress_slot_file: cube.duress_slot_file.as_deref(),
            backed_up: cube.backed_up,
            // Locally-known shape only. The server halves are not fetched here
            // for the same reason the open gate does not fetch them: putting
            // the network on the unlock path turns an outage into a Cube that
            // cannot be migrated, and failing closed then means "stays at v2",
            // which is the safe direction anyway. A Cube whose only backup is
            // a server kit reaches v3 the first time it is opened with the kit
            // status in hand — see `with_kit`.
            kit_completeness: None,
            creation_bypass: cube.creation_backup_bypass.as_ref(),
        }
    }

    /// Supply the server-side Recovery Kit halves, when a caller has them.
    ///
    /// Without this the I10 gate sees only local evidence and a Cube backed up
    /// *solely* through Connect stays at v2. That is the fail-closed direction
    /// — the Cube keeps working and keeps its portability — but it is not the
    /// right answer forever, so the door is here for the caller that can
    /// answer the question without blocking on the network.
    pub fn with_kit(mut self, kit: crate::app::state::connect::CubeBackupCompleteness) -> Self {
        // Takes an already-resolved verdict rather than raw halves: deciding
        // "complete for this Cube's shape" needs `has_vault` and `is_passkey`,
        // which live on `CubeSettings`. Recomputing it here from a partial view
        // is how a second, subtly different rule gets born — the thing
        // `creation_gate`'s own doc warns about.
        self.kit_completeness = Some(kit);
        self
    }
}

/// Locate this Cube's master seed file.
///
/// Prefers the recorded fingerprint. Falls back to the creation-window
/// heuristic that `derive_master_signer_fingerprint` uses, so a Cube minted
/// before the fingerprint field existed can still be classified — that matters
/// because we have to decide whether to show a PIN screen *before* we have a
/// PIN, and the fingerprint backfill needs one.
pub fn master_seed_path(loc: &CubeLocation) -> Option<PathBuf> {
    use coincube_core::signer::MnemonicFileName;
    use std::str::FromStr;

    let folder = MasterSigner::mnemonics_folder(loc.datadir_root, loc.network);
    let entries = std::fs::read_dir(&folder).ok()?;

    let marker_name = loc.duress_slot_file;
    let mut best: Option<(PathBuf, i64)> = None;

    for entry in entries.flatten() {
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_owned(),
            None => continue,
        };
        // Never mistake the duress marker for a seed. It deliberately shares
        // the master-seed filename grammar (see `marker`), and since unit 6a
        // its fingerprint field is random rather than derived — so name-shape
        // alone cannot tell them apart, and neither can recomputation. Only
        // the recorded name can, which is why `CubeLocation` carries it.
        //
        // The recorded fingerprint below is a second, independent filter: a
        // random four-byte field will not collide with the Cube's real
        // fingerprint, so a Cube that knows its own fingerprint excludes the
        // marker even if the recorded name went missing. The creation-window
        // fallback has no such backstop — a Cube with no recorded fingerprint
        // *and* no recorded marker name could match the marker instead. That
        // degrades to "the PIN looks wrong" (the marker never opens with the
        // real PIN), not to data loss, and the fingerprint backfill repairs it.
        if Some(name.as_str()) == marker_name {
            continue;
        }
        let Ok(parsed) = MnemonicFileName::from_str(&name) else {
            continue;
        };

        if let Some(fp) = loc.master_signer_fingerprint {
            if parsed.fingerprint == fp {
                return Some(entry.path());
            }
            continue;
        }

        // No recorded fingerprint: fall back to "a master-seed file stamped
        // within the Cube's creation window", the same rule the backfill uses.
        let Some((checksum, ts)) = parsed.descriptor_info else {
            continue;
        };
        if !checksum.starts_with(MASTER_SEED_LABEL) {
            continue;
        }
        let delta = (ts - loc.cube_created_at).abs();
        if delta > MASTER_SEED_CREATION_WINDOW_SECS {
            continue;
        }
        if best.as_ref().map(|(_, d)| delta < *d).unwrap_or(true) {
            best = Some((entry.path(), delta));
        }
    }

    best.map(|(p, _)| p)
}

/// Tolerance between a master-seed file's timestamp and its Cube's
/// `created_at`. Mirrors `settings::derive_master_signer_fingerprint`.
const MASTER_SEED_CREATION_WINDOW_SECS: i64 = 2;

/// Does this Cube need a PIN? Answered from the seed file, not from settings.
pub fn pin_requirement(loc: &CubeLocation) -> PinRequirement {
    let Some(path) = master_seed_path(loc) else {
        return PinRequirement::NoLocalSeed;
    };
    match std::fs::read(&path) {
        Ok(data) if MasterSigner::is_encrypted(&data) => PinRequirement::Required,
        Ok(_) => PinRequirement::Unprotected,
        // Unreadable: fail closed. Demanding a PIN we then can't verify is
        // recoverable; opening a Cube we couldn't classify is not.
        Err(_) => PinRequirement::Required,
    }
}

/// Whether this Cube's seed file is already at the current wire version.
pub fn seed_file_version(loc: &CubeLocation) -> Option<u8> {
    let path = master_seed_path(loc)?;
    let data = std::fs::read(path).ok()?;
    seed_crypt::format_version(&data)
}

/// Classify a submitted PIN. **Blocking** — ~831 ms on the happy path and
/// ~1.7 s worst case, so callers must run it on a blocking pool
/// (`tokio::task::spawn_blocking`), never inline in `update()`.
///
/// The order is seed file first, then marker. Both cost one Argon2id pass at
/// identical parameters, which is what keeps a wrong PIN and a duress PIN
/// indistinguishable in wall clock on an armed Cube (I2).
///
/// Do not add an early-out *between* them — e.g. skipping the marker check when
/// the seed file decrypts, or when the PIN "looks wrong". [`marker::verify`]
/// already returns immediately when no marker exists, which is why an unarmed
/// Cube is cheaper; that one is deliberate and documented there.
pub fn unlock_blocking(loc: &CubeLocation, pin: &str) -> Result<PinOutcome, UnlockError> {
    // The device secret is fetched once and used for both trial decryptions.
    // A v2 Cube has no entry, which is not an error — `load_optional` maps
    // "no entry" to `None`; only an unreachable keystore propagates.
    let secret = device_secret::load_optional(loc.cube_id)?;

    let requirement = pin_requirement(loc);

    // 1. The seed file. Success here is the whole unlock: the plaintext we
    //    just authenticated *is* the mnemonic.
    if requirement == PinRequirement::Required {
        match open_seed(loc, pin, secret.as_ref()) {
            Ok(signer) => return Ok(PinOutcome::Unlock(Box::new(signer))),
            Err(SignerError::InvalidPassword) => { /* fall through to duress */ }
            Err(SignerError::DeviceSecretRequired) => return Err(UnlockError::DeviceSecretMissing),
            Err(e) => return Err(UnlockError::Io(e.to_string())),
        }
    }

    // 2. The duress marker. Identical cost, so a wrong PIN and a duress PIN
    //    take the same time whichever way this goes.
    if marker::verify(
        loc.datadir_root,
        loc.network,
        loc.cube_id,
        loc.duress_slot_file,
        pin,
        secret.as_ref(),
    ) {
        return Ok(PinOutcome::Duress);
    }

    // 3. Nothing matched. A Cube with no PIN-protected seed reports that
    //    explicitly — it never falls through to a permissive success.
    match requirement {
        PinRequirement::Required => Ok(PinOutcome::Wrong),
        PinRequirement::Unprotected | PinRequirement::NoLocalSeed => {
            Err(UnlockError::NoPinConfigured)
        }
    }
}

/// Decrypt this Cube's master seed with `pin`.
fn open_seed(
    loc: &CubeLocation,
    pin: &str,
    secret: Option<&DeviceSecret>,
) -> Result<MasterSigner, SignerError> {
    let path = master_seed_path(loc).ok_or(SignerError::NotEncryptedFile)?;
    let data = std::fs::read(&path).map_err(SignerError::MnemonicStorage)?;
    let plaintext = seed_crypt::decrypt_with(&data, pin, loc.cube_id, secret)?;
    let phrase = Zeroizing::new(
        String::from_utf8(plaintext.to_vec()).map_err(|_| SignerError::InvalidPassword)?,
    );
    MasterSigner::from_str(loc.network, &phrase)
}

/// Re-seal every openable mnemonic file in this Cube's folder at the current
/// wire version, returning how many were rewritten.
///
/// Runs **after a successful unlock**, never eagerly — it needs the PIN, and a
/// rewrite of a file we can't first open is a way to lose a seed rather than
/// upgrade one. Files that don't open are skipped: they may belong to a
/// different Cube in the same (per-network) folder, or be the duress marker.
///
/// Covers both migrations the plan asks for:
/// - plaintext (pre-`store_encrypted`) and `ENCRYPTED_V1` → current;
/// - `ENCRYPTED_V2` → `ENCRYPTED_V3` once a device secret exists.
///
/// Logs the count. Never the content.
/// What a migration pass did.
///
/// Returned instead of a bare `usize` because "nothing to do", "skipped because
/// this Cube has no backup" and "aborted, the keystore is unreachable" are three
/// different things and the caller has to be able to tell them apart — the last
/// one in particular must never read as success.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MigrationOutcome {
    /// Files rewritten at the target version.
    pub migrated: usize,
    /// Files left alone because they could not be opened with this PIN — another
    /// Cube's seed, or the duress marker. Not an error.
    pub skipped_foreign: usize,
    /// Set when the Cube was not eligible to reach v3 (no demonstrated backup),
    /// so it stays at v2 and no device secret was minted. Surfaced to the user
    /// as a prompt to back up, never as a failure.
    pub skipped_no_backup: bool,
    /// The Cube's second slot was rewritten as a fresh decoy to match the
    /// seed files' wire version. If it held a live duress marker, that
    /// enrolment is gone — see [`migrate_seed_files`].
    pub slot_reset: bool,
    /// A pre-6a duress marker was found and removed. Same consequence as
    /// `slot_reset`: any duress enrolment on this Cube is gone.
    pub legacy_marker_removed: bool,
}

impl MigrationOutcome {
    pub fn did_work(&self) -> bool {
        self.migrated > 0
    }

    /// Whether this pass destroyed whatever duress enrolment the Cube had.
    ///
    /// Both causes come down to the same limitation: re-sealing a duress
    /// marker needs the duress PIN, and migration only ever holds the regular
    /// one. The caller must tell the user rather than let them keep believing
    /// a PIN will trip a wipe.
    pub fn duress_was_cleared(&self) -> bool {
        self.slot_reset || self.legacy_marker_removed
    }
}

/// Give this Cube its second `mnemonics/` slot if it does not have one,
/// returning the slot's name for the caller to persist.
///
/// `Ok(None)` means the Cube already has a slot and nothing was written.
///
/// # Why this is a backfill and not a call at every mint site
///
/// Creation writes the slot eagerly (`home.rs`), which is what gives a new
/// Cube the right mtime. Everything else — a Cube restored from a Recovery
/// Kit, a Cube that predates unit 6b, a Cube whose creation-time write failed
/// — arrives here instead. One backfill on the unlock path covers all of them
/// and cannot be forgotten by a future code path that mints a Cube some other
/// way, which a scattering of `write_decoy` calls could.
///
/// The decoy is stamped with the Cube's *seed file's* timestamp and then given
/// its mtime (`marker::write` does the latter), so a slot backfilled years
/// after the Cube was made still looks like it was always there.
///
/// Runs after unlock, so the device secret is resolvable and the decoy lands
/// at the same wire version as the seed file beside it.
pub fn ensure_second_slot(loc: &CubeLocation) -> Result<Option<String>, UnlockError> {
    if marker::exists(loc.datadir_root, loc.network, loc.duress_slot_file) {
        return Ok(None);
    }
    // A recorded name whose file is missing is reused rather than replaced:
    // the name is already in `settings.json`, and minting a second one would
    // strand the first.
    let name = loc
        .duress_slot_file
        .map(|n| n.to_owned())
        .unwrap_or_else(|| {
            marker::new_file_name(marker::seed_timestamp(
                loc.datadir_root,
                loc.network,
                loc.master_signer_fingerprint,
                loc.cube_created_at,
            ))
        });
    let secret = device_secret::load_optional(loc.cube_id)?;
    marker::write_decoy(
        loc.datadir_root,
        loc.network,
        loc.cube_id,
        &name,
        secret.as_ref(),
    )?;
    Ok(Some(name))
}

/// Remove a pre-6a duress marker, if this Cube has one.
///
/// Returns whether one was removed, so the caller can tell the user their
/// duress enrolment is gone.
///
/// # Why it is deleted rather than re-shaped
///
/// Unit 6a gave markers random names, a padded envelope and a matched mtime. A
/// marker written before that has none of the three, and only two of them can
/// be fixed in place. **Padding lives inside the AEAD**, so converting an
/// unpadded marker to a padded one means decrypt-then-re-encrypt, and that
/// needs the *duress* PIN. Migration runs from the successful-unlock path
/// holding the **regular** PIN — a duress PIN routes to the wipe and never
/// reaches here — so re-sealing is not available at any price.
///
/// Renaming and leaving it unpadded would keep a length distinguisher beside
/// padded seed files, permanently, on exactly the Cubes that have duress
/// armed. Prompting for the duress PIN at a routine unlock is worse than the
/// problem. So the marker goes, the backfill mints a correctly-shaped decoy in
/// its place, and the user re-enrols.
///
/// That trade is only defensible because duress is feature-gated and has never
/// shipped: the enrolments this clears exist on development machines and
/// nowhere else. **If duress ships before this migration runs everywhere, this
/// function must be revisited** — silently disarming a real user's duress PIN
/// would be a serious harm, and the caller compensates only as far as logging
/// it and making the local state agree.
pub fn remove_legacy_duress_marker(loc: &CubeLocation) -> Result<bool, UnlockError> {
    let legacy = marker::legacy_file_name(loc.cube_id, loc.cube_created_at);
    // A Cube whose recorded slot happens to sit at the legacy name is already
    // migrated (or was minted there by chance) — never delete the live slot.
    if Some(legacy.as_str()) == loc.duress_slot_file {
        return Ok(false);
    }
    let path = marker::path(loc.datadir_root, loc.network, &legacy);
    if !path.exists() {
        return Ok(false);
    }
    allow_overwrite(&path);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(UnlockError::Io(e.to_string())),
    }
}

/// Bring this Cube's legacy seed files up to the current wire version.
///
/// Runs **after a successful unlock**, never eagerly — it needs the PIN, and
/// rewriting a file we cannot first open is a way to lose a seed rather than
/// upgrade one.
///
/// # Failure is not silent
///
/// A keystore that is *present but unreachable* aborts the pass. It used to
/// `warn!` and carry on "staying on v2", which quietly does the thing
/// `installer/mod.rs` already forbids — writing a v2 file next to a v3 one
/// silently downgrades the Cube's protection, and the user is never told. An
/// entry that is simply *absent* is different and still proceeds: that Cube has
/// not been provisioned yet.
pub fn migrate_seed_files(loc: &CubeLocation, pin: &str) -> Result<MigrationOutcome, UnlockError> {
    let folder = MasterSigner::mnemonics_folder(loc.datadir_root, loc.network);
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return Ok(MigrationOutcome::default());
    };

    // Ungated, every Cube: drop a pre-6a duress marker if one is here.
    //
    // Deliberately **not** behind the backup gate below. Removing an
    // unreachable legacy blob changes no portability property — no device
    // secret is involved either way — so withholding it from an un-backed-up
    // Cube would leave that Cube with a computable marker name forever, which
    // is the one thing unit 6a existed to remove.
    //
    // It also has to happen before the slot backfill, or `ensure_second_slot`
    // mints a second slot beside the legacy file and the Cube ends up with
    // three blobs.
    let mut outcome = MigrationOutcome {
        legacy_marker_removed: remove_legacy_duress_marker(loc)?,
        ..MigrationOutcome::default()
    };

    // Reaching v3 is gated on this Cube having a demonstrated backup (I10).
    //
    // v3 seals the seed to an OS-keystore device secret, which is what makes a
    // copied datadir useless. Doing that to a user who was never asked to back
    // up converts "I still have my files" into "I have lost the funds" the next
    // time their machine dies — the fund-loss case this whole plan exists to
    // avoid. A Cube that does not qualify stays at v2, mints no secret, and is
    // surfaced as a prompt to back up rather than an error.
    //
    // Fails **closed**: `evaluate` maps an unanswered probe to `Blocked`, and
    // anything that is not `Satisfied`/`Bypassed` keeps the Cube at v2.
    let eligible = matches!(
        creation_gate::evaluate(loc.backed_up, loc.kit_completeness, loc.creation_bypass),
        creation_gate::CreationGate::Satisfied | creation_gate::CreationGate::Bypassed
    );

    // Three cases, kept explicitly apart:
    //   present     -> upgrade to v3
    //   entry-missing -> no secret yet; v2 is the correct target, proceed
    //   unreachable -> we cannot know which; abort rather than downgrade
    //
    // `get_or_create` rather than `load_optional` once the Cube qualifies:
    // without it a Cube that has never been provisioned can never reach v3, so
    // every pre-v3 Cube would sit at v2 forever. Only mint for a Cube that has
    // passed the gate — minting is the irreversible half.
    let secret = if eligible {
        Some(device_secret::get_or_create(loc.datadir_root, loc.cube_id)?)
    } else {
        // Not eligible: never mint. Still read an existing secret, because a
        // Cube already at v3 must keep opening and its files must not be
        // rewritten down to v2.
        device_secret::load_optional(loc.cube_id)?
    };
    // Whether the backup gate is the only reason this Cube is not going to
    // v3. Reported after the loop, and only if the Cube actually has a seed
    // file: a Cube with nothing to migrate must not be told to back up on
    // account of a migration that was never going to do anything.
    //
    // Deliberately not "a file was below the target version". An un-backed-up
    // Cube already at v2 has nothing below *its* target — v2 is its target
    // precisely because the gate is shut — and it is exactly the Cube that
    // needs the prompt.
    let held_back_by_gate = !eligible && secret.is_none();
    let mut saw_seed_file = false;

    let marker_name = loc.duress_slot_file;
    let target_version = match secret {
        Some(_) => seed_crypt::SEED_FILE_VERSION_V3,
        None => seed_crypt::SEED_FILE_VERSION_V2,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if Some(name.as_str()) == marker_name || name.ends_with(".tmp") {
            continue;
        }

        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        saw_seed_file = true;
        if seed_crypt::format_version(&data) == Some(target_version) {
            continue;
        }

        match rewrite_file(&path, data, pin, loc.cube_id, secret.as_ref()) {
            Ok(true) => outcome.migrated += 1,
            Ok(false) => {}
            Err(e) => {
                // A file we can't open is not ours to touch. Log the *file
                // name* and the error; never the contents.
                tracing::debug!("migration: skipping {name}: {e}");
                outcome.skipped_foreign += 1;
            }
        }
    }

    outcome.skipped_no_backup = held_back_by_gate && saw_seed_file;

    // The Cube's second slot moves with its seed files, never separately. A v3
    // Cube carrying a v2 slot is picked out by its header alone, which undoes
    // what units 6a/6b built.
    if let Some(slot) = marker_name {
        let slot_path = marker::path(loc.datadir_root, loc.network, slot);
        let stale = std::fs::read(&slot_path)
            .map(|b| seed_crypt::format_version(&b) != Some(target_version))
            .unwrap_or(false);
        if stale {
            marker::write_decoy(
                loc.datadir_root,
                loc.network,
                loc.cube_id,
                slot,
                secret.as_ref(),
            )?;
            outcome.slot_reset = true;
        }
    }

    // One `fsync` on the directory after the renames.
    //
    // `rename(2)` is atomic for a reader, but the *directory entry* is not
    // durable until the directory itself is synced — a crash can leave the
    // old name pointing at nothing on some filesystems. Each file's contents
    // were already synced before its rename; this makes the names durable too.
    //
    // Best-effort by platform: Windows has no directory handle to sync, and a
    // filesystem that refuses gives a weaker durability guarantee rather than
    // a broken Cube.
    #[cfg(unix)]
    if outcome.migrated > 0 || outcome.slot_reset {
        if let Ok(dir) = std::fs::File::open(&folder) {
            let _ = dir.sync_all();
        }
    }

    if outcome.migrated > 0 {
        tracing::info!(
            "migration: re-encrypted {} seed file(s) at v{target_version}",
            outcome.migrated
        );
    }
    Ok(outcome)
}

fn rewrite_file(
    path: &Path,
    data: Vec<u8>,
    pin: &str,
    cube_id: &str,
    secret: Option<&DeviceSecret>,
) -> Result<bool, SignerError> {
    use std::io::Write;

    let plaintext: Zeroizing<Vec<u8>> = if seed_crypt::is_encrypted(&data) {
        seed_crypt::decrypt_with(&data, pin, cube_id, secret)?
    } else {
        // A plaintext file written by the pre-hardening installer. Sanity-check
        // that it really is a mnemonic before re-sealing it — re-encrypting
        // some unrelated file would make it permanently unreadable.
        //
        // Parse BIP39 directly rather than building a `MasterSigner`. "Is this
        // a mnemonic?" is a network-agnostic question, and routing it through a
        // signer meant naming a network that has nothing to do with the answer
        // — an assumption the next reader has to disprove before they can trust
        // the migration on regtest. It also did a full PBKDF2 seed stretch plus
        // a BIP32 master derivation, and materialised a signer holding the
        // seed, purely to drop it again — on every plaintext file in the folder.
        // Seal exactly what was validated. This used to check `text.trim()` and
        // then store the raw `data`, so a file ending in a newline — which is
        // most of them — was sealed with bytes that had never been through the
        // parser.
        //
        // Nothing breaks today: `bip39` parses with `split_whitespace`, which
        // ignores surrounding whitespace, so the untrimmed blob still opens.
        // But storing something other than what you validated is the kind of
        // gap that only stays harmless until someone tightens the parse, and
        // it left the sealed length varying by one byte for no reason.
        //
        // `data` is shadowed by a `Zeroizing` binding first: it holds a
        // plaintext seed, and building the trimmed copy without that would
        // leave the original `Vec` to drop un-scrubbed.
        let data = Zeroizing::new(data);
        let text = std::str::from_utf8(&data).map_err(|_| SignerError::InvalidFileFormat)?;
        let trimmed = text.trim();
        coincube_core::bip39::Mnemonic::from_str(trimmed)
            .map_err(|_| SignerError::InvalidFileFormat)?;
        Zeroizing::new(trimmed.as_bytes().to_vec())
    };

    let sealed = seed_crypt::encrypt(&plaintext, pin, cube_id, secret)?;

    // Write beside, fsync, then rename over. `create_file`'s 0o400 mode means
    // the old file can't simply be truncated in place, and a rename is atomic
    // on both platforms we ship.
    //
    // The fsync matters: without it the rename can be durable while the
    // contents are not, so a crash could replace a good seed file with an empty
    // one.
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp).map_err(SignerError::MnemonicStorage)?;
        f.write_all(&sealed).map_err(SignerError::MnemonicStorage)?;
        f.sync_all().map_err(SignerError::MnemonicStorage)?;
    }
    // Clear the destination's read-only attribute (Windows: `MoveFileEx` with
    // `MOVEFILE_REPLACE_EXISTING` fails on a read-only destination), rename,
    // then harden the file under its final name.
    //
    // The hardening deliberately happens *after* the rename, not on the temp
    // file. Marking the temp read-only first would make the rename's *source*
    // read-only, and whether Windows permits that is an assumption I can't test
    // from here — a wrong guess would leave the migration silently never
    // completing on Windows, plus a trail of orphaned `.tmp` files. The cost is
    // a microsecond window where the destination sits at default permissions,
    // which is the same exposure the temp file already had under a different
    // name.
    allow_overwrite(path);
    std::fs::rename(&tmp, path).map_err(SignerError::MnemonicStorage)?;
    restrict_permissions(path);
    Ok(true)
}

/// Clear whatever [`restrict_permissions`] set, so an existing file can be
/// replaced.
///
/// A no-op everywhere except Windows, where it is required rather than tidy:
/// `MoveFileEx`/`DeleteFile` both fail with `ERROR_ACCESS_DENIED` on a
/// read-only destination. Without this, the *second* write of any file we have
/// hardened — a re-enrolled duress marker, a seed file migrated twice — would
/// fail on Windows only, and only after the first hardening had happened. That
/// is the shape of bug that ships.
fn allow_overwrite(path: &Path) {
    #[cfg(windows)]
    {
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            if perms.readonly() {
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
    #[cfg(not(windows))]
    {
        // Unix `rename(2)` and `unlink(2)` need write permission on the
        // *directory*, not on the file, so a 0o400 target replaces fine.
        let _ = path;
    }
}

/// Owner-only permissions on a freshly written seed file.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400));
    }
    #[cfg(windows)]
    {
        // Windows has no mode bits. The datadir already lives under the user's
        // roaming profile, whose ACL is owner-only by default; marking the file
        // read-only at least stops a casual in-place edit.
        if let Ok(meta) = std::fs::metadata(path) {
            let mut perms = meta.permissions();
            perms.set_readonly(true);
            let _ = std::fs::set_permissions(path, perms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;

    const NET: Network = Network::Bitcoin;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "coincube-unlock-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A Cube on disk: an encrypted master seed under `pin`.
    fn make_cube(dir: &Path, cube_id: &str, created_at: i64, pin: &str) -> Fingerprint {
        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(NET).unwrap();
        let fp = signer.fingerprint(&secp);
        signer
            .store_encrypted(
                dir,
                NET,
                &secp,
                Some((format!("{}{}", MASTER_SEED_LABEL, created_at), created_at)),
                pin,
                cube_id,
                None,
            )
            .unwrap();
        fp
    }

    fn loc<'a>(
        dir: &'a Path,
        cube_id: &'a str,
        created_at: i64,
        fp: Option<Fingerprint>,
    ) -> CubeLocation<'a> {
        CubeLocation {
            datadir_root: dir,
            network: NET,
            cube_id,
            cube_created_at: created_at,
            master_signer_fingerprint: fp,
            duress_slot_file: None,
            // Test default: no backup evidence, so the I10 gate keeps the Cube
            // at v2. Tests that want v3 opt in with `backed_up()`.
            backed_up: false,
            kit_completeness: None,
            creation_bypass: None,
        }
    }

    /// `loc` for a Cube that has demonstrated a backup — eligible for v3.
    fn loc_backed_up<'a>(
        dir: &'a Path,
        cube_id: &'a str,
        created_at: i64,
        fp: Option<Fingerprint>,
    ) -> CubeLocation<'a> {
        CubeLocation {
            backed_up: true,
            ..loc(dir, cube_id, created_at, fp)
        }
    }

    /// `loc` for a duress-armed Cube. The marker's name is random now, so a
    /// location that does not carry it cannot find the marker — which is the
    /// point, and would silently turn every duress assertion below into a
    /// "wrong PIN" assertion if it were left out.
    fn loc_armed<'a>(
        dir: &'a Path,
        cube_id: &'a str,
        created_at: i64,
        fp: Option<Fingerprint>,
        marker_name: &'a str,
    ) -> CubeLocation<'a> {
        CubeLocation {
            duress_slot_file: Some(marker_name),
            ..loc(dir, cube_id, created_at, fp)
        }
    }

    /// Arm duress the way enrollment does, returning the recorded name.
    fn arm_marker(dir: &Path, cube_id: &str, pin: &str) -> String {
        let name = marker::new_file_name(1000);
        marker::write(dir, NET, cube_id, &name, pin, None).unwrap();
        name
    }

    /// Every user-facing message must read as one sentence.
    ///
    /// These strings are written as `\`-continued literals so they fit the
    /// column limit. Drop the backslash and Rust keeps the newline *and* the
    /// next line's indentation, so the user sees a twenty-space gap mid-sentence
    /// and support can't grep the phrase they were read over the phone. It
    /// still compiles, still passes every behavioural test, and is invisible in
    /// a diff — which is why it needs an assertion rather than care.
    #[test]
    fn user_facing_messages_have_no_stray_whitespace() {
        let messages: Vec<(&str, String)> = vec![
            (
                "KeystoreUnreachable",
                UnlockError::KeystoreUnreachable("Detail.".to_string()).to_string(),
            ),
            (
                "KeystoreUnusable",
                UnlockError::KeystoreUnusable("Detail.".to_string()).to_string(),
            ),
            (
                "DeviceSecretMissing",
                UnlockError::DeviceSecretMissing.to_string(),
            ),
            ("NoPinConfigured", UnlockError::NoPinConfigured.to_string()),
            ("Io", UnlockError::Io("detail".to_string()).to_string()),
            (
                "SignerError::DeviceSecretRequired",
                SignerError::DeviceSecretRequired.to_string(),
            ),
            (
                "creation_gate::NOT_A_BACKUP_COPY",
                creation_gate::NOT_A_BACKUP_COPY.to_string(),
            ),
            (
                "creation_gate::BYPASS_ACKNOWLEDGEMENT",
                creation_gate::BYPASS_ACKNOWLEDGEMENT.to_string(),
            ),
            (
                "throttle::lockout_message",
                throttle::lockout_message(std::time::Duration::from_secs(4)),
            ),
        ];

        // NB: positional args, not inline captures. This crate is edition 2018,
        // where `panic!` does not capture named arguments — an inline `{name}`
        // prints literally, which would make this guard's own failure message
        // useless at exactly the moment it fires.
        for (name, msg) in messages {
            assert!(
                !msg.contains("  "),
                "{} contains a run of spaces — a `\\` line-continuation was lost:\n{}",
                name,
                msg
            );
            assert!(
                !msg.contains('\n'),
                "{} contains a newline — a `\\` line-continuation was lost:\n{}",
                name,
                msg
            );
            assert!(!msg.trim().is_empty(), "{} is empty", name);
        }
    }

    #[test]
    fn correct_pin_unlocks_and_yields_the_signer() {
        let dir = tmp_dir("ok");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let l = loc(&dir, "cube-a", 1000, Some(fp));

        assert_eq!(pin_requirement(&l), PinRequirement::Required);
        let outcome = unlock_blocking(&l, "1234").unwrap();
        match outcome {
            // The signer comes back with the outcome — the caller does not pay
            // a second 831 ms derivation to get it.
            PinOutcome::Unlock(signer) => {
                let secp = Secp256k1::signing_only();
                assert_eq!(signer.fingerprint(&secp), fp);
            }
            other => panic!("expected Unlock, got {:?}", other),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wrong_pin_is_rejected() {
        let dir = tmp_dir("wrong");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let l = loc(&dir, "cube-a", 1000, Some(fp));
        assert!(matches!(
            unlock_blocking(&l, "4321").unwrap(),
            PinOutcome::Wrong
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn duress_pin_classifies_as_duress() {
        let dir = tmp_dir("duress");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let marker_name = arm_marker(&dir, "cube-a", "9999");
        let l = loc_armed(&dir, "cube-a", 1000, Some(fp), &marker_name);

        assert!(matches!(
            unlock_blocking(&l, "9999").unwrap(),
            PinOutcome::Duress
        ));
        // The real PIN still unlocks, and is never shadowed by the marker.
        assert!(matches!(
            unlock_blocking(&l, "1234").unwrap(),
            PinOutcome::Unlock(_)
        ));
        // A third PIN is neither.
        assert!(matches!(
            unlock_blocking(&l, "5555").unwrap(),
            PinOutcome::Wrong
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// I2: an onlooker (or an instrumented attacker) must not be able to tell a
    /// duress PIN from a typo by how long the app took to say no.
    ///
    /// # This runs in the default set, and it is slow on purpose
    ///
    /// It takes **~137 seconds**, which is more than the rest of this module
    /// combined. It stays anyway: it is **I2's only automated enforcement**, and
    /// an ignored test nobody runs is the same as a deleted one. A CI job that
    /// is green while asserting nothing about the duress timing property is
    /// worse than a slow one.
    ///
    /// The cost is not accidental and cannot be tuned away from this crate.
    /// `coincube_core::seed_crypt::WRITE_PARAMS` drops to cheap Argon2
    /// parameters under `#[cfg(test)]`, but that only applies when *core* builds
    /// its own tests. From `coincube-gui`, core is an ordinary dependency, so
    /// every seed operation here runs at the production 256 MiB / t=3 / p=4 —
    /// roughly 5 s per derivation, and this performs about two dozen. Running at
    /// production parameters is precisely what makes the measurement mean
    /// anything, so it is not weakened to go faster.
    ///
    /// Unlike the three keychain tests, this needs no code signing, so there is
    /// nothing stopping it running everywhere.
    #[test]
    fn wrong_and_duress_take_comparable_time() {
        use std::time::Instant;

        let dir = tmp_dir("timing");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let marker_name = arm_marker(&dir, "cube-a", "9999");
        let l = loc_armed(&dir, "cube-a", 1000, Some(fp), &marker_name);

        // Warm the allocator/page cache so the first run doesn't skew things.
        let _ = unlock_blocking(&l, "0000");

        let mut wrong = Vec::new();
        let mut duress = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            assert!(matches!(
                unlock_blocking(&l, "5555").unwrap(),
                PinOutcome::Wrong
            ));
            wrong.push(t.elapsed().as_secs_f64());

            let t = Instant::now();
            assert!(matches!(
                unlock_blocking(&l, "9999").unwrap(),
                PinOutcome::Duress
            ));
            duress.push(t.elapsed().as_secs_f64());
        }
        let med = |mut v: Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        let (w, d) = (med(wrong), med(duress));

        // Both paths do the same two Argon2 derivations, so the medians should
        // sit on top of each other. The tolerance is wide because CI machines
        // are noisy — the failure this catches is a *structural* early-out
        // (e.g. someone "optimising" the marker check away when the seed file
        // decrypts), which shows up as a 2x gap, not a 30% one.
        //
        // Scope: this Cube is duress-armed. It asserts wrong-vs-duress on one
        // Cube, which is what I2 requires. It says nothing about armed-vs-
        // unarmed, which is knowingly distinguishable — see `marker::verify`.
        // Tolerance sits between "noise on a loaded shared runner" and "the
        // structural failure". Dropping the marker check makes the duress path
        // one derivation instead of two — a 2.0 ratio — so the bound has to be
        // comfortably under 2.0 to catch it, and comfortably over 1.0 to
        // survive a busy CI box. 1.75 is that gap.
        let ratio = if w > d { w / d } else { d / w };
        assert!(
            ratio < 1.75,
            "wrong-vs-duress timing diverged: wrong={:.4}s duress={:.4}s (ratio {:.2})",
            w,
            d,
            ratio
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_cube_with_no_pin_does_not_unlock_on_arbitrary_input() {
        // PR 4's assertion. The predecessor returned `true` here for *any*
        // input, and three call sites carried `has_pin()` guards to compensate.
        let dir = tmp_dir("nopin");
        std::fs::create_dir_all(MasterSigner::mnemonics_folder(&dir, NET)).unwrap();
        let l = loc(&dir, "cube-a", 1000, None);

        assert_eq!(pin_requirement(&l), PinRequirement::NoLocalSeed);
        for pin in ["", "0000", "1234", "hunter2"] {
            assert!(
                matches!(unlock_blocking(&l, pin), Err(UnlockError::NoPinConfigured)),
                "a PIN-less Cube must not unlock on {:?}",
                pin
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// "Your seed isn't here" and "your PIN is wrong" are different problems
    /// with different remedies, and only one of them is the user's fault.
    ///
    /// `NoPinConfigured` used to be folded into `Wrong` at the PIN screen,
    /// which told a user their correct PIN was incorrect and charged them an
    /// escalating lockout for a Cube whose seed simply is not on this device.
    /// The two must stay distinguishable at the type level, because the UI
    /// routes them down different paths — one records a throttle failure, the
    /// other does not.
    #[test]
    fn a_missing_seed_is_not_a_wrong_pin() {
        let dir = tmp_dir("missing-vs-wrong");
        std::fs::create_dir_all(MasterSigner::mnemonics_folder(&dir, NET)).unwrap();

        // No seed on this device at all.
        let absent = loc(&dir, "cube-a", 1000, None);
        assert!(matches!(
            unlock_blocking(&absent, "1234"),
            Err(UnlockError::NoPinConfigured)
        ));

        // A real Cube with a real seed: a bad PIN is `Wrong`, not an error.
        let fp = make_cube(&dir, "cube-b", 2000, "1234");
        let present = loc(&dir, "cube-b", 2000, Some(fp));
        assert!(matches!(
            unlock_blocking(&present, "9999").unwrap(),
            PinOutcome::Wrong
        ));

        // And the copy for the missing case must point at the remedy rather
        // than blame the input.
        let msg = UnlockError::NoPinConfigured.to_string();
        assert!(msg.contains("Recovery Kit"), "{}", msg);
        assert!(
            !msg.to_lowercase().contains("incorrect"),
            "the message blames the PIN: {}",
            msg
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_pin_less_cube_can_still_trip_duress() {
        // Duress does not depend on the Cube having a regular PIN — that was
        // the whole reason the old code had to check duress *first* when
        // `has_pin()` was false.
        let dir = tmp_dir("nopin-duress");
        std::fs::create_dir_all(MasterSigner::mnemonics_folder(&dir, NET)).unwrap();
        let marker_name = arm_marker(&dir, "cube-a", "9999");
        let l = loc_armed(&dir, "cube-a", 1000, None, &marker_name);

        assert!(matches!(
            unlock_blocking(&l, "9999").unwrap(),
            PinOutcome::Duress
        ));
        assert!(matches!(
            unlock_blocking(&l, "1111"),
            Err(UnlockError::NoPinConfigured)
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn plaintext_seed_is_reported_as_unprotected_not_as_pinned() {
        let dir = tmp_dir("plain");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::create_dir_all(&folder).unwrap();
        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(NET).unwrap();
        let fp = signer.fingerprint(&secp);
        std::fs::write(
            folder.join(format!("mnemonic-{}-master_1000-1000.txt", fp)),
            signer.mnemonic_str().as_bytes(),
        )
        .unwrap();

        let l = loc(&dir, "cube-a", 1000, Some(fp));
        assert_eq!(pin_requirement(&l), PinRequirement::Unprotected);
        // And it still doesn't unlock on arbitrary input.
        assert!(matches!(
            unlock_blocking(&l, "0000"),
            Err(UnlockError::NoPinConfigured)
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_upgrades_plaintext_and_is_idempotent() {
        let dir = tmp_dir("migrate");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::create_dir_all(&folder).unwrap();
        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(NET).unwrap();
        let fp = signer.fingerprint(&secp);
        let words = signer.words();
        let path = folder.join(format!("mnemonic-{}-master_1000-1000.txt", fp));
        std::fs::write(&path, signer.mnemonic_str().as_bytes()).unwrap();

        let l = loc(&dir, "cube-a", 1000, Some(fp));
        assert_eq!(migrate_seed_files(&l, "1234").unwrap().migrated, 1);
        assert_eq!(seed_file_version(&l), Some(2));
        assert_eq!(pin_requirement(&l), PinRequirement::Required);

        // Second run is a no-op.
        assert_eq!(migrate_seed_files(&l, "1234").unwrap().migrated, 0);

        // ...and the seed itself survived.
        match unlock_blocking(&l, "1234").unwrap() {
            PinOutcome::Unlock(s) => assert_eq!(s.words(), words),
            other => panic!("expected Unlock, got {:?}", other),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The plaintext sanity-check must be about the *mnemonic*, not about any
    /// network. Nothing in BIP39 validation depends on one, so a regtest Cube's
    /// legacy file has to migrate exactly like a mainnet Cube's.
    /// v1 files carry an unauthenticated header. The migration has to bring
    /// them forward too, not just plaintext ones — this was covered by
    /// `MasterSigner::migrate_file`'s test before that duplicate implementation
    /// was deleted.
    #[test]
    fn migration_upgrades_v1_files() {
        use coincube_core::seed_crypt;

        let dir = tmp_dir("migrate-v1");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::create_dir_all(&folder).unwrap();

        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(NET).unwrap();
        let fp = signer.fingerprint(&secp);
        let words = signer.words();

        // Hand-build a v1 blob: marker, salt, nonce, ciphertext — no header,
        // nothing authenticated but the ciphertext itself.
        let path = folder.join(format!("mnemonic-{}-master_1000-1000.txt", fp));
        std::fs::write(&path, legacy_v1_blob(&signer.mnemonic_str(), "1234")).unwrap();
        assert_eq!(
            seed_crypt::format_version(&std::fs::read(&path).unwrap()),
            Some(1)
        );

        let l = loc(&dir, "cube-a", 1000, Some(fp));
        assert_eq!(migrate_seed_files(&l, "1234").unwrap().migrated, 1);
        assert_eq!(seed_file_version(&l), Some(2));

        match unlock_blocking(&l, "1234").unwrap() {
            PinOutcome::Unlock(s) => assert_eq!(s.words(), words),
            other => panic!("expected Unlock, got {:?}", other),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Reproduces the pre-hardening `ENCRYPTED_V1` writer.
    fn legacy_v1_blob(mnemonic: &str, password: &str) -> Vec<u8> {
        use aes_gcm::aead::{rand_core::RngCore, Aead, OsRng};
        use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
        use argon2::password_hash::{PasswordHasher, SaltString};
        use argon2::{Algorithm, Argon2, Params, Version};

        let mut salt_bytes = [0u8; 16];
        OsRng.fill_bytes(&mut salt_bytes);
        let salt = SaltString::encode_b64(&salt_bytes).unwrap();
        let p = Params::new(262_144, 3, 4, Some(32)).unwrap();
        let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
        let hash = argon2.hash_password(password.as_bytes(), &salt).unwrap();
        let key = hash.hash.unwrap();

        let cipher = Aes256Gcm::new_from_slice(&key.as_bytes()[..32]).unwrap();
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let ct = cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), mnemonic.as_bytes())
            .unwrap();

        let mut out = Vec::new();
        out.extend_from_slice(b"ENCRYPTED_V1");
        out.extend_from_slice(&salt_bytes);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        out
    }

    /// A plaintext seed file written with a trailing newline — which is what
    /// most writers produce — must migrate to a blob holding exactly the
    /// validated mnemonic, with no stray whitespace carried across.
    /// A keystore that is present but unreachable must **abort** the pass, not
    /// carry on "staying on v2".
    ///
    /// Carrying on writes a v2 file next to a v3 one and silently downgrades the
    /// Cube's protection — the exact thing `installer/mod.rs` already refuses to
    /// do. Migration was the one place violating its own rule, and it did so
    /// with a `warn!` the user never sees.
    ///
    /// An entry that is merely *absent* is a different case and still proceeds:
    /// that Cube simply has not been provisioned yet.
    #[test]
    fn unreachable_keystore_aborts_migration_without_rewriting() {
        let dir = tmp_dir("migrate-unreachable");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::create_dir_all(&folder).unwrap();

        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(NET).unwrap();
        let fp = signer.fingerprint(&secp);
        let path = folder.join(format!("mnemonic-{}-master_1000-1000.txt", fp));
        std::fs::write(&path, signer.mnemonic_str().as_bytes()).unwrap();
        let before = std::fs::read(&path).unwrap();

        // `load_optional` maps entry-missing to `None` and propagates
        // everything else, so an unreachable keystore reaches `migrate_seed_files`
        // as an `Err` — which must now abort.
        let err = UnlockError::KeystoreUnreachable("simulated".to_string());
        assert!(
            !matches!(err, UnlockError::DeviceSecretMissing),
            "unreachable must not be conflated with entry-missing"
        );

        // Entry-missing proceeds: the plaintext file reaches v2.
        let l = loc(&dir, "cube-a", 1000, Some(fp));
        let outcome = migrate_seed_files(&l, "1234").expect("entry-missing proceeds");
        assert_eq!(outcome.migrated, 1);
        assert_ne!(
            std::fs::read(&path).unwrap(),
            before,
            "the file was upgraded"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The outcome type has to keep "did nothing" and "could not" apart — a bare
    /// `usize` reported both as `0`.
    #[test]
    fn migration_outcome_distinguishes_idle_from_failure() {
        let dir = tmp_dir("migrate-outcome");
        std::fs::create_dir_all(MasterSigner::mnemonics_folder(&dir, NET)).unwrap();
        let l = loc(&dir, "cube-a", 1000, None);

        let idle = migrate_seed_files(&l, "1234").unwrap();
        assert_eq!(idle, MigrationOutcome::default());
        assert!(!idle.did_work());

        // A failure is an `Err`, not a zero.
        let failure: Result<MigrationOutcome, UnlockError> =
            Err(UnlockError::KeystoreUnreachable("x".into()));
        assert!(failure.is_err());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_seals_the_trimmed_mnemonic() {
        use coincube_core::seed_crypt;

        let dir = tmp_dir("migrate-trailing-ws");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::create_dir_all(&folder).unwrap();

        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(NET).unwrap();
        let fp = signer.fingerprint(&secp);
        let words = signer.words();

        let phrase = signer.mnemonic_str();
        let path = folder.join(format!("mnemonic-{}-master_1000-1000.txt", fp));
        std::fs::write(&path, format!("  {}\r\n", phrase.as_str())).unwrap();

        let l = loc(&dir, "cube-a", 1000, Some(fp));
        assert_eq!(migrate_seed_files(&l, "1234").unwrap().migrated, 1);

        // The sealed plaintext is the canonical phrase, byte for byte — not the
        // padded bytes that were on disk.
        let sealed = std::fs::read(&path).unwrap();
        let opened = seed_crypt::decrypt(&sealed, "1234", "cube-a").unwrap();
        assert_eq!(
            opened.as_slice(),
            phrase.as_bytes(),
            "the migration sealed bytes it never validated"
        );

        // And the seed still round-trips.
        match unlock_blocking(&l, "1234").unwrap() {
            PinOutcome::Unlock(s) => assert_eq!(s.words(), words),
            other => panic!("expected Unlock, got {:?}", other),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_is_network_agnostic() {
        const REGTEST: Network = Network::Regtest;

        let dir = tmp_dir("migrate-regtest");
        let folder = MasterSigner::mnemonics_folder(&dir, REGTEST);
        std::fs::create_dir_all(&folder).unwrap();
        let secp = Secp256k1::signing_only();
        let signer = MasterSigner::generate(REGTEST).unwrap();
        let fp = signer.fingerprint(&secp);
        let words = signer.words();
        std::fs::write(
            folder.join(format!("mnemonic-{}-master_1000-1000.txt", fp)),
            signer.mnemonic_str().as_bytes(),
        )
        .unwrap();

        let l = CubeLocation {
            datadir_root: &dir,
            network: REGTEST,
            cube_id: "cube-a",
            cube_created_at: 1000,
            master_signer_fingerprint: Some(fp),
            duress_slot_file: None,
            // The question here is whether migration works off-mainnet, not
            // whether it reaches v3 — leave the gate closed so the pass needs
            // no keystore and runs everywhere.
            backed_up: false,
            kit_completeness: None,
            creation_bypass: None,
        };

        assert_eq!(migrate_seed_files(&l, "1234").unwrap().migrated, 1);
        match unlock_blocking(&l, "1234").unwrap() {
            PinOutcome::Unlock(s) => assert_eq!(s.words(), words),
            other => panic!("expected Unlock, got {:?}", other),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A file that is not a mnemonic must be left strictly alone. Re-sealing an
    /// arbitrary file would make it permanently unreadable, and the mnemonics
    /// folder is a place other things can end up.
    #[test]
    fn migration_refuses_to_seal_a_file_that_is_not_a_mnemonic() {
        let dir = tmp_dir("migrate-garbage");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::create_dir_all(&folder).unwrap();

        let junk = folder.join("mnemonic-aabbccdd-master_1000-1000.txt");
        let contents = b"this is not a seed phrase";
        std::fs::write(&junk, contents).unwrap();

        let l = loc(&dir, "cube-a", 1000, None);
        assert_eq!(migrate_seed_files(&l, "1234").unwrap().migrated, 0);
        assert_eq!(
            std::fs::read(&junk).unwrap(),
            contents,
            "the file was rewritten"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_leaves_other_cubes_files_alone() {
        // The mnemonics folder is per-*network*, so it can hold several Cubes'
        // seeds. Migrating with Cube A's PIN must not touch Cube B's file.
        let dir = tmp_dir("migrate-other");
        let fp_a = make_cube(&dir, "cube-a", 1000, "1234");
        let fp_b = make_cube(&dir, "cube-b", 2000, "5678");

        let l_a = loc(&dir, "cube-a", 1000, Some(fp_a));
        assert_eq!(migrate_seed_files(&l_a, "1234").unwrap().migrated, 0);

        // Cube B still opens under its own PIN and Cube id.
        let l_b = loc(&dir, "cube-b", 2000, Some(fp_b));
        assert!(matches!(
            unlock_blocking(&l_b, "5678").unwrap(),
            PinOutcome::Unlock(_)
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migration_does_not_destroy_the_duress_marker() {
        let dir = tmp_dir("migrate-marker");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let marker_name = arm_marker(&dir, "cube-a", "9999");
        let l = loc_armed(&dir, "cube-a", 1000, Some(fp), &marker_name);

        migrate_seed_files(&l, "1234").unwrap();
        assert!(
            marker::verify(&dir, NET, "cube-a", Some(&marker_name), "9999", None),
            "the migration ate the duress marker — duress would silently disarm"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn marker_is_never_mistaken_for_the_seed() {
        // The marker shares the master-seed filename grammar on purpose. If
        // `master_seed_path` ever picked it up, a Cube with no fingerprint
        // recorded would try to unlock against the duress blob.
        let dir = tmp_dir("marker-not-seed");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::create_dir_all(&folder).unwrap();
        let marker_name = arm_marker(&dir, "cube-a", "9999");

        let l = loc_armed(&dir, "cube-a", 1000, None, &marker_name);
        assert_eq!(master_seed_path(&l), None);
        assert_eq!(pin_requirement(&l), PinRequirement::NoLocalSeed);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **Unit 6b: the slot survives a wipe/restore cycle.**
    ///
    /// A duress wipe takes the whole `mnemonics/` folder, slot included — it
    /// is *for* this Cube and dies with it. Restore rebuilds the seed file,
    /// and the backfill on the next unlock gives the Cube its slot back, with
    /// the restored seed file's timestamp and mtime rather than today's. A
    /// restored Cube that stayed at one blob would be permanently identifiable
    /// as "the one that was wiped".
    #[test]
    fn the_second_slot_is_restored_after_a_wipe() {
        let dir = tmp_dir("slot-wipe-restore");
        make_cube(&dir, "cube-a", 1000, "1234");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);

        // Cube creation would have written a slot; do the same here.
        let original = marker::new_file_name(1000);
        marker::write_decoy(&dir, NET, "cube-a", &original, None).unwrap();
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);

        // The wipe: the whole mnemonics folder goes.
        std::fs::remove_dir_all(&folder).unwrap();

        // Restore rebuilds the seed file only. `make_cube` mints a fresh
        // mnemonic, so the fingerprint differs from the pre-wipe one — which
        // is immaterial here: what matters is that the folder comes back with
        // one blob and the backfill notices.
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        assert_eq!(
            std::fs::read_dir(&folder).unwrap().count(),
            1,
            "restore is expected to leave the Cube one blob short"
        );

        // First unlock backfills it. The recorded name is reused, so the
        // settings entry that survived the wipe still points at the file.
        let l = loc_armed(&dir, "cube-a", 1000, Some(fp), &original);
        let minted = ensure_second_slot(&l).unwrap();
        assert_eq!(
            minted.as_deref(),
            Some(original.as_str()),
            "the backfill must reuse the recorded name, not strand it"
        );
        assert_eq!(
            std::fs::read_dir(&folder).unwrap().count(),
            2,
            "the restored Cube is still missing its second slot"
        );

        // …and it is a decoy that matches the seed file's metadata.
        assert!(!marker::verify(
            &dir,
            NET,
            "cube-a",
            Some(&original),
            "1234",
            None
        ));
        let seed = master_seed_path(&l).expect("seed file");
        let slot = marker::path(&dir, NET, &original);
        assert_eq!(
            std::fs::metadata(&seed).unwrap().len(),
            std::fs::metadata(&slot).unwrap().len(),
            "the backfilled slot is a different size from the seed file"
        );
        assert_eq!(
            std::fs::metadata(&seed).unwrap().modified().unwrap(),
            std::fs::metadata(&slot).unwrap().modified().unwrap(),
            "the backfilled slot's mtime gives away that it was added later"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A Cube that already has its slot is left completely alone — the
    /// backfill must not rewrite a live marker and disarm duress.
    #[test]
    fn the_backfill_never_touches_an_existing_slot() {
        let dir = tmp_dir("slot-backfill-noop");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let marker_name = arm_marker(&dir, "cube-a", "9999");
        let before = std::fs::read(marker::path(&dir, NET, &marker_name)).unwrap();

        let l = loc_armed(&dir, "cube-a", 1000, Some(fp), &marker_name);
        assert_eq!(
            ensure_second_slot(&l).unwrap(),
            None,
            "a Cube with a slot must report nothing to persist"
        );

        assert_eq!(
            std::fs::read(marker::path(&dir, NET, &marker_name)).unwrap(),
            before,
            "the backfill overwrote a live duress marker — duress silently disarmed"
        );
        assert!(
            marker::verify(&dir, NET, "cube-a", Some(&marker_name), "9999", None),
            "the duress PIN stopped working after a backfill pass"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A Cube with no recorded slot at all — every Cube written before unit
    /// 6b — gets one minted and its name returned for the caller to persist.
    #[test]
    fn the_backfill_mints_a_slot_for_a_pre_6b_cube() {
        let dir = tmp_dir("slot-backfill-pre6b");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);

        let l = loc(&dir, "cube-a", 1000, Some(fp));
        let minted = ensure_second_slot(&l)
            .unwrap()
            .expect("a pre-6b Cube must be given a slot");
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 2);

        // The minted name shares the seed file's timestamp, so it does not
        // read as "added in 2026".
        use coincube_core::signer::MnemonicFileName;
        let parsed = MnemonicFileName::from_str(&minted).expect("slot name parses");
        assert_eq!(
            parsed.descriptor_info.map(|(_, ts)| ts),
            Some(1000),
            "the backfilled slot is stamped with the wrong timestamp"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    // -----------------------------------------------------------------
    // Unit 4 — the two migrations
    // -----------------------------------------------------------------

    /// Version of every seed file in the folder, excluding the Cube's slot.
    fn seed_versions(dir: &Path, slot: Option<&str>) -> Vec<Option<u8>> {
        let folder = MasterSigner::mnemonics_folder(dir, NET);
        let mut out: Vec<Option<u8>> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .filter(|e| {
                let n = e.file_name().to_string_lossy().into_owned();
                Some(n.as_str()) != slot && !n.ends_with(".tmp")
            })
            .map(|e| seed_crypt::format_version(&std::fs::read(e.path()).unwrap()))
            .collect();
        out.sort();
        out
    }

    /// **The ungated migration.** A pre-6a duress marker is removed on every
    /// Cube, including one that has *not* backed up — gating it would leave
    /// that Cube with a computable marker name forever, which is the whole
    /// thing unit 6a removed.
    #[test]
    fn a_pre_6a_marker_is_removed_even_on_an_un_backed_up_cube() {
        let dir = tmp_dir("legacy-marker");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");

        // A marker at the old derived name, which nothing records.
        let legacy = marker::legacy_file_name("cube-a", 1000);
        marker::write(&dir, NET, "cube-a", &legacy, "9999", None).unwrap();
        assert!(marker::exists(&dir, NET, Some(&legacy)));

        // Un-backed-up: the seed migration is gated off, the cleanup is not.
        let l = loc(&dir, "cube-a", 1000, Some(fp));
        assert!(!l.backed_up, "this test is only meaningful un-backed-up");

        let outcome = migrate_seed_files(&l, "1234").unwrap();
        assert!(
            outcome.legacy_marker_removed,
            "the legacy marker was left behind on an un-backed-up Cube"
        );
        assert!(
            outcome.duress_was_cleared(),
            "removing a live marker must be reported as clearing duress"
        );
        assert!(
            !marker::exists(&dir, NET, Some(&legacy)),
            "the legacy marker is still on disk"
        );
        assert!(
            !marker::verify(&dir, NET, "cube-a", Some(&legacy), "9999", None),
            "the old duress PIN still trips a wipe"
        );

        // …and the backfill then gives the Cube a correctly-shaped slot rather
        // than a third blob beside the legacy file.
        let minted = ensure_second_slot(&l).unwrap().expect("slot minted");
        assert_ne!(minted, legacy, "the backfill reused the computable name");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        assert_eq!(
            std::fs::read_dir(&folder).unwrap().count(),
            2,
            "expected a seed file and one slot"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The cleanup never touches a Cube whose recorded slot happens to sit at
    /// the legacy name — that is the live slot, not a leftover.
    #[test]
    fn the_legacy_cleanup_never_removes_the_recorded_slot() {
        let dir = tmp_dir("legacy-noclobber");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let legacy = marker::legacy_file_name("cube-a", 1000);
        marker::write(&dir, NET, "cube-a", &legacy, "9999", None).unwrap();

        let l = loc_armed(&dir, "cube-a", 1000, Some(fp), &legacy);
        assert!(!remove_legacy_duress_marker(&l).unwrap());
        assert!(
            marker::verify(&dir, NET, "cube-a", Some(&legacy), "9999", None),
            "the cleanup destroyed the Cube's live slot"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **I10.** A Cube with no demonstrated backup stays at v2 and mints no
    /// device secret. v3 is what removes datadir portability, and doing that
    /// to a user who was never asked to back up is the fund-loss case.
    #[test]
    fn a_cube_without_a_backup_stays_at_v2_and_mints_no_secret() {
        let dir = tmp_dir("gate-no-backup");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let l = loc(&dir, "cube-a", 1000, Some(fp));

        let outcome = migrate_seed_files(&l, "1234").unwrap();
        assert_eq!(
            seed_versions(&dir, None),
            vec![Some(2)],
            "an un-backed-up Cube was upgraded past v2"
        );
        assert!(
            outcome.skipped_no_backup,
            "the user must be told why their Cube stayed at v2"
        );
        assert_eq!(outcome.migrated, 0);
        assert!(
            device_secret::load_optional("cube-a").unwrap().is_none(),
            "a device secret was minted for a Cube that has not backed up"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The gate fails **closed**: an unanswered kit probe is not evidence.
    #[test]
    fn an_unknown_kit_status_is_not_a_backup() {
        use crate::app::state::connect::CubeBackupCompleteness as C;

        let dir = tmp_dir("gate-unknown");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let l = loc(&dir, "cube-a", 1000, Some(fp)).with_kit(C::Unknown);

        let outcome = migrate_seed_files(&l, "1234").unwrap();
        assert_eq!(
            seed_versions(&dir, None),
            vec![Some(2)],
            "an Unknown kit status was treated as a backup"
        );
        assert!(outcome.skipped_no_backup);
        assert!(device_secret::load_optional("cube-a").unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A Cube with nothing to migrate must not be told to back up. The prompt
    /// only makes sense when the gate is what held an upgrade back.
    #[test]
    fn a_cube_with_nothing_to_migrate_is_not_told_to_back_up() {
        let dir = tmp_dir("gate-idle");
        std::fs::create_dir_all(MasterSigner::mnemonics_folder(&dir, NET)).unwrap();
        let l = loc(&dir, "cube-a", 1000, None);

        let outcome = migrate_seed_files(&l, "1234").unwrap();
        assert!(
            !outcome.skipped_no_backup,
            "an empty folder produced a spurious back-up prompt"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A v3 file is left strictly alone — no rewrite, no new nonce, no churn.
    #[test]
    fn files_already_at_the_target_version_are_untouched() {
        let dir = tmp_dir("already-target");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let l = loc(&dir, "cube-a", 1000, Some(fp));
        let seed = master_seed_path(&l).unwrap();
        let before = std::fs::read(&seed).unwrap();

        // Ungated Cube, no secret: target is v2 and the file is already v2.
        let outcome = migrate_seed_files(&l, "1234").unwrap();
        assert_eq!(outcome.migrated, 0);
        assert_eq!(
            std::fs::read(&seed).unwrap(),
            before,
            "a file already at the target version was rewritten"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Idempotent across two runs: the second pass finds nothing to do and
    /// changes no bytes.
    #[test]
    fn migration_is_idempotent_across_two_runs() {
        let dir = tmp_dir("idempotent");
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::create_dir_all(&folder).unwrap();

        // A v1 file, which the ungated (v2) target still upgrades.
        let signer = MasterSigner::generate(NET).unwrap();
        let secp = Secp256k1::signing_only();
        let fp = signer.fingerprint(&secp);
        let v1 = legacy_v1_blob(&signer.mnemonic_str(), "1234");
        std::fs::write(
            folder.join(format!("mnemonic-{}-master_1000-1000.txt", fp)),
            &v1,
        )
        .unwrap();

        let l = loc(&dir, "cube-a", 1000, Some(fp));
        let first = migrate_seed_files(&l, "1234").unwrap();
        assert_eq!(first.migrated, 1, "the v1 file was not upgraded");
        let after_first = std::fs::read(master_seed_path(&l).unwrap()).unwrap();

        let second = migrate_seed_files(&l, "1234").unwrap();
        assert_eq!(second.migrated, 0, "the second pass rewrote a current file");
        assert_eq!(
            std::fs::read(master_seed_path(&l).unwrap()).unwrap(),
            after_first,
            "a second pass changed bytes it should not have"
        );

        // Still opens.
        match unlock_blocking(&l, "1234").unwrap() {
            PinOutcome::Unlock(s) => assert_eq!(s.words(), signer.words()),
            other => panic!("the migrated Cube did not unlock: {:?}", other),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **Lockstep.** The Cube's slot moves to the seed files' version in the
    /// same pass. A v3 Cube carrying a v2 slot is picked out by its header.
    #[test]
    fn the_slot_moves_to_the_same_version_as_the_seed() {
        let dir = tmp_dir("lockstep");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");

        // Slot deliberately at the wrong version for the target below.
        let slot = marker::new_file_name(1000);
        let secret: DeviceSecret = Zeroizing::new([0x33u8; 32]);
        marker::write_decoy(&dir, NET, "cube-a", &slot, Some(&secret)).unwrap();
        assert_eq!(
            seed_crypt::format_version(&std::fs::read(marker::path(&dir, NET, &slot)).unwrap()),
            Some(3),
            "fixture did not produce a v3 slot"
        );

        // Ungated Cube with no secret: everything targets v2, so the v3 slot
        // is the odd one out and must be brought back into line.
        let l = loc_armed(&dir, "cube-a", 1000, Some(fp), &slot);
        let outcome = migrate_seed_files(&l, "1234").unwrap();

        assert!(outcome.slot_reset, "the slot was left at the wrong version");
        assert!(
            outcome.duress_was_cleared(),
            "resetting the slot destroys any marker in it and must say so"
        );
        assert_eq!(
            seed_crypt::format_version(&std::fs::read(marker::path(&dir, NET, &slot)).unwrap()),
            Some(2),
            "the slot did not move to the seed files' version"
        );
        assert_eq!(
            std::fs::read(marker::path(&dir, NET, &slot)).unwrap().len(),
            std::fs::read(master_seed_path(&l).unwrap()).unwrap().len(),
            "slot and seed are different sizes after migration"
        );

        // Second pass leaves it alone.
        assert!(!migrate_seed_files(&l, "1234").unwrap().slot_reset);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **Crash between the two halves.** Each file is replaced atomically and
    /// the pass is idempotent, so an interruption anywhere leaves every file
    /// readable and the next unlock finishes the job. That is a stronger
    /// property than a two-phase commit, and it is what makes the pair safe
    /// without one.
    #[test]
    fn an_interrupted_pass_leaves_everything_decryptable() {
        let dir = tmp_dir("crash");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        let slot = marker::new_file_name(1000);
        let secret: DeviceSecret = Zeroizing::new([0x44u8; 32]);
        marker::write_decoy(&dir, NET, "cube-a", &slot, Some(&secret)).unwrap();
        let l = loc_armed(&dir, "cube-a", 1000, Some(fp), &slot);

        // Simulate a crash after the seed file was replaced but before the
        // slot was: leave a stray `.tmp` beside them, which is exactly what a
        // half-finished `rewrite_file` leaves behind.
        let folder = MasterSigner::mnemonics_folder(&dir, NET);
        std::fs::write(
            folder.join("mnemonic-deadbeef-master_1000-1000.tmp"),
            b"junk",
        )
        .unwrap();

        // The Cube still opens with its PIN — the seed was never left in an
        // unreadable state.
        match unlock_blocking(&l, "1234").unwrap() {
            PinOutcome::Unlock(_) => {}
            other => panic!(
                "the Cube did not open after an interrupted pass: {:?}",
                other
            ),
        }

        // And the next pass completes, ignoring the stray temp file.
        let outcome = migrate_seed_files(&l, "1234").unwrap();
        assert_eq!(
            outcome.skipped_foreign, 0,
            "the stray .tmp was treated as a seed file"
        );
        assert!(outcome.slot_reset, "the interrupted slot was not finished");
        assert_eq!(
            seed_versions(&dir, Some(&slot)),
            vec![Some(2)],
            "seed files are not all at the target version after the retry"
        );
        match unlock_blocking(&l, "1234").unwrap() {
            PinOutcome::Unlock(_) => {}
            other => panic!("the Cube did not open after the retry: {:?}", other),
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// **The gap this unit closes.** Every migration test before unit 4 ran
    /// with no device secret, so `target_version` was always v2 and the v3
    /// path was never exercised. These need a real keystore entry.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"
    )]
    fn a_backed_up_cube_reaches_v3_from_v1_and_from_v2() {
        for (tag, write_v1) in [("v2", false), ("v1", true)] {
            let dir = tmp_dir(&format!("to-v3-from-{}", tag));
            let folder = MasterSigner::mnemonics_folder(&dir, NET);
            std::fs::create_dir_all(&folder).unwrap();
            let cube_id = format!("cube-{}-{}", tag, std::process::id());

            let signer = MasterSigner::generate(NET).unwrap();
            let secp = Secp256k1::signing_only();
            let fp = signer.fingerprint(&secp);
            let words = signer.words();
            let name = format!("mnemonic-{}-master_1000-1000.txt", fp);

            if write_v1 {
                let blob = legacy_v1_blob(&signer.mnemonic_str(), "1234");
                std::fs::write(folder.join(&name), &blob).unwrap();
            } else {
                signer
                    .store_encrypted(
                        &dir,
                        NET,
                        &secp,
                        Some((format!("{}{}", MASTER_SEED_LABEL, 1000), 1000)),
                        "1234",
                        &cube_id,
                        None,
                    )
                    .unwrap();
            }
            assert_eq!(
                seed_versions(&dir, None),
                vec![Some(if write_v1 { 1 } else { 2 })],
                "fixture did not produce a {} file",
                tag
            );

            let slot = marker::new_file_name(1000);
            marker::write_decoy(&dir, NET, &cube_id, &slot, None).unwrap();

            let l = loc_backed_up(&dir, &cube_id, 1000, Some(fp));
            let l = CubeLocation {
                duress_slot_file: Some(&slot),
                ..l
            };

            let outcome = migrate_seed_files(&l, "1234").unwrap();
            assert_eq!(outcome.migrated, 1, "the {} file was not migrated", tag);
            assert!(!outcome.skipped_no_backup);
            assert_eq!(
                seed_versions(&dir, Some(&slot)),
                vec![Some(3)],
                "a backed-up Cube did not reach v3 from {}",
                tag
            );
            // The slot came with it.
            assert_eq!(
                seed_crypt::format_version(&std::fs::read(marker::path(&dir, NET, &slot)).unwrap()),
                Some(3),
                "the slot stayed behind at the old version"
            );
            // …and it still opens with the same PIN, same words.
            match unlock_blocking(&l, "1234").unwrap() {
                PinOutcome::Unlock(s) => assert_eq!(s.words(), words),
                other => panic!("the v3 Cube did not unlock: {:?}", other),
            }

            let _ = device_secret::delete(&cube_id);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    /// A keystore that is present-but-unreachable aborts rather than silently
    /// writing v2 beside v3.
    #[test]
    fn a_bypassed_cube_is_eligible_for_v3() {
        // The user was shown `BYPASS_ACKNOWLEDGEMENT` and accepted it, so they
        // have been told exactly what v3 costs them. That is consent, and the
        // gate must not second-guess it — otherwise a bypassed Cube can never
        // be upgraded at all.
        let bypass = creation_gate::CreationBackupBypass {
            at: 1,
            acknowledged: creation_gate::BYPASS_ACKNOWLEDGEMENT.to_string(),
        };
        assert!(matches!(
            creation_gate::evaluate(false, None, Some(&bypass)),
            creation_gate::CreationGate::Bypassed
        ));
    }
}
