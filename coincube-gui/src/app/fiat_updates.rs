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
        if fiat_price.currency() == crate::services::fiat::Currency::USD {
            if let Ok(price) = fiat_price.res.as_ref() {
                self.cache.btc_usd_price = Some(price.value);
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
