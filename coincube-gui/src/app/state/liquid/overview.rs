use std::convert::TryInto;
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::widget::{Column, Element};
use iced::Task;

use crate::app::breez_liquid::assets::usdt_asset_id;
use crate::app::cache::Cache;
use crate::app::menu::{LiquidSubMenu, Menu};
use crate::app::state::{redirect, State};
use crate::app::wallets::{DomainPayment, DomainPaymentDetails, LiquidBackend};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

/// LiquidOverview
pub struct LiquidOverview {
    breez_client: Arc<LiquidBackend>,
    btc_balance: Amount,
    usdt_balance: u64,
    recent_transaction: Vec<view::liquid::RecentTransaction>,
    recent_payments: Vec<DomainPayment>,
    selected_payment: Option<DomainPayment>,
    error: Option<String>,
}

impl LiquidOverview {
    pub fn new(breez_client: Arc<LiquidBackend>) -> Self {
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
                    &[],
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
                cache.show_direction_badges,
            )
            .map(view::Message::LiquidOverview);

            // Prepend a soft "not backed up" warning banner if the current
            // Cube's master seed hasn't been written down yet. The banner
            // lives at the state layer (rather than inside the view fn)
            // because liquid_overview_view is parameterised on
            // `LiquidOverviewMessage`, not the top-level `Message`.
            let content: Element<view::Message> =
                if !cache.current_cube_backed_up && !cache.current_cube_is_passkey {
                    Column::new()
                        .spacing(20)
                        .push(view::backup_warning_banner())
                        .push(send_view)
                        .into()
                } else {
                    send_view
                };

            view::dashboard(menu, cache, content)
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

                    let recent: Vec<DomainPayment> =
                        recent_payment.iter().take(5).cloned().collect();
                    self.recent_payments = recent.clone();

                    let usdt_id = usdt_asset_id(self.breez_client.network()).unwrap_or("");

                    if !recent.is_empty() {
                        let fiat_converter: Option<view::FiatAmountConverter> =
                            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                        let txns = recent
                            .iter()
                            .map(|payment| {
                                let status = payment.status;
                                let time_ago = format_time_ago(payment.timestamp.into());

                                // Detect USDt payments and build display string.
                                let usdt_amount_minor = match &payment.details {
                                    DomainPaymentDetails::LiquidAsset {
                                        asset_id,
                                        asset_info,
                                        ..
                                    } if !usdt_id.is_empty() && asset_id == usdt_id => {
                                        asset_info.as_ref().map(|i| i.amount_minor)
                                    }
                                    _ => None,
                                };
                                let is_usdt = usdt_amount_minor.is_some();

                                // For USDt, display the asset amount; otherwise display
                                // the BTC amount from `amount_sat`.
                                let amount = match usdt_amount_minor {
                                    Some(minor) => Amount::from_sat(minor),
                                    None => Amount::from_sat(payment.amount_sat),
                                };

                                // Only compute fiat for BTC rows; USDt has its own display.
                                let fiat_amount = if is_usdt {
                                    None
                                } else {
                                    fiat_converter
                                        .as_ref()
                                        .map(|c: &view::FiatAmountConverter| c.convert(amount))
                                };

                                let (desc, usdt_display) = if is_usdt {
                                    (
                                        "USDt Transfer".to_owned(),
                                        Some(format!(
                                            "{} USDt",
                                            crate::app::breez_liquid::assets::format_usdt_display(
                                                amount.to_sat()
                                            )
                                        )),
                                    )
                                } else {
                                    (payment.details.description().to_owned(), None)
                                };

                                let is_incoming = payment.is_incoming();
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
        // Load balance immediately for fast display, and trigger an SDK sync
        // in the background. When the sync completes the SDK fires
        // SdkEvent::Synced which will refresh the active panel automatically.
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
