use std::collections::HashSet;
use std::sync::Arc;

use coincube_ui::{
    component::{button, card, form, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{column, combo_box, container, qr_code, row, slider, Space},
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

use super::components::order_card::TakeOrderState;
use super::components::trade_card::{TradeRole, TradeStatus};
use super::components::{
    buy_sell_tabs, order_card, order_detail, payment_methods_for, trade_card, trade_status_filter,
    BuySellFilter, OrderType, P2POrder, P2PTrade, PricingMode, TradeFilter, FIAT_CURRENCIES,
};
use super::config::{load_mostro_config, save_mostro_config, MostroConfig, MostroNode};

/// Per-field validation warnings for the order creation form.
#[derive(Default)]
struct FormValidation {
    amount: Option<&'static str>,
    max_amount: Option<&'static str>,
    sats: Option<&'static str>,
    /// Owned string for dynamic range messages (includes node limits).
    sats_range: Option<String>,
    premium: Option<&'static str>,
    payment: Option<&'static str>,
    /// True when node limits haven't been fetched yet (market price only).
    node_limits_missing: bool,
}

impl FormValidation {
    fn has_errors(&self) -> bool {
        self.amount.is_some()
            || self.max_amount.is_some()
            || self.sats.is_some()
            || self.sats_range.is_some()
            || self.premium.is_some()
            || self.payment.is_some()
    }
}

/// Returns true if this DM action starts a countdown timer.
fn is_countdown_action(action: &str) -> bool {
    matches!(
        action,
        "PayInvoice" | "WaitingSellerToPay" | "AddInvoice" | "WaitingBuyerInvoice"
    )
}

/// A label–value row used in order confirmation and trade detail views.
fn detail_row(label: impl ToString, value: impl ToString) -> Element<'static, view::Message> {
    row![
        p2_regular(label.to_string()).width(Length::FillPortion(2)),
        p1_bold(value.to_string()).width(Length::FillPortion(3)),
    ]
    .spacing(8)
    .into()
}

/// Chat bubble style for own messages (accent tint, rounded).
fn chat_bubble_own(_theme: &coincube_ui::theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(iced::color!(0x2A1A00))),
        border: iced::Border {
            radius: 14.0.into(),
            width: 1.0,
            color: iced::color!(0x5A3A10),
        },
        ..Default::default()
    }
}

/// Chat bubble style for counterparty messages (dark foreground, rounded).
fn chat_bubble_peer(theme: &coincube_ui::theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.colors.general.foreground)),
        border: iced::Border {
            radius: 14.0.into(),
            width: 1.0,
            color: iced::color!(0x3F3F3F),
        },
        ..Default::default()
    }
}

/// Info row style (card-like background for the trade info section in chat).
fn chat_info_card(theme: &coincube_ui::theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.colors.general.foreground)),
        border: iced::Border {
            radius: 8.0.into(),
            width: 1.0,
            color: iced::color!(0x2A2A2A),
        },
        ..Default::default()
    }
}

/// Extract the text content from a SendDm payload JSON.
fn extract_chat_text(payload_json: &str) -> String {
    // Payload is serialized as e.g. {"TextMessage":"hello"}
    if let Ok(Some(mostro_core::message::Payload::TextMessage(text))) =
        serde_json::from_str::<Option<mostro_core::message::Payload>>(payload_json)
    {
        return text;
    }
    // Fallback: try to parse as a simple JSON string value
    if let Ok(text) = serde_json::from_str::<String>(payload_json) {
        return text;
    }
    payload_json.to_string()
}

/// Data for a chat message whose send is in-flight.
struct PendingChatMessage {
    order_id: String,
    cube_name: String,
    payload: String,
    timestamp: u64,
    /// Original input text, restored on send failure.
    original_text: String,
}

pub struct P2PPanel {
    wallet: Option<Arc<Wallet>>,
    mnemonic: String,
    // Node info (fetched from info event)
    node_currencies: Vec<String>,
    node_min_order_sats: Option<i64>,
    node_max_order_sats: Option<i64>,
    // Order book state
    orders: Vec<P2POrder>,
    /// Order IDs we created locally — used to ensure is_mine stays true even
    /// if the subscription delivers the event before the session is persisted.
    my_created_order_ids: HashSet<String>,
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
    trade_rating: u8, // 1-5 star rating for counterparty
    // Hold invoice to display (seller must pay after taking a buy order)
    pending_payment_invoice: Option<(String, String, Option<i64>, qr_code::Data)>, // (order_id, invoice, amount_sats, qr_data)
    // Chat
    show_chat: bool,
    chat_input: form::Value<String>,
    /// Holds the data for a chat message that is currently being sent.
    /// On success the message is appended to the transcript; on error the
    /// input text is restored and no phantom entry is created.
    pending_chat_message: Option<PendingChatMessage>,
    chat_show_trade_info: bool,
    chat_show_user_info: bool,
    // Mostro settings
    mostro_config: MostroConfig,
    new_relay_input: form::Value<String>,
    new_node_name_input: form::Value<String>,
    new_node_pubkey_input: form::Value<String>,
}

impl P2PPanel {
    pub fn new(wallet: Option<Arc<Wallet>>, mnemonic: String) -> Self {
        Self {
            wallet,
            mnemonic,
            node_currencies: Vec::new(),
            node_min_order_sats: None,
            node_max_order_sats: None,
            orders: Vec::new(),
            my_created_order_ids: HashSet::new(),
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
            trade_rating: 0,
            pending_payment_invoice: None,
            show_chat: false,
            chat_input: Default::default(),
            pending_chat_message: None,
            chat_show_trade_info: false,
            chat_show_user_info: false,
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
        self.trades
            .iter()
            .filter(|trade| {
                self.trade_filters.iter().any(|f| match f {
                    TradeFilter::All => true,
                    TradeFilter::Pending => trade.status == TradeStatus::Pending,
                    TradeFilter::Active => {
                        matches!(
                            trade.status,
                            TradeStatus::Active | TradeStatus::SettledHoldInvoice
                        )
                    }
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
            mnemonic: self.mnemonic.clone(),
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

    /// Validate the create-order form and return per-field warnings.
    fn validate_order_form(&self) -> FormValidation {
        let is_range = self.is_range_order();
        let effective_pricing_fixed = !is_range && self.create_pricing_mode == PricingMode::Fixed;

        let mut v = FormValidation::default();

        // Maximum reasonable fiat amount (no real P2P trade exceeds this in any currency)
        const MAX_FIAT: i64 = 999_999_999;

        // --- Amount (fiat — must be a positive integer, matching Mostro's i64 field) ---
        let min_amt: Option<i64> = self.create_min_amount.value.parse::<i64>().ok();
        if self.create_min_amount.value.is_empty() {
            v.amount = Some("Amount is required");
        } else if min_amt.is_none() {
            v.amount = Some("Enter a whole number");
        } else if min_amt.unwrap_or(0) <= 0 {
            v.amount = Some("Amount must be greater than 0");
        } else if min_amt.unwrap_or(0) > MAX_FIAT {
            v.amount = Some("Amount is too large");
        }

        // --- Max amount (range — must be a positive integer greater than min) ---
        if is_range {
            let max_amt: Option<i64> = self.create_max_amount.value.parse::<i64>().ok();
            if max_amt.is_none() {
                v.max_amount = Some("Enter a whole number");
            } else if max_amt.unwrap_or(0) <= 0 {
                v.max_amount = Some("Max must be greater than 0");
            } else if max_amt.unwrap_or(0) > MAX_FIAT {
                v.max_amount = Some("Max amount is too large");
            } else if let (Some(min), Some(max)) = (min_amt, max_amt) {
                if max <= min {
                    v.max_amount = Some("Max must be greater than min");
                }
            }
        }

        // --- Sats amount (fixed price — validated against node limits) ---
        if effective_pricing_fixed {
            let node_min = self.node_min_order_sats;
            let node_max = self.node_max_order_sats;

            if self.create_sats_amount.value.is_empty() {
                v.sats = Some("Sats amount is required for fixed price");
            } else if let Ok(sats) = self.create_sats_amount.value.parse::<i64>() {
                if sats <= 0 {
                    v.sats = Some("Sats must be greater than 0");
                } else if node_min.is_none() || node_max.is_none() {
                    // Node limits not loaded — block until we know the range
                    v.sats = Some("Waiting for node limits...");
                } else {
                    if let Some(min) = node_min {
                        if sats < min {
                            v.sats_range = Some(format!(
                                "Below minimum ({} sats)",
                                super::components::format_with_separators(min as u64),
                            ));
                        }
                    }
                    if v.sats_range.is_none() {
                        if let Some(max) = node_max {
                            if sats > max {
                                v.sats_range = Some(format!(
                                    "Above maximum ({} sats)",
                                    super::components::format_with_separators(max as u64),
                                ));
                            }
                        }
                    }
                }
            } else {
                v.sats = Some("Enter a valid number");
            }
        }

        // --- Market price: warn if node limits not loaded ---
        if !effective_pricing_fixed
            && (self.node_min_order_sats.is_none() || self.node_max_order_sats.is_none())
            && !self.create_min_amount.value.is_empty()
        {
            v.node_limits_missing = true;
        }

        // --- Premium ---
        if !effective_pricing_fixed && !self.create_premium.value.is_empty() {
            if let Ok(p) = self.create_premium.value.parse::<i64>() {
                if p < -100 || p > 100 {
                    v.premium = Some("Premium must be between -100 and 100");
                }
            } else {
                v.premium = Some("Enter a valid number");
            }
        }

        // --- Payment methods ---
        let has_payment = !self.create_payment_methods.is_empty()
            || !self.create_custom_payment_method.value.trim().is_empty();
        if !has_payment {
            v.payment = Some("Select at least one payment method");
        }

        v
    }

    /// Build a `TradeActionData` for the selected trade, dispatch the given async
    /// action, and route the result to `TradeActionResult`.
    fn perform_trade_action<F, Fut>(&mut self, invoice: Option<String>, action: F) -> Task<Message>
    where
        F: FnOnce(super::mostro::TradeActionData) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<super::mostro::TradeActionResponse, String>>
            + Send
            + 'static,
    {
        self.trade_action_loading = true;
        if let Some(ref order_id) = self.selected_trade {
            let data = super::mostro::TradeActionData {
                order_id: order_id.clone(),
                cube_name: self
                    .wallet
                    .as_ref()
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| "default".to_string()),
                mnemonic: self.mnemonic.clone(),
                invoice,
                mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                relay_urls: self.mostro_config.relays.clone(),
            };
            return Task::perform(action(data), |result| {
                Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
            });
        }
        self.trade_action_loading = false;
        Task::none()
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

        // Form validation (computed each render — only show after user has typed something)
        let v = self.validate_order_form();

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

        let mut amount_col = column![
            p2_regular(amount_label).style(theme::text::secondary),
            row![
                icon::coins_icon().style(theme::text::success),
                form::Form::new_amount_sats("Amount", &self.create_min_amount, |v| {
                    view::Message::P2P(P2PMessage::MinAmountEdited(v))
                })
                .padding(10),
                form::Form::new_amount_sats("Max (optional)", &self.create_max_amount, |v| {
                    view::Message::P2P(P2PMessage::MaxAmountEdited(v))
                })
                .padding(10),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
        ]
        .spacing(12);
        // Show amount warning (only after user started typing to avoid initial noise)
        if let Some(warn) = v.amount {
            if !self.create_min_amount.value.is_empty() {
                amount_col = amount_col.push(caption(warn).style(theme::text::warning));
            }
        }
        if let Some(warn) = v.max_amount {
            amount_col = amount_col.push(caption(warn).style(theme::text::warning));
        }
        // Show node order limits as a hint, or a warning if not loaded
        if let (Some(min), Some(max)) = (self.node_min_order_sats, self.node_max_order_sats) {
            amount_col = amount_col.push(
                caption(format!(
                    "Node accepts orders between {} and {} sats",
                    super::components::format_with_separators(min as u64),
                    super::components::format_with_separators(max as u64),
                ))
                .style(theme::text::secondary),
            );
        } else if v.node_limits_missing {
            amount_col = amount_col.push(
                caption("Node limits not loaded — order may be rejected")
                    .style(theme::text::warning),
            );
        }
        let amount_card = card::simple(amount_col).width(Length::Fill);

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

        let mut payment_col = column![
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
        .spacing(12);
        if let Some(warn) = v.payment {
            payment_col = payment_col.push(caption(warn).style(theme::text::warning));
        }
        let payment_card = card::simple(payment_col).width(Length::Fill);

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
        let pricing_card: Element<'a, view::Message> = if *effective_pricing_mode
            == PricingMode::Fixed
        {
            let mut sats_col = column![
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
            .spacing(12);
            // Show sats warnings (range warnings only after user started typing)
            if let Some(warn) = v.sats {
                if !self.create_sats_amount.value.is_empty() {
                    sats_col = sats_col.push(caption(warn).style(theme::text::warning));
                }
            } else if let Some(ref warn) = v.sats_range {
                sats_col = sats_col.push(caption(warn.as_str()).style(theme::text::warning));
            }
            // Show node limits hint
            if let (Some(min), Some(max)) = (self.node_min_order_sats, self.node_max_order_sats) {
                sats_col = sats_col.push(
                    caption(format!(
                        "Allowed: {} - {} sats",
                        super::components::format_with_separators(min as u64),
                        super::components::format_with_separators(max as u64),
                    ))
                    .style(theme::text::secondary),
                );
            }
            card::simple(sats_col).width(Length::Fill).into()
        } else {
            let premium_val: f32 = self.create_premium.value.parse::<f32>().unwrap_or(0.0);
            // Dynamic slider range: expand beyond ±10 if the current value is outside
            let slider_min: f32 = (-10.0f32).min(premium_val).max(-100.0);
            let slider_max: f32 = (10.0f32).max(premium_val).min(100.0);
            let premium_int = premium_val as i64;
            let premium_label = if premium_int > 0 {
                format!("+{}%", premium_int)
            } else {
                format!("{}%", premium_int)
            };

            let mut premium_col = column![
                p2_regular("Premium").style(theme::text::secondary),
                row![
                    icon::coins_icon().style(theme::text::success),
                    form::Form::new_trimmed("e.g. 5 or -3", &self.create_premium, |v| {
                        view::Message::P2P(P2PMessage::PremiumEdited(v))
                    })
                    .padding(10),
                    p1_bold(premium_label),
                ]
                .spacing(12)
                .align_y(iced::alignment::Vertical::Center),
                slider(slider_min..=slider_max, premium_val, |v: f32| {
                    view::Message::P2P(P2PMessage::PremiumEdited((v as i64).to_string()))
                })
                .step(1.0),
            ]
            .spacing(12);
            if !self.create_premium.value.is_empty() {
                if let Some(warn) = v.premium {
                    premium_col = premium_col.push(caption(warn).style(theme::text::warning));
                }
            }
            card::simple(premium_col).width(Length::Fill).into()
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

        // Submit: use form validation
        let can_submit =
            !v.has_errors() && !self.order_submitting && !self.create_min_amount.value.is_empty();

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
            TradeStatus::Active | TradeStatus::FiatSent | TradeStatus::SettledHoldInvoice => {
                theme::pill::success as fn(&_) -> _
            }
            TradeStatus::Success => theme::pill::primary as fn(&_) -> _,
            TradeStatus::Pending
            | TradeStatus::WaitingPayment
            | TradeStatus::WaitingBuyerInvoice => theme::pill::simple as fn(&_) -> _,
            TradeStatus::PaymentFailed
            | TradeStatus::Canceled
            | TradeStatus::CooperativelyCanceled
            | TradeStatus::Dispute
            | TradeStatus::Expired => theme::pill::warning as fn(&_) -> _,
        };

        // Is user the buyer? order_type always stores OUR perspective (what we're doing).
        let is_buyer = trade.order_type == OrderType::Buy;

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
                    format!("{:.2} {}", trade.fiat_amount, trade.fiat_currency)
                ),
                if trade.is_fixed_price() {
                    detail_row("Sats", format!("{}", trade.sats_amount.unwrap_or(0)))
                } else {
                    detail_row("Price", format!("Market {}", trade.premium_text()))
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

        // States where rating is available
        let can_rate = matches!(
            dm_action,
            Some("HoldInvoicePaymentSettled" | "PurchaseCompleted" | "Rate")
        );

        // Terminal states — no action buttons at all
        let is_terminal = matches!(
            dm_action,
            Some(
                "RateReceived"
                    | "CooperativeCancelAccepted"
                    | "AdminSettled"
                    | "AdminCanceled"
                    | "HoldInvoicePaymentCanceled"
            )
        ) || (!can_rate
            && matches!(
                trade.status,
                TradeStatus::Success | TradeStatus::Canceled | TradeStatus::Expired
            ));

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

        // --- Countdown timer for time-limited states ---
        // Only show for PayInvoice/WaitingSellerToPay and AddInvoice/WaitingBuyerInvoice
        if let Some(start_ts) = trade.countdown_start_ts {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let expiration_secs: u64 = 900; // 15 minutes
            let expires_at = start_ts + expiration_secs;
            if expires_at > now {
                let remaining = expires_at - now;
                let mins = remaining / 60;
                let secs = remaining % 60;
                actions = actions.push(
                    container(
                        column![
                            icon::clock_icon().size(24).style(theme::text::secondary),
                            p1_bold(format!("{:02}:{:02}", mins, secs)),
                            caption("Time remaining").style(theme::text::secondary),
                        ]
                        .spacing(4)
                        .align_x(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .center_x(Length::Fill)
                    .padding([12, 0]),
                );
            } else {
                actions = actions.push(
                    container(p2_regular("Time expired").style(theme::text::warning))
                        .width(Length::Fill)
                        .center_x(Length::Fill)
                        .padding([8, 0]),
                );
            }
        }

        if !is_terminal {
            // --- Status message for cooperative cancel ---
            if cancel_initiated_by_you {
                actions = actions.push(
                    card::simple(
                        column![
                            p1_bold("Cancellation requested"),
                            p2_regular(
                                "You have initiated a cooperative cancel. \
                                Waiting for your counterparty to accept. \
                                If they don't respond, you can open a dispute.",
                            )
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
                            p1_bold("Cancel requested by counterparty"),
                            p2_regular(
                                "Your counterparty wants to cancel this order. \
                                If you agree, press Cancel. Otherwise, you can \
                                open a dispute.",
                            )
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
                        column![
                            p1_bold("Dispute opened"),
                            p2_regular(
                                "You have initiated a dispute. A solver will be \
                                assigned soon to help resolve this.",
                            )
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            } else if matches!(dm_action, Some("DisputeInitiatedByPeer")) {
                actions = actions.push(
                    card::simple(
                        column![
                            p1_bold("Dispute opened by counterparty"),
                            p2_regular(
                                "Your counterparty has opened a dispute. A solver will be \
                                assigned soon to help resolve this.",
                            )
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            } else if matches!(dm_action, Some("AdminTookDispute")) {
                actions = actions.push(
                    card::simple(
                        column![
                            p1_bold("Admin reviewing dispute"),
                            p2_regular(
                                "A dispute resolver has been assigned to your case. \
                                They will contact you to help resolve the issue.",
                            )
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .width(Length::Fill),
                );
            }

            // --- Primary trade action buttons (only if not in cooperative cancel or dispute) ---
            // Button logic follows the Mostro protocol: Mostro sends different DM
            // actions to buyer and seller, so we match on (dm_action, role) pairs.
            if !in_cooperative_cancel && !dispute_initiated {
                match dm_action {
                    // ── Seller must pay hold invoice ──
                    Some("PayInvoice") | Some("WaitingSellerToPay") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Waiting for seller payment"),
                                        p2_regular(
                                            "A payment request has been sent to the seller. \
                                            If the seller doesn't complete the payment in time, \
                                            the trade will be canceled.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Pay the hold invoice"),
                                        p2_regular(
                                            "Please pay the hold invoice to lock your sats \
                                            and start the trade. If you don't pay in time, \
                                            the trade will be canceled.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Hold invoice paid / Trade active ──
                    Some("HoldInvoicePaymentAccepted") | Some("BuyerInvoiceAccepted") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Send fiat payment"),
                                        p2_regular(
                                            "Contact the seller to arrange payment. \
                                            Once you've sent the fiat money, \
                                            press the button below to notify the seller.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                            actions = actions.push(if !loading {
                                button::primary(None, "Confirm Fiat Sent")
                                    .on_press(p2p(P2PMessage::ConfirmFiatSent))
                                    .width(Length::Fill)
                            } else {
                                button::primary(None, "Confirm Fiat Sent").width(Length::Fill)
                            });
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Waiting for fiat payment"),
                                        p2_regular(
                                            "Contact the buyer to inform them how to send \
                                            the fiat payment. You'll be notified when \
                                            the buyer confirms the payment.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }
                    // BuyerTookOrder: seller is notified a buyer took their order
                    Some("BuyerTookOrder") => {
                        actions = actions.push(
                            card::simple(
                                column![
                                    p1_bold("Buyer took your order"),
                                    p2_regular(
                                        "A buyer has taken your order. Please wait while \
                                        they decide to proceed. If they accept, you'll \
                                        be notified to complete your part.",
                                    )
                                    .style(theme::text::secondary),
                                ]
                                .spacing(4),
                            )
                            .width(Length::Fill),
                        );
                    }

                    // ── Buyer must submit invoice ──
                    Some("AddInvoice") | Some("WaitingBuyerInvoice") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Submit your invoice"),
                                        p2_regular(
                                            "Please send a Lightning invoice or address \
                                            where you'll receive the sats. If you don't \
                                            provide one in time, the trade will be canceled.",
                                        )
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
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Waiting for buyer invoice"),
                                        p2_regular(
                                            "Payment received! Your sats are now held. \
                                            The buyer has been asked to provide an invoice. \
                                            If they don't do so in time, your sats will be \
                                            returned and the trade will be canceled.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Waiting for fiat payment (status-based fallback) ──
                    Some("WaitingPayment") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Send fiat payment"),
                                        p2_regular(
                                            "Contact the seller to arrange payment. \
                                            Once you've sent the fiat money, \
                                            press the button below to notify the seller.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                            actions = actions.push(if !loading {
                                button::primary(None, "Confirm Fiat Sent")
                                    .on_press(p2p(P2PMessage::ConfirmFiatSent))
                                    .width(Length::Fill)
                            } else {
                                button::primary(None, "Confirm Fiat Sent").width(Length::Fill)
                            });
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Waiting for fiat payment"),
                                        p2_regular(
                                            "The buyer has been notified to send fiat. \
                                            You'll be notified when they confirm.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Fiat sent / waiting for release ──
                    Some("FiatSentOk") | Some("FiatSent") => {
                        if !is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Buyer sent fiat"),
                                        p2_regular(
                                            "The buyer has confirmed sending the fiat payment. \
                                            Once you've verified receipt, release the sats. \
                                            This action cannot be undone.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                            actions = actions.push(if !loading {
                                button::primary(None, "Release Sats")
                                    .on_press(p2p(P2PMessage::ConfirmFiatReceived))
                                    .width(Length::Fill)
                            } else {
                                button::primary(None, "Release Sats").width(Length::Fill)
                            });
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Fiat sent"),
                                        p2_regular(
                                            "The seller has been notified that you sent the fiat. \
                                            Once the seller confirms receipt, they will release \
                                            the sats. If they don't, you can open a dispute.",
                                        )
                                        .style(theme::text::secondary),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Released / settling ──
                    Some("Released") | Some("Release") => {
                        actions = actions.push(
                            card::simple(
                                column![
                                    p1_bold("Sats released"),
                                    p2_regular(if is_buyer {
                                        "The seller has released the sats! Expect your \
                                        invoice to be paid shortly. Make sure your wallet \
                                        is online to receive via Lightning Network."
                                    } else {
                                        "You have released the sats. The payment is being \
                                        processed to the buyer's invoice."
                                    })
                                    .style(theme::text::secondary),
                                ]
                                .spacing(4),
                            )
                            .width(Length::Fill),
                        );
                    }

                    // ── Payment failed ──
                    Some("PaymentFailed") => {
                        if is_buyer {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Payment failed"),
                                        p2_regular(
                                            "The payment to your invoice could not be completed. \
                                            Please submit a new invoice to receive the sats.",
                                        )
                                        .style(theme::text::warning),
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
                        } else {
                            actions = actions.push(
                                card::simple(
                                    column![
                                        p1_bold("Payment failed"),
                                        p2_regular(
                                            "Payment to the buyer's invoice failed. \
                                            Waiting for the buyer to submit a new invoice.",
                                        )
                                        .style(theme::text::warning),
                                    ]
                                    .spacing(4),
                                )
                                .width(Length::Fill),
                            );
                        }
                    }

                    // ── Trade completed — rate counterparty ──
                    Some("HoldInvoicePaymentSettled")
                    | Some("PurchaseCompleted")
                    | Some("Rate") => {
                        let selected = self.trade_rating;
                        let star_buttons: Vec<Element<'_, view::Message>> = (1..=5u8)
                            .map(|i| {
                                let label = if i <= selected {
                                    "\u{2605}"
                                } else {
                                    "\u{2606}"
                                };
                                iced::widget::button(iced::widget::text(label).size(28).style(
                                    if i <= selected {
                                        theme::text::primary
                                    } else {
                                        theme::text::secondary
                                    },
                                ))
                                .on_press(p2p(P2PMessage::RatingSelected(i)))
                                .style(theme::button::container)
                                .into()
                            })
                            .collect();
                        let stars = iced::widget::Row::with_children(star_buttons).spacing(4);

                        actions = actions.push(
                            card::simple(
                                column![
                                    p1_bold("Trade Complete!"),
                                    p2_regular("Rate your counterparty")
                                        .style(theme::text::secondary),
                                    container(
                                        column![
                                            stars,
                                            p2_regular(if selected > 0 {
                                                format!("{} / 5", selected)
                                            } else {
                                                "Select a rating".to_string()
                                            })
                                            .style(theme::text::secondary),
                                        ]
                                        .spacing(4)
                                        .align_x(Alignment::Center),
                                    )
                                    .width(Length::Fill)
                                    .center_x(Length::Fill),
                                    if selected > 0 && !loading {
                                        button::primary(None, "Submit Rating")
                                            .on_press(p2p(P2PMessage::SubmitRating))
                                            .width(Length::Fill)
                                    } else {
                                        button::primary(None, "Submit Rating").width(Length::Fill)
                                    },
                                ]
                                .spacing(12),
                            )
                            .width(Length::Fill),
                        );
                    }

                    _ => {
                        // No DM action or unrecognized — fall back to status-based buttons
                        match trade.status {
                            TradeStatus::WaitingBuyerInvoice if is_buyer => {
                                actions = actions.push(
                                    card::simple(
                                        column![
                                            p2_regular(
                                                "Submit your Lightning invoice to receive sats"
                                            )
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
                            TradeStatus::Active if is_buyer => {
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
            // Hide once the trade is complete or past release
            let trade_complete = matches!(
                dm_action,
                Some(
                    "Released"
                        | "Release"
                        | "HoldInvoicePaymentSettled"
                        | "PurchaseCompleted"
                        | "Rate"
                        | "RateReceived"
                        | "AdminSettled"
                        | "AdminCanceled"
                        | "CooperativeCancelAccepted"
                )
            ) || matches!(
                trade.status,
                TradeStatus::Success | TradeStatus::Canceled | TradeStatus::CooperativelyCanceled
            );
            if !loading && !trade_complete {
                if cancel_initiated_by_peer {
                    actions = actions.push(
                        row![
                            button::secondary(None, "Close")
                                .on_press(p2p(P2PMessage::CloseTradeDetail))
                                .width(Length::Fill),
                            button::alert(None, "Accept Cancel")
                                .on_press(p2p(P2PMessage::CancelTrade))
                                .width(Length::Fill),
                            button::alert(None, "Dispute")
                                .on_press(p2p(P2PMessage::OpenDispute))
                                .width(Length::Fill),
                        ]
                        .spacing(8),
                    );
                } else if cancel_initiated_by_you {
                    actions = actions.push(
                        row![
                            button::secondary(None, "Close")
                                .on_press(p2p(P2PMessage::CloseTradeDetail))
                                .width(Length::Fill),
                            button::alert(None, "Dispute")
                                .on_press(p2p(P2PMessage::OpenDispute))
                                .width(Length::Fill),
                        ]
                        .spacing(8),
                    );
                } else if !dispute_initiated {
                    actions = actions.push(
                        row![
                            button::secondary(None, "Close")
                                .on_press(p2p(P2PMessage::CloseTradeDetail))
                                .width(Length::Fill),
                            button::secondary(None, "Cancel")
                                .on_press(p2p(P2PMessage::CancelTrade))
                                .width(Length::Fill),
                            button::alert(None, "Dispute")
                                .on_press(p2p(P2PMessage::OpenDispute))
                                .width(Length::Fill),
                        ]
                        .spacing(8),
                    );
                } else {
                    // In dispute — only Close
                    actions = actions.push(
                        button::secondary(None, "Close")
                            .on_press(p2p(P2PMessage::CloseTradeDetail))
                            .width(Length::Fill),
                    );
                }
            } else {
                // Terminal / loading / past release — just Close
                actions = actions.push(
                    button::secondary(None, "Close")
                        .on_press(p2p(P2PMessage::CloseTradeDetail))
                        .width(Length::Fill),
                );
            }
        } else {
            // Terminal state — only Close
            actions = actions.push(
                button::secondary(None, "Close")
                    .on_press(p2p(P2PMessage::CloseTradeDetail))
                    .width(Length::Fill),
            );
        }

        // ── Contact button ──
        // Show contact/chat button for active trades only (not completed/canceled)
        let chat_available = matches!(
            trade.status,
            TradeStatus::Active
                | TradeStatus::FiatSent
                | TradeStatus::SettledHoldInvoice
                | TradeStatus::CooperativelyCanceled
                | TradeStatus::Dispute
                | TradeStatus::PaymentFailed
        );

        let mut content = column![info_card, id_card, actions]
            .spacing(12)
            .width(Length::Fill);

        if chat_available {
            content = content.push(
                button::secondary(Some(icon::chat_icon()), "Contact")
                    .on_press(p2p(P2PMessage::OpenChat))
                    .width(Length::Fill),
            );
        }

        content.into()
    }

    fn chat_view<'a>(&'a self, trade: &'a P2PTrade) -> Element<'a, view::Message> {
        let p2p = |msg: P2PMessage| view::Message::P2P(msg);

        let chat_enabled = matches!(
            trade.status,
            TradeStatus::Active | TradeStatus::FiatSent | TradeStatus::SettledHoldInvoice
        );

        let cube_name = self
            .wallet
            .as_ref()
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "default".to_string());
        let all_messages = super::mostro::get_trade_messages(&cube_name, &trade.id);
        let chat_messages: Vec<&super::mostro::TradeMessage> = all_messages
            .iter()
            .filter(|m| m.action == "SendDm")
            .collect();

        // Nicknames for chat bubbles
        let peer_nick = trade
            .counterparty_pubkey
            .as_deref()
            .map(super::mostro::nickname_from_pubkey)
            .unwrap_or_else(|| "Peer".to_string());

        // ── Accordion tab buttons (Trade Information / User Information) ──
        let trade_info_btn = if self.chat_show_trade_info {
            button::primary(Some(icon::receipt_icon()), "Trade Information")
                .on_press(p2p(P2PMessage::ToggleChatTradeInfo))
                .width(Length::Fill)
        } else {
            button::secondary(Some(icon::receipt_icon()), "Trade Information")
                .on_press(p2p(P2PMessage::ToggleChatTradeInfo))
                .width(Length::Fill)
        };
        let user_info_btn = if self.chat_show_user_info {
            button::primary(Some(icon::person_icon()), "User Information")
                .on_press(p2p(P2PMessage::ToggleChatUserInfo))
                .width(Length::Fill)
        } else {
            button::secondary(Some(icon::person_icon()), "User Information")
                .on_press(p2p(P2PMessage::ToggleChatUserInfo))
                .width(Length::Fill)
        };
        let tab_row = container(row![trade_info_btn, user_info_btn].spacing(8))
            .padding([12, 16])
            .width(Length::Fill);

        // ── Trade Information panel (expandable) ──
        let order_short = &trade.id[..8.min(trade.id.len())];
        let trade_type_label = match trade.order_type {
            OrderType::Buy => "Buying",
            OrderType::Sell => "Selling",
        };
        let sats_text = trade
            .sats_amount
            .map(|s| format!("{} sats", super::components::format_with_separators(s)))
            .unwrap_or_else(|| "Market price".to_string());
        let created_date = {
            let dt = chrono::DateTime::from_timestamp(trade.created_at_ts, 0)
                .unwrap_or_default()
                .with_timezone(&chrono::Local);
            dt.format("%B %d, %Y").to_string()
        };

        let trade_info_panel: Element<'_, view::Message> = if self.chat_show_trade_info {
            container(
                column![
                    // Order ID
                    row![
                        p2_regular("Order ID:").style(theme::text::secondary),
                        Space::new().width(Length::Fill),
                        p2_regular(order_short),
                    ]
                    .spacing(8),
                    // Trade summary
                    container(
                        column![
                            p1_bold(format!(
                                "{} {} {}",
                                trade_type_label, sats_text, trade.fiat_currency
                            )),
                            p2_regular(format!(
                                "for {:.2} {}",
                                trade.fiat_amount, trade.fiat_currency
                            ))
                            .style(theme::text::secondary),
                        ]
                        .spacing(4),
                    )
                    .padding(12)
                    .width(Length::Fill)
                    .style(chat_info_card as fn(&_) -> _),
                    // Payment method
                    row![
                        icon::cash_icon().style(theme::text::secondary),
                        column![
                            p2_regular("Payment Method").style(theme::text::secondary),
                            p2_bold(&trade.payment_method),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                    // Created date
                    row![
                        icon::calendar_icon().style(theme::text::secondary),
                        column![
                            p2_regular("Created on").style(theme::text::secondary),
                            p2_bold(created_date),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(12),
            )
            .padding(iced::Padding {
                top: 0.0,
                right: 16.0,
                bottom: 12.0,
                left: 16.0,
            })
            .width(Length::Fill)
            .into()
        } else {
            Space::new().height(0).into()
        };

        // ── User Information panel (expandable) ──
        let user_info_panel: Element<'_, view::Message> = if self.chat_show_user_info {
            let identity =
                super::mostro::get_chat_identity_info(&cube_name, &self.mnemonic, &trade.id);
            let cp_nickname = identity
                .counterparty_nickname
                .as_deref()
                .unwrap_or("Unknown");
            let cp_pubkey_full = identity
                .counterparty_pubkey
                .as_deref()
                .unwrap_or("Unknown")
                .to_string();
            let our_nickname = identity.our_nickname.as_deref().unwrap_or("Unknown");
            let our_pubkey_full = identity
                .our_trade_pubkey
                .as_deref()
                .unwrap_or("Unknown")
                .to_string();
            let shared_key_full = identity
                .shared_key
                .as_deref()
                .unwrap_or("Not available")
                .to_string();

            container(
                column![
                    // Counterparty
                    row![
                        icon::person_icon().style(theme::text::secondary),
                        column![
                            p2_regular("Peer").style(theme::text::secondary),
                            p1_bold(cp_nickname),
                            caption(cp_pubkey_full).style(theme::text::secondary),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                    // Our trade key
                    row![
                        icon::key_icon().style(theme::text::secondary),
                        column![
                            p2_regular("You").style(theme::text::secondary),
                            p1_bold(our_nickname),
                            caption(our_pubkey_full).style(theme::text::secondary),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                    // Shared key
                    row![
                        icon::lock_icon().style(theme::text::secondary),
                        column![
                            p2_regular("Shared Key").style(theme::text::secondary),
                            p2_bold(shared_key_full),
                        ]
                        .spacing(2),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(12),
            )
            .padding(iced::Padding {
                top: 0.0,
                right: 16.0,
                bottom: 12.0,
                left: 16.0,
            })
            .width(Length::Fill)
            .into()
        } else {
            Space::new().height(0).into()
        };

        // Thin divider
        let divider: Element<'_, view::Message> =
            container(Space::new().height(1).width(Length::Fill))
                .style(
                    |_theme: &coincube_ui::theme::Theme| iced::widget::container::Style {
                        background: Some(iced::Background::Color(iced::color!(0x333333))),
                        ..Default::default()
                    },
                )
                .into();

        // ── Message list ──
        let mut msg_col = column![].spacing(10).padding([12, 16]);

        if chat_messages.is_empty() {
            msg_col = msg_col.push(
                container(
                    column![
                        icon::chat_icon().size(40.0).style(theme::text::secondary),
                        p1_regular("No messages yet").style(theme::text::secondary),
                        p2_regular("Send a message to start the conversation")
                            .style(theme::text::secondary),
                    ]
                    .spacing(8)
                    .align_x(iced::alignment::Horizontal::Center),
                )
                .padding(60)
                .width(Length::Fill)
                .center_x(Length::Fill),
            );
        } else {
            for msg in &chat_messages {
                let text = extract_chat_text(&msg.payload_json);
                let ts = chrono::DateTime::from_timestamp(msg.timestamp as i64, 0)
                    .unwrap_or_default()
                    .with_timezone(&chrono::Local);
                let time_str = ts.format("%H:%M").to_string();

                if msg.is_own {
                    msg_col = msg_col.push(
                        column![
                            container(caption("You").style(theme::text::secondary))
                                .width(Length::Fill)
                                .align_right(Length::Fill),
                            row![
                                Space::new().width(Length::FillPortion(3)),
                                container(p1_regular(text))
                                    .padding([10, 16])
                                    .style(chat_bubble_own as fn(&_) -> _)
                                    .max_width(480),
                            ],
                            container(caption(time_str).style(theme::text::secondary))
                                .width(Length::Fill)
                                .align_right(Length::Fill),
                        ]
                        .spacing(2),
                    );
                } else {
                    msg_col = msg_col.push(
                        column![
                            caption(peer_nick.as_str()).style(theme::text::primary),
                            row![
                                container(p1_regular(text))
                                    .padding([10, 16])
                                    .style(chat_bubble_peer as fn(&_) -> _)
                                    .max_width(480),
                                Space::new().width(Length::FillPortion(3)),
                            ],
                            caption(time_str).style(theme::text::secondary),
                        ]
                        .spacing(2),
                    );
                }
            }
        }

        let chat_scroll = iced::widget::scrollable(msg_col)
            .height(Length::Fill)
            .anchor_bottom();

        // ── Input area ──
        let input_area: Element<'_, view::Message> = if chat_enabled {
            let can_send = !self.chat_input.value.trim().is_empty();
            let send_btn = if can_send {
                button::primary(Some(icon::send_icon()), "Send")
                    .on_press(p2p(P2PMessage::SendChatMessage))
            } else {
                button::primary(Some(icon::send_icon()), "Send")
            };
            container(
                row![
                    form::Form::new("Type a message...", &self.chat_input, |v| {
                        view::Message::P2P(P2PMessage::ChatInputEdited(v))
                    })
                    .padding(10),
                    send_btn,
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center),
            )
            .padding([12, 16])
            .width(Length::Fill)
            .into()
        } else if !chat_messages.is_empty() {
            container(p2_regular("Chat is read-only").style(theme::text::secondary))
                .padding([12, 16])
                .width(Length::Fill)
                .center_x(Length::Fill)
                .into()
        } else {
            Space::new().height(0).into()
        };

        column![
            tab_row,
            trade_info_panel,
            user_info_panel,
            divider,
            chat_scroll,
            input_area,
        ]
        .spacing(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn payment_invoice_modal_view<'a>(
        &'a self,
        order_id: &str,
        invoice: &str,
        amount_sats: Option<i64>,
        qr_data: &'a qr_code::Data,
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
                container(
                    column![
                        p1_bold("Payment Required"),
                        p2_regular(format!(
                            "Order {} taken. Pay this hold invoice to lock {} for the trade.",
                            truncated_id, amount_text
                        ))
                        .style(theme::text::secondary),
                    ]
                    .spacing(8)
                    .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .center_x(Length::Fill),
                container(
                    container(
                        iced::widget::QRCode::<coincube_ui::theme::Theme>::new(qr_data)
                            .cell_size(2),
                    )
                    .padding(10)
                    .style(|_| {
                        iced::widget::container::Style::default().background(iced::Color::WHITE)
                    })
                    .max_width(280)
                    .max_height(280),
                )
                .width(Length::Fill)
                .center_x(Length::Fill),
                row![
                    button::primary(None, "Copy")
                        .on_press(view::Message::P2P(P2PMessage::CopyPaymentInvoice(
                            invoice.to_string(),
                        )))
                        .width(Length::Fill),
                    button::secondary(None, "Close")
                        .on_press(view::Message::P2P(P2PMessage::DismissPaymentInvoice))
                        .width(Length::Fill),
                ]
                .spacing(8),
                container(
                    button::transparent(None, "Cancel Order")
                        .on_press(view::Message::P2P(P2PMessage::CancelPaymentInvoice(
                            order_id.to_string(),
                        )))
                        .width(Length::Fill),
                )
                .width(Length::Fill)
                .center_x(Length::Fill),
            ]
            .spacing(16)
            .align_x(Alignment::Center),
        )
        .width(Length::Fixed(450.0))
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
                    // Built separately (not inside dashboard's scrollable) to avoid
                    // first-click issues when switching from the order list.
                    if let Some(ref selected_id) = self.selected_order {
                        if let Some(order) = self.orders.iter().find(|o| o.id == *selected_id) {
                            let take_state = if self.taking_order {
                                Some(TakeOrderState {
                                    amount: &self.take_order_amount,
                                    invoice: &self.take_order_invoice,
                                    submitting: self.take_order_submitting,
                                })
                            } else {
                                None
                            };
                            let has_vault = cache.has_vault;
                            let detail_content = iced::widget::scrollable(row![
                                Space::new().width(Length::FillPortion(1)),
                                column![
                                    Space::new().height(Length::Fixed(30.0)),
                                    column![
                                        h1("Order Details"),
                                        p2_regular("View order information")
                                            .style(theme::text::secondary),
                                    ]
                                    .spacing(8)
                                    .width(Length::Fill)
                                    .padding(20),
                                    container(order_detail(order, take_state))
                                        .padding([0, 20])
                                        .width(Length::Fill),
                                    Space::new().height(Length::Fixed(40.0)),
                                ]
                                .spacing(16)
                                .width(Length::FillPortion(8))
                                .max_width(1500),
                                Space::new().width(Length::FillPortion(1)),
                            ]);

                            return row![]
                                .push(
                                    view::sidebar(menu, cache, has_vault)
                                        .height(Length::Fill)
                                        .width(Length::Fixed(190.0)),
                                )
                                .push(
                                    iced::widget::Column::new()
                                        .push(view::warn(None))
                                        .push(
                                            container(detail_content)
                                                .center_x(Length::Fill)
                                                .style(theme::container::background)
                                                .height(Length::Fill),
                                        )
                                        .width(Length::Fill),
                                )
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .into();
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
                            Space::new().height(Length::Fixed(40.0)),
                        ]
                        .spacing(16),
                    );

                    // Show payment invoice modal if seller took a buy order
                    if let Some((ref oid, ref inv, amt, ref qr_data)) = self.pending_payment_invoice
                    {
                        coincube_ui::widget::modal::Modal::new(
                            overview_content,
                            self.payment_invoice_modal_view(oid, inv, amt, qr_data),
                        )
                        .on_blur(Some(view::Message::P2P(P2PMessage::DismissPaymentInvoice)))
                        .into()
                    } else {
                        overview_content
                    }
                }
                P2PSubMenu::MyTrades => {
                    // If a trade is selected, show its detail or chat view
                    if let Some(ref selected_id) = self.selected_trade {
                        if let Some(trade) = self.trades.iter().find(|t| t.id == *selected_id) {
                            if self.show_chat {
                                // Separate chat view — built without dashboard()
                                // to avoid nested scrollables.
                                let has_vault = cache.has_vault;
                                let order_short = &trade.id[..8.min(trade.id.len())];
                                let trade_type_label = match trade.order_type {
                                    OrderType::Buy => "BUY",
                                    OrderType::Sell => "SELL",
                                };
                                let peer_nick = trade
                                    .counterparty_pubkey
                                    .as_deref()
                                    .map(super::mostro::nickname_from_pubkey)
                                    .unwrap_or_else(|| "Peer".to_string());

                                // Header bar
                                let header = container(
                                    row![
                                        button::secondary(Some(icon::previous_icon()), "Back",)
                                            .on_press(view::Message::P2P(P2PMessage::CloseChat)),
                                        Space::new().width(Length::Fill),
                                        column![
                                            p1_bold(peer_nick),
                                            p2_regular(format!(
                                                "{} Order {}",
                                                trade_type_label, order_short
                                            ))
                                            .style(theme::text::secondary),
                                        ]
                                        .spacing(2)
                                        .align_x(iced::alignment::Horizontal::Center),
                                        Space::new().width(Length::Fill),
                                        // Spacer to balance the Back button
                                        Space::new().width(Length::Fixed(80.0)),
                                    ]
                                    .align_y(iced::alignment::Vertical::Center),
                                )
                                .padding([16, 20])
                                .width(Length::Fill)
                                .style(theme::container::foreground);

                                return row![]
                                    .push(
                                        view::sidebar(menu, cache, has_vault)
                                            .height(Length::Fill)
                                            .width(Length::Fixed(190.0)),
                                    )
                                    .push(
                                        iced::widget::Column::new()
                                            .push(view::warn(None))
                                            .push(header)
                                            .push(
                                                container(self.chat_view(trade))
                                                    .padding(20)
                                                    .width(Length::Fill)
                                                    .height(Length::Fill)
                                                    .style(theme::container::background),
                                            )
                                            .width(Length::Fill),
                                    )
                                    .width(Length::Fill)
                                    .height(Length::Fill)
                                    .into();
                            }
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
                                    Space::new().height(Length::Fixed(40.0)),
                                ]
                                .spacing(16),
                            );
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
                            Space::new().height(Length::Fixed(40.0)),
                        ]
                        .spacing(16),
                    )
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
                            Space::new().height(Length::Fixed(40.0)),
                        ]
                        .spacing(16),
                    );

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
                        Space::new().height(Length::Fixed(40.0)),
                    ]
                    .spacing(16),
                ),
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
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let cube_name = self
            .wallet
            .as_ref()
            .map(|w| w.name.clone())
            .unwrap_or_else(|| "default".to_string());
        let mnemonic = self.mnemonic.clone();
        let active_pubkey = self.mostro_config.active_pubkey_hex().to_string();
        let relays = self.mostro_config.relays.clone();
        let mostro_sub =
            super::mostro::mostro_subscription(cube_name, mnemonic, active_pubkey, relays);

        // Tick every second when viewing a trade detail (for action countdown timer)
        if self.selected_trade.is_some() {
            let timer = iced::time::every(std::time::Duration::from_secs(1))
                .map(|_| Message::View(view::Message::P2P(P2PMessage::TradeTimerTick)));
            Subscription::batch([mostro_sub, timer])
        } else {
            mostro_sub
        }
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
            P2PMessage::FiatCurrencyEdited(v) => {
                self.create_fiat_currency = v;
                self.create_payment_methods.clear();
                self.rebuild_payment_method_combo();
            }
            P2PMessage::SatsAmountEdited(v) => self.create_sats_amount.value = v,
            P2PMessage::PremiumEdited(v) => {
                // Allow empty, minus sign, or valid integers in -100..100 range
                let trimmed = v.trim();
                if trimmed.is_empty() || trimmed == "-" || trimmed.parse::<i64>().is_ok() {
                    self.create_premium.value = v;
                }
            }
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
                // Double-check validation before showing confirmation
                if self.validate_order_form().has_errors() {
                    return Task::none();
                }
                self.order_submit_error = None;
                self.confirming_order = true;
            }
            P2PMessage::CancelConfirmation => {
                self.confirming_order = false;
            }
            P2PMessage::ConfirmOrder => {
                // Final validation gate before sending to network
                if self.validate_order_form().has_errors() {
                    self.confirming_order = false;
                    return Task::none();
                }
                self.confirming_order = false;
                self.order_submitting = true;
                self.order_submit_error = None;

                let form = self.build_order_form();

                return Task::perform(super::mostro::submit_order(form), |result| {
                    Message::View(view::Message::P2P(P2PMessage::OrderSubmitResult(result)))
                });
            }
            P2PMessage::ClearForm => self.clear_create_form(),
            P2PMessage::MostroNodeInfoReceived {
                currencies,
                min_order_sats,
                max_order_sats,
            } => {
                if !currencies.is_empty() {
                    self.node_currencies = currencies;
                    self.rebuild_currency_combo();
                }
                self.node_min_order_sats = min_order_sats;
                self.node_max_order_sats = max_order_sats;
            }
            P2PMessage::MostroOrdersReceived(mut orders) => {
                // Patch is_mine for orders we created locally (handles race where
                // the subscription delivers the event before the session is persisted)
                for order in &mut orders {
                    if self.my_created_order_ids.contains(&order.id) {
                        order.is_mine = true;
                    }
                }
                self.orders = orders;
            }
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
            P2PMessage::SelectOrder(id) => {
                self.selected_order = Some(id);
                self.taking_order = false;
                self.take_order_amount = Default::default();
                self.take_order_invoice = Default::default();
                self.take_order_submitting = false;
            }
            P2PMessage::CloseOrderDetail => {
                self.selected_order = None;
                self.taking_order = false;
            }
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
                    mnemonic: self.mnemonic.clone(),
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
                    self.pending_payment_invoice = None;
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
                        // Track this order ID so we always mark it as ours, even if
                        // the subscription delivered the event before the session was saved.
                        self.my_created_order_ids.insert(order_id.clone());
                        if let Some(order) = self.orders.iter_mut().find(|o| o.id == order_id) {
                            order.is_mine = true;
                        }
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
                // Check if this order needs user input before confirming.
                // Non-range orders where user is selling don't need any input,
                // so skip straight to confirmation to avoid an invisible intermediate state.
                let needs_input = self
                    .selected_order
                    .as_ref()
                    .and_then(|id| self.orders.iter().find(|o| o.id == *id))
                    .map(|order| {
                        let is_buying = order.order_type == OrderType::Sell;
                        order.is_range_order() || is_buying
                    })
                    .unwrap_or(true);

                if needs_input {
                    self.taking_order = true;
                    self.take_order_amount = Default::default();
                    self.take_order_invoice = Default::default();
                    self.take_order_submitting = false;
                } else {
                    // No input needed — proceed directly
                    self.taking_order = true;
                    return self.update(
                        _daemon,
                        _cache,
                        Message::View(view::Message::P2P(P2PMessage::ConfirmTakeOrder)),
                    );
                }
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
                if let Some(ref selected_id) = self.selected_order {
                    if let Some(order) = self.orders.iter().find(|o| o.id == *selected_id) {
                        let amount = if order.is_range_order() {
                            match self.take_order_amount.value.parse::<i64>() {
                                Ok(v) => {
                                    let min = order.min_amount.unwrap_or(0.0) as i64;
                                    let max = order.max_amount.unwrap_or(i64::MAX as f64) as i64;
                                    if v < min || v > max {
                                        return Task::none();
                                    }
                                    Some(v)
                                }
                                Err(_) => return Task::none(),
                            }
                        } else {
                            None
                        };

                        self.take_order_submitting = true;
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
                            mnemonic: self.mnemonic.clone(),
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
                        match qr_code::Data::new(&invoice) {
                            Ok(qr_data) => {
                                self.pending_payment_invoice =
                                    Some((order_id, invoice, amount_sats, qr_data));
                            }
                            Err(e) => {
                                tracing::warn!("Failed to generate QR code: {e}");
                                return Task::done(Message::View(view::Message::ShowError(
                                    "Invoice too long for QR code display".to_string(),
                                )));
                            }
                        }
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
                return Task::done(Message::View(view::Message::Clipboard(invoice)));
            }
            P2PMessage::CancelPaymentInvoice(order_id) => {
                let data = super::mostro::TradeActionData {
                    order_id,
                    cube_name: self
                        .wallet
                        .as_ref()
                        .map(|w| w.name.clone())
                        .unwrap_or_else(|| "default".to_string()),
                    mnemonic: self.mnemonic.clone(),
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
            // Trade detail
            P2PMessage::SelectTrade(id) => {
                self.selected_trade = Some(id);
                self.trade_invoice_input = Default::default();
                self.trade_action_loading = false;
                self.trade_rating = 0;
                self.show_chat = false;
                self.chat_input = Default::default();
            }
            P2PMessage::CloseTradeDetail => {
                self.selected_trade = None;
                self.trade_invoice_input = Default::default();
                self.trade_rating = 0;
                self.trade_action_loading = false;
                self.show_chat = false;
                self.chat_input = Default::default();
            }
            // Trade actions
            P2PMessage::TradeInvoiceEdited(v) => {
                self.trade_invoice_input.value = v;
            }
            P2PMessage::SubmitInvoice => {
                let invoice = Some(self.trade_invoice_input.value.trim().to_string());
                return self.perform_trade_action(invoice, super::mostro::submit_invoice);
            }
            P2PMessage::ConfirmFiatSent => {
                return self.perform_trade_action(None, super::mostro::confirm_fiat_sent);
            }
            P2PMessage::ConfirmFiatReceived => {
                return self.perform_trade_action(None, super::mostro::confirm_fiat_received);
            }
            P2PMessage::RatingSelected(rating) => {
                self.trade_rating = rating;
            }
            P2PMessage::SubmitRating => {
                let rating = self.trade_rating;
                if rating == 0 {
                    return Task::none();
                }
                let Some(ref order_id) = self.selected_trade else {
                    return Task::none();
                };
                self.trade_action_loading = true;
                let data = super::mostro::TradeActionData {
                    order_id: order_id.clone(),
                    cube_name: self
                        .wallet
                        .as_ref()
                        .map(|w| w.name.clone())
                        .unwrap_or_else(|| "default".to_string()),
                    mnemonic: self.mnemonic.clone(),
                    invoice: None,
                    mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                    relay_urls: self.mostro_config.relays.clone(),
                };
                return Task::perform(super::mostro::rate_counterparty(data, rating), |result| {
                    Message::View(view::Message::P2P(P2PMessage::TradeActionResult(result)))
                });
            }
            P2PMessage::CancelTrade => {
                return self.perform_trade_action(None, super::mostro::cancel_trade);
            }
            P2PMessage::OpenDispute => {
                return self.perform_trade_action(None, super::mostro::open_dispute);
            }
            P2PMessage::TradeActionResult(result) => {
                self.trade_action_loading = false;
                self.trade_rating = 0;
                match result {
                    Ok(resp) => {
                        let super::mostro::TradeActionResponse::Success { new_status } = resp;
                        tracing::info!("Trade action succeeded: {}", new_status);

                        // Update last_dm_action, timestamp, and status on the matching in-memory trade
                        if let Some(ref order_id) = self.selected_trade {
                            if let Some(trade) = self.trades.iter_mut().find(|t| t.id == *order_id)
                            {
                                let ts = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                trade.last_dm_action = Some(new_status.clone());
                                if is_countdown_action(&new_status) {
                                    trade.countdown_start_ts = Some(ts);
                                } else {
                                    trade.countdown_start_ts = None;
                                }
                                if let Some(s) = super::mostro::dm_action_to_status(&new_status) {
                                    trade.status = s;
                                }
                            }
                            let cube_name = self
                                .wallet
                                .as_ref()
                                .map(|w| w.name.clone())
                                .unwrap_or_else(|| "default".to_string());
                            super::mostro::append_trade_message(
                                &cube_name,
                                order_id,
                                super::mostro::TradeMessage {
                                    timestamp: chrono::Utc::now().timestamp() as u64,
                                    action: new_status.clone(),
                                    payload_json: String::new(),
                                    is_own: false,
                                },
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
            // Chat
            P2PMessage::OpenChat => {
                self.show_chat = true;
                self.chat_input = Default::default();
            }
            P2PMessage::CloseChat => {
                self.show_chat = false;
                self.chat_input = Default::default();
            }
            P2PMessage::ChatInputEdited(v) => {
                self.chat_input.value = v;
            }
            P2PMessage::ToggleChatTradeInfo => {
                self.chat_show_trade_info = !self.chat_show_trade_info;
                if self.chat_show_trade_info {
                    self.chat_show_user_info = false;
                }
            }
            P2PMessage::ToggleChatUserInfo => {
                self.chat_show_user_info = !self.chat_show_user_info;
                if self.chat_show_user_info {
                    self.chat_show_trade_info = false;
                }
            }
            P2PMessage::SendChatMessage => {
                let text = self.chat_input.value.trim().to_string();
                if text.is_empty() {
                    return Task::none();
                }
                if let Some(ref order_id) = self.selected_trade {
                    let cube_name = self
                        .wallet
                        .as_ref()
                        .map(|w| w.name.clone())
                        .unwrap_or_else(|| "default".to_string());

                    // Build payload but defer persisting until send succeeds
                    let payload = serde_json::to_string(&Some(
                        mostro_core::message::Payload::TextMessage(text.clone()),
                    ))
                    .unwrap_or_default();

                    self.pending_chat_message = Some(PendingChatMessage {
                        order_id: order_id.clone(),
                        cube_name: cube_name.clone(),
                        payload,
                        timestamp: chrono::Utc::now().timestamp() as u64,
                        original_text: text.clone(),
                    });

                    self.chat_input = Default::default();

                    // Fire async send
                    let data = super::mostro::TradeActionData {
                        order_id: order_id.clone(),
                        cube_name,
                        mnemonic: self.mnemonic.clone(),
                        invoice: Some(text),
                        mostro_pubkey_hex: self.mostro_config.active_pubkey_hex().to_string(),
                        relay_urls: self.mostro_config.relays.clone(),
                    };
                    return Task::perform(super::mostro::send_chat_message(data), |result| {
                        Message::View(view::Message::P2P(P2PMessage::ChatMessageSent(result)))
                    });
                }
            }
            P2PMessage::ChatMessageSent(result) => {
                if let Some(pending) = self.pending_chat_message.take() {
                    match result {
                        Ok(()) => {
                            super::mostro::append_trade_message(
                                &pending.cube_name,
                                &pending.order_id,
                                super::mostro::TradeMessage {
                                    timestamp: pending.timestamp,
                                    action: "SendDm".to_string(),
                                    payload_json: pending.payload,
                                    is_own: true,
                                },
                            );
                        }
                        Err(e) => {
                            // Restore input so the user can retry
                            self.chat_input.value = pending.original_text;
                            return Task::done(Message::View(view::Message::ShowError(format!(
                                "Chat send failed: {}",
                                e
                            ))));
                        }
                    }
                }
            }
            // Timer tick — no-op; re-render happens automatically
            P2PMessage::TradeTimerTick => {}
            // Real-time DM updates
            P2PMessage::TradeUpdate {
                order_id,
                action,
                payload_json,
            } => {
                tracing::info!("Trade update for {}: {}", order_id, action);

                let is_chat = action == "SendDm";

                // Update last_dm_action and status on the matching in-memory trade
                // (skip for chat messages — they don't change trade status)
                if !is_chat {
                    let now_ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if let Some(trade) = self.trades.iter_mut().find(|t| t.id == order_id) {
                        trade.last_dm_action = Some(action.clone());
                        if is_countdown_action(&action) {
                            trade.countdown_start_ts = Some(now_ts);
                        } else {
                            trade.countdown_start_ts = None;
                        }
                        if let Some(new_status) = super::mostro::dm_action_to_status(&action) {
                            trade.status = new_status;
                        }
                    }
                }

                // Chat messages are already persisted by process_dm_notifications
                if is_chat {
                    return Task::none();
                }

                // Persist non-chat protocol messages to disk
                let cube_name = self
                    .wallet
                    .as_ref()
                    .map(|w| w.name.clone())
                    .unwrap_or_else(|| "default".to_string());
                super::mostro::append_trade_message(
                    &cube_name,
                    &order_id,
                    super::mostro::TradeMessage {
                        timestamp: chrono::Utc::now().timestamp() as u64,
                        action: action.clone(),
                        payload_json,
                        is_own: false,
                    },
                );

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
