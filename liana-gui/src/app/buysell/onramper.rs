const WIDGET_OPTIONS: &str = "{{BASE_URL}}/?apiKey={{API_KEY}}&mode={{MODE}}&partnerContext=CoincubeVault&defaultFiat={{DEFAULT_FIAT}}&wallets=btc:{{ADDRESS}}&onlyCryptoNetworks=bitcoin&sell_defaultFiat={{DEFAULT_FIAT}}&sell_onlyCryptoNetworks=bitcoin&redirectAtCheckout=true&enableCountrySelector=true&themeName=dark";

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

pub fn create_widget_url(currency: &str, address: Option<&str>, mode: &str) -> Option<String> {
    let url = WIDGET_OPTIONS
        .replace("{{BASE_URL}}", base_url())
        .replace("{{MODE}}", mode)
        .replace("{{API_KEY}}", api_key().as_deref()?)
        .replace("{{DEFAULT_FIAT}}", currency);

    // insert address if any
    Some(match address {
        Some(a) => url.replace("{{ADDRESS}}", a),
        None => url,
    })
}
