mod active;
mod global_home;
pub mod vault;

#[cfg(feature = "buysell")]
pub mod buysell;

use std::convert::TryInto;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{Amount, OutPoint};
use coincube_ui::widget::*;
use iced::{Subscription, Task};

use super::{cache::Cache, menu::Menu, message::Message, view, wallet::Wallet};

pub const HISTORY_EVENT_PAGE_SIZE: u64 = 20;

use crate::daemon::model::coin_is_owned;
use crate::daemon::{
    model::{remaining_sequence, Coin},
    Daemon,
};
pub use active::overview::ActiveOverview;
pub use active::receive::ActiveReceive;
pub use active::send::ActiveSend;
pub use active::settings::ActiveSettings;
pub use active::transactions::ActiveTransactions;
pub use global_home::GlobalHome;
pub use vault::coins::CoinsPanel;
pub use vault::label::LabelsEdited;
pub use vault::overview::VaultOverview;
pub use vault::psbts::PsbtsPanel;
pub use vault::receive::VaultReceivePanel;
pub use vault::settings::SettingsState;
pub use vault::spend::CreateSpendPanel;
pub use vault::transactions::VaultTransactionsPanel;

pub trait State {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message>;
    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _cache: &Cache,
        _message: Message,
    ) -> Task<Message> {
        Task::none()
    }
    fn subscription(&self) -> Subscription<Message> {
        Subscription::none()
    }

    fn close(&mut self) -> Task<Message> {
        Task::none()
    }
    fn interrupt(&mut self) {}
    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        Task::none()
    }
}

/// redirect to another state with a message menu
pub fn redirect(menu: Menu) -> Task<Message> {
    Task::perform(async { menu }, |menu| {
        Message::View(view::Message::Menu(menu))
    })
}

/// Returns fiat converter if the wallet setting is enabled and the cached price matches the setting.
pub fn fiat_converter_for_wallet(
    wallet: &Wallet,
    cache: &Cache,
) -> Option<view::FiatAmountConverter> {
    cache
        .fiat_price
        .as_ref()
        .filter(|p| wallet.fiat_price_is_relevant(p))
        .and_then(|p| p.try_into().ok())
}

/// Returns the confirmed and unconfirmed balances from `coins`, as well
/// as:
/// - the `OutPoint`s of those coins, if any, for which the current
///   `tip_height` is within 10% of the `timelock` expiring.
/// - the smallest number of blocks until the expiry of `timelock` among
///   all confirmed coins, if any.
///
/// The confirmed balance includes the values of any unconfirmed coins
/// from self.
fn coins_summary(
    coins: &[Coin],
    tip_height: u32,
    timelock: u16,
) -> (Amount, Amount, Vec<OutPoint>, Option<u32>) {
    let mut balance = Amount::from_sat(0);
    let mut unconfirmed_balance = Amount::from_sat(0);
    let mut expiring_coins = Vec::new();
    let mut remaining_seq = None;
    for coin in coins {
        if coin.spend_info.is_none() {
            // Include unconfirmed coins from self in confirmed balance.
            if coin_is_owned(coin) {
                balance += coin.amount;
                // Only consider confirmed coins for remaining seq
                // (they would not be considered as expiring so we can also skip that part)
                if coin.block_height.is_none() {
                    continue;
                }
                let seq = remaining_sequence(coin, tip_height, timelock);
                // Warn user for coins that are expiring in less than 10 percent of
                // the timelock.
                if seq <= timelock as u32 * 10 / 100 {
                    expiring_coins.push(coin.outpoint);
                }
                if let Some(last) = &mut remaining_seq {
                    if seq < *last {
                        *last = seq
                    }
                } else {
                    remaining_seq = Some(seq);
                }
            } else {
                unconfirmed_balance += coin.amount;
            }
        }
    }
    (balance, unconfirmed_balance, expiring_coins, remaining_seq)
}
