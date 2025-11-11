/// Checks if a country ISO code is in the African region (Mavapay supported)
pub fn mavapay_supported(iso_code: &str) -> bool {
    ["NG", "KE", "ZA"].contains(&iso_code)
}
