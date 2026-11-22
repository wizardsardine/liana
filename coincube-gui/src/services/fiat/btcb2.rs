//! BTCB2 prices come only from the authenticated Connect aggregate.
//! A stale or unavailable quote is never a Bitcoin/aggregator lookup trigger.
use serde::Deserialize;

use super::{
    api::{is_usable_price, GetPriceResult, PriceApiError},
    Currency,
};
use crate::services::{
    coincube::{CoincubeClient, CoincubeError},
    http::ResponseExt,
};

/// The oldest market/conversion timestamp in a quote may be 15 minutes old;
/// each exchange observation must still be no older than 5 minutes.
/// The API's `stale` flag is always authoritative.
pub const MAX_QUOTE_AGE: u64 = 900;
pub const MAX_MARKET_AGE: u64 = 300;
pub const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
pub struct Btcb2Quote {
    pub price: f64,
    pub fiat: String,
    pub sources: Vec<PriceObservation>,
    pub median: bool,
    pub stale: bool,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PriceObservation {
    pub name: String,
    pub price: f64,
    pub at: u64,
    pub market_at: u64,
}

pub fn unix_now() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

pub fn timestamp_fresh(at: u64, now: u64) -> bool {
    at > 0 && at <= now && now - at <= MAX_QUOTE_AGE
}

fn market_timestamp_fresh(at: u64, now: u64) -> bool {
    at > 0 && at <= now && now - at <= MAX_MARKET_AGE
}

impl Btcb2Quote {
    pub fn valid_until(&self) -> Option<u64> {
        let market_until = self.sources.iter().map(|source| source.market_at).min()?;
        Some(std::cmp::min(
            self.updated_at.checked_add(MAX_QUOTE_AGE)?,
            market_until.checked_add(MAX_MARKET_AGE)?,
        ))
    }

    pub fn usable_price(
        &self,
        currency: Currency,
        now: u64,
    ) -> Result<GetPriceResult, PriceApiError> {
        let unusable =
            || PriceApiError::CannotParseData("BTCB2 quote is unavailable or stale".into());
        if self.stale
            || !self.median
            || self.fiat != currency.to_string()
            || !is_usable_price(self.price)
            || !(1..=2).contains(&self.sources.len())
            || !timestamp_fresh(self.updated_at, now)
        {
            return Err(unusable());
        }
        if self.sources.iter().any(|source| {
            !matches!(source.name.as_str(), "nonkyc" | "neoxa")
                || !is_usable_price(source.price)
                || !timestamp_fresh(source.at, now)
                || !market_timestamp_fresh(source.market_at, now)
                || source.at > source.market_at
        }) || self.updated_at
            != self
                .sources
                .iter()
                .map(|source| source.at)
                .min()
                .unwrap_or(0)
        {
            return Err(unusable());
        }
        let expected = if self.sources.len() == 1 {
            self.sources[0].price
        } else {
            let first = &self.sources[0];
            let second = &self.sources[1];
            if first.name == second.name {
                return Err(unusable());
            }
            // Half-sums avoid overflowing two otherwise finite prices.
            let midpoint = first.price / 2.0 + second.price / 2.0;
            if !midpoint.is_finite() {
                return Err(unusable());
            }
            midpoint
        };
        if (self.price - expected).abs() > expected * 1e-12 {
            return Err(unusable());
        }
        Ok(GetPriceResult {
            value: self.price,
            updated_at: Some(self.updated_at),
        })
    }
}

#[derive(Deserialize)]
struct QuoteEnvelope {
    success: bool,
    data: Btcb2Quote,
    error: Option<serde_json::Value>,
}

impl CoincubeClient {
    /// Mainnet BTCB2 only. Callers must apply `usable_price` before fiat display.
    pub async fn btcb2_quote(&self, currency: Currency) -> Result<Btcb2Quote, CoincubeError> {
        let response = self
            .client
            .get(format!("{}/api/v1/price/bitcoin-blake2b", self.base_url))
            .query(&[("fiat", currency.to_string())])
            .send()
            .await?
            .check_success()
            .await?;
        let envelope: QuoteEnvelope = response.json().await?;
        if !envelope.success || envelope.error.is_some() {
            return Err(CoincubeError::Api("Invalid BTCB2 pricing response".into()));
        }
        Ok(envelope.data)
    }
}

pub fn request_error(error: CoincubeError) -> PriceApiError {
    match error {
        CoincubeError::Unsuccessful(info) => PriceApiError::NotSuccessResponse(info),
        _ => PriceApiError::RequestFailed("BTCB2 pricing is unavailable".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::{Method::GET, MockServer};
    use serde_json::json;

    fn quote() -> Btcb2Quote {
        Btcb2Quote {
            price: 102.0,
            fiat: "USD".into(),
            median: true,
            stale: false,
            updated_at: 1000,
            sources: vec![
                PriceObservation {
                    name: "nonkyc".into(),
                    price: 100.0,
                    at: 1000,
                    market_at: 1001,
                },
                PriceObservation {
                    name: "neoxa".into(),
                    price: 104.0,
                    at: 1001,
                    market_at: 1001,
                },
            ],
        }
    }

    #[test]
    fn quote_matrix_requires_fresh_matching_aggregate() {
        assert_eq!(
            quote().usable_price(Currency::USD, 1300).unwrap().value,
            102.0
        );
        assert!(quote().usable_price(Currency::USD, 1301).is_ok());
        assert!(quote().usable_price(Currency::USD, 1302).is_err());
        assert!(quote().usable_price(Currency::EUR, 1001).is_err());
        assert!(quote().usable_price(Currency::USD, 999).is_err());
        let variants: [fn(&mut Btcb2Quote); 15] = [
            |q| q.stale = true,
            |q| q.median = false,
            |q| q.price = 100.0,
            |q| q.price = f64::NAN,
            |q| q.price = 0.0,
            |q| {
                q.sources.pop();
            },
            |q| q.sources[0].name = "bitcoin".into(),
            |q| q.sources[0].name = "neoxa".into(),
            |q| q.sources[0].at = 0,
            |q| q.sources[0].at = 1002,
            |q| q.sources[0].market_at = 0,
            |q| q.sources[0].market_at = 999,
            |q| q.sources[0].price = -1.0,
            |q| q.updated_at = 1001,
            |q| {
                q.sources[0].price = 50.0;
            },
        ];
        for mutate in variants {
            let mut q = quote();
            mutate(&mut q);
            assert!(
                q.usable_price(Currency::USD, 1001).is_err(),
                "accepted {:?}",
                q
            );
        }
    }

    #[test]
    fn either_approved_market_can_supply_a_fresh_quote() {
        for name in ["nonkyc", "neoxa"] {
            let mut q = quote();
            q.sources.retain(|source| source.name == name);
            q.price = q.sources[0].price;
            q.updated_at = q.sources[0].at;
            assert_eq!(
                q.usable_price(Currency::USD, q.updated_at + 1)
                    .unwrap()
                    .value,
                q.price
            );

            let mut invalid = q.clone();
            invalid.price += 1.0;
            assert!(invalid
                .usable_price(Currency::USD, q.updated_at + 1)
                .is_err());
            let mut invalid = q.clone();
            invalid.sources[0].market_at = q.updated_at;
            assert!(invalid
                .usable_price(Currency::USD, q.updated_at + 301)
                .is_err());
            let mut invalid = q.clone();
            invalid.stale = true;
            assert!(invalid
                .usable_price(Currency::USD, q.updated_at + 1)
                .is_err());
        }
    }

    /// Two fresh markets may disagree; the displayed price is their average.
    #[test]
    fn conversion_allows_fifteen_minutes_but_markets_only_five() {
        assert_eq!(MAX_QUOTE_AGE, 900);
        assert_eq!(MAX_MARKET_AGE, 300);
        assert!(timestamp_fresh(1000, 1121));
        assert!(timestamp_fresh(1000, 1300));
        assert!(timestamp_fresh(1000, 1900));
        assert!(!timestamp_fresh(1000, 1901));
        assert!(market_timestamp_fresh(1600, 1900));
        assert!(!market_timestamp_fresh(1600, 1901));
        let mut older_conversion = quote();
        for source in &mut older_conversion.sources {
            source.at = 1000;
            source.market_at = 1800;
        }
        assert!(older_conversion.usable_price(Currency::USD, 1900).is_ok());
        assert_eq!(older_conversion.valid_until(), Some(1900));
        assert!(older_conversion.usable_price(Currency::USD, 1901).is_err());
        let mut old_market = older_conversion.clone();
        old_market.sources[0].market_at = 1599;
        assert!(old_market.usable_price(Currency::USD, 1900).is_err());
        let mut wide = quote();
        wide.sources[0].price = 100.0;
        wide.sources[1].price = 200.0;
        wide.price = 150.0;
        assert!(wide.usable_price(Currency::USD, 1001).is_ok());
    }

    #[tokio::test]
    async fn authenticated_client_uses_only_btcb2_route_and_selected_fiat() {
        let server = MockServer::start();
        let endpoint = server.mock(|when, then| {
            when.method(GET).path("/api/v1/price/bitcoin-blake2b")
                .query_param("fiat", "EUR").header("authorization", "Bearer synthetic-price-token");
            then.status(200).json_body(json!({"success": true, "error": null, "data": {
                "price": 102, "fiat": "EUR", "median": true, "stale": false, "updated_at": 1000,
                "sources": [{"name":"nonkyc", "price":100, "at":1000, "market_at":1000}, {"name":"neoxa", "price":104, "at":1000, "market_at":1000}]
            }}));
        });
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-price-token");
        let quote = client.btcb2_quote(Currency::EUR).await.unwrap();
        endpoint.assert();
        assert_eq!(
            quote.usable_price(Currency::EUR, 1000).unwrap().value,
            102.0
        );
    }

    #[tokio::test]
    async fn endpoint_failures_never_return_a_bitcoin_quote() {
        for status in [401, 404, 429, 503] {
            let server = MockServer::start();
            let endpoint = server.mock(|when, then| {
                when.method(GET).path("/api/v1/price/bitcoin-blake2b");
                then.status(status).json_body(json!({"success":false,"data":null,"error":{"code":"SERVICE_UNAVAILABLE","message":"Unavailable"}}));
            });
            let bitcoin = server.mock(|when, then| {
                when.method(GET).path("/api/v1/exchange-rates/price/USD");
                then.status(200).json_body(json!({"value":99999}));
            });
            let client = CoincubeClient::for_test(server.base_url());
            assert!(matches!(client.btcb2_quote(Currency::USD).await,
                Err(CoincubeError::Unsuccessful(info)) if info.status_code == status));
            endpoint.assert();
            bitcoin.assert_hits(0);
        }
    }
}

#[cfg(test)]
mod malformed_quote_tests {
    use super::*;
    use httpmock::{Method::GET, MockServer};
    use serde_json::json;

    #[tokio::test]
    async fn a_bitcoin_price_body_or_missing_required_field_is_not_a_btcb2_quote() {
        for body in [
            json!({"value": 99000}),
            json!({"success":true,"data":{"price":102,"fiat":"USD"}}),
        ] {
            let server = MockServer::start();
            let endpoint = server.mock(|when, then| {
                when.method(GET).path("/api/v1/price/bitcoin-blake2b");
                then.status(200).json_body(body);
            });
            let client = CoincubeClient::for_test(server.base_url());
            assert!(client.btcb2_quote(Currency::USD).await.is_err());
            endpoint.assert();
        }
    }
}
