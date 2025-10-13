/// Maps ISO 3166-1 alpha-2 country codes to their primary fiat currency codes
/// and currency symbols for display purposes.
use super::Currency;

/// Returns the primary fiat currency for a given country ISO code
pub fn currency_for_country(iso_code: &str) -> Currency {
    match iso_code.to_uppercase().as_str() {
        // African countries (Mavapay supported)
        "NG" => Currency::NGN, // Nigeria
        "KE" => Currency::KES, // Kenya
        "ZA" => Currency::ZAR, // South Africa

        // North America
        "US" => Currency::USD, // United States
        "CA" => Currency::CAD, // Canada
        "MX" => Currency::MXN, // Mexico
        "GT" => Currency::GTQ, // Guatemala
        "HN" => Currency::HNL, // Honduras
        "CR" => Currency::CRC, // Costa Rica
        "DO" => Currency::DOP, // Dominican Republic
        "SV" => Currency::SVC, // El Salvador

        // Europe
        "GB" => Currency::GBP, // United Kingdom
        "CH" => Currency::CHF, // Switzerland
        "NO" => Currency::NOK, // Norway
        "SE" => Currency::SEK, // Sweden
        "DK" => Currency::DKK, // Denmark
        "PL" => Currency::PLN, // Poland
        "CZ" => Currency::CZK, // Czech Republic
        "HU" => Currency::HUF, // Hungary
        "RO" => Currency::RON, // Romania
        "BA" => Currency::BAM, // Bosnia and Herzegovina
        "UA" => Currency::UAH, // Ukraine
        "RU" => Currency::RUB, // Russia
        "TR" => Currency::TRY, // Turkey
        "GE" => Currency::GEL, // Georgia

        // Eurozone countries
        "AT" | "BE" | "CY" | "EE" | "FI" | "FR" | "DE" | "GR" | "IE" | "IT" | "LV" | "LT"
        | "LU" | "MT" | "NL" | "PT" | "SK" | "SI" | "ES" => Currency::EUR,

        // Asia
        "CN" => Currency::CNY, // China
        "JP" => Currency::JPY, // Japan
        "KR" => Currency::KRW, // South Korea
        "IN" => Currency::INR, // India
        "ID" => Currency::IDR, // Indonesia
        "MY" => Currency::MYR, // Malaysia
        "SG" => Currency::SGD, // Singapore
        "TH" => Currency::THB, // Thailand
        "VN" => Currency::VND, // Vietnam
        "PH" => Currency::PHP, // Philippines
        "HK" => Currency::HKD, // Hong Kong
        "TW" => Currency::TWD, // Taiwan
        "PK" => Currency::PKR, // Pakistan
        "BD" => Currency::BDT, // Bangladesh
        "LK" => Currency::LKR, // Sri Lanka
        "MM" => Currency::MMK, // Myanmar
        "IL" => Currency::ILS, // Israel
        "SA" => Currency::SAR, // Saudi Arabia
        "AE" => Currency::AED, // United Arab Emirates
        "KW" => Currency::KWD, // Kuwait
        "BH" => Currency::BHD, // Bahrain
        "LB" => Currency::LBP, // Lebanon
        "AM" => Currency::AMD, // Armenia

        // South America
        "BR" => Currency::BRL, // Brazil
        "AR" => Currency::ARS, // Argentina
        "CL" => Currency::CLP, // Chile
        "CO" => Currency::COP, // Colombia
        "PE" => Currency::PEN, // Peru
        "VE" => Currency::VEF, // Venezuela

        // Oceania
        "AU" => Currency::AUD, // Australia
        "NZ" => Currency::NZD, // New Zealand

        // Africa (additional)
        "ZM" => Currency::ZMW, // Zambia

        // Default to USD for unknown countries
        _ => Currency::USD,
    }
}

/// Returns the currency symbol for a given country ISO code
pub fn currency_symbol_for_country(iso_code: &str) -> &'static str {
    match iso_code.to_uppercase().as_str() {
        // African countries
        "NG" => "₦",   // Nigerian Naira
        "KE" => "KSh", // Kenyan Shilling
        "ZA" => "R",   // South African Rand
        "ZM" => "ZK",  // Zambian Kwacha

        // North America
        "US" | "CA" | "MX" | "GT" | "HN" | "CR" | "DO" | "SV" => "$",

        // Europe
        "GB" => "£",   // British Pound
        "CH" => "CHF", // Swiss Franc
        "NO" => "kr",  // Norwegian Krone
        "SE" => "kr",  // Swedish Krona
        "DK" => "kr",  // Danish Krone
        "PL" => "zł",  // Polish Zloty
        "CZ" => "Kč",  // Czech Koruna
        "HU" => "Ft",  // Hungarian Forint
        "RO" => "lei", // Romanian Leu
        "BA" => "KM",  // Bosnia and Herzegovina Convertible Mark
        "UA" => "₴",   // Ukrainian Hryvnia
        "RU" => "₽",   // Russian Ruble
        "TR" => "₺",   // Turkish Lira
        "GE" => "₾",   // Georgian Lari

        // Eurozone
        "AT" | "BE" | "CY" | "EE" | "FI" | "FR" | "DE" | "GR" | "IE" | "IT" | "LV" | "LT"
        | "LU" | "MT" | "NL" | "PT" | "SK" | "SI" | "ES" => "€",

        // Asia
        "CN" => "¥",   // Chinese Yuan
        "JP" => "¥",   // Japanese Yen
        "KR" => "₩",   // South Korean Won
        "IN" => "₹",   // Indian Rupee
        "ID" => "Rp",  // Indonesian Rupiah
        "MY" => "RM",  // Malaysian Ringgit
        "SG" => "S$",  // Singapore Dollar
        "TH" => "฿",   // Thai Baht
        "VN" => "₫",   // Vietnamese Dong
        "PH" => "₱",   // Philippine Peso
        "HK" => "HK$", // Hong Kong Dollar
        "TW" => "NT$", // Taiwan Dollar
        "PK" => "₨",   // Pakistani Rupee
        "BD" => "৳",   // Bangladeshi Taka
        "LK" => "Rs",  // Sri Lankan Rupee
        "MM" => "K",   // Myanmar Kyat
        "IL" => "₪",   // Israeli Shekel
        "SA" => "﷼",   // Saudi Riyal
        "AE" => "د.إ", // UAE Dirham
        "KW" => "د.ك", // Kuwaiti Dinar
        "BH" => "د.ب", // Bahraini Dinar
        "LB" => "ل.ل", // Lebanese Pound
        "AM" => "֏",   // Armenian Dram

        // South America
        "BR" => "R$", // Brazilian Real
        "AR" => "$",  // Argentine Peso
        "CL" => "$",  // Chilean Peso
        "CO" => "$",  // Colombian Peso
        "PE" => "S/", // Peruvian Sol
        "VE" => "Bs", // Venezuelan Bolívar

        // Oceania
        "AU" => "A$",  // Australian Dollar
        "NZ" => "NZ$", // New Zealand Dollar

        // Default
        _ => "$",
    }
}

/// Checks if a country ISO code is in the African region (Mavapay supported)
pub fn is_african_country(iso_code: &str) -> bool {
    matches!(iso_code.to_uppercase().as_str(), "NG" | "KE" | "ZA")
}

/// Returns the Mavapay minor unit currency code for African countries
/// (e.g., NGNKOBO for Nigeria, KESCENT for Kenya, ZARCENT for South Africa)
pub fn mavapay_minor_unit_for_country(iso_code: &str) -> &'static str {
    match iso_code.to_uppercase().as_str() {
        "NG" => "NGNKOBO",
        "KE" => "KESCENT",
        "ZA" => "ZARCENT",
        _ => "BTCSAT", // Default to BTC satoshis
    }
}

/// Returns the Mavapay major unit currency code for African countries
pub fn mavapay_major_unit_for_country(iso_code: &str) -> &'static str {
    match iso_code.to_uppercase().as_str() {
        "NG" => "NGN",
        "KE" => "KES",
        "ZA" => "ZAR",
        _ => "BTC",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_african_countries() {
        assert_eq!(currency_for_country("NG"), Currency::NGN);
        assert_eq!(currency_for_country("KE"), Currency::KES);
        assert_eq!(currency_for_country("ZA"), Currency::ZAR);

        assert!(is_african_country("NG"));
        assert!(is_african_country("KE"));
        assert!(is_african_country("ZA"));
        assert!(!is_african_country("US"));
    }

    #[test]
    fn test_international_countries() {
        assert_eq!(currency_for_country("US"), Currency::USD);
        assert_eq!(currency_for_country("GB"), Currency::GBP);
        assert_eq!(currency_for_country("DE"), Currency::EUR);
        assert_eq!(currency_for_country("JP"), Currency::JPY);
    }

    #[test]
    fn test_mavapay_units() {
        assert_eq!(mavapay_minor_unit_for_country("NG"), "NGNKOBO");
        assert_eq!(mavapay_minor_unit_for_country("KE"), "KESCENT");
        assert_eq!(mavapay_minor_unit_for_country("ZA"), "ZARCENT");

        assert_eq!(mavapay_major_unit_for_country("NG"), "NGN");
        assert_eq!(mavapay_major_unit_for_country("KE"), "KES");
        assert_eq!(mavapay_major_unit_for_country("ZA"), "ZAR");
    }

    #[test]
    fn test_currency_symbols() {
        assert_eq!(currency_symbol_for_country("NG"), "₦");
        assert_eq!(currency_symbol_for_country("US"), "$");
        assert_eq!(currency_symbol_for_country("GB"), "£");
        assert_eq!(currency_symbol_for_country("DE"), "€");
    }
}
