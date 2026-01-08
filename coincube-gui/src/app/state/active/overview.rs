use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::model::PaymentDetails;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::widget::*;
use iced::Task;

use crate::app::menu::{ActiveSubMenu, Menu};
use crate::app::state::{redirect, State};
use crate::app::{breez::BreezClient, cache::Cache};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

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

    fn load_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();

        Task::perform(
            async move {
                let info = breez_client.info().await;
                let payments = breez_client.list_payments(Some(2)).await;

                let balance = info
                    .as_ref()
                    .map(|info| {
                        let balance =
                            info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat;
                        Amount::from_sat(balance)
                    })
                    .unwrap_or(Amount::ZERO);

                let error = match (&info, &payments) {
                    (Err(_), Err(_)) => Some("Couldn't fetch balance or transactions".to_string()),
                    (Err(_), _) => Some("Couldn't fetch account balance".to_string()),
                    (_, Err(_)) => Some("Couldn't fetch recent transactions".to_string()),
                    _ => None,
                };

                let payments = payments.unwrap_or_default();

                (balance, payments, error)
            },
            |(balance, recent_payment, error)| {
                if let Some(err) = error {
                    Message::View(view::Message::ActiveOverview(
                        view::ActiveOverviewMessage::Error(err),
                    ))
                } else {
                    Message::View(view::Message::ActiveOverview(
                        view::ActiveOverviewMessage::DataLoaded {
                            balance,
                            recent_payment,
                        },
                    ))
                }
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
            cache.bitcoin_unit.into(),
        )
        .map(view::Message::ActiveOverview);

        view::dashboard(menu, cache, None, send_view)
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
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

                    if !recent_payment.is_empty() {
                        let fiat_converter: Option<view::FiatAmountConverter> =
                            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                        let txns = recent_payment
                            .into_iter()
                            .map(|payment| {
                                let amount = Amount::from_sat(payment.amount_sat);
                                let status = payment.status;
                                let time_ago = format_time_ago(payment.timestamp);
                                let fiat_amount = fiat_converter
                                    .as_ref()
                                    .map(|c: &view::FiatAmountConverter| c.convert(amount));

                                let desc = match &payment.details {
                                    PaymentDetails::Lightning {
                                        payer_note,
                                        description,
                                        ..
                                    } => payer_note
                                        .as_ref()
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or(description),
                                    PaymentDetails::Liquid {
                                        payer_note,
                                        description,
                                        ..
                                    } => payer_note
                                        .as_ref()
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or(description),

                                    PaymentDetails::Bitcoin { description, .. } => description,
                                };

                                let is_incoming = matches!(
                                    payment.payment_type,
                                    breez_sdk_liquid::prelude::PaymentType::Receive
                                );
                                let details = payment.details.clone();
                                let sign = if is_incoming { "+" } else { "-" };
                                view::active::RecentTransaction {
                                    description: desc.to_owned(),
                                    time_ago,
                                    amount,
                                    fiat_amount,
                                    is_incoming,
                                    sign,
                                    status,
                                    details,
                                }
                            })
                            .collect();
                        self.recent_transaction = txns;
                    }
                }
                view::ActiveOverviewMessage::Error(err) => {
                    self.error = Some(err);
                }
                view::ActiveOverviewMessage::RefreshRequested => {
                    return self.load_balance();
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.load_balance()
    }
}
