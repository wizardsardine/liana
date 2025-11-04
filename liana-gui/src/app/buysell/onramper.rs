use liana::miniscript::bitcoin;

const WIDGET_OPTIONS: &str = "{{BASE_URL}}/?apiKey={{API_KEY}}&mode={{MODE}}&partnerContext=CoincubeVault&defaultFiat={{DEFAULT_FIAT}}&onlyCryptoNetworks=bitcoin&sell_defaultFiat={{DEFAULT_FIAT}}&sell_onlyCryptoNetworks=bitcoin&redirectAtCheckout=true&enableCountrySelector=true&themeName=dark";

pub fn create_widget_url(
    currency: &str,
    address: Option<&str>,
    mode: &str,
    network: bitcoin::Network,
) -> Result<String, &'static str> {
    let api_key = match network {
        bitcoin::Network::Bitcoin => {
            option_env!("ONRAMPER_API_KEY").ok_or("`ONRAMPER_API_KEY` not configured")?
        }
        _ => "pk_test_01K2HQVXK7F5C8RDZ36WV2W3F5",
    };

    let base_url = match network {
        bitcoin::Network::Bitcoin => "https://buy.onramper.com",
        _ => "https://buy.onramper.dev",
    };

    let mut url = WIDGET_OPTIONS
        .replace("{{BASE_URL}}", base_url)
        .replace("{{MODE}}", mode)
        .replace("{{API_KEY}}", api_key)
        .replace("{{DEFAULT_FIAT}}", currency);

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

        let result = create_widget_url("USD", Some("bc1qtest"), "buy", bitcoin::Network::Testnet);
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

        let result = create_widget_url("EUR", None, "sell", bitcoin::Network::Bitcoin);
        assert!(result.is_ok());

        let url = result.unwrap();
        assert!(url.contains("onlyCryptoNetworks=bitcoin"));
        assert!(url.contains("sell_onlyCryptoNetworks=bitcoin"));
        assert!(url.contains("mode=sell"));
        assert!(url.contains("defaultFiat=EUR"));
    }
}
