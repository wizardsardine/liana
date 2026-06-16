//! At-rest encryption for duress secrets (Phase 3).
//!
//! The duress code and any queued copy of it are encrypted at rest with a
//! **device key that is separate from the Cube key** (trust-posture invariant:
//! the wipe destroys Cube material, but the duress queue + code must survive it,
//! so they cannot be protected by the Cube key). The device key lives in
//! `duress.key` at the data-directory root — outside the `cubes/` tree, so it is
//! never touched by [`wipe::CubeWiper`](super::wipe::CubeWiper).
//!
//! This is not a substitute for OS keychain storage; it raises the bar so a
//! casual disk read doesn't reveal the raw duress code, while keeping the code
//! recoverable across reboots without any Cube being unlocked.

use base64::Engine;
use chacha20poly1305::{aead::Aead, ChaCha20Poly1305, KeyInit, Nonce};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Filename of the device key, relative to the data-directory root.
pub const DEVICE_KEY_FILE: &str = "duress.key";

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;

/// A persisted 32-byte device key used to encrypt duress secrets at rest.
#[derive(Clone)]
pub struct DeviceKey {
    key: [u8; KEY_LEN],
}

impl DeviceKey {
    /// Loads the device key from `coincube_dir/duress.key` **without** creating
    /// one. Returns `Ok(None)` when the file is absent — the device never
    /// enrolled, or the key was removed/not-yet-restored. `Err` only on a
    /// present-but-unreadable or wrong-length file.
    ///
    /// This is the read-only accessor for the activation and drain paths, which
    /// must NEVER mint a fresh key: a new key can't decrypt a `duress_code`
    /// sealed under the original, so minting one would make the drainer drop the
    /// queued activation as "undecryptable" and clobber the slot, defeating
    /// recovery if the original key later returns. Key creation belongs to the
    /// enrollment/registration paths via [`load_or_create`](Self::load_or_create).
    pub fn load(coincube_dir: &Path) -> io::Result<Option<Self>> {
        let path = Self::path(coincube_dir);
        match std::fs::read(&path) {
            Ok(bytes) if bytes.len() == KEY_LEN => {
                let mut key = [0u8; KEY_LEN];
                key.copy_from_slice(&bytes);
                Ok(Some(Self { key }))
            }
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duress.key has unexpected length",
            )),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Loads the device key, generating and persisting a fresh CSPRNG key on
    /// first use. The key file is created with `0o600` permissions on Unix.
    ///
    /// Only the enrollment / device-registration paths may create a key — see
    /// [`load`](Self::load) for why activation and drain must not.
    pub fn load_or_create(coincube_dir: &Path) -> io::Result<Self> {
        if let Some(existing) = Self::load(coincube_dir)? {
            return Ok(existing);
        }
        let key: [u8; KEY_LEN] = rand::random();
        write_key(&Self::path(coincube_dir), &key)?;
        Ok(Self { key })
    }

    /// Constructs a key from raw bytes (test helper / explicit injection).
    pub fn from_bytes(key: [u8; KEY_LEN]) -> Self {
        Self { key }
    }

    fn path(coincube_dir: &Path) -> PathBuf {
        coincube_dir.join(DEVICE_KEY_FILE)
    }

    /// Encrypts a plaintext secret, returning `base64(nonce || ciphertext+tag)`.
    pub fn encrypt(&self, plaintext: &str) -> Result<String, String> {
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let nonce_bytes: [u8; NONCE_LEN] = rand::random();
        let nonce = Nonce::from(nonce_bytes);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| format!("duress encrypt: {e}"))?;
        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ciphertext);
        Ok(base64::engine::general_purpose::STANDARD.encode(blob))
    }

    /// Reverses [`encrypt`](Self::encrypt).
    pub fn decrypt(&self, envelope_b64: &str) -> Result<String, String> {
        let blob = base64::engine::general_purpose::STANDARD
            .decode(envelope_b64)
            .map_err(|e| format!("duress decrypt b64: {e}"))?;
        if blob.len() < NONCE_LEN + 16 {
            return Err("duress envelope too short".to_string());
        }
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let nonce = Nonce::from_slice(&blob[..NONCE_LEN]);
        let plaintext = cipher
            .decrypt(nonce, &blob[NONCE_LEN..])
            .map_err(|e| format!("duress decrypt: {e}"))?;
        String::from_utf8(plaintext).map_err(|e| format!("duress decrypt utf8: {e}"))
    }
}

fn write_key(path: &Path, key: &[u8; KEY_LEN]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(key)?;
    f.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let k = DeviceKey::from_bytes([7u8; KEY_LEN]);
        let ct = k.encrypt("deadbeefcafebabe").unwrap();
        assert_ne!(ct, "deadbeefcafebabe", "ciphertext must not be plaintext");
        assert_eq!(k.decrypt(&ct).unwrap(), "deadbeefcafebabe");
    }

    #[test]
    fn nonce_makes_ciphertext_unique() {
        let k = DeviceKey::from_bytes([9u8; KEY_LEN]);
        assert_ne!(k.encrypt("same").unwrap(), k.encrypt("same").unwrap());
    }

    #[test]
    fn wrong_key_fails() {
        let a = DeviceKey::from_bytes([1u8; KEY_LEN]);
        let b = DeviceKey::from_bytes([2u8; KEY_LEN]);
        let ct = a.encrypt("secret").unwrap();
        assert!(b.decrypt(&ct).is_err());
    }

    #[test]
    fn load_or_create_persists_and_reloads() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-key-{}-{:p}",
            std::process::id(),
            &0u8
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let k1 = DeviceKey::load_or_create(&dir).unwrap();
        let ct = k1.encrypt("x").unwrap();
        // Reload must read the same key and decrypt prior ciphertext.
        let k2 = DeviceKey::load_or_create(&dir).unwrap();
        assert_eq!(k2.decrypt(&ct).unwrap(), "x");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_returns_none_when_absent_and_never_creates() {
        let dir = std::env::temp_dir().join(format!(
            "coincube-duress-key-loadonly-{}-{:p}",
            std::process::id(),
            &0u8
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Absent → None, and the call must NOT mint a key file.
        assert!(DeviceKey::load(&dir).unwrap().is_none());
        assert!(
            !DeviceKey::path(&dir).exists(),
            "load() must not create duress.key"
        );
        // After a create, load() reads the same key (decrypts prior ciphertext).
        let created = DeviceKey::load_or_create(&dir).unwrap();
        let ct = created.encrypt("y").unwrap();
        let loaded = DeviceKey::load(&dir)
            .unwrap()
            .expect("present after create");
        assert_eq!(loaded.decrypt(&ct).unwrap(), "y");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
