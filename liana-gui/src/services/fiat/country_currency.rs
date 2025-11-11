/// Returns the primary fiat currency for a given country ISO code
pub fn currency_for_country(iso_code: &str) -> Option<&'static str> {
    let countries = crate::services::geolocation::get_countries();
    countries
        .iter()
        .find(|c| c.code == iso_code)
        .map(|c| c.currency.name)
}

/// Checks if a country ISO code is in the African region (Mavapay supported)
pub fn mavapay_supported(iso_code: &str) -> bool {
    ["NG", "KE", "ZA"].contains(&iso_code)
}
