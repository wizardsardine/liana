//! Send-only recipient identification. No wallet state or payment amounts cross this boundary.
//! SDK errors, `unknown` and `incomplete` are silent; only a `tampered` verdict blocks a send.
use async_trait::async_trait;
use branta::{
    BrantaApiOptions, BrantaEnvironment, BrantaError, BrantaService, DestinationType, VerifyResult,
    VerifyStatus,
};
use coincube_core::miniscript::bitcoin::{Address, Network};
use iced::{
    futures::{stream, StreamExt},
    widget::image::Handle,
};
use std::{
    collections::VecDeque,
    fmt,
    io::Cursor,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::watch;

const TIMEOUT: Duration = Duration::from_secs(3);
const MAX_LOGO_BYTES: usize = 256 * 1024;
const MAX_LOGO_DIMENSION: u32 = 5000;
const LOGO_THUMBNAIL_DIMENSION: u32 = 64;
const MAX_REQUEST_BYTES: usize = 16 * 1024;
const MAX_IDENTITIES: usize = 4;
pub const MISMATCH_MESSAGE: &str =
    "This payment request’s address does not match its verification data. Do not send.";

#[derive(Clone, Copy)]
struct Preference {
    enabled: bool,
    epoch: u64,
}
struct Privacy {
    state: watch::Sender<Preference>,
}
impl Privacy {
    fn new(enabled: bool) -> Self {
        Self {
            state: watch::channel(Preference { enabled, epoch: 0 }).0,
        }
    }
    fn set(&self, enabled: bool) {
        self.state.send_if_modified(|s| {
            if s.enabled == enabled {
                return false;
            }
            s.enabled = enabled;
            s.epoch = s.epoch.wrapping_add(1);
            true
        });
    }
}
static PRIVACY: OnceLock<Arc<Privacy>> = OnceLock::new();
fn privacy() -> &'static Arc<Privacy> {
    PRIVACY.get_or_init(|| Arc::new(Privacy::new(true)))
}
/// Called before constructing any wallet panels. Does not initialise an SDK client.
pub fn set_enabled(enabled: bool) {
    privacy().set(enabled);
}
pub fn enabled() -> bool {
    privacy().state.borrow().enabled
}
/// Always explicit: the SDK silently falls back to staging when no environment is set.
pub fn environment(network: Network) -> BrantaEnvironment {
    if network == Network::Bitcoin {
        BrantaEnvironment::Production
    } else {
        BrantaEnvironment::Staging
    }
}
/// Send-only: no API key, HMAC secret or integration-metrics token is ever configured.
fn service_options(network: Network) -> BrantaApiOptions {
    BrantaApiOptions::new(environment(network))
}
/// Same prefixes the SDK classifies as BOLT11.
fn is_bolt11(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.starts_with("lnbc") || lower.starts_with("lntb") || lower.starts_with("lnbcrt")
}
/// Rejects any control byte or space. Applied to what Tenshu hands the SDK (a BOLT11
/// request, or a BIP21 path plus the forwarded parameters), never to ignored parameters.
fn has_control_or_space(raw: &str) -> bool {
    raw.bytes().any(|b| b <= 0x20 || b == 0x7f)
}
fn percent_decode(value: &str) -> String {
    percent_encoding::percent_decode_str(value)
        .decode_utf8_lossy()
        .into_owned()
}

fn should_forward_bitcoin_parameter(key: &str) -> Option<bool> {
    let forwarded_key = |key: &str| matches!(key, "branta_id" | "branta_secret" | "bc" | "tb");
    if key.contains('%') {
        // A percent-encoded spelling of an identifying key is ambiguous: reject it.
        if forwarded_key(&percent_decode(key).to_ascii_lowercase()) {
            return None;
        }
        return Some(false);
    }
    Some(forwarded_key(key))
}

/// Sensitive input deliberately has no derived Debug or serialization implementation.
#[derive(Clone)]
pub struct LookupRequest {
    raw: String,
    destination: String,
    kind: DestinationType,
}
impl fmt::Debug for LookupRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LookupRequest([redacted])")
    }
}
impl LookupRequest {
    pub fn bolt11(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if !raw.is_ascii() || has_control_or_space(raw) || raw.len() > MAX_REQUEST_BYTES {
            return None;
        }
        let dest = if raw
            .get(..10)
            .is_some_and(|s| s.eq_ignore_ascii_case("lightning:"))
        {
            &raw[10..]
        } else {
            raw
        };
        // Restrict to the route Tenshu will actually prepare. No URI alternate destinations.
        if dest.contains(['?', '#', '&', ':']) || !is_bolt11(dest) {
            return None;
        }
        Some(Self {
            raw: raw.into(),
            destination: dest.to_ascii_lowercase(),
            kind: DestinationType::Bolt11,
        })
    }
    pub fn bitcoin(raw: &str, address: &Address) -> Option<Self> {
        // Both the wallet's URL parser and the SDK drop interior tab, CR and LF, so do the
        // same first: all three then agree on every key and on the path being paid.
        let raw: String = raw
            .trim()
            .chars()
            .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
            .collect();
        if raw.len() > MAX_REQUEST_BYTES
            || !raw.get(..8)?.eq_ignore_ascii_case("bitcoin:")
            || raw.contains('#')
        {
            return None;
        }
        // Same split as the SDK's parser: path address, then the query.
        let (path, query) = raw[8..].split_once('?')?;
        if !path.is_ascii() || has_control_or_space(path) {
            return None;
        }
        let parsed: Address<coincube_core::miniscript::bitcoin::address::NetworkUnchecked> =
            path.parse().ok()?;
        if parsed.assume_checked().script_pubkey() != address.script_pubkey() {
            return None;
        }
        // VAULT pays only the path address, so the SDK sees only what identifies it: the
        // Branta pair and on-chain alternates (which must agree with the path or the
        // request is tampered). Other legs (`pay`, `lno`, `lightning`, ...) would make
        // the SDK return `unknown` before it decrypts and compares the address, hiding
        // a swapped path address. Parameters that are not forwarded are not inspected, so
        // an unencoded space or other noise in a `label` cannot switch the check off.
        let mut keys = std::collections::HashSet::new();
        let (mut id, mut secret) = (None, None);
        let mut forwarded = Vec::new();
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let key = key.to_ascii_lowercase();
            if !should_forward_bitcoin_parameter(&key)? {
                continue;
            }
            // Reject ambiguous or malformed identifying parameters before the SDK's
            // first-value-wins parser sees them.
            if !keys.insert(key.clone())
                || !pair.is_ascii()
                || has_control_or_space(pair)
                || !safe_percent_encoding(pair)
            {
                return None;
            }
            match key.as_str() {
                "branta_id" => id = Some(percent_decode(value)),
                "branta_secret" => secret = Some(percent_decode(value)),
                _ => (),
            }
            forwarded.push(pair);
        }
        if secret?.is_empty() {
            return None;
        }
        // The SDK sends branta_id as supplied. Ensure this is an encrypted
        // envelope, not a plaintext address placed in the ID field.
        use base64::Engine;
        let envelope = base64::engine::general_purpose::STANDARD.decode(id?).ok()?;
        if envelope.len() < 28 {
            return None;
        }
        Some(Self {
            raw: format!("bitcoin:{path}?{}", forwarded.join("&")),
            destination: address.to_string(),
            kind: DestinationType::BitcoinAddress,
        })
    }
}

#[derive(Clone)]
pub struct RecipientIdentity {
    pub platform: String,
    pub description: Option<String>,
    pub logo: Option<Handle>,
    pub logo_light: Option<Handle>,
    verify_url: String,
    privacy: Arc<Privacy>,
    epoch: u64,
}
impl fmt::Debug for RecipientIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RecipientIdentity([redacted])")
    }
}
impl RecipientIdentity {
    pub fn is_current(&self) -> bool {
        let s = self.privacy.state.borrow();
        s.enabled && s.epoch == self.epoch
    }
    pub fn open(&self) {
        if self.is_current() {
            let _ = crate::browser::open_url(&self.verify_url);
        }
    }
}
#[derive(Clone, Debug, Default)]
pub enum LookupResult {
    #[default]
    Silent,
    Identified(Vec<RecipientIdentity>),
    Tampered,
}

/// Narrow, injectable seam: automated tests never instantiate the official network backend.
#[async_trait]
pub trait LookupBackend: Send + Sync {
    async fn lookup(
        &self,
        network: Network,
        request: &LookupRequest,
    ) -> Result<VerifyResult, BrantaError>;
    async fn logo(&self, network: Network, url: &str) -> Option<Handle>;
}
struct OfficialBackend;
#[async_trait]
impl LookupBackend for OfficialBackend {
    async fn lookup(
        &self,
        network: Network,
        request: &LookupRequest,
    ) -> Result<VerifyResult, BrantaError> {
        // The complete original request is processed locally by the SDK. It transmits only
        // the branta_id or a derived lookup token; plain addresses make no request at all.
        BrantaService::new(service_options(network))
            .verify(&request.raw, None)
            .await
    }
    async fn logo(&self, network: Network, url: &str) -> Option<Handle> {
        fetch_logo(network, url).await
    }
}
#[derive(Clone)]
pub struct LookupTicket {
    privacy: Arc<Privacy>,
    epoch: u64,
    network: Network,
    backend: Arc<dyn LookupBackend>,
}
impl fmt::Debug for LookupTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LookupTicket")
    }
}
pub fn begin(network: Network) -> Option<LookupTicket> {
    ticket(privacy().clone(), network, Arc::new(OfficialBackend))
}
fn ticket(
    privacy: Arc<Privacy>,
    network: Network,
    backend: Arc<dyn LookupBackend>,
) -> Option<LookupTicket> {
    let state = *privacy.state.borrow();
    state.enabled.then_some(LookupTicket {
        privacy,
        epoch: state.epoch,
        network,
        backend,
    })
}
impl LookupTicket {
    pub fn is_current(&self) -> bool {
        let s = self.privacy.state.borrow();
        s.enabled && s.epoch == self.epoch
    }
    pub async fn lookup(&self, request: LookupRequest) -> LookupResult {
        let mut changes = self.privacy.state.subscribe();
        if !self.is_current() {
            return LookupResult::Silent;
        }
        tokio::select! {
            biased;
            _ = changes.changed() => LookupResult::Silent,
            result = self.lookup_inner(request, tokio::time::Instant::now() + TIMEOUT) => {
                if self.is_current() { result } else { LookupResult::Silent }
            }
        }
    }
    /// Bounded fanout, preserving the caller's row IDs including duplicate destinations.
    /// One deadline covers the entire batch, not three seconds per wave.
    pub async fn lookup_many(
        &self,
        requests: Vec<(usize, LookupRequest)>,
    ) -> Vec<(usize, LookupResult)> {
        let mut changes = self.privacy.state.subscribe();
        if !self.is_current() {
            return Vec::new();
        }
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        let mut pending = stream::iter(
            requests
                .into_iter()
                .map(|(id, request)| async move { (id, self.identify(request, deadline).await) }),
        )
        .buffer_unordered(3);
        let mut results = Vec::new();
        loop {
            tokio::select! {
                biased;
                _ = changes.changed() => return Vec::new(),
                next = pending.next() => match next { Some(item) => results.push(item), None => break }
            }
        }
        // Identify every recipient before optional images can consume the batch budget.
        let logos = stream::iter(
            results
                .into_iter()
                .map(|(id, (mut result, urls))| async move {
                    self.load_logos(&mut result, urls, deadline).await;
                    (id, result)
                }),
        )
        .buffer_unordered(3)
        .collect::<Vec<_>>();
        let results = tokio::select! {
            biased;
            _ = changes.changed() => return Vec::new(),
            results = logos => results,
        };
        if self.is_current() {
            results
        } else {
            Vec::new()
        }
    }
    async fn lookup_inner(
        &self,
        request: LookupRequest,
        deadline: tokio::time::Instant,
    ) -> LookupResult {
        let (mut result, urls) = self.identify(request, deadline).await;
        self.load_logos(&mut result, urls, deadline).await;
        if self.is_current() {
            result
        } else {
            LookupResult::Silent
        }
    }
    async fn identify(
        &self,
        request: LookupRequest,
        deadline: tokio::time::Instant,
    ) -> (LookupResult, Vec<(Option<String>, Option<String>)>) {
        if !self.is_current() || tokio::time::Instant::now() >= deadline {
            return (LookupResult::Silent, Vec::new());
        }
        let response =
            match tokio::time::timeout_at(deadline, self.backend.lookup(self.network, &request))
                .await
            {
                Ok(Ok(r)) => r,
                _ => return (LookupResult::Silent, Vec::new()),
            };
        match response.status {
            VerifyStatus::Tampered => return (LookupResult::Tampered, Vec::new()),
            VerifyStatus::Match => (),
            VerifyStatus::Unknown | VerifyStatus::Incomplete => {
                return (LookupResult::Silent, Vec::new())
            }
        }
        if !self.is_current() || !same_origin(self.network, &response.verify_url) {
            return (LookupResult::Silent, Vec::new());
        }
        let mut identities = Vec::new();
        let mut logo_urls = Vec::new();
        for payment in response
            .payments
            .into_iter()
            .filter(|p| {
                p.destinations.iter().any(|d| {
                    !d.is_encrypted
                        && d.r#type == Some(request.kind)
                        && destination_matches(&d.value, &request.destination, request.kind)
                })
            })
            .take(MAX_IDENTITIES)
        {
            let Some(platform) = payment.platform.as_deref().filter(|s| !s.trim().is_empty())
            else {
                continue;
            };
            if !self.is_current() {
                return (LookupResult::Silent, Vec::new());
            }
            logo_urls.push((payment.platform_logo_url, payment.platform_logo_light_url));
            identities.push(RecipientIdentity {
                platform: bounded_text(platform, 80),
                description: payment
                    .description
                    .as_deref()
                    .map(|s| bounded_text(s, 200))
                    .filter(|s| !s.is_empty()),
                logo: None,
                logo_light: None,
                verify_url: response.verify_url.clone(),
                privacy: self.privacy.clone(),
                epoch: self.epoch,
            });
        }
        if !self.is_current() || identities.is_empty() {
            (LookupResult::Silent, Vec::new())
        } else {
            (LookupResult::Identified(identities), logo_urls)
        }
    }
    async fn load_logos(
        &self,
        result: &mut LookupResult,
        urls: Vec<(Option<String>, Option<String>)>,
        deadline: tokio::time::Instant,
    ) {
        let LookupResult::Identified(identities) = result else {
            return;
        };
        if !self.is_current() || tokio::time::Instant::now() >= deadline {
            return;
        }
        let logos = async {
            let requests = urls
                .into_iter()
                .enumerate()
                .flat_map(|(index, (dark, light))| [(index, false, dark), (index, true, light)]);
            let mut pending = stream::iter(requests.map(|(index, light, url)| async move {
                (index, light, self.load_logo(url.as_deref()).await)
            }))
            .buffer_unordered(MAX_IDENTITIES * 2);
            // Preserve each completed variant even if another one misses the deadline.
            while let Some((index, light, handle)) = pending.next().await {
                if light {
                    identities[index].logo_light = handle;
                } else {
                    identities[index].logo = handle;
                }
            }
        };
        let _ = tokio::time::timeout_at(deadline, logos).await;
    }
    async fn load_logo(&self, url: Option<&str>) -> Option<Handle> {
        let url = url?;
        if !self.is_current() || !same_origin(self.network, url) {
            return None;
        }
        // lookup_inner bounds all optional logos by the remaining shared deadline.
        self.backend.logo(self.network, url).await
    }
}
// SDK query parsing uses byte slices for type detection. Reject malformed escapes
// and decoded non-ASCII before it can inspect alternate destinations.
fn safe_percent_encoding(raw: &str) -> bool {
    let mut bytes = raw.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let Some(a) = bytes.next().and_then(|b| (b as char).to_digit(16)) else {
                return false;
            };
            let Some(b) = bytes.next().and_then(|b| (b as char).to_digit(16)) else {
                return false;
            };
            if a * 16 + b > 127 {
                return false;
            }
        }
    }
    true
}
fn destination_matches(a: &str, b: &str, kind: DestinationType) -> bool {
    if kind == DestinationType::Bolt11 {
        return a.eq_ignore_ascii_case(b);
    }
    let parse = |s: &str| {
        s.parse::<Address<coincube_core::miniscript::bitcoin::address::NetworkUnchecked>>()
            .ok()
            .map(|a| a.assume_checked().script_pubkey())
    };
    matches!((parse(a), parse(b)), (Some(a), Some(b)) if a == b)
}
fn bounded_text(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| {
            !c.is_control() && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .take(max)
        .collect()
}
fn same_origin(network: Network, value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    let base = reqwest::Url::parse(environment(network).url()).expect("fixed HTTPS endpoint");
    url.scheme() == "https"
        && url.origin() == base.origin()
        && url.username().is_empty()
        && url.password().is_none()
}
type LogoCache = Mutex<VecDeque<(String, Handle)>>;
static LOGOS: OnceLock<LogoCache> = OnceLock::new();
static LOGO_DECODER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn logo_request_url(network: Network, value: &str) -> Option<reqwest::Url> {
    if !same_origin(network, value) {
        return None;
    }
    let mut url = reqwest::Url::parse(value).ok()?;
    // Branta's Active Storage blob URLs redirect to object storage. Its proxy
    // route serves the same signed blob without sending the client off-origin.
    if let Some(blob) = url
        .path()
        .strip_prefix("/rails/active_storage/blobs/redirect/")
    {
        let path = format!("/rails/active_storage/blobs/proxy/{blob}");
        url.set_path(&path);
    }
    Some(url)
}

async fn fetch_logo(network: Network, url: &str) -> Option<Handle> {
    let request_url = logo_request_url(network, url)?;
    let cache = LOGOS.get_or_init(|| Mutex::new(VecDeque::new()));
    if let Some((_, handle)) = cache.lock().ok()?.iter().find(|(key, _)| key == url) {
        return Some(handle.clone());
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(TIMEOUT)
        .build()
        .ok()?;
    let mut response = client.get(request_url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()?;
    let format = match content_type {
        "image/png" => image::ImageFormat::Png,
        "image/jpeg" => image::ImageFormat::Jpeg,
        _ => return None,
    };
    if response
        .content_length()
        .is_some_and(|len| len > MAX_LOGO_BYTES as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > MAX_LOGO_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    // Large but bounded originals must not block the async runtime or decode in
    // parallel across recipients. The permit remains held if the lookup expires
    // while its blocking decode finishes; no network work runs in this closure.
    let permit = LOGO_DECODER.acquire().await.ok()?;
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        decode_logo(&bytes, format)
    })
    .await
    .ok()??;
    let mut cache = cache.lock().ok()?;
    if cache.len() >= 32 {
        cache.pop_front();
    }
    cache.push_back((url.to_string(), handle.clone()));
    Some(handle)
}
fn decode_logo(bytes: &[u8], format: image::ImageFormat) -> Option<Handle> {
    if bytes.len() > MAX_LOGO_BYTES || image::guess_format(bytes).ok()? != format {
        return None;
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_LOGO_DIMENSION);
    limits.max_image_height = Some(MAX_LOGO_DIMENSION);
    limits.max_alloc = Some(100 * 1024 * 1024);
    reader.limits(limits);
    let image = reader
        .decode()
        .ok()?
        .thumbnail(LOGO_THUMBNAIL_DIMENSION, LOGO_THUMBNAIL_DIMENSION)
        .into_rgba8();
    Some(Handle::from_rgba(
        image.width(),
        image.height(),
        image.into_raw(),
    ))
}

#[cfg(test)]
pub fn test_ticket(enabled: bool) -> LookupTicket {
    LookupTicket {
        privacy: Arc::new(Privacy::new(enabled)),
        epoch: 0,
        network: Network::Bitcoin,
        backend: Arc::new(tests::Fake::default()),
    }
}
#[cfg(test)]
pub fn test_identity(ticket: &LookupTicket, platform: &str) -> RecipientIdentity {
    RecipientIdentity {
        platform: platform.into(),
        description: None,
        logo: None,
        logo_light: None,
        verify_url: "https://guardrail.branta.pro/v2/verify/test#secret-not-for-logs".into(),
        privacy: ticket.privacy.clone(),
        epoch: ticket.epoch,
    }
}
#[cfg(test)]
impl LookupTicket {
    pub fn set_test_enabled(&self, enabled: bool) {
        self.privacy.set(enabled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    pub(super) struct Fake {
        calls: AtomicUsize,
        logos: AtomicUsize,
        mode: u8,
    }
    #[async_trait]
    impl LookupBackend for Fake {
        async fn lookup(
            &self,
            _: Network,
            request: &LookupRequest,
        ) -> Result<VerifyResult, BrantaError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let verdict = |status| VerifyResult {
                status,
                payments: Vec::new(),
                verify_url: "https://guardrail.branta.pro/v2/verify/example".into(),
            };
            match self.mode {
                1 => return Ok(verdict(VerifyStatus::Tampered)),
                2 => return Ok(verdict(VerifyStatus::Incomplete)),
                3 => tokio::time::sleep(Duration::from_secs(10)).await,
                4 => return Ok(verdict(VerifyStatus::Unknown)),
                11 => return Err(BrantaError::RequestFailed { status: 503 }),
                _ => (),
            }
            let mut dest = branta::Destination::new(&request.destination, Some(request.kind));
            if self.mode == 5 {
                dest.is_encrypted = true;
            }
            if self.mode == 6 {
                dest.value = "lnbc-other".into();
            }
            if self.mode == 12 {
                dest.r#type = Some(DestinationType::BitcoinAddress);
            }
            Ok(VerifyResult {
                status: VerifyStatus::Match,
                payments: vec![branta::Payment {
                    platform: Some("Merchant".into()),
                    description: Some("An order".into()),
                    destinations: vec![dest],
                    platform_logo_url: Some("https://guardrail.branta.pro/logo.png".into()),
                    platform_logo_light_url: (matches!(self.mode, 7 | 9 | 10))
                        .then(|| "https://guardrail.branta.pro/light.png".into()),
                    ..Default::default()
                }],
                verify_url: "https://guardrail.branta.pro/v2/verify/example#secret".into(),
            })
        }
        async fn logo(&self, _: Network, url: &str) -> Option<Handle> {
            self.logos.fetch_add(1, Ordering::SeqCst);
            if self.mode == 7 {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            if self.mode == 9 || self.mode == 10 {
                let light = url.ends_with("light.png");
                if light == (self.mode == 9) {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                return Some(Handle::from_rgba(1, 1, vec![255; 4]));
            }
            if self.mode == 8 {
                tokio::time::sleep(Duration::from_millis(500)).await;
                return Some(Handle::from_rgba(1, 1, vec![255; 4]));
            }
            None
        }
    }
    fn fake_ticket(mode: u8) -> (LookupTicket, Arc<Fake>) {
        let fake = Arc::new(Fake {
            mode,
            ..Default::default()
        });
        (
            ticket(Arc::new(Privacy::new(true)), Network::Bitcoin, fake.clone()).unwrap(),
            fake,
        )
    }
    fn request() -> LookupRequest {
        LookupRequest::bolt11("lnbc1test").unwrap()
    }
    #[test]
    fn endpoints_and_unsupported_inputs() {
        assert_eq!(environment(Network::Bitcoin), BrantaEnvironment::Production);
        for n in [
            Network::Testnet,
            Network::Testnet4,
            Network::Signet,
            Network::Regtest,
        ] {
            assert_eq!(environment(n), BrantaEnvironment::Staging);
        }
        for raw in [
            "lno1offer",
            "user@example.org",
            "spark1address",
            "sp1address",
            "ark1address",
            "liquidnetwork:abc",
            "😀x",
            "lightning:lnbc1?branta_secret=x",
        ] {
            assert!(LookupRequest::bolt11(raw).is_none());
        }
        assert!(LookupRequest::bolt11("LIGHTNING:LNBC1TEST").is_some());
    }
    #[tokio::test]
    async fn positive_bolt11_and_logo_failure_are_nonblocking() {
        let (t, fake) = fake_ticket(0);
        let LookupResult::Identified(ids) = t.lookup(request()).await else {
            panic!("expected identity");
        };
        assert_eq!(ids[0].platform, "Merchant");
        assert!(ids[0].logo.is_none());
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fake.logos.load(Ordering::SeqCst), 1);
        assert!(!format!("{:?}", ids[0]).contains("secret"));
    }
    #[tokio::test]
    async fn logo_can_use_remaining_lookup_budget_and_disable_cancels_it() {
        let (t, _) = fake_ticket(8);
        let LookupResult::Identified(ids) = t.lookup(request()).await else {
            panic!("expected identity with a logo");
        };
        assert!(ids[0].logo.is_some());

        let (t, fake) = fake_ticket(7);
        let other = t.clone();
        let future = tokio::spawn(async move { other.lookup(request()).await });
        while fake.logos.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        t.set_test_enabled(false);
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(100), future)
                .await
                .unwrap()
                .unwrap(),
            LookupResult::Silent
        ));
    }
    #[tokio::test]
    async fn logo_deadline_preserves_identity_and_same_preference_does_not_cancel() {
        let (t, fake) = fake_ticket(7);
        let other = t.clone();
        let future = tokio::spawn(async move {
            other
                .lookup_inner(
                    request(),
                    tokio::time::Instant::now() + Duration::from_millis(50),
                )
                .await
        });
        while fake.logos.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        let mut changes = t.privacy.state.subscribe();
        t.set_test_enabled(true);
        assert!(!changes.has_changed().unwrap());
        let LookupResult::Identified(ids) = future.await.unwrap() else {
            panic!("logo deadline discarded identity")
        };
        assert_eq!(ids[0].platform, "Merchant");
        assert!(ids[0].logo.is_none() && ids[0].logo_light.is_none());
        t.set_test_enabled(false);
        assert!(changes.changed().await.is_ok());
        assert!(!ids[0].is_current());
    }
    #[tokio::test]
    async fn completed_logo_variant_survives_other_variant_timeout() {
        for mode in [9, 10] {
            let (t, _) = fake_ticket(mode);
            let LookupResult::Identified(ids) = t
                .lookup_inner(
                    request(),
                    tokio::time::Instant::now() + Duration::from_millis(100),
                )
                .await
            else {
                panic!("expected identity")
            };
            assert_eq!(ids[0].logo.is_some(), mode == 9);
            assert_eq!(ids[0].logo_light.is_some(), mode == 10);
        }
    }
    #[tokio::test]
    async fn stalled_logos_do_not_starve_later_recipient_lookups() {
        let (t, fake) = fake_ticket(7);
        let results = t
            .lookup_many((0..4).map(|id| (id, request())).collect())
            .await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 4);
        assert_eq!(results.len(), 4);
        assert!(results
            .iter()
            .all(|(_, result)| matches!(result, LookupResult::Identified(_))));
    }
    #[tokio::test]
    async fn disabled_makes_zero_lookup_and_logo_requests_and_invalidates_off_on() {
        let (t, fake) = fake_ticket(0);
        t.set_test_enabled(false);
        assert!(matches!(t.lookup(request()).await, LookupResult::Silent));
        assert_eq!(fake.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fake.logos.load(Ordering::SeqCst), 0);
        t.set_test_enabled(true);
        assert!(!t.is_current());
        assert!(matches!(t.lookup(request()).await, LookupResult::Silent));
    }
    #[tokio::test]
    async fn disabling_cancels_inflight_without_logo_request() {
        let (t, fake) = fake_ticket(3);
        let other = t.clone();
        let future = tokio::spawn(async move { other.lookup(request()).await });
        while fake.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        t.set_test_enabled(false);
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(100), future)
                .await
                .unwrap()
                .unwrap(),
            LookupResult::Silent
        ));
        assert_eq!(fake.logos.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn ordinary_errors_empty_unbound_and_timeout_are_silent_only_tampered_blocks() {
        for mode in [2, 3, 4, 5, 6, 11, 12] {
            let (t, _) = fake_ticket(mode);
            assert!(matches!(t.lookup(request()).await, LookupResult::Silent));
        }
        let (t, _) = fake_ticket(1);
        assert!(matches!(t.lookup(request()).await, LookupResult::Tampered));
    }
    #[tokio::test]
    async fn duplicate_requests_preserve_row_ids() {
        let (t, fake) = fake_ticket(0);
        let mut results = t.lookup_many(vec![(7, request()), (2, request())]).await;
        results.sort_by_key(|r| r.0);
        assert_eq!(results.iter().map(|r| r.0).collect::<Vec<_>>(), vec![2, 7]);
        assert_eq!(fake.calls.load(Ordering::SeqCst), 2);
    }
    #[test]
    fn logo_origins_decode_and_text_limits() {
        for url in [
            "http://guardrail.branta.pro/a",
            "https://evil.test/a",
            "https://guardrail.branta.pro.evil.test/a",
            "https://user@guardrail.branta.pro/a",
        ] {
            assert!(!same_origin(Network::Bitcoin, url));
        }
        assert!(!same_origin(
            Network::Regtest,
            "https://guardrail.branta.pro/a"
        ));
        assert!(decode_logo(b"not an image", image::ImageFormat::Png).is_none());
        assert_eq!(bounded_text("a\n\u{202e}b", 2), "ab");
        assert_eq!(bounded_text(&"z".repeat(300), 200).len(), 200);
    }
    #[test]
    fn active_storage_logos_use_same_origin_proxy_only() {
        let input = "https://guardrail.branta.pro/rails/active_storage/blobs/redirect/signed/logo%20dark.png?x=1";
        assert_eq!(
            logo_request_url(Network::Bitcoin, input).unwrap().as_str(),
            "https://guardrail.branta.pro/rails/active_storage/blobs/proxy/signed/logo%20dark.png?x=1"
        );
        assert!(logo_request_url(Network::Regtest, input).is_none());
        assert!(
            logo_request_url(Network::Bitcoin, "https://object-storage.test/logo.png").is_none()
        );
        let staging = input.replace("guardrail.branta.pro", "staging.guardrail.branta.pro");
        assert_eq!(
            logo_request_url(Network::Regtest, &staging)
                .unwrap()
                .host_str(),
            Some("staging.guardrail.branta.pro")
        );
        let ordinary = "https://guardrail.branta.pro/logo.png";
        assert_eq!(
            logo_request_url(Network::Bitcoin, ordinary)
                .unwrap()
                .as_str(),
            ordinary
        );
    }

    #[test]
    fn large_vendor_logo_becomes_bounded_thumbnail_and_oversize_is_refused() {
        fn png(width: u32, height: u32) -> Vec<u8> {
            let mut bytes = Cursor::new(Vec::new());
            use image::ImageEncoder;
            let image = image::RgbaImage::new(width, height);
            image::codecs::png::PngEncoder::new_with_quality(
                &mut bytes,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::NoFilter,
            )
            .write_image(
                image.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
            bytes.into_inner()
        }
        let bytes = png(4501, 4500);
        assert!(bytes.len() <= MAX_LOGO_BYTES);
        let handle = decode_logo(&bytes, image::ImageFormat::Png).unwrap();
        let Handle::Rgba {
            width,
            height,
            pixels,
            ..
        } = handle
        else {
            panic!("expected decoded thumbnail")
        };
        assert!((1..=64).contains(&width) && (1..=64).contains(&height));
        assert_eq!(pixels.len(), (width * height * 4) as usize);
        assert!(decode_logo(&bytes, image::ImageFormat::Jpeg).is_none());
        assert!(decode_logo(&png(5001, 1), image::ImageFormat::Png).is_none());
        assert!(decode_logo(&vec![0; MAX_LOGO_BYTES + 1], image::ImageFormat::Png).is_none());
    }
}

#[cfg(test)]
mod sdk_contract_tests {
    use super::*;
    use base64::Engine;
    const ADDRESS: &str = "bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg";
    const OTHER: &str = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
    /// Base64 with `+`, `/` and `=` padding, long enough to pass the envelope check.
    fn envelope() -> String {
        base64::engine::general_purpose::STANDARD.encode([0xfb, 0xff].repeat(16))
    }
    /// The SDK's HTTP client cannot be injected from outside the crate in 4.0.0. These run the
    /// real `verify` on paths that must finish before any request. The local environment means
    /// a regression fails fast instead of reaching a real Branta server.
    fn offline_sdk() -> BrantaService {
        BrantaService::new(BrantaApiOptions::new(BrantaEnvironment::Localhost))
    }
    #[test]
    fn backend_options_are_send_only_and_explicit() {
        for network in [
            Network::Bitcoin,
            Network::Testnet,
            Network::Testnet4,
            Network::Signet,
            Network::Regtest,
        ] {
            let options = service_options(network);
            assert_eq!(options.environment, Some(environment(network)));
            assert!(options.api_key.is_none());
            assert!(options.hmac_secret.is_none());
        }
    }
    #[tokio::test]
    async fn official_sdk_flags_conflicting_on_chain_addresses_before_lookup() {
        let uri = format!(
            "bitcoin:{ADDRESS}?bc={OTHER}&branta_id={}&branta_secret=key",
            envelope()
        );
        let result = offline_sdk().verify(&uri, None).await.unwrap();
        assert_eq!(result.status, VerifyStatus::Tampered);
    }
    #[tokio::test]
    async fn unverifiable_extra_legs_cannot_mask_tampering() {
        let address = ADDRESS
            .parse::<Address<coincube_core::miniscript::bitcoin::address::NetworkUnchecked>>()
            .unwrap()
            .require_network(Network::Bitcoin)
            .unwrap();
        // The SDK checks conflicting on-chain addresses before extra legs...
        let raw = format!(
            "bitcoin:{ADDRESS}?pay=1&bc={OTHER}&branta_id={}&branta_secret=key",
            envelope()
        );
        let direct = offline_sdk().verify(&raw, None).await.unwrap();
        assert_eq!(direct.status, VerifyStatus::Tampered);
        let raw = format!(
            "bitcoin:{ADDRESS}?pay=1&branta_id={}&branta_secret=key",
            envelope()
        );
        // ...but stops at a `pay=` leg with `unknown` before decrypting `branta_id`, which
        // would hide a swapped path address.
        let direct = offline_sdk().verify(&raw, None).await.unwrap();
        assert_eq!(direct.status, VerifyStatus::Unknown);
        // What the wrapper forwards has no such leg left to stop on.
        let request = LookupRequest::bitcoin(&raw, &address).unwrap();
        assert!(!request.raw.contains("pay="));
        let raw = format!(
            "bitcoin:{ADDRESS}?pay=1&bc={OTHER}&branta_id={}&branta_secret=key",
            envelope()
        );
        assert!(LookupRequest::bitcoin(&raw, &address).is_some_and(|r| r.raw.contains("bc=")));
    }
    #[tokio::test]
    async fn official_sdk_never_looks_up_plain_or_unbindable_destinations() {
        for input in [
            ADDRESS.to_string(),
            format!(
                "bitcoin:{ADDRESS}?lno=lno1offer&branta_id={}&branta_secret=key",
                envelope()
            ),
        ] {
            let result = offline_sdk().verify(&input, None).await.unwrap();
            assert_eq!(result.status, VerifyStatus::Unknown);
            assert!(result.payments.is_empty());
        }
    }
    #[test]
    fn request_preserves_original_bip21_and_rejects_ambiguous_or_plain_id() {
        let address = ADDRESS
            .parse::<Address<coincube_core::miniscript::bitcoin::address::NetworkUnchecked>>()
            .unwrap()
            .require_network(Network::Bitcoin)
            .unwrap();
        let encrypted = envelope();
        let raw = format!("bitcoin:{ADDRESS}?amount=0.001&branta_id={encrypted}&branta_secret=key");
        let request = LookupRequest::bitcoin(&raw, &address).unwrap();
        // Only the paid address, the Branta pair and on-chain alternates reach the SDK.
        assert_eq!(
            request.raw,
            format!("bitcoin:{ADDRESS}?branta_id={encrypted}&branta_secret=key")
        );
        let with_legs = format!(
            "bitcoin:{ADDRESS}?pay=1&lno=lno1offer&lightning=lnbc1x&bolt12=x&ark=x&sp=x&\
             bc={ADDRESS}&branta_id={encrypted}&branta_secret=key&label=Shop"
        );
        let request = LookupRequest::bitcoin(&with_legs, &address).unwrap();
        assert_eq!(
            request.raw,
            format!("bitcoin:{ADDRESS}?bc={ADDRESS}&branta_id={encrypted}&branta_secret=key")
        );
        assert!(!format!("{request:?}").contains("key"));
        let encoded = encrypted
            .replace('+', "%2B")
            .replace('/', "%2F")
            .replace('=', "%3D");
        assert!(LookupRequest::bitcoin(&raw.replace(&encrypted, &encoded), &address).is_some());
        // 4.0.0 compares bech32 case-insensitively on every network, so uppercase QR addresses
        // no longer need the 3.2.1-era rejection.
        let upper = format!(
            "bitcoin:{}?branta_id={encrypted}&branta_secret=key",
            ADDRESS.to_uppercase()
        );
        assert!(LookupRequest::bitcoin(&upper, &address).is_some());
        assert!(LookupRequest::bitcoin(&format!("{raw}&branta_id=duplicate"), &address).is_none());
        assert!(LookupRequest::bitcoin(
            &format!("bitcoin:{ADDRESS}?branta_id={ADDRESS}&branta_secret=key"),
            &address
        )
        .is_none());
        assert!(LookupRequest::bitcoin(
            &format!("bitcoin:{ADDRESS}?branta_id={encrypted}"),
            &address
        )
        .is_none());
        assert!(LookupRequest::bitcoin(
            &format!("bitcoin:{ADDRESS}?branta_id={encrypted}&branta_secret="),
            &address
        )
        .is_none());
        assert!(LookupRequest::bitcoin(
            &format!("bitcoin:{OTHER}?branta_id={encrypted}&branta_secret=key"),
            &address
        )
        .is_none());
        assert!(LookupRequest::bitcoin(&format!("{raw}&tb=%F0%9F%98%80"), &address).is_none());
        assert!(!safe_percent_encoding("x%z0"));
        // The SDK would strip the tab, see two branta_id keys and send the first (plain) one.
        let smuggled = format!(
            "bitcoin:{ADDRESS}?bran\tta_id={ADDRESS}&branta_id={encrypted}&branta_secret=key"
        );
        assert!(LookupRequest::bitcoin(&smuggled, &address).is_none());
        // Inside an identifying parameter, spaces and other controls are rejected.
        for control in [" ", "\u{7f}", "\u{1}"] {
            let raw = raw.replace("&branta_secret", &format!("{control}&branta_secret"));
            assert!(LookupRequest::bitcoin(&raw, &address).is_none());
        }
        // Tab, CR and LF are dropped first, as the wallet's parser and the SDK both do.
        for control in ["\t", "\r", "\n"] {
            let raw = raw.replace("&branta_secret", &format!("{control}&branta_secret"));
            let request = LookupRequest::bitcoin(&raw, &address).expect("normalized like the SDK");
            assert!(!request.raw.contains(['\t', '\r', '\n']));
        }
        let split_path = format!(
            "bitcoin:{}\n{}?{}",
            &ADDRESS[..10],
            &ADDRESS[10..],
            raw.split_once('?').unwrap().1
        );
        assert!(LookupRequest::bitcoin(&split_path, &address).is_some());
        // Noise in a parameter that is not forwarded must not switch the check off.
        for noise in [
            "label=Coffee Shop",
            "message=caf\u{e9}",
            "label=50%",
            "x%20y=1",
            "note=\u{7f}",
        ] {
            let request = LookupRequest::bitcoin(&format!("{raw}&{noise}"), &address)
                .unwrap_or_else(|| panic!("ignored parameter blocked the lookup: {:?}", noise));
            assert!(!request.raw.contains(noise), "{}", noise);
        }
        // A swapped path with a noisy label still reaches the SDK, which flags the mismatch.
        let swapped = format!(
            "bitcoin:{OTHER}?{}&label=x y",
            raw.split_once('?').unwrap().1
        );
        let other: Address = OTHER
            .parse::<Address<coincube_core::miniscript::bitcoin::address::NetworkUnchecked>>()
            .unwrap()
            .assume_checked();
        assert!(LookupRequest::bitcoin(&swapped, &other).is_some());
        // A percent-encoded spelling of an identifying key is ambiguous.
        assert!(LookupRequest::bitcoin(&format!("{raw}&branta%5Fid=x"), &address).is_none());
        assert!(LookupRequest::bolt11("lnbc1te\tst").is_none());
        assert!(LookupRequest::bolt11(" lnbc1test\n").is_some());
    }
}

#[cfg(test)]
mod vendor_integrity {
    #[test]
    fn vendored_sdk_matches_reviewed_source_and_documented_adapters() {
        use sha2::{Digest, Sha256};
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../vendor/branta");
        let sha = |path: &str| hex::encode(Sha256::digest(std::fs::read(root.join(path)).unwrap()));
        let hashes: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("UPSTREAM_SHA256.json")).unwrap())
                .unwrap();
        assert_eq!(hashes["commit"], "6292bbd0c6a720e5c9273a4f4e8b827097db6e5e");
        let files = hashes["files"].as_object().unwrap();
        let patched = hashes["patched_files"].as_object().unwrap();
        for (path, expected) in files {
            assert_eq!(
                sha(path),
                expected.as_str().unwrap(),
                "unreviewed SDK change: {path}"
            );
        }
        for (path, adapter) in patched {
            assert!(
                !files.contains_key(path),
                "{} listed as both upstream and patched",
                path
            );
            assert_eq!(
                sha(path),
                adapter["patched_sha256"].as_str().unwrap(),
                "unreviewed SDK patch: {path}"
            );
        }
        // Every vendored file is pinned or one of Tenshu's own records, so a new unreviewed
        // file (a build.rs, tests/ or benches/ that Cargo would auto-detect) cannot slip in.
        // A Cargo.lock or target/ left by running the SDK's own suite (the vendored crate is
        // its own Cargo root) is not compiled into the wallet and is ignored.
        let own = ["Cargo.toml", "TENSHU_PROVENANCE.md", "UPSTREAM_SHA256.json"];
        let mut pending = vec![root.clone()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    if path != root.join("target") {
                        pending.push(path);
                    }
                    continue;
                }
                let relative = path.strip_prefix(&root).unwrap().to_str().unwrap();
                let relative = relative.replace('\\', "/");
                assert!(
                    files.contains_key(&relative)
                        || patched.contains_key(&relative)
                        || own.contains(&relative.as_str())
                        || relative == "Cargo.lock",
                    "unpinned SDK file: {}",
                    relative
                );
            }
        }
        assert_eq!(
            sha("Cargo.toml"),
            hashes["adapted_manifest_sha256"].as_str().unwrap()
        );
    }
}
