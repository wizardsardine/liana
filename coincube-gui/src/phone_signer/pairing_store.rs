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
    /// Phone's TLS cert pin: `SHA-256(self-signed cert DER)`, 32
    /// raw bytes. Captured from the live TLS handshake via
    /// [`crate::phone_signer::transport::PairedTransport::peer_cert_fingerprint`]
    /// at pairing time and used to verify the phone's cert on every
    /// subsequent reconnect (matched by
    /// [`crate::phone_signer::tls::PinnedVerifier`]).
    ///
    /// **Not an Ed25519 pubkey** despite the on-disk JSON field name
    /// (kept as `"identity_pubkey"` for backward compat with v1.0
    /// stores via `#[serde(rename)]`). Attempting Ed25519 signature
    /// verification against these bytes would silently fail — they
    /// are a SHA-256 digest, not a curve point.
    #[serde(rename = "identity_pubkey")]
    pub cert_pin: [u8; 32],

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
    let bytes = serde_json::to_vec_pretty(file).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

/// Append (or replace by cert pin) a paired-phone record and persist.
pub fn upsert(dir: &CoincubeDirectory, phone: PairedPhone) -> std::io::Result<PairingStoreFile> {
    let mut file = load(dir)?;
    if let Some(existing) = file
        .phones
        .iter_mut()
        .find(|p| p.cert_pin == phone.cert_pin)
    {
        *existing = phone;
    } else {
        file.phones.push(phone);
    }
    save(dir, &file)?;
    Ok(file)
}

/// Remove a paired phone by cert pin. No-op if not present.
pub fn remove(dir: &CoincubeDirectory, cert_pin: &[u8; 32]) -> std::io::Result<PairingStoreFile> {
    let mut file = load(dir)?;
    file.phones.retain(|p| &p.cert_pin != cert_pin);
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
            cert_pin: [seed; 32],
            name: format!("Phone {}", seed),
            paired_at_unix: 1_700_000_000 + seed as u64,
            wallet_fingerprints: vec![Fingerprint::from([seed, seed, seed, seed])],
            fallback_addr: if seed.is_multiple_of(2) {
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
        assert_eq!(read.phones[0].cert_pin, file.phones[0].cert_pin);
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
    fn upsert_replaces_existing_by_cert_pin() {
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
        assert_eq!(read.phones[0].cert_pin, [2u8; 32]);
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

    /// Regression: the in-memory field is `cert_pin` (it's a cert
    /// SHA-256, not an Ed25519 pubkey), but for backward compat
    /// with v1.0 stores the on-disk JSON key MUST stay
    /// `"identity_pubkey"`. A future serde refactor that dropped
    /// the `#[serde(rename)]` would silently invalidate every
    /// installed user's pairing store.
    #[test]
    fn on_disk_json_field_name_stays_identity_pubkey() {
        let phone = sample_phone(0xab);
        let json = serde_json::to_string(&phone).expect("serialize");
        assert!(
            json.contains("\"identity_pubkey\""),
            "on-disk JSON must keep the v1.0 field name; got {}",
            json,
        );
        assert!(
            !json.contains("\"cert_pin\""),
            "on-disk JSON must NOT leak the renamed Rust identifier; got {}",
            json,
        );
        // And round-trips: a v1.0 file using `identity_pubkey` must
        // still deserialize.
        let legacy = r#"{
            "identity_pubkey": [1,2,3,4,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
            "name": "Legacy",
            "paired_at_unix": 1700000000,
            "wallet_fingerprints": [],
            "fallback_addr": null
        }"#;
        let decoded: PairedPhone = serde_json::from_str(legacy).expect("decode legacy");
        assert_eq!(decoded.cert_pin[..4], [1, 2, 3, 4]);
        assert_eq!(decoded.name, "Legacy");
    }
}
