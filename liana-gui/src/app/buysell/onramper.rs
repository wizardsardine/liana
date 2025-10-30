const WIDGET_OPTIONS: &str = "{{BASE_URL}}/?apiKey={{API_KEY}}&mode={{MODE}}&partnerContext=CoincubeVault&defaultFiat={{DEFAULT_FIAT}}&onlyCryptoNetworks={{NETWORK}}&sell_defaultFiat={{DEFAULT_FIAT}}&sell_onlyCryptoNetworks={{NETWORK}}&redirectAtCheckout=true&enableCountrySelector=true&themeName=dark";

pub fn api_key() -> Option<String> {
    // Always read from runtime environment (supports .env file loaded via dotenv)
    std::env::var("ONRAMPER_API_KEY")
        .ok()
        .map(|key| key.trim_matches('"').to_string())
}

fn base_url() -> &'static str {
    // Use dev URL for test API keys (pk_test_*), production URL for live keys (pk_prod_*)
    match api_key() {
        Some(key) if key.starts_with("pk_test_") => "https://buy.onramper.dev",
        _ => "https://buy.onramper.com",
    }
}

pub fn create_widget_url(
    currency: &str,
    address: Option<&str>,
    mode: &str,
) -> Result<String, String> {
    let api_key = api_key().ok_or_else(|| {
        "Onramper API key not configured. Please set `ONRAMPER_API_KEY` in .env".to_string()
    })?;

    let mut url = WIDGET_OPTIONS
        .replace("{{BASE_URL}}", base_url())
        .replace("{{MODE}}", mode)
        .replace("{{API_KEY}}", &api_key)
        .replace("{{DEFAULT_FIAT}}", currency)
        .replace("{{NETWORK}}", "bitcoin");

    // insert address if any
    if let Some(a) = address {
        let opt = format!("&wallets=btc:{}", a);
        url.push_str(&opt);
    }

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_widget_url_mainnet() {
        std::env::set_var("ONRAMPER_API_KEY", "test_key");

        let result = create_widget_url("USD", Some("bc1qtest"), "buy");
        assert!(result.is_ok());

        let url = result.unwrap();
        assert!(url.contains("onlyCryptoNetworks=bitcoin"));
        assert!(url.contains("sell_onlyCryptoNetworks=bitcoin"));
        assert!(url.contains("mode=buy"));
        assert!(url.contains("defaultFiat=USD"));
        assert!(url.contains("wallets=btc:bc1qtest"));
    }

    #[test]
    fn test_create_widget_url_with_no_address() {
        std::env::set_var("ONRAMPER_API_KEY", "test_key");

        let result = create_widget_url("EUR", None, "sell");
        assert!(result.is_ok());

        let url = result.unwrap();
        assert!(url.contains("onlyCryptoNetworks=bitcoin"));
        assert!(url.contains("sell_onlyCryptoNetworks=bitcoin"));
        assert!(url.contains("mode=sell"));
        assert!(url.contains("defaultFiat=EUR"));
    }
}
