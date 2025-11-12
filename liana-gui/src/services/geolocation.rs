use serde::Deserialize;
use std::time::Duration;

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
struct CountryResponse {
    country: String, // Country name (e.g., "United States")
    #[serde(rename = "isoCode")]
    iso_code: String, // ISO 3166-1 alpha-2 code (e.g., "US")
}

#[derive(Clone)]
pub struct HttpGeoLocator {
    base_url: String,
    client: reqwest::Client,
}

impl HttpGeoLocator {
    pub fn new(base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client");

        Self { base_url, client }
    }

    /// Detects the user's country and returns (country_name, iso_code)
    pub async fn detect(&self) -> Result<(String, String), String> {
        let url = format!(
            "{}/api/v1/geolocation/country",
            self.base_url.trim_end_matches('/')
        );

        let res = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|e| format!("request failed: {}", e))?;

        if !res.status().is_success() {
            return Err(format!("server returned status {}", res.status()));
        }

        let body = res
            .json::<CountryResponse>()
            .await
            .map_err(|e| format!("invalid response: {}", e))?;

        Ok((body.country, body.iso_code))
    }
}

pub struct CachedGeoLocator {
    inner: HttpGeoLocator,
}

impl CachedGeoLocator {
    pub fn new_from_env() -> Self {
        let base = std::env::var("COINCUBE_API_URL")
            .unwrap_or_else(|_| "https://dev-api.coincube.io".to_string());
        let inner = HttpGeoLocator::new(base);
        Self { inner }
    }

    /// Returns a default country name for common ISO codes (for debugging)
    fn default_country_name(iso_code: &str) -> Option<&'static str> {
        let countries = get_countries();
        countries
            .iter()
            .find(|c| c.code == iso_code)
            .map(|c| c.name)
    }

    /// Detects the user's country and returns (country_name, iso_code)
    pub async fn detect_country(&self) -> Result<(String, String), String> {
        if cfg!(debug_assertions) {
            if let Ok(forced_iso) = std::env::var("FORCE_ISOCODE") {
                // Check if country name is also forced
                let pair = match std::env::var("FORCE_COUNTRY") {
                    Ok(forced_name) => (forced_name, forced_iso),
                    Err(_) => match Self::default_country_name(&forced_iso) {
                        Some(name) => (name.to_string(), forced_iso),
                        None => panic!("Unknown country iso code: {}", forced_iso),
                    },
                };

                tracing::info!("🔧 [DEBUG] Forced country: {} ({})", &pair.0, &pair.1);
                return Ok(pair);
            }
        }

        // Fetch fresh
        self.inner.detect().await
    }
}
