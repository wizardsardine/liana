use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::model::PaymentDetails;
use breez_sdk_liquid::prelude::Payment;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::widget::*;
use iced::Task;

use crate::app::breez::assets::usdt_asset_id;
use crate::app::menu::{LiquidSubMenu, Menu};
use crate::app::state::{redirect, State};
use crate::app::{breez::BreezClient, cache::Cache};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

/// LiquidOverview
pub struct LiquidOverview {
    breez_client: Arc<BreezClient>,
    btc_balance: Amount,
    usdt_balance: u64,
    recent_transaction: Vec<view::liquid::RecentTransaction>,
    recent_payments: Vec<Payment>,
    selected_payment: Option<Payment>,
    error: Option<String>,
}

impl LiquidOverview {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            btc_balance: Amount::from_sat(0),
            usdt_balance: 0,
            recent_transaction: Vec::new(),
            recent_payments: Vec::new(),
            selected_payment: None,
            error: None,
        }
    }

    fn load_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();

        Task::perform(
            async move {
                let info = breez_client.info().await;
                let payments = breez_client.list_payments(Some(20)).await;

                let balance = info
                    .as_ref()
                    .map(|info| {
                        let balance =
                            info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat;
                        Amount::from_sat(balance)
                    })
                    .unwrap_or(Amount::ZERO);

                let usdt_id = usdt_asset_id(breez_client.network()).unwrap_or("");
                let usdt_balance = info
                    .as_ref()
                    .ok()
                    .and_then(|info| {
                        info.wallet_info.asset_balances.iter().find_map(|ab| {
                            if ab.asset_id == usdt_id {
                                Some(ab.balance_sat)
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or(0);

                let error = match (&info, &payments) {
                    (Err(_), Err(_)) => Some("Couldn't fetch balance or transactions".to_string()),
                    (Err(_), _) => Some("Couldn't fetch account balance".to_string()),
                    (_, Err(_)) => Some("Couldn't fetch recent transactions".to_string()),
                    _ => None,
                };

                let payments = payments.unwrap_or_default();

                (balance, usdt_balance, payments, error)
            },
            |(balance, usdt_balance, recent_payment, error)| {
                if let Some(err) = error {
                    Message::View(view::Message::LiquidOverview(
                        view::LiquidOverviewMessage::Error(err),
                    ))
                } else {
                    Message::View(view::Message::LiquidOverview(
                        view::LiquidOverviewMessage::DataLoaded {
                            balance,
                            usdt_balance,
                            recent_payment,
                        },
                    ))
                }
            },
        )
    }
}

impl State for LiquidOverview {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        if let Some(payment) = &self.selected_payment {
            view::dashboard(
                menu,
                cache,
                view::liquid::transaction_detail_view(
                    payment,
                    fiat_converter,
                    cache.bitcoin_unit,
                    usdt_asset_id(self.breez_client.network()).unwrap_or(""),
                ),
            )
        } else {
            let send_view = view::liquid::liquid_overview_view(
                self.btc_balance,
                self.usdt_balance,
                fiat_converter,
                &self.recent_transaction,
                self.error.as_deref(),
                cache.bitcoin_unit,
                cache.btc_usd_price,
            )
            .map(view::Message::LiquidOverview);

            view::dashboard(menu, cache, send_view)
        }
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::LiquidOverview(ref msg)) = message {
            match msg {
                view::LiquidOverviewMessage::SendLbtc => {
                    return Task::batch(vec![
                        redirect(Menu::Liquid(LiquidSubMenu::Send)),
                        Task::done(Message::View(view::Message::LiquidSend(
                            view::LiquidSendMessage::PresetAsset(
                                crate::app::state::liquid::send::SendAsset::Lbtc,
                            ),
                        ))),
                    ]);
                }
                view::LiquidOverviewMessage::ReceiveLbtc => {
                    return Task::batch(vec![
                        redirect(Menu::Liquid(LiquidSubMenu::Receive)),
                        Task::done(Message::View(view::Message::LiquidReceive(
                            view::LiquidReceiveMessage::SetReceiveAsset(
                                crate::app::state::liquid::send::SendAsset::Lbtc,
                            ),
                        ))),
                    ]);
                }
                view::LiquidOverviewMessage::SendUsdt => {
                    return Task::batch(vec![
                        redirect(Menu::Liquid(LiquidSubMenu::Send)),
                        Task::done(Message::View(view::Message::LiquidSend(
                            view::LiquidSendMessage::PresetAsset(
                                crate::app::state::liquid::send::SendAsset::Usdt,
                            ),
                        ))),
                    ]);
                }
                view::LiquidOverviewMessage::ReceiveUsdt => {
                    return Task::batch(vec![
                        redirect(Menu::Liquid(LiquidSubMenu::Receive)),
                        Task::done(Message::View(view::Message::LiquidReceive(
                            view::LiquidReceiveMessage::SetReceiveAsset(
                                crate::app::state::liquid::send::SendAsset::Usdt,
                            ),
                        ))),
                    ]);
                }
                view::LiquidOverviewMessage::History => {
                    return redirect(Menu::Liquid(LiquidSubMenu::Transactions(None)));
                }
                view::LiquidOverviewMessage::SelectTransaction(idx) => {
                    if let Some(payment) = self.recent_payments.get(*idx).cloned() {
                        self.selected_payment = Some(payment.clone());
                        return Task::batch(vec![
                            redirect(Menu::Liquid(LiquidSubMenu::Transactions(None))),
                            Task::done(Message::View(view::Message::PreselectPayment(payment))),
                        ]);
                    }
                }
                view::LiquidOverviewMessage::DataLoaded {
                    balance,
                    usdt_balance,
                    recent_payment,
                } => {
                    self.error = None;
                    self.btc_balance = *balance;
                    self.usdt_balance = *usdt_balance;

                    let recent: Vec<Payment> = recent_payment.iter().take(5).cloned().collect();
                    self.recent_payments = recent.clone();

                    let usdt_id =
                        crate::app::breez::assets::usdt_asset_id(self.breez_client.network())
                            .unwrap_or("");

                    if !recent.is_empty() {
                        let fiat_converter: Option<view::FiatAmountConverter> =
                            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                        let txns = recent
                            .iter()
                            .map(|payment| {
                                let status = payment.status;
                                let time_ago = format_time_ago(payment.timestamp.into());
                                let amount = Amount::from_sat(payment.amount_sat);
                                let fiat_amount = fiat_converter
                                    .as_ref()
                                    .map(|c: &view::FiatAmountConverter| c.convert(amount));

                                // Detect USDt payments and build display string
                                let is_usdt = matches!(
                                    &payment.details,
                                    PaymentDetails::Liquid { asset_id, .. }
                                        if !usdt_id.is_empty() && asset_id == usdt_id
                                );

                                let (desc, usdt_display) = if is_usdt {
                                    let display =
                                        if let PaymentDetails::Liquid { asset_info, .. } =
                                            &payment.details
                                        {
                                            if let Some(info) = asset_info {
                                                crate::app::breez::assets::format_usdt_display(
                                                (info.amount
                                                    * 10_f64.powi(
                                                        crate::app::breez::assets::USDT_PRECISION
                                                            as i32,
                                                    ))
                                                .round()
                                                    as u64,
                                            )
                                            } else {
                                                crate::app::breez::assets::format_usdt_display(
                                                    payment.amount_sat,
                                                )
                                            }
                                        } else {
                                            crate::app::breez::assets::format_usdt_display(
                                                payment.amount_sat,
                                            )
                                        };
                                    (
                                        "USDt Transfer".to_owned(),
                                        Some(format!("{} USDt", display)),
                                    )
                                } else {
                                    let d: &str = match &payment.details {
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
                                    (d.to_owned(), None)
                                };

                                let is_incoming = matches!(
                                    payment.payment_type,
                                    breez_sdk_liquid::prelude::PaymentType::Receive
                                );
                                let details = payment.details.clone();
                                let fees_sat = Amount::from_sat(payment.fees_sat);
                                view::liquid::RecentTransaction {
                                    description: desc,
                                    time_ago,
                                    amount,
                                    fiat_amount,
                                    is_incoming,
                                    status,
                                    details,
                                    fees_sat,
                                    usdt_display,
                                }
                            })
                            .collect();
                        self.recent_transaction = txns;
                    } else {
                        self.recent_transaction = Vec::new();
                    }
                }
                view::LiquidOverviewMessage::Error(err) => {
                    self.error = Some(err.to_string());
                    return Task::done(Message::View(view::Message::ShowError(err.to_string())));
                }
                view::LiquidOverviewMessage::RefreshRequested => {
                    return self.load_balance();
                }
            }
        }
        if let Message::View(view::Message::Close) | Message::View(view::Message::Reload) = message
        {
            self.selected_payment = None;
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
        self.selected_payment = None;
        self.load_balance()
    }
}
