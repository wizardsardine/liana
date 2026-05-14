//! Placeholder Spark asset registry.
//!
//! Phase 2 scope: Bitcoin + Lightning only. Spark SDK 0.13.1 also ships
//! Spark-native tokens (BTKN) and a Stable Balance feature (USDB under
//! the hood), but those are invisible to the user in our Phase 6 plan —
//! Stable Balance surfaces as a Settings toggle, not a user-visible asset,
//! and BTKN has no shipping use case for us yet.
//!
//! This module exists so future phases have a stable place to extend the
//! asset list. The `list_assets()` accessor returns an owned `Vec` so
//! callers don't have to care whether the list is static or derived from
//! backend state (it'll need to pull from the bridge once per-cube asset
//! discovery is wired).

/// A Spark-side asset that the UI can display in a picker / balance row.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum SparkAsset {
    /// Native Spark Bitcoin balance.
    Bitcoin,
    /// Lightning payments (BOLT11 / BOLT12 / LNURL). Rendered as a
    /// separate asset even though it's routed over Bitcoin under the
    /// hood, matching how the UI already treats Liquid's Lightning
    /// method.
    Lightning,
}

impl SparkAsset {
    /// Short label for display in pickers (not localized).
    pub fn label(self) -> &'static str {
        match self {
            Self::Bitcoin => "BTC",
            Self::Lightning => "Lightning",
        }
    }
}

/// Return the full asset list the Spark panels should render today.
///
/// Phase 6 will add a Stable Balance toggle in Settings (not an asset
/// entry) and later phases may add more Spark-native assets here.
pub fn list_assets() -> Vec<SparkAsset> {
    vec![SparkAsset::Bitcoin, SparkAsset::Lightning]
}

/// Largest base-10 exponent that fits in a `u64` (10^19 ≤ u64::MAX < 10^20).
/// Real-world token metadata uses single-digit decimals; clamping rather
/// than panicking shields the formatter from a malformed `decimals`
/// coming back from the bridge.
pub(crate) const MAX_TOKEN_DECIMALS_U64: u32 = 19;

/// Format a token base-unit amount with two decimal places of
/// fractional precision. Mirrors [`crate::app::breez_liquid::assets::format_usdt_display`]
/// but parameterized on the token's own decimals (USDB ships at 6,
/// future tokens may differ). Half-up rounding with carry to keep the
/// output stable when the fractional rounds up to ten.
pub fn format_token_display(amount: u64, decimals: u32) -> String {
    const DISPLAY_DECIMALS: u32 = 2;
    if decimals == 0 {
        return amount.to_string();
    }
    // Clamp once at the top so every subsequent `10_u64.pow(...)`
    // stays within u64 range. 10^19 fits in u64; 10^20 would panic in
    // debug and wrap in release.
    let decimals = decimals.min(MAX_TOKEN_DECIMALS_U64);
    let divisor = 10_u64.pow(decimals);
    let mut whole = amount / divisor;
    let frac = amount % divisor;
    if decimals <= DISPLAY_DECIMALS {
        let scale = 10_u64.pow(DISPLAY_DECIMALS - decimals);
        return format!(
            "{}.{:0>width$}",
            whole,
            frac * scale,
            width = DISPLAY_DECIMALS as usize
        );
    }
    let scale = 10_u64.pow(decimals - DISPLAY_DECIMALS);
    let mut frac_2dp = (frac + scale / 2) / scale;
    let carry_threshold = 10_u64.pow(DISPLAY_DECIMALS);
    if frac_2dp >= carry_threshold {
        frac_2dp = 0;
        whole += 1;
    }
    format!(
        "{}.{:0>width$}",
        whole,
        frac_2dp,
        width = DISPLAY_DECIMALS as usize
    )
}

/// Convert a USDB-style stable token holding (a 1:1 USD peg) into its
/// sat-equivalent at the supplied BTC/USD price. Used by the overview
/// to fold Stable Balance into the unified portfolio total — same
/// pattern as Liquid's `usdt_balance` → `usdt_as_sats` calculation.
/// Returns `0` when there's no price reference yet.
pub fn stable_token_as_sats(amount: u64, decimals: u32, btc_usd_price: Option<f64>) -> u64 {
    let Some(price) = btc_usd_price.filter(|p| *p > 0.0) else {
        return 0;
    };
    // Same clamp as `format_token_display`: keeps `decimals as i32`
    // well-defined and `10_f64.powi(decimals)` far away from the f64
    // ~10^308 ceiling. Anything past 19 is already implausible token
    // metadata and would saturate the formatter anyway.
    let decimals = decimals.min(MAX_TOKEN_DECIMALS_U64);
    let usd_value = amount as f64 / 10_f64.powi(decimals as i32);
    (usd_value / price * 100_000_000.0) as u64
}
