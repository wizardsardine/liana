use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::time::Duration;

const MEMPOOL_FEES_API_URL: &str = "https://mempool.space/api/v1/fees/recommended";
const ESPLORA_FEES_API_URL: &str = "https://blockstream.info/api/fee-estimates";

/// Upper bound (sat/vB) we will accept from an external fee source. Anything
/// above this is treated as bogus and clamped. This is deliberately generous
/// (well above any realistic mempool spike) but bounded, so a poisoned or
/// malicious upstream cannot inject an absurd rate. Defense-in-depth: the
/// funds-critical `create_spend` path also enforces its own MAX_FEERATE, but the
/// estimator must not emit garbage regardless of who consumes it (CC-DESK-001).
const MAX_SANE_FEERATE: f64 = 5_000.0;

/// Validate and clamp a fee rate ingested from an external source. Rejects
/// non-finite (`NaN`/`inf`) and non-positive values, and clamps anything above
/// `MAX_SANE_FEERATE`. Returns `None` for values that should be discarded.
fn sanitize_feerate(fee: f64) -> Option<f64> {
    if !fee.is_finite() || fee <= 0.0 {
        return None;
    }
    Some(fee.min(MAX_SANE_FEERATE))
}

#[derive(Default, Clone)]
pub struct FeeEstimator;

impl FeeEstimator {
    pub fn new() -> Self {
        Self
    }

    pub async fn get_high_priority_rate(&self) -> Result<usize, FeeEstimatorError> {
        let fee = self.get_fee_rate(BlockTarget::Fastest).await?;
        Ok(fee.round().max(1.0) as usize)
    }

    pub async fn get_mid_priority_rate(&self) -> Result<usize, FeeEstimatorError> {
        let fee = self.get_fee_rate(BlockTarget::Standard).await?;
        Ok(fee.round().max(1.0) as usize)
    }

    pub async fn get_low_priority_rate(&self) -> Result<usize, FeeEstimatorError> {
        let fee = self.get_fee_rate(BlockTarget::Economy).await?;
        Ok(fee.round().max(1.0) as usize)
    }

    async fn get_fee_rate(&self, target: BlockTarget) -> Result<f64, FeeEstimatorError> {
        let fees = self.estimate_fees().await?;
        fees.get(&target)
            .copied()
            .ok_or_else(|| FeeEstimatorError::MissingData(format!("{target:?} missing")))
    }

    async fn estimate_fees(&self) -> Result<HashMap<BlockTarget, f64>, FeeEstimatorError> {
        let client = reqwest::ClientBuilder::new()
            .timeout(Duration::from_secs(10))
            .build()?;
        let (mempool_res, esplora_res) = tokio::join!(
            Self::fetch_mempool_fees(&client),
            Self::fetch_esplora_fees(&client),
        );

        let mut combined: HashMap<BlockTarget, Vec<f64>> = HashMap::new();
        combined.insert(BlockTarget::Fastest, vec![]);
        combined.insert(BlockTarget::Standard, vec![]);
        combined.insert(BlockTarget::Economy, vec![]);

        // CC-DESK-001: sanitize each externally-supplied rate before it enters
        // the average. Non-finite/non-positive values are dropped; excessive
        // values are clamped to MAX_SANE_FEERATE.
        if let Ok(fees) = mempool_res {
            for (target, fee) in fees {
                if let Some(v) = sanitize_feerate(fee) {
                    combined.get_mut(&target).unwrap().push(v);
                }
            }
        }
        if let Ok(fees) = esplora_res {
            for (target, fee) in fees {
                if let Some(v) = sanitize_feerate(fee) {
                    combined.get_mut(&target).unwrap().push(v);
                }
            }
        }

        if combined.values().all(|v| v.is_empty()) {
            return Err(FeeEstimatorError::NoFeeSources);
        }

        let mut final_fees = HashMap::new();

        for (target, list) in combined {
            if !list.is_empty() {
                let avg = list.iter().sum::<f64>() / list.len() as f64;
                // Belt-and-suspenders: the inputs are already sanitized, so the
                // average is finite and in-range, but clamp again defensively.
                final_fees.insert(target, avg.min(MAX_SANE_FEERATE).max(1.0));
            }
        }

        Ok(final_fees)
    }

    async fn fetch_mempool_fees(
        client: &Client,
    ) -> Result<HashMap<BlockTarget, f64>, FeeEstimatorError> {
        let response = client
            .get(MEMPOOL_FEES_API_URL)
            .send()
            .await?
            .json::<MempoolFeeResponse>()
            .await?;

        let mut map = HashMap::new();
        map.insert(BlockTarget::Fastest, response.fastest_fee);
        map.insert(BlockTarget::Standard, response.hour_fee);
        map.insert(BlockTarget::Economy, response.economy_fee);

        Ok(map)
    }

    async fn fetch_esplora_fees(
        client: &Client,
    ) -> Result<HashMap<BlockTarget, f64>, FeeEstimatorError> {
        let response = client
            .get(ESPLORA_FEES_API_URL)
            .send()
            .await?
            .json::<EsploraFeeResponse>()
            .await?;

        let mut map = HashMap::new();

        map.insert(BlockTarget::Fastest, response.one);
        map.insert(BlockTarget::Standard, response.six);
        map.insert(BlockTarget::Economy, response.twenty_four);

        Ok(map)
    }
}

#[derive(Debug, Deserialize)]
struct MempoolFeeResponse {
    #[serde(rename = "fastestFee")]
    fastest_fee: f64,
    #[serde(rename = "hourFee")]
    hour_fee: f64,
    #[serde(rename = "economyFee")]
    economy_fee: f64,
}

#[derive(Debug, Deserialize)]
struct EsploraFeeResponse {
    #[serde(rename = "1")]
    one: f64,
    #[serde(rename = "6")]
    six: f64,
    #[serde(rename = "24")]
    twenty_four: f64,
}

#[derive(Debug)]
pub enum FeeEstimatorError {
    Http(reqwest::Error),
    Json(serde_json::Error),
    MissingData(String),
    NoFeeSources,
}

impl fmt::Display for FeeEstimatorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeeEstimatorError::Http(e) => write!(f, "HTTP request failed: {}", e),
            FeeEstimatorError::Json(e) => write!(f, "JSON parsing failed: {}", e),
            FeeEstimatorError::MissingData(msg) => write!(f, "Missing data: {}", msg),
            FeeEstimatorError::NoFeeSources => write!(f, "No fee sources available"),
        }
    }
}

impl Error for FeeEstimatorError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            FeeEstimatorError::Http(e) => Some(e),
            FeeEstimatorError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for FeeEstimatorError {
    fn from(e: reqwest::Error) -> Self {
        FeeEstimatorError::Http(e)
    }
}

impl From<serde_json::Error> for FeeEstimatorError {
    fn from(e: serde_json::Error) -> Self {
        FeeEstimatorError::Json(e)
    }
}

#[derive(Debug, Eq, PartialEq, Hash)]
pub enum BlockTarget {
    Fastest,
    Standard,
    Economy,
}

#[cfg(test)]
mod tests {
    use super::{sanitize_feerate, MAX_SANE_FEERATE};

    #[test]
    fn rejects_non_finite_and_non_positive() {
        assert_eq!(sanitize_feerate(f64::NAN), None);
        assert_eq!(sanitize_feerate(f64::INFINITY), None);
        assert_eq!(sanitize_feerate(f64::NEG_INFINITY), None);
        assert_eq!(sanitize_feerate(0.0), None);
        assert_eq!(sanitize_feerate(-5.0), None);
    }

    #[test]
    fn clamps_excessive_and_passes_normal() {
        assert_eq!(sanitize_feerate(12.0), Some(12.0));
        assert_eq!(sanitize_feerate(MAX_SANE_FEERATE + 1_000.0), Some(MAX_SANE_FEERATE));
        assert_eq!(sanitize_feerate(MAX_SANE_FEERATE), Some(MAX_SANE_FEERATE));
    }
}
