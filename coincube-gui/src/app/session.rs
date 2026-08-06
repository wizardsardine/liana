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
use coincube_core::signer::MasterSigner;
use zeroize::Zeroizing;

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

/// The open Cube's PIN, if `cube_id` is the Cube that is actually open **and**
/// that Cube has a PIN at all. A passkey Cube answers `None`.
pub fn pin_for(cube_id: &str) -> Option<Zeroizing<String>> {
    lock_session()
        .as_ref()
        .filter(|s| s.cube_id == cube_id)
        .and_then(|s| s.pin.clone())
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
