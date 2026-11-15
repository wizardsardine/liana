//! Cross-chain stablecoin send — the decision logic, kept apart from the
//! panel's Iced plumbing so it can be tested without a running bridge.
//!
//! The user-facing shape of this feature is "send USDT/USDC to an address on
//! another chain, funded from your Bitcoin balance". Everything here exists to
//! keep three ways of losing money closed off:
//!
//! 1. **Sending to the wrong chain.** A Tron address and an EVM address are
//!    both opaque strings to a human, and USDT exists on both. The SDK tells us
//!    which family an address belongs to; [`chain_confirmation`] turns that
//!    into a sentence the user has to read before confirming.
//! 2. **Sending against a stale quote.** Cross-chain quotes expire. See
//!    [`QuoteCountdown`].
//! 3. **Paying twice on a retry.** See [`RetryPolicy`].

use coincube_spark_protocol::{
    CrossChainAddress, CrossChainQuote, CrossChainRoute, CrossChainRoutesOk,
};

/// SDK-enforced slippage bounds, in basis points. Mirrors
/// `MIN/MAX_CROSS_CHAIN_SLIPPAGE_BPS` in `breez-sdk-spark`; the bridge
/// re-checks, but validating here gives the user a real message instead of an
/// opaque SDK error.
pub const MIN_SLIPPAGE_BPS: u32 = 10;
pub const MAX_SLIPPAGE_BPS: u32 = 500;
/// What the SDK uses when we don't specify. Surfaced so the advanced
/// disclosure can show the default rather than an empty box.
pub const DEFAULT_SLIPPAGE_BPS: u32 = 100;

/// Cross-chain send is mainnet-only: the providers (Orchestra, Boltz) and the
/// destination chains are all real-money networks with no test deployment the
/// SDK will talk to. Spark itself also runs on regtest, so the panel's ordinary
/// availability check is not enough to gate this — hence a separate predicate.
pub fn supported_on(network: coincube_core::miniscript::bitcoin::Network) -> bool {
    matches!(
        network,
        coincube_core::miniscript::bitcoin::Network::Bitcoin
    )
}

/// Validate a user-entered slippage tolerance.
///
/// An empty string means "use the default" — not an error, so the advanced
/// disclosure can be opened and left alone.
pub fn parse_slippage_bps(input: &str) -> Result<Option<u32>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let bps: u32 = trimmed
        .parse()
        .map_err(|_| "Slippage must be a whole number of basis points.".to_string())?;
    if !(MIN_SLIPPAGE_BPS..=MAX_SLIPPAGE_BPS).contains(&bps) {
        return Err(coincube_ui::localized_format!(
            "Slippage must be between {MIN_SLIPPAGE_BPS} and {MAX_SLIPPAGE_BPS} basis points \
             ({:.2}%–{:.0}%).",
            "El deslizamiento debe estar entre {MIN_SLIPPAGE_BPS} y {MAX_SLIPPAGE_BPS} puntos \
             básicos ({:.2} %–{:.0} %).",
            MIN_SLIPPAGE_BPS as f64 / 100.0,
            MAX_SLIPPAGE_BPS as f64 / 100.0,
        ));
    }
    Ok(Some(bps))
}

/// Human name for an address family. Used in the confirmation sentence, so it
/// has to read as something a user recognises — "Ethereum / EVM", not "evm".
fn family_label(family: &str) -> &str {
    match family {
        "evm" => coincube_ui::locale::copy("an EVM chain"),
        "solana" => "Solana",
        "tron" => "Tron",
        other => other,
    }
}

/// The sentence the user must read before a cross-chain send.
///
/// This exists because the failure it prevents is unrecoverable. Address
/// formats do not announce their chain: a `T…` string is a Tron address, and
/// USDT-on-Tron and USDT-on-Ethereum are different assets on different
/// networks. Sending to the right *string* on the wrong *chain* burns the
/// funds. So the detected chain and asset are stated explicitly, in words,
/// rather than being left implicit in a dropdown the user will not read.
pub fn chain_confirmation(address: &CrossChainAddress, route: &CrossChainRoute) -> String {
    coincube_ui::localized_format!(
        "Sending {} on {} — this address was detected as {}. \
         Funds sent to the wrong network cannot be recovered.",
        "Enviando {} en {}: esta dirección se detectó como {}. \
         Los fondos enviados a la red equivocada no se pueden recuperar.",
        route.asset,
        route.chain,
        family_label(&address.family),
    )
}

/// Display name for a route's destination chain. The SDK reports lowercase
/// slugs (`"base"`, `"bsc"`) from one provider and display names
/// (`"Arbitrum One"`) from another; both should read as the network's name.
pub fn chain_name(chain: &str) -> String {
    let known = match chain.trim().to_ascii_lowercase().as_str() {
        "ethereum" | "eth" | "mainnet" => "Ethereum",
        "base" => "Base",
        "arbitrum" | "arbitrum one" => "Arbitrum",
        "optimism" | "op" => "Optimism",
        "polygon" | "matic" => "Polygon",
        "bsc" | "bnb" | "binance" => "BNB Smart Chain",
        "avalanche" | "avax" => "Avalanche",
        "solana" => "Solana",
        "tron" => "Tron",
        "plasma" => "Plasma",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    let mut chars = chain.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// What a typed destination turned out to be, for the Details step of a
/// stablecoin send — before Prepare, so the user sees which network the
/// address belongs to while they can still change it.
///
/// Detection asks the bridge (the same `get_cross_chain_routes` Prepare uses),
/// so it can never disagree with what Prepare will do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectedNetwork {
    /// The bridge doesn't read this as a cross-chain address.
    NotCrossChain,
    /// A cross-chain address, but no route delivers the picked coin there.
    Unreachable { family: String },
    /// The networks the picked coin can be delivered on, by display name,
    /// deduplicated and in the bridge's order.
    Chains { family: String, chains: Vec<String> },
}

/// Classify a routes lookup for the coin picked in THEY RECEIVE (`asset` is
/// `"USDT"` / `"USDC"`, compared case-insensitively).
pub fn detect_network(found: &CrossChainRoutesOk, asset: &str) -> DetectedNetwork {
    let Some(address) = &found.address else {
        return DetectedNetwork::NotCrossChain;
    };
    let mut chains: Vec<String> = Vec::new();
    for route in found
        .routes
        .iter()
        .filter(|r| r.asset.eq_ignore_ascii_case(asset))
    {
        let name = chain_name(&route.chain);
        if !chains.contains(&name) {
            chains.push(name);
        }
    }
    if chains.is_empty() {
        DetectedNetwork::Unreachable {
            family: address.family.clone(),
        }
    } else {
        DetectedNetwork::Chains {
            family: address.family.clone(),
            chains,
        }
    }
}

/// Join names as prose: "A", "A and B", "A, B and C".
fn join_names(names: &[String]) -> String {
    let and = coincube_ui::locale::copy(" and ");
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{}{and}{last}", init.join(", ")),
    }
}

/// The address family as a noun phrase for the detection line.
fn detected_family_label(family: &str) -> &str {
    match family {
        "evm" => coincube_ui::locale::copy("An EVM address"),
        "solana" => coincube_ui::locale::copy("A Solana address"),
        "tron" => coincube_ui::locale::copy("A Tron address"),
        _ => coincube_ui::locale::copy("A cross-chain address"),
    }
}

impl DetectedNetwork {
    /// What follows the coin on the THEY RECEIVE tile ("USDt on Tron").
    /// `None` keeps the generic "cross-chain" wording.
    pub fn tile_place(&self) -> Option<String> {
        match self {
            DetectedNetwork::Chains { chains, .. } if chains.len() <= 2 => Some(
                coincube_ui::localized_format!("on {}", "en {}", join_names(chains)),
            ),
            DetectedNetwork::Chains { chains, .. } => Some(coincube_ui::localized_format!(
                "on {} networks",
                "en {} redes",
                chains.len()
            )),
            _ => None,
        }
    }

    /// The line under the Destination field. `coin` is the picked coin's
    /// display label ("USDt").
    pub fn note(&self, coin: &str) -> String {
        match self {
            DetectedNetwork::NotCrossChain => coincube_ui::localized_format!(
                "This isn't a {coin} address on a supported network: Ethereum or another EVM \
                 chain, Tron, or Solana.",
                "Esta no es una dirección de {coin} en una red compatible: Ethereum u otra red \
                 EVM, Tron o Solana.",
            ),
            DetectedNetwork::Unreachable { family } => coincube_ui::localized_format!(
                "{} — no route can deliver {coin} there right now.",
                "{}: ahora mismo ninguna ruta puede entregar {coin} ahí.",
                detected_family_label(family),
            ),
            DetectedNetwork::Chains { family, chains } if chains.len() == 1 => {
                coincube_ui::localized_format!(
                    "{} — {coin} will be sent on {}.",
                    "{}: se enviarán {coin} en {}.",
                    detected_family_label(family),
                    chains[0],
                )
            }
            DetectedNetwork::Chains { family, chains } => coincube_ui::localized_format!(
                "{} — it works on {}. Choose the network the recipient uses for {coin}.",
                "{}: funciona en {}. Elige la red que usa el destinatario para {coin}.",
                detected_family_label(family),
                join_names(chains),
            ),
        }
    }

    /// Whether this is a definite "can't send here" — styled as a warning.
    pub fn is_problem(&self) -> bool {
        !matches!(self, DetectedNetwork::Chains { .. })
    }
}

/// A stablecoin destination as Details knows it: what the bridge said about
/// the typed address, and — once there is one — the network the user is
/// sending on.
///
/// Prepare quotes the route this names directly, so the network is settled on
/// Details, where it can still be changed, rather than on a separate
/// confirmation step after it.
#[derive(Debug, Clone)]
pub struct DetectedDestination {
    /// The trimmed destination this was looked up for. A lookup only counts
    /// while the field still holds exactly this.
    pub input: String,
    /// The parsed address, when the bridge read one.
    pub address: Option<CrossChainAddress>,
    /// The routes that deliver the picked coin, in the bridge's order.
    pub routes: Vec<CrossChainRoute>,
    pub network: DetectedNetwork,
    /// The chosen network's display name. Set on its own when only one
    /// network is possible; an EVM address that several reach waits for the
    /// user, because guessing the chain is how funds are lost.
    pub chosen: Option<String>,
}

impl DetectedDestination {
    /// Build from a routes lookup for `input`, keeping only routes for `asset`.
    pub fn new(input: String, found: CrossChainRoutesOk, asset: &str) -> Self {
        let network = detect_network(&found, asset);
        let routes: Vec<CrossChainRoute> = found
            .routes
            .into_iter()
            .filter(|r| r.asset.eq_ignore_ascii_case(asset))
            .collect();
        let chosen = match &network {
            DetectedNetwork::Chains { chains, .. } if chains.len() == 1 => Some(chains[0].clone()),
            _ => None,
        };
        Self {
            input,
            address: found.address,
            routes,
            network,
            chosen,
        }
    }

    /// The networks to choose between (display names), empty unless there
    /// are several.
    pub fn choices(&self) -> &[String] {
        match &self.network {
            DetectedNetwork::Chains { chains, .. } if chains.len() > 1 => chains,
            _ => &[],
        }
    }

    /// Choose a network by display name. Ignored for one this address can't
    /// use.
    pub fn choose(&mut self, chain: &str) {
        if self.choices().iter().any(|c| c == chain) {
            self.chosen = Some(chain.to_string());
        }
    }

    /// The route Prepare quotes: the bridge's first route on the chosen
    /// network.
    pub fn route(&self) -> Option<&CrossChainRoute> {
        let chosen = self.chosen.as_deref()?;
        self.routes.iter().find(|r| chain_name(&r.chain) == chosen)
    }

    /// Whether the user picked the network from several (an EVM address).
    /// Only then does Review ask them to confirm it: an address that belongs
    /// to one network already says where it goes.
    pub fn network_was_chosen(&self) -> bool {
        !self.choices().is_empty()
    }

    /// What follows the coin on the THEY RECEIVE tile.
    pub fn tile_place(&self) -> Option<String> {
        match &self.chosen {
            Some(chain) => Some(coincube_ui::localized_format!("on {}", "en {}", chain)),
            None => self.network.tile_place(),
        }
    }
}

/// How much more than a route's published minimum Details asks for.
///
/// The published bound is a floor, not a promise: the SDK documents that a
/// route can enforce a tighter one, the provider's fee comes out of the
/// amount, and a USD floor moves with the price between Details and the
/// quote. Sending the bare minimum was seen to fail (1,200 sats published,
/// 1,500 accepted), so Details asks for a quarter more and rounds up.
const MINIMUM_BUFFER_PERCENT: u64 = 25;

/// Sats for `cents` US cents at `btc_usd`, rounded up.
fn usd_cents_as_sats(cents: u64, btc_usd: f64) -> Option<u64> {
    if !btc_usd.is_finite() || btc_usd <= 0.0 {
        return None;
    }
    let sats = (cents as f64 / 100.0) / btc_usd * 100_000_000.0;
    sats.is_finite().then(|| sats.ceil() as u64)
}

/// The smallest amount Details lets through on `route`, in sats: the larger of
/// its published sats and USD floors, plus [`MINIMUM_BUFFER_PERCENT`], rounded
/// up to the next hundred. `None` when the route publishes no floor we can
/// price (a USD floor needs a BTC/USD price).
pub fn effective_min_sats(route: &CrossChainRoute, btc_usd: Option<f64>) -> Option<u64> {
    let limits = route.btc_limits?;
    let usd = limits
        .min_usd_cents
        .zip(btc_usd)
        .and_then(|(cents, price)| usd_cents_as_sats(cents, price));
    let floor = limits.min_sats.into_iter().chain(usd).max()?;
    let buffered = floor.saturating_add(floor.saturating_mul(MINIMUM_BUFFER_PERCENT).div_ceil(100));
    Some(buffered.div_ceil(100).saturating_mul(100))
}

/// The largest amount `route` publishes it accepts, in sats — no buffer, the
/// smaller of its sats and USD ceilings.
pub fn max_sats(route: &CrossChainRoute, btc_usd: Option<f64>) -> Option<u64> {
    let limits = route.btc_limits?;
    let usd = limits
        .max_usd_cents
        .zip(btc_usd)
        .and_then(|(cents, price)| {
            if !price.is_finite() || price <= 0.0 {
                return None;
            }
            let sats = (cents as f64 / 100.0) / price * 100_000_000.0;
            sats.is_finite().then(|| sats.floor() as u64)
        });
    limits.max_sats.into_iter().chain(usd).min()
}

/// Why an amount can't go on a route, before anything is quoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmountIssue {
    BelowMinimum { min_sats: u64 },
    AboveMaximum { max_sats: u64 },
}

/// Check `amount_sat` against `route`'s bounds.
pub fn amount_issue(
    route: &CrossChainRoute,
    amount_sat: u64,
    btc_usd: Option<f64>,
) -> Option<AmountIssue> {
    if let Some(min_sats) = effective_min_sats(route, btc_usd) {
        if amount_sat < min_sats {
            return Some(AmountIssue::BelowMinimum { min_sats });
        }
    }
    if let Some(max_sats) = max_sats(route, btc_usd) {
        if amount_sat > max_sats {
            return Some(AmountIssue::AboveMaximum { max_sats });
        }
    }
    None
}

impl AmountIssue {
    /// The line under the Amount field. `network` is the chosen network's
    /// display name.
    pub fn message(&self, network: &str) -> String {
        match self {
            AmountIssue::BelowMinimum { min_sats } => coincube_ui::localized_format!(
                "Too small for {network}. Send at least {} sats: that covers the route's \
                 minimum, the fee taken from the amount, and the price moving before the quote.",
                "Es muy poco para {network}. Envía al menos {} sats: así se cubren el mínimo \
                 de la ruta, la comisión que se descuenta del importe y la variación del precio \
                 antes de la cotización.",
                format_sats(*min_sats),
            ),
            AmountIssue::AboveMaximum { max_sats } => coincube_ui::localized_format!(
                "Too large for {network}. This route takes at most {} sats.",
                "Es demasiado para {network}. Esta ruta acepta como máximo {} sats.",
                format_sats(*max_sats),
            ),
        }
    }
}

fn format_sats(sats: u64) -> String {
    coincube_ui::component::amount::format_u64_as_string(sats, ",")
}

/// Whether a prepare error is the provider refusing the amount as too small
/// (`SdkError::CrossChainAmountOutOfRange` below its floor). The bridge passes
/// the SDK's message through as text, which leads with the provider's reason
/// and ends "(published minimum: …)".
pub fn is_amount_too_small_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("amount too small") || lower.contains("published minimum")
}

/// How the quote's expiry should be presented, and whether sending is still
/// allowed.
///
/// Past `expires_at`, the quoted rate is void and the provider will reject (or
/// worse, re-price) the send — so the panel counts down and blocks confirmation
/// at zero rather than letting the user send against a number that is no longer
/// true. See [`parse_expires_at`] for the two on-wire timestamp formats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuoteCountdown {
    /// Still valid, with this many whole seconds left.
    Valid { seconds_left: i64 },
    /// Expired — the panel must re-quote before it can send.
    Expired,
    /// `expires_at` didn't parse. Treated as expired: an unreadable expiry is
    /// not evidence of a *valid* quote, and forcing a re-quote costs the user a
    /// second, whereas sending against an unknown-age rate costs them money.
    Unknown,
}

impl QuoteCountdown {
    pub fn can_send(&self) -> bool {
        matches!(self, QuoteCountdown::Valid { .. })
    }
}

/// Parse a quote's `expires_at` into a UTC instant.
///
/// The SDK is not consistent across cross-chain providers: the **Orchestra**
/// path emits an RFC3339 timestamp (`2026-07-16T05:35:30Z`), while the **Boltz**
/// path emits a bare **Unix epoch seconds** string (`1784180130`) — see the
/// `validate_quote_expiry` helpers in each provider module of `breez-sdk-spark`.
/// The bridge passes whichever through verbatim, so accept both here; parsing
/// only RFC3339 read every boltz quote as already-expired.
fn parse_expires_at(expires_at: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let trimmed = expires_at.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    // Boltz: bare Unix epoch seconds.
    chrono::DateTime::from_timestamp(trimmed.parse().ok()?, 0)
}

/// Resolve a quote's `expires_at` against the current time.
pub fn quote_countdown(
    quote: &CrossChainQuote,
    now: chrono::DateTime<chrono::Utc>,
) -> QuoteCountdown {
    let Some(expires) = parse_expires_at(&quote.expires_at) else {
        return QuoteCountdown::Unknown;
    };
    let seconds_left = (expires - now).num_seconds();
    if seconds_left > 0 {
        QuoteCountdown::Valid { seconds_left }
    } else {
        QuoteCountdown::Expired
    }
}

/// What the panel may offer after a cross-chain send fails.
///
/// The distinction is not cosmetic. A send that failed *ambiguously* — a
/// dropped connection, a timeout — may or may not have moved funds. Whether it
/// is safe to press "try again" depends entirely on whether the source leg
/// honours an idempotency key:
///
/// - BTC-funded sends go out as a Spark transfer, which does. Re-sending with
///   the same key is a no-op if the first one landed.
/// - Token-funded sends go through `transfer_tokens`, which has **no**
///   idempotency hook. A blind retry can pay twice.
///
/// v1 only offers BTC-funded routes, so in practice this is always
/// [`RetryPolicy::SafeToRetry`] today. It is computed from the quote rather
/// than assumed, so that adding token sources later cannot silently start
/// offering an unsafe retry button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryPolicy {
    /// The route's source leg is BTC, so a retry can re-send the **same**
    /// prepared quote and the provider's swap-id dedup stops the BTC leg paying
    /// twice — while that quote is still valid. The panel downgrades to
    /// [`Self::MustCheckStateFirst`] once it expires (a re-prepare would mint a
    /// new swap id with no dedup against a maybe-landed attempt).
    SafeToRetry,
    /// No idempotency guarantee on this route's source leg. The panel must send
    /// the user to check the payment's real state instead of offering a retry.
    MustCheckStateFirst,
}

impl RetryPolicy {
    pub fn for_quote(quote: &CrossChainQuote) -> Self {
        if quote.retry_safe {
            RetryPolicy::SafeToRetry
        } else {
            RetryPolicy::MustCheckStateFirst
        }
    }

    /// Message shown alongside a failed cross-chain send.
    pub fn guidance(&self) -> &'static str {
        match self {
            RetryPolicy::SafeToRetry => {
                "Try again re-sends this same quote, so it can't pay twice."
            }
            RetryPolicy::MustCheckStateFirst => {
                "Check this payment's status in Transactions before retrying. \
                 This route cannot guarantee a retry won't send twice."
            }
        }
    }

    pub fn may_retry(&self) -> bool {
        matches!(self, RetryPolicy::SafeToRetry)
    }
}

/// Render an amount held in an asset's base units as a decimal string.
///
/// Cross-chain amounts are *not* sats: they're in the destination asset's base
/// units, with a per-asset scale (USDC on most chains has 6 decimals, not 8).
/// Formatting one with the sat helpers would misreport the amount by a factor
/// of 100. Hence a dedicated formatter that takes `decimals` from the route.
pub fn format_asset_amount(base_units: u128, decimals: u8) -> String {
    if decimals == 0 {
        return base_units.to_string();
    }
    let scale = 10u128.pow(decimals as u32);
    let whole = base_units / scale;
    let frac = base_units % scale;
    // Trim trailing zeros so "12.500000" reads as "12.5", but keep at least two
    // places so a stablecoin amount still looks like money.
    let frac_str = format!("{:0width$}", frac, width = decimals as usize);
    let trimmed = frac_str.trim_end_matches('0');
    let frac_display = if trimmed.len() < 2 {
        format!("{:0<2}", trimmed)
    } else {
        trimmed.to_string()
    };
    format!("{whole}.{frac_display}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::Network;

    fn route(asset: &str, chain: &str, decimals: u8) -> CrossChainRoute {
        CrossChainRoute {
            provider: "orchestra".to_string(),
            chain: chain.to_string(),
            chain_id: None,
            asset: asset.to_string(),
            contract_address: None,
            decimals,
            btc_source_supported: true,
            btc_limits: None,
        }
    }

    fn quote(expires_at: &str, retry_safe: bool) -> CrossChainQuote {
        CrossChainQuote {
            route: route("USDC", "base", 6),
            estimated_out: 0,
            fee_amount: 0,
            source_transfer_fee_sats: 0,
            expires_at: expires_at.to_string(),
            retry_safe,
        }
    }

    // ── Mainnet guard ───────────────────────────────────────────────────

    #[test]
    fn cross_chain_is_mainnet_only() {
        assert!(supported_on(Network::Bitcoin));
        // Spark itself runs on regtest, so this must be gated separately from
        // the panel's own availability check — the providers and destination
        // chains have no test deployment.
        assert!(!supported_on(Network::Regtest));
        assert!(!supported_on(Network::Signet));
        assert!(!supported_on(Network::Testnet));
        assert!(!supported_on(Network::Testnet4));
    }

    // ── Slippage bounds ─────────────────────────────────────────────────

    #[test]
    fn empty_slippage_means_use_the_default() {
        assert_eq!(parse_slippage_bps(""), Ok(None));
        assert_eq!(parse_slippage_bps("   "), Ok(None));
    }

    #[test]
    fn slippage_accepts_the_sdk_range_inclusive() {
        assert_eq!(parse_slippage_bps("10"), Ok(Some(10)));
        assert_eq!(parse_slippage_bps("100"), Ok(Some(100)));
        assert_eq!(parse_slippage_bps("500"), Ok(Some(500)));
    }

    #[test]
    fn slippage_rejects_out_of_range_and_nonsense() {
        // Just outside each bound — the off-by-one the SDK would reject.
        assert!(parse_slippage_bps("9").is_err());
        assert!(parse_slippage_bps("501").is_err());
        assert!(parse_slippage_bps("0").is_err());
        assert!(parse_slippage_bps("abc").is_err());
        assert!(parse_slippage_bps("1.5").is_err());
        // Negative parses fail rather than wrapping to a huge u32.
        assert!(parse_slippage_bps("-100").is_err());
    }

    // ── Quote expiry ────────────────────────────────────────────────────

    #[test]
    fn a_future_quote_is_valid_and_counts_down() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-14T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let q = quote("2026-07-14T12:00:30Z", true);
        assert_eq!(
            quote_countdown(&q, now),
            QuoteCountdown::Valid { seconds_left: 30 }
        );
        assert!(quote_countdown(&q, now).can_send());
    }

    #[test]
    fn an_elapsed_quote_blocks_sending() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-07-14T12:00:31Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let q = quote("2026-07-14T12:00:30Z", true);
        assert_eq!(quote_countdown(&q, now), QuoteCountdown::Expired);
        assert!(!quote_countdown(&q, now).can_send());
    }

    #[test]
    fn a_future_boltz_epoch_seconds_quote_is_valid_and_counts_down() {
        // Boltz carries expires_at as bare Unix epoch seconds, not RFC3339 —
        // the exact shape from the bug report. 1784180130 == 2026-07-16T05:35:30Z.
        let now = chrono::DateTime::from_timestamp(1_784_180_100, 0).unwrap();
        let q = quote("1784180130", true);
        assert_eq!(
            quote_countdown(&q, now),
            QuoteCountdown::Valid { seconds_left: 30 }
        );
        assert!(quote_countdown(&q, now).can_send());
    }

    #[test]
    fn an_elapsed_boltz_epoch_seconds_quote_is_expired() {
        let now = chrono::DateTime::from_timestamp(1_784_180_131, 0).unwrap();
        let q = quote("1784180130", true);
        assert_eq!(quote_countdown(&q, now), QuoteCountdown::Expired);
        assert!(!quote_countdown(&q, now).can_send());
    }

    #[test]
    fn an_unparseable_expiry_blocks_sending_rather_than_defaulting_open() {
        let now = chrono::Utc::now();
        // An unreadable expiry is not evidence the quote is *good*. Failing
        // toward "re-quote" costs a second; failing toward "send" costs money.
        assert_eq!(
            quote_countdown(&quote("", true), now),
            QuoteCountdown::Unknown
        );
        assert_eq!(
            quote_countdown(&quote("not-a-timestamp", true), now),
            QuoteCountdown::Unknown
        );
        assert!(!QuoteCountdown::Unknown.can_send());
    }

    // ── Retry safety ────────────────────────────────────────────────────

    #[test]
    fn a_retry_safe_quote_may_retry() {
        let policy = RetryPolicy::for_quote(&quote("2026-07-14T12:00:30Z", true));
        assert_eq!(policy, RetryPolicy::SafeToRetry);
        assert!(policy.may_retry());
    }

    #[test]
    fn a_token_sourced_send_must_never_offer_a_blind_retry() {
        // `transfer_tokens` has no idempotency hook in the SDK, so pressing
        // "try again" after an ambiguous failure could pay twice. The panel
        // must route the user to check payment state instead.
        let policy = RetryPolicy::for_quote(&quote("2026-07-14T12:00:30Z", false));
        assert_eq!(policy, RetryPolicy::MustCheckStateFirst);
        assert!(!policy.may_retry());
        assert!(policy.guidance().contains("before retrying"));
    }

    // ── Chain confirmation ──────────────────────────────────────────────

    #[test]
    fn confirmation_names_the_asset_and_chain_explicitly() {
        let address = CrossChainAddress {
            address: "TQ2XYZ".to_string(),
            family: "tron".to_string(),
            contract_address: None,
            chain_id: None,
            amount: None,
        };
        let msg = chain_confirmation(&address, &route("USDT", "tron", 6));
        // The whole point: a human cannot tell a Tron address by looking, so
        // the words "Tron" and "USDT" have to appear.
        assert!(msg.contains("USDT"));
        assert!(msg.contains("Tron"));
        assert!(msg.contains("cannot be recovered"));
    }

    // ── Asset amount formatting ─────────────────────────────────────────

    #[test]
    fn asset_amounts_scale_by_the_routes_decimals_not_by_sats() {
        // 12.5 USDC at 6 decimals. Formatting this with a sat helper (1e8)
        // would render 0.125 — off by 100x.
        assert_eq!(format_asset_amount(12_500_000, 6), "12.50");
        assert_eq!(format_asset_amount(1_000_000, 6), "1.00");
        // The same base-unit count at 18 decimals is a completely different
        // amount — dust, not a million. Rendered at full precision rather than
        // rounded to "0.00", so a wrong-decimals bug shows up as an absurd
        // number on screen instead of hiding behind a plausible zero.
        assert_eq!(format_asset_amount(1_000_000, 18), "0.000000000001");
        assert_eq!(format_asset_amount(0, 6), "0.00");
        assert_eq!(format_asset_amount(1_234_567, 6), "1.234567");
    }

    // ── Network detection on Details ────────────────────────────────────

    fn found(family: Option<&str>, routes: Vec<CrossChainRoute>) -> CrossChainRoutesOk {
        CrossChainRoutesOk {
            address: family.map(|family| CrossChainAddress {
                address: "addr".to_string(),
                family: family.to_string(),
                contract_address: None,
                chain_id: None,
                amount: None,
            }),
            routes,
        }
    }

    #[test]
    fn chain_names_read_as_network_names_whatever_the_provider_reports() {
        assert_eq!(chain_name("base"), "Base");
        assert_eq!(chain_name("Arbitrum One"), "Arbitrum");
        assert_eq!(chain_name("bsc"), "BNB Smart Chain");
        assert_eq!(chain_name("tron"), "Tron");
        assert_eq!(chain_name("somechain"), "Somechain");
        assert_eq!(chain_name(""), "");
    }

    #[test]
    fn a_tron_address_names_tron_on_the_tile_and_under_the_field() {
        let detected = detect_network(&found(Some("tron"), vec![route("USDT", "tron", 6)]), "USDT");
        assert_eq!(
            detected,
            DetectedNetwork::Chains {
                family: "tron".into(),
                chains: vec!["Tron".into()],
            }
        );
        assert_eq!(detected.tile_place().as_deref(), Some("on Tron"));
        let note = detected.note("USDt");
        assert!(note.contains("Tron address") && note.contains("USDt will be sent on Tron"));
        assert!(!detected.is_problem());
    }

    #[test]
    fn an_evm_address_lists_only_the_picked_coins_chains_once_each() {
        let detected = detect_network(
            &found(
                Some("evm"),
                vec![
                    route("USDT", "ethereum", 6),
                    route("USDC", "base", 6),
                    route("USDT", "arbitrum", 6),
                    route("USDT", "Arbitrum One", 6),
                    route("usdt", "bsc", 18),
                ],
            ),
            "USDT",
        );
        let DetectedNetwork::Chains { chains, .. } = &detected else {
            panic!("expected chains, got {:?}", detected);
        };
        assert_eq!(chains, &["Ethereum", "Arbitrum", "BNB Smart Chain"]);
        // Too many for the tile: it gives a count and the note lists them.
        assert_eq!(detected.tile_place().as_deref(), Some("on 3 networks"));
        let note = detected.note("USDt");
        assert!(note.contains("Ethereum, Arbitrum and BNB Smart Chain"));
        assert!(note.contains("Choose the network the recipient uses"));
    }

    #[test]
    fn two_chains_fit_on_the_tile() {
        let detected = detect_network(
            &found(
                Some("evm"),
                vec![route("USDC", "ethereum", 6), route("USDC", "base", 6)],
            ),
            "USDC",
        );
        assert_eq!(
            detected.tile_place().as_deref(),
            Some("on Ethereum and Base")
        );
    }

    #[test]
    fn an_address_no_route_reaches_is_a_problem_with_no_network_on_the_tile() {
        let detected = detect_network(
            &found(Some("solana"), vec![route("USDC", "solana", 6)]),
            "USDT",
        );
        assert_eq!(
            detected,
            DetectedNetwork::Unreachable {
                family: "solana".into()
            }
        );
        assert!(detected.is_problem());
        assert_eq!(detected.tile_place(), None);
        assert!(detected.note("USDt").contains("no route can deliver USDt"));
    }

    #[test]
    fn a_non_cross_chain_destination_lists_the_supported_networks() {
        let detected = detect_network(&found(None, vec![]), "USDT");
        assert_eq!(detected, DetectedNetwork::NotCrossChain);
        assert!(detected.is_problem());
        assert_eq!(detected.tile_place(), None);
        let note = detected.note("USDt");
        assert!(note.contains("Ethereum") && note.contains("Tron") && note.contains("Solana"));
    }

    // ── Route minimums on Details ───────────────────────────────────────

    fn limited(min_sats: Option<u64>, min_usd_cents: Option<u64>) -> CrossChainRoute {
        CrossChainRoute {
            btc_limits: Some(coincube_spark_protocol::CrossChainRouteLimits {
                min_sats,
                min_usd_cents,
                ..Default::default()
            }),
            ..route("USDT", "solana", 6)
        }
    }

    #[test]
    fn the_minimum_asked_for_carries_a_quarter_more_rounded_up() {
        // The case seen in use: 1,200 published, 1,200 refused, 1,500 sent.
        assert_eq!(
            effective_min_sats(&limited(Some(1_200), None), None),
            Some(1_500)
        );
        assert_eq!(
            effective_min_sats(&limited(Some(1_000), None), None),
            Some(1_300)
        );
        // The larger floor wins: 0.80 USD at 50,000 is 1,600 sats → 2,000.
        assert_eq!(
            effective_min_sats(&limited(Some(1_200), Some(80)), Some(50_000.0)),
            Some(2_000)
        );
        // A USD floor needs a price; without one only the sats floor counts.
        assert_eq!(effective_min_sats(&limited(None, Some(80)), None), None);
        assert_eq!(effective_min_sats(&route("USDT", "solana", 6), None), None);
    }

    #[test]
    fn amounts_are_checked_against_both_ends() {
        let mut capped = limited(Some(1_200), None);
        capped.btc_limits.as_mut().unwrap().max_sats = Some(100_000);
        assert_eq!(
            amount_issue(&capped, 1_499, None),
            Some(AmountIssue::BelowMinimum { min_sats: 1_500 })
        );
        assert_eq!(amount_issue(&capped, 1_500, None), None);
        assert_eq!(
            amount_issue(&capped, 100_001, None),
            Some(AmountIssue::AboveMaximum { max_sats: 100_000 })
        );
        assert!(AmountIssue::BelowMinimum { min_sats: 1_500 }
            .message("Solana")
            .contains("at least 1,500 sats"));
    }

    #[test]
    fn the_providers_too_small_refusal_is_recognised() {
        assert!(is_amount_too_small_error(
            "Spark bridge error (sdk): Amount too small (published minimum: 1200 base units or \
             0.80 USD)"
        ));
        assert!(!is_amount_too_small_error(
            "Amount too large (published maximum: 9.00 USD)"
        ));
    }

    #[test]
    fn one_network_is_chosen_for_you_several_are_not() {
        let single = DetectedDestination::new(
            "T".into(),
            found(Some("tron"), vec![route("USDT", "tron", 6)]),
            "USDT",
        );
        assert_eq!(single.chosen.as_deref(), Some("Tron"));
        assert!(!single.network_was_chosen());
        assert_eq!(single.route().map(|r| r.chain.as_str()), Some("tron"));

        let mut evm = DetectedDestination::new(
            "0x".into(),
            found(
                Some("evm"),
                vec![route("USDT", "ethereum", 6), route("USDT", "base", 6)],
            ),
            "USDT",
        );
        assert_eq!(evm.chosen, None);
        assert!(evm.route().is_none());
        assert!(evm.network_was_chosen());
        evm.choose("Base");
        assert_eq!(evm.route().map(|r| r.chain.as_str()), Some("base"));
        assert_eq!(evm.tile_place().as_deref(), Some("on Base"));
    }
}
