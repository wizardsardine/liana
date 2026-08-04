//! Escalating delay on wrong PIN attempts.
//!
//! # This is hygiene, not a security control
//!
//! An offline attacker with the datadir never touches the UI — they run
//! Argon2id in a loop against the seed file and this code is not in their way.
//! Say so plainly rather than letting anyone treat it as a defence.
//!
//! What it *does* buy: a laptop thief who opens the app and starts typing
//! doesn't get 10,000 free guesses through the front door. At 831 ms a guess
//! they were already looking at ~2.3 hours; the escalating delay turns that
//! into "not by hand".
//!
//! # Why the state does not live in settings.json
//!
//! Because the user can trivially delete settings.json, and so can the thief.
//! The counter lives in its own file **outside** any network directory —
//! alongside the duress local state, which is kept there for the same reason.
//! Deleting it is still possible; it is just no longer a side effect of the
//! obvious "reset my settings" move.
//!
//! It is deliberately **not** inside a Cube's data directory either: a duress
//! wipe takes those, and a wipe must not hand the attacker a fresh counter.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "unlock-throttle.json";

/// Delay after the Nth consecutive failure: 0, 0, 1s, 2s, 4s, 8s… capped.
///
/// The first two failures are free — a mistyped PIN is the overwhelmingly
/// common case and punishing it teaches users nothing.
const FREE_ATTEMPTS: u32 = 2;
const MAX_DELAY: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThrottleState {
    /// Consecutive failures across the whole device, per Cube.
    #[serde(default)]
    failures: std::collections::BTreeMap<String, CubeThrottle>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct CubeThrottle {
    consecutive_failures: u32,
    /// Unix seconds of the last failure. Absolute rather than an `Instant` so
    /// the penalty survives a restart — otherwise "quit and relaunch" is the
    /// bypass.
    last_failure_unix: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn path(root: &Path) -> PathBuf {
    root.join(FILE_NAME)
}

impl ThrottleState {
    /// Load, mapping a missing file to the empty state.
    ///
    /// A **corrupt** file also maps to empty rather than erroring. That is the
    /// permissive direction, and it is the right one here: this is not a
    /// security control, and failing closed would mean an unparseable counter
    /// file locks a user out of their own wallet permanently.
    pub fn load(root: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path(root)) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    fn save(&self, root: &Path) {
        let Ok(bytes) = serde_json::to_vec_pretty(self) else {
            return;
        };
        if let Err(e) = std::fs::write(path(root), bytes) {
            tracing::debug!("could not persist the unlock throttle: {e}");
        }
    }

    /// How much longer this Cube must wait before another attempt is accepted.
    /// `Duration::ZERO` when it may try now.
    pub fn remaining_lockout(&self, cube_id: &str) -> Duration {
        let Some(entry) = self.failures.get(cube_id) else {
            return Duration::ZERO;
        };
        let penalty = penalty_for(entry.consecutive_failures);
        if penalty.is_zero() {
            return Duration::ZERO;
        }
        let elapsed = now_unix().saturating_sub(entry.last_failure_unix);
        penalty.saturating_sub(Duration::from_secs(elapsed))
    }

    /// Record a wrong PIN and return the new lockout.
    pub fn record_failure(&mut self, root: &Path, cube_id: &str) -> Duration {
        let entry = self.failures.entry(cube_id.to_string()).or_default();
        entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        entry.last_failure_unix = now_unix();
        let penalty = penalty_for(entry.consecutive_failures);
        self.save(root);
        penalty
    }

    /// Clear this Cube's counter after a successful unlock.
    ///
    /// Also called on a **duress** activation: the counter must not survive as
    /// evidence that someone was guessing, and the Cube is being wiped anyway.
    pub fn record_success(&mut self, root: &Path, cube_id: &str) {
        if self.failures.remove(cube_id).is_some() {
            self.save(root);
        }
    }
}

/// 1s, 2s, 4s, 8s… after the free attempts, capped at [`MAX_DELAY`].
fn penalty_for(consecutive_failures: u32) -> Duration {
    if consecutive_failures <= FREE_ATTEMPTS {
        return Duration::ZERO;
    }
    let step = consecutive_failures - FREE_ATTEMPTS - 1;
    // Saturate rather than overflow: 2^32 seconds is not a number we want to
    // reach by shifting.
    let secs = 1u64.checked_shl(step.min(32)).unwrap_or(u64::MAX);
    Duration::from_secs(secs).min(MAX_DELAY)
}

/// User-facing text for a remaining lockout.
pub fn lockout_message(remaining: Duration) -> String {
    let secs = remaining.as_secs().max(1);
    if secs < 60 {
        format!("Too many incorrect PINs. Try again in {secs}s.")
    } else {
        let mins = secs.div_ceil(60);
        format!("Too many incorrect PINs. Try again in {mins} minute(s).")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "coincube-throttle-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn first_attempts_are_free_then_it_escalates() {
        assert_eq!(penalty_for(0), Duration::ZERO);
        assert_eq!(penalty_for(1), Duration::ZERO);
        assert_eq!(penalty_for(2), Duration::ZERO);
        assert_eq!(penalty_for(3), Duration::from_secs(1));
        assert_eq!(penalty_for(4), Duration::from_secs(2));
        assert_eq!(penalty_for(5), Duration::from_secs(4));
        assert_eq!(penalty_for(6), Duration::from_secs(8));
    }

    #[test]
    fn the_delay_is_capped_and_never_overflows() {
        // A determined guesser must not be able to push this into a value that
        // wraps, or into a lockout measured in years.
        assert_eq!(penalty_for(40), MAX_DELAY);
        assert_eq!(penalty_for(u32::MAX), MAX_DELAY);
    }

    #[test]
    fn failures_accumulate_and_success_clears_them() {
        let root = tmp("accumulate");
        let mut st = ThrottleState::load(&root);
        assert_eq!(st.remaining_lockout("cube-a"), Duration::ZERO);

        for _ in 0..3 {
            st.record_failure(&root, "cube-a");
        }
        assert!(st.remaining_lockout("cube-a") > Duration::ZERO);

        st.record_success(&root, "cube-a");
        assert_eq!(st.remaining_lockout("cube-a"), Duration::ZERO);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_penalty_survives_a_restart() {
        // Quitting and relaunching must not reset the counter — otherwise it
        // is a two-keystroke bypass.
        let root = tmp("restart");
        let mut st = ThrottleState::load(&root);
        for _ in 0..4 {
            st.record_failure(&root, "cube-a");
        }
        let reloaded = ThrottleState::load(&root);
        assert!(reloaded.remaining_lockout("cube-a") > Duration::ZERO);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Guesses must accumulate across *every* surface that checks the PIN, not
    /// just the unlock screen.
    ///
    /// The Settings → Backup Master Seed and Recovery Kit flows also decrypt the
    /// seed with a user-supplied PIN. They sit behind an unlocked Cube, so they
    /// are not the laptop-thief case — but they are the route to the
    /// *permanent* secret, and leaving either unthrottled would mean the
    /// counter could be sidestepped by guessing at a different door.
    #[test]
    fn the_counter_is_shared_across_pin_surfaces() {
        let root = tmp("shared");

        // Three failures anywhere put the Cube into lockout...
        let mut st = ThrottleState::load(&root);
        for _ in 0..3 {
            st.record_failure(&root, "cube-a");
        }

        // ...and a *different* surface, loading the state fresh, sees it.
        let seen_elsewhere = ThrottleState::load(&root).remaining_lockout("cube-a");
        assert!(
            seen_elsewhere > Duration::ZERO,
            "a second PIN surface would have offered unlimited fresh guesses"
        );

        // A success anywhere clears it for everyone.
        ThrottleState::load(&root).record_success(&root, "cube-a");
        assert_eq!(
            ThrottleState::load(&root).remaining_lockout("cube-a"),
            Duration::ZERO
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cubes_are_throttled_independently() {
        let root = tmp("independent");
        let mut st = ThrottleState::load(&root);
        for _ in 0..5 {
            st.record_failure(&root, "cube-a");
        }
        assert!(st.remaining_lockout("cube-a") > Duration::ZERO);
        assert_eq!(st.remaining_lockout("cube-b"), Duration::ZERO);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_corrupt_counter_file_does_not_lock_the_user_out() {
        // Failing closed here would mean an unparseable JSON file bricks a
        // wallet. This is hygiene, not a control — it fails open on purpose.
        let root = tmp("corrupt");
        std::fs::write(path(&root), b"{not json").unwrap();
        let st = ThrottleState::load(&root);
        assert_eq!(st, ThrottleState::default());
        assert_eq!(st.remaining_lockout("cube-a"), Duration::ZERO);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lockout_copy_reads_sensibly_at_both_scales() {
        assert!(lockout_message(Duration::from_secs(4)).contains("4s"));
        assert!(lockout_message(Duration::from_secs(120)).contains("2 minute"));
        // Sub-second remainders still say something, never "0s".
        assert!(lockout_message(Duration::from_millis(200)).contains("1s"));
    }
}
