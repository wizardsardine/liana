use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::app::cache::{FiatPrice, FiatPriceRequest};
use crate::services::fiat::{Currency, PriceSource};

/// Time to live of the list of available currencies for a given `PriceSource`.
const CURRENCIES_TTL: Duration = Duration::from_secs(3_600); // 1 hour

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

    /// Get available currencies for a given source if they exist and are not older than `CURRENCIES_TTL`.
    pub fn fresh_currencies(&self, source: PriceSource) -> Option<&Vec<Currency>> {
        self.fiat_prices.fresh_currencies(source)
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

    /// Insert available currencies for a given source into the cache.
    pub fn insert_currencies(
        &mut self,
        source: PriceSource,
        instant: Instant,
        currencies: Vec<Currency>,
    ) {
        self.fiat_prices
            .insert_currencies(source, instant, currencies);
    }
}

#[derive(Default)]
struct FiatPricesCache {
    /// Fiat price for a given source and currency.
    prices: HashMap<(PriceSource, Currency), FiatPrice>,
    /// Any pending requests that have not yet completed.
    pending_requests: HashMap<(PriceSource, Currency), FiatPriceRequest>,
    /// Available currencies for each source, along with the instant of when they were fetched.
    currencies: HashMap<PriceSource, (Instant, Vec<Currency>)>,
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

    fn fresh_currencies(&self, source: PriceSource) -> Option<&Vec<Currency>> {
        self.currencies
            .get(&source)
            .filter(|(instant, _)| instant.elapsed() <= CURRENCIES_TTL)
            .map(|(_, list)| list)
    }

    fn insert_price(&mut self, fiat_price: FiatPrice) {
        self.prices
            .insert((fiat_price.source(), fiat_price.currency()), fiat_price);
    }

    fn insert_price_request(&mut self, request: FiatPriceRequest) {
        self.pending_requests
            .insert((request.source, request.currency), request);
    }

    fn insert_currencies(
        &mut self,
        source: PriceSource,
        instant: Instant,
        currencies: Vec<Currency>,
    ) {
        self.currencies.insert(source, (instant, currencies));
    }

    fn remove_price_request(&mut self, source: PriceSource, currency: Currency) {
        self.pending_requests.remove(&(source, currency));
    }
}
