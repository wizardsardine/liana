const WIDGET_OPTIONS: &'static str = "apiKey={{API_KEY}&mode=buy,sell&partnerContext=CoincubeVault&defaultFiat={{DEFAULT_FIAT}}&defaultAmount={{DEFAULT_AMOUNT}}&wallets=btc:{{WALLET_ADDRESS}}&onlyCryptoNetworks=bitcoin&sell_defaultFiat={{DEFAULT_FIAT}}&sell_onlyCryptoNetworks=bitcoin&redirectAtCheckout=true&enableCountrySelector=true";

const fn api_key() -> &'static str {
    if cfg!(debug_assertions) {
        "pk_test_01K2HQVXK7F5C8RDZ36WV2W3F5"
    } else {
        option_env!("ONRAMPER_API_KEY")
            .expect("Onramper API key should be set as an environment variable at compile time")
    }
}

const fn base_url() -> &'static str {
    if cfg!(debug_assertions) {
        "https://buy.onramper.dev"
    } else {
        "https://buy.onramper.com"
    }
}

