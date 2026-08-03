//! Secrets that live for exactly as long as a Cube is unlocked.
//!
//! Today that is one thing: the unlock PIN. It is needed after unlock, not
//! just during it —
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
//! `BreezClient`'s signer and inside the Spark bridge subprocess. A 4-digit PIN
//! alongside it does not change the threat model, and this module is what makes
//! it possible to *drop* both deterministically on lock.
//!
//! Every accessor returns a `Zeroizing` clone, so callers cannot hold a
//! reference across a lock.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

struct Session {
    /// The Cube this PIN belongs to. Checked on read so a stale session from a
    /// previously-open Cube can never hand its PIN to a different one.
    cube_id: String,
    pin: Zeroizing<String>,
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

/// Record a successful unlock. Replaces any previous session — opening a
/// second Cube without closing the first is not a thing the UI can do, but if
/// it ever becomes one, the newest wins and the old PIN is zeroized here.
pub fn open(cube_id: impl Into<String>, pin: Zeroizing<String>) {
    *lock_session() = Some(Session {
        cube_id: cube_id.into(),
        pin,
        last_activity: Instant::now(),
    });
}

/// The open Cube's PIN, if `cube_id` is the Cube that is actually open.
pub fn pin_for(cube_id: &str) -> Option<Zeroizing<String>> {
    lock_session()
        .as_ref()
        .filter(|s| s.cube_id == cube_id)
        .map(|s| s.pin.clone())
}

/// The open Cube's PIN, whichever Cube that is. Use [`pin_for`] when the
/// caller knows which Cube it means — this exists for the installer, which
/// runs before the restored Cube's id is minted.
pub fn current_pin() -> Option<Zeroizing<String>> {
    lock_session().as_ref().map(|s| s.pin.clone())
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
    lock_session()
        .as_ref()
        .map(|s| s.last_activity.elapsed())
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
