//! Gui-side client for the `coincube-spark-bridge` subprocess.
//!
//! Architecture
//! ------------
//!
//! The Breez Spark SDK lives in a sibling binary because its dep graph
//! (rusqlite / libsqlite3-sys / tokio_with_wasm) can't be unified with
//! breez-sdk-liquid's. See `coincube-spark-bridge/Cargo.toml` for the
//! companion crate.
//!
//! [`SparkClient`] owns a [`tokio::process::Child`] and three background
//! tokio tasks:
//!
//! - **writer**: pulls [`Request`] frames from an mpsc channel and
//!   writes them as JSON lines to the child's stdin.
//! - **reader**: reads JSON lines from the child's stdout, parses
//!   [`Frame`]s, and routes [`Response`]s through a shared pending map
//!   (`id -> oneshot::Sender`). [`Event`] frames go to a future event
//!   channel (not wired in Phase 3 — just logged for now).
//! - **stderr pump**: logs each stderr line from the bridge at warn level.
//!
//! A request goes like: allocate id, insert `oneshot::Sender` into pending
//! map, send `Request` over the writer channel, await the oneshot. The
//! reader task resolves oneshots by id as responses come back, so
//! concurrent requests don't block each other.
//!
//! Lifecycle
//! ---------
//!
//! [`SparkClient::connect`] spawns the bridge, performs the
//! [`Method::Init`] handshake, and returns the client on success. If the
//! bridge exits before responding, or returns an error, the call fails
//! and the child is cleaned up. On drop the client sends a best-effort
//! [`Method::Shutdown`] (non-blocking fire-and-forget) and kills the
//! child if it didn't exit on its own — `kill_on_drop(true)` on the
//! `Command` ensures the OS reaps it even if the graceful path fails.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use coincube_spark_protocol::{
    ClaimDepositOk, ClaimDepositParams, ErrorKind, ErrorPayload, Event, Frame, GetInfoOk,
    GetInfoParams, GetUserSettingsOk, InitParams, ListPaymentsOk, ListPaymentsParams,
    ListUnclaimedDepositsOk, Method, OkPayload, ParseInputOk, ParseInputParams,
    PrepareLnurlPayParams, PrepareSendOk, PrepareSendParams, ReceiveBolt11Params,
    ReceiveOnchainParams, ReceivePaymentOk, Request, Response, ResponseResult, SendPaymentOk,
    SendPaymentParams, SetStableBalanceParams,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tracing::{debug, error, warn};

use super::config::SparkConfig;

/// Shared pending-request table. Each entry maps an outstanding request
/// id to the oneshot sender that the caller is awaiting.
type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

/// Handle to a running `coincube-spark-bridge` subprocess.
///
/// Clone-safe: the underlying state is `Arc`-shared, so multiple panels
/// can call methods concurrently. Dropping the last clone triggers a
/// best-effort graceful shutdown of the child process.
#[derive(Clone)]
pub struct SparkClient {
    inner: Arc<SparkClientInner>,
}

/// Shared flag: `true` once the client is shut down (explicitly or
/// via bridge crash). Shared between `SparkClientInner` and
/// `spawn_reader_task` so the reader can mark the client dead when
/// stdout closes unexpectedly.
type ClosedFlag = Arc<std::sync::atomic::AtomicBool>;

struct SparkClientInner {
    next_id: AtomicU64,
    request_tx: mpsc::UnboundedSender<Request>,
    pending: PendingMap,
    /// Broadcast channel into which the reader task pushes every
    /// [`Event`] frame received from the bridge.
    event_tx: broadcast::Sender<Event>,
    child: Mutex<Option<Child>>,
    /// True once `shutdown()` was called, the client was dropped, or
    /// the reader task detected that the bridge subprocess exited —
    /// further requests short-circuit with [`SparkClientError::BridgeUnavailable`].
    closed: ClosedFlag,
}

impl std::fmt::Debug for SparkClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparkClient").finish_non_exhaustive()
    }
}

impl SparkClient {
    /// Spawn the bridge subprocess, hand it a mnemonic + config, and
    /// return the connected client.
    ///
    /// `mnemonic` is passed into the bridge over stdin and then dropped
    /// by the caller (see [`super::mod::load_spark_client`] for the
    /// zeroizing wrapper). The bridge keeps it in memory for the
    /// session lifetime.
    pub async fn connect(config: SparkConfig, mnemonic: &str) -> Result<Self, SparkClientError> {
        if config.api_key.is_empty() {
            return Err(SparkClientError::Config(
                "Spark SDK API key is empty — set BREEZ_API_KEY at build time".to_string(),
            ));
        }

        let bridge_path = resolve_bridge_path()?;
        debug!("spawning Spark bridge at {:?}", bridge_path);

        let mut child = Command::new(&bridge_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                SparkClientError::BridgeUnavailable(format!(
                    "failed to spawn {}: {}",
                    bridge_path.display(),
                    e
                ))
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            SparkClientError::BridgeUnavailable("bridge stdin was not piped".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            SparkClientError::BridgeUnavailable("bridge stdout was not piped".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            SparkClientError::BridgeUnavailable("bridge stderr was not piped".to_string())
        })?;

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let (request_tx, request_rx) = mpsc::unbounded_channel::<Request>();
        // Buffer 64 events — at the bridge's event rate (one per SDK
        // sync tick + one per payment state change) that's several
        // minutes of headroom even if a subscriber is paused.
        let (event_tx, _) = broadcast::channel::<Event>(64);

        let closed: ClosedFlag = Arc::new(std::sync::atomic::AtomicBool::new(false));

        spawn_writer_task(stdin, request_rx, Arc::clone(&pending), Arc::clone(&closed));
        spawn_reader_task(
            stdout,
            Arc::clone(&pending),
            event_tx.clone(),
            Arc::clone(&closed),
        );
        spawn_stderr_task(stderr);

        let inner = Arc::new(SparkClientInner {
            next_id: AtomicU64::new(1),
            request_tx,
            pending,
            event_tx,
            child: Mutex::new(Some(child)),
            closed,
        });
        let client = Self { inner };

        // Perform the init handshake. If this fails, drop the client so
        // the child process is killed via `kill_on_drop`.
        let init_params = InitParams {
            api_key: config.api_key,
            network: config.network,
            mnemonic: mnemonic.to_string(),
            mnemonic_passphrase: None,
            storage_dir: config
                .storage_dir
                .to_str()
                .ok_or_else(|| {
                    SparkClientError::Config(
                        "Spark storage_dir contains non-UTF-8 bytes".to_string(),
                    )
                })?
                .to_string(),
        };

        match client.request(Method::Init(init_params)).await? {
            OkPayload::Init {} => Ok(client),
            other => Err(SparkClientError::Protocol(format!(
                "init returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Fetch wallet info (balance, identity pubkey).
    pub async fn get_info(&self) -> Result<GetInfoOk, SparkClientError> {
        match self
            .request(Method::GetInfo(GetInfoParams {
                ensure_synced: Some(true),
            }))
            .await?
        {
            OkPayload::GetInfo(info) => Ok(info),
            other => Err(SparkClientError::Protocol(format!(
                "get_info returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// List recent payments.
    pub async fn list_payments(
        &self,
        limit: Option<u32>,
    ) -> Result<ListPaymentsOk, SparkClientError> {
        match self
            .request(Method::ListPayments(ListPaymentsParams {
                limit,
                offset: Some(0),
            }))
            .await?
        {
            OkPayload::ListPayments(list) => Ok(list),
            other => Err(SparkClientError::Protocol(format!(
                "list_payments returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4e: classify a user-supplied destination string.
    ///
    /// Calls `BreezSdk::parse` on the bridge side and returns a
    /// high-level [`ParseInputOk`] tag the gui can branch on. The Send
    /// panel uses this before `prepare_send` to route LNURL /
    /// Lightning-address inputs to [`Self::prepare_lnurl_pay`].
    pub async fn parse_input(&self, input: String) -> Result<ParseInputOk, SparkClientError> {
        match self
            .request(Method::ParseInput(ParseInputParams { input }))
            .await?
        {
            OkPayload::ParseInput(ok) => Ok(ok),
            other => Err(SparkClientError::Protocol(format!(
                "parse_input returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4e: prepare an LNURL-pay / Lightning-address send.
    ///
    /// Companion to [`Self::prepare_send`] for the LNURL code path.
    /// Returns the same [`PrepareSendOk`] shape so the gui's state
    /// machine doesn't need a parallel send branch — the bridge
    /// remembers which pending map the handle belongs to and dispatches
    /// to `sdk.lnurl_pay` vs `sdk.send_payment` transparently when the
    /// gui calls [`Self::send_payment`] with the handle.
    ///
    /// `amount_sat` is required (LNURL servers always specify a
    /// min/max range). `comment` is forwarded if the server allows
    /// comments.
    pub async fn prepare_lnurl_pay(
        &self,
        input: String,
        amount_sat: u64,
        comment: Option<String>,
    ) -> Result<PrepareSendOk, SparkClientError> {
        match self
            .request(Method::PrepareLnurlPay(PrepareLnurlPayParams {
                input,
                amount_sat,
                comment,
            }))
            .await?
        {
            OkPayload::PrepareSend(ok) => Ok(ok),
            other => Err(SparkClientError::Protocol(format!(
                "prepare_lnurl_pay returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4c: parse a destination + compute a send preview.
    ///
    /// `input` accepts BOLT11 invoices, BIP21 URIs, and on-chain Bitcoin
    /// addresses. LNURL / Lightning Address destinations should go
    /// through [`Self::parse_input`] + [`Self::prepare_lnurl_pay`]
    /// instead — `prepare_send` rejects them at the SDK level.
    ///
    /// `amount_sat` is required for amountless invoices and on-chain
    /// sends; ignored otherwise. Returns a [`PrepareSendOk`] whose
    /// `handle` must be echoed back to [`Self::send_payment`] to
    /// execute the send. The bridge holds the full SDK prepare
    /// response under that key — the handle is single-use.
    pub async fn prepare_send(
        &self,
        input: String,
        amount_sat: Option<u64>,
    ) -> Result<PrepareSendOk, SparkClientError> {
        match self
            .request(Method::PrepareSend(PrepareSendParams { input, amount_sat }))
            .await?
        {
            OkPayload::PrepareSend(prepare) => Ok(prepare),
            other => Err(SparkClientError::Protocol(format!(
                "prepare_send returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4c: execute a previously-prepared send.
    ///
    /// `prepare_handle` must come from a prior [`Self::prepare_send`]
    /// response. It is consumed by the bridge on success or failure —
    /// calling twice with the same handle returns a
    /// [`SparkClientError::BridgeError`] with
    /// [`ErrorKind::BadRequest`].
    pub async fn send_payment(
        &self,
        prepare_handle: String,
    ) -> Result<SendPaymentOk, SparkClientError> {
        match self
            .request(Method::SendPayment(SendPaymentParams { prepare_handle }))
            .await?
        {
            OkPayload::SendPayment(sent) => Ok(sent),
            other => Err(SparkClientError::Protocol(format!(
                "send_payment returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4c: generate a BOLT11 invoice.
    ///
    /// `amount_sat = None` produces an amountless invoice. `description`
    /// is shown to the payer's wallet. `expiry_secs = None` defers to
    /// the SDK default (typically 24h).
    pub async fn receive_bolt11(
        &self,
        amount_sat: Option<u64>,
        description: String,
        expiry_secs: Option<u32>,
    ) -> Result<ReceivePaymentOk, SparkClientError> {
        match self
            .request(Method::ReceiveBolt11(ReceiveBolt11Params {
                amount_sat,
                description,
                expiry_secs,
            }))
            .await?
        {
            OkPayload::ReceivePayment(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "receive_bolt11 returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4c: generate an on-chain Bitcoin deposit address.
    ///
    /// Note: Spark's on-chain receive model requires a separate
    /// `claim_deposit` call once the incoming tx has confirmed —
    /// that's Phase 4d work. Phase 4c just returns the address and
    /// trusts the user / background sync to complete the claim
    /// eventually.
    pub async fn receive_onchain(
        &self,
        new_address: Option<bool>,
    ) -> Result<ReceivePaymentOk, SparkClientError> {
        match self
            .request(Method::ReceiveOnchain(ReceiveOnchainParams { new_address }))
            .await?
        {
            OkPayload::ReceivePayment(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "receive_onchain returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4f: list on-chain deposits the SDK has noticed but not
    /// yet claimed into the Spark wallet. Drives the "Pending
    /// deposits" card in the Receive panel.
    pub async fn list_unclaimed_deposits(
        &self,
    ) -> Result<ListUnclaimedDepositsOk, SparkClientError> {
        match self.request(Method::ListUnclaimedDeposits).await? {
            OkPayload::ListUnclaimedDeposits(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "list_unclaimed_deposits returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 4f: claim a specific (txid, vout) deposit into the Spark
    /// wallet. Returns the resulting payment id + claimed amount.
    /// Fails with [`SparkClientError::BridgeError`] / [`ErrorKind::Sdk`]
    /// when the deposit isn't mature yet — the gui should gate the
    /// Claim button on the deposit's `is_mature` field to avoid
    /// firing pre-mature claims.
    pub async fn claim_deposit(
        &self,
        txid: String,
        vout: u32,
    ) -> Result<ClaimDepositOk, SparkClientError> {
        match self
            .request(Method::ClaimDeposit(ClaimDepositParams { txid, vout }))
            .await?
        {
            OkPayload::ClaimDeposit(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "claim_deposit returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 6: read the SDK's `UserSettings` (Stable Balance on/off,
    /// private mode). Boolean-flattened on the bridge side so the gui
    /// never sees the USDB token label.
    pub async fn get_user_settings(&self) -> Result<GetUserSettingsOk, SparkClientError> {
        match self.request(Method::GetUserSettings).await? {
            OkPayload::GetUserSettings(resp) => Ok(resp),
            other => Err(SparkClientError::Protocol(format!(
                "get_user_settings returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Phase 6: activate (enabled=true) or deactivate (false) the
    /// Stable Balance feature. The bridge translates this into
    /// `update_user_settings(stable_balance_active_label = ...)`.
    pub async fn set_stable_balance(&self, enabled: bool) -> Result<(), SparkClientError> {
        match self
            .request(Method::SetStableBalance(SetStableBalanceParams { enabled }))
            .await?
        {
            OkPayload::SetStableBalance {} => Ok(()),
            other => Err(SparkClientError::Protocol(format!(
                "set_stable_balance returned unexpected payload: {:?}",
                other
            ))),
        }
    }

    /// Subscribe to bridge [`Event`] frames. Each call returns a fresh
    /// `broadcast::Receiver` — each subscriber gets its own independent
    /// cursor over the buffered events. The [`iced::Subscription`]
    /// helper below wraps this into an iced subscription stream.
    pub fn subscribe_events(&self) -> broadcast::Receiver<Event> {
        self.inner.event_tx.subscribe()
    }

    /// Build an iced [`Subscription`](iced::Subscription) over the
    /// bridge's event stream. Fires a [`SparkClientEvent`] every time
    /// the bridge forwards an SDK event, and silently resumes when a
    /// subscriber lags.
    ///
    /// The state parameter hashes on a per-client identity (the
    /// `event_tx` pointer) so swapping out the SparkClient on a
    /// reconnect produces a fresh subscription instead of re-binding
    /// to the old channel.
    pub fn event_subscription(&self) -> iced::Subscription<SparkClientEvent> {
        iced::Subscription::run_with(
            SparkEventSubscriptionState {
                client: self.clone(),
            },
            make_spark_event_stream,
        )
    }

    /// Gracefully shut down the bridge subprocess. After this returns
    /// the client is no longer usable.
    pub async fn shutdown(&self) -> Result<(), SparkClientError> {
        if self.inner.closed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        // Best-effort: send Shutdown and wait up to 5s for the child
        // to exit, otherwise kill it.
        let shutdown_result =
            tokio::time::timeout(Duration::from_secs(5), self.request(Method::Shutdown)).await;
        match shutdown_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => warn!("Spark bridge shutdown RPC failed: {}", e),
            Err(_) => warn!("Spark bridge shutdown RPC timed out"),
        }

        let mut guard = self.inner.child.lock().await;
        if let Some(mut child) = guard.take() {
            match tokio::time::timeout(Duration::from_secs(2), child.wait()).await {
                Ok(Ok(status)) => debug!("Spark bridge exited with status {}", status),
                Ok(Err(e)) => warn!("failed to wait() for Spark bridge: {}", e),
                Err(_) => {
                    warn!("Spark bridge did not exit within 2s, killing");
                    let _ = child.kill().await;
                }
            }
        }
        Ok(())
    }

    /// Send a request and await its response.
    ///
    /// Wires up an oneshot channel in the pending map keyed by a fresh
    /// monotonic id, pushes the [`Request`] through the writer channel,
    /// and awaits the oneshot. Any error response is translated into
    /// [`SparkClientError::BridgeError`].
    async fn request(&self, method: Method) -> Result<OkPayload, SparkClientError> {
        // Allow Shutdown through even after closed is set — shutdown()
        // flips the flag first to block new RPCs, then sends the
        // Shutdown request itself. Every other method is rejected once
        // closed is true.
        if !matches!(method, Method::Shutdown) && self.inner.closed.load(Ordering::SeqCst) {
            return Err(SparkClientError::BridgeUnavailable(
                "Spark client has been shut down".to_string(),
            ));
        }

        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel::<Response>();
        self.inner.pending.lock().await.insert(id, tx);

        let request = Request { id, method };
        if self.inner.request_tx.send(request).is_err() {
            // Writer task exited — bridge is dead.
            self.inner.pending.lock().await.remove(&id);
            return Err(SparkClientError::BridgeUnavailable(
                "Spark bridge writer task exited".to_string(),
            ));
        }

        // 30s is plenty for connect + info + list; longer timeouts can
        // be plumbed per-method later if we add heavy RPCs.
        let response = match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(SparkClientError::BridgeUnavailable(
                    "Spark bridge reader closed the response channel".to_string(),
                ));
            }
            Err(_) => {
                self.inner.pending.lock().await.remove(&id);
                return Err(SparkClientError::BridgeUnavailable(format!(
                    "Spark bridge did not respond within 30s (id={})",
                    id
                )));
            }
        };

        match response.result {
            ResponseResult::Ok(payload) => Ok(payload),
            ResponseResult::Err(ErrorPayload { kind, message }) => {
                Err(SparkClientError::BridgeError { kind, message })
            }
        }
    }
}

// Drop is implemented on `SparkClientInner` (not `SparkClient`)
// because `SparkClient` is `Clone` — panels and subscription
// descriptors create short-lived clones that are discarded
// frequently. If Drop were on `SparkClient`, every clone drop
// would kill the bridge. Putting it on the inner struct behind
// `Arc` means it fires exactly once, when the last strong
// reference is released.
impl Drop for SparkClientInner {
    fn drop(&mut self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.request_tx.send(Request {
            id: u64::MAX,
            method: Method::Shutdown,
        });
    }
}

// ---------------------------------------------------------------------------
// Bridge binary discovery
// ---------------------------------------------------------------------------

/// Locate the `coincube-spark-bridge` executable.
///
/// Precedence:
/// 1. `COINCUBE_SPARK_BRIDGE_PATH` env var (absolute path override).
/// 2. Sibling of the current executable, for packaged builds.
/// 3. Workspace `target/debug` / `target/release`, for `cargo run`.
fn resolve_bridge_path() -> Result<PathBuf, SparkClientError> {
    if let Ok(override_path) = std::env::var("COINCUBE_SPARK_BRIDGE_PATH") {
        let p = PathBuf::from(override_path);
        if p.exists() {
            return Ok(p);
        }
        return Err(SparkClientError::BridgeUnavailable(format!(
            "COINCUBE_SPARK_BRIDGE_PATH={} does not exist",
            p.display()
        )));
    }

    let exe_name = if cfg!(windows) {
        "coincube-spark-bridge.exe"
    } else {
        "coincube-spark-bridge"
    };

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(dir) = current_exe.parent() {
            let sibling = dir.join(exe_name);
            if sibling.exists() {
                return Ok(sibling);
            }
        }
    }

    // Dev fallback: look relative to the workspace so `cargo run` works
    // out of the box without copying the bridge binary.
    let workspace_root = env!("CARGO_MANIFEST_DIR");
    for profile in ["debug", "release"] {
        let candidate = PathBuf::from(workspace_root)
            .join("..")
            .join("coincube-spark-bridge")
            .join("target")
            .join(profile)
            .join(exe_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(SparkClientError::BridgeUnavailable(format!(
        "could not locate {} — set COINCUBE_SPARK_BRIDGE_PATH or run `cargo build \
         --manifest-path coincube-spark-bridge/Cargo.toml` first",
        exe_name
    )))
}

// ---------------------------------------------------------------------------
// Background tasks
// ---------------------------------------------------------------------------

fn spawn_writer_task(
    mut stdin: tokio::process::ChildStdin,
    mut request_rx: mpsc::UnboundedReceiver<Request>,
    pending: PendingMap,
    closed: ClosedFlag,
) {
    tokio::spawn(async move {
        while let Some(request) = request_rx.recv().await {
            let frame = Frame::Request(request);
            let line = match serde_json::to_string(&frame) {
                Ok(s) => s,
                Err(e) => {
                    error!("failed to serialize Spark bridge request: {}", e);
                    break;
                }
            };
            if stdin.write_all(line.as_bytes()).await.is_err()
                || stdin.write_all(b"\n").await.is_err()
                || stdin.flush().await.is_err()
            {
                warn!("Spark bridge writer: stdin closed");
                break;
            }
        }

        // Mark client as closed and drain pending requests so callers
        // fail fast instead of waiting for the full timeout.
        closed.store(true, Ordering::SeqCst);
        let mut map = pending.lock().await;
        if !map.is_empty() {
            warn!(
                "Spark bridge writer exited with {} pending request(s) — failing them",
                map.len()
            );
            for (id, sender) in map.drain() {
                let _ = sender.send(Response {
                    id,
                    result: ResponseResult::Err(ErrorPayload {
                        kind: ErrorKind::NotConnected,
                        message: "Spark bridge writer failed — stdin broken or serialization error"
                            .to_string(),
                    }),
                });
            }
        }
    });
}

fn spawn_reader_task(
    stdout: tokio::process::ChildStdout,
    pending: PendingMap,
    event_tx: broadcast::Sender<Event>,
    closed: ClosedFlag,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let frame: Frame = match serde_json::from_str(&line) {
                        Ok(f) => f,
                        Err(e) => {
                            error!(
                                "Spark bridge protocol error — unparseable line: {} ({})",
                                line, e
                            );
                            break;
                        }
                    };
                    match frame {
                        Frame::Response(resp) => {
                            if let Some(sender) = pending.lock().await.remove(&resp.id) {
                                let _ = sender.send(resp);
                            } else {
                                warn!(
                                    "Spark bridge response for unknown id {} — dropping",
                                    resp.id
                                );
                            }
                        }
                        Frame::Event(event) => {
                            debug!("Spark bridge event: {:?}", event);
                            let _ = event_tx.send(event);
                        }
                        Frame::Request(_) => {
                            warn!("Spark bridge sent a Request frame — ignoring");
                        }
                    }
                }
                Ok(None) => {
                    debug!("Spark bridge stdout closed");
                    break;
                }
                Err(e) => {
                    warn!("Spark bridge stdout read error: {}", e);
                    break;
                }
            }
        }

        // Bridge is gone — mark the client as closed so new RPCs
        // fail immediately, then drain any in-flight requests so
        // callers don't hang for the full 30s timeout.
        closed.store(true, Ordering::SeqCst);
        let mut map = pending.lock().await;
        if !map.is_empty() {
            warn!(
                "Spark bridge reader exited with {} pending request(s) — failing them",
                map.len()
            );
            for (id, sender) in map.drain() {
                let _ = sender.send(Response {
                    id,
                    result: coincube_spark_protocol::ResponseResult::Err(
                        coincube_spark_protocol::ErrorPayload {
                            kind: ErrorKind::NotConnected,
                            message: "Spark bridge subprocess exited unexpectedly".to_string(),
                        },
                    ),
                });
            }
        }
    });
}

fn spawn_stderr_task(stderr: tokio::process::ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            warn!(target: "spark_bridge", "{}", line);
        }
    });
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum SparkClientError {
    /// Missing / unparseable config (API key, storage dir).
    Config(String),
    /// Bridge subprocess couldn't be started or died unexpectedly.
    BridgeUnavailable(String),
    /// Bridge returned an error response for a request.
    BridgeError { kind: ErrorKind, message: String },
    /// JSON-RPC framing error (malformed response, unexpected payload).
    Protocol(String),
}

impl std::fmt::Display for SparkClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "Spark config error: {}", msg),
            Self::BridgeUnavailable(msg) => {
                write!(f, "Spark bridge subprocess unavailable: {}", msg)
            }
            Self::BridgeError { kind, message } => {
                write!(f, "Spark bridge returned {:?}: {}", kind, message)
            }
            Self::Protocol(msg) => write!(f, "Spark protocol error: {}", msg),
        }
    }
}

impl std::error::Error for SparkClientError {}

// ---------------------------------------------------------------------------
// Iced subscription for bridge events
// ---------------------------------------------------------------------------

/// Domain wrapper around [`Event`] so the app-level [`crate::app::Message`]
/// doesn't need to depend on the protocol crate directly.
///
/// Phase 4d just forwards the protocol variant as-is (zero translation
/// cost). Phase 4e / 5 can promote this to a typed enum if panels
/// start branching on event-specific data.
#[derive(Debug, Clone)]
pub struct SparkClientEvent(pub Event);

/// Subscription identity — hashes on the broadcast sender's pointer
/// so a fresh `SparkClient` (e.g. after the user re-unlocks a cube)
/// produces a brand-new subscription instead of reusing the old one.
struct SparkEventSubscriptionState {
    client: SparkClient,
}

impl std::hash::Hash for SparkEventSubscriptionState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let ptr = Arc::as_ptr(&self.client.inner) as usize;
        ptr.hash(state);
    }
}

/// Build the iced [`Stream`](iced::futures::Stream) that drains the
/// broadcast channel into iced's runtime. Uses `iced::stream::channel`
/// with a 100-slot buffer mirroring the Liquid subscription pattern.
fn make_spark_event_stream(
    state: &SparkEventSubscriptionState,
) -> impl iced::futures::Stream<Item = SparkClientEvent> {
    let client = state.client.clone();
    iced::stream::channel(
        100,
        move |mut output: iced::futures::channel::mpsc::Sender<SparkClientEvent>| async move {
            let mut receiver = client.subscribe_events();
            loop {
                match receiver.recv().await {
                    Ok(event) => {
                        use iced::futures::SinkExt;
                        if output.send(SparkClientEvent(event)).await.is_err() {
                            // iced runtime dropped the sink — time to
                            // stop pumping events for this subscription.
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!(
                            "Spark event subscription lagged by {} events, resuming",
                            skipped
                        );
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Sender dropped — the SparkClient is gone.
                        // Park the task forever so iced keeps the
                        // Subscription id alive until the parent
                        // re-instantiates the state.
                        std::future::pending::<()>().await;
                        break;
                    }
                }
            }
            std::future::pending::<()>().await;
        },
    )
}
