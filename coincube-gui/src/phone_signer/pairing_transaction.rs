//! Durable provisional rows. Pending entries expose the exact prior row, never
//! the proposed trust binding. A fresh explicit pairing recovers an abandoned
//! transaction; every mutation checks its ID so stale cleanup cannot touch it.
use super::pairing_store::{self, PairedPhone, PairingStoreFile};
use crate::dir::CoincubeDirectory;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, sync::Mutex};
pub(super) static WRITER: Mutex<()> = Mutex::new(());
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    id: String,
    previous: Option<PairedPhone>,
    candidate: PairedPhone,
    #[serde(default)]
    state: State,
    // Older journals exposed `finished` before the final acknowledgement.
    // Treat those records as uncertain, requiring explicit re-pair.
    #[serde(default, rename = "finished", skip_serializing)]
    legacy_finished: bool,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    #[default]
    Provisional,
    Decided,
    Completed,
}
impl Entry {
    fn state(&self) -> State {
        if self.legacy_finished {
            State::Decided
        } else {
            self.state
        }
    }
}
fn journal(dir: &CoincubeDirectory) -> io::Result<BTreeMap<String, Entry>> {
    match std::fs::read(dir.path().join("pairing-transactions.json")) {
        Ok(b) => {
            let mut entries: BTreeMap<String, Entry> =
                serde_json::from_slice(&b).map_err(io::Error::other)?;
            // Normalize before any whole-journal rewrite so another entry's
            // mutation cannot downgrade a legacy irreversible decision.
            for entry in entries.values_mut() {
                entry.state = entry.state();
                entry.legacy_finished = false;
            }
            Ok(entries)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e),
    }
}
fn save(dir: &CoincubeDirectory, entries: &BTreeMap<String, Entry>) -> io::Result<()> {
    pairing_store::write_durable(
        &dir.path().join("pairing-transactions.json"),
        &serde_json::to_vec(entries).map_err(io::Error::other)?,
    )
}
fn replace(file: &mut PairingStoreFile, pin: &[u8; 32], row: Option<PairedPhone>) {
    file.phones.retain(|p| &p.cert_pin != pin);
    if let Some(row) = row {
        file.phones.push(row);
    }
}
pub(super) fn visible(
    dir: &CoincubeDirectory,
    mut file: PairingStoreFile,
) -> io::Result<PairingStoreFile> {
    for entry in journal(dir)?
        .values()
        .filter(|e| e.state() != State::Completed)
    {
        replace(&mut file, &entry.candidate.cert_pin, entry.previous.clone());
    }
    Ok(file)
}
fn stale() -> io::Error {
    io::Error::other("Pairing transaction superseded; pair again")
}
pub struct PairingTransaction {
    dir: CoincubeDirectory,
    key: String,
    id: String,
}
impl PairingTransaction {
    pub fn prepare(
        dir: &CoincubeDirectory,
        id: String,
        mut candidate: PairedPhone,
    ) -> io::Result<Self> {
        let _guard = WRITER.lock().unwrap();
        let key = hex::encode(candidate.cert_pin);
        let visible = pairing_store::load_visible(dir)?;
        let previous = visible
            .phones
            .into_iter()
            .find(|p| p.cert_pin == candidate.cert_pin);
        if let Some(old) = &previous {
            candidate.name = old.name.clone();
            candidate.fallback_addr = old.fallback_addr.clone();
        }
        let mut entries = journal(dir)?;
        // Re-pair is explicit recovery: restore an abandoned provisional raw row.
        let mut raw = pairing_store::load_raw(dir)?;
        replace(&mut raw, &candidate.cert_pin, previous.clone());
        pairing_store::save(dir, &raw)?;
        entries.insert(
            key.clone(),
            Entry {
                id: id.clone(),
                previous,
                candidate,
                state: State::Provisional,
                legacy_finished: false,
            },
        );
        save(dir, &entries)?;
        Ok(Self {
            dir: dir.clone(),
            key,
            id,
        })
    }
    pub fn write_candidate(&self) -> io::Result<()> {
        let _guard = WRITER.lock().unwrap();
        let entries = journal(&self.dir)?;
        let entry = entries
            .get(&self.key)
            .filter(|e| e.id == self.id)
            .ok_or_else(stale)?;
        if entry.state() != State::Provisional {
            return Err(stale());
        }
        let mut raw = pairing_store::load_raw(&self.dir)?;
        replace(
            &mut raw,
            &entry.candidate.cert_pin,
            Some(entry.candidate.clone()),
        );
        pairing_store::save(&self.dir, &raw)
    }
    /// Called under PairingRun's cancellation lock, after COMMITTED.
    /// The durable decision remains hidden even if this task is dropped.
    pub fn decide(&self) -> io::Result<()> {
        self.transition(State::Provisional, State::Decided)
            .map(|_| ())
    }
    /// Only the validated FINISHED response authorizes public visibility.
    pub fn finish(&self) -> io::Result<PairedPhone> {
        self.transition(State::Decided, State::Completed)
    }
    fn transition(&self, from: State, to: State) -> io::Result<PairedPhone> {
        let _guard = WRITER.lock().unwrap();
        let mut entries = journal(&self.dir)?;
        let entry = entries
            .get_mut(&self.key)
            .filter(|e| e.id == self.id)
            .ok_or_else(stale)?;
        if entry.state() != from {
            return Err(io::Error::other(
                "Invalid pairing transaction transition; pair again",
            ));
        }
        entry.state = to;
        entry.legacy_finished = false;
        let candidate = entry.candidate.clone();
        save(&self.dir, &entries)?;
        Ok(candidate)
    }
    pub fn rollback(&self) -> io::Result<()> {
        let _guard = WRITER.lock().unwrap();
        let mut entries = journal(&self.dir)?;
        let Some(entry) = entries.get(&self.key).filter(|e| e.id == self.id) else {
            return Ok(());
        };
        // Durable ownership/state, not an in-memory retain flag, determines
        // rollback legality after task abort or an uncertain storage return.
        if entry.state() != State::Provisional {
            return Ok(());
        }
        let mut raw = pairing_store::load_raw(&self.dir)?;
        replace(&mut raw, &entry.candidate.cert_pin, entry.previous.clone());
        pairing_store::save(&self.dir, &raw)?;
        entries.remove(&self.key);
        save(&self.dir, &entries)
    }
}

impl Drop for PairingTransaction {
    fn drop(&mut self) {
        if let Err(error) = self.rollback() {
            tracing::error!(%error, "Pairing cleanup pending; pair again");
        }
    }
}
/// A user mutation supersedes provisional authority for this certificate.
/// First restore the visible row, then discard the journal; a crash between
/// these writes still exposes the previous row, never the candidate.
pub(super) fn revoke(dir: &CoincubeDirectory, pin: &[u8; 32]) -> io::Result<()> {
    let mut entries = journal(dir)?;
    if let Some(entry) = entries.remove(&hex::encode(pin)) {
        let mut raw = pairing_store::load_raw(dir)?;
        if entry.state() != State::Completed {
            replace(&mut raw, pin, entry.previous);
        }
        pairing_store::save(dir, &raw)?;
        save(dir, &entries)?;
    }
    Ok(())
}
