use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::model::PaymentDetails;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::widget::*;
use iced::Task;

use crate::app::menu::{ActiveSubMenu, Menu};
use crate::app::state::active::send::format_time_ago;
use crate::app::state::{redirect, State};
use crate::app::{breez::BreezClient, cache::Cache};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;

/// ActiveOverview
pub struct ActiveOverview {
    breez_client: Arc<BreezClient>,
    btc_balance: Amount,
    recent_transaction: Vec<view::active::RecentTransaction>,
    error: Option<String>,
}

impl ActiveOverview {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            btc_balance: Amount::from_sat(0),
            recent_transaction: Vec::new(),
            error: None,
        }
    }

    fn load_data(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();
        Task::perform(
            async move {
                let info = breez_client.info().await.ok();
                let balance = info
                    .as_ref()
                    .map(|i| {
                        Amount::from_sat(
                            i.wallet_info.balance_sat + i.wallet_info.pending_receive_sat
                                - i.wallet_info.pending_send_sat,
                        )
                    })
                    .unwrap_or(Amount::from_sat(0));

                let payments = breez_client
                    .list_payments(Some(2))
                    .await
                    .ok()
                    .unwrap_or(Vec::new());

                (balance, payments)
            },
            |(balance, recent_payment)| {
                Message::View(view::Message::ActiveOverview(
                    view::ActiveOverviewMessage::DataLoaded {
                        balance,
                        recent_payment,
                    },
                ))
            },
        )
    }
}

impl State for ActiveOverview {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        let send_view = view::active::active_overview_view(
            self.btc_balance,
            fiat_converter,
            &self.recent_transaction,
            self.error.as_deref(),
        )
        .map(view::Message::ActiveOverview);

        view::dashboard(menu, cache, None, send_view)
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::ActiveOverview(msg)) = message {
            match msg {
                view::ActiveOverviewMessage::Send => {
                    return redirect(Menu::Active(ActiveSubMenu::Send));
                }
                view::ActiveOverviewMessage::Receive => {
                    return redirect(Menu::Active(ActiveSubMenu::Receive));
                }
                view::ActiveOverviewMessage::History => {
                    return redirect(Menu::Active(ActiveSubMenu::Transactions(None)));
                }
                view::ActiveOverviewMessage::DataLoaded {
                    balance,
                    recent_payment,
                } => {
                    self.btc_balance = balance;

                    if recent_payment.len() > 0 {
                        let fiat_converter: Option<view::FiatAmountConverter> =
                            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                        let txns = recent_payment
                            .into_iter()
                            .map(|payment| {
                                let amount = Amount::from_sat(payment.amount_sat);
                                let status = payment.status;
                                let mut description = String::from("Zap!");
                                let time_ago = format_time_ago(payment.timestamp);
                                let fiat_amount = fiat_converter
                                    .as_ref()
                                    .map(|c: &view::FiatAmountConverter| c.convert(amount));

                                let d = match &payment.details {
                                    PaymentDetails::Lightning { description, .. }
                                    | PaymentDetails::Liquid { description, .. }
                                    | PaymentDetails::Bitcoin { description, .. } => description,
                                };

                                if !d.is_empty() {
                                    description = format!("{} ({})", description, d);
                                }
                                let is_incoming = matches!(
                                    payment.payment_type,
                                    breez_sdk_liquid::prelude::PaymentType::Receive
                                );
                                let sign = if is_incoming { "+" } else { "-" };
                                view::active::RecentTransaction {
                                    description,
                                    time_ago,
                                    amount,
                                    fiat_amount,
                                    is_incoming,
                                    sign,
                                    status,
                                }
                            })
                            .collect();
                        self.recent_transaction = txns;
                    }
                }
                view::ActiveOverviewMessage::BreezEvent(event) => {
                    use breez_sdk_liquid::prelude::SdkEvent;
                    log::info!("Received Breez Event: {:?}", event);
                    match event {
                        SdkEvent::PaymentPending { .. }
                        | SdkEvent::PaymentSucceeded { .. }
                        | SdkEvent::PaymentFailed { .. }
                        | SdkEvent::PaymentWaitingConfirmation { .. } => {
                            return self.load_data();
                        }
                        _ => {}
                    }
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        self.breez_client.subscription().map(|e| {
            Message::View(view::Message::ActiveOverview(
                view::ActiveOverviewMessage::BreezEvent(e),
            ))
        })
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.load_data()
    }
}
