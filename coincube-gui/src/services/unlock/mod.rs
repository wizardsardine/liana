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
//! Same for duress: `duress_pin_hash` is replaced by a marker blob sealed at
//! *identical* parameters ([`marker`]), so a wrong PIN and a duress PIN cost
//! the same wall clock and look the same on disk (invariant I2).
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
            Self::DeviceSecretMissing => write!(
                f,
                "Part of this Cube's encryption key was stored in this computer's system \
                 keychain, and it's no longer there. This can happen after a keychain reset, \
                 a disk restore, or moving the Cube folder between machines. Your PIN alone \
                 can't open it. Restore this Cube from its Recovery Kit."
            ),
            Self::NoPinConfigured => write!(
                f,
                "This Cube has no PIN-protected seed on this device."
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
}

impl<'a> CubeLocation<'a> {
    pub fn new(datadir_root: &'a Path, cube: &'a CubeSettings) -> Self {
        Self {
            datadir_root,
            network: cube.network,
            cube_id: &cube.id,
            cube_created_at: cube.created_at,
            master_signer_fingerprint: cube.master_signer_fingerprint,
        }
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

    let marker_name = marker::file_name(loc.cube_id, loc.cube_created_at);
    let mut best: Option<(PathBuf, i64)> = None;

    for entry in entries.flatten() {
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_owned(),
            None => continue,
        };
        // Never mistake the duress marker for a seed. It deliberately shares
        // the master-seed filename grammar (see `marker`), so name-shape alone
        // cannot tell them apart — only this exact-name check can.
        if name == marker_name {
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
/// indistinguishable in wall clock (I2). Do not add an early-out.
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
            Err(SignerError::DeviceSecretRequired) => {
                return Err(UnlockError::DeviceSecretMissing)
            }
            Err(e) => return Err(UnlockError::Io(e.to_string())),
        }
    }

    // 2. The duress marker. Identical cost, so a wrong PIN and a duress PIN
    //    take the same time whichever way this goes.
    if marker::verify(
        loc.datadir_root,
        loc.network,
        loc.cube_id,
        loc.cube_created_at,
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
pub fn migrate_seed_files(loc: &CubeLocation, pin: &str) -> usize {
    let folder = MasterSigner::mnemonics_folder(loc.datadir_root, loc.network);
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return 0;
    };

    let secret = match device_secret::load_optional(loc.cube_id) {
        Ok(s) => s,
        Err(e) => {
            // Not fatal: without a secret we can still bring v1/plaintext up to
            // v2, which is the security-critical half.
            tracing::warn!("migration: device secret unavailable, staying on v2: {e}");
            None
        }
    };

    let marker_name = marker::file_name(loc.cube_id, loc.cube_created_at);
    let target_version = match secret {
        Some(_) => seed_crypt::SEED_FILE_VERSION_V3,
        None => seed_crypt::SEED_FILE_VERSION_V2,
    };

    let mut migrated = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == marker_name || name.ends_with(".tmp") {
            continue;
        }

        let Ok(data) = std::fs::read(&path) else {
            continue;
        };
        if seed_crypt::format_version(&data) == Some(target_version) {
            continue;
        }

        match rewrite_file(&path, data, pin, loc.cube_id, secret.as_ref()) {
            Ok(true) => migrated += 1,
            Ok(false) => {}
            Err(e) => {
                // A file we can't open is not ours to touch. Log the *file
                // name* and the error; never the contents.
                tracing::debug!("migration: skipping {name}: {e}");
            }
        }
    }

    if migrated > 0 {
        tracing::info!(
            "migration: re-encrypted {migrated} seed file(s) at v{target_version}"
        );
    }
    migrated
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
        let text = String::from_utf8(data.clone()).map_err(|_| SignerError::InvalidFileFormat)?;
        MasterSigner::from_str(coincube_core::miniscript::bitcoin::Network::Bitcoin, text.trim())
            .map_err(|_| SignerError::InvalidFileFormat)?;
        Zeroizing::new(data)
    };

    let sealed = seed_crypt::encrypt(&plaintext, pin, cube_id, secret)?;

    // Write beside, fsync, then rename over. `create_file`'s 0o400 mode means
    // the old file can't simply be truncated in place, and a rename is atomic
    // on both platforms we ship.
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp).map_err(SignerError::MnemonicStorage)?;
        f.write_all(&sealed).map_err(SignerError::MnemonicStorage)?;
        f.sync_all().map_err(SignerError::MnemonicStorage)?;
    }
    restrict_permissions(&tmp);
    std::fs::rename(&tmp, path).map_err(SignerError::MnemonicStorage)?;
    Ok(true)
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

    fn loc<'a>(dir: &'a Path, cube_id: &'a str, created_at: i64, fp: Option<Fingerprint>) -> CubeLocation<'a> {
        CubeLocation {
            datadir_root: dir,
            network: NET,
            cube_id,
            cube_created_at: created_at,
            master_signer_fingerprint: fp,
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
        marker::write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();
        let l = loc(&dir, "cube-a", 1000, Some(fp));

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
    #[test]
    fn wrong_and_duress_take_comparable_time() {
        use std::time::Instant;

        let dir = tmp_dir("timing");
        let fp = make_cube(&dir, "cube-a", 1000, "1234");
        marker::write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();
        let l = loc(&dir, "cube-a", 1000, Some(fp));

        // Warm the allocator/page cache so the first run doesn't skew things.
        let _ = unlock_blocking(&l, "0000");

        let mut wrong = Vec::new();
        let mut duress = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            assert!(matches!(unlock_blocking(&l, "5555").unwrap(), PinOutcome::Wrong));
            wrong.push(t.elapsed().as_secs_f64());

            let t = Instant::now();
            assert!(matches!(unlock_blocking(&l, "9999").unwrap(), PinOutcome::Duress));
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
        // decrypts, or skipping it when no marker exists), which shows up as a
        // 2x gap, not a 30% one.
        let ratio = if w > d { w / d } else { d / w };
        assert!(
            ratio < 1.6,
            "wrong-vs-duress timing diverged: wrong={:.4}s duress={:.4}s (ratio {:.2})",
            w, d, ratio
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

    #[test]
    fn a_pin_less_cube_can_still_trip_duress() {
        // Duress does not depend on the Cube having a regular PIN — that was
        // the whole reason the old code had to check duress *first* when
        // `has_pin()` was false.
        let dir = tmp_dir("nopin-duress");
        std::fs::create_dir_all(MasterSigner::mnemonics_folder(&dir, NET)).unwrap();
        marker::write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();
        let l = loc(&dir, "cube-a", 1000, None);

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
        assert_eq!(migrate_seed_files(&l, "1234"), 1);
        assert_eq!(seed_file_version(&l), Some(2));
        assert_eq!(pin_requirement(&l), PinRequirement::Required);

        // Second run is a no-op.
        assert_eq!(migrate_seed_files(&l, "1234"), 0);

        // ...and the seed itself survived.
        match unlock_blocking(&l, "1234").unwrap() {
            PinOutcome::Unlock(s) => assert_eq!(s.words(), words),
            other => panic!("expected Unlock, got {:?}", other),
        }
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
        assert_eq!(migrate_seed_files(&l_a, "1234"), 0);

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
        marker::write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();
        let l = loc(&dir, "cube-a", 1000, Some(fp));

        migrate_seed_files(&l, "1234");
        assert!(
            marker::verify(&dir, NET, "cube-a", 1000, "9999", None),
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
        marker::write(&dir, NET, "cube-a", 1000, "9999", None).unwrap();

        let l = loc(&dir, "cube-a", 1000, None);
        assert_eq!(master_seed_path(&l), None);
        assert_eq!(pin_requirement(&l), PinRequirement::NoLocalSeed);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
