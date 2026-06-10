//! Duress mode — on-device state model and persistence.
//!
//! The desktop is where duress *happens*. The on-device trust anchor is an
//! atomic local wipe of all Cube data (implemented in later phases); this
//! module holds the durable state that survives that wipe and drives the
//! orchestration.
//!
//! Two files live at the **root** of the Coincube data directory, *outside*
//! the per-network `cubes/` tree, so they survive `wipe::execute_atomic()`:
//!
//!   * [`DURESS_STATE_FILE`] — [`DuressLocalState`], the per-device duress
//!     configuration and live status.
//!   * [`DURESS_QUEUE_FILE`] — the persistent retry queue of
//!     [`PendingActivation`]s (drained by the Phase 4 background loop). This is
//!     the source of truth that guarantees the activation `POST` eventually
//!     fires even if power is pulled mid-activation.
//!
//! Both stores are written atomically (temp file + `fsync` + `rename`) so a
//! crash never leaves a half-written JSON document on disk.

pub mod cipher;
pub mod drain;
pub mod enroll;
pub mod journal;
pub mod orchestrator;
pub mod queue;
pub mod wipe;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// UI-facing duress lifecycle events. Emitted by the orchestrator and the gRPC
/// stream dispatcher; the app shell routes them to the cryptic screen (Phase 5)
/// and back.
///
/// `Activated` (local duress-PIN path) implies the local Cube wipe ran;
/// `ActivatedRemote` (Phase 7b, triggered elsewhere) locks the screen but does
/// **not** wipe — the only behavioural difference between the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuressEvent {
    /// Local duress-PIN activation — Cube data was wiped on this device.
    Activated,
    /// Remote activation received over the Connect gRPC stream — screen locks,
    /// no wipe.
    ActivatedRemote,
    /// Duress cleared (server-side all-clear) — exit the cryptic screen.
    Cleared,
}

/// Filename for the persisted [`DuressLocalState`], relative to the Coincube
/// data-directory root.
pub const DURESS_STATE_FILE: &str = "duress-state.json";

/// Filename for the persisted activation retry queue, relative to the Coincube
/// data-directory root.
pub const DURESS_QUEUE_FILE: &str = "duress-queue.json";

/// Filename for this device's stable enrollment fingerprint, relative to the
/// Coincube data-directory root.
pub const DEVICE_FINGERPRINT_FILE: &str = "duress-device-fingerprint";

/// Loads this device's **stable** duress enrollment fingerprint, generating and
/// persisting one (a random UUID) on first use.
///
/// The server keys its per-device duress rows and `this_device_registered`
/// semantics on this value, so it MUST be the same across launches, repeat
/// enrollments, and re-registrations on the same install — a fresh value each
/// time would make the server unable to recognise the same desktop. Persisted
/// at the data-directory root, outside the wiped `cubes/` tree.
pub fn device_fingerprint(coincube_dir: &Path) -> io::Result<String> {
    let path = coincube_dir.join(DEVICE_FINGERPRINT_FILE);
    match std::fs::read_to_string(&path) {
        // Accept only a well-formed UUID. A non-empty-but-corrupt file (e.g. a
        // truncated write from a crash) must NOT be handed to the server as a
        // fingerprint — treat it like "missing" and re-mint.
        Ok(s) if uuid::Uuid::parse_str(s.trim()).is_ok() => Ok(s.trim().to_string()),
        Ok(_) => create_fingerprint(&path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_fingerprint(&path),
        Err(e) => Err(e),
    }
}

/// Mints a fresh fingerprint and persists it atomically (temp file + `fsync` +
/// `rename`), so a crash mid-write can never leave a truncated id at the final
/// path — a reader sees either the old file or the complete new one.
fn create_fingerprint(path: &Path) -> io::Result<String> {
    let fp = uuid::Uuid::new_v4().to_string();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(fp.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(fp)
}

/// This desktop's persisted duress configuration and live status.
///
/// `duress_code` is **this desktop's own** ~128-bit code, generated locally
/// with a CSPRNG and stored encrypted at rest with a key separate from the Cube
/// key (the encryption layer is added in Phase 3). The server holds only the
/// argon2id hash, on a per-device row; the plaintext never leaves this desktop
/// and the user never sees it. Other desktops on the same account each generate
/// their own independent code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuressLocalState {
    /// Whether duress is enrolled for this account from this desktop's view.
    pub enrolled: bool,
    /// Whether duress is currently active (locks the app into the cryptic
    /// screen).
    pub active: bool,
    /// When the account can be cleared with the all-clear passphrase. Only the
    /// recovery flow on a *trusted device* ever renders this to the user.
    #[serde(default)]
    pub unlock_at: Option<DateTime<Utc>>,
    /// Timestamp of the last local activation attempt (diagnostic only).
    #[serde(default)]
    pub last_activation_attempt: Option<DateTime<Utc>>,
    /// The in-flight activation, mirrored from the retry queue for quick
    /// inspection. The queue file is the durable source of truth.
    #[serde(default)]
    pub pending_activation: Option<PendingActivation>,
    /// This desktop's own duress code (encrypted at rest in Phase 3).
    #[serde(default)]
    pub duress_code: Option<String>,
    /// The Connect account id this device enrolled under, persisted so the
    /// unauthenticated `trigger-with-code` POST can address the right account
    /// at activation time even without a live session. `None` for sovereign
    /// (no-Connect) enrollment — that path wipes locally with no server POST.
    #[serde(default)]
    pub account_id: Option<String>,
}

/// A durably-enqueued activation `POST` that must eventually reach Connect.
///
/// Enqueued *before* the wipe begins so that even if power is pulled in the
/// next millisecond, the queue + journal together guarantee both the `POST` and
/// the wipe complete on next launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingActivation {
    pub account_id: String,
    /// Copied from [`DuressLocalState::duress_code`] at enqueue time; carries
    /// the same encrypted-at-rest treatment.
    pub duress_code: String,
    pub enqueued_at: DateTime<Utc>,
    #[serde(default)]
    pub attempts: u32,
}

impl DuressLocalState {
    /// Absolute path to the state file given the Coincube data-directory root.
    pub fn path(coincube_dir: &Path) -> PathBuf {
        coincube_dir.join(DURESS_STATE_FILE)
    }

    /// Loads the persisted state, returning [`DuressLocalState::default`] when
    /// the file does not exist yet (first launch / never enrolled).
    pub fn load(coincube_dir: &Path) -> io::Result<Self> {
        load_json(&Self::path(coincube_dir))
    }

    /// Atomically persists the state to the state file.
    pub fn save(&self, coincube_dir: &Path) -> io::Result<()> {
        write_json_atomic(&Self::path(coincube_dir), self)
    }
}

/// Reads and deserializes a JSON document, treating "file not found" as
/// `T::default()`. Any other I/O or parse error is surfaced.
fn load_json<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e),
    }
}

/// Serializes `value` to pretty JSON and writes it to `path` atomically:
/// write to a sibling `*.tmp`, `fsync`, then `rename` over the target. The
/// rename is atomic on every supported platform, so a reader never observes a
/// torn write and a crash leaves either the old file or the new one — never a
/// truncated document.
fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_state() -> DuressLocalState {
        DuressLocalState {
            enrolled: true,
            active: false,
            unlock_at: Some(Utc.with_ymd_and_hms(2026, 6, 9, 12, 0, 0).unwrap()),
            last_activation_attempt: None,
            pending_activation: Some(PendingActivation {
                account_id: "acct_123".to_string(),
                duress_code: "enc:deadbeef".to_string(),
                enqueued_at: Utc.with_ymd_and_hms(2026, 6, 8, 9, 30, 0).unwrap(),
                attempts: 2,
            }),
            duress_code: Some("enc:cafebabe".to_string()),
            account_id: Some("acct_123".to_string()),
        }
    }

    #[test]
    fn state_serde_round_trip() {
        let state = sample_state();
        let json = serde_json::to_string(&state).unwrap();
        let back: DuressLocalState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn camel_case_wire_shape() {
        let state = sample_state();
        let json = serde_json::to_string(&state).unwrap();
        // Field names must be camelCase on the wire.
        assert!(json.contains("\"lastActivationAttempt\""));
        assert!(json.contains("\"pendingActivation\""));
        assert!(json.contains("\"duressCode\""));
        assert!(json.contains("\"enqueuedAt\""));
    }

    #[test]
    fn device_fingerprint_is_stable_across_calls() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-fp-{}-{:p}",
            std::process::id(),
            &0u8
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = device_fingerprint(&dir).unwrap();
        let b = device_fingerprint(&dir).unwrap();
        assert_eq!(a, b, "fingerprint must be stable across calls");
        assert!(!a.trim().is_empty());
        // A fresh data dir yields a different fingerprint.
        let dir2 = dir.join("other");
        std::fs::create_dir_all(&dir2).unwrap();
        assert_ne!(device_fingerprint(&dir2).unwrap(), a);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn device_fingerprint_rejects_corrupt_file_and_remints() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-fp-corrupt-{}-{:p}",
            std::process::id(),
            &0u8
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A truncated / non-UUID value (e.g. left by a crash mid-write) must
        // not be accepted; it gets re-minted into a valid UUID.
        let path = dir.join(DEVICE_FINGERPRINT_FILE);
        std::fs::write(&path, b"not-a-uuid-truncat").unwrap();
        let fp = device_fingerprint(&dir).unwrap();
        assert!(
            uuid::Uuid::parse_str(&fp).is_ok(),
            "re-minted value must be a UUID"
        );
        // The corrupt content is gone, and the value is now stable.
        assert_eq!(device_fingerprint(&dir).unwrap(), fp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_is_default() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-test-missing-{}",
            std::process::id()
        ));
        // Ensure it does not exist.
        let _ = std::fs::remove_dir_all(&dir);
        let loaded = DuressLocalState::load(&dir).unwrap();
        assert_eq!(loaded, DuressLocalState::default());
    }

    #[test]
    fn save_then_load_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-test-rt-{}-{:p}",
            std::process::id(),
            &0u8
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let state = sample_state();
        state.save(&dir).unwrap();
        let loaded = DuressLocalState::load(&dir).unwrap();
        assert_eq!(state, loaded);
        // No stray temp file left behind.
        assert!(!dir.join("duress-state.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
