//! Persisted list of phones this desktop has paired with.
//!
//! Stored as a single JSON file under [`CoincubeDirectory`] (alongside
//! the bitbox noise pairing config). One file holds N entries so we
//! can render a "Paired phones" table directly from disk.
//!
//! Skeleton status: types and load/save signatures are in place;
//! the file I/O paths are stubbed and marked TODO.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

use crate::dir::CoincubeDirectory;

/// On-disk record for a single paired phone. We persist the phone's
/// stable Ed25519 identity pubkey (captured during the pairing
/// handshake) plus enough metadata to render a "Paired phones" row
/// without doing any I/O.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairedPhone {
    /// Phone's long-lived Ed25519 identity pubkey, 32 raw bytes.
    /// Used for mutual auth on every subsequent reconnect — the
    /// ephemeral pairing PSK is dropped after the initial handshake.
    pub identity_pubkey: [u8; 32],

    /// User-facing name. Defaults to the `device_name` reported by
    /// the phone in `PairingComplete`; the settings panel lets the
    /// user rename it.
    pub name: String,

    /// When this pairing was finalised, as unix seconds. Drives the
    /// "Paired on …" row in the settings table. Stored as `u64`
    /// instead of `chrono::DateTime` so the on-disk record doesn't
    /// require pulling in the chrono `serde` feature.
    pub paired_at_unix: u64,

    /// Master fingerprints of the wallets this phone is allowed to
    /// sign for. Today this is always a single fingerprint (the one
    /// that authored the offer), but the list shape lets us extend to
    /// multi-wallet pairing without a migration.
    pub wallet_fingerprints: Vec<Fingerprint>,

    /// Optional manually-entered fallback target. Phase 3 surfaces a
    /// "Connect by IP" field for networks where mDNS is blocked.
    /// `host:port` form; `None` means rely on mDNS.
    pub fallback_addr: Option<String>,
}

/// Top-level on-disk layout. Wrapped in a struct so we can grow the
/// file (e.g. add a global "auto-accept new phones" flag) without
/// breaking the JSON shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PairingStoreFile {
    #[serde(default)]
    pub phones: Vec<PairedPhone>,
}

/// JSON file name under [`CoincubeDirectory`]. Top-level (not under
/// a per-network subdir) so the same paired phone can sign for any
/// network that wallet supports.
const STORE_FILENAME: &str = "paired-phones.json";

fn store_path(dir: &CoincubeDirectory) -> PathBuf {
    dir.path().join(STORE_FILENAME)
}

/// Load the paired-phones list. Returns an empty list if the file
/// doesn't exist yet (i.e. nothing has ever been paired).
pub fn load(dir: &CoincubeDirectory) -> std::io::Result<PairingStoreFile> {
    match std::fs::read(store_path(dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PairingStoreFile::default()),
        Err(e) => Err(e),
    }
}

/// Atomically replace the paired-phones list on disk.
pub fn save(dir: &CoincubeDirectory, file: &PairingStoreFile) -> std::io::Result<()> {
    let path = store_path(dir);
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

/// Append (or replace by pubkey) a paired-phone record and persist.
pub fn upsert(dir: &CoincubeDirectory, phone: PairedPhone) -> std::io::Result<PairingStoreFile> {
    let mut file = load(dir)?;
    if let Some(existing) = file
        .phones
        .iter_mut()
        .find(|p| p.identity_pubkey == phone.identity_pubkey)
    {
        *existing = phone;
    } else {
        file.phones.push(phone);
    }
    save(dir, &file)?;
    Ok(file)
}

/// Remove a paired phone by identity pubkey. No-op if not present.
pub fn remove(
    dir: &CoincubeDirectory,
    identity_pubkey: &[u8; 32],
) -> std::io::Result<PairingStoreFile> {
    let mut file = load(dir)?;
    file.phones
        .retain(|p| &p.identity_pubkey != identity_pubkey);
    save(dir, &file)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::bip32::Fingerprint;

    /// Allocate a fresh temp `CoincubeDirectory` per test. We don't pull
    /// `tempfile` into deps for one helper — `std::env::temp_dir()` plus
    /// a uuid subfolder is sufficient and aligns with the test's
    /// single-process scope.
    fn fresh_dir() -> CoincubeDirectory {
        let mut path = std::env::temp_dir();
        path.push(format!("coincube-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("mkdir tempdir");
        CoincubeDirectory::new(path)
    }

    fn sample_phone(seed: u8) -> PairedPhone {
        PairedPhone {
            identity_pubkey: [seed; 32],
            name: format!("Phone {}", seed),
            paired_at_unix: 1_700_000_000 + seed as u64,
            wallet_fingerprints: vec![Fingerprint::from([seed, seed, seed, seed])],
            fallback_addr: if seed % 2 == 0 {
                Some(format!("10.0.0.{}:8443", seed))
            } else {
                None
            },
        }
    }

    #[test]
    fn load_on_missing_file_returns_empty() {
        let dir = fresh_dir();
        let file = load(&dir).expect("load");
        assert!(file.phones.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = fresh_dir();
        let mut file = PairingStoreFile::default();
        file.phones.push(sample_phone(7));
        file.phones.push(sample_phone(8));
        save(&dir, &file).expect("save");
        let read = load(&dir).expect("load");
        assert_eq!(read.phones.len(), 2);
        assert_eq!(
            read.phones[0].identity_pubkey,
            file.phones[0].identity_pubkey
        );
        assert_eq!(read.phones[0].name, file.phones[0].name);
        assert_eq!(read.phones[1].fallback_addr, file.phones[1].fallback_addr);
        assert_eq!(
            read.phones[1].wallet_fingerprints,
            file.phones[1].wallet_fingerprints
        );
    }

    #[test]
    fn upsert_adds_new_entry() {
        let dir = fresh_dir();
        upsert(&dir, sample_phone(1)).expect("upsert add");
        upsert(&dir, sample_phone(2)).expect("upsert add 2");
        let read = load(&dir).expect("load");
        assert_eq!(read.phones.len(), 2);
    }

    #[test]
    fn upsert_replaces_existing_by_pubkey() {
        let dir = fresh_dir();
        upsert(&dir, sample_phone(1)).expect("first");
        let mut updated = sample_phone(1);
        updated.name = "Renamed".into();
        upsert(&dir, updated).expect("replace");
        let read = load(&dir).expect("load");
        assert_eq!(read.phones.len(), 1);
        assert_eq!(read.phones[0].name, "Renamed");
    }

    #[test]
    fn remove_deletes_present_entry() {
        let dir = fresh_dir();
        upsert(&dir, sample_phone(1)).expect("upsert");
        upsert(&dir, sample_phone(2)).expect("upsert 2");
        remove(&dir, &[1u8; 32]).expect("remove");
        let read = load(&dir).expect("load");
        assert_eq!(read.phones.len(), 1);
        assert_eq!(read.phones[0].identity_pubkey, [2u8; 32]);
    }

    #[test]
    fn remove_is_noop_when_absent() {
        let dir = fresh_dir();
        upsert(&dir, sample_phone(1)).expect("upsert");
        remove(&dir, &[42u8; 32]).expect("remove missing");
        let read = load(&dir).expect("load");
        assert_eq!(read.phones.len(), 1);
    }

    #[test]
    fn save_writes_pretty_json_no_tmp_left_behind() {
        // Atomic rename should leave only the final file on disk, no
        // .json.tmp sibling.
        let dir = fresh_dir();
        let mut file = PairingStoreFile::default();
        file.phones.push(sample_phone(3));
        save(&dir, &file).expect("save");
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("readdir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(entries.contains(&STORE_FILENAME.to_string()));
        assert!(
            !entries.iter().any(|n| n.ends_with(".tmp")),
            "tmp file leaked: {:?}",
            entries
        );
    }
}
