//! Liquid cross-asset Swap panel.
//!
//! Surfaces the existing Breez SDK Liquid cross-asset (SideSwap) rail —
//! already wired through [`crate::app::state::liquid::send`] as a
//! self-targeted cross-asset send — as a dedicated Aqua-style Swap
//! screen for L-BTC ↔ USDt.
//!
//! PR 1 scope: routing + entry points only. This panel loads balances
//! and renders a placeholder; the debounced quote engine and
//! review/confirm flow land in later PRs.

use std::convert::TryInto;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{Amount, Network};
use coincube_ui::widget::Element;
use iced::Task;

use super::send::SendAsset;
use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::state::State;
use crate::app::wallets::LiquidBackend;
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// Swap is only available where SideSwap is — i.e. mainnet. Mirrors the
/// `cross_asset_supported` predicate in [`crate::app::state::liquid::send`]
/// so the Swap entry points and quote engine share one capability gate.
pub fn swap_supported(network: Network) -> bool {
    matches!(network, Network::Bitcoin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_supported_only_on_mainnet() {
        // Must match `LiquidSend::cross_asset_supported`, which gates the
        // cross-asset send path on `Network::Bitcoin`.
        assert!(swap_supported(Network::Bitcoin));
        assert!(!swap_supported(Network::Testnet));
        assert!(!swap_supported(Network::Signet));
        assert!(!swap_supported(Network::Regtest));
    }
}

/// Cross-asset swap flow state. Initialised L-BTC → USDt (Aqua default).
pub struct LiquidSwap {
    breez_client: Arc<LiquidBackend>,
    /// Asset the user pays with.
    from_asset: SendAsset,
    /// Asset the user receives.
    to_asset: SendAsset,
    btc_balance: Amount,
    usdt_balance: u64,
    error: Option<String>,
}

impl LiquidSwap {
    pub fn new(breez_client: Arc<LiquidBackend>) -> Self {
        Self {
            breez_client,
            from_asset: SendAsset::Lbtc,
            to_asset: SendAsset::Usdt,
            btc_balance: Amount::from_sat(0),
            usdt_balance: 0,
            error: None,
        }
    }

    fn load_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                let info = breez_client.info().await;
                let btc_balance = info
                    .as_ref()
                    .map(|info| {
                        Amount::from_sat(
                            info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat,
                        )
                    })
                    .unwrap_or(Amount::ZERO);

                let usdt_id =
                    crate::app::breez_liquid::assets::usdt_asset_id(breez_client.network())
                        .unwrap_or("");
                let usdt_balance = info
                    .as_ref()
                    .ok()
                    .and_then(|info| {
                        info.wallet_info
                            .asset_balances
                            .iter()
                            .find_map(|ab| (ab.asset_id == usdt_id).then_some(ab.balance_sat))
                    })
                    .unwrap_or(0);

                match info {
                    Ok(_) => Ok((btc_balance, usdt_balance)),
                    Err(_) => Err("Couldn't fetch account balance".to_string()),
                }
            },
            |result| match result {
                Ok((btc_balance, usdt_balance)) => Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::DataLoaded {
                        btc_balance,
                        usdt_balance,
                    },
                )),
                Err(err) => Message::View(view::Message::LiquidSwap(
                    view::LiquidSwapMessage::Error(err),
                )),
            },
        )
    }
}

impl State for LiquidSwap {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
        let swap_view = view::liquid::liquid_swap_view(view::liquid::LiquidSwapConfig {
            from_asset: self.from_asset,
            to_asset: self.to_asset,
            btc_balance: self.btc_balance,
            usdt_balance: self.usdt_balance,
            fiat_converter,
            bitcoin_unit: cache.bitcoin_unit,
            error: self.error.as_deref(),
        })
        .map(view::Message::LiquidSwap);

        view::dashboard(menu, cache, swap_view)
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::LiquidSwap(ref msg)) = message {
            match msg {
                view::LiquidSwapMessage::DataLoaded {
                    btc_balance,
                    usdt_balance,
                } => {
                    self.error = None;
                    self.btc_balance = *btc_balance;
                    self.usdt_balance = *usdt_balance;
                }
                view::LiquidSwapMessage::Error(err) => {
                    self.error = Some(err.clone());
                }
                view::LiquidSwapMessage::RefreshRequested => {
                    return self.load_balance();
                }
            }
        }
        Task::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        // Fast balance load plus a background SDK sync — matches the
        // Overview panel's reload so figures reconcile after a swap.
        let breez = self.breez_client.clone();
        Task::batch(vec![
            Task::perform(
                async move {
                    let _ = breez.sync().await;
                },
                |_| Message::CacheUpdated,
            ),
            self.load_balance(),
        ])
    }
}
