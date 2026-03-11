use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::model::PaymentDetails;
use breez_sdk_liquid::prelude::Payment;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::widget::*;
use iced::Task;

use crate::app::breez::assets::{asset_kind_for_id, usdt_asset_id, AssetKind, USDT_PRECISION};
use crate::app::menu::{Menu, UsdtSubMenu};
use crate::app::state::{redirect, State};
use crate::app::view::liquid::RecentTransaction;
use crate::app::{breez::BreezClient, cache::Cache};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

pub struct UsdtOverview {
    breez_client: Arc<BreezClient>,
    usdt_balance: u64,
    recent_transaction: Vec<RecentTransaction>,
    recent_payments: Vec<Payment>,
    selected_payment: Option<Payment>,
    error: Option<String>,
}

impl UsdtOverview {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
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
                let payments = breez_client.list_payments(None).await;

                let usdt_balance = info
                    .as_ref()
                    .ok()
                    .and_then(|info| {
                        info.wallet_info.asset_balances.iter().find_map(|ab| {
                            if asset_kind_for_id(&ab.asset_id, breez_client.network())
                                == Some(AssetKind::Usdt)
                            {
                                Some(ab.balance_sat)
                            } else {
                                None
                            }
                        })
                    })
                    .unwrap_or(0);

                let error = match (&info, &payments) {
                    (Err(_), Err(_)) => Some("Couldn't fetch balance or transactions".to_string()),
                    (Err(_), _) => Some("Couldn't fetch USDt balance".to_string()),
                    (_, Err(_)) => Some("Couldn't fetch recent transactions".to_string()),
                    _ => None,
                };

                let payments = payments.unwrap_or_default();
                (usdt_balance, payments, error)
            },
            |(usdt_balance, recent_payment, error)| {
                if let Some(err) = error {
                    Message::View(view::Message::UsdtOverview(
                        view::UsdtOverviewMessage::Error(err),
                    ))
                } else {
                    Message::View(view::Message::UsdtOverview(
                        view::UsdtOverviewMessage::DataLoaded {
                            usdt_balance,
                            recent_payment,
                        },
                    ))
                }
            },
        )
    }
}

impl State for UsdtOverview {
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
            let overview_view = view::usdt::usdt_overview_view(
                self.usdt_balance,
                &self.recent_transaction,
                self.error.as_deref(),
                cache.bitcoin_unit,
            )
            .map(view::Message::UsdtOverview);

            view::dashboard(menu, cache, overview_view)
        }
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::UsdtOverview(ref msg)) = message {
            match msg {
                view::UsdtOverviewMessage::SendUsdt => {
                    return redirect(Menu::Usdt(UsdtSubMenu::Send));
                }
                view::UsdtOverviewMessage::ReceiveUsdt => {
                    return Task::batch(vec![
                        redirect(Menu::Usdt(UsdtSubMenu::Receive)),
                        Task::done(Message::View(view::Message::LiquidReceive(
                            view::LiquidReceiveMessage::ToggleMethod(view::ReceiveMethod::Usdt),
                        ))),
                    ]);
                }
                view::UsdtOverviewMessage::History => {
                    return redirect(Menu::Usdt(UsdtSubMenu::Transactions(None)));
                }
                view::UsdtOverviewMessage::SelectTransaction(idx) => {
                    if let Some(payment) = self.recent_payments.get(*idx).cloned() {
                        self.selected_payment = Some(payment.clone());
                        return Task::batch(vec![
                            redirect(Menu::Usdt(UsdtSubMenu::Transactions(None))),
                            Task::done(Message::View(view::Message::PreselectPayment(payment))),
                        ]);
                    }
                }
                view::UsdtOverviewMessage::DataLoaded {
                    usdt_balance,
                    recent_payment,
                } => {
                    self.error = None;
                    self.usdt_balance = *usdt_balance;

                    // Filter to only USDt payments
                    let usdt_payments: Vec<Payment> = recent_payment
                        .iter()
                        .filter(|p| {
                            matches!(
                                &p.details,
                                PaymentDetails::Liquid { asset_id, .. }
                                    if usdt_asset_id(self.breez_client.network())
                                        .map(|id| id == asset_id.as_str())
                                        .unwrap_or(false)
                            )
                        })
                        .take(5)
                        .cloned()
                        .collect();

                    self.recent_payments = usdt_payments.clone();

                    if !usdt_payments.is_empty() {
                        let _fiat_converter: Option<view::FiatAmountConverter> =
                            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                        let txns = usdt_payments
                            .iter()
                            .map(|payment| {
                                let status = payment.status;
                                let time_ago = format_time_ago(payment.timestamp.into());

                                let amount = if let PaymentDetails::Liquid {
                                    asset_info: Some(ref ai),
                                    ..
                                } = &payment.details
                                {
                                    Amount::from_sat(
                                        (ai.amount * 10_f64.powi(USDT_PRECISION as i32)).round()
                                            as u64,
                                    )
                                } else {
                                    Amount::from_sat(payment.amount_sat)
                                };

                                let is_incoming = matches!(
                                    payment.payment_type,
                                    breez_sdk_liquid::prelude::PaymentType::Receive
                                );
                                let details = payment.details.clone();
                                let fees_sat = Amount::from_sat(payment.fees_sat);
                                RecentTransaction {
                                    description: "USDt Transfer".to_owned(),
                                    time_ago,
                                    amount,
                                    fiat_amount: None,
                                    is_incoming,
                                    status,
                                    details,
                                    fees_sat,
                                }
                            })
                            .collect();
                        self.recent_transaction = txns;
                    } else {
                        self.recent_transaction = Vec::new();
                    }
                }
                view::UsdtOverviewMessage::Error(err) => {
                    self.error = Some(err.to_string());
                    return Task::done(Message::View(view::Message::ShowError(err.to_string())));
                }
                view::UsdtOverviewMessage::RefreshRequested => {
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
