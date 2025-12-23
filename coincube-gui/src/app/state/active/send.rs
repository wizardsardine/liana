use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::model::PaymentDetails;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{component::form, widget::*};
use iced::Task;

use crate::app::menu::{ActiveSubMenu, Menu};
use crate::app::state::{redirect, State};
use crate::app::{breez::BreezClient, cache::Cache};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

/// ActiveSend manages the Lightning Network send interface
pub struct ActiveSend {
    breez_client: Arc<BreezClient>,
    btc_balance: Amount,
    recent_transaction: Vec<view::active::RecentTransaction>,
    input: form::Value<String>,
    error: Option<String>,
}

impl ActiveSend {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            btc_balance: Amount::from_sat(0),
            recent_transaction: Vec::new(),
            input: form::Value::default(),
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
                        Amount::from_sat(
                            (info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat)
                                .saturating_sub(info.wallet_info.pending_send_sat),
                        )
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
                    Message::View(view::Message::ActiveSend(view::ActiveSendMessage::Error(
                        err,
                    )))
                } else {
                    Message::View(view::Message::ActiveSend(
                        view::ActiveSendMessage::DataLoaded {
                            balance,
                            recent_payment,
                        },
                    ))
                }
            },
        )
    }
}

impl State for ActiveSend {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        let send_view = view::active::active_send_view(
            self.btc_balance,
            fiat_converter,
            &self.recent_transaction,
            &self.input,
            self.error.as_deref(),
        )
        .map(view::Message::ActiveSend);

        view::dashboard(menu, cache, None, send_view)
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::ActiveSend(msg)) = message {
            match msg {
                view::ActiveSendMessage::InvoiceEdited(value) => {
                    self.input.value = value;
                    self.input.valid = !self.input.value.trim().is_empty();
                    self.error = None;
                }
                view::ActiveSendMessage::Send => {
                    tracing::info!("Send payment to: {}", self.input.value);
                }
                view::ActiveSendMessage::History => {
                    return redirect(Menu::Active(ActiveSubMenu::Transactions(None)));
                }
                view::ActiveSendMessage::DataLoaded {
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
                                let time_ago = format_time_ago(payment.timestamp);
                                let fiat_amount = fiat_converter
                                    .as_ref()
                                    .map(|c: &view::FiatAmountConverter| c.convert(amount));

                                let desc = match &payment.details {
                                    PaymentDetails::Lightning { description, .. }
                                    | PaymentDetails::Liquid { description, .. }
                                    | PaymentDetails::Bitcoin { description, .. } => description,
                                };

                                let is_incoming = matches!(
                                    payment.payment_type,
                                    breez_sdk_liquid::prelude::PaymentType::Receive
                                );
                                let sign = if is_incoming { "+" } else { "-" };
                                view::active::RecentTransaction {
                                    description: desc.to_owned(),
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
                view::ActiveSendMessage::BreezEvent(event) => {
                    use breez_sdk_liquid::prelude::SdkEvent;
                    log::info!("Received Breez Event: {:?}", event);
                    match event {
                        SdkEvent::PaymentPending { .. }
                        | SdkEvent::PaymentSucceeded { .. }
                        | SdkEvent::PaymentFailed { .. }
                        | SdkEvent::PaymentWaitingConfirmation { .. } => {
                            return self.load_balance();
                        }
                        _ => {}
                    }
                }
                view::ActiveSendMessage::Error(err) => {
                    self.error = Some(err);
                }
            }
        }
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        self.breez_client.subscription().map(|e| {
            Message::View(view::Message::ActiveSend(
                view::ActiveSendMessage::BreezEvent(e),
            ))
        })
    }

    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        _wallet: Arc<Wallet>,
    ) -> Task<Message> {
        self.load_balance()
    }
}
