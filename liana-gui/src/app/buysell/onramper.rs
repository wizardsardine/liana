const WIDGET_OPTIONS: &str = "{{BASE_URL}}/?apiKey={{API_KEY}}&mode=buy,sell&partnerContext=CoincubeVault&defaultFiat={{DEFAULT_FIAT}}&defaultAmount={{DEFAULT_AMOUNT}}&wallets=btc:{{WALLET_ADDRESS}}&onlyCryptoNetworks=bitcoin&sell_defaultFiat={{DEFAULT_FIAT}}&sell_onlyCryptoNetworks=bitcoin&redirectAtCheckout=true&enableCountrySelector=true&themeName=dark";

const fn api_key() -> &'static str {
    if cfg!(debug_assertions) {
        "pk_test_01K2HQVXK7F5C8RDZ36WV2W3F5"
    } else {
        env!("ONRAMPER_API_KEY")
    }
}

const fn base_url() -> &'static str {
    if cfg!(debug_assertions) {
        "https://buy.onramper.dev"
    } else {
        "https://buy.onramper.com"
    }
}

pub fn create_widget_url(currency: &str, amount: &str, wallet: &str) -> String {
    WIDGET_OPTIONS
        .replace("{{BASE_URL}}", base_url())
        .replace("{{API_KEY}}", api_key())
        .replace("{{DEFAULT_FIAT}}", currency)
        .replace("{{WALLET_ADDRESS}}", wallet)
        .replace("{{DEFAULT_AMOUNT}}", amount)
}
