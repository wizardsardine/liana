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

use keyring::Entry;
use zeroize::Zeroizing;

use coincube_core::seed_crypt::DeviceSecret;

use super::UnlockError;

/// Keyring service name. Versioned so a future re-key can coexist.
const SERVICE: &str = "io.coincube.tenshu.device-secret.v1";

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

fn entry(cube_id: &str) -> Result<Entry, UnlockError> {
    Entry::new(SERVICE, cube_id).map_err(|e| UnlockError::KeystoreUnreachable(e.to_string()))
}

/// Probe the keystore by writing and reading back a throwaway item.
///
/// A real round-trip, not a "does the crate compile" check: on Linux the
/// Secret Service may be absent, present-but-locked, or present-and-broken, and
/// only one of those is usable. The probe item is deleted afterwards; a failure
/// to delete is not treated as a probe failure.
pub fn capability() -> Capability {
    const PROBE_ID: &str = "__capability_probe__";
    let probe = match Entry::new(SERVICE, PROBE_ID) {
        Ok(e) => e,
        Err(e) => return Capability::Unavailable(describe(&e.to_string())),
    };
    if let Err(e) = probe.set_password("probe") {
        return Capability::Unavailable(describe(&e.to_string()));
    }
    let read_back = probe.get_password();
    let _ = probe.delete_credential();
    match read_back {
        Ok(v) if v == "probe" => Capability::Available,
        Ok(_) => Capability::Unavailable(
            "Your system keychain returned unexpected data. Coincube can't rely on it \
             to protect this Cube."
                .to_string(),
        ),
        Err(e) => Capability::Unavailable(describe(&e.to_string())),
    }
}

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

/// Fetch this Cube's device secret.
///
/// Three outcomes, and keeping them apart is the whole point (I7):
///
/// - `Ok(Some(secret))` — usable.
/// - `Err(EntryMissing)` — the keystore works but has no item for this Cube.
///   Either the Cube predates v3, or the entry was deleted / the keychain was
///   reset. For a v3 file that is unrecoverable from this machine, and the copy
///   must point at the Recovery Kit.
/// - `Err(KeystoreUnreachable)` — the keystore itself couldn't be reached
///   (locked keychain, no Secret Service, D-Bus down). Transient; retrying
///   after unlocking the keychain works.
///
/// None of the three is ever reported as a wrong PIN.
pub fn load(cube_id: &str) -> Result<DeviceSecret, UnlockError> {
    let entry = entry(cube_id)?;
    match entry.get_password() {
        Ok(encoded) => decode(&encoded),
        Err(keyring::Error::NoEntry) => Err(UnlockError::DeviceSecretMissing),
        Err(e) => Err(UnlockError::KeystoreUnreachable(describe(&e.to_string()))),
    }
}

/// Fetch the secret, mapping "no entry" to `None`. For callers that are asking
/// *whether* this Cube is on v3 rather than trying to open it.
pub fn load_optional(cube_id: &str) -> Result<Option<DeviceSecret>, UnlockError> {
    match load(cube_id) {
        Ok(s) => Ok(Some(s)),
        Err(UnlockError::DeviceSecretMissing) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Mint a device secret for `cube_id` if it doesn't have one, and return it.
///
/// Idempotent: an existing entry is returned untouched. Overwriting one would
/// make every v3 file sealed under it permanently unopenable, so this function
/// must never be "fixed" into an unconditional write.
pub fn get_or_create(cube_id: &str) -> Result<DeviceSecret, UnlockError> {
    if let Some(existing) = load_optional(cube_id)? {
        return Ok(existing);
    }

    let mut secret: DeviceSecret = Zeroizing::new([0u8; 32]);
    {
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(secret.as_mut());
    }

    let entry = entry(cube_id)?;
    let encoded = Zeroizing::new(encode(&secret));
    entry
        .set_password(&encoded)
        .map_err(|e| UnlockError::KeystoreUnreachable(describe(&e.to_string())))?;

    // Read it straight back. A keystore that accepts a write and then can't
    // return it would otherwise strand the Cube at the *next* open, long after
    // the user could do anything about it.
    let verify = load(cube_id)?;
    if verify[..] != secret[..] {
        return Err(UnlockError::KeystoreUnreachable(
            "Your system keychain didn't store this Cube's key correctly.".to_string(),
        ));
    }
    Ok(secret)
}

/// Remove this Cube's entry. Used when Cube creation is abandoned after the
/// secret was minted, so a retry doesn't inherit a secret whose seed file was
/// never written.
///
/// **Not** called by the duress wipe — see the module docs.
pub fn delete(cube_id: &str) -> Result<(), UnlockError> {
    match entry(cube_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(UnlockError::KeystoreUnreachable(describe(&e.to_string()))),
    }
}

fn encode(secret: &DeviceSecret) -> String {
    secret.iter().map(|b| format!("{:02x}", b)).collect()
}

fn decode(encoded: &str) -> Result<DeviceSecret, UnlockError> {
    let bytes = hex::decode(encoded).map_err(|_| {
        UnlockError::KeystoreUnreachable(
            "This Cube's system-keychain entry is corrupt.".to_string(),
        )
    })?;
    if bytes.len() != 32 {
        return Err(UnlockError::KeystoreUnreachable(
            "This Cube's system-keychain entry is the wrong size.".to_string(),
        ));
    }
    let mut out: DeviceSecret = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let secret: DeviceSecret = Zeroizing::new([0xABu8; 32]);
        let encoded = encode(&secret);
        assert_eq!(encoded.len(), 64);
        assert_eq!(decode(&encoded).unwrap()[..], secret[..]);
    }

    #[test]
    fn corrupt_entry_is_a_keystore_error_not_a_wrong_pin() {
        // I7 again: whatever went wrong with the keychain, it is never the
        // user's PIN that gets blamed.
        assert!(matches!(
            decode("nothex"),
            Err(UnlockError::KeystoreUnreachable(_))
        ));
        assert!(matches!(
            decode("abcd"),
            Err(UnlockError::KeystoreUnreachable(_))
        ));
    }

    #[test]
    fn linux_unavailable_copy_names_the_real_remedy() {
        // The message a headless-Linux user sees has to tell them what to do,
        // not just that something failed.
        let msg = describe("connection refused");
        assert!(msg.contains("connection refused"));
        if cfg!(target_os = "linux") {
            assert!(msg.contains("Secret Service"));
        }
    }
}
