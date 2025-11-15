use serde::Deserialize;
use std::time::Duration;

use crate::services::http::ResponseExt;

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Country {
    pub name: &'static str,
    pub code: &'static str,
    pub flag: &'static str,
    pub currency: Currency,
}

impl std::fmt::Display for Country {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.code)
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Currency {
    pub code: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
}

pub fn get_countries() -> &'static [Country] {
    static COUNTRIES_JSON: &'static str = include_str!("countries.json");
    static COUNTRIES: std::sync::OnceLock<Vec<Country>> = std::sync::OnceLock::new();

    COUNTRIES
        .get_or_init(|| serde_json::from_str(COUNTRIES_JSON).unwrap())
        .as_slice()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CountryResponse {
    country: String,
    iso_code: String,
}

#[derive(Clone)]
pub struct HttpGeoLocator {
    base_url: &'static str,
    client: reqwest::Client,
}

impl HttpGeoLocator {
    pub fn new() -> Self {
        let base_url = match cfg!(debug_assertions) {
            false => "https://api.coincube.io",
            true => option_env!("COINCUBE_API_URL").unwrap_or("https://dev-api.coincube.io"),
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");

        Self { base_url, client }
    }

    /// Detects the user's country and returns (country_name, iso_code)
    pub async fn locate(&self) -> Result<(String, String), String> {
        // allow users (and developers) to override detected ISO_CODE
        if let Ok(forced_iso) = std::env::var("FORCE_ISOCODE") {
            let (iso, name) = match {
                get_countries()
                    .iter()
                    .find(|c| c.code == forced_iso)
                    .map(|c| c.name)
            } {
                Some(name) => (name.to_string(), forced_iso),
                None => panic!("Unknown country iso code: {}", forced_iso),
            };

            tracing::info!("Forced country: {} ({})", &iso, &name);
            return Ok((iso, name));
        }

        let url = format!("{}/api/v1/geolocation/country", self.base_url);

        let res = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let res = match res.check_success().await {
            Ok(r) => r,
            Err(e) => return Err(e.text),
        };

        let body = res
            .json::<CountryResponse>()
            .await
            .map_err(|e| format!("invalid response: {}", e))?;

        Ok((body.country, body.iso_code))
    }
}
