//! Authenticated local IPC. Every request checks the live grant twice.
use crate::{Error, ReadPermissions, Reader, Request, Response};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    task::JoinHandle,
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use tokio::{
    net::{UnixListener, UnixStream},
    task::JoinSet,
};
#[cfg(windows)]
pub(crate) mod windows;

/// Maximum serialized request frame size, including the newline, in bytes.
pub const REQUEST_LIMIT: usize = 4096;
/// Maximum serialized response frame size, including the newline, in bytes.
pub const RESPONSE_LIMIT: usize = 65536;
/// Wall-clock budget for one peer connection and its wallet read.
pub const DEADLINE: Duration = Duration::from_secs(10);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    credential: String,
    request: Request,
}

/// Memory-only bearer grant bound to one wallet, capability set and expiry.
enum Scope {
    Read {
        wallet_id: String,
        permissions: ReadPermissions,
    },
    Proposal,
    Creation,
    Transaction {
        wallet_id: String,
    },
}

/// Memory-only bearer grant. Read and proposal scopes cannot upgrade each other.
pub struct Grant {
    session_valid: Box<dyn Fn() -> bool + Send + Sync>,
    credential: String,
    scope: Scope,
    expires: Option<Instant>,
    revoked: AtomicBool,
    native_admission: Mutex<()>,
    /// Completed successful operations; this is not a connected-client count.
    pub successful_calls: AtomicU64,
}
impl Grant {
    /// Generate a fresh 256-bit credential and a monotonic expiration deadline.
    ///
    /// Capabilities are balance, confirmed history and policy, in that order.
    pub fn new(wallet_id: String, capabilities: [bool; 3], duration: Duration) -> Self {
        Self::for_scope(
            Scope::Read {
                wallet_id,
                permissions: ReadPermissions::from_legacy(capabilities),
            },
            duration,
        )
    }
    /// New preparation-only grant, with a mandatory trusted session guard.
    /// It cannot list or read wallets. The trusted desktop must supply a fresh
    /// proposal store and a guard for the correct unlocked, empty Cube.
    pub fn for_proposals(
        duration: Duration,
        valid: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self::for_scope(Scope::Proposal, duration).with_session_guard(valid)
    }
    fn for_scope(scope: Scope, duration: Duration) -> Self {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        Self {
            session_valid: Box::new(|| true),
            credential: bytes.iter().map(|b| format!("{b:02x}")).collect(),
            scope,
            expires: Some(Instant::now() + duration),
            revoked: AtomicBool::new(false),
            native_admission: Mutex::new(()),
            successful_calls: AtomicU64::new(0),
        }
    }
    /// Restore an explicitly paired read client. The trusted host must enforce
    /// live session identity; durable pairing is never an unattended unlock.
    pub fn paired(
        wallet_id: String,
        capabilities: [bool; 3],
        credential: String,
        valid: impl Fn() -> bool + Send + Sync + 'static,
    ) -> io::Result<Self> {
        if credential.len() != 64 || !credential.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(io::Error::other("invalid pairing credential"));
        }
        let mut grant = Self::new(wallet_id, capabilities, Duration::from_secs(1));
        grant.credential = credential;
        grant.expires = None;
        Ok(grant.with_session_guard(valid))
    }
    /// Restore a preparation-only pairing without preserving proposal approval.
    pub fn paired_proposals(
        credential: String,
        valid: impl Fn() -> bool + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let mut grant = Self::paired(String::new(), [false; 3], credential, valid)?;
        grant.scope = Scope::Proposal;
        Ok(grant)
    }
    /// Restore separately consented public-key wallet preparation, not creation authority.
    pub fn paired_creation(
        credential: String,
        valid: impl Fn() -> bool + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let mut grant = Self::paired(String::new(), [false; 3], credential, valid)?;
        grant.scope = Scope::Creation;
        Ok(grant)
    }
    /// Restore separate unsigned-transaction preparation authority for one wallet.
    pub fn paired_transactions(
        wallet_id: String,
        credential: String,
        valid: impl Fn() -> bool + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let mut grant = Self::paired(wallet_id.clone(), [false; 3], credential, valid)?;
        grant.scope = Scope::Transaction { wallet_id };
        Ok(grant)
    }
    async fn wait_expiry(&self) {
        if let Some(expiry) = self.expires {
            tokio::time::sleep_until(tokio::time::Instant::from_std(expiry)).await;
        } else {
            std::future::pending::<()>().await;
        }
    }
    /// Bind authorization to the desktop's live unlocked-session generation.
    pub fn with_session_guard(mut self, valid: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.session_valid = Box::new(valid);
        self
    }
    /// Irreversibly invalidate the grant, including future completion checks.
    pub fn revoke(&self) {
        let _admission = self
            .native_admission
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.revoked.store(true, Ordering::SeqCst);
    }
    /// Trusted native commit gate, never an MCP operation. The callback must
    /// atomically check the captured desktop session and exact reviewed state.
    /// The grant is consumed on success, before any async durable operation.
    /// A prior revoke wins; an admitted native operation cannot be undone by
    /// revoking afterward. No mutex is held across async work.
    pub fn admit_native<T>(&self, admit: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        let _guard = self
            .native_admission
            .lock()
            .map_err(|_| Error::AccessDenied)?;
        if !matches!(self.scope, Scope::Proposal | Scope::Creation)
            || self.revoked.load(Ordering::SeqCst)
            || self.expires.is_some_and(|expiry| Instant::now() >= expiry)
        {
            return Err(Error::AccessDenied);
        }
        // Deliberately no session_valid call: the callback must hold the native
        // session lock through admission, not make a racy boolean snapshot.
        let value = admit()?;
        // A slow native validation/metadata fsync cannot extend the grant's
        // monotonic lifetime. No wallet operation may begin on this error.
        if self.expires.is_some_and(|expiry| Instant::now() >= expiry) {
            self.revoked.store(true, Ordering::SeqCst);
            return Err(Error::AccessDenied);
        }
        self.revoked.store(true, Ordering::SeqCst);
        Ok(value)
    }

    /// Require an unrevoked grant, unexpired lifetime and valid desktop session.
    pub fn active(&self) -> bool {
        !self.revoked.load(Ordering::SeqCst)
            && self.expires.is_none_or(|expiry| Instant::now() < expiry)
            && (self.session_valid)()
    }
    /// Check live authorization, constant-time credential equality, scope and input.
    ///
    /// Wallet identity and capabilities are checked before validation errors are
    /// returned, so an unauthorized caller does not learn wallet details.
    pub fn authorize(&self, credential: &str, request: &Request) -> Result<(), Error> {
        if !self.active() || !bool::from(self.credential.as_bytes().ct_eq(credential.as_bytes())) {
            return Err(Error::AccessDenied);
        }
        match (&self.scope, request) {
            (Scope::Proposal, Request::PrepareProposal(_) | Request::Proposal(_)) => {}
            (Scope::Creation, Request::PrepareCreation(_) | Request::Proposal(_)) => {}
            (Scope::Transaction { .. }, Request::List(_)) => {}
            (Scope::Transaction { wallet_id }, Request::PrepareTransaction(input))
                if input.wallet_id == *wallet_id => {}
            (Scope::Transaction { wallet_id }, Request::Transaction(input))
                if input.wallet_id == *wallet_id => {}
            (Scope::Read { .. }, Request::List(_)) => {}
            (
                Scope::Read {
                    wallet_id,
                    permissions,
                },
                _,
            ) => {
                let Some((wallet, cap)) = request.scope() else {
                    return Err(Error::AccessDenied);
                };
                if wallet != wallet_id || !permissions.permits(cap) {
                    return Err(Error::AccessDenied);
                }
            }
            _ => return Err(Error::AccessDenied),
        }
        if !request.valid() {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
    /// Return the bearer credential only for private trusted-host configuration.
    /// Never include this value in model tool arguments, results or logs.
    pub fn credential(&self) -> &str {
        &self.credential
    }
}

/// Session-owned private listener; dropping it revokes access and aborts workers.
pub struct Server {
    /// Shared grant used for trusted settings status and explicit revocation.
    pub grant: Arc<Grant>,
    /// Private endpoint path copied into trusted client configuration.
    pub socket: PathBuf,
    task: JoinHandle<()>,
    _directory: Option<tempfile::TempDir>,
}
#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum AcceptErrorAction {
    Retry,
    Backoff,
    Revoke,
}

#[cfg(unix)]
fn accept_error_action(error: &io::Error) -> AcceptErrorAction {
    // A peer can disappear before accept completes. File descriptor and memory
    // pressure can also clear without rebuilding the desktop's paired grant.
    // Delay resource retries so a persistent EMFILE/ENFILE cannot spin.
    match error.raw_os_error() {
        Some(libc::ECONNABORTED | libc::EINTR | libc::EAGAIN | libc::EPROTO) => {
            AcceptErrorAction::Retry
        }
        Some(libc::EMFILE | libc::ENFILE | libc::ENOMEM | libc::ENOBUFS) => {
            AcceptErrorAction::Backoff
        }
        _ => AcceptErrorAction::Revoke,
    }
}

impl Server {
    /// Bind a protected current-logon named pipe and start authenticated reads.
    /// Requires an active Tokio runtime; setup errors do not authorize a client.
    #[cfg(windows)]
    pub fn start(reader: Arc<dyn Reader>, grant: Grant) -> io::Result<Self> {
        windows::start(reader, grant)
    }
    #[cfg(unix)]
    /// Bind a fresh private Unix endpoint and start bounded, authenticated reads.
    /// Requires an active Tokio runtime; setup errors do not authorize a client.
    pub fn start(reader: Arc<dyn Reader>, grant: Grant) -> io::Result<Self> {
        // /tmp keeps sockaddr_un paths short even on macOS. tempfile creates
        // the directory atomically with 0700; never chmod an existing path.
        let directory = tempfile::Builder::new()
            .prefix("coincube-mcp-")
            .tempdir_in("/tmp")?;
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
        let socket = directory.path().join("rpc");
        let listener = std::os::unix::net::UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let grant = Arc::new(grant);
        let permission = grant.clone();
        let task = tokio::spawn(async move {
            let Ok(listener) = UnixListener::from_std(listener) else {
                permission.revoke();
                return;
            };
            let mut workers = JoinSet::new();
            loop {
                tokio::select! {
                    _ = permission.wait_expiry() => { permission.revoke(); break; }
                    Some(_) = workers.join_next(), if !workers.is_empty() => {}
                    connection = listener.accept(), if workers.len() < 8 => {
                        let (stream, _) = match connection {
                            Ok(accepted) => accepted,
                            Err(error) => match accept_error_action(&error) {
                                AcceptErrorAction::Retry => {
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                    continue;
                                }
                                AcceptErrorAction::Backoff => {
                                    tokio::time::sleep(Duration::from_millis(100)).await;
                                    continue;
                                }
                                AcceptErrorAction::Revoke => { permission.revoke(); break; }
                            },
                        };
                        let reader = reader.clone(); let grant = permission.clone();
                        workers.spawn(async move { let _ = tokio::time::timeout(DEADLINE, serve(stream, reader, grant)).await; });
                    }
                }
            }
            // JoinSet drop aborts all in-flight reads, including stalled peers.
        });
        Ok(Self {
            grant,
            socket,
            task,
            _directory: Some(directory),
        })
    }
}
impl Drop for Server {
    /// Revoke synchronously before scheduling listener and worker cancellation.
    fn drop(&mut self) {
        self.grant.revoke();
        self.task.abort();
    }
}

/// Read one newline-terminated frame, rejecting truncation and oversized input.
/// The caller supplies the connection deadline; this helper only bounds bytes.
pub async fn line<R: tokio::io::AsyncRead + Unpin>(reader: R, max: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    BufReader::new(reader)
        .take((max + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .await?;
    if bytes.len() > max || bytes.last() != Some(&b'\n') {
        return Err(io::Error::other("invalid frame"));
    }
    Ok(bytes)
}
/// Authenticate one request before and after a read, then write a bounded result.
async fn serve<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    mut stream: S,
    reader: Arc<dyn Reader>,
    grant: Arc<Grant>,
) -> io::Result<()> {
    let bytes = line(&mut stream, REQUEST_LIMIT).await?;
    let result = match serde_json::from_slice::<Envelope>(&bytes) {
        Ok(envelope) => {
            match grant.authorize(&envelope.credential, &envelope.request) {
                Err(e) => Err(e),
                Ok(()) => {
                    let request = envelope.request;
                    let result = reader.read(request.clone()).await;
                    // Never release a completion from a revoked or expired session.
                    grant.authorize(&envelope.credential, &request).and(result)
                }
            }
        }
        Err(_) => Err(Error::InvalidInput),
    };
    if result.is_ok() {
        grant.successful_calls.fetch_add(1, Ordering::Relaxed);
    }
    let mut bytes = serde_json::to_vec(&result).map_err(|_| io::Error::other("serialization"))?;
    if bytes.len() >= RESPONSE_LIMIT {
        bytes = serde_json::to_vec(&Err::<Response, _>(Error::Unavailable))?;
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).await?;
    // A Windows pipe server must keep its handle alive until the client has
    // consumed the reply. The client closes after its single response; the
    // existing deadline bounds peers that leave their handle open.
    #[cfg(windows)]
    {
        let mut end = [0u8; 1];
        let _ = stream.read(&mut end).await;
    }
    Ok(())
}
/// Make one authenticated local request within the frame and time limits.
/// Endpoint and credential are trusted host configuration, never tool arguments;
/// transport failures become sanitized unavailable errors.
pub async fn call(socket: &Path, credential: &str, request: Request) -> Result<Response, Error> {
    let operation = async {
        #[cfg(unix)]
        let mut stream = UnixStream::connect(socket)
            .await
            .map_err(|_| Error::Unavailable)?;
        #[cfg(windows)]
        let mut stream = windows::connect(socket)
            .await
            .map_err(|_| Error::Unavailable)?;
        let envelope = Envelope {
            credential: credential.into(),
            request,
        };
        let mut bytes = serde_json::to_vec(&envelope).map_err(|_| Error::InvalidInput)?;
        bytes.push(b'\n');
        if bytes.len() > REQUEST_LIMIT {
            return Err(Error::InvalidInput);
        }
        stream
            .write_all(&bytes)
            .await
            .map_err(|_| Error::Unavailable)?;
        let bytes = line(&mut stream, RESPONSE_LIMIT)
            .await
            .map_err(|_| Error::Unavailable)?;
        serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?
    };
    tokio::time::timeout(DEADLINE, operation)
        .await
        .map_err(|_| Error::Unavailable)?
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn transient_accept_failures_preserve_pairing_and_resource_errors_back_off() {
        for code in [libc::ECONNABORTED, libc::EINTR, libc::EAGAIN, libc::EPROTO] {
            assert_eq!(
                accept_error_action(&io::Error::from_raw_os_error(code)),
                AcceptErrorAction::Retry
            );
        }
        for code in [libc::EMFILE, libc::ENFILE, libc::ENOMEM, libc::ENOBUFS] {
            assert_eq!(
                accept_error_action(&io::Error::from_raw_os_error(code)),
                AcceptErrorAction::Backoff
            );
        }
        assert_eq!(
            accept_error_action(&io::Error::from_raw_os_error(libc::EBADF)),
            AcceptErrorAction::Revoke
        );
    }
}
