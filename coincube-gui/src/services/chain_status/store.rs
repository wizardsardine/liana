//! The Vault's ancestry check results, kept next to its daemon data. A coin
//! with no qualifying path is not searched again on every launch, and a
//! found path is kept so it can be written back while it waits to be
//! re-checked.
//!
//! Only display state lives here. Each verified path is decoded against the
//! outpoint it is filed under and structurally re-verified on load; a record
//! that fails either check is dropped. One that passes loads as
//! [`ProofEntry::Saved`], not verified: decoding proves the links fit, not
//! that the chain still holds them, so it waits for a fresh check. A failed
//! check is not kept, so a restart retries it; a path whose check failed is
//! kept, so a failure never erases it.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

use coincube_core::{claim_ancestry::retained::RetainedPath, miniscript::bitcoin::OutPoint};
use serde::{Deserialize, Serialize};

use super::ProofEntry;

const FILE: &str = "chain-proofs.json";
const VERSION: u32 = 1;
/// A Vault would need thousands of maximal paths to reach this.
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize, Default)]
struct Record {
    version: u32,
    #[serde(default)]
    verified: Vec<VerifiedRecord>,
    #[serde(default)]
    no_path: Vec<NoPathRecord>,
}

#[derive(Serialize, Deserialize)]
struct VerifiedRecord {
    outpoint: OutPoint,
    /// [`RetainedPath::encode`], hex.
    path: String,
}

#[derive(Serialize, Deserialize)]
struct NoPathRecord {
    outpoint: OutPoint,
    /// Unix seconds.
    checked_at: u64,
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

/// Load what a previous run recorded. A missing, oversized or unreadable
/// file is an empty store.
pub fn load(data_dir: &Path) -> HashMap<OutPoint, ProofEntry> {
    let file = path(data_dir);
    let Ok(metadata) = std::fs::metadata(&file) else {
        return HashMap::new();
    };
    if metadata.len() > MAX_BYTES {
        tracing::warn!("ignoring oversized {}", file.display());
        return HashMap::new();
    }
    let record: Record = match std::fs::read(&file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    {
        Some(record) => record,
        None => {
            tracing::warn!("ignoring unreadable {}", file.display());
            return HashMap::new();
        }
    };
    if record.version != VERSION {
        return HashMap::new();
    }
    let mut proofs = HashMap::new();
    for entry in record.no_path {
        proofs.insert(
            entry.outpoint,
            ProofEntry::NoQualifyingAncestry {
                checked_at: UNIX_EPOCH + Duration::from_secs(entry.checked_at),
            },
        );
    }
    for entry in record.verified {
        let decoded = hex::decode(&entry.path)
            .ok()
            .and_then(|bytes| RetainedPath::decode(entry.outpoint, &bytes).ok());
        match decoded {
            Some(path) => {
                proofs.insert(entry.outpoint, ProofEntry::Saved(path));
            }
            None => tracing::warn!("dropping an invalid ancestry record for {}", entry.outpoint),
        }
    }
    proofs
}

/// Replace the file with what `proofs` holds now, for the coins still in the
/// Vault (`keep`). Failed checks are not written.
pub fn save(
    data_dir: &Path,
    proofs: &HashMap<OutPoint, ProofEntry>,
    keep: impl Fn(&OutPoint) -> bool,
) -> io::Result<()> {
    let mut record = Record {
        version: VERSION,
        ..Default::default()
    };
    for (outpoint, entry) in proofs.iter().filter(|(outpoint, _)| keep(outpoint)) {
        match entry {
            ProofEntry::Verified(path)
            | ProofEntry::Saved(path)
            | ProofEntry::SavedRetry { path, .. } => record.verified.push(VerifiedRecord {
                outpoint: *outpoint,
                path: hex::encode(path.encode()),
            }),
            ProofEntry::NoQualifyingAncestry { checked_at } => record.no_path.push(NoPathRecord {
                outpoint: *outpoint,
                checked_at: checked_at
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |age| age.as_secs()),
            }),
            ProofEntry::Failed { .. } => {}
        }
    }
    record.verified.sort_by_key(|entry| entry.outpoint);
    record.no_path.sort_by_key(|entry| entry.outpoint);
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    write_atomically(&path(data_dir), &bytes)
}

/// Write through a sibling temp file, owner-only, and fsync the directory so
/// the rename survives a crash.
pub(crate) fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write;

    let tmp = path.with_extension("tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::{
        claim_ancestry::search::OwnedLink,
        miniscript::bitcoin::{
            absolute, consensus::serialize, script::Builder, transaction, Amount, ScriptBuf,
            Sequence, Transaction, TxIn, TxOut, Witness,
        },
    };
    use std::time::SystemTime;

    fn temp_dir() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("coincube-chain-proofs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    /// A coinbase and one spend of it: the spend's output 0 has a one-link
    /// path to the coinbase.
    fn verified_path() -> (OutPoint, RetainedPath) {
        let coinbase = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: Builder::new().push_int(962_000).push_int(1).into_script(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let spend = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(coinbase.compute_txid(), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(40_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        let selected = OutPoint::new(spend.compute_txid(), 0);
        let path = RetainedPath::new(
            selected,
            vec![
                OwnedLink {
                    transaction: serialize(&spend),
                    parent_input: Some(0),
                },
                OwnedLink {
                    transaction: serialize(&coinbase),
                    parent_input: None,
                },
            ],
        )
        .unwrap();
        (selected, path)
    }

    #[test]
    fn round_trips_verified_and_no_path_entries() {
        let dir = temp_dir();
        let (selected, retained) = verified_path();
        let other = OutPoint::new(selected.txid, 7);
        let checked_at = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let mut proofs = HashMap::new();
        proofs.insert(selected, ProofEntry::Verified(retained));
        proofs.insert(other, ProofEntry::NoQualifyingAncestry { checked_at });
        proofs.insert(
            OutPoint::new(selected.txid, 9),
            ProofEntry::Failed {
                failed_at: SystemTime::now(),
            },
        );
        save(&dir, &proofs, |_| true).unwrap();
        let loaded = load(&dir);
        assert_eq!(loaded.len(), 2, "failed checks are not kept");
        let saved = loaded.get(&selected).expect("the path is kept");
        assert!(
            matches!(saved, ProofEntry::Saved(p) if p.selected() == selected),
            "a loaded path is never verified until a fresh check"
        );
        assert_eq!(saved.outcome(), None, "shown as checking");
        assert!(matches!(
            loaded.get(&other),
            Some(ProofEntry::NoQualifyingAncestry { checked_at: at }) if *at == checked_at
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(&dir)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_saved_path_is_checked_again_and_saved_again() {
        let dir = temp_dir();
        let (selected, retained) = verified_path();
        let mut proofs = HashMap::new();
        proofs.insert(selected, ProofEntry::Saved(retained));
        let candidates = crate::services::chain_status::proof_candidates(
            vec![selected],
            |_| crate::services::entangled::Entanglement::NotEntangled,
            &proofs,
            &std::collections::HashSet::new(),
            SystemTime::now(),
        );
        assert_eq!(candidates, vec![selected], "due for a fresh check at once");
        save(&dir, &proofs, |_| true).unwrap();
        assert!(matches!(
            load(&dir).get(&selected),
            Some(ProofEntry::Saved(_))
        ));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A saved path's fresh check fails (Connect answered 503): the path
    /// survives in memory and on disk, and waits the usual failed-check
    /// interval before the next attempt instead of being erased.
    #[test]
    fn a_failed_check_never_erases_a_saved_path() {
        use crate::services::chain_status::{proof_candidates, FAILED_RETRY};
        let dir = temp_dir();
        let (selected, retained) = verified_path();
        let failed_at = SystemTime::now();
        let entry = ProofEntry::after_check(
            Some(&ProofEntry::Saved(retained)),
            ProofEntry::Failed { failed_at },
        );
        assert!(
            matches!(&entry, ProofEntry::SavedRetry { path, .. } if path.selected() == selected),
            "the path is kept"
        );
        assert_eq!(entry.outcome(), None, "still shown as checking");

        let proofs: HashMap<_, _> = [(selected, entry)].into();
        let due = |now| {
            proof_candidates(
                vec![selected],
                |_| crate::services::entangled::Entanglement::NotEntangled,
                &proofs,
                &std::collections::HashSet::new(),
                now,
            )
        };
        assert!(due(failed_at).is_empty(), "not retried at once");
        assert_eq!(due(failed_at + FAILED_RETRY), vec![selected]);

        save(&dir, &proofs, |_| true).unwrap();
        assert!(
            matches!(load(&dir).get(&selected), Some(ProofEntry::Saved(p)) if p.selected() == selected),
            "written back, and a restart re-checks it"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A finished answer still replaces a saved path, and a failure with no
    /// path held stays a plain failure.
    #[test]
    fn only_an_unfinished_check_keeps_the_held_path() {
        let (_, retained) = verified_path();
        let saved = ProofEntry::Saved(retained);
        let no_path = ProofEntry::NoQualifyingAncestry {
            checked_at: SystemTime::now(),
        };
        assert!(matches!(
            ProofEntry::after_check(Some(&saved), no_path),
            ProofEntry::NoQualifyingAncestry { .. }
        ));
        let failed = ProofEntry::Failed {
            failed_at: SystemTime::now(),
        };
        assert!(matches!(
            ProofEntry::after_check(None, failed.clone()),
            ProofEntry::Failed { .. }
        ));
        assert!(matches!(
            ProofEntry::after_check(
                Some(&ProofEntry::NoQualifyingAncestry {
                    checked_at: SystemTime::now()
                }),
                failed
            ),
            ProofEntry::Failed { .. }
        ));
    }

    #[test]
    fn a_path_filed_under_another_outpoint_is_dropped() {
        let dir = temp_dir();
        let (selected, retained) = verified_path();
        let wrong = OutPoint::new(selected.txid, 1);
        let record = Record {
            version: VERSION,
            verified: vec![VerifiedRecord {
                outpoint: wrong,
                path: hex::encode(retained.encode()),
            }],
            no_path: vec![],
        };
        std::fs::write(path(&dir), serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(load(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn spent_coins_are_not_written_and_junk_is_empty() {
        let dir = temp_dir();
        let (selected, retained) = verified_path();
        let mut proofs = HashMap::new();
        proofs.insert(selected, ProofEntry::Verified(retained));
        save(&dir, &proofs, |_| false).unwrap();
        assert!(load(&dir).is_empty());
        std::fs::write(path(&dir), b"not json").unwrap();
        assert!(load(&dir).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
