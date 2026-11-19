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
            // Keep the last good quote for this source and currency on errors.
            && !self.cache.fiat_price.as_ref().is_some_and(|cached| {
                cached.source() == fiat_price.source()
                    && cached.currency() == fiat_price.currency()
                    && (cached.requested_at() >= fiat_price.requested_at()
                        || (cached.res.is_ok() && fiat_price.res.is_err()))
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
    fn invalid_response_does_not_replace_last_good_display_quote() {
        let _guard = session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let root = tempfile::tempdir().unwrap();
        let (mut app, _) = shell_tests::bitcoin_app(root.path());
        app.cube_settings.fiat_price = Some(settings::fiat::PriceSetting {
            source: PriceSource::Coincube,
            currency: Currency::EUR,
            is_enabled: true,
        });

        let start = Instant::now();
        let quote = |seconds, value| {
            let mut request = cache::FiatPriceRequest::new(PriceSource::Coincube, Currency::EUR);
            request.instant = start + Duration::from_secs(seconds);
            cache::FiatPrice {
                request,
                res: PriceSource::Coincube
                    .parse_price_data(Currency::EUR, &serde_json::json!({"value": value})),
                btcb2_valid_until: None,
            }
        };

        // An initial failure still reaches the display when no quote is available.
        assert!(iced_runtime::task::into_stream(app.handle_fiat_price(quote(0, 0.0))).is_some());
        assert!(app.cache.fiat_price.as_ref().unwrap().res.is_err());
        assert!(
            iced_runtime::task::into_stream(app.handle_fiat_price(quote(1, 60_000.0))).is_some()
        );

        assert!(iced_runtime::task::into_stream(app.handle_fiat_price(quote(3, -1.0))).is_none());
        let held = app.cache.fiat_price.as_ref().unwrap();
        assert_eq!(held.requested_at(), start + Duration::from_secs(1));
        assert_eq!(held.res.as_ref().unwrap().value, 60_000.0);

        // A rejected failure must not advance the quote's freshness watermark.
        assert!(
            iced_runtime::task::into_stream(app.handle_fiat_price(quote(2, 61_000.0))).is_some()
        );
        assert_eq!(
            app.cache
                .fiat_price
                .as_ref()
                .unwrap()
                .res
                .as_ref()
                .unwrap()
                .value,
            61_000.0
        );
    }

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
