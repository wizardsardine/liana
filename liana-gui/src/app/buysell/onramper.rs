const WIDGET_OPTIONS: &str = "{{BASE_URL}}/?apiKey={{API_KEY}}&mode=buy,sell&partnerContext=CoincubeVault&defaultFiat={{DEFAULT_FIAT}}&defaultAmount={{DEFAULT_AMOUNT}}&wallets=btc:{{WALLET_ADDRESS}}&onlyCryptoNetworks=bitcoin&sell_defaultFiat={{DEFAULT_FIAT}}&sell_onlyCryptoNetworks=bitcoin&redirectAtCheckout=true&enableCountrySelector=true&themeName=dark";

pub fn api_key() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("ONRAMPER_API_KEY").ok()
    } else {
        // TODO: Move to .env and use in build.rs for compile-time env
        option_env!("ONRAMPER_API_KEY").map(|s| s.to_string())
    }
}

const fn base_url() -> &'static str {
    if cfg!(debug_assertions) {
        "https://buy.onramper.dev"
    } else {
        "https://buy.onramper.com"
    }
}

pub fn create_widget_url(currency: &str, amount: &str, wallet: &str) -> Option<String> {
    let key = api_key()?;
    Some(
        WIDGET_OPTIONS
            .replace("{{BASE_URL}}", base_url())
            .replace("{{API_KEY}}", &key)
            .replace("{{DEFAULT_FIAT}}", currency)
            .replace("{{WALLET_ADDRESS}}", wallet)
            .replace("{{DEFAULT_AMOUNT}}", amount),
    )
}
