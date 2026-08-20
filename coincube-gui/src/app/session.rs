//! Secrets that live for exactly as long as a Cube is unlocked.
//!
//! Two things: the unlock **PIN**, and the **master signer** that verifying it
//! produced.
//!
//! The signer is here because verifying a PIN *is* decrypting the seed file, so
//! by the time unlock succeeds the plaintext is already in hand. Dropping it
//! meant the Liquid and Spark loaders each re-ran Argon2id at 256 MiB
//! immediately afterwards — three ~831 ms derivations per unlock where one will
//! do. Caching it is what makes `services::unlock`'s "we do not pay 831 ms
//! twice" true rather than aspirational.
//!
//! It is a cache and never the source of truth: every consumer falls back to
//! reading the seed file, and two guards (`cube_id` and the signer's
//! fingerprint) mean it can never answer for a key the caller did not ask for.
//!
//! The PIN is needed after unlock, not just during it —
//!
//! - the Vault installer has to encrypt the hot signer it generates, and it is
//!   launched from inside an already-open Cube (`SetupVault`), long after the
//!   PIN entry screen is gone;
//! - the startup migration re-encrypts legacy plaintext / v1 seed files, which
//!   requires the PIN in hand;
//! - the Tier 1 device-secret migration rewrites the seed file at v3 on first
//!   successful unlock.
//!
//! The alternative was threading a `Zeroizing<String>` through `Loader::new`,
//! `App::new`, `App::new_without_wallet`, `Installer::new` and three messages
//! that carry state between them. That is a lot of surface for a value whose
//! real lifetime is "while this Cube is open", and it would still leave the
//! auto-lock path (PR 13) with no single place to clear.
//!
//! # What this is not
//!
//! It is not a widening of the secret's exposure. The decrypted mnemonic
//! already sits in process memory for the whole session — inside the
//! `BreezClient`'s signer and inside the Spark bridge subprocess — so holding
//! one more copy here does not change the threat model. What it does change is
//! that there is now a single place that drops *all* of it deterministically:
//! [`close`], called on lock, on idle auto-lock, and on duress activation.
//!
//! Accessors hand out owned copies (`Zeroizing` for the PIN, a re-derived
//! signer), so no caller can hold a reference across a lock.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
use coincube_core::signer::{MasterSigner, SignerError};
use subtle::ConstantTimeEq as _;
use zeroize::Zeroizing;

use crate::app::settings::CubeSettings;

struct Session {
    /// The Cube this PIN belongs to. Checked on read so a stale session from a
    /// previously-open Cube can never hand its PIN to a different one.
    cube_id: String,
    /// `None` for a **passkey** Cube, which has no PIN at all: its master seed
    /// comes from a WebAuthn assertion, not from an encrypted file.
    ///
    /// This is an `Option` rather than an empty string on purpose. The two
    /// consumers of a PIN after unlock both do something irreversible with it —
    /// the Vault installer *encrypts a hot signer* under it, and duress
    /// enrolment validates the duress PIN against it — and an empty string is a
    /// value they would happily use. `None` makes them refuse instead, which is
    /// the only safe direction to fail.
    pin: Option<Zeroizing<String>>,
    /// The master signer the unlock already decrypted, and the fingerprint it
    /// belongs to.
    ///
    /// Verifying the PIN *is* decrypting the seed file, so by the time unlock
    /// succeeds the signer is in hand. Throwing it away meant the Liquid and
    /// Spark loaders each re-ran Argon2id at 256 MiB — three full derivations
    /// per unlock (~2.5 s) where one will do. Keeping it here is what makes the
    /// "we don't pay 831 ms twice" claim in `services::unlock` true.
    ///
    /// The fingerprint is recorded at store time so a lookup is a comparison
    /// rather than a fresh secp context, and so a signer can never be handed to
    /// a caller asking for a different key.
    signer: Option<(Fingerprint, MasterSigner)>,
    /// Last time the user did something. Drives idle auto-lock.
    last_activity: Instant,
}

fn cell() -> &'static Mutex<Option<Session>> {
    static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    SESSION.get_or_init(|| Mutex::new(None))
}

/// A poisoned mutex here means a thread panicked while holding the session.
/// Recover the guard rather than propagating: refusing to hand back the PIN
/// would strand an unlocked Cube, and refusing to *clear* it would be worse.
fn lock_session() -> std::sync::MutexGuard<'static, Option<Session>> {
    cell().lock().unwrap_or_else(|e| e.into_inner())
}

/// Record a successful PIN unlock. Replaces any previous session — opening a
/// second Cube without closing the first is not a thing the UI can do, but if
/// it ever becomes one, the newest wins and the old PIN is zeroized here.
pub fn open(cube_id: impl Into<String>, pin: Zeroizing<String>) {
    open_inner(cube_id.into(), Some(pin));
}

/// Record a successful unlock for a Cube that **has** no PIN — a passkey Cube.
///
/// Everything a session provides other than the PIN still applies: idle
/// auto-lock, the current Cube id, and the signer the assertion just derived.
/// [`pin_for`] and [`current_pin`] answer `None`, so the two callers that would
/// otherwise act on a PIN (the Vault installer's seed encryption and duress
/// enrolment's duress-PIN validation) refuse rather than proceed on an empty
/// string.
pub fn open_without_pin(cube_id: impl Into<String>) {
    open_inner(cube_id.into(), None);
}

fn open_inner(cube_id: String, pin: Option<Zeroizing<String>>) {
    let mut guard = lock_session();
    // Re-opening the *same* Cube keeps the signer the unlock already decrypted.
    // The PIN screen stores it before this runs, and clobbering it here would
    // silently reinstate the extra 831 ms derivation this exists to avoid.
    let signer = guard
        .take()
        .filter(|s| s.cube_id == cube_id)
        .and_then(|s| s.signer);
    *guard = Some(Session {
        cube_id,
        pin,
        signer,
        last_activity: Instant::now(),
    });
}

/// Hand the session the signer that verifying the PIN just produced.
///
/// Called from the unlock's blocking task. It goes here rather than through an
/// iced message because messages must be `Clone` and are cloned freely — every
/// copy would be another master seed on the heap.
pub fn store_unlocked_signer(cube_id: &str, fingerprint: Fingerprint, signer: MasterSigner) {
    let mut guard = lock_session();
    match guard.as_mut() {
        // Only ever attach to the session for the Cube it belongs to.
        Some(s) if s.cube_id == cube_id => s.signer = Some((fingerprint, signer)),
        // No session yet (the PIN screen stores before `open`): stand one up
        // holding just the signer. `open` will fill in the PIN and preserve it.
        _ => {
            *guard = Some(Session {
                cube_id: cube_id.to_string(),
                // No PIN yet, and possibly never — a passkey Cube stores its
                // signer here and never calls `open`. `None`, not `""`.
                pin: None,
                signer: Some((fingerprint, signer)),
                last_activity: Instant::now(),
            })
        }
    }
}

/// The already-decrypted signer for `cube_id`, if it is the open Cube *and*
/// the stored signer is the one the caller is asking for.
///
/// Returns an independent copy each call — `try_clone` re-derives from the
/// mnemonic, which is a BIP-39 seed stretch and a BIP-32 master derivation
/// (~1 ms), not another Argon2id pass (~831 ms). Callers must fall back to
/// reading the seed file when this is `None`; it is a cache, never the source
/// of truth.
///
/// The fingerprint check is what makes this safe: a Cube can hold more than one
/// signer (the master seed and a Vault hot signer), and handing a caller the
/// wrong one would produce a valid-looking wallet that is not the one it asked
/// for.
pub fn unlocked_signer(cube_id: &str, fingerprint: Fingerprint) -> Option<MasterSigner> {
    lock_session()
        .as_ref()
        .filter(|s| s.cube_id == cube_id)
        .and_then(|s| s.signer.as_ref())
        .filter(|(fp, _)| *fp == fingerprint)
        .and_then(|(_, signer)| signer.try_clone().ok())
}

/// The already-decrypted signer for `cube_id`, released only to a caller that
/// can produce the PIN this session was opened with.
///
/// This is the seed-*reveal* surfaces' door to the cache — `load_mnemonic_words`
/// and its three callers (Backup Master Seed, Recovery Kit, Full-Cube escrow).
/// [`unlocked_signer`] is not usable there: it asks only "is this the open
/// Cube?", which is the right question for the Liquid and Spark loaders (they
/// run *because* an unlock just succeeded) and the wrong one for a screen that
/// puts the permanent secret in front of the user. Those screens have their own
/// PIN prompt, and it has to actually mean something.
///
/// Both guards and the clone happen under **one** acquisition of the session
/// lock. Reading the PIN with [`pin_for`] and then fetching the signer with
/// [`unlocked_signer`] would be two, with an idle auto-lock or a Cube switch
/// able to land in between.
///
/// # This is not a cheap Argon2 oracle, but it is a cheap check
///
/// It runs no Argon2id, so a guess here costs microseconds where
/// trial-decrypting the seed file costs ~831 ms. That is not the failure
/// `PLAN-cube-unlock-hardening` I1 is about — nothing is written to disk for an
/// offline attacker to grind, and reaching this code at all means the Cube is
/// already open in a live process. What *does* rate-limit these surfaces is the
/// escalating lockout in `services::unlock::throttle`, which every caller checks
/// before it gets here and charges on every `InvalidPassword` below.
///
/// The comparison is constant-time for the same belt-and-braces reason the
/// pairing code is: `==` on a secret short-circuits at the first differing byte,
/// and that is free to get right here and awkward to notice later.
///
/// # Errors
///
/// - `PasswordRequired` — no session, a session belonging to a different Cube,
///   or a **passkey** Cube (no PIN at all; those flows re-derive through a
///   WebAuthn assertion instead, and must not be handed the seed for the empty
///   string).
/// - `InvalidPassword` — the PIN is wrong. The only variant callers charge to
///   the throttle; see `is_wrong_pin`.
/// - `SignerNotFound` — the session holds no signer, or holds one for a
///   different key. A Cube can hold both a master seed and a Vault hot signer.
///
/// Everything except `InvalidPassword` means "this cache cannot answer" and the
/// caller should read the seed file instead.
pub fn unlocked_signer_with_pin_verification(
    cube_id: &str,
    fingerprint: Fingerprint,
    pin: &str,
) -> Result<MasterSigner, SignerError> {
    let guard = lock_session();
    let session = guard
        .as_ref()
        .filter(|s| s.cube_id == cube_id)
        .ok_or(SignerError::PasswordRequired)?;

    let current_pin = session.pin.as_ref().ok_or(SignerError::PasswordRequired)?;

    if !bool::from(current_pin.as_bytes().ct_eq(pin.as_bytes())) {
        return Err(SignerError::InvalidPassword);
    }

    let (fp, signer) = session
        .signer
        .as_ref()
        .ok_or(SignerError::SignerNotFound(fingerprint))?;

    if *fp != fingerprint {
        return Err(SignerError::SignerNotFound(fingerprint));
    }

    signer
        .try_clone()
        .map_err(|_| SignerError::SignerNotFound(fingerprint))
}

/// The open Cube's PIN, if `cube_id` is the Cube that is actually open **and**
/// that Cube has a PIN at all. A passkey Cube answers `None`.
pub fn pin_for(cube_id: &str) -> Option<Zeroizing<String>> {
    lock_session()
        .as_ref()
        .filter(|s| s.cube_id == cube_id)
        .and_then(|s| s.pin.clone())
}

/// The credential this Cube's **seed files** are encrypted under, whichever
/// shape of Cube it is.
///
/// The two shapes disagree about what that credential even is, and every caller
/// that reads or writes a seed file needs the same answer:
///
/// - a **PIN** Cube: the session PIN ([`pin_for`]);
/// - a **passkey** Cube: a key derived from the master seed the unlock
///   assertion produced ([`crate::services::passkey::seed_password`]) — it has
///   no PIN, so `pin_for` answers `None` for it by design.
///
/// This is the read-side twin of what the Vault installer writes with
/// ([`crate::installer::Context::seed_password`]). Keeping both on one
/// definition of "the password" is the point: they must agree exactly or the
/// file the installer writes is one nothing can open again.
///
/// `None` means this cache cannot answer — no session, a session for a
/// different Cube, or a passkey Cube whose signer is not in the session (which
/// is not a normal state: the unlock parks it there). Callers treat that as
/// "no hot signer available", never as "the seed is gone".
pub fn seed_file_password(cube: &CubeSettings) -> Option<Zeroizing<String>> {
    if let Some(pin) = pin_for(&cube.id) {
        return Some(pin);
    }
    if !cube.is_passkey_cube() {
        return None;
    }
    // Deriving needs the Cube's *master* seed specifically, so go through
    // `unlocked_signer`, which refuses to answer for any other key.
    let signer = unlocked_signer(&cube.id, cube.master_signer_fingerprint?)?;
    Some(crate::services::passkey::seed_password::derive(
        &signer,
        cube.network,
    ))
}

/// The open Cube's PIN, whichever Cube that is. Use [`pin_for`] when the
/// caller knows which Cube it means — this exists for the installer, which
/// runs before the restored Cube's id is minted. `None` for a passkey Cube.
pub fn current_pin() -> Option<Zeroizing<String>> {
    lock_session().as_ref().and_then(|s| s.pin.clone())
}

/// Id of the currently open Cube, if any.
pub fn current_cube_id() -> Option<String> {
    lock_session().as_ref().map(|s| s.cube_id.clone())
}

/// Note user activity, deferring idle auto-lock.
pub fn touch() {
    if let Some(s) = lock_session().as_mut() {
        s.last_activity = Instant::now();
    }
}

/// How long the open Cube has been idle. `None` when nothing is open.
pub fn idle_for() -> Option<Duration> {
    lock_session().as_ref().map(|s| s.last_activity.elapsed())
}

/// Whether a Cube is currently open.
pub fn is_open() -> bool {
    lock_session().is_some()
}

/// Drop and zeroize the session. Idempotent; safe to call on a closed session.
///
/// Called on lock, on returning to the launcher, and on duress activation.
pub fn close() {
    *lock_session() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    // These share one process-global, so they run under a mutex of their own
    // rather than being allowed to interleave.
    fn guard() -> std::sync::MutexGuard<'static, ()> {
        static M: OnceLock<Mutex<()>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn open_then_read_then_close() {
        let _g = guard();
        close();
        assert!(!is_open());
        assert_eq!(current_pin(), None);

        open("cube-a", Zeroizing::new("1234".to_string()));
        assert!(is_open());
        assert_eq!(pin_for("cube-a").as_deref(), Some(&"1234".to_string()));
        assert_eq!(current_cube_id().as_deref(), Some("cube-a"));

        close();
        assert!(!is_open());
        assert_eq!(pin_for("cube-a"), None);
    }

    fn passkey_cube(id: &str) -> CubeSettings {
        use coincube_core::miniscript::bitcoin::Network;
        let mut cube = CubeSettings::new_with_raw_id(
            id.to_string(),
            "Passkey Cube".to_string(),
            Network::Bitcoin,
        );
        cube.passkey_metadata = Some(crate::app::settings::PasskeyMetadata {
            credential_id: "Y3JlZC1pZA==".to_string(),
            rp_id: "coincube.io".to_string(),
            created_at: 1_786_122_245,
            label: None,
        });
        cube
    }

    /// A PIN Cube's seed files are encrypted under its PIN, and that is the
    /// answer `seed_file_password` must give — the installer wrote them that
    /// way.
    #[test]
    fn a_pin_cubes_seed_password_is_its_pin() {
        use coincube_core::miniscript::bitcoin::Network;
        let _g = guard();
        close();

        let cube = CubeSettings::new_with_raw_id(
            "cube-pin".to_string(),
            "PIN Cube".to_string(),
            Network::Bitcoin,
        );
        assert_eq!(
            seed_file_password(&cube),
            None,
            "no session means no credential to hand out"
        );

        open("cube-pin", Zeroizing::new("1234".to_string()));
        assert_eq!(seed_file_password(&cube).as_deref(), Some(&"1234".into()));

        close();
    }

    /// The passkey arm: no PIN exists, so the answer is derived from the master
    /// seed the assertion produced. This is the read side of what the Vault
    /// installer encrypts with — they must be the same value.
    #[test]
    fn a_passkey_cubes_seed_password_is_derived_from_its_master_seed() {
        use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
        use coincube_core::miniscript::bitcoin::Network;

        let _g = guard();
        close();

        let signer = MasterSigner::generate(Network::Bitcoin).unwrap();
        let fp = signer.fingerprint(&Secp256k1::signing_only());
        let expected =
            crate::services::passkey::seed_password::derive(&signer, Network::Bitcoin).to_string();

        let cube = passkey_cube("cube-passkey").with_master_signer(fp);

        // No session yet: nothing to derive from, and emphatically not an empty
        // string.
        assert_eq!(seed_file_password(&cube), None);

        open_without_pin("cube-passkey");
        store_unlocked_signer("cube-passkey", fp, signer);

        assert_eq!(
            seed_file_password(&cube).as_deref(),
            Some(&expected),
            "a passkey Cube's seed password comes from its own master seed"
        );
        assert_eq!(
            pin_for("cube-passkey"),
            None,
            "and it is still not a PIN — nothing may treat it as one"
        );

        close();
    }

    /// The same guard `pin_for` has. A session for another Cube must not supply
    /// a credential here either — deriving from the wrong seed would encrypt a
    /// file nothing could open again.
    #[test]
    fn a_passkey_cube_gets_nothing_from_another_cubes_session() {
        use coincube_core::miniscript::bitcoin::secp256k1::Secp256k1;
        use coincube_core::miniscript::bitcoin::Network;

        let _g = guard();
        close();

        let signer = MasterSigner::generate(Network::Bitcoin).unwrap();
        let fp = signer.fingerprint(&Secp256k1::signing_only());
        open_without_pin("cube-other");
        store_unlocked_signer("cube-other", fp, signer);

        let cube = passkey_cube("cube-passkey").with_master_signer(fp);
        assert_eq!(seed_file_password(&cube), None);

        close();
    }

    #[test]
    fn pin_is_not_handed_to_a_different_cube() {
        // The whole reason `pin_for` takes an id: a stale session must not
        // supply credentials for a Cube it does not belong to.
        let _g = guard();
        close();
        open("cube-a", Zeroizing::new("1234".to_string()));
        assert_eq!(pin_for("cube-b"), None);
        close();
    }

    #[test]
    fn reopening_replaces_the_previous_session() {
        let _g = guard();
        close();
        open("cube-a", Zeroizing::new("1234".to_string()));
        open("cube-b", Zeroizing::new("9876".to_string()));
        assert_eq!(pin_for("cube-a"), None);
        assert_eq!(pin_for("cube-b").as_deref(), Some(&"9876".to_string()));
        close();
    }

    fn signer() -> (Fingerprint, MasterSigner) {
        use coincube_core::miniscript::bitcoin::{secp256k1::Secp256k1, Network};
        let secp = Secp256k1::signing_only();
        let s = MasterSigner::generate(Network::Bitcoin).unwrap();
        (s.fingerprint(&secp), s)
    }

    #[test]
    fn the_unlocked_signer_round_trips() {
        let _g = guard();
        close();
        let (fp, s) = signer();
        let words = s.words();

        open("cube-a", Zeroizing::new("1234".to_string()));
        store_unlocked_signer("cube-a", fp, s);

        let got = unlocked_signer("cube-a", fp).expect("the unlock's signer is reusable");
        assert_eq!(got.words(), words);
        // And again — it is a cache, not a one-shot handoff. Liquid and Spark
        // both need one.
        assert_eq!(unlocked_signer("cube-a", fp).unwrap().words(), words);
        close();
    }

    #[test]
    fn the_pin_verified_signer_round_trips() {
        let _g = guard();
        close();
        let (fp, s) = signer();
        let words = s.words();

        store_unlocked_signer("cube-a", fp, s);
        open("cube-a", Zeroizing::new("1234".to_string()));

        let got = unlocked_signer_with_pin_verification("cube-a", fp, "1234")
            .expect("the unlock's own PIN opens the cache");
        assert_eq!(got.words(), words);
        // A cache, not a one-shot: the Recovery Kit flow can be run twice in
        // one session without re-unlocking the Cube.
        assert_eq!(
            unlocked_signer_with_pin_verification("cube-a", fp, "1234")
                .unwrap()
                .words(),
            words
        );
        close();
    }

    #[test]
    fn a_wrong_pin_never_reaches_the_cached_signer() {
        // The whole reason this function exists rather than `unlocked_signer`.
        // The seed-reveal screens prompt for a PIN; if the cache answered
        // regardless, that prompt would be decoration and anyone at an
        // unattended open Cube would get the permanent secret for free.
        let _g = guard();
        close();
        let (fp, s) = signer();

        store_unlocked_signer("cube-a", fp, s);
        open("cube-a", Zeroizing::new("1234".to_string()));

        assert!(
            matches!(
                unlocked_signer_with_pin_verification("cube-a", fp, "1235"),
                Err(SignerError::InvalidPassword)
            ),
            "a wrong PIN must be `InvalidPassword` — it is the only variant \
             `is_wrong_pin` charges to the unlock throttle, so anything else \
             gives an attacker unlimited free guesses at this surface"
        );
        close();
    }

    #[test]
    fn a_passkey_session_refuses_the_pin_door_rather_than_matching_nothing() {
        // A passkey Cube has `pin: None`. This must be `PasswordRequired` —
        // "the cache cannot answer, ask the disk" — and never a match. The
        // callers re-derive through a WebAuthn assertion instead.
        let _g = guard();
        close();
        let (fp, s) = signer();

        store_unlocked_signer("cube-passkey", fp, s);
        open_without_pin("cube-passkey");

        for pin in ["", "1234"] {
            assert!(
                matches!(
                    unlocked_signer_with_pin_verification("cube-passkey", fp, pin),
                    Err(SignerError::PasswordRequired)
                ),
                "a passkey Cube handed out its seed for {:?}",
                pin
            );
        }
        close();
    }

    #[test]
    fn the_pin_door_keeps_the_cube_and_fingerprint_guards() {
        // Same two guards as `unlocked_signer`, and they must not be weakened
        // just because a PIN was also presented. The error *kind* matters as
        // much as the refusal: neither of these is a wrong PIN, so neither may
        // cost the user a throttle penalty for a fault no PIN can fix.
        let _g = guard();
        close();
        let (fp, s) = signer();
        let (other_fp, _other) = signer();

        store_unlocked_signer("cube-a", fp, s);
        open("cube-a", Zeroizing::new("1234".to_string()));

        assert!(
            matches!(
                unlocked_signer_with_pin_verification("cube-b", fp, "1234"),
                Err(SignerError::PasswordRequired)
            ),
            "wrong Cube — even with cube-a's PIN"
        );
        assert!(
            matches!(
                unlocked_signer_with_pin_verification("cube-a", other_fp, "1234"),
                Err(SignerError::SignerNotFound(_))
            ),
            "wrong fingerprint — a Cube can hold a master seed and a Vault hot signer"
        );

        close();
        assert!(
            matches!(
                unlocked_signer_with_pin_verification("cube-a", fp, "1234"),
                Err(SignerError::PasswordRequired)
            ),
            "a locked Cube must not reveal its seed to the PIN it was opened with"
        );
    }

    #[test]
    fn a_session_holding_no_signer_asks_the_caller_to_read_the_disk() {
        // The restore path opens a Cube without ever storing a signer. The
        // right answer is `SignerNotFound` — not a wrong PIN — so the caller
        // falls through to the seed file instead of telling the user their
        // correct PIN was wrong.
        let _g = guard();
        close();
        let (fp, _s) = signer();

        open("cube-a", Zeroizing::new("1234".to_string()));
        assert!(matches!(
            unlocked_signer_with_pin_verification("cube-a", fp, "1234"),
            Err(SignerError::SignerNotFound(_))
        ));
        close();
    }

    #[test]
    fn a_cached_signer_is_never_handed_to_the_wrong_cube_or_key() {
        // The two guards that make caching a decrypted seed acceptable. Getting
        // either wrong hands a caller a valid-looking wallet that is not the
        // one it asked for.
        let _g = guard();
        close();
        let (fp, s) = signer();
        let (other_fp, _other) = signer();

        open("cube-a", Zeroizing::new("1234".to_string()));
        store_unlocked_signer("cube-a", fp, s);

        assert!(unlocked_signer("cube-b", fp).is_none(), "wrong Cube");
        assert!(
            unlocked_signer("cube-a", other_fp).is_none(),
            "wrong fingerprint — a Cube can hold a master seed and a Vault hot signer"
        );
        close();
    }

    #[test]
    fn locking_drops_the_cached_signer() {
        // Otherwise a locked Cube could be reloaded from cache without anyone
        // re-entering a PIN, which would make the auto-lock decorative.
        let _g = guard();
        close();
        let (fp, s) = signer();
        open("cube-a", Zeroizing::new("1234".to_string()));
        store_unlocked_signer("cube-a", fp, s);
        assert!(unlocked_signer("cube-a", fp).is_some());

        close();
        assert!(unlocked_signer("cube-a", fp).is_none());
    }

    #[test]
    fn opening_a_different_cube_drops_the_previous_signer() {
        let _g = guard();
        close();
        let (fp, s) = signer();
        store_unlocked_signer("cube-a", fp, s);
        open("cube-b", Zeroizing::new("9876".to_string()));
        assert!(unlocked_signer("cube-a", fp).is_none());
        close();
    }

    #[test]
    fn store_before_open_survives_the_open() {
        // This is the real ordering: the PIN screen stores the signer from its
        // blocking task, and `gui::tab` calls `open` afterwards. If `open`
        // clobbered the signer, the whole optimisation would silently do
        // nothing — and nothing would fail.
        let _g = guard();
        close();
        let (fp, s) = signer();
        let words = s.words();

        store_unlocked_signer("cube-a", fp, s);
        open("cube-a", Zeroizing::new("1234".to_string()));

        assert_eq!(
            unlocked_signer("cube-a", fp).map(|s| s.words()),
            Some(words),
            "`open` dropped the signer the unlock had already decrypted"
        );
        assert_eq!(pin_for("cube-a").as_deref(), Some(&"1234".to_string()));
        close();
    }

    #[test]
    fn a_passkey_session_has_a_signer_and_no_pin() {
        // The distinction that keeps an empty string out of the Vault
        // installer's seed encryption and out of duress-PIN validation. Both
        // treat `None` as "refuse"; both would happily use `""`.
        let _g = guard();
        close();
        let (fp, s) = signer();
        let words = s.words();

        store_unlocked_signer("cube-passkey", fp, s);
        open_without_pin("cube-passkey");

        assert!(is_open());
        assert_eq!(current_cube_id().as_deref(), Some("cube-passkey"));
        assert_eq!(
            unlocked_signer("cube-passkey", fp).map(|s| s.words()),
            Some(words),
            "`open_without_pin` dropped the signer the assertion derived"
        );
        assert_eq!(
            pin_for("cube-passkey"),
            None,
            "a passkey Cube must never hand out a PIN, not even an empty one"
        );
        assert_eq!(current_pin(), None);
        close();
    }

    #[test]
    fn idle_is_reset_by_touch() {
        let _g = guard();
        close();
        assert_eq!(idle_for(), None);
        open("cube-a", Zeroizing::new("1234".to_string()));
        std::thread::sleep(Duration::from_millis(20));
        let before = idle_for().unwrap();
        assert!(before >= Duration::from_millis(20));
        touch();
        assert!(idle_for().unwrap() < before);
        close();
    }
}
