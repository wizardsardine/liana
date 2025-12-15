pub mod api;
pub mod client;

pub use api::*;
pub use client::MavapayClient;

/// Checks if a country ISO code is in the African region (Mavapay supported)
#[inline(always)]
pub fn mavapay_supported(iso_code: &str) -> bool {
    ["NG", "KE", "ZA"].contains(&iso_code)
}
