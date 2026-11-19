//! Serialised, atomic updates to the managed nodes' `bitcoin.conf` files.
//!
//! Two things can go wrong when the managed `bitcoin.conf` is edited without
//! coordination, and both are about *other writers of the same bytes*:
//!
//! 1. **Port allocation.** A new network section's RPC/P2P ports are chosen by
//!    reading every port already recorded — this file's other sections and the
//!    other chain family's file — and steering around them. Two allocations
//!    running at the same moment can both pass that check before either
//!    persists, and hand out the same port twice.
//! 2. **Read-modify-write.** Every writer of the file rebuilds it from a struct
//!    it read moments earlier. A writer that read before another one persisted
//!    a new section erases that section when it writes ("last writer wins").
//!    The inbound-Tor preparation and `bitcoind::maybe_start`'s pre-spawn
//!    rewrite do exactly this on every start.
//!
//! Both are closed by one datadir-wide, OS-backed exclusive lock
//! ([`ManagedConfLock`]) held across the whole read → decide → write span
//! ([`update_managed_conf`]), and by replacing the file atomically
//! ([`write_conf_atomically`]) so a reader never sees a torn file.
//!
//! What the lock is **not**: it does not reserve ports against processes that
//! do not take it — a third-party program, or an older COINCUBE binary still
//! running against the same datadir — and it does not make the conf and the
//! flavour ledger one crash-atomic transaction (they are two files; the ledger
//! is recorded first, as before). Bind-and-release port probing remains the
//! candidate source; see `docs/BTCB2_MANAGED_NODE.md`.
//!
//! Lock ordering: this is a leaf lock. The history controller may acquire it
//! after its per-network lease when changing pruning mode; the reverse order
//! is prohibited. Other writers acquire it alone. The node-identity marker lock in `bitcoind::ensure_node_instance_marker`
//! is taken *after* it is released, never inside, and nothing blocking — no
//! Tor bootstrap, no node spawn, no RPC — runs while it is held.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use tracing::{info, warn};

use crate::dir::CoincubeDirectory;
use crate::node::bitcoind::{
    allocate_managed_ports, internal_bitcoind_config_path, internal_bitcoind_datadir_for,
    reserved_managed_ports, InternalBitcoindConfig, InternalBitcoindConfigError, NodeChainFamily,
    PortAllocationError,
};

// ---------------------------------------------------------------------------
// The lock
// ---------------------------------------------------------------------------

/// Name of the lock file, directly under the COINCUBE datadir — *outside* both
/// managed-node roots, because it guards the port space and the conf files of
/// every chain family and every network at once.
pub const MANAGED_CONF_LOCK_FILE: &str = "managed-node.lock";

/// Path of the datadir-wide managed-node configuration lock.
pub fn managed_conf_lock_path(coincube_datadir: &CoincubeDirectory) -> PathBuf {
    coincube_datadir.path().join(MANAGED_CONF_LOCK_FILE)
}

/// Retry policy for the lock, as (maximum attempts, delay between them).
///
/// Production makes at most 40 attempts, with 50 ms sleeps between failed
/// attempts and no sleep after the last one. This is an attempt bound, not a
/// wall-clock deadline: exhausting it costs all lock calls plus 39 actual
/// sleeps (1.95 s of requested sleep). Scheduling and lock-call costs make the
/// elapsed time platform- and load-dependent; it may exceed two seconds.
/// The test default of 500 attempts and 10 ms sleeps is likewise not a deadline.
///
/// The same production bound as the node-identity marker lock: real holders
/// finish in microseconds (a read, an allocation, a rename), so contention is
/// rare and brief, but a wedged holder must not wedge every start behind it.
///
/// Under test the default is *generous* (a loaded CI runner can take well
/// over a second to schedule a thread and flush a file), so a test whose
/// contenders are meant to succeed rarely fails on wall-clock luck — margin,
/// not immunity; a test that wants the `Busy` path sets a short bound for its
/// own thread with [`with_quick_lock_bound`] instead of waiting the default
/// out. The marker lock has the same bound shape with its own, separate
/// override (`bitcoind::with_quick_marker_lock_bound`), scoped by a guard that
/// restores the prior value on return or unwind. The helpers below do the
/// same; neither override reaches the other lock or a spawned thread.
fn lock_acquisition_bound() -> (u32, std::time::Duration) {
    #[cfg(not(test))]
    {
        (40, std::time::Duration::from_millis(50))
    }
    #[cfg(test)]
    {
        lock_bound_override().unwrap_or((500, std::time::Duration::from_millis(10)))
    }
}

#[cfg(test)]
thread_local! {
    /// A per-thread override of the default test bound (see
    /// [`lock_acquisition_bound`]). Thread-local on purpose: a contender
    /// spawned by a test keeps the generous default while the test's own
    /// thread can ask for the `Busy` path quickly.
    static LOCK_BOUND_OVERRIDE: std::cell::Cell<Option<(u32, std::time::Duration)>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn lock_bound_override() -> Option<(u32, std::time::Duration)> {
    LOCK_BOUND_OVERRIDE.with(|b| b.get())
}

#[cfg(test)]
struct LockBoundGuard(Option<(u32, std::time::Duration)>);

#[cfg(test)]
impl LockBoundGuard {
    fn quick() -> Self {
        Self(
            LOCK_BOUND_OVERRIDE
                .with(|bound| bound.replace(Some((3, std::time::Duration::from_millis(5))))),
        )
    }
}

#[cfg(test)]
impl Drop for LockBoundGuard {
    fn drop(&mut self) {
        LOCK_BOUND_OVERRIDE.with(|bound| bound.set(self.0));
    }
}

/// Run `body` with this thread's lock acquisition bound set short (3 × 5 ms),
/// for tests that want to observe `Busy` through the public paths
/// (`update_managed_conf`, `prepare_inbound_tor`, the loader …) without
/// waiting the generous default out. Test-only.
#[cfg(test)]
pub(crate) fn with_quick_lock_bound<T>(body: impl FnOnce() -> T) -> T {
    let _restore = LockBoundGuard::quick();
    body()
}

/// [`with_quick_lock_bound`] for an async body. The override is thread-local,
/// so this is only meaningful on a current-thread runtime (the `#[tokio::test]`
/// default), where the future is polled on the calling thread.
#[cfg(test)]
pub(crate) async fn with_quick_lock_bound_async<F: std::future::Future>(body: F) -> F::Output {
    let _restore = LockBoundGuard::quick();
    body.await
}

/// Why the lock could not be taken.
#[derive(Debug)]
pub enum ManagedConfLockError {
    /// Another holder — a thread of this process or another process — kept the
    /// lock through every acquisition attempt. Nothing was read or written; retry.
    Busy { path: PathBuf },
    /// The lock file could not be created or locked.
    Io { path: PathBuf, error: io::Error },
}

impl fmt::Display for ManagedConfLockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy { path } => write!(
                f,
                "another setup is updating the managed node configuration ({}); try again",
                path.display()
            ),
            Self::Io { path, error } => write!(
                f,
                "could not lock the managed node configuration at {}: {}",
                path.display(),
                error
            ),
        }
    }
}

impl std::error::Error for ManagedConfLockError {}

/// An exclusive hold on the managed-node configuration of one COINCUBE datadir.
///
/// OS-backed (`fs4`, i.e. `flock`/`LockFileEx`), so it excludes other
/// processes as well as other threads, and it is released by the OS when the
/// holder's descriptor closes — including when the process dies — so there is
/// no stale lock to break and no timeout to guess. Released explicitly on
/// [`Drop`], which also runs on a panic. The lock file itself is left in place
/// forever: it carries no state, and its existence is not what locks anything.
///
/// Note on cancellation: dropping an `iced` task or a `JoinHandle` does not
/// abort a `spawn_blocking` worker that is holding this. The lock is released
/// when that worker actually returns or unwinds, not when the caller stops
/// waiting for it.
#[derive(Debug)]
pub struct ManagedConfLock {
    file: std::fs::File,
    path: PathBuf,
}

impl ManagedConfLock {
    /// Take the lock using the attempt-limited retry policy; elapsed time is not bounded.
    pub fn acquire(coincube_datadir: &CoincubeDirectory) -> Result<Self, ManagedConfLockError> {
        let (attempts, retry) = lock_acquisition_bound();
        Self::acquire_with_bound(coincube_datadir, attempts, retry)
    }

    /// [`Self::acquire`] with an explicit attempt count and between-attempt delay.
    /// Tests use this to exercise contention without the production retry count.
    pub fn acquire_with_bound(
        coincube_datadir: &CoincubeDirectory,
        attempts: u32,
        retry: std::time::Duration,
    ) -> Result<Self, ManagedConfLockError> {
        use fs4::fs_std::FileExt;

        let path = managed_conf_lock_path(coincube_datadir);
        let io = |error: io::Error| ManagedConfLockError::Io {
            path: path.clone(),
            error,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(io)?;
        for attempt in 0..attempts {
            if file.try_lock_exclusive().map_err(io)? {
                return Ok(Self { file, path });
            }
            if attempt + 1 < attempts {
                std::thread::sleep(retry);
            }
        }
        Err(ManagedConfLockError::Busy { path })
    }

    /// Where the lock lives.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ManagedConfLock {
    fn drop(&mut self) {
        use fs4::fs_std::FileExt;
        // Explicit rather than left to the descriptor closing, so the release
        // does not depend on the order fields happen to drop in.
        let _ = FileExt::unlock(&self.file);
    }
}

// ---------------------------------------------------------------------------
// Atomic replacement of a conf file
// ---------------------------------------------------------------------------

/// Steps of [`write_conf_atomically`] a test can make fail, since none of them
/// can be provoked from outside. Compiled out of production builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfWriteStep {
    /// Writing the staged bytes.
    StageWrite,
    /// Flushing the staged file.
    StageSync,
    /// Renaming the staged file over the destination.
    Rename,
    /// Flushing the parent directory after the rename (unix).
    DirSync,
}

/// A test hook: called at each [`ConfWriteStep`] with the staging path.
#[cfg(test)]
type ConfWriteHook = Box<dyn Fn(ConfWriteStep, &Path) -> io::Result<()>>;

#[cfg(test)]
thread_local! {
    /// A hook rather than a set of armed points: the interesting observations
    /// are made *mid-write* (what mode the staging file has while it exists),
    /// which needs the staging path in hand.
    static CONF_WRITE_HOOK: std::cell::RefCell<Option<ConfWriteHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Run the test hook for `step` with the staging path, if one is installed.
#[inline]
fn hook(step: ConfWriteStep, staging: &Path) -> io::Result<()> {
    #[cfg(test)]
    {
        CONF_WRITE_HOOK.with(|h| match h.borrow().as_ref() {
            Some(hook) => hook(step, staging),
            None => Ok(()),
        })
    }
    #[cfg(not(test))]
    {
        let _ = (step, staging);
        Ok(())
    }
}

/// Why, and how far, an atomic conf replacement failed.
///
/// The distinction is the point: a failure *before* the rename leaves the
/// destination holding exactly the bytes it held; a failure *after* it leaves
/// the destination holding the complete new bytes, with only the directory
/// entry's durability across a crash in doubt. A caller that reads every error
/// as "nothing was written" would act on a false premise in the second case.
#[derive(Debug)]
pub enum ConfWriteError {
    /// The destination was not touched.
    NotReplaced(io::Error),
    /// The destination now holds the complete new bytes, but the directory
    /// entry naming them was not confirmed durable.
    ReplacedNotDurable(io::Error),
}

impl fmt::Display for ConfWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotReplaced(e) => write!(f, "the file was not replaced: {}", e),
            Self::ReplacedNotDurable(e) => write!(
                f,
                "the file was replaced but its directory entry was not confirmed durable: {}",
                e
            ),
        }
    }
}

impl std::error::Error for ConfWriteError {}

/// A unique staging name beside `path`: `<name>.<pid>.<seq>.tmp`.
///
/// Unique rather than a fixed sibling, so two writers of the same file — which
/// the lock prevents for conf writers of this process, but not for a stray
/// older binary — can never truncate each other's staging file. Uniqueness is
/// not assumed, only attempted: the name is created with `create_new`, and a
/// collision (a leftover from a dead process with the same pid, say) moves on
/// to the next sequence number and never touches the file that is there.
fn staging_path(path: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "conf".to_string());
    path.with_file_name(format!(
        "{}.{}.{}.tmp",
        name,
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ))
}

/// How many staging names to try before giving up on a directory full of
/// colliding leftovers. Each attempt is a distinct name; none is ever removed.
const STAGING_NAME_ATTEMPTS: usize = 16;

/// A staging file this operation created (`create_new` succeeded), and so may
/// remove. Nothing else in the directory is ever removed: a name that already
/// existed belongs to someone else — a previous writer of this process, a
/// stray older binary — whatever it is called.
struct OwnedStaging {
    path: PathBuf,
    file: std::fs::File,
}

/// Create a fresh, private staging file from the names `next_name` yields,
/// trying the next one on each `AlreadyExists` and leaving any colliding file
/// untouched. Production yields [`staging_path`] names; tests inject
/// collisions.
fn create_owned_staging(
    path: &Path,
    mut next_name: impl FnMut() -> PathBuf,
) -> io::Result<OwnedStaging> {
    for _ in 0..STAGING_NAME_ATTEMPTS {
        let candidate = next_name();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => {
                return Ok(OwnedStaging {
                    path: candidate,
                    file,
                })
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "could not find a free staging name beside {} after {} attempts",
            path.display(),
            STAGING_NAME_ATTEMPTS
        ),
    ))
}

/// Replace `path` with `contents` so a reader sees either the old file or the
/// complete new one, never a torn one.
///
/// The bytes are staged in a private sibling file (mode `0600` on unix: the
/// conf can carry `rpcauth` material), flushed, given the destination's
/// existing permissions if there is one — so a user's restrictive mode is
/// preserved and a fresh file starts private — and renamed into place. On unix
/// the parent directory is flushed afterwards so the rename itself survives a
/// crash; Windows has no equivalent and NTFS journals the rename.
///
/// On a pre-rename failure the staging file *this call created* is removed
/// and the destination is untouched ([`ConfWriteError::NotReplaced`]); a
/// staging name that already existed is never removed — the write moves to
/// another name instead. On a post-rename failure the destination already
/// holds the new bytes ([`ConfWriteError::ReplacedNotDurable`]).
pub fn write_conf_atomically(path: &Path, contents: &[u8]) -> Result<(), ConfWriteError> {
    write_conf_atomically_with(path, contents, || staging_path(path))
}

/// [`write_conf_atomically`] with the staging-name source injected, so a
/// collision with a file that is already there can be produced on demand.
fn write_conf_atomically_with(
    path: &Path,
    contents: &[u8],
    next_name: impl FnMut() -> PathBuf,
) -> Result<(), ConfWriteError> {
    use std::io::Write;

    let parent = path.parent().ok_or_else(|| {
        ConfWriteError::NotReplaced(io::Error::other("the conf path has no parent directory"))
    })?;
    std::fs::create_dir_all(parent).map_err(ConfWriteError::NotReplaced)?;
    // The mode to restore, read before anything is staged: `None` for a new file.
    let existing_permissions = std::fs::metadata(path).ok().map(|m| m.permissions());
    // Ownership is established here and only here: a failure to create means
    // there is nothing of ours to clean up.
    let OwnedStaging {
        path: staging,
        mut file,
    } = create_owned_staging(path, next_name).map_err(ConfWriteError::NotReplaced)?;

    let staged = (|| -> io::Result<()> {
        hook(ConfWriteStep::StageWrite, &staging)?;
        file.write_all(contents)?;
        hook(ConfWriteStep::StageSync, &staging)?;
        file.sync_all()?;
        if let Some(permissions) = existing_permissions {
            std::fs::set_permissions(&staging, permissions)?;
        }
        Ok(())
    })();
    // Past creation, every failure path removes the one file this call owns.
    if let Err(e) = staged {
        drop(file);
        let _ = std::fs::remove_file(&staging);
        return Err(ConfWriteError::NotReplaced(e));
    }
    drop(file);
    if let Err(e) =
        hook(ConfWriteStep::Rename, &staging).and_then(|()| std::fs::rename(&staging, path))
    {
        let _ = std::fs::remove_file(&staging);
        return Err(ConfWriteError::NotReplaced(e));
    }

    // Past this line the destination has already changed.
    let synced = (|| -> io::Result<()> {
        hook(ConfWriteStep::DirSync, &staging)?;
        #[cfg(unix)]
        {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    synced.map_err(ConfWriteError::ReplacedNotDurable)
}

// ---------------------------------------------------------------------------
// Locked read → edit → persist
// ---------------------------------------------------------------------------

/// Why a locked conf update did not complete.
#[derive(Debug)]
pub enum ManagedConfError {
    /// The lock could not be taken; nothing was read or written.
    Lock(ManagedConfLockError),
    /// This family's conf exists but could not be read; nothing was written.
    Unreadable(InternalBitcoindConfigError),
    /// The edit itself refused (a port could not be allocated, the other
    /// family's conf was unreadable, or the caller's own check failed);
    /// nothing was written.
    Edit(ManagedConfEditError),
    /// The conf could not be replaced; it still holds its previous bytes.
    /// The edit has already run, so anything it recorded on the way (the
    /// flavour ledger, for the installer and settings writers) stays.
    NotReplaced(io::Error),
}

impl fmt::Display for ManagedConfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lock(e) => write!(f, "{}", e),
            Self::Unreadable(e) => {
                write!(f, "could not read the managed node configuration: {}", e)
            }
            Self::Edit(e) => write!(f, "{}", e),
            Self::NotReplaced(e) => {
                write!(f, "could not write the managed node configuration: {}", e)
            }
        }
    }
}

impl std::error::Error for ManagedConfError {}

/// Refusals an edit closure can raise.
#[derive(Debug)]
pub enum ManagedConfEditError {
    Ports(PortAllocationError),
    Other(String),
}

impl fmt::Display for ManagedConfEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ports(e) => write!(f, "{}", e),
            Self::Other(e) => write!(f, "{}", e),
        }
    }
}

impl From<PortAllocationError> for ManagedConfEditError {
    fn from(e: PortAllocationError) -> Self {
        Self::Ports(e)
    }
}

impl From<String> for ManagedConfEditError {
    fn from(e: String) -> Self {
        Self::Other(e)
    }
}

/// Whether the persisted conf was confirmed durable.
#[derive(Debug)]
pub enum ConfDurability {
    /// Written, renamed, and the directory entry flushed.
    Durable,
    /// Written and renamed — the destination holds the complete new bytes —
    /// but the directory entry's durability across a crash was not confirmed.
    /// Callers proceed (the bytes are there) and log the reason.
    ReplacedNotDurable(io::Error),
    /// The edit asked for no write.
    Unchanged,
}

/// The outcome of a completed [`update_managed_conf`].
#[derive(Debug)]
pub struct ManagedConfOutcome<T> {
    pub value: T,
    pub durability: ConfDurability,
}

impl<T> ManagedConfOutcome<T> {
    /// Log a non-durable replacement and hand back the value: what every
    /// caller that has nothing better to do with the distinction does.
    pub fn logged(self, what: &str) -> T {
        if let ConfDurability::ReplacedNotDurable(e) = &self.durability {
            warn!(
                "{}: the managed node configuration was replaced but its directory entry \
                 was not confirmed durable ({}); the new contents are in place",
                what, e
            );
        }
        self.value
    }
}

/// What an edit closure sees: the fresh conf (read under the lock) and a way
/// to pick ports for a new section against everything currently recorded.
pub struct ManagedConfTxn<'a> {
    coincube_datadir: &'a CoincubeDirectory,
    family: NodeChainFamily,
    /// This family's conf as it is on disk right now, or `None` if there is
    /// none yet. An unreadable file never reaches the edit.
    pub conf: Option<InternalBitcoindConfig>,
    /// Where it lives.
    pub path: PathBuf,
}

impl<'a> ManagedConfTxn<'a> {
    /// RPC and P2P ports for a new network section of `conf` (the conf the edit
    /// is about to persist, so its other sections are reserved too), drawing
    /// candidates from `next_candidate` and steering around every port any
    /// managed-node conf under this datadir records — the other family's
    /// sections and Tor ports included. Refuses if the other family's conf
    /// exists but cannot be read.
    pub fn allocate_ports<E: fmt::Display>(
        &self,
        conf: &InternalBitcoindConfig,
        next_candidate: impl FnMut() -> Result<u16, E>,
    ) -> Result<(u16, u16), PortAllocationError> {
        let reserved = reserved_managed_ports(self.coincube_datadir, self.family, conf)?;
        allocate_managed_ports(&reserved, next_candidate)
    }
}

/// Take the datadir-wide lock, read `family`'s conf fresh, let `edit` decide
/// what to persist, and replace the file atomically — all under the one lock.
///
/// `edit` returns the value to hand back and the conf to persist (`None` to
/// persist nothing). Anything durable the edit needs recorded *before* the
/// conf changes — the flavour ledger, whose entry must exist before a legacy
/// marker is erased — is the edit's to write, inside the closure, in that
/// order; the two files are not one transaction. So: an early refusal
/// ([`ManagedConfError::Lock`], [`ManagedConfError::Unreadable`], or the
/// edit's own [`ManagedConfError::Edit`]) leaves both files untouched, while a
/// write failure ([`ManagedConfError::NotReplaced`]) leaves the conf
/// byte-identical but keeps whatever the edit already recorded.
///
/// Nothing slow belongs in `edit`: no Tor bootstrap, no node spawn, no RPC.
/// Callers that need those do them *outside*, then run a second short update
/// that re-reads and merges only the fields they own (see
/// `tor::prepare_inbound_tor`).
pub fn update_managed_conf<T, E>(
    coincube_datadir: &CoincubeDirectory,
    family: NodeChainFamily,
    edit: E,
) -> Result<ManagedConfOutcome<T>, ManagedConfError>
where
    E: FnOnce(
        &ManagedConfTxn<'_>,
    ) -> Result<(T, Option<InternalBitcoindConfig>), ManagedConfEditError>,
{
    let _lock = ManagedConfLock::acquire(coincube_datadir).map_err(ManagedConfError::Lock)?;
    let path =
        internal_bitcoind_config_path(&internal_bitcoind_datadir_for(coincube_datadir, family));
    let conf = match InternalBitcoindConfig::from_file(&path) {
        Ok(conf) => Some(conf),
        Err(InternalBitcoindConfigError::FileNotFound) => None,
        Err(e) => return Err(ManagedConfError::Unreadable(e)),
    };
    let txn = ManagedConfTxn {
        coincube_datadir,
        family,
        conf,
        path: path.clone(),
    };
    let (value, to_persist) = edit(&txn).map_err(ManagedConfError::Edit)?;
    let durability = match to_persist {
        None => ConfDurability::Unchanged,
        Some(conf) => {
            info!("Writing to file {}", path.display());
            let mut bytes = Vec::new();
            conf.to_ini()
                .write_to(&mut bytes)
                .map_err(|e| ManagedConfError::NotReplaced(io::Error::other(e.to_string())))?;
            match write_conf_atomically(&path, &bytes) {
                Ok(()) => ConfDurability::Durable,
                Err(ConfWriteError::ReplacedNotDurable(e)) => ConfDurability::ReplacedNotDurable(e),
                Err(ConfWriteError::NotReplaced(e)) => {
                    return Err(ManagedConfError::NotReplaced(e))
                }
            }
        }
    };
    Ok(ManagedConfOutcome { value, durability })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::bitcoind::{InternalBitcoindNetworkConfig, NodeFlavor, PRUNE_DEFAULT};
    use coincube_core::miniscript::bitcoin::Network;
    use std::sync::{Arc, Barrier};

    #[test]
    fn quick_lock_bound_restores_nested_and_panicking_calls() {
        let original = lock_bound_override();
        with_quick_lock_bound(|| {
            let outer = lock_bound_override();
            with_quick_lock_bound(|| assert_eq!(lock_bound_override(), outer));
            assert_eq!(lock_bound_override(), outer);
            assert!(std::panic::catch_unwind(|| {
                with_quick_lock_bound(|| panic!("synthetic panic"));
            })
            .is_err());
            assert_eq!(lock_bound_override(), outer);
        });
        assert_eq!(lock_bound_override(), original);
    }

    #[tokio::test]
    async fn quick_lock_bound_restores_async_unwind_and_cancellation() {
        use iced_runtime::futures::futures::FutureExt;
        let original = lock_bound_override();
        with_quick_lock_bound_async(async {
            let outer = lock_bound_override();
            with_quick_lock_bound_async(async {}).await;
            assert_eq!(lock_bound_override(), outer);
            assert!(
                std::panic::AssertUnwindSafe(with_quick_lock_bound_async(async {
                    panic!("synthetic panic");
                }))
                .catch_unwind()
                .await
                .is_err()
            );
            assert_eq!(lock_bound_override(), outer);
            assert!(with_quick_lock_bound_async(std::future::pending::<()>())
                .now_or_never()
                .is_none());
            assert_eq!(lock_bound_override(), outer);
        })
        .await;
        assert_eq!(lock_bound_override(), original);
    }

    fn temp_datadir(tag: &str) -> (PathBuf, CoincubeDirectory) {
        let base = std::env::temp_dir().join(format!(
            "coincube-managed-conf-{}-{}-{:?}",
            tag,
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        (base.clone(), CoincubeDirectory::new(base))
    }

    fn section(rpc_port: u16, p2p_port: u16) -> InternalBitcoindNetworkConfig {
        InternalBitcoindNetworkConfig {
            rpc_port,
            p2p_port,
            prune: PRUNE_DEFAULT,
            rpc_auth: None,
        }
    }

    /// Install `hook` for the duration of `body`, on this thread only.
    fn with_write_hook<T>(
        hook: impl Fn(ConfWriteStep, &Path) -> io::Result<()> + 'static,
        body: impl FnOnce() -> T,
    ) -> T {
        CONF_WRITE_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
        let out = body();
        CONF_WRITE_HOOK.with(|h| *h.borrow_mut() = None);
        out
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn quick() -> (u32, std::time::Duration) {
        (3, std::time::Duration::from_millis(5))
    }

    // ── the lock ──────────────────────────────────────────────────────

    // One holder at a time, a bounded wait for the next, release on drop, and
    // a lock file that lives outside both family roots and is never removed.
    #[test]
    fn lock_is_exclusive_bounded_and_released_on_drop() {
        let (base, datadir) = temp_datadir("lock");
        let path = managed_conf_lock_path(&datadir);
        assert_eq!(path, base.join(MANAGED_CONF_LOCK_FILE));
        for family in NodeChainFamily::ALL.iter().copied() {
            let root = crate::node::bitcoind::internal_bitcoind_directory_for(&datadir, family);
            assert!(
                !path.starts_with(&root),
                "the lock must not live under {}",
                root.display()
            );
        }

        let held = ManagedConfLock::acquire(&datadir).unwrap();
        assert_eq!(held.path(), path.as_path());
        let (attempts, retry) = quick();
        let started = std::time::Instant::now();
        match ManagedConfLock::acquire_with_bound(&datadir, attempts, retry) {
            Err(ManagedConfLockError::Busy { path: busy }) => assert_eq!(busy, path),
            other => panic!("expected Busy, got {:?}", other),
        }
        // Bounded: roughly attempts × retry, not forever.
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        drop(held);
        let again = ManagedConfLock::acquire_with_bound(&datadir, attempts, retry).unwrap();
        drop(again);
        assert!(path.exists(), "the lock file is left in place");
        assert_eq!(std::fs::read(&path).unwrap(), b"", "and carries no state");
        let _ = std::fs::remove_dir_all(&base);
    }

    // A holder that panics still releases: the guard's Drop runs on unwind.
    #[test]
    fn lock_is_released_when_its_holder_panics() {
        let (base, datadir) = temp_datadir("panic");
        let d = datadir.clone();
        let result = std::thread::spawn(move || {
            let _held = ManagedConfLock::acquire(&d).unwrap();
            panic!("holder dies");
        })
        .join();
        assert!(result.is_err());
        let (attempts, retry) = quick();
        assert!(ManagedConfLock::acquire_with_bound(&datadir, attempts, retry).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    // Dropping the handle of a `spawn_blocking` worker does not abort it: the
    // lock stays held until the worker itself returns. This pins the accurate
    // statement, not a hoped-for immediate release.
    #[tokio::test]
    async fn lock_is_held_until_a_spawn_blocking_worker_returns_not_when_its_handle_is_dropped() {
        let (base, datadir) = temp_datadir("spawn-blocking");
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (returned_tx, returned_rx) = std::sync::mpsc::channel::<()>();
        let d = datadir.clone();
        let handle = tokio::task::spawn_blocking(move || {
            let held = ManagedConfLock::acquire(&d).unwrap();
            started_tx.send(()).unwrap();
            // Hold until told to, not for a wall-clock interval: the test
            // observes `Busy` while this is pending, then releases it.
            release_rx.recv().unwrap();
            drop(held);
            returned_tx.send(()).unwrap();
        });
        started_rx.recv().unwrap();
        // "Cancel" the task: the worker keeps running regardless.
        drop(handle);
        let (attempts, retry) = quick();
        assert!(matches!(
            ManagedConfLock::acquire_with_bound(&datadir, attempts, retry),
            Err(ManagedConfLockError::Busy { .. })
        ));
        // Only once the worker itself has released and returned is the lock
        // free — and then it is, deterministically.
        release_tx.send(()).unwrap();
        returned_rx.recv().unwrap();
        assert!(ManagedConfLock::acquire_with_bound(&datadir, attempts, retry).is_ok());
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The other side of [`lock_contention_and_release_across_processes`]: run
    /// as a child process, hold the lock on the datadir named by
    /// `COINCUBE_TEST_LOCK_DATADIR`, and follow stdin. Ignored so it never
    /// runs on its own.
    #[test]
    #[ignore]
    fn child_lock_holder() {
        use std::io::{BufRead, Write};
        let datadir = CoincubeDirectory::new(PathBuf::from(
            std::env::var("COINCUBE_TEST_LOCK_DATADIR").expect("datadir"),
        ));
        let held = ManagedConfLock::acquire(&datadir).expect("child acquires");
        let mut out = std::io::stdout();
        writeln!(out, "LOCKED").unwrap();
        out.flush().unwrap();
        let stdin = std::io::stdin();
        let mut held = Some(held);
        for line in stdin.lock().lines() {
            match line.as_deref() {
                Ok("release") => {
                    held.take();
                    writeln!(out, "RELEASED").unwrap();
                    out.flush().unwrap();
                }
                Ok("exit") => break,
                _ => break,
            }
        }
        std::process::exit(0);
    }

    // The requested cross-process evidence: a real child process holds the
    // lock, this process is refused (bounded), the child's explicit release
    // lets this process in, and a child that is killed while holding it
    // releases it by dying — no stale lock, nothing to clean up.
    #[test]
    fn lock_contention_and_release_across_processes() {
        use std::io::{BufRead, BufReader, Write};
        use std::process::{Command, Stdio};

        let (base, datadir) = temp_datadir("child");
        let spawn_child = || {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "node::managed_conf::tests::child_lock_holder",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("COINCUBE_TEST_LOCK_DATADIR", datadir.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn child test process");
            let stdout = BufReader::new(child.stdout.take().unwrap());
            let mut lines = stdout.lines();
            // libtest prints its own header lines — and, with `--nocapture`,
            // the `test … ...` prefix on the *same* line as the child's first
            // output — so match the end of the line, not the whole of it.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            loop {
                match lines.next() {
                    Some(Ok(line)) if line.trim_end().ends_with("LOCKED") => break,
                    Some(Ok(_)) => {}
                    other => panic!("child never reported LOCKED: {:?}", other),
                }
                assert!(std::time::Instant::now() < deadline, "child took too long");
            }
            (child, lines)
        };
        let (attempts, retry) = quick();

        // Graceful: refused while the child holds it, admitted once it releases.
        let (mut child, mut lines) = spawn_child();
        assert!(matches!(
            ManagedConfLock::acquire_with_bound(&datadir, attempts, retry),
            Err(ManagedConfLockError::Busy { .. })
        ));
        {
            let stdin = child.stdin.as_mut().unwrap();
            writeln!(stdin, "release").unwrap();
            stdin.flush().unwrap();
        }
        loop {
            match lines.next() {
                Some(Ok(line)) if line.trim_end().ends_with("RELEASED") => break,
                Some(Ok(_)) => {}
                other => panic!("child never reported RELEASED: {:?}", other),
            }
        }
        // The child is still alive; only the lock is gone.
        assert!(child.try_wait().unwrap().is_none());
        let mine = ManagedConfLock::acquire_with_bound(&datadir, attempts, retry).unwrap();
        drop(mine);
        {
            let stdin = child.stdin.as_mut().unwrap();
            writeln!(stdin, "exit").unwrap();
            stdin.flush().unwrap();
        }
        assert!(child.wait().unwrap().success());

        // Process death: the OS releases the lock with the descriptor.
        let (mut child, _lines) = spawn_child();
        assert!(matches!(
            ManagedConfLock::acquire_with_bound(&datadir, attempts, retry),
            Err(ManagedConfLockError::Busy { .. })
        ));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(ManagedConfLock::acquire_with_bound(&datadir, attempts, retry).is_ok());
        assert!(managed_conf_lock_path(&datadir).exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    // ── atomic replacement ────────────────────────────────────────────

    #[test]
    fn atomic_write_replaces_completely_and_preserves_modes() {
        let (base, _datadir) = temp_datadir("atomic");
        let path = base.join("dir").join("bitcoin.conf");

        // A new file: complete contents, private mode, no staging file left.
        write_conf_atomically(&path, b"first\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"first\n");
        #[cfg(unix)]
        assert_eq!(mode(&path), 0o600, "a fresh conf starts private");
        let entries = |dir: &Path| -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(dir)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        assert_eq!(entries(path.parent().unwrap()), vec!["bitcoin.conf"]);

        // An existing file keeps whatever mode it had — restrictive or not.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for m in [0o600u32, 0o640, 0o644] {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(m)).unwrap();
                write_conf_atomically(&path, format!("mode {m:o}\n").as_bytes()).unwrap();
                assert_eq!(mode(&path), m, "mode {:o} preserved across replacement", m);
                assert_eq!(
                    std::fs::read(&path).unwrap(),
                    format!("mode {m:o}\n").as_bytes()
                );
            }
            assert_eq!(entries(path.parent().unwrap()), vec!["bitcoin.conf"]);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    fn assert_pre_rename_failures(
        base: &Path,
        path: &Path,
        old: &[u8],
        no_staging: &impl Fn(&Path),
    ) {
        for failing in [
            ConfWriteStep::StageWrite,
            ConfWriteStep::StageSync,
            ConfWriteStep::Rename,
        ] {
            let seen_mode = std::sync::Arc::new(std::sync::Mutex::new(None::<u32>));
            let record = seen_mode.clone();
            let result = with_write_hook(
                move |step, staging| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if staging.exists() {
                            let m =
                                std::fs::metadata(staging).unwrap().permissions().mode() & 0o777;
                            *record.lock().unwrap() = Some(m);
                        }
                    }
                    #[cfg(not(unix))]
                    let _ = staging;
                    if step == failing {
                        Err(io::Error::other(format!("injected at {:?}", step)))
                    } else {
                        Ok(())
                    }
                },
                || write_conf_atomically(path, b"new\n"),
            );
            match result {
                Err(ConfWriteError::NotReplaced(e)) => {
                    assert!(e.to_string().contains("injected"), "{}", e)
                }
                other => panic!("{:?}: expected NotReplaced, got {:?}", failing, other),
            }
            assert_eq!(
                std::fs::read(path).unwrap(),
                old,
                "{:?}: destination changed",
                failing
            );
            no_staging(base);
            #[cfg(unix)]
            assert_eq!(
                *seen_mode.lock().unwrap(),
                Some(0o600),
                "{:?}: the staging file must be private while it exists",
                failing
            );
        }
    }

    // Pre-rename failures leave the destination byte-identical and no staging
    // file; the staging file is private while it exists; a post-rename
    // durability failure leaves the complete new bytes and says so.
    #[test]
    fn atomic_write_failures_are_classified_by_which_side_of_the_rename() {
        let (base, _datadir) = temp_datadir("atomic-fail");
        let path = base.join("bitcoin.conf");
        write_conf_atomically(&path, b"old\n").unwrap();
        let old = std::fs::read(&path).unwrap();
        let no_staging = |dir: &Path| {
            assert!(
                !std::fs::read_dir(dir).unwrap().any(|e| e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")),
                "a staging file was left behind"
            );
        };

        assert_pre_rename_failures(&base, &path, &old, &no_staging);

        // Post-rename: the directory flush fails, but the file is already new.
        let result = with_write_hook(
            |step, _| {
                if step == ConfWriteStep::DirSync {
                    Err(io::Error::other("injected dir sync"))
                } else {
                    Ok(())
                }
            },
            || write_conf_atomically(&path, b"new\n"),
        );
        match result {
            Err(ConfWriteError::ReplacedNotDurable(e)) => {
                assert!(e.to_string().contains("injected dir sync"))
            }
            other => panic!("expected ReplacedNotDurable, got {:?}", other),
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"new\n");
        no_staging(&base);

        // Through the locked update, the same two outcomes are reported apart:
        // a pre-rename failure is an error with the old bytes on disk …
        let mut conf = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        conf.networks
            .insert(Network::Bitcoin, section(41001, 41002));
        let conf_path = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &CoincubeDirectory::new(base.clone()),
            NodeChainFamily::Bitcoin,
        ));
        std::fs::create_dir_all(conf_path.parent().unwrap()).unwrap();
        conf.to_file(&conf_path).unwrap();
        let before = std::fs::read(&conf_path).unwrap();
        let datadir = CoincubeDirectory::new(base.clone());
        let result = with_write_hook(
            |step, _| {
                if step == ConfWriteStep::Rename {
                    Err(io::Error::other("injected rename"))
                } else {
                    Ok(())
                }
            },
            || {
                update_managed_conf(&datadir, NodeChainFamily::Bitcoin, |txn| {
                    let mut c = txn.conf.clone().unwrap();
                    c.networks.insert(Network::Testnet4, section(41003, 41004));
                    Ok(((), Some(c)))
                })
            },
        );
        assert!(matches!(result, Err(ManagedConfError::NotReplaced(_))));
        assert_eq!(std::fs::read(&conf_path).unwrap(), before);
        // … and a post-rename one is `Ok` with the new bytes and the
        // durability caveat attached.
        let result = with_write_hook(
            |step, _| {
                if step == ConfWriteStep::DirSync {
                    Err(io::Error::other("injected dir sync"))
                } else {
                    Ok(())
                }
            },
            || {
                update_managed_conf(&datadir, NodeChainFamily::Bitcoin, |txn| {
                    let mut c = txn.conf.clone().unwrap();
                    c.networks.insert(Network::Testnet4, section(41003, 41004));
                    Ok(((), Some(c)))
                })
            },
        )
        .unwrap();
        assert!(matches!(
            result.durability,
            ConfDurability::ReplacedNotDurable(_)
        ));
        let after = InternalBitcoindConfig::from_file(&conf_path).unwrap();
        assert!(after.networks.contains_key(&Network::Testnet4));
        let _ = std::fs::remove_dir_all(&base);
    }

    // A staging name that is already taken belongs to someone else: the write
    // moves to the next name and the colliding file is left exactly as it
    // was; only the staging file this call created is ever cleaned up. With
    // every name taken, the write refuses and still removes nothing.
    #[test]
    fn a_staging_collision_preserves_the_existing_file_and_moves_on() {
        let (base, _datadir) = temp_datadir("collision");
        let path = base.join("bitcoin.conf");
        write_conf_atomically(&path, b"old\n").unwrap();
        let old = std::fs::read(&path).unwrap();
        // Someone else's staging file, under a name we are about to be handed.
        let theirs = base.join("bitcoin.conf.12345.0.tmp");
        std::fs::write(&theirs, b"theirs, in progress\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&theirs, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let ours = base.join("bitcoin.conf.12345.1.tmp");
        let names = |seq: Vec<PathBuf>| {
            let mut seq = seq.into_iter();
            move || seq.next().expect("ran out of staging names")
        };

        // Collision first, then a free name: the write succeeds through the
        // free name, the colliding file is untouched, and our staging file is
        // gone after the rename.
        write_conf_atomically_with(&path, b"new\n", names(vec![theirs.clone(), ours.clone()]))
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new\n");
        assert_eq!(std::fs::read(&theirs).unwrap(), b"theirs, in progress\n");
        #[cfg(unix)]
        assert_eq!(
            mode(&theirs),
            0o644,
            "the colliding file's mode is not ours to change"
        );
        assert!(!ours.exists());

        // Only collisions on offer: refused as `NotReplaced(AlreadyExists)`,
        // destination byte-identical, the colliding file still there.
        write_conf_atomically(&path, &old).unwrap();
        let result = write_conf_atomically_with(&path, b"newer\n", || theirs.clone());
        match result {
            Err(ConfWriteError::NotReplaced(e)) => {
                assert_eq!(e.kind(), io::ErrorKind::AlreadyExists, "{}", e)
            }
            other => panic!("expected NotReplaced, got {:?}", other),
        }
        assert_eq!(std::fs::read(&path).unwrap(), old);
        assert_eq!(std::fs::read(&theirs).unwrap(), b"theirs, in progress\n");

        // Owned-stage cleanup is unchanged: a failure after our own creation
        // removes our file and, again, nobody else's.
        let result = with_write_hook(
            |step, _| {
                if step == ConfWriteStep::StageSync {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            },
            || {
                write_conf_atomically_with(
                    &path,
                    b"newer\n",
                    names(vec![theirs.clone(), ours.clone()]),
                )
            },
        );
        assert!(matches!(result, Err(ConfWriteError::NotReplaced(_))));
        assert!(!ours.exists(), "our staging file is cleaned up");
        assert!(theirs.exists(), "theirs is not");
        assert_eq!(std::fs::read(&path).unwrap(), old);
        let _ = std::fs::remove_dir_all(&base);
    }

    // ── contention ────────────────────────────────────────────────────

    /// A candidate source every contender shares: the same sequence, so the
    /// only thing that can keep two allocations apart is the reservation the
    /// first one persisted before the second one read.
    fn same_candidates() -> impl FnMut() -> Result<u16, String> {
        let mut seq = vec![40001u16, 40002, 40003, 40004, 40005, 40006].into_iter();
        move || seq.next().ok_or_else(|| "dry".to_string())
    }

    fn allocate_and_persist(
        datadir: &CoincubeDirectory,
        family: NodeChainFamily,
        network: Network,
    ) -> (u16, u16) {
        let flavor = family.allowed_flavors()[0];
        update_managed_conf(datadir, family, |txn| {
            let mut conf = txn
                .conf
                .clone()
                .unwrap_or_else(|| InternalBitcoindConfig::for_flavor(flavor));
            let ports = txn.allocate_ports(&conf, same_candidates())?;
            conf.networks.insert(network, section(ports.0, ports.1));
            Ok((ports, Some(conf)))
        })
        .unwrap()
        .value
    }

    // Two setups allocating at the same moment — one per family — from the
    // same candidate sequence end up with disjoint ports, both persisted, and
    // an unrelated pre-existing section survives. The barrier releases them
    // *before* either tries the lock, never inside it.
    #[test]
    fn contending_allocations_across_families_get_distinct_ports() {
        let (base, datadir) = temp_datadir("contend-families");
        // Something already there that neither contender is about.
        let mut seed = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        seed.networks
            .insert(Network::Regtest, section(49001, 49002));
        let bitcoin_conf = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &datadir,
            NodeChainFamily::Bitcoin,
        ));
        std::fs::create_dir_all(bitcoin_conf.parent().unwrap()).unwrap();
        seed.to_file(&bitcoin_conf).unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let run = |family: NodeChainFamily| {
            let datadir = datadir.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                allocate_and_persist(&datadir, family, Network::Bitcoin)
            })
        };
        let a = run(NodeChainFamily::Bitcoin);
        let b = run(NodeChainFamily::BitcoinBlake2b);
        let (ra, rb) = (a.join().unwrap(), b.join().unwrap());
        let mut all = vec![ra.0, ra.1, rb.0, rb.1];
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 4, "ports collided: {:?} / {:?}", ra, rb);
        // Each got the first two candidates *free at its turn*: whichever ran
        // second saw the first's persisted section and skipped past it.
        assert!(
            (ra == (40001, 40002) && rb == (40003, 40004))
                || (rb == (40001, 40002) && ra == (40003, 40004)),
            "{:?} / {:?}",
            ra,
            rb
        );
        // Both persisted, and the unrelated section is intact.
        let bitcoin = InternalBitcoindConfig::from_file(&bitcoin_conf).unwrap();
        assert_eq!(
            bitcoin.networks.get(&Network::Regtest),
            Some(&section(49001, 49002))
        );
        assert_eq!(
            bitcoin.networks.get(&Network::Bitcoin),
            Some(&section(ra.0, ra.1))
        );
        let blake2b_conf = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &datadir,
            NodeChainFamily::BitcoinBlake2b,
        ));
        let blake2b = InternalBitcoindConfig::from_file(&blake2b_conf).unwrap();
        assert_eq!(
            blake2b.networks.get(&Network::Bitcoin),
            Some(&section(rb.0, rb.1))
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // The same for two networks of one family: the second allocation sees the
    // first's new section in the fresh read and steers around it, and both
    // sections are in the one file afterwards.
    #[test]
    fn contending_allocations_across_networks_of_one_family_get_distinct_ports() {
        let (base, datadir) = temp_datadir("contend-networks");
        let barrier = Arc::new(Barrier::new(2));
        let run = |network: Network| {
            let datadir = datadir.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                allocate_and_persist(&datadir, NodeChainFamily::Bitcoin, network)
            })
        };
        let a = run(Network::Bitcoin);
        let b = run(Network::Testnet4);
        let (ra, rb) = (a.join().unwrap(), b.join().unwrap());
        let mut all = vec![ra.0, ra.1, rb.0, rb.1];
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), 4, "ports collided: {:?} / {:?}", ra, rb);
        let conf = InternalBitcoindConfig::from_file(&internal_bitcoind_config_path(
            &internal_bitcoind_datadir_for(&datadir, NodeChainFamily::Bitcoin),
        ))
        .unwrap();
        assert_eq!(
            conf.networks.get(&Network::Bitcoin),
            Some(&section(ra.0, ra.1))
        );
        assert_eq!(
            conf.networks.get(&Network::Testnet4),
            Some(&section(rb.0, rb.1))
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // Refusals happen before any write: a busy lock, an unreadable own conf,
    // and an edit that refuses all leave the file byte-identical.
    #[test]
    fn locked_update_refuses_before_writing() {
        let (base, datadir) = temp_datadir("refuse");
        let conf_path = internal_bitcoind_config_path(&internal_bitcoind_datadir_for(
            &datadir,
            NodeChainFamily::Bitcoin,
        ));
        std::fs::create_dir_all(conf_path.parent().unwrap()).unwrap();
        let mut seed = InternalBitcoindConfig::for_flavor(NodeFlavor::Knots);
        seed.networks
            .insert(Network::Bitcoin, section(41001, 41002));
        seed.to_file(&conf_path).unwrap();
        let before = std::fs::read(&conf_path).unwrap();

        // Busy: another holder (a separately opened descriptor conflicts
        // exactly as another process would).
        let held = ManagedConfLock::acquire(&datadir).unwrap();
        let result = with_quick_lock_bound(|| {
            update_managed_conf(&datadir, NodeChainFamily::Bitcoin, |txn| {
                let mut c = txn.conf.clone().unwrap();
                c.networks.insert(Network::Testnet4, section(41003, 41004));
                Ok(((), Some(c)))
            })
        });
        assert!(matches!(
            result,
            Err(ManagedConfError::Lock(ManagedConfLockError::Busy { .. }))
        ));
        assert_eq!(std::fs::read(&conf_path).unwrap(), before);
        drop(held);

        // The edit refuses (e.g. no acceptable port): nothing written.
        let result = update_managed_conf(&datadir, NodeChainFamily::Bitcoin, |txn| {
            let conf = txn.conf.clone().unwrap();
            let dry = || -> Result<u16, String> { Err("dry".to_string()) };
            txn.allocate_ports(&conf, dry)?;
            Ok(((), Some(conf)))
        });
        assert!(matches!(
            result,
            Err(ManagedConfError::Edit(ManagedConfEditError::Ports(_)))
        ));
        assert_eq!(std::fs::read(&conf_path).unwrap(), before);

        // Own conf unreadable (malformed): refused, not replaced by a fresh one.
        std::fs::write(&conf_path, "[main]\nrpcport=notanumber\nport=1\n").unwrap();
        let malformed = std::fs::read(&conf_path).unwrap();
        let result = update_managed_conf(&datadir, NodeChainFamily::Bitcoin, |_txn| {
            Ok(((), Some(InternalBitcoindConfig::new())))
        });
        assert!(matches!(result, Err(ManagedConfError::Unreadable(_))));
        assert_eq!(std::fs::read(&conf_path).unwrap(), malformed);

        // No conf at all is not an error: the edit sees `None`.
        std::fs::remove_file(&conf_path).unwrap();
        let outcome = update_managed_conf(&datadir, NodeChainFamily::Bitcoin, |txn| {
            assert!(txn.conf.is_none());
            Ok((true, None))
        })
        .unwrap();
        assert!(outcome.value);
        assert!(matches!(outcome.durability, ConfDurability::Unchanged));
        assert!(!conf_path.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
