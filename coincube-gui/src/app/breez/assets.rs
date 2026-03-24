use breez_sdk_liquid::bitcoin::Network;

// ---------------------------------------------------------------------------
// Mainnet Liquid asset IDs
// ---------------------------------------------------------------------------

pub const LBTC_ASSET_ID_MAINNET: &str =
    "6f0279e9ed041c3d710a9f57d0c02928416460c4b722ae3457a11eec381c526d";

pub const USDT_ASSET_ID_MAINNET: &str =
    "ce091c998b83c78bb71a632313ba3760f1763d9cfcffae02258ffa9865a37bd2";

// ---------------------------------------------------------------------------
// Regtest Liquid asset IDs
// L-BTC on regtest uses the native Elements regtest asset.
// USDt on regtest is environment-specific (must be issued locally) — no fixed
// constant is provided; callers should treat None as "not available."
// ---------------------------------------------------------------------------

pub const LBTC_ASSET_ID_REGTEST: &str =
    "5ac9f65c0efcc4775e0baec4ec03abdde22473cd3cf33c0419ca290e0751b225";

// ---------------------------------------------------------------------------
// Precision
// ---------------------------------------------------------------------------

pub const USDT_PRECISION: u8 = 8;
pub const LBTC_PRECISION: u8 = 8;

// ---------------------------------------------------------------------------
// AssetKind
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Lbtc,
    Usdt,
}

impl AssetKind {
    pub fn ticker(self) -> &'static str {
        match self {
            AssetKind::Lbtc => "L-BTC",
            AssetKind::Usdt => "USDt",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            AssetKind::Lbtc => "Liquid Bitcoin",
            AssetKind::Usdt => "Tether USD",
        }
    }

    pub fn precision(self) -> u8 {
        match self {
            AssetKind::Lbtc => LBTC_PRECISION,
            AssetKind::Usdt => USDT_PRECISION,
        }
    }

    /// Returns the asset ID string for this kind on the given network.
    pub fn asset_id(self, network: Network) -> Option<&'static str> {
        match self {
            AssetKind::Lbtc => lbtc_asset_id(network),
            AssetKind::Usdt => usdt_asset_id(network),
        }
    }
}

// ---------------------------------------------------------------------------
// Asset ID helpers
// ---------------------------------------------------------------------------

/// Returns the L-BTC asset ID for the given network, or `None` if unsupported.
pub fn lbtc_asset_id(network: Network) -> Option<&'static str> {
    match network {
        Network::Bitcoin => Some(LBTC_ASSET_ID_MAINNET),
        Network::Regtest => Some(LBTC_ASSET_ID_REGTEST),
        _ => None,
    }
}

/// Returns the USDt asset ID for the given network, or `None` if unsupported/unknown.
/// Regtest USDt is environment-specific — callers must handle `None` gracefully.
pub fn usdt_asset_id(network: Network) -> Option<&'static str> {
    match network {
        Network::Bitcoin => Some(USDT_ASSET_ID_MAINNET),
        _ => None,
    }
}

/// Resolves the `AssetKind` for a given raw asset ID and network.
/// Returns `None` for unrecognised asset IDs.
pub fn asset_kind_for_id(asset_id: &str, network: Network) -> Option<AssetKind> {
    if lbtc_asset_id(network) == Some(asset_id) {
        Some(AssetKind::Lbtc)
    } else if usdt_asset_id(network) == Some(asset_id) {
        Some(AssetKind::Usdt)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Amount formatting
// ---------------------------------------------------------------------------

/// Format a `u64` base-unit amount as a decimal display string using `precision`
/// decimal places.
///
/// ```
/// use coincube_gui::app::breez::assets::format_asset_amount;
/// assert_eq!(format_asset_amount(100_000_000, 8), "1.00000000");
/// assert_eq!(format_asset_amount(50_000_000, 8), "0.50000000");
/// ```
pub fn format_asset_amount(amount: u64, precision: u8) -> String {
    if precision == 0 {
        return amount.to_string();
    }
    let divisor = 10_u64.pow(precision as u32);
    let whole = amount / divisor;
    let frac = amount % divisor;
    format!("{}.{:0>width$}", whole, frac, width = precision as usize)
}

/// Number of decimal places to show when displaying USDt to the user.
pub const USDT_DISPLAY_DECIMALS: u8 = 2;

/// Format a USDt base-unit amount (8-decimal precision) for display with 2 decimal places.
pub fn format_usdt_display(amount: u64) -> String {
    let divisor = 10_u64.pow(USDT_PRECISION as u32);
    let mut whole = amount / divisor;
    let frac = amount % divisor;
    let scale = 10_u64.pow((USDT_PRECISION - USDT_DISPLAY_DECIMALS) as u32);
    let mut frac_2dp = (frac + scale / 2) / scale;
    let carry_threshold = 10_u64.pow(USDT_DISPLAY_DECIMALS as u32);
    if frac_2dp >= carry_threshold {
        frac_2dp = 0;
        whole += 1;
    }
    format!(
        "{}.{:0>width$}",
        whole,
        frac_2dp,
        width = USDT_DISPLAY_DECIMALS as usize
    )
}

// ---------------------------------------------------------------------------
// Amount parsing
// ---------------------------------------------------------------------------

/// Parses a decimal asset amount string into integer minor units without using f64.
///
/// Rejects scientific notation, invalid characters, and fractional digits that exceed
/// `scale`. Returns `None` for malformed or empty inputs. Zero values return `Some(0)`.
///
/// ```
/// use coincube_gui::app::breez::assets::parse_asset_to_minor_units;
/// assert_eq!(parse_asset_to_minor_units("1.50", 8), Some(150_000_000));
/// assert_eq!(parse_asset_to_minor_units("1e2",  8), None);   // scientific notation rejected
/// assert_eq!(parse_asset_to_minor_units("0.0",  8), Some(0)); // zero yields Some(0)
/// assert_eq!(parse_asset_to_minor_units("0",    8), Some(0)); // zero yields Some(0)
/// assert_eq!(parse_asset_to_minor_units("1.000000001", 8), None);  // too many decimals
/// ```
pub fn parse_asset_to_minor_units(s: &str, scale: u8) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains(['e', 'E']) {
        return None;
    }
    let (whole_str, frac_str) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if whole_str.is_empty() && frac_str.is_empty() {
        return None;
    }
    if frac_str.len() > scale as usize {
        return None;
    }
    let whole: u64 = if whole_str.is_empty() {
        0
    } else {
        whole_str.parse().ok()?
    };
    let frac_padded: u64 = if frac_str.is_empty() {
        0
    } else {
        if !frac_str.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let padding = scale as usize - frac_str.len();
        let padded = format!("{}{}", frac_str, "0".repeat(padding));
        padded.parse().ok()?
    };
    let multiplier = 10_u64.pow(scale as u32);
    let minor_units = whole.checked_mul(multiplier)?.checked_add(frac_padded)?;
    Some(minor_units)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usdt_asset_id_mainnet() {
        assert_eq!(
            usdt_asset_id(Network::Bitcoin),
            Some("ce091c998b83c78bb71a632313ba3760f1763d9cfcffae02258ffa9865a37bd2")
        );
    }

    #[test]
    fn test_lbtc_asset_id_mainnet() {
        assert_eq!(
            lbtc_asset_id(Network::Bitcoin),
            Some("6f0279e9ed041c3d710a9f57d0c02928416460c4b722ae3457a11eec381c526d")
        );
    }

    #[test]
    fn test_lbtc_asset_id_regtest() {
        assert_eq!(
            lbtc_asset_id(Network::Regtest),
            Some("5ac9f65c0efcc4775e0baec4ec03abdde22473cd3cf33c0419ca290e0751b225")
        );
    }

    #[test]
    fn test_usdt_asset_id_regtest_is_none() {
        assert_eq!(usdt_asset_id(Network::Regtest), None);
    }

    #[test]
    fn test_asset_kind_for_id_usdt() {
        assert_eq!(
            asset_kind_for_id(USDT_ASSET_ID_MAINNET, Network::Bitcoin),
            Some(AssetKind::Usdt)
        );
    }

    #[test]
    fn test_asset_kind_for_id_lbtc() {
        assert_eq!(
            asset_kind_for_id(LBTC_ASSET_ID_MAINNET, Network::Bitcoin),
            Some(AssetKind::Lbtc)
        );
    }

    #[test]
    fn test_asset_kind_for_id_unknown() {
        assert_eq!(asset_kind_for_id("unknown_id", Network::Bitcoin), None);
    }

    #[test]
    fn test_format_asset_amount_one() {
        assert_eq!(format_asset_amount(100_000_000, 8), "1.00000000");
    }

    #[test]
    fn test_format_asset_amount_zero() {
        assert_eq!(format_asset_amount(0, 8), "0.00000000");
    }

    #[test]
    fn test_format_asset_amount_fractional() {
        assert_eq!(format_asset_amount(123_456_789, 8), "1.23456789");
    }

    #[test]
    fn test_format_asset_amount_half() {
        assert_eq!(format_asset_amount(50_000_000, 8), "0.50000000");
    }

    #[test]
    fn test_format_asset_amount_zero_precision() {
        assert_eq!(format_asset_amount(42, 0), "42");
    }

    #[test]
    fn test_asset_kind_ticker() {
        assert_eq!(AssetKind::Usdt.ticker(), "USDt");
        assert_eq!(AssetKind::Lbtc.ticker(), "L-BTC");
    }

    #[test]
    fn test_asset_kind_precision() {
        assert_eq!(AssetKind::Usdt.precision(), 8);
        assert_eq!(AssetKind::Lbtc.precision(), 8);
    }
}
