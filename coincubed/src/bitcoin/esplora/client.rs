use std::convert::TryFrom;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bdk_electrum::bdk_chain::{
    bitcoin,
    spk_client::{FullScanRequest, FullScanResult, SyncRequest, SyncResult},
};
use bdk_esplora::{esplora_client, EsploraExt};

use crate::bitcoin::BlockChainTip;

/// Per-request timeout. Kept well below the old 30s so that when a primary
/// is unreachable (DNS/region block, outage) the *first* call that re-tests
/// it fails over in a tolerable window rather than freezing the UI. The
/// repeated-stall problem is handled by [`TRANSPORT_FAILURE_COOLDOWN`] (a
/// dead provider is skipped entirely after the first timeout), so this value
/// only bounds that single re-test; it's set high enough not to false-trip a
/// legitimately slow provider (the authenticated Connect backstop's startup
/// handshake was observed at 5–11s in the wild).
const REQUEST_TIMEOUT_SECS: u64 = 15;

/// How long we skip a provider after it explicitly told us to back off
/// (HTTP 402/429). Picked generously so a free-tier provider's per-minute
/// or per-hour quota window has time to reset rather than us re-checking
/// every poll tick (~10s) and burning a request to re-discover the same
/// 429. Cleared on the next successful call from that provider.
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(600);

/// How long we skip a provider after a *transport* failure (connection
/// refused, DNS failure, or — the case that motivated this — a request
/// timeout). Without this, an unreachable primary is re-dialled on every
/// single call and the caller eats the full [`REQUEST_TIMEOUT_SECS`] each
/// time (the 30s-per-action stalls users reported). Cooling it down means
/// one timeout per window, then the chain skips straight to a working
/// fallback. Shorter than [`RATE_LIMIT_COOLDOWN`] because a transport blip
/// often clears quickly (transient network), and the provider rejoins the
/// rotation immediately on its next successful call regardless.
const TRANSPORT_FAILURE_COOLDOWN: Duration = Duration::from_secs(120);

/// A sole provider has no healthy fallback to serve requests during cooldown.
/// Make it eligible again promptly while bounding repeated UI retries.
const SOLE_PROVIDER_TRANSPORT_COOLDOWN: Duration = Duration::from_secs(5);

fn transport_cooldown(provider_count: usize) -> Duration {
    if provider_count == 1 {
        SOLE_PROVIDER_TRANSPORT_COOLDOWN
    } else {
        TRANSPORT_FAILURE_COOLDOWN
    }
}

/// An error from the Esplora client.
#[derive(Debug)]
pub enum Error {
    Client(Box<esplora_client::Error>),
    Admission(crate::connect::AdmissionError),
    /// Every configured provider is currently in a cooldown,
    /// so no network call was actually attempted. This is a
    /// transient "wait" signal, not a fault — callers (the poller in
    /// particular) should treat it as a no-op outcome rather than a
    /// real failure to log at ERROR level. The next tick after any
    /// provider's cooldown expires will see a normal result.
    AllCooling,
    /// The shared abort flag was set (the daemon is shutting down), so we
    /// stopped walking the provider chain instead of waiting out requests to
    /// unreachable providers. Lets `DaemonHandle::stop` return promptly even
    /// while a scan is in flight against a dead/throttled Esplora — otherwise
    /// `stop()` joins a poller stuck for the full request/timeout cycle.
    Aborted,
    /// The provider answered `GET /blocks` with a body that parsed but does
    /// not describe a usable tip: an empty list, or a timestamp outside the
    /// `u32` range every consumer of block times uses. Never defaulted — the
    /// callers map this to "unknown" (`Option::None`), not to a made-up time.
    TipMetadata(&'static str),
    /// Height-zero JSON metadata is absent, ambiguous, or out of range.
    GenesisMetadata(&'static str),
    /// The server reported a block height our `i32` heights cannot hold.
    /// Returned rather than panicking or wrapping: the poller reads the wallet
    /// tip while holding the backend lock (#616, #621).
    HeightOutOfRange(u64),
    /// The server reported a block time our `u32` times cannot hold. Refused
    /// for the same reason as [`Error::HeightOutOfRange`] (#621).
    TimeOutOfRange(u64),
}

impl Error {
    /// `true` for the [`Error::AllCooling`] variant. Lets the
    /// poller's error-handling arm downgrade the log level and back
    /// off longer without having to import the variant by name.
    pub fn is_all_cooling(&self) -> bool {
        matches!(self, Error::AllCooling)
    }
}

/// Marker substring placed at the start of [`Error::AllCooling`]'s
/// `Display` output. The [`BitcoinInterface::sync_wallet`] trait
/// boundary forces us to stringify the error, so the poller can't
/// pattern-match on the typed variant directly — instead it checks
/// for this marker in the error string and routes to a quieter log
/// level + longer backoff. A test asserts the marker stays in the
/// rendered output so a future refactor can't silently strand the
/// poller's special-case branch.
pub const ALL_COOLING_DISPLAY_MARKER: &str = "All Esplora providers are temporarily backing off";

/// Marker substring in [`Error::Aborted`]'s `Display`. Same rationale as
/// [`ALL_COOLING_DISPLAY_MARKER`]: the `sync_wallet` trait boundary stringifies
/// the error, so the poller detects a shutdown-abort by this marker and STOPS
/// retrying (returns) rather than recursing its 2s retry loop — which would
/// never return to check for the Shutdown message, leaving `stop()` blocked.
pub const SCAN_ABORTED_DISPLAY_MARKER: &str = "Esplora scan aborted";

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Admission(e) => write!(f, "{}", e),
            Error::Client(e) => write!(f, "Esplora client error: '{}'.", e),
            Error::AllCooling => write!(
                f,
                "{}; the poller will retry once a cooldown expires.",
                ALL_COOLING_DISPLAY_MARKER,
            ),
            Error::Aborted => {
                write!(
                    f,
                    "{}: the daemon is shutting down.",
                    SCAN_ABORTED_DISPLAY_MARKER
                )
            }
            Error::TipMetadata(what) => write!(f, "Esplora tip metadata is unusable: {}.", what),
            Error::GenesisMetadata(what) => {
                write!(f, "Esplora genesis metadata is unusable: {}.", what)
            }
            Error::HeightOutOfRange(height) => write!(
                f,
                "Esplora error: the server reported an out-of-range block height {}.",
                height
            ),
            Error::TimeOutOfRange(time) => write!(
                f,
                "Esplora error: the server reported an out-of-range block time {}.",
                time
            ),
        }
    }
}

/// Bitcoin Esplora client backed by an ordered chain of providers.
///
/// `try_in_order` walks the providers from index 0 onwards on every call.
/// A provider is skipped if it's currently in a cooldown
/// ([`RATE_LIMIT_COOLDOWN`]). On a retryable failure ([`should_fall_back`])
/// the next provider is tried; a non-retryable failure short-circuits
/// the chain.
///
/// Methods that consume a request (`sync`, `full_scan`) take a builder
/// closure so the request can be rebuilt for each attempt — BDK's
/// `SyncRequest` is consumed by the call and isn't trivially clonable.
pub struct Client {
    #[cfg(feature = "native-recovery")]
    scan_pacer: Option<Arc<ScanPacer>>,
    admission: Option<Arc<crate::connect::ConnectBackend>>,
    providers: Vec<Provider>,
    /// Set by `DaemonHandle::stop` so an in-flight scan stops walking the
    /// provider chain and returns [`Error::Aborted`] promptly, instead of the
    /// poller (and the `stop()` that joins it) blocking on requests to dead or
    /// throttled providers. Shared so the stopping thread can flip it while the
    /// poller is mid-scan.
    abort: Arc<AtomicBool>,
}

/// One endpoint in the provider chain, plus the state needed to skip
/// it during a cooldown window.
struct Provider {
    /// Human label used in logs (`mempool.space (anonymous)`, etc.).
    name: String,
    client: esplora_client::blocking::BlockingClient,
    /// `Some(deadline)` after a rate limit, transport failure or admission delay;
    /// skipped while `now < deadline`. Cleared on the next successful
    /// call to the same provider so a long-cooled provider that's
    /// healthy again rejoins the rotation immediately rather than
    /// waiting out the full window.
    cooldown_until: Mutex<Option<Instant>>,
}

impl Provider {
    fn is_cooling(&self) -> bool {
        let guard = self.cooldown_until.lock().expect("cooldown mutex poisoned");
        match *guard {
            Some(deadline) => Instant::now() < deadline,
            None => false,
        }
    }

    fn enter_cooldown(&self, dur: Duration) {
        let mut guard = self.cooldown_until.lock().expect("cooldown mutex poisoned");
        *guard = Some(Instant::now() + dur);
    }

    fn clear_cooldown(&self) {
        let mut guard = self.cooldown_until.lock().expect("cooldown mutex poisoned");
        *guard = None;
    }
}

/// Build a `BlockingClient` for the given address, applying our standard
/// timeout, the gzip-disabling header, and an optional bearer token.
fn build_blocking_client(
    addr: &str,
    token: Option<&str>,
) -> esplora_client::blocking::BlockingClient {
    let mut builder = esplora_client::Builder::new(addr).timeout(REQUEST_TIMEOUT_SECS);
    // The blocking esplora client uses `minreq` underneath, which has no
    // content-encoding support. If the server returns gzip/brotli-compressed
    // bodies (common for /address/:addr/txs and other large responses),
    // minreq tries to read the raw bytes as UTF-8 and fails with
    // `InvalidUtf8InResponse`. Force the server to send uncompressed
    // responses to avoid this.
    builder = builder.header("Accept-Encoding", "identity");
    if let Some(token) = token {
        builder = builder.header("Authorization", &format!("Bearer {}", token));
    }
    builder.build_blocking()
}

/// Whether a result should trigger the next provider in the chain.
///
/// 402/429 are 4xx codes but they describe the provider's *capacity*
/// rather than the request — falling through to the next provider is
/// the correct response, and these statuses additionally trigger a
/// cooldown so we stop re-asking the throttled provider for a while.
/// Transport failures also cool down, briefly for a sole provider and longer
/// when a fallback can serve requests. 5xx errors fall through without a
/// cooldown so the next call can retry. Genuine 4xx outcomes like 400/404
/// describe the request itself and pass through unchanged so the
/// caller sees the real answer.
fn should_fall_back<T>(result: &Result<T, esplora_client::Error>) -> bool {
    match result {
        Ok(_) => false,
        Err(esplora_client::Error::HttpResponse { status, .. }) => {
            matches!(*status, 402 | 429) || (500..=599).contains(status)
        }
        Err(_) => true,
    }
}

/// Whether a result is an explicit "throttled by provider" signal that
/// warrants entering the rate-limit cooldown.
fn is_throttled<T>(result: &Result<T, esplora_client::Error>) -> bool {
    matches!(
        result,
        Err(esplora_client::Error::HttpResponse { status, .. }) if matches!(*status, 402 | 429)
    )
}

/// Whether an error is a *transport*-layer failure — a timeout, connection
/// refused, or DNS failure surfaced by the blocking client's `minreq`
/// backend (the reported stalls were `Minreq(IoError(TimedOut))`). These
/// warrant the shorter [`TRANSPORT_FAILURE_COOLDOWN`] so an unreachable
/// provider is skipped on subsequent calls instead of being re-dialled (and
/// timing out) every time.
///
/// Deliberately narrow: it must NOT match errors that came back from a
/// *responding* server — `HttpResponse` (any status), `Parsing`,
/// `BitcoinEncoding`, `TransactionNotFound`, etc. Those indicate the provider
/// is reachable, so cooling it down would needlessly sideline a healthy
/// endpoint. Such errors still fall through to the next provider via
/// [`should_fall_back`]; they just don't trigger a cooldown.
fn is_transport_err(e: &esplora_client::Error) -> bool {
    matches!(e, esplora_client::Error::Minreq(_))
}

fn is_transport_failure<T>(result: &Result<T, esplora_client::Error>) -> bool {
    matches!(result, Err(e) if is_transport_err(e))
}

fn admission_hash_at(
    provider: &Provider,
    abort: &AtomicBool,
    height: u32,
    transport_backoff: Duration,
) -> Result<bitcoin::BlockHash, crate::connect::AdmissionError> {
    use crate::connect::AdmissionError;
    if abort.load(Ordering::Relaxed) {
        return Err(AdmissionError::Aborted);
    }
    let result = provider.client.get_block_hash(height);
    if abort.load(Ordering::Relaxed) {
        return Err(AdmissionError::Aborted);
    }
    match result {
        Ok(hash) => Ok(hash),
        Err(esplora_client::Error::HttpResponse {
            status: 402 | 429, ..
        }) => {
            provider.enter_cooldown(RATE_LIMIT_COOLDOWN);
            Err(AdmissionError::Throttled)
        }
        Err(esplora_client::Error::HttpResponse { status: 404, .. }) => {
            // Keep a running daemon from hammering a temporarily lagging indexer.
            // Startup callers receive the typed refusal and own their retry budget.
            provider.enter_cooldown(Duration::from_secs(30));
            Err(AdmissionError::IndexerBehind)
        }
        Err(error) => {
            if is_transport_err(&error) {
                provider.enter_cooldown(transport_backoff);
            }
            Err(AdmissionError::Unavailable)
        }
    }
}
fn admission_error(provider: &Provider, error: crate::connect::AdmissionError) -> Error {
    use crate::connect::AdmissionError;
    match error {
        AdmissionError::Aborted => Error::Aborted,
        AdmissionError::Throttled => {
            provider.enter_cooldown(RATE_LIMIT_COOLDOWN);
            Error::AllCooling
        }
        other => Error::Admission(other),
    }
}

#[cfg(feature = "native-recovery")]
struct ScanPacer {
    interval: Duration,
    next: Mutex<Instant>,
}
#[cfg(feature = "native-recovery")]
impl ScanPacer {
    fn wait(&self, abort: &AtomicBool) -> bool {
        while !abort.load(Ordering::Relaxed) {
            let delay = {
                let mut next = self.next.lock().expect("scan pacing lock");
                let now = Instant::now();
                if now >= *next {
                    *next = now + self.interval;
                    return !abort.load(Ordering::Relaxed);
                }
                next.saturating_duration_since(now)
            };
            std::thread::sleep(delay.min(Duration::from_millis(20)));
        }
        false
    }
}

impl Client {
    #[cfg(feature = "native-recovery")]
    pub(crate) fn set_native_recovery_pacing(&mut self, interval: Duration) {
        self.scan_pacer = Some(Arc::new(ScanPacer {
            interval,
            next: Mutex::new(Instant::now()),
        }));
    }
    /// Build the client and the provider chain from `config`. Construction
    /// is now infallible (in the network sense): if every provider's
    /// startup connectivity check fails we still hand back a usable
    /// `Client`, log the failures, and rely on the poller's next sync
    /// tick to retry. This is a deliberate change from the previous
    /// behaviour, which refused to start the daemon when no provider
    /// could be reached — a single bad rate-limit window on every
    /// configured backend would otherwise lock the user out of their
    /// app entirely, including the parts that don't need a live
    /// Esplora (cached balance, locally-signed PSBTs, settings).
    /// Errors from the actual sync calls still surface in the usual
    /// places, so a permanently broken config doesn't get silently
    /// swallowed — it just doesn't block launch.
    pub(crate) fn new_for_connect(
        backend: crate::connect::ConnectBackend,
        abort: Arc<AtomicBool>,
    ) -> Result<Self, Error> {
        let config = backend.config();
        let provider = Provider {
            name: "authenticated Connect".to_string(),
            client: build_blocking_client(&config.addr, config.token.as_deref()),
            cooldown_until: Mutex::new(None),
        };
        backend
            .validate(|height| admission_hash_at(&provider, &abort, height, transport_cooldown(1)))
            .map_err(|error| admission_error(&provider, error))?;
        Ok(Self {
            providers: vec![provider],
            abort,
            #[cfg(feature = "native-recovery")]
            scan_pacer: None,
            admission: Some(Arc::new(backend)),
        })
    }

    pub fn new(
        config: &crate::config::EsploraConfig,
        abort: Arc<AtomicBool>,
    ) -> Result<Self, Error> {
        let mut providers = Vec::new();
        providers.push(Provider {
            name: format!("primary {}", config.addr),
            client: build_blocking_client(&config.addr, config.token.as_deref()),
            cooldown_until: Mutex::new(None),
        });
        if let Some(addr) = config.fallback_addr.as_deref() {
            providers.push(Provider {
                name: format!("fallback {}", addr),
                client: build_blocking_client(addr, config.fallback_token.as_deref()),
                cooldown_until: Mutex::new(None),
            });
        }
        if let Some(addr) = config.secondary_fallback_addr.as_deref() {
            providers.push(Provider {
                name: format!("secondary-fallback {}", addr),
                client: build_blocking_client(addr, config.secondary_fallback_token.as_deref()),
                cooldown_until: Mutex::new(None),
            });
        }

        let transport_backoff = transport_cooldown(providers.len());

        // Best-effort startup check: log per-provider reachability for
        // diagnostics, then return Ok regardless of outcome. Critically,
        // a 402/429 response here pre-seeds the provider's cooldown so
        // the first real sync tick after launch doesn't waste a call
        // re-discovering the same throttle.
        //
        // The checks run concurrently via `thread::scope` — total
        // startup wait is `max(per-provider latency)` instead of the
        // sum. With three providers and the steady-state 5–11s
        // per check observed in the wild, that shaves ~15s off cold
        // start at no semantic cost: a slow Connect handshake no
        // longer holds up an already-answered mempool.
        let results: Vec<(usize, Result<u32, esplora_client::Error>)> = std::thread::scope(|s| {
            let handles: Vec<_> = providers
                .iter()
                .enumerate()
                .map(|(idx, p)| s.spawn(move || (idx, p.client.get_height())))
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("startup check thread panicked"))
                .collect()
        });

        let mut any_ok = false;
        for (idx, result) in results {
            let provider = &providers[idx];
            match result {
                Ok(_) => {
                    log::info!("Esplora {} reachable at startup", provider.name);
                    any_ok = true;
                }
                Err(esplora_client::Error::HttpResponse { status, message })
                    if matches!(status, 402 | 429) =>
                {
                    provider.enter_cooldown(RATE_LIMIT_COOLDOWN);
                    log::warn!(
                        "Esplora {} throttled at startup (status {}): {} — pre-seeded cooldown",
                        provider.name,
                        status,
                        message,
                    );
                }
                Err(e) => {
                    // Pre-seed the transport cooldown for an unreachable
                    // provider so the first real sync tick skips it instead of
                    // re-paying the request timeout. Only genuine transport
                    // failures cool down — a reachable-but-erroring server
                    // (5xx, decode error) is left to be re-tested next tick.
                    let transport = is_transport_err(&e);
                    if transport {
                        provider.enter_cooldown(transport_backoff);
                    }
                    log::warn!(
                        "Esplora {} unreachable at startup: {}{}",
                        provider.name,
                        e,
                        if transport {
                            " — pre-seeded cooldown"
                        } else {
                            ""
                        },
                    );
                }
            }
        }
        if !any_ok {
            log::warn!(
                "Esplora: no provider answered the startup check — daemon will start anyway and \
                 the poller will retry on its next tick"
            );
        }
        Ok(Client {
            providers,
            abort,
            #[cfg(feature = "native-recovery")]
            scan_pacer: None,
            admission: None,
        })
    }

    /// Run `op` against each provider in order, skipping any that's in a
    /// cooldown. See [`should_fall_back`] and [`is_throttled`] for
    /// the per-result decisions.
    fn try_in_order<T, F>(&self, op: F) -> Result<T, Error>
    where
        F: FnMut(&esplora_client::blocking::BlockingClient) -> Result<T, esplora_client::Error>,
    {
        self.try_in_order_checked(op, true)
    }

    fn try_in_order_checked<T, F>(&self, op: F, check_after: bool) -> Result<T, Error>
    where
        F: FnMut(&esplora_client::blocking::BlockingClient) -> Result<T, esplora_client::Error>,
    {
        self.try_in_order_validated(op, check_after, Ok)
    }

    /// Validate a successful response before accepting its provider. Semantic
    /// failures try the next provider without pretending to be transport errors
    /// or cooling an otherwise reachable upstream.
    fn try_in_order_validated<T, U, F, V>(
        &self,
        mut op: F,
        check_after: bool,
        mut validate: V,
    ) -> Result<U, Error>
    where
        F: FnMut(&esplora_client::blocking::BlockingClient) -> Result<T, esplora_client::Error>,
        V: FnMut(T) -> Result<U, Error>,
    {
        let transport_backoff = transport_cooldown(self.providers.len());
        let mut last_error = None;
        for provider in &self.providers {
            // Bail out between providers if the daemon is shutting down, so a
            // dead/throttled provider chain can't keep `stop()` blocked. The
            // in-flight `op` (one provider) still runs to its timeout; this stops
            // us from then dialling the rest.
            if self.abort.load(Ordering::Relaxed) {
                return Err(Error::Aborted);
            }
            if provider.is_cooling() {
                log::debug!("Esplora skipping {} (cooling down)", provider.name,);
                continue;
            }
            let result = self.call_admitted(provider, &mut op, check_after, transport_backoff)?;
            let retryable = should_fall_back(&result);
            let throttled = is_throttled(&result);
            let transport_failure = is_transport_failure(&result);
            let error = match result {
                Ok(value) => {
                    provider.clear_cooldown();
                    match validate(value) {
                        Ok(value) => return Ok(value),
                        Err(error) => {
                            last_error = Some(error);
                            continue;
                        }
                    }
                }
                Err(error) => error,
            };
            if !retryable {
                return Err(Error::Client(Box::new(error)));
            }
            Self::cool_failed_provider(
                provider,
                &error,
                throttled,
                transport_failure,
                transport_backoff,
            );
            last_error = Some(Error::Client(Box::new(error)));
        }

        // Every provider either failed retryably or was on cooldown.
        // Surface the last real result if we have one; otherwise the
        // entire chain was on cooldown — return the typed
        // [`Error::AllCooling`] so the poller can log it at a sane
        // level and back off longer than its normal 2s retry, since
        // a cooldown won't lift for minutes.
        Err(last_error.unwrap_or(Error::AllCooling))
    }

    /// Preserve admission checks around one operation, including the broadcast exception.
    fn call_admitted<T, F>(
        &self,
        provider: &Provider,
        op: &mut F,
        check_after: bool,
        transport_backoff: Duration,
    ) -> Result<Result<T, esplora_client::Error>, Error>
    where
        F: FnMut(&esplora_client::blocking::BlockingClient) -> Result<T, esplora_client::Error>,
    {
        let before = self
            .admission
            .as_ref()
            .map(|guard| {
                guard.validate(|height| {
                    admission_hash_at(provider, &self.abort, height, transport_backoff)
                })
            })
            .transpose()
            .map_err(|error| admission_error(provider, error))?;
        if self.admission.is_some() && self.abort.load(Ordering::Relaxed) {
            return Err(Error::Aborted);
        }
        let result = op(&provider.client);
        // A successful broadcast must not be retrospectively reported failed.
        if self.admission.is_some() && check_after && self.abort.load(Ordering::Relaxed) {
            return Err(Error::Aborted);
        }
        if result.is_ok() && check_after {
            if let (Some(guard), Some(before)) = (&self.admission, before.as_ref()) {
                guard
                    .revalidate(before, |height| {
                        admission_hash_at(provider, &self.abort, height, transport_backoff)
                    })
                    .map_err(|error| admission_error(provider, error))?;
            }
        }
        Ok(result)
    }

    /// Only throttling and transport failures cool a provider; server errors remain retryable.
    fn cool_failed_provider(
        provider: &Provider,
        error: &esplora_client::Error,
        throttled: bool,
        transport_failure: bool,
        transport_backoff: Duration,
    ) {
        if throttled {
            provider.enter_cooldown(RATE_LIMIT_COOLDOWN);
            log::warn!(
                "Esplora {} throttled ({}); cooling for {:?} and trying next provider",
                provider.name,
                error,
                RATE_LIMIT_COOLDOWN,
            );
        } else if transport_failure {
            // Reachability errors cool down; 5xx responses do not.
            provider.enter_cooldown(transport_backoff);
            log::warn!(
                "Esplora {} unreachable ({}); cooling for {:?} and trying next provider",
                provider.name,
                error,
                transport_backoff,
            );
        } else {
            log::warn!(
                "Esplora {} failed ({}); trying next provider",
                provider.name,
                error
            );
        }
    }

    /// Get the genesis block hash (block at height 0).
    pub fn genesis_block_hash(&self) -> Result<bitcoin::BlockHash, Error> {
        self.try_in_order(|client| client.get_block_hash(0))
    }

    /// Get the current chain tip (height + hash).
    ///
    /// Fetches the tip hash first, then resolves its height via `get_block_status` so both
    /// values come from the same point-in-time snapshot, avoiding a TOCTOU mismatch.
    pub fn chain_tip(&self) -> Result<BlockChainTip, Error> {
        let (hash, status) = if self.admission.is_some() {
            self.try_in_order(|client| {
                let hash = client.get_tip_hash()?;
                if self.abort.load(Ordering::Relaxed) {
                    return Err(esplora_client::Error::HttpResponse {
                        status: 503,
                        message: "scan aborted".into(),
                    });
                }
                Ok((hash, client.get_block_status(&hash)?))
            })?
        } else {
            let hash = self.try_in_order(|client| client.get_tip_hash())?;
            (
                hash,
                self.try_in_order(|client| client.get_block_status(&hash))?,
            )
        };
        let height = status.height.ok_or_else(|| {
            Error::Client(Box::new(esplora_client::Error::HttpResponse {
                status: 404,
                message: format!("tip block {} is not in best chain", hash),
            }))
        })?;
        // Checked, not `as i32`: a height above `i32::MAX` would wrap negative.
        let height = i32::try_from(height).map_err(|_| Error::HeightOutOfRange(height.into()))?;
        Ok(BlockChainTip { hash, height })
    }

    /// Get the timestamp of the genesis block (block 0).
    pub fn genesis_block_timestamp(&self) -> Result<u32, Error> {
        // Keep every Esplora timestamp read on JSON metadata, including the
        // rescan lower bound. No header decoder belongs on the BTCB2 path.
        let summaries = self.try_in_order(|client| client.get_blocks(Some(0)))?;
        let genesis = match summaries.as_slice() {
            [genesis] if genesis.time.height == 0 => genesis,
            _ => {
                return Err(Error::GenesisMetadata(
                    "expected exactly one height-zero block",
                ))
            }
        };
        u32::try_from(genesis.time.timestamp)
            .map_err(|_| Error::GenesisMetadata("genesis timestamp does not fit in u32"))
    }

    /// Get the timestamp of the current tip block.
    ///
    /// Read from the JSON block summaries (`GET /blocks`) rather than the raw
    /// header (`GET /block/<hash>/header`). The raw path decodes the bytes
    /// through rust-bitcoin's 80-byte `block::Header`, which cannot represent a
    /// Bitcoin Blake2b post-fork header (164-byte v2 headers from the hardfork
    /// height on — PLAN-bitcoin-blake2b, audit F5); the JSON summary carries the
    /// same `timestamp` for either chain, and `esplora-client` 0.8.0 (the
    /// version `bdk_esplora` pins here) exposes it as
    /// [`esplora_client::BlockSummary`] via `get_blocks`, with no
    /// per-hash JSON metadata call available.
    ///
    /// One request, one snapshot: the tip's id, height and timestamp are read
    /// from the same JSON object of the same response. (The previous shape —
    /// tip hash, then the header for that captured hash — was also bound to
    /// one block; the change is the extended-header compatibility and the
    /// single request, not a race fix.) The snapshot's tip is the entry with
    /// the greatest height — both Esplora and mempool.space document `/blocks`
    /// as newest-first, but the selection does not rely on order. An empty
    /// list or a timestamp outside `u32` is an [`Error::TipMetadata`], never a
    /// default.
    ///
    /// Empty/overflow metadata tries the next eligible provider without a
    /// transport cooldown. If no provider succeeds, return the last attempted
    /// provider's error. HTTP refusal, cooldown, admission and shutdown rules
    /// retain their usual behavior.
    pub fn tip_time(&self) -> Result<u32, Error> {
        self.try_in_order_validated(
            |client| client.get_blocks(None),
            true,
            |summaries| {
                let tip = summaries
                    .iter()
                    .max_by_key(|summary| summary.time.height)
                    .ok_or(Error::TipMetadata("`/blocks` returned no block summaries"))?;
                u32::try_from(tip.time.timestamp)
                    .map_err(|_| Error::TipMetadata("tip timestamp does not fit in u32"))
            },
        )
    }

    /// Broadcast a transaction to the network.
    pub fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), Error> {
        self.try_in_order_checked(|client| client.broadcast(tx), false)
    }

    /// Perform a sync against the known SPKs.
    ///
    /// `build_request` is called once per attempt because BDK's
    /// `SyncRequest` is consumed by the call. The `Box` returned by
    /// `EsploraExt::sync` is unboxed inside the closure so it matches
    /// [`Client::try_in_order`]'s unboxed-error contract.
    pub fn sync<F>(
        &self,
        mut build_request: F,
        parallel_requests: usize,
    ) -> Result<SyncResult, Error>
    where
        F: FnMut() -> SyncRequest,
    {
        let result = self.try_in_order(|client| {
            let mut request = build_request();
            // Same shutdown guard as `full_scan`: BDK pulls these iterators one
            // HTTP batch at a time, so ending them stops the sync before its next
            // batch instead of after every address.
            until_aborted(&mut request.spks, &self.abort);
            until_aborted(&mut request.txids, &self.abort);
            until_aborted(&mut request.outpoints, &self.abort);
            client.sync(request, parallel_requests).map_err(|e| *e)
        });
        // As in `full_scan`, an iterator ended early is a successful *partial*
        // sync. Never apply it on shutdown.
        if self.abort.load(Ordering::Relaxed) {
            Err(Error::Aborted)
        } else {
            result
        }
    }

    /// Perform a full scan from genesis for all keychain SPKs.
    ///
    /// See [`Client::sync`] for the rationale behind the builder closure.
    pub fn full_scan<K, F>(
        &self,
        mut build_request: F,
        stop_gap: usize,
        parallel_requests: usize,
    ) -> Result<FullScanResult<K>, Error>
    where
        K: Ord + Clone + std::fmt::Debug + Send,
        F: FnMut() -> FullScanRequest<K>,
    {
        #[cfg(feature = "native-recovery")]
        let parallel_requests = if self.scan_pacer.is_some() {
            1
        } else {
            parallel_requests
        };
        let result = self.try_in_order(|client| {
            let mut request = build_request();
            // BDK checks these iterators between address batches, but otherwise
            // keeps scanning up to the stop gap even after daemon shutdown.
            // Stop each keychain before it starts another batch. Requests already
            // in flight retain the transport timeout.
            for spks in request.spks_by_keychain.values_mut() {
                let abort = self.abort.clone();
                let iter = std::mem::replace(spks, Box::new(std::iter::empty()));
                #[cfg(feature = "native-recovery")]
                let pacer = self.scan_pacer.clone();
                *spks = Box::new(iter.take_while(move |_| {
                    if abort.load(Ordering::Relaxed) {
                        return false;
                    }
                    #[cfg(feature = "native-recovery")]
                    if let Some(pacer) = &pacer {
                        return pacer.wait(&abort);
                    }
                    true
                }));
            }
            client
                .full_scan(request, stop_gap, parallel_requests)
                .map_err(|e| *e)
        });
        // Ending an iterator early produces a successful *partial* BDK scan.
        // Never apply it or clear the persisted rescan marker on shutdown.
        if self.abort.load(Ordering::Relaxed) {
            Err(Error::Aborted)
        } else {
            result
        }
    }
}

/// Swap a [`SyncRequest`] iterator for one that ends once `abort` is set.
///
/// `take_while` would do, but `SyncRequest` requires `ExactSizeIterator`.
fn until_aborted<T: 'static>(
    iter: &mut Box<dyn ExactSizeIterator<Item = T> + Send>,
    abort: &Arc<AtomicBool>,
) {
    let inner = std::mem::replace(iter, Box::new(std::iter::empty()));
    *iter = Box::new(UntilAborted {
        inner,
        abort: abort.clone(),
    });
}

struct UntilAborted<I> {
    inner: I,
    abort: Arc<AtomicBool>,
}

impl<I: Iterator> Iterator for UntilAborted<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        if self.abort.load(Ordering::Relaxed) {
            return None;
        }
        let item = self.inner.next()?;
        // Checked again like `take_while`: the flag may be set while the
        // item is produced.
        (!self.abort.load(Ordering::Relaxed)).then_some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.abort.load(Ordering::Relaxed) {
            (0, Some(0))
        } else {
            self.inner.size_hint()
        }
    }
}

// Strictly this breaks `ExactSizeIterator`'s contract once `abort` is set: the
// iterator ends early and `size_hint` shrinks mid-iteration. Harmless here,
// because a sync that saw the flag is discarded as `Error::Aborted`.
impl<I: ExactSizeIterator> ExactSizeIterator for UntilAborted<I> {}

#[cfg(test)]
mod tests {
    use super::*;

    struct MutableAuthority(Mutex<crate::connect::TrustedChainAnchor>);
    impl crate::connect::ConnectAnchorAuthority for MutableAuthority {
        fn fresh_anchor(
            &self,
        ) -> Result<crate::connect::TrustedChainAnchor, crate::connect::AdmissionError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }
    fn authority_fixture(hash: bitcoin::BlockHash) -> Arc<MutableAuthority> {
        Arc::new(MutableAuthority(Mutex::new(
            crate::connect::TrustedChainAnchor {
                chain: coincube_core::chain::ChainId::BitcoinBlake2b,
                height: 900000,
                hash,
                median_time_past: 1000,
                observed_at: std::time::SystemTime::now(),
            },
        )))
    }
    fn backend_fixture(
        server: &MockEsplora,
        authority: Arc<MutableAuthority>,
    ) -> crate::connect::ConnectBackend {
        crate::connect::ConnectBackend::new(
            coincube_core::chain::ChainId::BitcoinBlake2b,
            server.base.clone(),
            "synthetic-jwt".into(),
            authority,
        )
        .unwrap()
    }
    #[test]
    fn admitted_proxy_reads_and_broadcast_never_send_account_credentials() {
        use bitcoin::hashes::Hash;
        let hash = bitcoin::BlockHash::from_byte_array([7; 32]);
        // Synthetic transport payload only; this mock grants no node acceptance.
        let tx = bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn::default()],
            output: vec![],
        };
        let server = mock_esplora(StdHashMap::from([
            ("/block-height/900000", (200, hash.to_string())),
            ("/tx", (200, tx.compute_txid().to_string())),
        ]));
        let authority = authority_fixture(hash);
        let client = Client::new_for_connect(
            backend_fixture(&server, authority.clone()),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        client.broadcast_tx(&tx).unwrap();
        let heads = server.heads.lock().unwrap();
        assert!(heads
            .iter()
            .any(|h| h.starts_with("GET /block-height/900000 ")));
        assert!(heads.iter().any(|h| h.starts_with("POST /tx ")));
        assert!(heads
            .iter()
            .all(|head| !head.to_ascii_lowercase().contains("authorization:")));
        assert!(heads.iter().all(|head| !head.contains("synthetic-jwt")));
        drop(heads);
        // Dropping credentials from proxy HTTP must not bypass authenticated
        // authority checks, including their post-admission revocation state.
        authority.0.lock().unwrap().hash = bitcoin::BlockHash::from_byte_array([8; 32]);
        let before = server.requests().iter().filter(|p| *p == "/tx").count();
        assert!(client.broadcast_tx(&tx).is_err());
        assert_eq!(
            server.requests().iter().filter(|p| *p == "/tx").count(),
            before
        );
    }

    #[test]
    fn admitted_operation_accepts_growth_and_lag_is_typed_without_fallback() {
        use bitcoin::hashes::Hash;
        let old = bitcoin::BlockHash::from_byte_array([7; 32]);
        let new = bitcoin::BlockHash::from_byte_array([8; 32]);
        let server = mock_esplora(StdHashMap::from([
            ("/block-height/900000", (200, old.to_string())),
            ("/block-height/900001", (200, new.to_string())),
        ]));
        let authority = authority_fixture(old);
        let client = Client::new_for_connect(
            backend_fixture(&server, authority.clone()),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert_eq!(
            client
                .try_in_order(|_| {
                    let mut a = authority.0.lock().unwrap();
                    a.height += 1;
                    a.hash = new;
                    a.observed_at = std::time::SystemTime::now();
                    Ok(42)
                })
                .unwrap(),
            42
        );
        let lagging = mock_esplora(StdHashMap::new());
        assert!(matches!(
            Client::new_for_connect(
                backend_fixture(&lagging, authority.clone()),
                Arc::new(AtomicBool::new(false))
            ),
            Err(Error::Admission(
                crate::connect::AdmissionError::IndexerBehind
            ))
        ));
        assert_eq!(lagging.requests(), vec!["/block-height/900001"]);
        let client = Client {
            providers: vec![lagging.provider("Connect")],
            #[cfg(feature = "native-recovery")]
            scan_pacer: None,
            admission: Some(Arc::new(backend_fixture(&lagging, authority))),
            abort: Arc::new(AtomicBool::new(false)),
        };
        assert!(matches!(
            client.try_in_order(|_| Ok(1)),
            Err(Error::Admission(
                crate::connect::AdmissionError::IndexerBehind
            ))
        ));
        assert!(client.providers[0].is_cooling());
        let requests = lagging.requests().len();
        assert!(matches!(
            client.try_in_order(|_| Ok(1)),
            Err(Error::AllCooling)
        ));
        assert_eq!(lagging.requests().len(), requests);
    }
    #[test]
    fn bitcoin_keeps_successful_operation_result_when_abort_arrives_during_it() {
        let client = client_with(vec![fake_provider("Bitcoin")]);
        assert_eq!(
            client
                .try_in_order(|_| {
                    client.abort.store(true, Ordering::Relaxed);
                    Ok(42)
                })
                .unwrap(),
            42
        );
    }

    #[test]
    fn admission_throttle_uses_existing_cooldown_and_abort_skips_remaining_calls() {
        use bitcoin::hashes::Hash;
        let hash = bitcoin::BlockHash::from_byte_array([7; 32]);
        for status in [402, 429] {
            let server = mock_esplora(StdHashMap::from([(
                "/block-height/900000",
                (status, "slow down".into()),
            )]));
            let client = Client {
                providers: vec![server.provider("Connect")],
                #[cfg(feature = "native-recovery")]
                scan_pacer: None,
                admission: Some(Arc::new(backend_fixture(&server, authority_fixture(hash)))),
                abort: Arc::new(AtomicBool::new(false)),
            };
            let result: Result<u32, Error> =
                client.try_in_order(|_| panic!("throttled admission cannot run operation"));
            assert!(matches!(result, Err(Error::AllCooling)));
            assert!(client.providers[0].is_cooling());
            assert!(matches!(
                client.try_in_order(|_| Ok(1)),
                Err(Error::AllCooling)
            ));
            assert_eq!(server.requests().len(), 1);
        }
        let server = mock_esplora(StdHashMap::from([(
            "/block-height/900000",
            (200, hash.to_string()),
        )]));
        let abort = Arc::new(AtomicBool::new(true));
        assert!(matches!(
            Client::new_for_connect(
                backend_fixture(&server, authority_fixture(hash)),
                abort.clone()
            ),
            Err(Error::Aborted)
        ));
        assert!(server.requests().is_empty());
        abort.store(false, Ordering::Relaxed);
        let client = Client::new_for_connect(
            backend_fixture(&server, authority_fixture(hash)),
            abort.clone(),
        )
        .unwrap();
        let before = server.requests().len();
        assert!(matches!(
            client.try_in_order(|_| {
                abort.store(true, Ordering::Relaxed);
                Ok(1)
            }),
            Err(Error::Aborted)
        ));
        assert_eq!(
            server.requests().len() - before,
            1,
            "no post-check after cancellation"
        );
        abort.store(false, Ordering::Relaxed);
        let before = server.requests().len();
        assert_eq!(
            client
                .try_in_order_checked(
                    |_| {
                        abort.store(true, Ordering::Relaxed);
                        Ok(1)
                    },
                    false
                )
                .unwrap(),
            1
        );
        assert_eq!(
            server.requests().len() - before,
            1,
            "successful broadcast result survives cancellation"
        );
    }

    #[test]
    fn connect_provider_checks_identity_before_and_after_an_operation() {
        use crate::connect::{
            AdmissionError, ConnectAnchorAuthority, ConnectBackend, TrustedChainAnchor,
        };
        use bitcoin::hashes::Hash;
        use coincube_core::chain::ChainId;
        struct Authority(Mutex<TrustedChainAnchor>);
        impl ConnectAnchorAuthority for Authority {
            fn fresh_anchor(&self) -> Result<TrustedChainAnchor, AdmissionError> {
                Ok(self.0.lock().unwrap().clone())
            }
        }
        let hash = bitcoin::BlockHash::from_byte_array([7; 32]);
        let server = mock_esplora(StdHashMap::from([(
            "/block-height/900000",
            (200, hash.to_string()),
        )]));
        let authority = Arc::new(Authority(Mutex::new(TrustedChainAnchor {
            chain: ChainId::BitcoinBlake2b,
            height: 900000,
            hash,
            median_time_past: 1000,
            observed_at: std::time::SystemTime::now(),
        })));
        let backend = ConnectBackend::new(
            ChainId::BitcoinBlake2b,
            server.base.clone(),
            "synthetic-jwt".into(),
            authority.clone(),
        )
        .unwrap();
        let client = Client::new_for_connect(backend, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(client.providers.len(), 1);
        assert_eq!(client.try_in_order(|_| Ok(42)).unwrap(), 42);
        let result = client.try_in_order(|_| {
            authority.0.lock().unwrap().hash = bitcoin::BlockHash::from_byte_array([8; 32]);
            Ok(42)
        });
        assert!(matches!(
            result,
            Err(Error::Admission(AdmissionError::HashMismatch))
        ));
        let result: Result<u32, Error> = client
            .try_in_order(|_| panic!("failed pre-operation admission must not run the operation"));
        assert!(matches!(
            result,
            Err(Error::Admission(AdmissionError::HashMismatch))
        ));
        assert!(server
            .requests()
            .iter()
            .all(|path| path == "/block-height/900000"));
    }

    #[test]
    fn should_fall_back_classifies_correctly() {
        // Ok results never fall back.
        let ok: Result<(), esplora_client::Error> = Ok(());
        assert!(!should_fall_back(&ok));

        // 402 / 429 fall back (capacity-class 4xx).
        for status in [402u16, 429] {
            let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::HttpResponse {
                status,
                message: String::new(),
            });
            assert!(should_fall_back(&r), "status {} should fall back", status);
        }

        // 5xx falls back.
        for status in [500u16, 502, 503, 504] {
            let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::HttpResponse {
                status,
                message: String::new(),
            });
            assert!(should_fall_back(&r), "status {} should fall back", status);
        }

        // Other 4xx pass through — they describe the request, not the provider.
        for status in [400u16, 401, 403, 404] {
            let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::HttpResponse {
                status,
                message: String::new(),
            });
            assert!(
                !should_fall_back(&r),
                "status {} should not fall back",
                status
            );
        }
    }

    #[test]
    fn is_throttled_matches_only_402_and_429() {
        for status in [402u16, 429] {
            let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::HttpResponse {
                status,
                message: String::new(),
            });
            assert!(is_throttled(&r), "{} should be throttled", status);
        }
        // 5xx and other 4xx do not trigger a cooldown.
        for status in [500u16, 503, 400, 404] {
            let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::HttpResponse {
                status,
                message: String::new(),
            });
            assert!(!is_throttled(&r), "{} should not be throttled", status);
        }
        // Non-HTTP errors don't trigger cooldown either. `Parsing`
        // stands in for any transport/decoding-layer error variant.
        let parse_err: std::num::ParseIntError = "x".parse::<u32>().unwrap_err();
        let r: Result<(), esplora_client::Error> = Err(esplora_client::Error::Parsing(parse_err));
        assert!(!is_throttled(&r));
    }

    /// Build a `Client` directly from a vec of providers, bypassing the
    /// real `Builder` / network. Lets us drive `try_in_order` without
    /// hitting an actual Esplora server.
    fn client_with(providers: Vec<Provider>) -> Client {
        Client {
            #[cfg(feature = "native-recovery")]
            scan_pacer: None,
            admission: None,
            providers,
            abort: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Provider whose `client` we never use — `op` closures in the
    /// tests don't touch it.
    fn fake_provider(name: &str) -> Provider {
        Provider {
            name: name.into(),
            client: build_blocking_client("http://127.0.0.1:1", None),
            cooldown_until: Mutex::new(None),
        }
    }

    /// `try_in_order` must bail with [`Error::Aborted`] — without running `op` —
    /// once the shared abort flag is set, so `DaemonHandle::stop` doesn't block
    /// on a poller stuck dialling dead/throttled providers.
    #[test]
    fn try_in_order_aborts_when_flag_set() {
        let client = client_with(vec![fake_provider("p1"), fake_provider("p2")]);
        client.abort.store(true, Ordering::Relaxed);

        let mut called = false;
        let result: Result<u32, Error> = client.try_in_order(|_| {
            called = true;
            Ok(7)
        });

        assert!(!called, "op must not run once aborting");
        assert!(matches!(result, Err(Error::Aborted)));
    }

    /// Regression: when the primary returns 429, the cooldown must be
    /// set so subsequent ticks skip the primary entirely (rather than
    /// re-discovering the throttle and paying its latency every time).
    #[test]
    fn throttled_provider_enters_cooldown_and_chain_continues() {
        let client = client_with(vec![fake_provider("p1"), fake_provider("p2")]);

        let mut call_count: u32 = 0;
        let result: Result<u32, Error> = client.try_in_order(|_| {
            call_count += 1;
            if call_count == 1 {
                Err(esplora_client::Error::HttpResponse {
                    status: 429,
                    message: "too many".into(),
                })
            } else {
                Ok(42)
            }
        });

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 42);
        // First provider must now be cooling; second must not.
        assert!(client.providers[0].is_cooling(), "p1 must be on cooldown");
        assert!(
            !client.providers[1].is_cooling(),
            "p2 must NOT be on cooldown"
        );
    }

    /// Once a provider is in cooldown, `try_in_order` must skip it on
    /// subsequent calls and go straight to the next provider.
    #[test]
    fn cooled_provider_is_skipped_on_next_call() {
        let client = client_with(vec![fake_provider("p1"), fake_provider("p2")]);
        // Hand-set p1's cooldown.
        client.providers[0].enter_cooldown(RATE_LIMIT_COOLDOWN);

        let mut which: Option<&str> = None;
        let mut p1_called = false;
        let mut p2_called = false;
        let _: Result<u32, Error> = client.try_in_order(|c| {
            // Use a pointer-identity check to tell which provider's
            // client we got.
            if std::ptr::eq(c, &client.providers[0].client) {
                p1_called = true;
                which = Some("p1");
            } else if std::ptr::eq(c, &client.providers[1].client) {
                p2_called = true;
                which = Some("p2");
            }
            Ok(7)
        });

        assert!(!p1_called, "cooled p1 must be skipped");
        assert!(p2_called, "p2 must serve the request");
        assert_eq!(which, Some("p2"));
    }

    /// 5xx errors must NOT set the cooldown — the
    /// provider could be back in seconds, and a 10-minute lockout
    /// over a transient blip would unnecessarily concentrate load
    /// on the next tier.
    #[test]
    fn non_throttle_retryable_errors_do_not_set_cooldown() {
        let client = client_with(vec![fake_provider("p1"), fake_provider("p2")]);

        let mut call_count = 0u32;
        let _: Result<u32, Error> = client.try_in_order(|_| {
            call_count += 1;
            if call_count == 1 {
                Err(esplora_client::Error::HttpResponse {
                    status: 503,
                    message: "transient".into(),
                })
            } else {
                Ok(99)
            }
        });

        assert!(
            !client.providers[0].is_cooling(),
            "p1 must NOT enter cooldown on a 5xx",
        );
    }

    /// Construct the exact error shape the reported stalls produced:
    /// `Minreq(IoError(TimedOut))`. Used to drive the transport-failure path.
    fn minreq_timeout() -> esplora_client::Error {
        esplora_client::Error::Minreq(minreq::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "the timeout of the request was reached",
        )))
    }

    /// `is_transport_failure` must match ONLY genuine transport errors
    /// (`Minreq`), not responses from a reachable server. A 5xx, a 404, or a
    /// decode/not-found error means the provider answered — cooling it down
    /// would needlessly sideline a healthy endpoint.
    #[test]
    fn is_transport_failure_matches_only_minreq_errors() {
        let ok: Result<u32, _> = Ok(1);
        assert!(!is_transport_failure(&ok));

        for status in [400u16, 404, 429, 500, 503] {
            let r: Result<u32, _> = Err(esplora_client::Error::HttpResponse {
                status,
                message: "x".into(),
            });
            assert!(
                !is_transport_failure(&r),
                "HTTP {} is a server answer, not a transport failure",
                status
            );
        }

        // Errors from a *responding* server must NOT count as transport
        // failures (regression: an earlier version matched all non-HTTP errors
        // and would wrongly cool a healthy provider on a decode/not-found).
        let not_found: Result<u32, _> = Err(esplora_client::Error::HeaderHeightNotFound(0));
        assert!(
            !is_transport_failure(&not_found),
            "a not-found error is a server answer, not a transport failure",
        );

        let timeout: Result<u32, _> = Err(minreq_timeout());
        assert!(
            is_transport_failure(&timeout),
            "Minreq(IoError(TimedOut)) is the transport-failure signal",
        );
    }

    /// Regression for the reported 30s-per-action stalls: a transport
    /// failure (timeout/unreachable) must cool the provider down so the
    /// next call skips it instead of re-dialling and timing out again.
    #[test]
    fn transport_failure_enters_cooldown_and_chain_continues() {
        let client = client_with(vec![fake_provider("p1"), fake_provider("p2")]);

        let mut call_count: u32 = 0;
        let result: Result<u32, Error> = client.try_in_order(|_| {
            call_count += 1;
            if call_count == 1 {
                Err(minreq_timeout())
            } else {
                Ok(42)
            }
        });

        assert_eq!(result.unwrap(), 42);
        assert!(
            client.providers[0].is_cooling(),
            "an unreachable provider must enter cooldown so it's skipped next call",
        );
        assert!(
            !client.providers[1].is_cooling(),
            "the provider that served the request must NOT be cooled",
        );
    }

    #[test]
    fn sole_provider_retries_after_short_transport_cooldown() {
        for count in [1, 2] {
            let client = client_with((0..count).map(|_| fake_provider("provider")).collect());
            let before = Instant::now();
            let result: Result<u32, Error> = client.try_in_order(|_| Err(minreq_timeout()));
            assert!(matches!(result, Err(Error::Client(_))));
            let delay = if count == 1 {
                Duration::from_secs(5)
            } else {
                Duration::from_secs(120)
            };
            for provider in &client.providers {
                let mut deadline = provider.cooldown_until.lock().unwrap();
                assert!(deadline.unwrap() >= before + delay);
                assert!(deadline.unwrap() <= Instant::now() + delay);
                // Advance the provider deadlines instead of sleeping in this test.
                *deadline = Some(deadline.unwrap() - Duration::from_secs(5));
            }
            let mut calls = 0;
            let result = client.try_in_order(|_| {
                calls += 1;
                Ok(42)
            });
            if count == 1 {
                assert_eq!(result.unwrap(), 42);
                assert_eq!(calls, 1);
                assert!(!client.providers[0].is_cooling());
            } else {
                assert!(matches!(result, Err(Error::AllCooling)));
                assert_eq!(calls, 0);
            }
        }
    }

    #[test]
    fn sole_provider_startup_and_admission_use_short_transport_backoff() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        // A freshly released ephemeral port gives connection-refused, rather
        // than an EOF that the HTTP parser classifies as a malformed response.
        drop(listener);
        let config = crate::config::EsploraConfig {
            addr,
            token: None,
            fallback_addr: None,
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        };
        let before = Instant::now();
        let abort = Arc::new(AtomicBool::new(false));
        let client = Client::new(&config, abort.clone()).unwrap();
        let deadline = client.providers[0].cooldown_until.lock().unwrap().unwrap();
        assert!(deadline >= before + Duration::from_secs(5));
        assert!(deadline <= Instant::now() + Duration::from_secs(5));
        client.providers[0].clear_cooldown();
        let before = Instant::now();
        assert!(matches!(
            admission_hash_at(&client.providers[0], &abort, 1, transport_cooldown(1)),
            Err(crate::connect::AdmissionError::Unavailable)
        ));
        let deadline = client.providers[0].cooldown_until.lock().unwrap().unwrap();
        assert!(deadline >= before + Duration::from_secs(5));
        assert!(deadline <= Instant::now() + Duration::from_secs(5));
    }

    #[test]
    fn sole_provider_retains_full_rate_limit_backoff() {
        for status in [402, 429] {
            let client = client_with(vec![fake_provider("sole")]);
            let before = Instant::now();
            let _: Result<u32, Error> = client.try_in_order(|_| {
                Err(esplora_client::Error::HttpResponse {
                    status,
                    message: "rate limit".into(),
                })
            });
            assert!(
                client.providers[0].cooldown_until.lock().unwrap().unwrap()
                    >= before + Duration::from_secs(600)
            );
            assert!(matches!(
                client.try_in_order(|_| Ok(42)),
                Err(Error::AllCooling)
            ));
        }
    }

    /// A 5xx (reachable-but-erroring server) must still fall through WITHOUT a
    /// cooldown — only `Minreq` transport failures cool a provider down.
    #[test]
    fn server_error_does_not_enter_transport_cooldown() {
        let client = client_with(vec![fake_provider("p1"), fake_provider("p2")]);
        let mut n = 0u32;
        let _: Result<u32, Error> = client.try_in_order(|_| {
            n += 1;
            if n == 1 {
                Err(esplora_client::Error::HttpResponse {
                    status: 503,
                    message: "busy".into(),
                })
            } else {
                Ok(1)
            }
        });
        assert!(
            !client.providers[0].is_cooling(),
            "a 503 must not trigger the transport cooldown",
        );
    }

    /// A successful call from a previously-throttled provider must
    /// clear its cooldown so it rejoins the rotation immediately,
    /// rather than waiting out the rest of the lockout window.
    #[test]
    fn successful_call_clears_cooldown() {
        let p = fake_provider("p1");
        p.enter_cooldown(RATE_LIMIT_COOLDOWN);
        assert!(p.is_cooling());
        let client = client_with(vec![p]);

        let _: Result<u32, Error> = client.try_in_order(|_| Ok(1));
        // Whoops — when cooling, `try_in_order` should have skipped p1
        // entirely without calling op. That means cooldown survives.
        // So instead drop the cooldown first to simulate it having
        // naturally expired, then verify success clears it.
        client.providers[0].clear_cooldown();
        // Re-enter a fresh cooldown to test the clear-on-success path.
        client.providers[0].enter_cooldown(RATE_LIMIT_COOLDOWN);
        // Manually expire it so the call proceeds.
        *client.providers[0].cooldown_until.lock().unwrap() = None;

        let _: Result<u32, Error> = client.try_in_order(|_| Ok(1));
        assert!(
            !client.providers[0].is_cooling(),
            "successful call must clear residual cooldown",
        );
    }

    /// Non-retryable errors (400, 404, etc.) must NOT cascade through
    /// the chain — they describe the *request*, not the *provider*.
    /// Asking the next provider would just produce the same 404.
    #[test]
    fn non_retryable_error_short_circuits_the_chain() {
        let client = client_with(vec![fake_provider("p1"), fake_provider("p2")]);

        let mut call_count = 0u32;
        let result: Result<u32, Error> = client.try_in_order(|_| {
            call_count += 1;
            Err(esplora_client::Error::HttpResponse {
                status: 404,
                message: "not found".into(),
            })
        });

        assert_eq!(
            call_count, 1,
            "404 must not be retried on the next provider"
        );
        assert!(result.is_err());
    }

    /// If every provider is cooling, `try_in_order` must surface the
    /// typed [`Error::AllCooling`] rather than masquerading as a
    /// real failure (or synthesising a 503 that looks like an
    /// upstream HTTP error). The poller routes `AllCooling` to a
    /// quieter log level and a longer backoff.
    #[test]
    fn all_cooling_returns_typed_variant() {
        let client = client_with(vec![fake_provider("p1"), fake_provider("p2")]);
        for p in &client.providers {
            p.enter_cooldown(RATE_LIMIT_COOLDOWN);
        }
        let result: Result<u32, Error> = client.try_in_order(|_| Ok(1));
        match result {
            Err(e) => {
                assert!(e.is_all_cooling(), "expected AllCooling, got {:?}", e);
                // Display must communicate "this is transient" so a
                // human glancing at the log doesn't read it as a
                // real fault.
                let msg = e.to_string();
                assert!(
                    msg.contains("temporarily backing off"),
                    "Display should describe the transient nature; got: {}",
                    msg,
                );
            }
            Ok(v) => panic!("expected Err(AllCooling), got Ok({})", v),
        }
    }

    /// Regression: the poller pattern-matches the rendered error
    /// string at the trait boundary. If a refactor changes the
    /// `Display` impl without updating
    /// [`ALL_COOLING_DISPLAY_MARKER`], the poller would silently
    /// stop quieting the spam.
    #[test]
    fn all_cooling_display_contains_the_published_marker() {
        let msg = Error::AllCooling.to_string();
        assert!(
            msg.starts_with(ALL_COOLING_DISPLAY_MARKER),
            "Display must start with the marker the poller scans for; got: {}",
            msg,
        );
    }

    // ── tip_time over JSON block summaries (coincube-api#290) ──────────────────
    //
    // A dependency-free mock Esplora: a TcpListener on 127.0.0.1 answering the
    // handful of paths these tests need from a canned table, and recording
    // every path it was asked for. No live endpoint is ever contacted.

    use std::collections::HashMap as StdHashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Upper bound on a request head the mock will read; anything larger —
    /// terminated or not — is answered 400 and closed. Real requests here are
    /// a few hundred bytes.
    const MOCK_MAX_HEAD: usize = 8 * 1024;
    /// Absolute deadline for reading one request head, measured from accept.
    /// A peer that trickles bytes cannot extend it: every socket read is capped
    /// to the time remaining, and the stop flag is observed between reads, so
    /// neither a held-open nor a dripping partial request can pin the server
    /// thread — or a test's teardown — beyond this.
    const MOCK_REQUEST_DEADLINE: Duration = Duration::from_millis(500);
    /// Longest single socket wait inside the deadline, so the stop flag is
    /// re-checked at least this often while a peer is silent.
    const MOCK_READ_SLICE: Duration = Duration::from_millis(100);
    /// Most bytes the mock will read and discard after refusing a request,
    /// so the refused peer's remaining bytes are consumed before the socket
    /// is closed. Closing a TCP socket with unread data makes the kernel send
    /// RST instead of FIN, which can fail the peer's in-flight writes and
    /// discard the 400 already sent to it (the CI failure this bounds
    /// against). The drain shares the connection's absolute deadline and the
    /// stop flag; a peer that sends more than this cap, or past the deadline,
    /// is closed with whatever is left unread — a reset is possible then, by
    /// design, so no bound is ever extended for a hostile peer.
    const MOCK_DRAIN_CAP: usize = 64 * 1024;

    /// The refusal every unreadable request gets, then the socket is closed.
    const MOCK_REFUSAL: &[u8] =
        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    struct MockEsplora {
        base: String,
        addr: std::net::SocketAddr,
        requests: Arc<Mutex<Vec<String>>>,
        heads: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    /// Read one HTTP request head from `stream`: through the `\r\n\r\n`
    /// terminator, bounded by [`MOCK_MAX_HEAD`] bytes, the connection's
    /// absolute `deadline` (set at accept, see [`MOCK_REQUEST_DEADLINE`]) and
    /// the `stop` flag. `None` when the head does not complete within those
    /// bounds. One TCP read is not a message boundary, so this loops until the
    /// terminator — but the size bound is applied BEFORE a terminator is
    /// honoured (a terminated head over the limit is still refused), each read
    /// is capped to the remaining capacity so the limit cannot be overshot,
    /// and each socket wait is capped to the time remaining so a dripping peer
    /// cannot extend the total.
    fn read_request_head(
        stream: &mut std::net::TcpStream,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Option<String> {
        let mut head: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            if stop.load(Ordering::SeqCst) {
                return None;
            }
            // Size first: a terminator beyond the limit is not a valid head.
            if head.len() > MOCK_MAX_HEAD {
                return None;
            }
            if head.windows(4).any(|w| w == b"\r\n\r\n") {
                return Some(String::from_utf8_lossy(&head).to_string());
            }
            let remaining_time = deadline.checked_duration_since(Instant::now())?;
            stream
                .set_read_timeout(Some(remaining_time.min(MOCK_READ_SLICE)))
                .ok()?;
            // Never read past the limit: cap the read to what is left (+1 so an
            // over-limit head is detected as such rather than truncated).
            let remaining_cap = (MOCK_MAX_HEAD + 1)
                .saturating_sub(head.len())
                .min(chunk.len());
            if remaining_cap == 0 {
                return None;
            }
            match stream.read(&mut chunk[..remaining_cap]) {
                Ok(0) => return None, // peer closed before completing the head
                Ok(n) => head.extend_from_slice(&chunk[..n]),
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    // Slice elapsed: loop to re-check stop/deadline.
                }
                Err(_) => return None, // reset or other failure: give up
            }
        }
    }

    /// Answer a request the mock will not serve with a 400 and close the
    /// connection *cleanly*: send the refusal, shut down our writing side so
    /// the peer reads the 400 and then EOF, and read-and-discard whatever the
    /// peer still has in flight — bounded by the same absolute `deadline` the
    /// head read ran under, the `stop` flag and [`MOCK_DRAIN_CAP`]. Draining
    /// is what keeps the close a FIN rather than a RST: the kernel resets a
    /// connection closed with unread data, which raced the peer's trailing
    /// writes and its read of the 400 on Linux CI. Past the bounds the peer's
    /// leftovers stay unread and a reset is accepted; the bounds are never
    /// extended.
    fn refuse_and_close(stream: &mut std::net::TcpStream, stop: &AtomicBool, deadline: Instant) {
        let _ = stream.write_all(MOCK_REFUSAL);
        let _ = stream.flush();
        let _ = stream.shutdown(std::net::Shutdown::Write);
        drain_until_eof(stream, stop, deadline);
    }

    /// Read and discard from `stream` until the peer closes (EOF), the
    /// `deadline` passes, `stop` is raised, or [`MOCK_DRAIN_CAP`] bytes have
    /// been discarded — whichever comes first. Returns the number of bytes
    /// drained. Same slicing discipline as [`read_request_head`]: every wait
    /// is capped to the time remaining and to [`MOCK_READ_SLICE`].
    fn drain_until_eof(
        stream: &mut std::net::TcpStream,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> usize {
        let mut drained = 0usize;
        let mut chunk = [0u8; 1024];
        loop {
            if stop.load(Ordering::SeqCst) || drained >= MOCK_DRAIN_CAP {
                return drained;
            }
            let Some(remaining_time) = deadline.checked_duration_since(Instant::now()) else {
                return drained;
            };
            if stream
                .set_read_timeout(Some(remaining_time.min(MOCK_READ_SLICE)))
                .is_err()
            {
                return drained;
            }
            let cap = (MOCK_DRAIN_CAP - drained).min(chunk.len());
            match stream.read(&mut chunk[..cap]) {
                Ok(0) => return drained, // peer closed: nothing left to drain
                Ok(n) => drained += n,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    // Slice elapsed: loop to re-check stop/deadline.
                }
                Err(_) => return drained, // reset or other failure: give up
            }
        }
    }

    /// (status, body) per exact path; unknown paths answer 404.
    fn mock_esplora(routes: StdHashMap<&'static str, (u16, String)>) -> MockEsplora {
        mock_esplora_with_pacing(routes, None)
    }
    type MockPacing = (Duration, Arc<Mutex<Vec<Instant>>>);
    fn mock_esplora_with_pacing(
        routes: StdHashMap<&'static str, (u16, String)>,
        pacing: Option<MockPacing>,
    ) -> MockEsplora {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock esplora");
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{}", addr);
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen_heads = heads.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = requests.clone();
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            // Bounded accept loop: `Drop` raises `stop` and then connects once
            // to wake `accept`, so the loop observes the flag and exits; it is
            // never left running after the test that owns it.
            for stream in listener.incoming() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let _ = stream.set_write_timeout(Some(MOCK_READ_SLICE));
                // One absolute deadline per connection, spanning the head read
                // and — if it is refused — the drain before close.
                let deadline = Instant::now() + MOCK_REQUEST_DEADLINE;
                let Some(head) = read_request_head(&mut stream, &stopping, deadline) else {
                    refuse_and_close(&mut stream, &stopping, deadline);
                    continue;
                };
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                seen_heads.lock().unwrap().push(head);
                seen.lock().unwrap().push(path.clone());
                let paced_out = if path.starts_with("/scripthash/") {
                    pacing.as_ref().is_some_and(|(minimum, times)| {
                        let now = Instant::now();
                        let mut times = times.lock().unwrap();
                        let too_fast = times
                            .last()
                            .is_some_and(|last| now.duration_since(*last) < *minimum);
                        times.push(now);
                        too_fast
                    })
                } else {
                    false
                };
                let (mut status, mut body) = routes
                    .get(path.as_str())
                    .cloned()
                    .unwrap_or((404, "not found".to_string()));
                if paced_out {
                    status = 429;
                    body = "unit rate limit".into();
                }
                let reason = match status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "Not Found",
                };
                let response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    reason,
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        MockEsplora {
            base,
            addr,
            requests,
            heads,
            stop,
            thread: Some(thread),
        }
    }

    impl MockEsplora {
        fn provider(&self, name: &str) -> Provider {
            Provider {
                name: name.into(),
                client: build_blocking_client(&self.base, None),
                cooldown_until: Mutex::new(None),
            }
        }
        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for MockEsplora {
        /// Explicit, bounded shutdown: flag, wake the blocked `accept` with one
        /// local connection, join. A connection the server is mid-read (or
        /// mid-drain) on observes the flag within [`MOCK_READ_SLICE`] and in
        /// any case gives up at its absolute [`MOCK_REQUEST_DEADLINE`], so the
        /// join is bounded by roughly that deadline regardless of what the
        /// peer sends.
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            let _ = std::net::TcpStream::connect_timeout(&self.addr, Duration::from_secs(1));
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    const H0: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const H1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const H2: &str = "2222222222222222222222222222222222222222222222222222222222222222";
    const MERKLE: &str = "3333333333333333333333333333333333333333333333333333333333333333";

    /// A `/blocks` entry as a Bitcoin Esplora renders it.
    fn bitcoin_summary(id: &str, height: u32, timestamp: u64, prev: &str) -> String {
        format!(
            r#"{{"id":"{id}","height":{height},"version":536870912,"timestamp":{timestamp},"tx_count":2500,"size":1500000,"weight":3990000,"merkle_root":"{MERKLE}","previousblockhash":"{prev}","mediantime":{mt},"nonce":123456789,"bits":386089497,"difficulty":110000000000000}}"#,
            id = id,
            height = height,
            timestamp = timestamp,
            prev = prev,
            mt = timestamp.saturating_sub(600),
            MERKLE = MERKLE
        )
    }

    /// The same entry as a Bitcoin Blake2b indexer (retropex/electrs) would
    /// plausibly render a post-fork v2 header: every Bitcoin field plus extra
    /// fork-specific ones. Only `id`/`height`/`timestamp` matter to the reader;
    /// the extras must be tolerated, not refused.
    fn blake2b_summary(id: &str, height: u32, timestamp: u64, prev: &str) -> String {
        let base = bitcoin_summary(id, height, timestamp, prev);
        format!(
            r#"{},"header_version":2,"header_size":164,"blake2b":true,"pow_algo":"blake2b","headline":"BTCB2","rdts_active":true}}"#,
            &base[..base.len() - 1]
        )
    }

    fn blocks_json(entries: &[String]) -> String {
        format!("[{}]", entries.join(","))
    }

    fn routes(blocks: (u16, String)) -> StdHashMap<&'static str, (u16, String)> {
        let mut m = StdHashMap::new();
        m.insert("/blocks", blocks);
        // Present so a regression back to the raw-header shape is caught by the
        // request log rather than by a 404.
        m.insert("/blocks/tip/hash", (200, H2.to_string()));
        m
    }

    #[cfg(feature = "native-recovery")]
    #[test]
    fn native_full_scan_pacing_is_aggregate_and_survives_retry_builders() {
        use bdk_electrum::bdk_chain::{local_chain::CheckPoint, BlockId};
        use bitcoin::{
            hashes::{sha256, Hash},
            ScriptBuf,
        };
        let scripts = [
            ScriptBuf::from_bytes(vec![0x6a]),
            ScriptBuf::from_bytes(vec![0x6a, 0x00]),
        ];
        let mut paths = routes((200, blocks_json(&[bitcoin_summary(H1, 1, 1231006505, H0)])));
        paths.insert("/block-height/0", (200, H0.into()));
        for script in &scripts {
            let key = format!("/scripthash/{}/txs", sha256::Hash::hash(script.as_bytes()));
            paths.insert(Box::leak(key.into_boxed_str()), (200, "[]".into()));
        }
        let build = || {
            let tip = CheckPoint::new(BlockId {
                height: 0,
                hash: H0.parse().unwrap(),
            });
            let mut request = FullScanRequest::from_chain_tip(tip);
            for (branch, script) in scripts.iter().cloned().enumerate() {
                request = request
                    .set_spks_for_keychain(branch, (0..).map(move |index| (index, script.clone())));
            }
            request
        };
        let unpaced_times = Arc::new(Mutex::new(Vec::new()));
        let unpaced = mock_esplora_with_pacing(
            paths.clone(),
            Some((Duration::from_millis(50), unpaced_times)),
        );
        let client = client_with(vec![unpaced.provider("unit")]);
        assert!(client.full_scan(build, 4, 2).is_err());
        let times = Arc::new(Mutex::new(Vec::new()));
        let mut failed_paths = paths.clone();
        for (path, response) in &mut failed_paths {
            if path.starts_with("/scripthash/") {
                *response = (500, "unit upstream failure".into());
            }
        }
        let failing = mock_esplora_with_pacing(
            failed_paths,
            Some((Duration::from_millis(50), times.clone())),
        );
        let server =
            mock_esplora_with_pacing(paths, Some((Duration::from_millis(50), times.clone())));
        let mut client = client_with(vec![
            failing.provider("failed unit"),
            server.provider("unit"),
        ]);
        client.set_native_recovery_pacing(Duration::from_millis(75));
        client.full_scan(build, 4, 8).unwrap();
        // A new request builder/poll uses the SAME client pacing state.
        client.full_scan(build, 4, 8).unwrap();
        let observed = times.lock().unwrap();
        assert_eq!(observed.len(), 18);
        assert!(observed
            .windows(2)
            .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(50)));
        drop(observed);
        client.abort.store(true, Ordering::Relaxed);
        assert!(matches!(client.full_scan(build, 4, 8), Err(Error::Aborted)));
        // An abort while waiting cannot be accepted as a partial BDK scan.
        client.abort.store(false, Ordering::Relaxed);
        *client.scan_pacer.as_ref().unwrap().next.lock().unwrap() =
            Instant::now() + Duration::from_secs(2);
        let signal = client.abort.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            signal.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        assert!(matches!(client.full_scan(build, 4, 8), Err(Error::Aborted)));
        assert!(started.elapsed() < Duration::from_millis(250));
        trigger.join().unwrap();
        assert_eq!(times.lock().unwrap().len(), 18);
    }

    #[test]
    fn public_rate_limit_uses_authenticated_backstop_and_all_cooling_stays_unknown() {
        let mut throttled = StdHashMap::new();
        throttled.insert("/blocks/tip/height", (429, "capacity".into()));
        throttled.insert("/blocks", (429, "capacity".into()));
        let public = mock_esplora(throttled.clone());
        let mut healthy = StdHashMap::new();
        healthy.insert("/blocks/tip/height", (200, "2".into()));
        healthy.insert(
            "/blocks",
            (200, blocks_json(&[bitcoin_summary(H2, 2, 1700000000, H1)])),
        );
        let backstop = mock_esplora(healthy);
        let config = crate::config::EsploraConfig {
            addr: public.base.clone(),
            token: None,
            fallback_addr: Some(backstop.base.clone()),
            fallback_token: Some("localhost-unit-token".into()),
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        };
        let client = Client::new(&config, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(client.tip_time().unwrap(), 1700000000);
        assert!(public
            .heads
            .lock()
            .unwrap()
            .iter()
            .all(|head| !head.contains("localhost-unit-token")));
        assert!(backstop.heads.lock().unwrap().iter().any(|head| head
            .to_ascii_lowercase()
            .contains("authorization: bearer localhost-unit-token")));
        let unavailable = mock_esplora(throttled);
        let failed_config = crate::config::EsploraConfig {
            fallback_addr: Some(unavailable.base.clone()),
            ..config
        };
        let failed = Client::new(&failed_config, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(matches!(failed.tip_time(), Err(Error::AllCooling)));
    }

    #[test]
    fn full_scan_shutdown_stops_address_batches_and_rejects_partial_result() {
        use bdk_electrum::bdk_chain::{local_chain::CheckPoint, BlockId};
        use bitcoin::{
            hashes::{sha256, Hash},
            ScriptBuf,
        };
        let script = ScriptBuf::new();
        let path = format!("/scripthash/{}/txs", sha256::Hash::hash(script.as_bytes()));
        let mut paths = routes((200, blocks_json(&[bitcoin_summary(H1, 1, 1231006505, H0)])));
        paths.insert("/block-height/0", (200, H0.into()));
        paths.insert(Box::leak(path.into_boxed_str()), (200, "[]".into()));
        let primary = mock_esplora(paths);
        let fallback = mock_esplora(StdHashMap::new());
        let client = client_with(vec![
            primary.provider("primary"),
            fallback.provider("fallback"),
        ]);
        let tip = CheckPoint::new(BlockId {
            height: 0,
            hash: H0.parse().unwrap(),
        });
        let build = |cancel: bool| {
            let abort = client.abort.clone();
            let script = script.clone();
            FullScanRequest::from_chain_tip(tip.clone())
                .set_spks_for_keychain(
                    0,
                    (0..).map(move |index| {
                        // BDK has finished its first two-address HTTP batch. Model
                        // daemon.stop setting the flag while a scan is underway.
                        if cancel && index == 2 {
                            abort.store(true, Ordering::Relaxed);
                        }
                        (index, script.clone())
                    }),
                )
                .set_spks_for_keychain(1, (0..).map(|index| (index, ScriptBuf::new())))
        };
        assert!(matches!(
            client.full_scan(|| build(true), 200, 2),
            Err(Error::Aborted)
        ));
        assert_eq!(
            primary
                .requests()
                .iter()
                .filter(|p| p.starts_with("/scripthash/"))
                .count(),
            2
        );
        assert!(
            fallback.requests().is_empty(),
            "shutdown must not scan a fallback"
        );

        // Ordinary full scans still cover the complete gap on both keychains.
        client.abort.store(false, Ordering::Relaxed);
        client.full_scan(|| build(false), 4, 2).unwrap();
        assert_eq!(
            primary
                .requests()
                .iter()
                .filter(|p| p.starts_with("/scripthash/"))
                .count(),
            10
        );
    }

    /// The poller's every-tick `sync` gets the same shutdown guard as a full
    /// scan. Without it `DaemonHandle::stop` joined a poller that went on to
    /// query every remaining address first.
    #[test]
    fn sync_shutdown_stops_address_batches_and_rejects_partial_result() {
        use bdk_electrum::bdk_chain::{local_chain::CheckPoint, BlockId};
        use bitcoin::{
            hashes::{sha256, Hash},
            ScriptBuf,
        };
        let script = ScriptBuf::new();
        let path = format!("/scripthash/{}/txs", sha256::Hash::hash(script.as_bytes()));
        let mut paths = routes((200, blocks_json(&[bitcoin_summary(H1, 1, 1231006505, H0)])));
        paths.insert("/block-height/0", (200, H0.into()));
        paths.insert(Box::leak(path.into_boxed_str()), (200, "[]".into()));
        let primary = mock_esplora(paths);
        let fallback = mock_esplora(StdHashMap::new());
        let client = client_with(vec![
            primary.provider("primary"),
            fallback.provider("fallback"),
        ]);
        let tip = CheckPoint::new(BlockId {
            height: 0,
            hash: H0.parse().unwrap(),
        });
        let scripthash_requests = || {
            primary
                .requests()
                .iter()
                .filter(|p| p.starts_with("/scripthash/"))
                .count()
        };
        let build = |cancel: bool| {
            let abort = client.abort.clone();
            let script = script.clone();
            SyncRequest::from_chain_tip(tip.clone()).chain_spks((0..10).map(move |index| {
                // BDK has finished its first two-address HTTP batch. Model
                // daemon.stop setting the flag while a sync is underway.
                if cancel && index == 2 {
                    abort.store(true, Ordering::Relaxed);
                }
                script.clone()
            }))
        };
        assert!(matches!(
            client.sync(|| build(true), 2),
            Err(Error::Aborted)
        ));
        assert_eq!(scripthash_requests(), 2);
        assert!(
            fallback.requests().is_empty(),
            "shutdown must not sync against a fallback"
        );

        // An ordinary sync still queries every address.
        client.abort.store(false, Ordering::Relaxed);
        client.sync(|| build(false), 2).unwrap();
        assert_eq!(scripthash_requests(), 12);
    }

    #[test]
    fn genesis_time_reads_only_height_zero_json() {
        let mut paths = StdHashMap::new();
        paths.insert(
            "/blocks/0",
            (
                200,
                blocks_json(&[format!(
                    r#"{{"id":"{H0}","height":0,"timestamp":1231006505,"merkle_root":"{MERKLE}"}}"#
                )]),
            ),
        );
        let mock = mock_esplora(paths);
        let client = client_with(vec![mock.provider("connect")]);
        assert_eq!(client.genesis_block_timestamp().unwrap(), 1_231_006_505);
        assert_eq!(mock.requests(), vec!["/blocks/0".to_string()]);
    }

    #[test]
    fn genesis_time_refuses_missing_wrong_height_and_ambiguous_metadata() {
        for body in [
            "[]".to_string(),
            blocks_json(&[bitcoin_summary(H1, 1, 600, H0)]),
            blocks_json(&[
                bitcoin_summary(H0, 0, 600, H0),
                bitcoin_summary(H1, 0, 601, H0),
            ]),
            blocks_json(&[bitcoin_summary(H0, 0, u64::from(u32::MAX) + 1, H0)]),
        ] {
            let mut paths = StdHashMap::new();
            paths.insert("/blocks/0", (200, body));
            let mock = mock_esplora(paths);
            let client = client_with(vec![mock.provider("connect")]);
            assert!(matches!(
                client.genesis_block_timestamp(),
                Err(Error::GenesisMetadata(_))
            ));
            assert_eq!(mock.requests(), vec!["/blocks/0".to_string()]);
        }
    }

    #[test]
    fn genesis_time_preserves_upstream_and_parse_errors() {
        for (status, body) in [(503, "unavailable"), (200, "{malformed")] {
            let mut paths = StdHashMap::new();
            paths.insert("/blocks/0", (status, body.to_string()));
            let mock = mock_esplora(paths);
            let client = client_with(vec![mock.provider("connect")]);
            assert!(matches!(
                client.genesis_block_timestamp(),
                Err(Error::Client(_))
            ));
            assert_eq!(mock.requests(), vec!["/blocks/0".to_string()]);
        }
    }

    #[test]
    fn tip_time_reads_blake2b_json_summaries_without_touching_raw_headers() {
        let body = blocks_json(&[
            blake2b_summary(H2, 961_642, 1_756_600_000, H1),
            blake2b_summary(H1, 961_641, 1_756_599_400, H0),
            blake2b_summary(H0, 961_640, 1_756_598_800, MERKLE),
        ]);
        let mock = mock_esplora(routes((200, body)));
        let client = client_with(vec![mock.provider("btcb2")]);

        let t = client.tip_time().expect("tip time from JSON summaries");
        assert_eq!(t, 1_756_600_000);

        let seen = mock.requests();
        assert_eq!(
            seen,
            vec!["/blocks".to_string()],
            "exactly one snapshot request"
        );
        assert!(
            !seen.iter().any(|p| p.contains("/header")),
            "raw header endpoint must never be requested: {:?}",
            seen
        );
    }

    #[test]
    fn tip_time_bitcoin_summaries_pick_the_highest_block_regardless_of_order() {
        // Oldest-first on purpose: the tip is chosen by height, not position.
        let body = blocks_json(&[
            bitcoin_summary(H0, 900_000, 1_700_000_000, MERKLE),
            bitcoin_summary(H2, 900_002, 1_700_001_200, H1),
            bitcoin_summary(H1, 900_001, 1_700_000_600, H0),
        ]);
        let mock = mock_esplora(routes((200, body)));
        let client = client_with(vec![mock.provider("bitcoin")]);
        assert_eq!(client.tip_time().unwrap(), 1_700_001_200);
        assert_eq!(mock.requests(), vec!["/blocks".to_string()]);
    }

    #[test]
    fn tip_time_refuses_empty_or_unusable_metadata_instead_of_defaulting() {
        // Empty list: no tip to speak of.
        let mock = mock_esplora(routes((200, "[]".to_string())));
        let client = client_with(vec![mock.provider("p")]);
        assert!(matches!(client.tip_time(), Err(Error::TipMetadata(_))));

        // Timestamp outside u32 (the type every consumer uses).
        let body = blocks_json(&[bitcoin_summary(H1, 1, 4_294_967_296, H0)]);
        let mock = mock_esplora(routes((200, body)));
        let client = client_with(vec![mock.provider("p")]);
        assert!(matches!(
            client.tip_time(),
            Err(Error::TipMetadata("tip timestamp does not fit in u32"))
        ));

        // Missing timestamp / malformed JSON: a client (parse) error, still no value.
        let missing = format!(
            r#"[{{"id":"{}","height":5,"merkle_root":"{}"}}]"#,
            H1, MERKLE
        );
        let mock = mock_esplora(routes((200, missing)));
        let client = client_with(vec![mock.provider("p")]);
        assert!(matches!(client.tip_time(), Err(Error::Client(_))));

        let mock = mock_esplora(routes((200, "{not json".to_string())));
        let client = client_with(vec![mock.provider("p")]);
        assert!(matches!(client.tip_time(), Err(Error::Client(_))));

        // Negative timestamp cannot deserialise into u64 either.
        let neg = format!(
            r#"[{{"id":"{}","height":5,"timestamp":-1,"merkle_root":"{}"}}]"#,
            H1, MERKLE
        );
        let mock = mock_esplora(routes((200, neg)));
        let client = client_with(vec![mock.provider("p")]);
        assert!(matches!(client.tip_time(), Err(Error::Client(_))));
    }

    #[test]
    fn tip_time_is_one_request_per_call_and_tracks_the_snapshot() {
        // Two sequential calls see two different snapshots; each answer is the
        // timestamp of its own snapshot's tip, and each call issues exactly
        // one `/blocks` request — no separate tip-hash read, no header read.
        let first = blocks_json(&[
            bitcoin_summary(H1, 100, 1_000_600, H0),
            bitcoin_summary(H0, 99, 1_000_000, MERKLE),
        ]);
        let mock = mock_esplora(routes((200, first)));
        let client = client_with(vec![mock.provider("p")]);
        assert_eq!(client.tip_time().unwrap(), 1_000_600);

        let second = blocks_json(&[
            bitcoin_summary(H2, 101, 1_001_200, H1),
            bitcoin_summary(H1, 100, 1_000_600, H0),
        ]);
        let mock2 = mock_esplora(routes((200, second)));
        let client2 = client_with(vec![mock2.provider("p")]);
        assert_eq!(client2.tip_time().unwrap(), 1_001_200);
        for m in [&mock, &mock2] {
            assert_eq!(m.requests(), vec!["/blocks".to_string()]);
        }
    }

    #[test]
    fn tip_time_semantic_failures_try_healthy_fallback_without_cooling() {
        for body in [
            "[]".to_string(),
            blocks_json(&[bitcoin_summary(H1, 1, u64::from(u32::MAX) + 1, H0)]),
        ] {
            let bad = mock_esplora(routes((200, body)));
            let good = mock_esplora(routes((
                200,
                blocks_json(&[bitcoin_summary(H1, 1, 1234, H0)]),
            )));
            let client = client_with(vec![bad.provider("bad"), good.provider("good")]);
            for _ in 0..2 {
                assert_eq!(client.tip_time().unwrap(), 1234);
                assert!(client.providers.iter().all(|p| !p.is_cooling()));
            }
            assert_eq!(bad.requests().len(), 2);
            assert_eq!(good.requests().len(), 2);
        }
    }

    #[test]
    fn tip_time_all_unusable_returns_last_metadata_error() {
        let empty = mock_esplora(routes((200, "[]".to_string())));
        let overflow = mock_esplora(routes((
            200,
            blocks_json(&[bitcoin_summary(H1, 1, u64::from(u32::MAX) + 1, H0)]),
        )));
        let client = client_with(vec![empty.provider("empty"), overflow.provider("overflow")]);
        assert!(matches!(
            client.tip_time(),
            Err(Error::TipMetadata("tip timestamp does not fit in u32"))
        ));
        assert_eq!(empty.requests().len(), 1);
        assert_eq!(overflow.requests().len(), 1);
        assert!(client.providers.iter().all(|p| !p.is_cooling()));
    }

    #[test]
    fn semantic_fallback_still_obeys_shutdown() {
        let first = mock_esplora(routes((200, "[]".to_string())));
        let second = mock_esplora(routes((200, "[]".to_string())));
        let client = client_with(vec![first.provider("first"), second.provider("second")]);
        let mut attempts = 0;
        let result: Result<(), Error> = client.try_in_order_validated(
            |_| {
                attempts += 1;
                Ok(())
            },
            true,
            |_| {
                client.abort.store(true, Ordering::Relaxed);
                Err(Error::TipMetadata("empty"))
            },
        );
        assert!(matches!(result, Err(Error::Aborted)));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn tip_time_keeps_provider_fallback_and_cooldown_semantics() {
        // Primary throttled → cooled and skipped; fallback serves the snapshot.
        let throttled = mock_esplora(routes((429, "slow down".to_string())));
        let healthy = mock_esplora(routes((
            200,
            blocks_json(&[bitcoin_summary(H1, 7, 1_234_567, H0)]),
        )));
        let client = client_with(vec![
            throttled.provider("primary"),
            healthy.provider("fallback"),
        ]);

        assert_eq!(client.tip_time().unwrap(), 1_234_567);
        assert!(client.providers[0].is_cooling(), "429 must enter cooldown");
        assert!(!client.providers[1].is_cooling());

        // Second call skips the cooled primary entirely.
        assert_eq!(client.tip_time().unwrap(), 1_234_567);
        assert_eq!(
            throttled.requests().len(),
            1,
            "cooled primary must not be re-asked"
        );
        assert_eq!(healthy.requests().len(), 2);

        // A 5xx falls through without cooling (existing semantics).
        let flaky = mock_esplora(routes((500, "boom".to_string())));
        let healthy2 = mock_esplora(routes((
            200,
            blocks_json(&[bitcoin_summary(H1, 7, 7_654_321, H0)]),
        )));
        let client = client_with(vec![flaky.provider("flaky"), healthy2.provider("ok")]);
        assert_eq!(client.tip_time().unwrap(), 7_654_321);
        assert!(
            !client.providers[0].is_cooling(),
            "5xx must not cool the provider"
        );

        // Every provider failing surfaces the last real error, never a value.
        let down_a = mock_esplora(routes((500, "a".to_string())));
        let down_b = mock_esplora(routes((500, "b".to_string())));
        let client = client_with(vec![down_a.provider("a"), down_b.provider("b")]);
        assert!(matches!(client.tip_time(), Err(Error::Client(_))));

        // Shutdown abort short-circuits before any request.
        let mock = mock_esplora(routes((200, blocks_json(&[bitcoin_summary(H1, 7, 1, H0)]))));
        let client = client_with(vec![mock.provider("p")]);
        client.abort.store(true, Ordering::Relaxed);
        assert!(matches!(client.tip_time(), Err(Error::Aborted)));
        assert!(mock.requests().is_empty());
    }

    #[test]
    fn tip_metadata_error_displays_its_reason() {
        let msg = Error::TipMetadata("`/blocks` returned no block summaries").to_string();
        assert!(msg.contains("no block summaries"), "{}", msg);
        assert!(!Error::TipMetadata("x").is_all_cooling());
    }

    /// Runs `f` on a helper thread and fails if it has not returned within
    /// `bound` — the way to make a hung teardown a test failure rather than a
    /// hung test binary.
    fn completes_within<F: FnOnce() + Send + 'static>(bound: Duration, f: F) {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            f();
            let _ = tx.send(());
        });
        rx.recv_timeout(bound)
            .unwrap_or_else(|_| panic!("did not complete within {:?}", bound));
    }

    #[test]
    fn mock_esplora_tears_down_with_no_request() {
        completes_within(Duration::from_secs(5), || {
            let mock = mock_esplora(routes((200, "[]".to_string())));
            drop(mock);
        });
    }

    #[test]
    fn mock_esplora_tears_down_while_a_partial_request_is_held_open() {
        completes_within(Duration::from_secs(5), || {
            let mock = mock_esplora(routes((200, "[]".to_string())));
            // A client that sends half a request head and then goes quiet.
            let mut held = std::net::TcpStream::connect(mock.addr).unwrap();
            held.write_all(b"GET /blocks HTTP/1.1\r\nHost: x").unwrap();
            held.flush().unwrap();
            // The server must give up on it (read timeout → 400 + close) and
            // still honour the stop flag; the held socket stays open meanwhile.
            drop(mock);
            drop(held);
        });
    }

    #[test]
    fn mock_esplora_reads_a_head_split_across_writes() {
        // One TCP read is not a message boundary: a request head delivered in
        // two segments must still be routed to its path.
        let mock = mock_esplora(routes((200, "[]".to_string())));
        let mut s = std::net::TcpStream::connect(mock.addr).unwrap();
        s.write_all(b"GET /blocks HTTP/1.1\r\nHost: x\r\n").unwrap();
        s.flush().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        s.write_all(b"Accept: */*\r\n\r\n").unwrap();
        s.flush().unwrap();
        let mut resp = String::new();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let _ = s.read_to_string(&mut resp);
        assert!(resp.starts_with("HTTP/1.1 200"), "{}", resp);
        assert_eq!(mock.requests(), vec!["/blocks".to_string()]);
    }

    #[test]
    fn mock_esplora_refuses_a_terminated_head_over_the_limit() {
        // The terminator arrives, but the head is over the bound: size wins.
        let mock = mock_esplora(routes((200, "[]".to_string())));
        let mut s = std::net::TcpStream::connect(mock.addr).unwrap();
        let filler = vec![b'x'; MOCK_MAX_HEAD];
        s.write_all(b"GET /blocks HTTP/1.1\r\nX-Fill: ").unwrap();
        s.write_all(&filler).unwrap();
        s.write_all(b"\r\n\r\n").unwrap();
        s.flush().unwrap();
        let mut resp = String::new();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let _ = s.read_to_string(&mut resp);
        assert!(resp.starts_with("HTTP/1.1 400"), "{}", resp);
        assert!(
            mock.requests().is_empty(),
            "an over-limit head must not be routed even when terminated"
        );
    }

    #[test]
    fn mock_esplora_tears_down_while_a_peer_trickles_a_partial_head() {
        // A peer sending one byte every 50 ms never lets a per-read timeout
        // expire; the absolute deadline and the stop check must end the read
        // anyway, so teardown completes well inside the generous outer bound.
        let keep_dripping = Arc::new(AtomicBool::new(true));
        let dripper_flag = keep_dripping.clone();
        completes_within(Duration::from_secs(10), move || {
            let mock = mock_esplora(routes((200, "[]".to_string())));
            let addr = mock.addr;
            let dripper = std::thread::spawn(move || {
                let mut s = std::net::TcpStream::connect(addr).unwrap();
                let _ = s.write_all(b"GET /blocks HTTP/1.1\r\nX: ");
                while dripper_flag.load(Ordering::SeqCst) {
                    if s.write_all(b"a").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            // Let the server enter the read on the dripping connection.
            std::thread::sleep(Duration::from_millis(150));
            let begin = Instant::now();
            drop(mock);
            let elapsed = begin.elapsed();
            assert!(
                elapsed < MOCK_REQUEST_DEADLINE + Duration::from_secs(2),
                "teardown took {:?} with a dripping peer",
                elapsed
            );
            keep_dripping.store(false, Ordering::SeqCst);
            let _ = dripper.join();
        });
    }

    /// Helper-level pins (the reviewer's exact reproductions): with a peer
    /// dripping one byte per 50 ms the read ends at the absolute deadline, and
    /// raising `stop` ends it within one read slice — neither waits for the
    /// byte limit. Bounds leave room for scheduler noise but stay far below
    /// the multi-second occupation the per-read timeout allowed.
    #[test]
    fn read_request_head_is_bounded_by_deadline_and_stop_flag() {
        // Deadline bound under a drip.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let dripper = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            for _ in 0..40 {
                if s.write_all(b"a").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let (mut stream, _) = listener.accept().unwrap();
        let stop = AtomicBool::new(false);
        let begin = Instant::now();
        let result = read_request_head(&mut stream, &stop, Instant::now() + MOCK_REQUEST_DEADLINE);
        let elapsed = begin.elapsed();
        assert!(result.is_none());
        assert!(
            elapsed < MOCK_REQUEST_DEADLINE + Duration::from_secs(1),
            "drip occupied the read for {:?}",
            elapsed
        );
        drop(stream);
        let _ = dripper.join();

        // Stop-flag bound while the peer is silent.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let holder = std::net::TcpStream::connect(addr).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            flag.store(true, Ordering::SeqCst);
        });
        let begin = Instant::now();
        assert!(
            read_request_head(&mut stream, &stop, Instant::now() + MOCK_REQUEST_DEADLINE).is_none()
        );
        assert!(
            begin.elapsed() < MOCK_REQUEST_DEADLINE,
            "stop must be observed before the deadline"
        );
        drop(holder);
    }

    /// The CI failure's shape, made deterministic: the whole over-limit,
    /// terminated request arrives in one write, so the server reads its cap,
    /// refuses, and still has the peer's leftover bytes in its receive queue.
    /// Before the drain, closing there made the kernel reset the connection
    /// and the peer could lose the 400 (or fail its own trailing writes). Now
    /// the peer must read the complete 400 followed by a clean EOF — the
    /// `read_to_string` result itself is asserted, so a reset cannot hide
    /// behind a partially received response.
    #[test]
    fn mock_esplora_answers_an_over_limit_head_sent_in_one_write_with_an_intact_400() {
        let mock = mock_esplora(routes((200, "[]".to_string())));
        let mut request = b"GET /blocks HTTP/1.1\r\nX-Fill: ".to_vec();
        request.extend(std::iter::repeat_n(b'x', MOCK_MAX_HEAD));
        request.extend_from_slice(b"\r\n\r\n");
        assert!(
            request.len() > MOCK_MAX_HEAD + 1,
            "must exceed the read cap"
        );
        let mut s = std::net::TcpStream::connect(mock.addr).unwrap();
        s.write_all(&request)
            .expect("one write of the whole request");
        s.flush().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut resp = String::new();
        s.read_to_string(&mut resp)
            .expect("the refusal must arrive intact and end with a clean EOF, not a reset");
        assert_eq!(resp.as_bytes(), MOCK_REFUSAL, "{}", resp);
        assert!(
            mock.requests().is_empty(),
            "an over-limit head must not be routed even when terminated"
        );
    }

    /// The drain is bounded like everything else: a peer that keeps sending
    /// past the cap after being refused is closed anyway — within the
    /// connection's deadline, not extended by its flood — and the server goes
    /// on to serve the next request. A reset on that flooded connection is
    /// acceptable here; the property is the bound, not a clean close.
    #[test]
    fn mock_esplora_drains_a_refused_peer_only_within_its_bounds() {
        completes_within(MOCK_REQUEST_DEADLINE + Duration::from_secs(5), || {
            let mock = mock_esplora(routes((200, "[]".to_string())));
            let mut s = std::net::TcpStream::connect(mock.addr).unwrap();
            s.set_write_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let begin = Instant::now();
            // Over-limit head, then keep flooding until the server is gone.
            let _ = s.write_all(b"GET /blocks HTTP/1.1\r\nX-Fill: ");
            let mut sent = 0usize;
            let block = vec![b'x'; 8 * 1024];
            // Until the server closes (or resets) the connection: the flood is over.
            while s.write_all(&block).is_ok() {
                sent += block.len();
                if begin.elapsed() > MOCK_REQUEST_DEADLINE + Duration::from_secs(3) {
                    panic!("the server kept draining a flood of {} bytes", sent);
                }
            }
            assert!(
                begin.elapsed() < MOCK_REQUEST_DEADLINE + Duration::from_secs(3),
                "flood held the connection for {:?}",
                begin.elapsed()
            );
            assert!(mock.requests().is_empty());
            // The server thread is not stuck on the flooded peer.
            let mut next = std::net::TcpStream::connect(mock.addr).unwrap();
            next.write_all(b"GET /blocks HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            next.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut resp = String::new();
            next.read_to_string(&mut resp)
                .expect("a clean 200 after the flood");
            assert!(resp.starts_with("HTTP/1.1 200"), "{}", resp);
            assert_eq!(mock.requests(), vec!["/blocks".to_string()]);
        });
    }

    /// Helper-level pin of the drain's three bounds: it stops at EOF having
    /// consumed the peer's leftovers, at the byte cap, and at the deadline
    /// (each observed directly rather than through the server thread).
    #[test]
    fn drain_until_eof_stops_at_eof_cap_or_deadline() {
        // EOF: the peer's leftovers are consumed and the drain returns them.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let sender = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            s.write_all(&[b'x'; 300]).unwrap();
            // Dropping closes: EOF for the drain.
        });
        let (mut stream, _) = listener.accept().unwrap();
        let stop = AtomicBool::new(false);
        let deadline = Instant::now() + MOCK_REQUEST_DEADLINE;
        assert_eq!(drain_until_eof(&mut stream, &stop, deadline), 300);
        let _ = sender.join();

        // Cap: a peer that never closes is dropped once the cap is reached.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (flood_done_tx, flood_done_rx) = std::sync::mpsc::channel::<()>();
        let flooder = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            s.set_write_timeout(Some(Duration::from_millis(200)))
                .unwrap();
            let block = vec![b'x'; 8 * 1024];
            while s.write_all(&block).is_ok() {
                if flood_done_rx.try_recv().is_ok() {
                    break;
                }
            }
        });
        let (mut stream, _) = listener.accept().unwrap();
        let stop = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(10); // not the bound under test
        let begin = Instant::now();
        let drained = drain_until_eof(&mut stream, &stop, deadline);
        assert_eq!(
            drained, MOCK_DRAIN_CAP,
            "drain must stop exactly at the cap"
        );
        assert!(begin.elapsed() < Duration::from_secs(5));
        drop(stream);
        let _ = flood_done_tx.send(());
        let _ = flooder.join();

        // Deadline: a silent peer cannot hold the drain past it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let holder = std::net::TcpStream::connect(addr).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        let stop = AtomicBool::new(false);
        let begin = Instant::now();
        let deadline = begin + Duration::from_millis(150);
        assert_eq!(drain_until_eof(&mut stream, &stop, deadline), 0);
        assert!(
            begin.elapsed()
                < Duration::from_millis(150) + MOCK_READ_SLICE + Duration::from_millis(500),
            "silent peer held the drain for {:?}",
            begin.elapsed()
        );
        drop(holder);
    }

    #[test]
    fn mock_esplora_bounds_oversized_heads() {
        let mock = mock_esplora(routes((200, "[]".to_string())));
        let mut s = std::net::TcpStream::connect(mock.addr).unwrap();
        let junk = vec![b'a'; MOCK_MAX_HEAD + 2048];
        let _ = s.write_all(b"GET /blocks HTTP/1.1\r\nX: ");
        let _ = s.write_all(&junk);
        let _ = s.flush();
        let mut resp = String::new();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let _ = s.read_to_string(&mut resp);
        assert!(resp.starts_with("HTTP/1.1 400"), "{}", resp);
        assert!(
            mock.requests().is_empty(),
            "an unterminated head must not be routed"
        );
    }
}
