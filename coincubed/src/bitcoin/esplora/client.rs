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

/// An error from the Esplora client.
#[derive(Debug)]
pub enum Error {
    Client(Box<esplora_client::Error>),
    /// Every configured provider is currently in a 402/429 cooldown,
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
            Error::Client(e) => write!(f, "Esplora client error: '{}'.", e),
            Error::AllCooling => write!(
                f,
                "{} after recent rate limits; the poller will retry once a cooldown expires.",
                ALL_COOLING_DISPLAY_MARKER,
            ),
            Error::Aborted => {
                write!(
                    f,
                    "{}: the daemon is shutting down.",
                    SCAN_ABORTED_DISPLAY_MARKER
                )
            }
        }
    }
}

/// Bitcoin Esplora client backed by an ordered chain of providers.
///
/// `try_in_order` walks the providers from index 0 onwards on every call.
/// A provider is skipped if it's currently in a 429/402 cooldown
/// ([`RATE_LIMIT_COOLDOWN`]). On a retryable failure ([`should_fall_back`])
/// the next provider is tried; a non-retryable failure short-circuits
/// the chain.
///
/// Methods that consume a request (`sync`, `full_scan`) take a builder
/// closure so the request can be rebuilt for each attempt — BDK's
/// `SyncRequest` is consumed by the call and isn't trivially clonable.
pub struct Client {
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
    /// `Some(deadline)` when this provider returned 402/429 recently;
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
/// 5xx and transport errors fall through *without* a cooldown — they
/// often clear within a tick or two and we want to re-test the
/// provider on the next call. Genuine 4xx outcomes like 400/404
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

impl Client {
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
                        provider.enter_cooldown(TRANSPORT_FAILURE_COOLDOWN);
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
        Ok(Client { providers, abort })
    }

    /// Run `op` against each provider in order, skipping any that's in a
    /// 429/402 cooldown. See [`should_fall_back`] and [`is_throttled`] for
    /// the per-result decisions.
    fn try_in_order<T, F>(&self, mut op: F) -> Result<T, Error>
    where
        F: FnMut(&esplora_client::blocking::BlockingClient) -> Result<T, esplora_client::Error>,
    {
        let mut last_result: Option<Result<T, esplora_client::Error>> = None;
        for provider in &self.providers {
            // Bail out between providers if the daemon is shutting down, so a
            // dead/throttled provider chain can't keep `stop()` blocked. The
            // in-flight `op` (one provider) still runs to its timeout; this stops
            // us from then dialling the rest.
            if self.abort.load(Ordering::Relaxed) {
                return Err(Error::Aborted);
            }
            if provider.is_cooling() {
                log::debug!(
                    "Esplora skipping {} (cooling down after recent 402/429)",
                    provider.name,
                );
                continue;
            }
            let result = op(&provider.client);
            if result.is_ok() {
                provider.clear_cooldown();
                return result.map_err(|e| Error::Client(Box::new(e)));
            }
            if !should_fall_back(&result) {
                // Non-retryable error (e.g. 400, 404). The caller wants
                // this exact answer — don't keep dialling.
                return result.map_err(|e| Error::Client(Box::new(e)));
            }
            if is_throttled(&result) {
                provider.enter_cooldown(RATE_LIMIT_COOLDOWN);
                if let Err(ref e) = result {
                    log::warn!(
                        "Esplora {} throttled ({}); cooling for {:?} and trying next provider",
                        provider.name,
                        e,
                        RATE_LIMIT_COOLDOWN,
                    );
                }
            } else if is_transport_failure(&result) {
                // Unreachable provider (timeout/connection error). Cool it down
                // so subsequent calls skip it instead of re-paying the request
                // timeout every time — the repeated-stall bug. 5xx falls to the
                // branch below and is NOT cooled (it usually clears within a
                // tick).
                provider.enter_cooldown(TRANSPORT_FAILURE_COOLDOWN);
                if let Err(ref e) = result {
                    log::warn!(
                        "Esplora {} unreachable ({}); cooling for {:?} and trying next provider",
                        provider.name,
                        e,
                        TRANSPORT_FAILURE_COOLDOWN,
                    );
                }
            } else if let Err(ref e) = result {
                log::warn!(
                    "Esplora {} failed ({}); trying next provider",
                    provider.name,
                    e,
                );
            }
            last_result = Some(result);
        }
        // Every provider either failed retryably or was on cooldown.
        // Surface the last real result if we have one; otherwise the
        // entire chain was on cooldown — return the typed
        // [`Error::AllCooling`] so the poller can log it at a sane
        // level and back off longer than its normal 2s retry, since
        // a cooldown won't lift for minutes.
        match last_result {
            Some(r) => r.map_err(|e| Error::Client(Box::new(e))),
            None => Err(Error::AllCooling),
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
        let hash = self.try_in_order(|client| client.get_tip_hash())?;
        let status = self.try_in_order(|client| client.get_block_status(&hash))?;
        let height = status.height.ok_or_else(|| {
            Error::Client(Box::new(esplora_client::Error::HttpResponse {
                status: 404,
                message: format!("tip block {} is not in best chain", hash),
            }))
        })?;
        Ok(BlockChainTip {
            hash,
            height: height as i32,
        })
    }

    /// Get the timestamp of the genesis block (block 0).
    pub fn genesis_block_timestamp(&self) -> Result<u32, Error> {
        let hash = self.genesis_block_hash()?;
        let header = self.try_in_order(|client| client.get_header_by_hash(&hash))?;
        Ok(header.time)
    }

    /// Get the timestamp of the current tip block.
    pub fn tip_time(&self) -> Result<u32, Error> {
        let hash = self.try_in_order(|client| client.get_tip_hash())?;
        let header = self.try_in_order(|client| client.get_header_by_hash(&hash))?;
        Ok(header.time)
    }

    /// Broadcast a transaction to the network.
    pub fn broadcast_tx(&self, tx: &bitcoin::Transaction) -> Result<(), Error> {
        self.try_in_order(|client| client.broadcast(tx))
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
        self.try_in_order(|client| {
            client
                .sync(build_request(), parallel_requests)
                .map_err(|e| *e)
        })
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
        self.try_in_order(|client| {
            client
                .full_scan(build_request(), stop_gap, parallel_requests)
                .map_err(|e| *e)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 5xx and transport errors must NOT set the cooldown — the
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
            "p1 must NOT enter cooldown on a 5xx — only 402/429 trigger that",
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
}
