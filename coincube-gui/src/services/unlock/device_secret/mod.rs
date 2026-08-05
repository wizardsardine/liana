//! Per-Cube device secret, held in the OS keystore.
//!
//! 32 CSPRNG bytes minted once per Cube and mixed into the seed file's key
//! (`ENCRYPTED_V3`). It never leaves this machine and no duress path consults
//! it, which is what distinguishes it from the server-issued wrapping key the
//! duress design rejects — see
//! `company-brain/decisions/2026-08-01-local-device-secret-two-key-cube-encryption.md`
//! (invariants I4 and I6).
//!
//! # What this buys, and what it does not
//!
//! It defends the **offline** case: a stolen disk, a leaked backup, a recovered
//! drive. Copying the datadir no longer copies the wallet.
//!
//! It does **not** defend against malware running as the user — that code can
//! ask the keystore for the same item this code does. Do not let any security
//! copy claim otherwise.
//!
//! # A duress wipe leaves the entry behind
//!
//! Deliberately. After a wipe the seed file is gone, so the secret is inert
//! either way, and adding a keystore write to the wipe path — a path that must
//! not fail — buys nothing. Matches existing practice for the duress retry
//! queue and `services/duress/cipher.rs`.

#[cfg(target_os = "macos")]
mod apple;

use std::path::Path;
use std::sync::{Mutex, OnceLock};

use zeroize::Zeroizing;

use coincube_core::seed_crypt::DeviceSecret;

use super::UnlockError;

/// Keyring service name. Versioned so a future re-key can coexist.
const SERVICE: &str = "io.coincube.tenshu.device-secret.v1";

/// Serialises provisioning **within this process**.
///
/// The file lock below covers separate processes, but two threads in one
/// process open separate file descriptions and `flock` semantics there are
/// platform-dependent, so the two guards are not redundant.
fn provision_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// Serialises provisioning **across processes** — two Coincube instances on one
/// machine, or an app launched twice.
///
/// Held for the whole check-then-create window. Without it both instances see
/// "no entry", both mint, and one wins the keyring while the other has already
/// sealed a seed file under the loser: an unopenable Cube, created by a race
/// nobody would reproduce on demand.
///
/// The lock file lives beside the other root-level state (`duress-*.json`,
/// `unlock-throttle.json`) rather than inside a network directory, because the
/// keyring is per-machine, not per-network.
fn cross_process_guard(datadir_root: &Path) -> Result<std::fs::File, UnlockError> {
    use fs4::fs_std::FileExt;
    let path = datadir_root.join(".device-secret.lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| UnlockError::Io(format!("couldn't open the provisioning lock: {e}")))?;
    file.lock_exclusive()
        .map_err(|e| UnlockError::Io(format!("couldn't take the provisioning lock: {e}")))?;
    Ok(file)
}

/// What the platform can actually do, probed rather than assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    /// A keystore is present and round-trips a value.
    Available,
    /// No usable keystore. Carries a user-facing reason.
    ///
    /// On Linux this is the common case: a headless box or a minimal WM has no
    /// Secret Service at all. Per invariant I7 the app **refuses at Cube
    /// creation** rather than silently falling back to PIN-only — a user who
    /// believes they have two-factor protection and has one is worse off than a
    /// user who was told the truth.
    Unavailable(String),
}

impl Capability {
    pub fn is_available(&self) -> bool {
        matches!(self, Capability::Available)
    }
}

/// Raw-byte keychain access, split by platform.
///
/// macOS goes through `SecItem` directly ([`apple`]) because `keyring` 3.3
/// cannot set the two attributes the design requires. Everywhere else keyring's
/// byte API is used — note `get_secret`/`set_secret`, not
/// `get_password`/`set_password`: the string form forced a hex round-trip
/// through a `String` the caller could not scrub.
mod backend {
    use super::{UnlockError, SERVICE};
    use zeroize::Zeroizing;

    #[cfg(target_os = "macos")]
    pub fn load(cube_id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, UnlockError> {
        super::apple::load(SERVICE, cube_id)
    }

    #[cfg(target_os = "macos")]
    pub fn add_if_absent(cube_id: &str, secret: &[u8]) -> Result<(), UnlockError> {
        super::apple::add_if_absent(SERVICE, cube_id, secret)?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    pub fn verify_attributes(cube_id: &str) -> Result<(), UnlockError> {
        super::apple::verify_attributes(SERVICE, cube_id)
    }

    #[cfg(target_os = "macos")]
    pub fn delete(cube_id: &str) -> Result<(), UnlockError> {
        super::apple::delete(SERVICE, cube_id)
    }

    #[cfg(not(target_os = "macos"))]
    fn entry(cube_id: &str) -> Result<keyring::Entry, UnlockError> {
        keyring::Entry::new(SERVICE, cube_id)
            .map_err(|e| UnlockError::KeystoreUnreachable(super::describe(&e.to_string())))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn load(cube_id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, UnlockError> {
        match entry(cube_id)?.get_secret() {
            // Wrapped the instant it exists — before any length check or
            // conversion can return early past it.
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(UnlockError::KeystoreUnreachable(super::describe(
                &e.to_string(),
            ))),
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn add_if_absent(cube_id: &str, secret: &[u8]) -> Result<(), UnlockError> {
        // Windows Credential Manager and Secret Service have no atomic
        // create-if-absent. The caller holds both provisioning locks, so the
        // check-then-set window is closed against this application; a foreign
        // writer racing us is out of scope, and the mandatory read-back in
        // `get_or_create` still returns whatever actually landed.
        entry(cube_id)?
            .set_secret(secret)
            .map_err(|e| UnlockError::KeystoreUnreachable(super::describe(&e.to_string())))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn verify_attributes(_cube_id: &str) -> Result<(), UnlockError> {
        // Windows DPAPI and Secret Service expose no equivalent of
        // `kSecAttrSynchronizable`; neither syncs these items off-machine by
        // default. Nothing to assert, so nothing is claimed.
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn delete(cube_id: &str) -> Result<(), UnlockError> {
        match entry(cube_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(UnlockError::KeystoreUnreachable(super::describe(
                &e.to_string(),
            ))),
        }
    }
}

/// Probe the keystore by writing and reading back a throwaway item.
///
/// A real round-trip, not a "does the crate compile" check: on Linux the
/// Secret Service may be absent, present-but-locked, or present-and-broken, and
/// only one of those is usable. The probe item is deleted afterwards; a failure
/// to delete is not treated as a probe failure.
pub fn capability() -> Capability {
    let probe = probe_account();
    let probe_value = [0x5Au8; 32];

    let _ = backend::delete(&probe);
    if let Err(e) = backend::add_if_absent(&probe, &probe_value) {
        return Capability::Unavailable(e.to_string());
    }
    let read_back = backend::load(&probe);
    // The attribute contract is part of "usable", not a separate nicety: an
    // item that syncs to iCloud is worse than no item at all, because the user
    // believes they have a second factor that is in fact escrowed.
    let attrs = backend::verify_attributes(&probe);
    let _ = backend::delete(&probe);

    match (read_back, attrs) {
        (Ok(Some(v)), Ok(())) if v.as_slice() == probe_value => Capability::Available,
        // Written, then gone. Its own outcome: "returned unexpected data" points
        // the user at a corrupt keychain when what they have is one that does
        // not keep what it accepts.
        (Ok(None), Ok(())) => Capability::Unavailable(
            "Your system keychain accepted a test item and then reported it missing. \
             Coincube can't rely on it to protect this Cube."
                .to_string(),
        ),
        (Ok(_), Ok(())) => Capability::Unavailable(
            "Your system keychain returned unexpected data. Coincube can't rely on it \
             to protect this Cube."
                .to_string(),
        ),
        (Err(e), _) | (_, Err(e)) => Capability::Unavailable(e.to_string()),
    }
}

/// A keychain account name used by exactly one [`capability`] call.
///
/// A fixed name made two probes destructive to each other: the second one's
/// opening `delete` removes the item the first just wrote, so the first reads
/// back nothing and reports the keychain unusable — which refuses Cube creation
/// on a machine where nothing is wrong. Two Coincube windows opening the create
/// form together is enough to hit it.
///
/// Same shape as the tests' `probe_cube_id`, plus a counter so two threads
/// inside one process cannot land on the same name whatever the clock's
/// resolution.
fn probe_account() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "__capability_probe__-{}-{}-{}",
        std::process::id(),
        nanos,
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// Only the keyring-backed platforms surface raw crate errors; macOS builds its
/// own messages in [`apple`], so this is dead there rather than missing.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn describe(detail: &str) -> String {
    if cfg!(target_os = "linux") {
        format!(
            "No system keyring is available on this session ({detail}). Coincube stores \
             part of your Cube's encryption key in the keyring, so this Cube can't be \
             created here. Log in to a desktop session with a running Secret Service \
             (GNOME Keyring or KWallet), or use a hardware security key."
        )
    } else {
        format!("Coincube couldn't reach your system keychain ({detail}).")
    }
}

fn to_secret(bytes: &Zeroizing<Vec<u8>>) -> Result<DeviceSecret, UnlockError> {
    if bytes.len() != 32 {
        return Err(UnlockError::KeystoreUnreachable(
            "This Cube's system-keychain entry is the wrong size.".to_string(),
        ));
    }
    let mut out: DeviceSecret = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(bytes);
    Ok(out)
}

/// Fetch this Cube's device secret.
///
/// Three outcomes, and keeping them apart is the whole point (I7):
///
/// - `Ok(secret)` — usable.
/// - `Err(DeviceSecretMissing)` — the keystore works but has no item for this
///   Cube. For a v3 seed file that is terminal on this machine, and the copy
///   must point at the Recovery Kit.
/// - `Err(KeystoreUnreachable)` — the keystore itself couldn't be reached
///   (locked keychain, no Secret Service, D-Bus down). Transient.
///
/// None of the three is ever reported as a wrong PIN.
pub fn load(cube_id: &str) -> Result<DeviceSecret, UnlockError> {
    match backend::load(cube_id)? {
        Some(bytes) => to_secret(&bytes),
        None => Err(UnlockError::DeviceSecretMissing),
    }
}

/// Fetch the secret, mapping "no entry" to `None`. For callers asking *whether*
/// this Cube is on v3 rather than trying to open it.
pub fn load_optional(cube_id: &str) -> Result<Option<DeviceSecret>, UnlockError> {
    match load(cube_id) {
        Ok(s) => Ok(Some(s)),
        Err(UnlockError::DeviceSecretMissing) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Mint a device secret for `cube_id` if it doesn't have one, and return the
/// value the keyring actually holds.
///
/// # Race safety
///
/// Provisioning is serialised by two guards — a process-global mutex and an
/// exclusive lock on `<datadir>/.device-secret.lock` — held across the whole
/// check-then-create. On macOS the write itself is additionally atomic:
/// `SecItemAdd` returns `errSecDuplicateItem` rather than replacing, so a
/// concurrent winner is never clobbered.
///
/// The return value is **always re-read from the keystore**, never the locally
/// generated candidate. That is the property that matters: whatever raced, the
/// bytes handed back are the bytes the keyring will hand back at the next
/// unlock. Returning the candidate would let a loser seal a seed file under a
/// secret the keyring does not hold — an unopenable Cube, produced by a race.
///
/// Idempotent: an existing entry is returned untouched. Overwriting one would
/// make every v3 file sealed under it permanently unopenable, so this must
/// never be "fixed" into an unconditional write.
///
/// Either way the entry's attributes are verified before it is handed back —
/// a pre-existing item gets the same contract as one minted here.
pub fn get_or_create(datadir_root: &Path, cube_id: &str) -> Result<DeviceSecret, UnlockError> {
    let _in_process = provision_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _cross_process = cross_process_guard(datadir_root)?;

    let secret = match load_optional(cube_id)? {
        Some(existing) => existing,
        None => {
            let mut candidate: DeviceSecret = Zeroizing::new([0u8; 32]);
            {
                use rand::RngCore;
                rand::rngs::OsRng.fill_bytes(candidate.as_mut());
            }
            backend::add_if_absent(cube_id, candidate.as_ref())?;
            // Read back unconditionally — see the note above.
            load(cube_id)?
        }
    };

    // Both paths, one check. A keystore that accepts a write and then can't
    // return it would otherwise strand the Cube at the *next* open, long after
    // the user could act — and an entry that predates the attribute contract
    // (an older build wrote these through `keyring`'s legacy file-keychain
    // API, which sets neither attribute) is exactly the case the existing-item
    // path used to wave through. An item quietly syncing to iCloud is worse
    // than no item at all: the user believes they have a second factor that is
    // in fact escrowed.
    //
    // Read-only, so this does not disturb the idempotence above.
    backend::verify_attributes(cube_id)?;
    Ok(secret)
}

/// Remove this Cube's entry.
///
/// # Nothing in the app calls this on abandoned creation
///
/// It used to say it did. It does not, and adding it would not help: creation
/// retries reuse `pending_cube_id`, so a second attempt provisions the *same*
/// entry and `get_or_create` returns it untouched. The orphan a truly abandoned
/// creation leaves is 32 bytes keyed by a Cube id that will never exist again —
/// inert, and cheaper to leave than to add a delete on a failure path.
///
/// It is kept because the tests need it and because a deliberate "forget this
/// Cube" action would want it. **Not** called by the duress wipe either — see
/// the module docs for why a wipe deliberately leaves the entry behind.
pub fn delete(cube_id: &str) -> Result<(), UnlockError> {
    backend::delete(cube_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "coincube-devsecret-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A distinct id per run so concurrent test binaries don't collide in the
    /// shared login keychain.
    fn probe_cube_id(tag: &str) -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        format!(
            "test-{}-{}-{}",
            tag,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    #[test]
    fn wrong_sized_entries_are_a_keystore_error_not_a_wrong_pin() {
        // I7: whatever went wrong with the keychain, it is never the user's PIN
        // that gets blamed.
        assert!(matches!(
            to_secret(&Zeroizing::new(vec![0u8; 16])),
            Err(UnlockError::KeystoreUnreachable(_))
        ));
        assert!(matches!(
            to_secret(&Zeroizing::new(Vec::new())),
            Err(UnlockError::KeystoreUnreachable(_))
        ));
        assert!(to_secret(&Zeroizing::new(vec![7u8; 32])).is_ok());
    }

    /// Probes must not be able to delete each other's items.
    ///
    /// `capability()` opens by deleting its own probe account. When that name
    /// was a constant, a second probe wiped the first one's item mid-round-trip
    /// and the first reported the keychain unusable — refusing Cube creation on
    /// a machine where nothing was wrong.
    #[test]
    fn each_capability_probe_gets_its_own_account() {
        let names: std::collections::HashSet<String> = (0..64).map(|_| probe_account()).collect();
        assert_eq!(names.len(), 64, "probe account names repeated: {:?}", names);

        // Concurrency is the case that matters — a shared clock reading is not
        // enough on its own.
        let threads: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(|| (0..32).map(|_| probe_account()).collect::<Vec<_>>()))
            .collect();
        let concurrent: std::collections::HashSet<String> = threads
            .into_iter()
            .flat_map(|h| h.join().expect("probe thread"))
            .collect();
        assert_eq!(
            concurrent.len(),
            8 * 32,
            "two threads shared a probe account"
        );
    }

    #[test]
    fn linux_unavailable_copy_names_the_real_remedy() {
        let msg = describe("connection refused");
        assert!(msg.contains("connection refused"));
        if cfg!(target_os = "linux") {
            assert!(msg.contains("Secret Service"));
        }
    }

    /// **The race regression test.**
    ///
    /// Two threads provisioning the same Cube must not mint different secrets.
    /// If they do, one of them seals a seed file under a value the keyring does
    /// not hold and that Cube can never be opened again — a permanent loss
    /// produced by a race nobody would reproduce on demand.
    ///
    /// This exercises the real keychain, the real locks, and the real
    /// read-back.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"
    )]
    fn concurrent_provisioning_yields_one_secret() {
        let root = tmp_root("race");
        let cube_id = probe_cube_id("race");
        let _ = delete(&cube_id);

        const THREADS: usize = 8;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let root = root.clone();
            let cube_id = cube_id.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                // Line every thread up so they contend for real.
                barrier.wait();
                get_or_create(&root, &cube_id).map(|s| s.to_vec())
            }));
        }

        let results: Vec<Vec<u8>> = handles
            .into_iter()
            .map(|h| {
                h.join()
                    .expect("thread panicked")
                    .expect("provisioning failed")
            })
            .collect();

        let first = &results[0];
        assert_eq!(first.len(), 32);
        for (i, r) in results.iter().enumerate() {
            assert_eq!(
                r, first,
                "thread {i} received a different secret — a racing writer would have \
                 sealed a seed file the keyring can no longer open"
            );
        }

        // And what every thread got is what the keyring actually holds.
        let stored = load(&cube_id).expect("entry exists").to_vec();
        assert_eq!(&stored, first, "the returned value is not the stored value");

        delete(&cube_id).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    /// A second call must return the existing secret, never a fresh one —
    /// overwriting would orphan every v3 file already sealed under it.
    #[test]
    #[cfg_attr(
        target_os = "macos",
        ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"
    )]
    fn provisioning_is_idempotent() {
        let root = tmp_root("idempotent");
        let cube_id = probe_cube_id("idempotent");
        let _ = delete(&cube_id);

        let a = get_or_create(&root, &cube_id).unwrap().to_vec();
        let b = get_or_create(&root, &cube_id).unwrap().to_vec();
        assert_eq!(a, b);

        delete(&cube_id).unwrap();
        assert!(matches!(
            load(&cube_id),
            Err(UnlockError::DeviceSecretMissing)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Missing entry and unreachable keystore stay distinct from each other and
    /// from a wrong PIN (I7).
    #[test]
    fn missing_entry_is_its_own_error() {
        let cube_id = probe_cube_id("absent");
        let _ = delete(&cube_id);
        assert!(matches!(
            load(&cube_id),
            Err(UnlockError::DeviceSecretMissing)
        ));
        assert_eq!(load_optional(&cube_id).unwrap(), None);
    }
}
