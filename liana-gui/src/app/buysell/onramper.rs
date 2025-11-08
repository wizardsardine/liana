use liana::miniscript::bitcoin;

const WIDGET_OPTIONS: &str = "{{BASE_URL}}/?apiKey={{API_KEY}}&mode={{MODE}}&partnerContext=CoincubeVault&defaultFiat={{DEFAULT_FIAT}}&onlyCryptoNetworks=bitcoin&sell_defaultFiat={{DEFAULT_FIAT}}&sell_onlyCryptoNetworks=bitcoin&redirectAtCheckout=true&enableCountrySelector=false&themeName=dark";

pub fn create_widget_url(
    currency: &str,
    address: Option<&str>,
    mode: &str,
    network: bitcoin::Network,
) -> Result<String, &'static str> {
    let api_key = match network {
        bitcoin::Network::Bitcoin => option_env!("ONRAMPER_API_KEY").ok_or(
            "`ONRAMPER_API_KEY` not configured, unable to proceed with mainnet transactions",
        )?,
        _ => "pk_test_01K2HQVXK7F5C8RDZ36WV2W3F5",
    };

    let base_url = match network {
        bitcoin::Network::Bitcoin => "https://buy.onramper.com",
        bitcoin::Network::Testnet | bitcoin::Network::Testnet4 => "https://buy.onramper.dev",
        _ => return Err("Onramper is only supported for mainnet and testnet wallets"),
    };

    let mut url = WIDGET_OPTIONS
        .replace("{{BASE_URL}}", base_url)
        .replace("{{MODE}}", mode)
        .replace("{{API_KEY}}", api_key)
        .replace("{{DEFAULT_FIAT}}", currency);

    // insert address if any
    if let Some(a) = address {
        let content = format!("wallets=btc:{}", a);

        match option_env!("ONRAMPER_SIGNING_SECRET") {
            Some(secret) => {
                log::warn!("`ONRAMPER_SIGNING_SECRET` was set at compile time. Onramper URL signatures will be generated and included");

                let mut engine = bitcoin_hashes::HmacEngine::<bitcoin_hashes::sha256::Hash>::new(
                    secret.as_bytes(),
                );
                bitcoin_hashes::HashEngine::input(&mut engine, content.as_bytes());
                let password_hmac =
                    <bitcoin_hashes::HmacSha256 as bitcoin_hashes::GeneralHash>::from_engine(
                        engine,
                    );
                let signature = hex::encode(password_hmac.as_ref());

                // assemble signed request
                let append = format!("&{}&signature={}", content, signature);
                url.push_str(&append);
            }
            None => {
                log::warn!("`ONRAMPER_SIGNING_SECRET` was not set at compile time. Onramper URL signatures will be excluded");

                let append = format!("&{}", content);
                url.push_str(&append);
            }
        };
    }

    Ok(url)
}
