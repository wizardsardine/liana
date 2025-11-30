use serde::Deserialize;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

const MEMPOOL_FEES_API_URL: &str = "https://mempool.space/api/v1/fees/recommended";
const ESPLORA_FEES_API_URL: &str = "https://blockstream.info/api/fee-estimates";

pub struct FeeEstimator;

impl FeeEstimator {
    pub fn new() -> Self {
        Self
    }

    pub async fn get_high_priority_rate(&self) -> Result<usize, FeeEstimatorError> {
        let fee = self.get_fee_rate(BlockTarget::Fastest).await?;
        Ok(fee.round() as usize)
    }

    pub async fn get_mid_priority_rate(&self) -> Result<usize, FeeEstimatorError> {
        let fee = self.get_fee_rate(BlockTarget::Standard).await?;
        Ok(fee.round() as usize)
    }

    pub async fn get_low_priority_rate(&self) -> Result<usize, FeeEstimatorError> {
        let fee = self.get_fee_rate(BlockTarget::Economy).await?;
        Ok(fee.round() as usize)
    }

    async fn get_fee_rate(&self, target: BlockTarget) -> Result<f64, FeeEstimatorError> {
        let fees = self.estimate_fees().await?;
        fees.get(&target)
            .copied()
            .ok_or_else(|| FeeEstimatorError::MissingData(format!("{target:?} missing")))
    }

    async fn estimate_fees(&self) -> Result<HashMap<BlockTarget, f64>, FeeEstimatorError> {
        let (mempool_res, esplora_res) =
            tokio::join!(Self::fetch_mempool_fees(), Self::fetch_esplora_fees(),);

        let mut combined: HashMap<BlockTarget, Vec<f64>> = HashMap::new();
        combined.insert(BlockTarget::Fastest, vec![]);
        combined.insert(BlockTarget::Standard, vec![]);
        combined.insert(BlockTarget::Economy, vec![]);

        if let Ok(fees) = mempool_res {
            for (target, fee) in fees {
                combined.get_mut(&target).unwrap().push(fee);
            }
        }
        if let Ok(fees) = esplora_res {
            for (target, fee) in fees {
                combined.get_mut(&target).unwrap().push(fee);
            }
        }

        if combined.values().all(|v| v.is_empty()) {
            return Err(FeeEstimatorError::NoFeeSources);
        }

        let mut final_fees = HashMap::new();

        for (target, list) in combined {
            if !list.is_empty() {
                let avg = list.iter().sum::<f64>() / list.len() as f64;
                final_fees.insert(target, avg);
            }
        }

        Ok(final_fees)
    }

    async fn fetch_mempool_fees() -> Result<HashMap<BlockTarget, f64>, FeeEstimatorError> {
        let response = reqwest::get(MEMPOOL_FEES_API_URL)
            .await?
            .json::<MempoolFeeResponse>()
            .await?;

        let mut map = HashMap::new();
        map.insert(BlockTarget::Fastest, response.fastest_fee);
        map.insert(BlockTarget::Standard, response.hour_fee);
        map.insert(BlockTarget::Economy, response.economy_fee);

        Ok(map)
    }

    async fn fetch_esplora_fees() -> Result<HashMap<BlockTarget, f64>, FeeEstimatorError> {
        let response = reqwest::get(ESPLORA_FEES_API_URL)
            .await?
            .json::<EsploraFeeResponse>()
            .await?
            .fees;

        let mut map = HashMap::new();

        let fee_1 = *response.get("1").ok_or_else(|| {
            FeeEstimatorError::MissingData("Esplora missing 1-block estimate".into())
        })?;

        let fee_6 = *response.get("6").ok_or_else(|| {
            FeeEstimatorError::MissingData("Esplora missing 6-block estimate".into())
        })?;

        let fee_24 = *response.get("24").ok_or_else(|| {
            FeeEstimatorError::MissingData("Esplora missing 24-block estimate".into())
        })?;

        map.insert(BlockTarget::Fastest, fee_1);
        map.insert(BlockTarget::Standard, fee_6);
        map.insert(BlockTarget::Economy, fee_24);

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
    #[serde(flatten)]
    fees: HashMap<String, f64>,
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
