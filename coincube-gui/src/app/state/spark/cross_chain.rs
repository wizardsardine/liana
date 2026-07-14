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

use coincube_spark_protocol::{CrossChainAddress, CrossChainQuote, CrossChainRoute};

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
        return Err(format!(
            "Slippage must be between {MIN_SLIPPAGE_BPS} and {MAX_SLIPPAGE_BPS} basis points \
             ({:.2}%–{:.0}%).",
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
        "evm" => "an EVM chain",
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
    format!(
        "Sending {} on {} — this address was detected as {}. \
         Funds sent to the wrong network cannot be recovered.",
        route.asset,
        route.chain,
        family_label(&address.family),
    )
}

/// How the quote's expiry should be presented, and whether sending is still
/// allowed.
///
/// The SDK hands back an ISO8601 `expires_at`. Past it, the quoted rate is void
/// and the provider will reject (or worse, re-price) the send — so the panel
/// counts down and blocks confirmation at zero rather than letting the user
/// send against a number that is no longer true.
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

/// Resolve a quote's `expires_at` against the current time.
pub fn quote_countdown(
    quote: &CrossChainQuote,
    now: chrono::DateTime<chrono::Utc>,
) -> QuoteCountdown {
    let Ok(expires) = chrono::DateTime::parse_from_rfc3339(&quote.expires_at) else {
        return QuoteCountdown::Unknown;
    };
    let seconds_left = (expires.with_timezone(&chrono::Utc) - now).num_seconds();
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
    /// The send carried an idempotency key the SDK honours — retrying with the
    /// same key cannot double-pay.
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
                "This send can be safely retried — it carries an idempotency key, \
                 so a retry cannot pay twice."
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
}
