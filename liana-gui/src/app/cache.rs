use crate::{
    daemon::{
        model::{Coin, ListCoinsResult},
        Daemon, DaemonError,
    },
    dir::LianaDirectory,
    services::fiat::{
        api::{GetPriceResult, PriceApi, PriceApiError},
        client::PriceClient,
        Currency, PriceSource,
    },
};
use liana::miniscript::bitcoin::Network;
use lianad::commands::CoinStatus;
use std::sync::Arc;

pub const FIAT_PRICE_UPDATE_INTERVAL_SECS: u64 = 300;

#[derive(Debug, Clone)]
pub struct Cache {
    pub datadir_path: LianaDirectory,
    pub network: Network,
    /// The `last_poll_timestamp` when starting the application.
    pub last_poll_at_startup: Option<u32>,
    pub daemon_cache: DaemonCache,
    pub fiat_price_cache: FiatPriceCache,
}

/// only used for tests.
impl std::default::Default for Cache {
    fn default() -> Self {
        Self {
            datadir_path: LianaDirectory::new(std::path::PathBuf::new()),
            network: Network::Bitcoin,
            last_poll_at_startup: None,
            daemon_cache: DaemonCache::default(),
            fiat_price_cache: FiatPriceCache::default(),
        }
    }
}

impl Cache {
    pub fn blockheight(&self) -> i32 {
        self.daemon_cache.blockheight
    }

    pub fn coins(&self) -> &[Coin] {
        &self.daemon_cache.coins
    }

    pub fn rescan_progress(&self) -> Option<f64> {
        self.daemon_cache.rescan_progress
    }

    pub fn sync_progress(&self) -> f64 {
        self.daemon_cache.sync_progress
    }

    pub fn last_poll_timestamp(&self) -> Option<u32> {
        self.daemon_cache.last_poll_timestamp
    }
}

/// The cache for dynamic daemon data.
#[derive(Debug, Clone)]
pub struct DaemonCache {
    pub blockheight: i32,
    pub coins: Vec<Coin>,
    pub rescan_progress: Option<f64>,
    pub sync_progress: f64,
    /// The most recent `last_poll_timestamp`.
    pub last_poll_timestamp: Option<u32>,
}

/// only used for tests.
impl std::default::Default for DaemonCache {
    fn default() -> Self {
        Self {
            blockheight: 0,
            coins: Vec::new(),
            rescan_progress: None,
            sync_progress: 1.0,
            last_poll_timestamp: None,
        }
    }
}

/// Get the coins that should be cached.
pub async fn coins_to_cache(
    daemon: Arc<dyn Daemon + Sync + Send>,
) -> Result<ListCoinsResult, DaemonError> {
    daemon
        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
        .await
}

/// The cache for fiat price data.
#[derive(Debug, Clone, Default)]
pub struct FiatPriceCache {
    /// The last fetched fiat price, if any.
    pub fiat_price: Option<FiatPrice>,
    /// A pending request, if any.
    ///
    /// This is the last request that may or may not yet have been completed.
    pub last_request: Option<FiatPriceRequest>,
}

/// Represents a fiat price fetched from the API together with the request that was used to fetch it.
#[derive(Debug, Clone)]
pub struct FiatPrice {
    pub res: Result<GetPriceResult, PriceApiError>, // also store error in case we want to display it to user
    pub request: FiatPriceRequest,
}

impl FiatPrice {
    pub fn source(&self) -> PriceSource {
        self.request.source
    }

    pub fn currency(&self) -> Currency {
        self.request.currency
    }

    pub fn requested_at(&self) -> u64 {
        self.request.timestamp
    }
}

/// Represents a fiat price request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiatPriceRequest {
    pub source: PriceSource,
    pub currency: Currency,
    pub timestamp: u64,
}

impl FiatPriceRequest {
    /// Sends the request using the default client for the given source.
    pub async fn send_default(self) -> FiatPrice {
        let client = PriceClient::default_from_source(self.source);
        FiatPrice {
            res: client.get_price(self.currency).await,
            request: self,
        }
    }
}
