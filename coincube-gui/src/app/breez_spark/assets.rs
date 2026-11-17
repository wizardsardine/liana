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

use coincube_ui::component::amount::format_u64_as_string;

/// A Spark-side asset that the UI can display in a picker / balance row.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum SparkAsset {
    /// Native Spark Bitcoin balance.
    Bitcoin,
    /// Lightning payments (BOLT11 / BOLT12 / LNURL). Rendered as a
    /// separate asset even though it's routed over Bitcoin under the
    /// hood.
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
/// fractional precision, parameterized on the token's own decimals (USDB ships at 6,
/// future tokens may differ). Half-up rounding with carry to keep the
/// output stable when the fractional rounds up to ten.
pub fn format_token_display(amount: u64, decimals: u32) -> String {
    format_token_display_u128(u128::from(amount), decimals)
}

/// [`format_token_display`] at the SDK's full `u128` token precision, for
/// the USDB balance (`UsdbBalanceSnapshot`), which the protocol keeps
/// unclamped.
pub fn format_token_display_u128(amount: u128, decimals: u32) -> String {
    const DISPLAY_DECIMALS: u32 = 2;
    if decimals == 0 {
        return group_thousands(amount);
    }
    // Clamp so `10_u128.pow(...)` stays in range; real token metadata uses
    // single-digit decimals, and the u64 path has always clamped at 19.
    let decimals = decimals.min(MAX_TOKEN_DECIMALS_U64);
    let divisor = 10_u128.pow(decimals);
    let mut whole = amount / divisor;
    let frac = amount % divisor;
    if decimals <= DISPLAY_DECIMALS {
        let scale = 10_u128.pow(DISPLAY_DECIMALS - decimals);
        return format!(
            "{}.{:0>width$}",
            group_thousands(whole),
            frac * scale,
            width = DISPLAY_DECIMALS as usize
        );
    }
    let scale = 10_u128.pow(decimals - DISPLAY_DECIMALS);
    let mut frac_2dp = (frac + scale / 2) / scale;
    let carry_threshold = 10_u128.pow(DISPLAY_DECIMALS);
    if frac_2dp >= carry_threshold {
        frac_2dp = 0;
        whole += 1;
    }
    format!(
        "{}.{:0>width$}",
        group_thousands(whole),
        frac_2dp,
        width = DISPLAY_DECIMALS as usize
    )
}

/// `value` with "," between thousands, as `format_u64_as_string` does,
/// for values past `u64::MAX` too.
fn group_thousands(value: u128) -> String {
    match std::convert::TryFrom::try_from(value) {
        Ok(value) => format_u64_as_string(value, ","),
        Err(_) => {
            let digits = value.to_string();
            let mut out = String::with_capacity(digits.len() + digits.len() / 3);
            for (i, digit) in digits.chars().enumerate() {
                if i > 0 && (digits.len() - i).is_multiple_of(3) {
                    out.push(',');
                }
                out.push(digit);
            }
            out
        }
    }
}

/// Convert a USDB-style stable token holding (a 1:1 USD peg) into its
/// sat-equivalent at the supplied BTC/USD price. Used by the overview
/// to fold Stable Balance into the unified portfolio total.
/// Returns `0` when there's no price reference yet.
pub fn stable_token_as_sats(amount: u64, decimals: u32, btc_usd_price: Option<f64>) -> u64 {
    stable_token_as_sats_u128(u128::from(amount), decimals, btc_usd_price)
}

/// [`stable_token_as_sats`] for a full-precision `u128` holding.
pub fn stable_token_as_sats_u128(amount: u128, decimals: u32, btc_usd_price: Option<f64>) -> u64 {
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

/// The USDB the GUI shows and values: the bridge's validated
/// `GetInfoOk::usdb_balance`, with a zero balance read as none (S1b). The
/// legacy `stable_balance` is never a fallback: it isn't checked against the
/// configured token's metadata.
pub fn usdb_holding(
    info: &coincube_spark_protocol::GetInfoOk,
) -> Option<coincube_spark_protocol::UsdbBalanceSnapshot> {
    info.usdb_balance.clone().filter(|usdb| usdb.balance > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_assets_have_stable_order_and_labels() {
        assert_eq!(
            list_assets(),
            vec![SparkAsset::Bitcoin, SparkAsset::Lightning]
        );
        assert_eq!(SparkAsset::Bitcoin.label(), "BTC");
        assert_eq!(SparkAsset::Lightning.label(), "Lightning");
    }

    #[test]
    fn full_precision_usdb_formats_and_values_like_the_u64_path() {
        for (amount, decimals) in [(0u64, 6u32), (12_400_000, 6), (999_995, 6), (42, 0)] {
            assert_eq!(
                format_token_display_u128(u128::from(amount), decimals),
                format_token_display(amount, decimals)
            );
            assert_eq!(
                stable_token_as_sats_u128(u128::from(amount), decimals, Some(50_000.0)),
                stable_token_as_sats(amount, decimals, Some(50_000.0))
            );
        }
        // Past u64::MAX the whole part keeps its digits and grouping.
        let big = u128::from(u64::MAX) * 1_000_000 + 1_234_567;
        assert_eq!(
            format_token_display_u128(big, 6),
            "18,446,744,073,709,551,616.23"
        );
    }

    #[test]
    fn usdb_holding_reads_only_the_validated_field() {
        use coincube_spark_protocol::{GetInfoOk, StableBalanceSnapshot, UsdbBalanceSnapshot};
        let usdb = |balance: u128| UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance,
            decimals: 6,
            ticker: "USDB".into(),
        };
        let legacy = Some(StableBalanceSnapshot {
            balance: 7_000_000,
            decimals: 6,
            ticker: "USDB".into(),
        });
        let info = |usdb_balance| GetInfoOk {
            balance_sats: 1,
            identity_pubkey: String::new(),
            stable_balance: legacy.clone(),
            usdb_balance,
            recoverable_funds_sats: 0,
        };
        assert_eq!(
            usdb_holding(&info(Some(usdb(5_000_000)))),
            Some(usdb(5_000_000))
        );
        assert_eq!(usdb_holding(&info(Some(usdb(0)))), None, "zero is none");
        assert_eq!(
            usdb_holding(&info(None)),
            None,
            "the legacy field is never a fallback"
        );
    }

    /// Every screen that shows or values USDB reads it through
    /// `usdb_holding`, so none can fall back to the legacy field (S1b E1).
    /// The Stable Balance reconcile in `app/mod.rs` reads the legacy field on
    /// purpose: it is about the SDK feature's own state, not a balance shown.
    #[test]
    fn usdb_balance_call_sites_read_the_validated_field() {
        for (name, source) in [
            ("state/spark/mod.rs", include_str!("../state/spark/mod.rs")),
            (
                "state/spark/overview.rs",
                include_str!("../state/spark/overview.rs"),
            ),
            (
                "state/global_home.rs",
                include_str!("../state/global_home.rs"),
            ),
            ("view/p2p/panel.rs", include_str!("../view/p2p/panel.rs")),
        ] {
            assert!(
                source.contains("usdb_holding(&info)"),
                "{} reads USDB through usdb_holding",
                name
            );
            assert!(
                !source.contains("info.stable_balance") && !source.contains(".stable_balance\n"),
                "{} never reads the legacy stable_balance",
                name
            );
        }
    }

    #[test]
    fn format_token_display_handles_precision_edges() {
        assert_eq!(format_token_display(42, 0), "42");
        assert_eq!(format_token_display(12, 1), "1.20");
        assert_eq!(format_token_display(105, 2), "1.05");
        assert_eq!(format_token_display(1_234_567, 6), "1.23");
        assert_eq!(format_token_display(1_999_999, 6), "2.00");
        assert_eq!(format_token_display(1_234_567_890_000, 6), "1,234,567.89");
    }

    #[test]
    fn format_token_display_clamps_extreme_decimals() {
        assert_eq!(
            format_token_display(u64::MAX, MAX_TOKEN_DECIMALS_U64 + 1),
            "1.84"
        );
    }

    #[test]
    fn format_token_display_rounds_half_up_and_carries_to_whole_units() {
        assert_eq!(format_token_display(1_234_999, 6), "1.23");
        assert_eq!(format_token_display(1_235_000, 6), "1.24");
        assert_eq!(format_token_display(999_995, 6), "1.00");
        assert_eq!(format_token_display(9_999_950, 6), "10.00");
    }

    #[test]
    fn stable_token_as_sats_converts_usd_pegged_balance() {
        assert_eq!(stable_token_as_sats(50_000_000, 6, Some(50_000.0)), 100_000);
        assert_eq!(stable_token_as_sats(123, 0, Some(41_000.0)), 300_000);
    }

    #[test]
    fn stable_token_as_sats_requires_positive_price() {
        assert_eq!(stable_token_as_sats(50_000_000, 6, None), 0);
        assert_eq!(stable_token_as_sats(50_000_000, 6, Some(0.0)), 0);
        assert_eq!(stable_token_as_sats(50_000_000, 6, Some(-1.0)), 0);
    }

    #[test]
    fn stable_token_as_sats_clamps_extreme_decimals_and_truncates_subsat_values() {
        assert_eq!(
            stable_token_as_sats(u64::MAX, MAX_TOKEN_DECIMALS_U64 + 1, Some(1.0)),
            184_467_440
        );
        assert_eq!(stable_token_as_sats(1, 6, Some(100_000.0)), 0);
    }
}
