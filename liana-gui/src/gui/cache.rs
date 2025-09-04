use std::collections::HashMap;
use std::time::Duration;

use crate::app::cache::{FiatPrice, FiatPriceRequest};
use crate::services::fiat::{Currency, PriceSource};

/// How long a cached fiat price is considered fresh.
const FIAT_PRICE_TTL: Duration = Duration::from_secs(300);

/// A global cache for the application.
#[derive(Default)]
pub struct GlobalCache {
    fiat_prices: FiatPricesCache,
}

impl GlobalCache {
    /// Get a fiat price from the cache if it exists and is not older than `FIAT_PRICE_TTL`.
    pub fn fresh_fiat_price(&self, source: PriceSource, currency: Currency) -> Option<&FiatPrice> {
        self.fiat_prices.fresh_price(source, currency)
    }

    /// Get a pending fiat price request if it exists.
    pub fn pending_fiat_price_request(
        &self,
        source: PriceSource,
        currency: Currency,
    ) -> Option<&FiatPriceRequest> {
        self.fiat_prices.pending_request(source, currency)
    }

    /// Insert a fiat price into the cache.
    pub fn insert_fiat_price(&mut self, fiat_price: FiatPrice) {
        self.fiat_prices.insert_price(fiat_price);
    }

    /// Insert a pending fiat price request into the cache.
    pub fn insert_fiat_price_request(&mut self, request: FiatPriceRequest) {
        self.fiat_prices.insert_price_request(request);
    }

    /// Remove a pending fiat price request from the cache.
    pub fn remove_fiat_price_request(&mut self, source: PriceSource, currency: Currency) {
        self.fiat_prices.remove_price_request(source, currency);
    }
}

#[derive(Default)]
struct FiatPricesCache {
    /// Fiat price for a given source and currency.
    prices: HashMap<(PriceSource, Currency), FiatPrice>,
    /// Any pending requests that have not yet completed.
    pending_requests: HashMap<(PriceSource, Currency), FiatPriceRequest>,
}

impl FiatPricesCache {
    fn price(&self, source: PriceSource, currency: Currency) -> Option<&FiatPrice> {
        self.prices.get(&(source, currency))
    }

    fn fresh_price(&self, source: PriceSource, currency: Currency) -> Option<&FiatPrice> {
        self.price(source, currency)
            .filter(|price| price.requested_at().elapsed() <= FIAT_PRICE_TTL)
    }

    fn pending_request(
        &self,
        source: PriceSource,
        currency: Currency,
    ) -> Option<&FiatPriceRequest> {
        self.pending_requests.get(&(source, currency))
    }

    fn insert_price(&mut self, fiat_price: FiatPrice) {
        self.prices
            .insert((fiat_price.source(), fiat_price.currency()), fiat_price);
    }

    fn insert_price_request(&mut self, request: FiatPriceRequest) {
        self.pending_requests
            .insert((request.source, request.currency), request);
    }

    fn remove_price_request(&mut self, source: PriceSource, currency: Currency) {
        self.pending_requests.remove(&(source, currency));
    }
}
