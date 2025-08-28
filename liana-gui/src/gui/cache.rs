use std::collections::HashMap;

use crate::app::cache::{FiatPrice, FiatPriceRequest, FIAT_PRICE_UPDATE_INTERVAL_SECS};
use crate::app::message::FiatMessage;
use crate::services::fiat::{Currency, PriceSource};
use crate::utils::now;

/// Time to live of the list of available currencies for a given `PriceSource`.
const CURRENCIES_LIST_TTL_SECS: u64 = 3_600; // 1 hour

#[derive(Default)]
pub struct FiatPricesCache {
    /// Fiat price for a given source and currency.
    prices: HashMap<(PriceSource, Currency), FiatPrice>,
    /// Last request for a given source and currency, if any,
    /// which may not have completed yet.
    last_requests: HashMap<(PriceSource, Currency), FiatPriceRequest>,
    /// Available currencies for each source, along with the timestamp of when they were fetched.
    currencies: HashMap<PriceSource, (u64, Vec<Currency>)>,
}

/// A global cache for the application.
#[derive(Default)]
pub struct GlobalCache {
    fiat_prices: FiatPricesCache,
}

impl GlobalCache {
    /// Get a fiat price from the cache if it exists.
    pub fn fiat_price(&self, source: PriceSource, currency: Currency) -> Option<&FiatPrice> {
        self.fiat_prices.prices.get(&(source, currency))
    }

    /// Insert a fiat price into the cache.
    pub fn insert_fiat_price(&mut self, fiat_price: FiatPrice) {
        self.fiat_prices.prices.insert(
            (fiat_price.source(), fiat_price.currency()),
            fiat_price.clone(),
        );
    }

    /// Get the last fiat price request from the cache if it exists.
    pub fn last_fiat_price_request(
        &self,
        source: PriceSource,
        currency: Currency,
    ) -> Option<&FiatPriceRequest> {
        self.fiat_prices.last_requests.get(&(source, currency))
    }

    /// Insert a fiat price request into the cache.
    pub fn insert_fiat_price_request(&mut self, request: FiatPriceRequest) {
        self.fiat_prices
            .last_requests
            .insert((request.source, request.currency), request);
    }

    /// Insert a list of available currencies for a given source into the cache.
    pub fn insert_currencies(
        &mut self,
        source: PriceSource,
        timestamp: u64,
        currencies: Vec<Currency>,
    ) {
        self.fiat_prices
            .currencies
            .insert(source, (timestamp, currencies));
    }

    /// Process a fiat message and determine what action should be taken.
    pub fn fiat_message_action(&self, fiat_msg: &FiatMessage) -> FiatMessageAction {
        match fiat_msg {
            FiatMessage::GetPrice(source, currency) => {
                self.process_price_request(*source, *currency)
            }
            FiatMessage::ListCurrencies(source) => self.process_currencies_request(*source),
            _ => FiatMessageAction::None,
        }
    }

    /// Process a price request and determine if we can use cached data or need to fetch.
    fn process_price_request(&self, source: PriceSource, currency: Currency) -> FiatMessageAction {
        // Check if we have a valid cached price.
        // We add a small buffer of 5 seconds to ensure we don't accidentally skip a regular update.
        let now = now().as_secs();
        if let Some(cached_price) = self
            .fiat_price(source, currency)
            .filter(|p| p.requested_at() + FIAT_PRICE_UPDATE_INTERVAL_SECS > now + 5)
        {
            return FiatMessageAction::UseCachedPrice(cached_price.clone());
        }
        FiatMessageAction::RequestPrice(source, currency)
    }

    /// Process a currencies list request and determine if we can use cached data or need to fetch.
    fn process_currencies_request(&self, source: PriceSource) -> FiatMessageAction {
        let now = now().as_secs();

        // Check if we have valid cached currencies
        if let Some((timestamp, currencies)) = self.fiat_prices.currencies.get(&source) {
            if now.saturating_sub(*timestamp) <= CURRENCIES_LIST_TTL_SECS {
                return FiatMessageAction::UseCachedCurrencies(
                    source,
                    *timestamp,
                    currencies.clone(),
                );
            }
        }
        // Need to request currencies
        FiatMessageAction::RequestCurrencies(source)
    }
}

/// Represents a required action as determined by the cache.
#[derive(Debug, Clone)]
pub enum FiatMessageAction {
    /// Fetch the price using the contained request.
    RequestPrice(PriceSource, Currency),
    /// Use the cached fiat price.
    UseCachedPrice(FiatPrice),
    /// Send a new request for currencies for the contained source.
    RequestCurrencies(PriceSource),
    /// Use the cached currencies list.
    UseCachedCurrencies(PriceSource, u64, Vec<Currency>),
    /// No specific action needed.
    None,
}
