//! Global app-wide display mode preference: should the primary
//! (large) header value lead with fiat or with bitcoin?
//!
//! Stored at the top of `Settings` (not per-cube) because it's a UX
//! leaning rather than per-wallet state, and persisted via the existing
//! `update_settings_file` flow.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayMode {
    /// Fiat is the large primary value; bitcoin is the secondary line.
    #[default]
    FiatNative,
    /// Bitcoin (BTC or SATS per the per-cube unit setting) is the
    /// primary value; fiat is the secondary line.
    BitcoinNative,
}

impl DisplayMode {
    pub fn flipped(self) -> Self {
        match self {
            Self::FiatNative => Self::BitcoinNative,
            Self::BitcoinNative => Self::FiatNative,
        }
    }
}
