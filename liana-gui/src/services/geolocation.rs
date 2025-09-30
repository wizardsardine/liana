use serde::Deserialize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Region {
    Africa,
    International,
}

#[derive(Debug, Clone, Deserialize)]
struct RegionResponse {
    region: String,
    country: String,
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

    pub async fn detect(&self) -> Result<(Region, String), String> {
        let url = format!(
            "{}/api/v1/geolocation/region",
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
            .json::<RegionResponse>()
            .await
            .map_err(|e| format!("invalid response: {}", e))?;

        let region = match body.region.to_ascii_lowercase().as_str() {
            "africa" => Region::Africa,
            _ => Region::International,
        };

        Ok((region, body.country))
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    region: Region,
    country: String,
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

    pub async fn detect_region(&self) -> Result<(Region, String), String> {
        // Developer override for testing (preferred)
        if let Ok(force) = std::env::var("FORCE_REGION") {
            let f = force.to_ascii_lowercase();
            let region = match f.as_str() {
                "africa" => Some(Region::Africa),
                "international" => Some(Region::International),
                _ => None,
            };
            if let Some(r) = region {
                // Allow overriding country for testing; else use reasonable defaults per region
                let forced_country = std::env::var("FORCE_COUNTRY").ok();
                let country = forced_country
                    .map(|c| c.trim().to_uppercase())
                    .filter(|c| c.len() == 2)
                    .unwrap_or_else(|| match r {
                        Region::Africa => "NG".to_string(),
                        Region::International => "US".to_string(),
                    });
                return Ok((r, country));
            }
        }

        // Backward-compat: support older env name
        if let Ok(force) = std::env::var("FORCE_PROVIDER") {
            let f = force.to_ascii_lowercase();
            let region = match f.as_str() {
                "africa" => Some(Region::Africa),
                "international" => Some(Region::International),
                _ => None,
            };
            if let Some(r) = region {
                let forced_country = std::env::var("FORCE_COUNTRY").ok();
                let country = forced_country
                    .map(|c| c.trim().to_uppercase())
                    .filter(|c| c.len() == 2)
                    .unwrap_or_else(|| match r {
                        Region::Africa => "NG".to_string(),
                        Region::International => "US".to_string(),
                    });
                return Ok((r, country));
            }
        }

        // Check cache
        if let Some(entry) = self.cache.lock().expect("poisoned").as_ref() {
            if entry.cached_at.elapsed() < self.ttl {
                return Ok((entry.region.clone(), entry.country.clone()));
            }
        }

        // Fetch fresh
        let result = self.inner.detect().await;
        if let Ok((region, country)) = result.clone() {
            let mut guard = self.cache.lock().expect("poisoned");
            *guard = Some(CacheEntry {
                region: region.clone(),
                country: country.clone(),
                cached_at: Instant::now(),
            });
        }
        result
    }
}
