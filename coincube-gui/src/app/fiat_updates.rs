//! App-owned fiat result routing and cache admission.
use super::*;

impl App {
    pub(super) fn handle_fiat_price(&mut self, fiat_price: cache::FiatPrice) -> Task<Message> {
        if self.cube_settings.network.is_blake2b() {
            let relevant = self
                .cube_settings
                .fiat_price
                .as_ref()
                .is_some_and(|setting| {
                    setting.is_enabled && setting.currency == fiat_price.currency()
                });
            if relevant && self.cache.accept_btcb2_price(fiat_price) {
                return Task::done(Message::CacheUpdated);
            }
            return Task::none();
        }
        // A scoped BTCB2 quote must never seed Bitcoin/USD conversion.
        if fiat_price.request.origin.is_some() {
            return Task::none();
        }
        let mut updated = false;

        // Always extract BTC/USD price for USDt→sats conversion,
        // regardless of whether fiat display is enabled.
        if fiat_price.currency() == crate::services::fiat::Currency::USD
            && self
                .cache
                .btc_usd_requested_at
                .is_none_or(|at| at < fiat_price.requested_at())
        {
            if let Ok(price) = fiat_price.res.as_ref() {
                self.cache.btc_usd_price = Some(price.value);
                self.cache.btc_usd_requested_at = Some(fiat_price.requested_at());
                updated = true;
            }
        }

        // Store user's selected currency price (only when fiat display is enabled).
        let is_relevant = self.cube_settings.fiat_price.as_ref().is_some_and(|sett| {
            sett.is_enabled
                && sett.source == fiat_price.source()
                && sett.currency == fiat_price.currency()
        });

        if is_relevant
            // make sure we only update if the price is newer than the cached one
            && !self.cache.fiat_price.as_ref().is_some_and(|cached| {
                cached.source() == fiat_price.source()
                    && cached.currency() == fiat_price.currency()
                    && cached.requested_at() >= fiat_price.requested_at()
            })
        {
            self.cache.fiat_price = Some(fiat_price);
            updated = true;
        }

        if updated {
            return Task::done(Message::CacheUpdated);
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::fiat::{api::GetPriceResult, Currency, PriceSource};
    use std::time::{Duration, Instant};

    #[test]
    fn late_usd_response_does_not_replace_the_newer_conversion_rate() {
        let _guard = session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = shell_tests::bitcoin_app(root.path());
        app.cube_settings.fiat_price = None;
        let start = Instant::now();
        let quote = |instant, value| {
            let mut request = cache::FiatPriceRequest::new(PriceSource::Coincube, Currency::USD);
            request.instant = instant;
            cache::FiatPrice {
                request,
                res: Ok(GetPriceResult {
                    value,
                    updated_at: None,
                }),
                btcb2_valid_until: None,
            }
        };
        assert!(iced_runtime::task::into_stream(
            app.handle_fiat_price(quote(start + Duration::from_secs(1), 66_000.0))
        )
        .is_some());
        assert!(
            iced_runtime::task::into_stream(app.handle_fiat_price(quote(start, 60_000.0)))
                .is_none()
        );
        assert_eq!(app.cache.btc_usd_price, Some(66_000.0));
        assert!(app.cache.fiat_price.is_none());
    }
}
