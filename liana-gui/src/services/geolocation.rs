use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
struct CountryResponse {
    country: String,      // Country name (e.g., "United States")
    #[serde(rename = "isoCode")]
    iso_code: String,     // ISO 3166-1 alpha-2 code (e.g., "US")
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

#[derive(Debug, Clone)]
struct CacheEntry {
    country_name: String,
    iso_code: String,
    cached_at: Instant,
}

pub struct CachedGeoLocator {
    inner: HttpGeoLocator,
    cache: Arc<Mutex<Option<CacheEntry>>>,
    ttl: Duration,
}

impl CachedGeoLocator {
    pub fn new_from_env() -> Self {
        let base = std::env::var("COINCUBE_API_URL")
            .unwrap_or_else(|_| "https://dev-api.coincube.io".to_string());
        let inner = HttpGeoLocator::new(base);
        Self {
            inner,
            cache: Arc::new(Mutex::new(None)),
            ttl: Duration::from_secs(18_000), // 5 hours
        }
    }

    /// Returns a default country name for common ISO codes (for debugging)
    fn default_country_name(iso_code: &str) -> String {
        match iso_code {
            "NG" => "Nigeria",
            "KE" => "Kenya",
            "ZA" => "South Africa",
            "US" => "United States",
            "GB" => "United Kingdom",
            "DE" => "Germany",
            "FR" => "France",
            "IT" => "Italy",
            "ES" => "Spain",
            "JP" => "Japan",
            "CN" => "China",
            "IN" => "India",
            "BR" => "Brazil",
            "CA" => "Canada",
            "AU" => "Australia",
            _ => iso_code,
        }
        .to_string()
    }

    /// Detects the user's country and returns (country_name, iso_code)
    pub async fn detect_country(&self) -> Result<(String, String), String> {
        // Developer override for testing
        // FORCE_ISOCODE: Set the ISO code (e.g., "NG", "US", "GB")
        // FORCE_COUNTRY: Set the country name (optional, will derive from ISO if not set)
        if let Ok(forced_iso) = std::env::var("FORCE_ISOCODE") {
            let iso = forced_iso.trim().to_uppercase();
            if !iso.is_empty() && iso.len() == 2 {
                // Check if country name is also forced
                let country_name = if let Ok(forced_name) = std::env::var("FORCE_COUNTRY") {
                    let name = forced_name.trim();
                    if !name.is_empty() {
                        name.to_string()
                    } else {
                        // Derive from ISO code
                        Self::default_country_name(&iso)
                    }
                } else {
                    // Derive from ISO code
                    Self::default_country_name(&iso)
                };

                tracing::info!("🔧 [DEBUG] Forced country: {} ({})", country_name, iso);
                return Ok((country_name, iso));
            }
        }

        // Check cache
        if let Some(entry) = self.cache.lock().expect("poisoned").as_ref() {
            if entry.cached_at.elapsed() < self.ttl {
                return Ok((entry.country_name.clone(), entry.iso_code.clone()));
            }
        }

        // Fetch fresh
        let result = self.inner.detect().await;
        if let Ok((country_name, iso_code)) = result.clone() {
            let mut guard = self.cache.lock().expect("poisoned");
            *guard = Some(CacheEntry {
                country_name: country_name.clone(),
                iso_code: iso_code.clone(),
                cached_at: Instant::now(),
            });
        }
        result
    }
}
