use std::sync::Arc;

use coincube_ui::{
    component::{button, card, form, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{column, combo_box, container, row, Space},
    Alignment, Length, Subscription, Task,
};

use crate::app::{
    cache::Cache,
    menu::Menu,
    menu::P2PSubMenu,
    message::Message,
    view::{self, message::P2PMessage},
    wallet::Wallet,
    State,
};

use super::components::trade_card::{TradeRole, TradeStatus};
use super::components::{
    buy_sell_tabs, order_card, order_detail, payment_methods_for, trade_card, trade_status_filter,
    BuySellFilter, OrderType, P2POrder, P2PTrade, PricingMode, TradeFilter, FIAT_CURRENCIES,
};
use super::config::{load_mostro_config, save_mostro_config, MostroConfig, MostroNode};

pub struct P2PPanel {
    wallet: Option<Arc<Wallet>>,
    // Node currencies (fetched from info event, empty = use fallback)
    node_currencies: Vec<String>,
    // Order book state
    orders: Vec<P2POrder>,
    buy_sell_filter: BuySellFilter,
    selected_order: Option<String>,
    // Trades state
    trades: Vec<P2PTrade>,
    trade_filters: Vec<TradeFilter>,
    // Create Order form state
    create_order_type: OrderType,
    create_pricing_mode: PricingMode,
    create_fiat_currency: String,
    currency_combo_state: combo_box::State<String>,
    create_sats_amount: form::Value<String>,
    create_premium: form::Value<String>,
    create_payment_methods: Vec<String>,
    create_custom_payment_method: form::Value<String>,
    payment_method_combo_state: combo_box::State<String>,
    create_min_amount: form::Value<String>,
    create_max_amount: form::Value<String>,
    create_lightning_address: form::Value<String>,
    // Order submission state
    confirming_order: bool,
    order_submitting: bool,
    order_submit_error: Option<String>,
    // Take order state
    taking_order: bool,
    take_order_amount: form::Value<String>,
    take_order_invoice: form::Value<String>,
    take_order_submitting: bool,
    // Trade detail state
    selected_trade: Option<String>,
    trade_invoice_input: form::Value<String>,
    trade_action_loading: bool,
    // Hold invoice to display (seller must pay after taking a buy order)
    pending_payment_invoice: Option<(String, String, Option<i64>)>, // (order_id, invoice, amount_sats)
    // Mostro settings
    mostro_config: MostroConfig,
    new_relay_input: form::Value<String>,
    new_node_name_input: form::Value<String>,
    new_node_pubkey_input: form::Value<String>,
}

impl P2PPanel {
    pub fn new(wallet: Option<Arc<Wallet>>) -> Self {
        Self {
            wallet,
            node_currencies: Vec::new(),
            orders: Vec::new(),
            buy_sell_filter: BuySellFilter::Sell,
            selected_order: None,
            trades: Vec::new(),
            trade_filters: vec![TradeFilter::All],
            create_order_type: OrderType::Buy,
            create_pricing_mode: PricingMode::Market,
            create_fiat_currency: "USD".to_string(),
            currency_combo_state: combo_box::State::new(
                FIAT_CURRENCIES.iter().map(|s| s.to_string()).collect(),
            ),
            create_sats_amount: Default::default(),
            create_premium: Default::default(),
            create_payment_methods: Vec::new(),
            create_custom_payment_method: Default::default(),
            payment_method_combo_state: combo_box::State::new(
                payment_methods_for("USD")
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            ),
            create_min_amount: Default::default(),
            create_max_amount: Default::default(),
            create_lightning_address: Default::default(),
            confirming_order: false,
            order_submitting: false,
            order_submit_error: None,
            taking_order: false,
            take_order_amount: Default::default(),
            take_order_invoice: Default::default(),
            take_order_submitting: false,
            selected_trade: None,
            trade_invoice_input: Default::default(),
            trade_action_loading: false,
            pending_payment_invoice: None,
            mostro_config: load_mostro_config(),
            new_relay_input: Default::default(),
            new_node_name_input: Default::default(),
            new_node_pubkey_input: Default::default(),
        }
    }

    fn filtered_orders(&self) -> Vec<&P2POrder> {
        self.orders
            .iter()
            .filter(|order| match self.buy_sell_filter {
                // "BUY BTC" tab: show sell orders (counterparty is selling)
                BuySellFilter::Buy => order.order_type == OrderType::Sell,
                // "SELL BTC" tab: show buy orders (counterparty is buying)
                BuySellFilter::Sell => order.order_type == OrderType::Buy,
            })
            .collect()
    }

    fn filtered_trades(&self) -> Vec<&P2PTrade> {
        if self.trade_filters.contains(&TradeFilter::All) {
            return self.trades.iter().collect();
        }
        self.trades
            .iter()
            .filter(|trade| {
                self.trade_filters.iter().any(|f| match f {
                    TradeFilter::All => true,
                    TradeFilter::Pending => trade.status == TradeStatus::Pending,
                    TradeFilter::Active => trade.status == TradeStatus::Active,
                    TradeFilter::WaitingPayment => trade.status == TradeStatus::WaitingPayment,
                    TradeFilter::WaitingInvoice => trade.status == TradeStatus::WaitingBuyerInvoice,
                    TradeFilter::FiatSent => trade.status == TradeStatus::FiatSent,
                    TradeFilter::Success => {
                        matches!(trade.status, TradeStatus::Success | TradeStatus::Expired)
                    }
                    TradeFilter::Canceled => matches!(
                        trade.status,
                        TradeStatus::Canceled | TradeStatus::CooperativelyCanceled
                    ),
                    TradeFilter::PayingSats => {
                        trade.last_dm_action.as_deref() == Some("HoldInvoicePaymentSettled")
                    }
                    TradeFilter::Dispute => trade.status == TradeStatus::Dispute,
                })
            })
            .collect()
    }

    fn clear_create_form(&mut self) {
        self.create_order_type = OrderType::Buy;
        self.create_pricing_mode = PricingMode::Market;
        self.create_fiat_currency = "USD".to_string();
        self.create_sats_amount = Default::default();
        self.create_premium = Default::default();
        self.create_payment_methods = Vec::new();
        self.create_custom_payment_method = Default::default();
        self.rebuild_payment_method_combo();
        self.create_min_amount = Default::default();
        self.create_max_amount = Default::default();
        self.create_lightning_address = Default::default();
    }

    fn rebuild_currency_combo(&mut self) {
        let options: Vec<String> = if self.node_currencies.is_empty() {
            FIAT_CURRENCIES.iter().map(|s| s.to_string()).collect()
        } else {
            self.node_currencies.clone()
        };
        self.currency_combo_state = combo_box::State::new(options);
    }

    fn rebuild_payment_method_combo(&mut self) {
        let options: Vec<String> = payment_methods_for(&self.create_fiat_currency)
            .iter()
            .filter(|&&m| !self.create_payment_methods.contains(&m.to_string()))
            .map(|s| s.to_string())
            .collect();
        self.payment_method_combo_state = combo_box::State::new(options);
    }

    fn build_order_form(&self) -> super::mostro::OrderFormData {
        let mut payment_methods = self.create_payment_methods.clone();
        let custom = self.create_custom_payment_method.value.trim().to_string();
        if !custom.is_empty() && !payment_methods.contains(&custom) {
            payment_methods.push(custom);
        }

        let is_range = self.is_range_order();

        super::mostro::OrderFormData {
            kind: match self.create_order_type {
                OrderType::Buy => mostro_core::order::Kind::Buy,
                OrderType::Sell => mostro_core::order::Kind::Sell,
            },
            fiat_code: self.create_fiat_currency.clone(),
            fiat_amount: if is_range {
                0
            } else {
                self.create_min_amount.value.parse().unwrap_or(0)
            },
            min_amount: if is_range {
                self.create_min_amount.value.parse().ok()
            } else {
                None
            },
            max_amount: if is_range {
                self.create_max_amount.value.parse().ok()
            } else {
                None
            },
            amount: if self.create_pricing_mode == PricingMode::Fixed {
                self.create_sats_amount.value.parse().unwrap_or(0)
            } else {
                0
            },
            premium: if self.create_pricing_mode == PricingMode::Market {
                self.create_premium.value.parse().unwrap_or(0)
            } else {
                0
            },
            payment_method: payment_methods.join(","),
            cube_name: self
                .wallet
                .as_ref()
                .map(|w| w.name.clone())
                .unwrap_or_else(|| "default".to_string()),
            buyer_invoice: if self.create_order_type == OrderType::Buy {
                let addr = self.create_lightning_address.value.trim().to_string();
                if addr.is_empty() {
                    None
                } else {
                    Some(addr)
                }
            } else {
                None
            },
            expiry_days: 1,
            mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
            relay_urls: self.mostro_config.relays.clone(),
        }
    }

    fn is_range_order(&self) -> bool {
        !self.create_min_amount.value.is_empty() && !self.create_max_amount.value.is_empty()
    }

    fn order_confirmation_view(&self) -> Element<'_, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);
        let is_range = self.is_range_order();

        let kind_label = match self.create_order_type {
            OrderType::Buy => "Buy",
            OrderType::Sell => "Sell",
        };

        let amount_label = if self.create_pricing_mode == PricingMode::Fixed
            && !self.create_sats_amount.value.is_empty()
        {
            format!("{} sats", self.create_sats_amount.value)
        } else {
            "Market price".to_string()
        };

        let fiat_label = if is_range {
            format!(
                "{}-{} {}",
                self.create_min_amount.value,
                self.create_max_amount.value,
                self.create_fiat_currency
            )
        } else {
            format!(
                "{} {}",
                self.create_min_amount.value, self.create_fiat_currency
            )
        };

        let mut payment_methods = self.create_payment_methods.clone();
        let custom = self.create_custom_payment_method.value.trim().to_string();
        if !custom.is_empty() && !payment_methods.contains(&custom) {
            payment_methods.push(custom);
        }
        let payment_label = payment_methods.join(", ");

        let premium_label = if self.create_pricing_mode == PricingMode::Market {
            format!("{}%", self.create_premium.value.parse::<i64>().unwrap_or(0))
        } else {
            "N/A".to_string()
        };

        let detail_row = |label: &str, value: &str| -> Element<'_, view::Message> {
            row![
                p2_regular(label).width(Length::FillPortion(2)),
                p1_bold(value).width(Length::FillPortion(3)),
            ]
            .spacing(8)
            .into()
        };

        let mut details = column![
            detail_row("Order Type", kind_label),
            detail_row("Currency", &self.create_fiat_currency),
            detail_row("Amount", &amount_label),
            detail_row("Fiat Amount", &fiat_label),
            detail_row("Payment", &payment_label),
            detail_row("Premium", &premium_label),
        ]
        .spacing(8);

        if self.create_order_type == OrderType::Buy
            && !self.create_lightning_address.value.trim().is_empty()
        {
            details = details.push(detail_row(
                "Invoice",
                self.create_lightning_address.value.trim(),
            ));
        }

        details = details.push(detail_row("Expiry", "24 hours"));

        card::simple(
            column![
                p1_bold("Please review your order:"),
                details,
                row![
                    button::secondary(None, "Cancel")
                        .on_press(p2p(P2PMessage::CancelConfirmation))
                        .width(Length::Fill),
                    button::primary(None, "Confirm")
                        .on_press(p2p(P2PMessage::ConfirmOrder))
                        .width(Length::Fill),
                ]
                .spacing(8),
            ]
            .spacing(16),
        )
        .width(Length::Fixed(500.0))
        .into()
    }

    fn create_order_view<'a>(&'a self) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);
        let is_range = self.is_range_order();
        // Fixed price not available for range orders
        let effective_pricing_mode = if is_range {
            &PricingMode::Market
        } else {
            &self.create_pricing_mode
        };

        // Order type toggle
        let buy_btn = if self.create_order_type == OrderType::Buy {
            button::primary(None, "Buy")
        } else {
            button::secondary(None, "Buy")
        }
        .on_press(p2p(P2PMessage::OrderTypeSelected(OrderType::Buy)))
        .width(Length::Fill);

        let sell_btn = if self.create_order_type == OrderType::Sell {
            button::primary(None, "Sell")
        } else {
            button::secondary(None, "Sell")
        }
        .on_press(p2p(P2PMessage::OrderTypeSelected(OrderType::Sell)))
        .width(Length::Fill);

        // Banner text based on order type
        let banner_text = match self.create_order_type {
            OrderType::Buy => "You want to buy Bitcoin",
            OrderType::Sell => "You want to sell Bitcoin",
        };

        let banner = container(
            p1_bold(banner_text)
                .width(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center),
        )
        .padding([12, 20])
        .width(Length::Fill)
        .style(theme::container::foreground);

        // Order type card
        let order_type_card = container(row![buy_btn, sell_btn].spacing(8).width(Length::Fill))
            .padding(4)
            .width(Length::Fill)
            .style(theme::container::foreground_rounded);

        // Currency card
        let currency_combo = combo_box(
            &self.currency_combo_state,
            "Search currency...",
            Some(&self.create_fiat_currency),
            |selected: String| view::Message::P2P(P2PMessage::FiatCurrencyEdited(selected)),
        )
        .padding(10)
        .width(Length::Fill);

        let currency_label = match self.create_order_type {
            OrderType::Buy => "Select the fiat currency you will pay with",
            OrderType::Sell => "Select the fiat currency you want to receive",
        };

        let currency_card = card::simple(
            column![
                p2_regular(currency_label).style(theme::text::secondary),
                row![
                    icon::dollar_icon().style(theme::text::success),
                    currency_combo,
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
            ]
            .spacing(12),
        )
        .width(Length::Fill);

        // Amount card (min required, max optional — fills max to make a range order)
        let amount_label = match self.create_order_type {
            OrderType::Buy => "Enter amount you want to send",
            OrderType::Sell => "Enter amount you want to receive",
        };

        let amount_card = card::simple(
            column![
                p2_regular(amount_label).style(theme::text::secondary),
                row![
                    icon::coins_icon().style(theme::text::success),
                    form::Form::new_amount_numeric("Amount", &self.create_min_amount, |v| {
                        view::Message::P2P(P2PMessage::MinAmountEdited(v))
                    })
                    .padding(10),
                    form::Form::new_amount_numeric(
                        "Max (optional)",
                        &self.create_max_amount,
                        |v| { view::Message::P2P(P2PMessage::MaxAmountEdited(v)) }
                    )
                    .padding(10),
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center),
            ]
            .spacing(12),
        )
        .width(Length::Fill);

        // Payment method card
        let pm_combo = combo_box(
            &self.payment_method_combo_state,
            "Select payment methods",
            None::<&String>,
            |selected: String| view::Message::P2P(P2PMessage::PaymentMethodSelected(selected)),
        )
        .padding(10)
        .width(Length::Fill);

        let pm_tags: Element<'a, view::Message> = if self.create_payment_methods.is_empty() {
            column![].into()
        } else {
            row(self.create_payment_methods.iter().map(|method| {
                let method_clone = method.clone();
                container(
                    row![
                        p2_regular(method.as_str()),
                        button::secondary_compact(None, "x").on_press(view::Message::P2P(
                            P2PMessage::PaymentMethodRemoved(method_clone,)
                        )),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center),
                )
                .padding([4, 8])
                .style(theme::pill::simple)
                .into()
            }))
            .spacing(6)
            .wrap()
            .into()
        };

        let payment_card = card::simple(
            column![
                p2_regular("Payment methods for").style(theme::text::secondary),
                row![icon::card_icon().style(theme::text::success), pm_combo,]
                    .spacing(12)
                    .align_y(iced::alignment::Vertical::Center),
                pm_tags,
                form::Form::new_trimmed(
                    "Enter custom payment method",
                    &self.create_custom_payment_method,
                    |v| { view::Message::P2P(P2PMessage::CustomPaymentMethodEdited(v)) }
                )
                .padding(10),
                if self.create_custom_payment_method.value.is_empty() {
                    button::secondary(None, "Add Custom")
                } else {
                    button::secondary(None, "Add Custom")
                        .on_press(view::Message::P2P(P2PMessage::AddCustomPaymentMethod))
                },
            ]
            .spacing(12),
        )
        .width(Length::Fill);

        // Price type card
        let market_btn = if *effective_pricing_mode == PricingMode::Market {
            button::primary(None, "Market Rate")
        } else {
            button::secondary(None, "Market Rate")
        }
        .on_press(p2p(P2PMessage::PricingModeSelected(PricingMode::Market)))
        .width(Length::Fill);

        let fixed_btn = if *effective_pricing_mode == PricingMode::Fixed {
            button::primary(None, "Fixed Price")
        } else {
            button::secondary(None, "Fixed Price")
        }
        .width(Length::Fill);
        let fixed_btn = if is_range {
            fixed_btn
        } else {
            fixed_btn.on_press(p2p(P2PMessage::PricingModeSelected(PricingMode::Fixed)))
        };

        let price_type_card = card::simple(
            column![
                p2_regular("Price type").style(theme::text::secondary),
                row![
                    icon::dollar_icon().style(theme::text::success),
                    row![market_btn, fixed_btn].spacing(8).width(Length::Fill),
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
            ]
            .spacing(12),
        )
        .width(Length::Fill);

        // Pricing-mode-dependent field card
        let pricing_card: Element<'a, view::Message> =
            if *effective_pricing_mode == PricingMode::Fixed {
                card::simple(
                    column![
                        p2_regular("Sats Amount").style(theme::text::secondary),
                        row![
                            icon::bitcoin_icon().style(theme::text::success),
                            form::Form::new_amount_sats(
                                "Enter sats amount",
                                &self.create_sats_amount,
                                |v| { view::Message::P2P(P2PMessage::SatsAmountEdited(v)) }
                            )
                            .padding(10),
                        ]
                        .spacing(12)
                        .align_y(iced::alignment::Vertical::Center),
                    ]
                    .spacing(12),
                )
                .width(Length::Fill)
                .into()
            } else {
                card::simple(
                    column![
                        p2_regular("Premium (%)").style(theme::text::secondary),
                        row![
                            icon::coins_icon().style(theme::text::success),
                            form::Form::new_amount_numeric(
                                "Enter premium percentage",
                                &self.create_premium,
                                |v| { view::Message::P2P(P2PMessage::PremiumEdited(v)) }
                            )
                            .padding(10),
                        ]
                        .spacing(12)
                        .align_y(iced::alignment::Vertical::Center),
                    ]
                    .spacing(12),
                )
                .width(Length::Fill)
                .into()
            };

        // Lightning address card (Buy orders only)
        let lightning_address_card: Element<'a, view::Message> =
            if self.create_order_type == OrderType::Buy {
                card::simple(
                    column![
                        p2_regular("Lightning Address (optional)").style(theme::text::secondary),
                        row![
                            icon::lightning_icon().style(theme::text::success),
                            form::Form::new_trimmed(
                                "Enter lightning address",
                                &self.create_lightning_address,
                                |v| { view::Message::P2P(P2PMessage::LightningAddressEdited(v)) }
                            )
                            .padding(10),
                        ]
                        .spacing(12)
                        .align_y(iced::alignment::Vertical::Center),
                    ]
                    .spacing(12),
                )
                .width(Length::Fill)
                .into()
            } else {
                column![].into()
            };

        // Expiry days
        // Error message
        let error_msg: Element<'a, view::Message> = if let Some(ref err) = self.order_submit_error {
            p2_regular(err.as_str()).style(theme::text::warning).into()
        } else {
            column![].into()
        };

        // Submit: amount (min_amount) is always required
        let has_amount = !self.create_min_amount.value.is_empty();
        let has_payment_method = !self.create_payment_methods.is_empty()
            || !self.create_custom_payment_method.value.is_empty();
        let can_submit = has_amount && has_payment_method && !self.order_submitting;

        let submit_btn = if self.order_submitting {
            button::primary(None, "Submit").width(Length::Fill)
        } else if can_submit {
            button::primary(None, "Submit")
                .on_press(p2p(P2PMessage::SubmitOrder))
                .width(Length::Fill)
        } else {
            button::primary(None, "Submit").width(Length::Fill)
        };

        let form_dirty = self.create_fiat_currency != "USD"
            || !self.create_min_amount.value.is_empty()
            || !self.create_max_amount.value.is_empty()
            || !self.create_sats_amount.value.is_empty()
            || !self.create_premium.value.is_empty()
            || !self.create_payment_methods.is_empty()
            || !self.create_custom_payment_method.value.is_empty()
            || !self.create_lightning_address.value.is_empty();

        let buttons: Element<'a, view::Message> = if form_dirty && !self.order_submitting {
            row![
                button::secondary(None, "Clear")
                    .on_press(p2p(P2PMessage::ClearForm))
                    .width(Length::Fill),
                submit_btn,
            ]
            .spacing(8)
            .into()
        } else {
            row![submit_btn].into()
        };

        column![
            banner,
            order_type_card,
            currency_card,
            amount_card,
            payment_card,
            price_type_card,
            pricing_card,
            lightning_address_card,
            error_msg,
            buttons,
        ]
        .spacing(16)
        .width(Length::Fill)
        .into()
    }
    fn trade_detail_view<'a>(&'a self, trade: &'a P2PTrade) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);

        // Own pending order — show order-book-style view with Cancel
        if trade.role == TradeRole::Creator && trade.status == TradeStatus::Pending {
            let badge_style = match trade.order_type {
                OrderType::Buy => theme::pill::success as fn(&_) -> _,
                OrderType::Sell => theme::pill::warning as fn(&_) -> _,
            };
            let heading = match trade.order_type {
                OrderType::Buy => "You are buying",
                OrderType::Sell => "You are selling",
            };

            let info_card = card::simple(
                column![
                    container(p2_regular(match trade.order_type {
                        OrderType::Buy => "BUYING",
                        OrderType::Sell => "SELLING",
                    }))
                    .padding([4, 12])
                    .style(badge_style),
                    p2_regular(heading).style(theme::text::secondary),
                    row!(
                        h2(format!("{:.2}", trade.fiat_amount)),
                        p1_bold(format!(" {}", trade.fiat_currency)).style(theme::text::secondary)
                    )
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                    if trade.is_fixed_price() {
                        row![
                            p2_regular("for").style(theme::text::secondary),
                            p2_bold(format!("{} sats", trade.sats_amount.unwrap_or(0)))
                        ]
                        .spacing(8)
                    } else {
                        row![
                            p2_regular("Market Price").style(theme::text::secondary),
                            p2_bold(trade.premium_text())
                        ]
                        .spacing(8)
                    },
                    if !trade.payment_method.is_empty() {
                        container(p2_regular(&trade.payment_method))
                            .padding(12)
                            .width(Length::Fill)
                            .style(theme::container::background)
                    } else {
                        container(column![]).width(Length::Fill)
                    },
                ]
                .spacing(8),
            )
            .width(Length::Fill);

            let id_card = card::simple(
                row![
                    icon::clipboard_icon().style(theme::text::secondary),
                    column![
                        p2_regular("Order ID").style(theme::text::secondary),
                        row![
                            p2_regular(&trade.id).style(theme::text::secondary),
                            Space::new().width(Length::Fill),
                            button::secondary_compact(None, "Copy")
                                .on_press(view::Message::Clipboard(trade.id.clone())),
                        ]
                        .spacing(8)
                        .align_y(iced::alignment::Vertical::Center),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
            )
            .width(Length::Fill);

            let note = column![
                p2_regular("This order will be published for 24 hours.")
                    .style(theme::text::secondary),
                p2_regular("You can cancel it anytime before someone takes it.")
                    .style(theme::text::secondary),
            ]
            .spacing(4);

            let close_btn = button::secondary(None, "Close")
                .on_press(p2p(P2PMessage::CloseTradeDetail))
                .width(Length::Fill);
            let cancel_btn = button::alert(None, "Cancel Order")
                .on_press(p2p(P2PMessage::CancelOrder(trade.id.clone())))
                .width(Length::Fill);

            return column![
                info_card,
                id_card,
                note,
                row![close_btn, cancel_btn].spacing(8),
            ]
            .spacing(12)
            .width(Length::Fill)
            .into();
        }

        let type_badge_style = match trade.order_type {
            OrderType::Buy => theme::pill::success as fn(&_) -> _,
            OrderType::Sell => theme::pill::warning as fn(&_) -> _,
        };

        let status_badge_style = match trade.status {
            TradeStatus::Active | TradeStatus::FiatSent => theme::pill::success as fn(&_) -> _,
            TradeStatus::Success => theme::pill::primary as fn(&_) -> _,
            TradeStatus::Pending
            | TradeStatus::WaitingPayment
            | TradeStatus::WaitingBuyerInvoice => theme::pill::simple as fn(&_) -> _,
            TradeStatus::Canceled
            | TradeStatus::CooperativelyCanceled
            | TradeStatus::Dispute
            | TradeStatus::Expired => theme::pill::warning as fn(&_) -> _,
        };

        // Is user the buyer? order_type always stores OUR perspective (what we're doing).
        let is_buyer = trade.order_type == OrderType::Buy;

        let detail_row = |label: &str, value: &str| -> Element<'_, view::Message> {
            row![
                p2_regular(label).width(Length::FillPortion(2)),
                p1_bold(value).width(Length::FillPortion(3)),
            ]
            .spacing(8)
            .into()
        };

        // Trade info card
        let info_card = card::simple(
            column![
                // Header badges
                row![
                    container(p2_regular(trade.order_type_label()))
                        .padding([4, 12])
                        .style(type_badge_style),
                    container(p2_regular(trade.status.label()))
                        .padding([4, 12])
                        .style(status_badge_style),
                    container(p2_regular(trade.role.label()))
                        .padding([4, 12])
                        .style(theme::pill::simple as fn(&_) -> _),
                ]
                .spacing(8),
                detail_row(
                    "Amount",
                    &format!("{:.2} {}", trade.fiat_amount, trade.fiat_currency)
                ),
                if trade.is_fixed_price() {
                    detail_row("Sats", &format!("{}", trade.sats_amount.unwrap_or(0)))
                } else {
                    detail_row("Price", &format!("Market {}", trade.premium_text()))
                },
                if !trade.payment_method.is_empty() {
                    detail_row("Payment", &trade.payment_method)
                } else {
                    column![].into()
                },
                detail_row("Created", &trade.time_ago),
            ]
            .spacing(8),
        )
        .width(Length::Fill);

        // Order ID card
        let id_card = card::simple(
            row![
                icon::clipboard_icon().style(theme::text::secondary),
                column![
                    p2_regular("Order ID").style(theme::text::secondary),
                    row![
                        p2_regular(&trade.id).style(theme::text::secondary),
                        Space::new().width(Length::Fill),
                        button::secondary_compact(None, "Copy")
                            .on_press(view::Message::Clipboard(trade.id.clone())),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(4)
                .width(Length::Fill),
            ]
            .spacing(12)
            .align_y(iced::alignment::Vertical::Center),
        )
        .width(Length::Fill);

        // Action buttons driven by last_dm_action (DM-driven), falling back to status (event-driven)
        let mut actions = column![].spacing(8);
        let loading = self.trade_action_loading;
        let dm_action = trade.last_dm_action.as_deref();

        // Terminal states — no action buttons at all
        let is_terminal = matches!(
            dm_action,
            Some(
                "HoldInvoicePaymentSettled"
                    | "PurchaseCompleted"
                    | "Rate"
                    | "CooperativeCancelAccepted"
                    | "AdminSettled"
                    | "AdminCanceled"
            )
        ) || matches!(
            trade.status,
            TradeStatus::Success | TradeStatus::Canceled | TradeStatus::Expired
        );

        // Cooperative cancel tracking
        let cancel_initiated_by_you = matches!(dm_action, Some("CooperativeCancelInitiatedByYou"));
        let cancel_initiated_by_peer =
            matches!(dm_action, Some("CooperativeCancelInitiatedByPeer"));
        let in_cooperative_cancel = cancel_initiated_by_you || cancel_initiated_by_peer;

        // Dispute tracking
        let dispute_initiated = matches!(
            dm_action,
            Some("DisputeInitiatedByYou" | "DisputeInitiatedByPeer" | "AdminTookDispute")
        ) || matches!(trade.status, TradeStatus::Dispute);

        if !is_terminal {
            // --- Status message for cooperative cancel ---
            if cancel_initiated_by_you {
                actions = actions.push(
                    card::simple(
                        column![
                            p2_regular("You initiated a cooperative cancel")
                                .style(theme::text::warning),
                            p2_regular("Waiting for counterparty to accept...")
                                .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            } else if cancel_initiated_by_peer {
                actions = actions.push(
                    card::simple(
                        column![
                            p2_regular("Counterparty requested a cooperative cancel")
                                .style(theme::text::warning),
                            p2_regular("Accept the cancel or open a dispute")
                                .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            }

            // --- Status message for dispute ---
            if matches!(dm_action, Some("DisputeInitiatedByYou")) {
                actions = actions.push(
                    card::simple(
                        p2_regular("You opened a dispute. Waiting for admin...")
                            .style(theme::text::warning),
                    )
                    .width(Length::Fill),
                );
            } else if matches!(dm_action, Some("DisputeInitiatedByPeer")) {
                actions = actions.push(
                    card::simple(
                        p2_regular("Counterparty opened a dispute. Waiting for admin...")
                            .style(theme::text::warning),
                    )
                    .width(Length::Fill),
                );
            } else if matches!(dm_action, Some("AdminTookDispute")) {
                actions = actions.push(
                    card::simple(
                        p2_regular("Admin is reviewing the dispute").style(theme::text::warning),
                    )
                    .width(Length::Fill),
                );
            }

            // --- Primary trade action buttons (only if not in cooperative cancel or dispute) ---
            if !in_cooperative_cancel && !dispute_initiated {
                match dm_action {
                    Some("AddInvoice") | Some("WaitingBuyerInvoice") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p2_regular("Submit Lightning Invoice")
                                            .style(theme::text::secondary),
                                        form::Form::new_trimmed(
                                            "Enter lightning invoice or address",
                                            &self.trade_invoice_input,
                                            |v| view::Message::P2P(P2PMessage::TradeInvoiceEdited(
                                                v
                                            )),
                                        )
                                        .padding(10),
                                        if !self.trade_invoice_input.value.is_empty() && !loading {
                                            button::primary(None, "Submit Invoice")
                                                .on_press(p2p(P2PMessage::SubmitInvoice))
                                                .width(Length::Fill)
                                        } else {
                                            button::primary(None, "Submit Invoice")
                                                .width(Length::Fill)
                                        },
                                    ]
                                    .spacing(8),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }
                    Some("HoldInvoicePaymentAccepted") | Some("WaitingPayment") => {
                        if is_buyer {
                            actions = actions.push(if !loading {
                                button::primary(None, "Confirm Fiat Sent")
                                    .on_press(p2p(P2PMessage::ConfirmFiatSent))
                                    .width(Length::Fill)
                            } else {
                                button::primary(None, "Confirm Fiat Sent").width(Length::Fill)
                            });
                        }
                    }
                    Some("FiatSentOk") | Some("FiatSent") => {
                        if !is_buyer {
                            actions = actions.push(if !loading {
                                button::primary(None, "Release (Confirm Fiat Received)")
                                    .on_press(p2p(P2PMessage::ConfirmFiatReceived))
                                    .width(Length::Fill)
                            } else {
                                button::primary(None, "Release (Confirm Fiat Received)")
                                    .width(Length::Fill)
                            });
                        }
                    }
                    Some("PayInvoice") => {
                        actions = actions.push(
                            p2_regular("Waiting for hold invoice payment...")
                                .style(theme::text::secondary),
                        );
                    }
                    _ => {
                        // No DM action or unrecognized — fall back to status-based buttons
                        match trade.status {
                            TradeStatus::WaitingBuyerInvoice if is_buyer => {
                                actions = actions.push(
                                    card::simple(
                                        column![
                                            p2_regular("Submit Lightning Invoice")
                                                .style(theme::text::secondary),
                                            form::Form::new_trimmed(
                                                "Enter lightning invoice or address",
                                                &self.trade_invoice_input,
                                                |v| view::Message::P2P(
                                                    P2PMessage::TradeInvoiceEdited(v)
                                                ),
                                            )
                                            .padding(10),
                                            if !self.trade_invoice_input.value.is_empty()
                                                && !loading
                                            {
                                                button::primary(None, "Submit Invoice")
                                                    .on_press(p2p(P2PMessage::SubmitInvoice))
                                                    .width(Length::Fill)
                                            } else {
                                                button::primary(None, "Submit Invoice")
                                                    .width(Length::Fill)
                                            },
                                        ]
                                        .spacing(8),
                                    )
                                    .width(Length::Fill),
                                );
                            }
                            TradeStatus::WaitingPayment if is_buyer => {
                                actions = actions.push(if !loading {
                                    button::primary(None, "Confirm Fiat Sent")
                                        .on_press(p2p(P2PMessage::ConfirmFiatSent))
                                        .width(Length::Fill)
                                } else {
                                    button::primary(None, "Confirm Fiat Sent").width(Length::Fill)
                                });
                            }
                            TradeStatus::FiatSent if !is_buyer => {
                                actions = actions.push(if !loading {
                                    button::primary(None, "Release (Confirm Fiat Received)")
                                        .on_press(p2p(P2PMessage::ConfirmFiatReceived))
                                        .width(Length::Fill)
                                } else {
                                    button::primary(None, "Release (Confirm Fiat Received)")
                                        .width(Length::Fill)
                                });
                            }
                            _ => {}
                        }
                    }
                }
            }

            // --- Cancel / Dispute footer buttons ---
            if !loading {
                if cancel_initiated_by_peer {
                    // Peer requested cancel — show "Accept Cancel" + Dispute
                    actions = actions.push(
                        row![
                            button::secondary(None, "Accept Cancel")
                                .on_press(p2p(P2PMessage::CancelTrade))
                                .width(Length::Fill),
                            button::secondary(None, "Dispute")
                                .on_press(p2p(P2PMessage::OpenDispute))
                                .width(Length::Fill),
                        ]
                        .spacing(8),
                    );
                } else if cancel_initiated_by_you {
                    // You initiated cancel — only Dispute available
                    actions = actions.push(
                        button::secondary(None, "Dispute")
                            .on_press(p2p(P2PMessage::OpenDispute))
                            .width(Length::Fill),
                    );
                } else if !dispute_initiated {
                    // Normal active trade — Cancel + Dispute
                    actions = actions.push(
                        row![
                            button::secondary(None, "Cancel")
                                .on_press(p2p(P2PMessage::CancelTrade))
                                .width(Length::Fill),
                            button::secondary(None, "Dispute")
                                .on_press(p2p(P2PMessage::OpenDispute))
                                .width(Length::Fill),
                        ]
                        .spacing(8),
                    );
                }
                // If dispute_initiated: no cancel/dispute buttons (already in dispute)
            }
        }

        // Close button
        let close_btn = button::secondary(None, "Close")
            .on_press(p2p(P2PMessage::CloseTradeDetail))
            .width(Length::Fill);

        column![info_card, id_card, actions, close_btn]
            .spacing(12)
            .width(Length::Fill)
            .into()
    }

    fn payment_invoice_modal_view<'a>(
        &'a self,
        order_id: &str,
        invoice: &str,
        amount_sats: Option<i64>,
    ) -> Element<'a, view::Message> {
        let amount_text = match amount_sats {
            Some(amt) => format!("{} sats", amt),
            None => "amount TBD".to_string(),
        };

        let truncated_id = if order_id.len() > 8 {
            &order_id[..8]
        } else {
            order_id
        };

        card::simple(
            column![
                p1_bold("Payment Required"),
                p2_regular(format!(
                    "Order {} taken. Pay this hold invoice to lock {} for the trade.",
                    truncated_id, amount_text
                ))
                .style(theme::text::secondary),
                container(
                    p2_regular(invoice)
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::Glyph),
                )
                .padding(12)
                .width(Length::Fill)
                .style(theme::container::background),
                row![
                    button::secondary(None, "Close")
                        .on_press(view::Message::P2P(P2PMessage::DismissPaymentInvoice))
                        .width(Length::Fill),
                    button::primary(None, "Copy Invoice")
                        .on_press(view::Message::P2P(P2PMessage::CopyPaymentInvoice(
                            invoice.to_string(),
                        )))
                        .width(Length::Fill),
                ]
                .spacing(8),
            ]
            .spacing(12),
        )
        .width(Length::Fixed(500.0))
        .into()
    }

    fn take_order_modal_view<'a>(&'a self, order: &'a P2POrder) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);

        // Is user buying? (taking a sell order)
        let is_buying = order.order_type == OrderType::Sell;

        let mut modal_content = column![p1_bold("Take Order"),].spacing(12);

        // For range orders, show amount input
        if order.is_range_order() {
            modal_content = modal_content.push(
                column![
                    p2_regular(format!(
                        "Enter fiat amount ({:.0} - {:.0} {})",
                        order.min_amount.unwrap_or(0.0),
                        order.max_amount.unwrap_or(0.0),
                        order.fiat_currency
                    ))
                    .style(theme::text::secondary),
                    form::Form::new_amount_numeric("Fiat amount", &self.take_order_amount, |v| {
                        view::Message::P2P(P2PMessage::TakeOrderAmountEdited(v))
                    },)
                    .padding(10),
                ]
                .spacing(8),
            );
        }

        // If buying, show invoice input
        if is_buying {
            modal_content = modal_content.push(
                column![
                    p2_regular("Lightning invoice or address (optional)")
                        .style(theme::text::secondary),
                    form::Form::new_trimmed(
                        "Enter lightning invoice or address",
                        &self.take_order_invoice,
                        |v| view::Message::P2P(P2PMessage::TakeOrderInvoiceEdited(v)),
                    )
                    .padding(10),
                ]
                .spacing(8),
            );
        }

        let can_confirm = if order.is_range_order() {
            !self.take_order_amount.value.is_empty()
        } else {
            true
        };

        let confirm_btn = if can_confirm && !self.take_order_submitting {
            button::primary(None, "Confirm")
                .on_press(p2p(P2PMessage::ConfirmTakeOrder))
                .width(Length::Fill)
        } else {
            button::primary(None, "Confirm").width(Length::Fill)
        };

        modal_content = modal_content.push(
            row![
                button::secondary(None, "Cancel")
                    .on_press(p2p(P2PMessage::CancelTakeOrder))
                    .width(Length::Fill),
                confirm_btn,
            ]
            .spacing(8),
        );

        card::simple(modal_content)
            .width(Length::Fixed(500.0))
            .into()
    }

    fn mostro_settings_view<'a>(&'a self) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);

        // ── Nodes card ──
        let node_rows: Vec<Element<'a, view::Message>> = self
            .mostro_config
            .nodes
            .iter()
            .map(|node| {
                let is_active = node.pubkey_hex == self.mostro_config.active_node_pubkey;
                let truncated_pubkey = if node.pubkey_hex.len() > 16 {
                    format!(
                        "{}...{}",
                        &node.pubkey_hex[..8],
                        &node.pubkey_hex[node.pubkey_hex.len() - 8..]
                    )
                } else {
                    node.pubkey_hex.clone()
                };

                let select_btn = if is_active {
                    button::primary(None, "Active").width(Length::Shrink)
                } else {
                    button::secondary(None, "Select")
                        .on_press(p2p(P2PMessage::MostroSelectActiveNode(
                            node.pubkey_hex.clone(),
                        )))
                        .width(Length::Shrink)
                };

                let remove_btn = button::secondary_compact(Some(icon::trash_icon()), "")
                    .on_press(p2p(P2PMessage::MostroRemoveNode(node.pubkey_hex.clone())));

                row![
                    column![
                        p2_bold(node.name.as_str()),
                        p2_regular(truncated_pubkey.as_str()).style(theme::text::secondary),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                    select_btn,
                    remove_btn,
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .into()
            })
            .collect();

        let nodes_card = card::simple(
            column![
                p1_bold("Mostro Nodes"),
                p2_regular("Select which Mostro node to connect to").style(theme::text::secondary),
            ]
            .spacing(4)
            .push(column(node_rows).spacing(12))
            .push(
                column![
                    p2_regular("Add a new node").style(theme::text::secondary),
                    form::Form::new("Node name", &self.new_node_name_input, |v| {
                        view::Message::P2P(P2PMessage::MostroNodeNameInputEdited(v))
                    })
                    .padding(10),
                    form::Form::new_trimmed(
                        "Node pubkey (hex)",
                        &self.new_node_pubkey_input,
                        |v| { view::Message::P2P(P2PMessage::MostroNodePubkeyInputEdited(v)) }
                    )
                    .padding(10),
                    if self.new_node_name_input.value.trim().is_empty()
                        || self.new_node_pubkey_input.value.trim().is_empty()
                    {
                        button::secondary(Some(icon::plus_icon()), "Add Node")
                    } else {
                        button::secondary(Some(icon::plus_icon()), "Add Node")
                            .on_press(p2p(P2PMessage::MostroAddNode))
                    },
                ]
                .spacing(8),
            )
            .spacing(16),
        )
        .width(Length::Fill);

        // ── Relays card ──
        let relay_rows: Vec<Element<'a, view::Message>> = self
            .mostro_config
            .relays
            .iter()
            .map(|relay| {
                let remove_btn = button::secondary_compact(Some(icon::trash_icon()), "")
                    .on_press(p2p(P2PMessage::MostroRemoveRelay(relay.clone())));

                row![
                    icon::globe_icon().style(theme::text::secondary),
                    p2_regular(relay.as_str()).width(Length::Fill),
                    remove_btn,
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
                .into()
            })
            .collect();

        let relays_card = card::simple(
            column![
                p1_bold("Relays"),
                p2_regular("Relays used to connect to Mostro nodes").style(theme::text::secondary),
            ]
            .spacing(4)
            .push(column(relay_rows).spacing(12))
            .push(
                row![
                    form::Form::new_trimmed(
                        "wss://relay.example.com",
                        &self.new_relay_input,
                        |v| { view::Message::P2P(P2PMessage::MostroRelayInputEdited(v)) }
                    )
                    .padding(10),
                    if self.new_relay_input.value.trim().is_empty() {
                        button::secondary(Some(icon::plus_icon()), "Add")
                    } else {
                        button::secondary(Some(icon::plus_icon()), "Add")
                            .on_press(p2p(P2PMessage::MostroAddRelay))
                    },
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center),
            )
            .spacing(16),
        )
        .width(Length::Fill);

        column![nodes_card, relays_card]
            .spacing(16)
            .width(Length::Fill)
            .into()
    }
}

impl State for P2PPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        if let Menu::P2P(submenu) = menu {
            match submenu {
                P2PSubMenu::Overview => {
                    // If an order is selected, show its detail view
                    if let Some(ref selected_id) = self.selected_order {
                        if let Some(order) = self.orders.iter().find(|o| o.id == *selected_id) {
                            let content: Element<'_, view::Message> = view::dashboard(
                                menu,
                                cache,
                                column![
                                    column![
                                        h1("Order Details"),
                                        p2_regular("View order information")
                                            .style(theme::text::secondary),
                                    ]
                                    .spacing(8)
                                    .width(Length::Fill)
                                    .padding(20),
                                    container(order_detail(order))
                                        .padding([0, 20])
                                        .width(Length::Fill),
                                ]
                                .spacing(16),
                            )
                            .into();

                            // Show take order modal overlay if taking
                            if self.taking_order {
                                return coincube_ui::widget::modal::Modal::new(
                                    content,
                                    self.take_order_modal_view(order),
                                )
                                .on_blur(Some(view::Message::P2P(P2PMessage::CancelTakeOrder)))
                                .into();
                            }

                            return content;
                        }
                    }

                    let filtered_orders = self.filtered_orders();

                    // Count offers per tab
                    let buy_count = self
                        .orders
                        .iter()
                        .filter(|o| o.order_type == OrderType::Sell)
                        .count();
                    let sell_count = self
                        .orders
                        .iter()
                        .filter(|o| o.order_type == OrderType::Buy)
                        .count();

                    let overview_content: Element<'_, view::Message> = view::dashboard(
                        menu,
                        cache,
                        column![
                            // Title and filters
                            column![
                                h1("P2P Order Book"),
                                p2_regular(
                                    "Browse and take P2P trading orders from the Mostro network"
                                )
                                .style(theme::text::secondary),
                            ]
                            .spacing(8)
                            .width(Length::Fill)
                            .padding(20),
                            // Buy / Sell tabs with counts
                            container(buy_sell_tabs(&self.buy_sell_filter, buy_count, sell_count,))
                                .padding([0, 20])
                                .width(Length::Fill),
                            // Orders list
                            if filtered_orders.is_empty() {
                                Element::from(
                                    container(p1_bold("No orders found"))
                                        .padding(40)
                                        .align_x(Alignment::Center)
                                        .width(Length::Fill),
                                )
                            } else {
                                Element::from(
                                    column(
                                        filtered_orders
                                            .iter()
                                            .map(|order| order_card(order).into()),
                                    )
                                    .spacing(16)
                                    .width(Length::Fill)
                                    .padding([0, 20]),
                                )
                            },
                        ]
                        .spacing(16),
                    )
                    .into();

                    // Show payment invoice modal if seller took a buy order
                    if let Some((ref oid, ref inv, amt)) = self.pending_payment_invoice {
                        coincube_ui::widget::modal::Modal::new(
                            overview_content,
                            self.payment_invoice_modal_view(oid, inv, amt),
                        )
                        .on_blur(Some(view::Message::P2P(P2PMessage::DismissPaymentInvoice)))
                        .into()
                    } else {
                        overview_content
                    }
                }
                P2PSubMenu::MyTrades => {
                    // If a trade is selected, show its detail view
                    if let Some(ref selected_id) = self.selected_trade {
                        if let Some(trade) = self.trades.iter().find(|t| t.id == *selected_id) {
                            return view::dashboard(
                                menu,
                                cache,
                                column![
                                    column![
                                        h1("Trade Details"),
                                        p2_regular("View trade information and take actions")
                                            .style(theme::text::secondary),
                                    ]
                                    .spacing(8)
                                    .width(Length::Fill)
                                    .padding(20),
                                    container(self.trade_detail_view(trade))
                                        .padding([0, 20])
                                        .width(Length::Fill),
                                ]
                                .spacing(16),
                            )
                            .into();
                        }
                    }

                    let filtered = self.filtered_trades();
                    let shown_count = filtered.len();

                    view::dashboard(
                        menu,
                        cache,
                        column![
                            // Title
                            column![
                                h1("My Trades"),
                                p2_regular("Your active and completed P2P trades")
                                    .style(theme::text::secondary),
                            ]
                            .spacing(8)
                            .width(Length::Fill)
                            .padding(20),
                            // Trade status filter
                            container(trade_status_filter(&self.trade_filters, shown_count,))
                                .padding([0, 20])
                                .width(Length::Fill),
                            // Trade list
                            if filtered.is_empty() {
                                Element::from(
                                    container(p1_bold("No trades yet"))
                                        .padding(40)
                                        .align_x(Alignment::Center)
                                        .width(Length::Fill),
                                )
                            } else {
                                Element::from(
                                    column(filtered.iter().map(|trade| trade_card(trade).into()))
                                        .spacing(16)
                                        .width(Length::Fill)
                                        .padding([0, 20]),
                                )
                            },
                        ]
                        .spacing(16),
                    )
                    .into()
                }
                P2PSubMenu::CreateOrder => {
                    let content: Element<'_, view::Message> = view::dashboard(
                        menu,
                        cache,
                        column![
                            column![
                                h1("Create P2P Order"),
                                p2_regular(
                                    "Create a new buy or sell order for the P2P marketplace"
                                )
                                .style(theme::text::secondary),
                            ]
                            .spacing(8)
                            .width(Length::Fill)
                            .padding(20),
                            container(self.create_order_view())
                                .padding([0, 20])
                                .width(Length::Fill),
                        ]
                        .spacing(16),
                    )
                    .into();

                    if self.confirming_order {
                        coincube_ui::widget::modal::Modal::new(
                            content,
                            self.order_confirmation_view(),
                        )
                        .on_blur(Some(view::Message::P2P(P2PMessage::CancelConfirmation)))
                        .into()
                    } else {
                        content
                    }
                }
                P2PSubMenu::Settings => view::dashboard(
                    menu,
                    cache,
                    column![
                        column![
                            h1("P2P Settings"),
                            p2_regular("Configure Mostro nodes and relays")
                                .style(theme::text::secondary),
                        ]
                        .spacing(8)
                        .width(Length::Fill)
                        .padding(20),
                        container(self.mostro_settings_view())
                            .padding([0, 20])
                            .width(Length::Fill),
                    ]
                    .spacing(16),
                )
                .into(),
            }
        } else {
            view::dashboard(
                menu,
                cache,
                view::placeholder(
                    icon::person_icon().size(100),
                    "P2P Trading",
                    "Mostro P2P trading integration",
                ),
            )
            .into()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let cube_name = self
            .wallet
            .as_ref()
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "default".to_string());
        let active_pubkey = self.mostro_config.active_pubkey_hex().to_string();
        let relays = self.mostro_config.relays.clone();
        super::mostro::mostro_subscription(cube_name, active_pubkey, relays)
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn crate::daemon::Daemon + Sync + Send>>,
        _cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let msg = match message {
            Message::View(view::Message::P2P(msg)) => msg,
            _ => return Task::none(),
        };
        match msg {
            P2PMessage::OrderTypeSelected(t) => self.create_order_type = t,
            P2PMessage::PricingModeSelected(m) => self.create_pricing_mode = m,
            P2PMessage::FiatAmountEdited(_) => {}
            P2PMessage::FiatCurrencyEdited(v) => {
                self.create_fiat_currency = v;
                self.create_payment_methods.clear();
                self.rebuild_payment_method_combo();
            }
            P2PMessage::SatsAmountEdited(v) => self.create_sats_amount.value = v,
            P2PMessage::PremiumEdited(v) => self.create_premium.value = v,
            P2PMessage::PaymentMethodSelected(v) => {
                if !self.create_payment_methods.contains(&v) {
                    self.create_payment_methods.push(v);
                    self.rebuild_payment_method_combo();
                }
            }
            P2PMessage::PaymentMethodRemoved(v) => {
                self.create_payment_methods.retain(|m| m != &v);
                self.rebuild_payment_method_combo();
            }
            P2PMessage::CustomPaymentMethodEdited(v) => {
                self.create_custom_payment_method.value = v;
            }
            P2PMessage::AddCustomPaymentMethod => {
                let custom = self.create_custom_payment_method.value.trim().to_string();
                if !custom.is_empty() && !self.create_payment_methods.contains(&custom) {
                    self.create_payment_methods.push(custom);
                    self.create_custom_payment_method = Default::default();
                    self.rebuild_payment_method_combo();
                }
            }
            P2PMessage::MinAmountEdited(v) => {
                self.create_min_amount.value = v;
                if self.is_range_order() {
                    self.create_pricing_mode = PricingMode::Market;
                }
            }
            P2PMessage::MaxAmountEdited(v) => {
                self.create_max_amount.value = v;
                if self.is_range_order() {
                    self.create_pricing_mode = PricingMode::Market;
                }
            }
            P2PMessage::LightningAddressEdited(v) => {
                self.create_lightning_address.value = v;
            }
            P2PMessage::ExpiryDaysEdited(_) => {}
            P2PMessage::SubmitOrder => {
                self.order_submit_error = None;
                self.confirming_order = true;
            }
            P2PMessage::CancelConfirmation => {
                self.confirming_order = false;
            }
            P2PMessage::ConfirmOrder => {
                self.confirming_order = false;
                self.order_submitting = true;
                self.order_submit_error = None;

                let form = self.build_order_form();

                return Task::perform(super::mostro::submit_order(form), |result| {
                    Message::View(view::Message::P2P(P2PMessage::OrderSubmitResult(result)))
                });
            }
            P2PMessage::ClearForm => self.clear_create_form(),
            P2PMessage::MostroNodeInfoReceived { currencies } => {
                self.node_currencies = currencies;
                self.rebuild_currency_combo();
            }
            P2PMessage::MostroOrdersReceived(orders) => self.orders = orders,
            P2PMessage::MostroTradesReceived(trades) => self.trades = trades,
            P2PMessage::BuySellFilterChanged(filter) => self.buy_sell_filter = filter,
            P2PMessage::TradeFilterChanged(filter) => {
                if filter == TradeFilter::All {
                    // "All" is exclusive — toggle it alone
                    if self.trade_filters.contains(&TradeFilter::All) {
                        // Already showing all, do nothing
                    } else {
                        self.trade_filters = vec![TradeFilter::All];
                    }
                } else if self.trade_filters.contains(&filter) {
                    // Deselect this filter
                    self.trade_filters.retain(|f| f != &filter);
                    if self.trade_filters.is_empty() {
                        self.trade_filters = vec![TradeFilter::All];
                    }
                } else {
                    // Select this filter, remove "All" if present
                    self.trade_filters.retain(|f| f != &TradeFilter::All);
                    self.trade_filters.push(filter);
                }
            }
            P2PMessage::SelectOrder(id) => self.selected_order = Some(id),
            P2PMessage::CloseOrderDetail => self.selected_order = None,
            P2PMessage::CopyOrderId(id) => {
                return Task::done(Message::View(view::Message::Clipboard(id)));
            }
            P2PMessage::CancelOrder(order_id) => {
                let data = super::mostro::TradeActionData {
                    order_id,
                    cube_name: self
                        .wallet
                        .as_ref()
                        .map(|w| w.name.clone())
                        .unwrap_or_else(|| "default".to_string()),
                    invoice: None,
                    mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                    relay_urls: self.mostro_config.relays.clone(),
                };
                return Task::perform(super::mostro::cancel_trade(data), |result| {
                    Message::View(view::Message::P2P(P2PMessage::CancelOrderResult(
                        result.map(|_| ()),
                    )))
                });
            }
            P2PMessage::CancelOrderResult(result) => match result {
                Ok(()) => {
                    self.selected_order = None;
                    return Task::done(Message::View(view::Message::ShowSuccess(
                        "Order canceled".to_string(),
                    )));
                }
                Err(e) => {
                    return Task::done(Message::View(view::Message::ShowError(format!(
                        "Cancel failed: {}",
                        e
                    ))));
                }
            },
            P2PMessage::OrderSubmitResult(result) => {
                self.order_submitting = false;
                match result {
                    Ok(resp) => {
                        let super::mostro::OrderSubmitResponse::Success { order_id } = resp;
                        tracing::info!("Order created: {}", order_id);
                        self.clear_create_form();
                        return Task::done(Message::View(view::Message::ShowSuccess(format!(
                            "Order created successfully ({})",
                            order_id
                        ))));
                    }
                    Err(e) => {
                        self.order_submit_error = Some(e);
                    }
                }
            }
            // Mostro settings
            P2PMessage::MostroRelayInputEdited(v) => {
                self.new_relay_input.value = v;
                self.new_relay_input.warning = None;
                self.new_relay_input.valid = true;
            }
            P2PMessage::MostroAddRelay => {
                let url = self.new_relay_input.value.trim().to_string();
                if !url.starts_with("wss://") {
                    self.new_relay_input.valid = false;
                    self.new_relay_input.warning = Some("URL must start with wss://");
                } else if self.mostro_config.relays.contains(&url) {
                    self.new_relay_input.valid = false;
                    self.new_relay_input.warning = Some("Relay already exists");
                } else {
                    self.mostro_config.relays.push(url);
                    self.new_relay_input = Default::default();
                    let _ = save_mostro_config(&self.mostro_config);
                }
            }
            P2PMessage::MostroRemoveRelay(url) => {
                self.mostro_config.relays.retain(|r| r != &url);
                self.mostro_config.ensure_defaults();
                let _ = save_mostro_config(&self.mostro_config);
            }
            P2PMessage::MostroNodeNameInputEdited(v) => {
                self.new_node_name_input.value = v;
                self.new_node_name_input.warning = None;
                self.new_node_name_input.valid = true;
            }
            P2PMessage::MostroNodePubkeyInputEdited(v) => {
                self.new_node_pubkey_input.value = v;
                self.new_node_pubkey_input.warning = None;
                self.new_node_pubkey_input.valid = true;
            }
            P2PMessage::MostroAddNode => {
                let name = self.new_node_name_input.value.trim().to_string();
                let pubkey = self.new_node_pubkey_input.value.trim().to_string();
                if name.is_empty() {
                    self.new_node_name_input.valid = false;
                    self.new_node_name_input.warning = Some("Node name is required");
                } else if nostr_sdk::PublicKey::from_hex(&pubkey).is_err() {
                    self.new_node_pubkey_input.valid = false;
                    self.new_node_pubkey_input.warning = Some("Invalid hex pubkey");
                } else if self
                    .mostro_config
                    .nodes
                    .iter()
                    .any(|n| n.pubkey_hex == pubkey)
                {
                    self.new_node_pubkey_input.valid = false;
                    self.new_node_pubkey_input.warning =
                        Some("Node with this pubkey already exists");
                } else {
                    self.mostro_config.nodes.push(MostroNode {
                        name,
                        pubkey_hex: pubkey,
                    });
                    self.new_node_name_input = Default::default();
                    self.new_node_pubkey_input = Default::default();
                    let _ = save_mostro_config(&self.mostro_config);
                }
            }
            P2PMessage::MostroRemoveNode(pubkey) => {
                self.mostro_config.nodes.retain(|n| n.pubkey_hex != pubkey);
                self.mostro_config.ensure_defaults();
                let _ = save_mostro_config(&self.mostro_config);
            }
            P2PMessage::MostroSelectActiveNode(pubkey) => {
                self.mostro_config.active_node_pubkey = pubkey;
                let _ = save_mostro_config(&self.mostro_config);
            }
            // Take order flow
            P2PMessage::TakeOrder => {
                self.taking_order = true;
                self.take_order_amount = Default::default();
                self.take_order_invoice = Default::default();
                self.take_order_submitting = false;
            }
            P2PMessage::TakeOrderAmountEdited(v) => {
                self.take_order_amount.value = v;
            }
            P2PMessage::TakeOrderInvoiceEdited(v) => {
                self.take_order_invoice.value = v;
            }
            P2PMessage::CancelTakeOrder => {
                self.taking_order = false;
            }
            P2PMessage::ConfirmTakeOrder => {
                self.take_order_submitting = true;

                if let Some(ref selected_id) = self.selected_order {
                    if let Some(order) = self.orders.iter().find(|o| o.id == *selected_id) {
                        let amount = if order.is_range_order() {
                            self.take_order_amount.value.parse::<i64>().ok()
                        } else {
                            None
                        };
                        let invoice = {
                            let v = self.take_order_invoice.value.trim().to_string();
                            if v.is_empty() {
                                None
                            } else {
                                Some(v)
                            }
                        };

                        let data = super::mostro::TakeOrderData {
                            order_id: order.id.clone(),
                            order_type: order.order_type.clone(),
                            cube_name: self
                                .wallet
                                .as_ref()
                                .map(|w| w.name.clone())
                                .unwrap_or_else(|| "default".to_string()),
                            amount,
                            lightning_invoice: invoice,
                            mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                            relay_urls: self.mostro_config.relays.clone(),
                        };

                        return Task::perform(super::mostro::take_order(data), |result| {
                            Message::View(view::Message::P2P(P2PMessage::TakeOrderResult(result)))
                        });
                    }
                }
                self.take_order_submitting = false;
            }
            P2PMessage::TakeOrderResult(result) => {
                self.take_order_submitting = false;
                self.taking_order = false;
                match result {
                    Ok(super::mostro::TakeOrderResponse::Success { order_id, .. }) => {
                        tracing::info!("Order taken: {}", order_id);

                        self.selected_order = None;
                        return Task::done(Message::View(view::Message::ShowSuccess(format!(
                            "Order taken successfully ({})",
                            order_id
                        ))));
                    }
                    Ok(super::mostro::TakeOrderResponse::PaymentRequired {
                        order_id,
                        invoice,
                        amount_sats,
                        ..
                    }) => {
                        tracing::info!("Order taken, payment required: {}", order_id);

                        self.selected_order = None;
                        self.pending_payment_invoice = Some((order_id, invoice, amount_sats));
                    }
                    Err(e) => {
                        return Task::done(Message::View(view::Message::ShowError(e)));
                    }
                }
            }
            P2PMessage::DismissPaymentInvoice => {
                self.pending_payment_invoice = None;
            }
            P2PMessage::CopyPaymentInvoice(invoice) => {
                self.pending_payment_invoice = None;
                return Task::done(Message::View(view::Message::Clipboard(invoice)));
            }
            // Trade detail
            P2PMessage::SelectTrade(id) => {
                self.selected_trade = Some(id);
                self.trade_invoice_input = Default::default();
                self.trade_action_loading = false;
            }
            P2PMessage::CloseTradeDetail => {
                self.selected_trade = None;
            }
            // Trade actions
            P2PMessage::TradeInvoiceEdited(v) => {
                self.trade_invoice_input.value = v;
            }
            P2PMessage::SubmitInvoice => {
                self.trade_action_loading = true;
                if let Some(ref order_id) = self.selected_trade {
                    let data = super::mostro::TradeActionData {
                        order_id: order_id.clone(),
                        cube_name: self
                            .wallet
                            .as_ref()
                            .map(|w| w.name.clone())
                            .unwrap_or_else(|| "default".to_string()),
                        invoice: Some(self.trade_invoice_input.value.trim().to_string()),
                        mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                        relay_urls: self.mostro_config.relays.clone(),
                    };
                    return Task::perform(super::mostro::submit_invoice(data), |result| {
                        Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
                    });
                }
                self.trade_action_loading = false;
            }
            P2PMessage::ConfirmFiatSent => {
                self.trade_action_loading = true;
                if let Some(ref order_id) = self.selected_trade {
                    let data = super::mostro::TradeActionData {
                        order_id: order_id.clone(),
                        cube_name: self
                            .wallet
                            .as_ref()
                            .map(|w| w.name.clone())
                            .unwrap_or_else(|| "default".to_string()),
                        invoice: None,
                        mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                        relay_urls: self.mostro_config.relays.clone(),
                    };
                    return Task::perform(super::mostro::confirm_fiat_sent(data), |result| {
                        Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
                    });
                }
                self.trade_action_loading = false;
            }
            P2PMessage::ConfirmFiatReceived => {
                self.trade_action_loading = true;
                if let Some(ref order_id) = self.selected_trade {
                    let data = super::mostro::TradeActionData {
                        order_id: order_id.clone(),
                        cube_name: self
                            .wallet
                            .as_ref()
                            .map(|w| w.name.clone())
                            .unwrap_or_else(|| "default".to_string()),
                        invoice: None,
                        mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                        relay_urls: self.mostro_config.relays.clone(),
                    };
                    return Task::perform(super::mostro::confirm_fiat_received(data), |result| {
                        Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
                    });
                }
                self.trade_action_loading = false;
            }
            P2PMessage::CancelTrade => {
                self.trade_action_loading = true;
                if let Some(ref order_id) = self.selected_trade {
                    let data = super::mostro::TradeActionData {
                        order_id: order_id.clone(),
                        cube_name: self
                            .wallet
                            .as_ref()
                            .map(|w| w.name.clone())
                            .unwrap_or_else(|| "default".to_string()),
                        invoice: None,
                        mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                        relay_urls: self.mostro_config.relays.clone(),
                    };
                    return Task::perform(super::mostro::cancel_trade(data), |result| {
                        Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
                    });
                }
                self.trade_action_loading = false;
            }
            P2PMessage::OpenDispute => {
                self.trade_action_loading = true;
                if let Some(ref order_id) = self.selected_trade {
                    let data = super::mostro::TradeActionData {
                        order_id: order_id.clone(),
                        cube_name: self
                            .wallet
                            .as_ref()
                            .map(|w| w.name.clone())
                            .unwrap_or_else(|| "default".to_string()),
                        invoice: None,
                        mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                        relay_urls: self.mostro_config.relays.clone(),
                    };
                    return Task::perform(super::mostro::open_dispute(data), |result| {
                        Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
                    });
                }
                self.trade_action_loading = false;
            }
            P2PMessage::TradeActionResult(result) => {
                self.trade_action_loading = false;
                match result {
                    Ok(resp) => {
                        let super::mostro::TradeActionResponse::Success { new_status } = resp;
                        tracing::info!("Trade action succeeded: {}", new_status);

                        // Update last_dm_action and status on the matching in-memory trade
                        if let Some(ref order_id) = self.selected_trade {
                            if let Some(trade) = self.trades.iter_mut().find(|t| t.id == *order_id)
                            {
                                trade.last_dm_action = Some(new_status.clone());
                                if let Some(s) = super::mostro::dm_action_to_status(&new_status) {
                                    trade.status = s;
                                }
                            }
                            let cube_name = self
                                .wallet
                                .as_ref()
                                .map(|w| w.name.clone())
                                .unwrap_or_else(|| "default".to_string());
                            super::mostro::update_trade_dm_action(
                                &cube_name,
                                order_id,
                                &new_status,
                            );
                        }

                        return Task::done(Message::View(view::Message::ShowSuccess(format!(
                            "Action completed: {}",
                            new_status
                        ))));
                    }
                    Err(e) => {
                        return Task::done(Message::View(view::Message::ShowError(e)));
                    }
                }
            }
            // Real-time DM updates
            P2PMessage::TradeUpdate {
                order_id, action, ..
            } => {
                tracing::info!("Trade update for {}: {}", order_id, action);

                // Update last_dm_action and status on the matching in-memory trade
                if let Some(trade) = self.trades.iter_mut().find(|t| t.id == order_id) {
                    trade.last_dm_action = Some(action.clone());
                    // Also update the displayed status from the DM action
                    if let Some(new_status) = super::mostro::dm_action_to_status(&action) {
                        trade.status = new_status;
                    }
                }

                // Persist to disk
                let cube_name = self
                    .wallet
                    .as_ref()
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| "default".to_string());
                super::mostro::update_trade_dm_action(&cube_name, &order_id, &action);

                return Task::done(Message::View(view::Message::ShowSuccess(format!(
                    "Trade update: {} ({})",
                    action,
                    &order_id[..8.min(order_id.len())]
                ))));
            }
        }
        Task::none()
    }
}
