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
