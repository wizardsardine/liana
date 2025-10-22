use liana::miniscript::bitcoin::Network;

const WIDGET_OPTIONS: &str = "{{BASE_URL}}/?apiKey={{API_KEY}}&mode={{MODE}}&partnerContext=CoincubeVault&defaultFiat={{DEFAULT_FIAT}}&wallets=btc:{{ADDRESS}}&onlyCryptoNetworks={{NETWORK}}&sell_defaultFiat={{DEFAULT_FIAT}}&sell_onlyCryptoNetworks={{NETWORK}}&redirectAtCheckout=true&enableCountrySelector=true&themeName=dark";

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

/// Maps Bitcoin network to Onramper network identifier
/// Returns None for unsupported networks (Signet, Regtest)
fn network_to_onramper_id(network: Network) -> Option<&'static str> {
    match network {
        Network::Bitcoin => Some("bitcoin"),
        Network::Testnet | Network::Testnet4 => Some("bitcoin-testnet"),
        Network::Signet | Network::Regtest => None,
    }
}

pub fn create_widget_url(
    currency: &str,
    address: Option<&str>,
    mode: &str,
    network: Network,
) -> Result<String, String> {
    let network_id = network_to_onramper_id(network).ok_or_else(|| {
        format!(
            "Onramper does not support {} network. Only Bitcoin mainnet and testnet are available.",
            network
        )
    })?;

    let api_key = api_key().ok_or_else(|| {
        "Onramper API key not configured. Please set `ONRAMPER_API_KEY` in .env".to_string()
    })?;

    let url = WIDGET_OPTIONS
        .replace("{{BASE_URL}}", base_url())
        .replace("{{MODE}}", mode)
        .replace("{{API_KEY}}", &api_key)
        .replace("{{DEFAULT_FIAT}}", currency)
        .replace("{{NETWORK}}", network_id);

    // insert address if any
    Ok(match address {
        Some(a) => url.replace("{{ADDRESS}}", a),
        None => url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use liana::miniscript::bitcoin::Network;

    #[test]
    fn test_network_to_onramper_id_mainnet() {
        assert_eq!(network_to_onramper_id(Network::Bitcoin), Some("bitcoin"));
    }

    #[test]
    fn test_network_to_onramper_id_testnet() {
        assert_eq!(
            network_to_onramper_id(Network::Testnet),
            Some("bitcoin-testnet")
        );
    }

    #[test]
    fn test_network_to_onramper_id_testnet4() {
        assert_eq!(
            network_to_onramper_id(Network::Testnet4),
            Some("bitcoin-testnet")
        );
    }

    #[test]
    fn test_network_to_onramper_id_signet_unsupported() {
        assert_eq!(network_to_onramper_id(Network::Signet), None);
    }

    #[test]
    fn test_network_to_onramper_id_regtest_unsupported() {
        assert_eq!(network_to_onramper_id(Network::Regtest), None);
    }

    #[test]
    fn test_create_widget_url_unsupported_network() {
        let result = create_widget_url("USD", None, "buy", Network::Signet);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("Onramper does not support signet network"));
    }

    #[test]
    fn test_create_widget_url_mainnet() {
        // Set test API key
        std::env::set_var("ONRAMPER_API_KEY", "test_key");

        let result = create_widget_url("USD", Some("bc1qtest"), "buy", Network::Bitcoin);
        assert!(result.is_ok());

        let url = result.unwrap();
        assert!(url.contains("onlyCryptoNetworks=bitcoin"));
        assert!(url.contains("sell_onlyCryptoNetworks=bitcoin"));
        assert!(url.contains("mode=buy"));
        assert!(url.contains("defaultFiat=USD"));
        assert!(url.contains("wallets=btc:bc1qtest"));
    }

    #[test]
    fn test_create_widget_url_testnet() {
        // Set test API key
        std::env::set_var("ONRAMPER_API_KEY", "test_key");

        let result = create_widget_url("EUR", None, "sell", Network::Testnet);
        assert!(result.is_ok());

        let url = result.unwrap();
        assert!(url.contains("onlyCryptoNetworks=bitcoin-testnet"));
        assert!(url.contains("sell_onlyCryptoNetworks=bitcoin-testnet"));
        assert!(url.contains("mode=sell"));
        assert!(url.contains("defaultFiat=EUR"));
    }
}
