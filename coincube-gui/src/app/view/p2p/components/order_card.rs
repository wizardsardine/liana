use coincube_ui::locale::copy as localized_copy;
use coincube_ui::widget::action::Action;
use coincube_ui::{
    component::{card, form, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    advanced::widget::Id,
    widget::{column, container, row, text::Wrapping, Space},
    Alignment, Length,
};

use crate::app::view::{self, message::P2PMessage};

use coincube_ui::component::amount::format_f64_as_string;

use super::format_with_separators;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrderType {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PricingMode {
    Market,
    Fixed,
}

#[derive(Debug, Clone)]
pub struct P2POrder {
    pub id: String,
    pub order_type: OrderType,
    pub fiat_amount: f64,
    pub fiat_currency: String,
    pub min_amount: Option<f64>,
    pub max_amount: Option<f64>,
    pub sats_amount: Option<u64>,
    pub premium_percent: Option<f64>,
    pub payment_methods: Vec<String>,
    pub seller_rating: Option<f32>,
    pub seller_reviews: Option<u32>,
    pub seller_days_old: Option<u32>,
    pub created_at: String,
    pub created_at_ts: u64,
    pub time_ago: String,
    pub is_mine: bool,
}

impl P2POrder {
    pub fn is_range_order(&self) -> bool {
        self.min_amount.is_some() && self.max_amount.is_some()
    }

    pub fn is_fixed_price(&self) -> bool {
        self.sats_amount.is_some() && self.sats_amount != Some(0)
    }

    pub fn premium_text(&self) -> String {
        super::format_premium(self.premium_percent)
    }

    pub fn order_type_label(&self) -> &'static str {
        match self.order_type {
            OrderType::Buy => "BUYING",
            OrderType::Sell => "SELLING",
        }
    }
}

/// Columns shared by the heading and every order button at wide widths.
fn book_columns<'a>(
    side: impl Into<Element<'a, view::Message>>,
    amount: impl Into<Element<'a, view::Message>>,
    payment: impl Into<Element<'a, view::Message>>,
    price: impl Into<Element<'a, view::Message>>,
    reputation: impl Into<Element<'a, view::Message>>,
    trailing: impl Into<Element<'a, view::Message>>,
) -> Row<'a, view::Message> {
    row![
        container(side).width(Length::Fixed(120.0)),
        container(amount).width(Length::Fixed(145.0)),
        container(payment).width(Length::Fill).clip(true),
        container(price).width(Length::Fixed(125.0)),
        container(reputation).width(Length::Fixed(140.0)),
        container(trailing).width(Length::Fixed(16.0)),
    ]
    .spacing(8)
    .align_y(iced::alignment::Vertical::Center)
    .width(Length::Fill)
}

pub fn order_table_header<'a>() -> Element<'a, view::Message> {
    container(book_columns(
        caption(localized_copy("Side")).style(theme::text::secondary),
        caption(localized_copy("Amount")).style(theme::text::secondary),
        caption(localized_copy("Payment Method"))
            .wrapping(Wrapping::None)
            .style(theme::text::secondary),
        caption(localized_copy("Market Price")).style(theme::text::secondary),
        caption(localized_copy("Creator Reputation")).style(theme::text::secondary),
        caption(""),
    ))
    .padding([8, 12])
    .width(Length::Fill)
    .into()
}

fn book_amount(order: &P2POrder) -> String {
    let amount = match (order.min_amount, order.max_amount) {
        (Some(min), Some(max)) => format!(
            "{} – {}",
            format_f64_as_string(min, ",", 0, false),
            format_f64_as_string(max, ",", 0, false)
        ),
        _ => format_f64_as_string(order.fiat_amount, ",", 2, false),
    };
    format!("{} {}", amount, order.fiat_currency)
}

fn book_price(order: &P2POrder) -> String {
    if let Some(sats) = order.sats_amount.filter(|sats| *sats > 0) {
        format!("Fixed · {} sats", format_with_separators(sats))
    } else {
        match order.premium_percent {
            Some(premium) if premium > 0.0 => format!("Market +{}%", premium),
            Some(premium) if premium < 0.0 => format!("Market {}%", premium),
            _ => "Market".to_string(),
        }
    }
}

fn book_reputation(order: &P2POrder) -> String {
    match (order.seller_rating, order.seller_days_old) {
        (Some(rating), Some(days)) => format!("★ {:.1} · {} days", rating, days),
        (Some(rating), None) => format!("★ {:.1}", rating),
        (None, Some(days)) => format!("No rating · {} days", days),
        (None, None) => "No rating".to_string(),
    }
}

fn book_side_label(order: &P2POrder) -> &'static str {
    match (&order.order_type, order.is_mine) {
        (OrderType::Buy, true) => localized_copy("YOU ARE BUYING"),
        (OrderType::Sell, true) => localized_copy("YOU ARE SELLING"),
        (OrderType::Sell, false) => "BUYING",
        (OrderType::Buy, false) => "SELLING",
    }
}

pub fn order_row_id(id: &str) -> Id {
    Id::from(format!("p2p-book-order-{id}"))
}

pub fn detail_action_id(id: &str, action: &str) -> Id {
    Id::from(format!("p2p-book-detail-{id}-{action}"))
}

pub fn detail_input_id(id: &str, field: &str) -> coincube_ui::widget::text_input::Id {
    coincube_ui::widget::text_input::Id::new(&format!("p2p-book-detail-{id}-input-{field}"))
}

pub fn detail_primary_id(order: &P2POrder) -> Id {
    detail_action_id(&order.id, if order.is_mine { "close" } else { "take" })
}

fn selling_style(theme: &coincube_ui::theme::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme.colors.shell.accent_ink),
    }
}

fn book_row_style(
    theme: &coincube_ui::theme::Theme,
    status: iced::widget::button::Status,
) -> iced::widget::button::Style {
    let mut style = theme::button::transparent(theme, status);
    if matches!(status, iced::widget::button::Status::Hovered) {
        style.background = Some(iced::Background::Color(theme.colors.shell.hover));
    }
    style
}

/// A single keyboard-actionable order entry. External orders describe the
/// taker's action; the user's own orders describe their maker action.
/// `order_type` remains the maker's side for selection and trading.
pub fn order_card<'a>(order: &'a P2POrder, compact: bool) -> Element<'a, view::Message> {
    let side = book_side_label(order);
    let side_style = match (&order.order_type, order.is_mine) {
        (OrderType::Sell, false) | (OrderType::Buy, true) => theme::text::success as fn(&_) -> _,
        (OrderType::Buy, false) | (OrderType::Sell, true) => selling_style as fn(&_) -> _,
    };
    let payment = if order.payment_methods.is_empty() {
        "Unavailable".to_string()
    } else {
        order.payment_methods.join(", ")
    };
    let amount = book_amount(order);
    let price = book_price(order);
    let reputation = book_reputation(order);
    let content: Element<'a, view::Message> = if compact {
        column![
            row![
                p2_bold(side).style(side_style),
                Space::new().width(Length::Fill),
                p2_bold(amount).style(theme::text::primary),
                p2_regular("›").style(theme::text::secondary),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
            p2_regular(payment).style(theme::text::primary),
            row![
                p2_regular(price).style(theme::text::primary),
                Space::new().width(Length::Fill),
                p2_regular(reputation).style(theme::text::secondary),
            ]
            .spacing(8),
        ]
        .spacing(6)
        .into()
    } else {
        book_columns(
            p2_bold(side).size(13).style(side_style),
            p2_bold(amount).size(13).style(theme::text::primary),
            p2_regular(payment)
                .size(13)
                .wrapping(Wrapping::None)
                .style(theme::text::primary),
            p2_regular(price).size(13).style(theme::text::primary),
            p2_regular(reputation)
                .size(13)
                .style(theme::text::secondary),
            p2_regular("›").style(theme::text::secondary),
        )
        .into()
    };

    Action::new(
        order_row_id(&order.id),
        container(content).padding([10, 12]).width(Length::Fill),
        view::Message::FocusAction,
    )
    .style(book_row_style)
    .padding(0)
    .focus_radius(0.0)
    .on_press(view::Message::P2P(P2PMessage::SelectOrder(
        order.id.clone(),
    )))
    .width(Length::Fill)
    .into()
}

/// Detail view for a selected order.
/// State passed into order_detail for inline take-order fields.
pub struct TakeOrderState<'a> {
    pub amount: &'a form::Value<String>,
    pub invoice: &'a form::Value<String>,
    pub submitting: bool,
}

fn detail_button<'a>(
    order_id: &str,
    control: &'static str,
    label: &'static str,
    style: fn(
        &coincube_ui::theme::Theme,
        iced::widget::button::Status,
    ) -> iced::widget::button::Style,
    width: Length,
    on_press: Option<view::Message>,
) -> Element<'a, view::Message> {
    Action::new(
        detail_action_id(order_id, control),
        container(p2_regular(label))
            .width(width)
            .align_x(iced::alignment::Horizontal::Center),
        view::Message::FocusAction,
    )
    .style(style)
    .width(width)
    .padding([8, 12])
    .on_press_maybe(on_press)
    .into()
}

pub fn order_detail<'a>(
    order: &'a P2POrder,
    take_state: Option<TakeOrderState<'a>>,
) -> Container<'a, view::Message> {
    let amount_card = order_amount_card(order);

    // Payment method section with icon
    let payment_card = card::simple(
        row![
            icon::cash_icon().style(theme::text::secondary),
            column![
                p2_regular("Payment Method").style(theme::text::secondary),
                p1_bold(order.payment_methods.join(", ")),
            ]
            .spacing(4),
        ]
        .spacing(12)
        .align_y(iced::alignment::Vertical::Center),
    )
    .width(Length::Fill);

    // Created time with icon + time remaining
    let expires_at = order.created_at_ts + 86400; // 24 hours
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let created_date = {
        let dt = chrono::DateTime::from_timestamp(order.created_at_ts as i64, 0)
            .unwrap_or_default()
            .with_timezone(&chrono::Local);
        dt.format(&coincube_ui::locale::date_pattern("%b %d, %Y %H:%M", &dt))
            .to_string()
    };

    let time_card = card::simple(
        row![
            icon::clock_icon().style(theme::text::secondary),
            column![
                p2_regular("Created On").style(theme::text::secondary),
                p1_bold(format!("Created on: {}", created_date)),
            ]
            .spacing(4),
        ]
        .spacing(12)
        .align_y(iced::alignment::Vertical::Center),
    )
    .width(Length::Fill);

    // Order ID with icon and copy button
    let order_id_card = card::simple(
        row![
            icon::clipboard_icon().style(theme::text::secondary),
            column![
                p2_regular("Order ID").style(theme::text::secondary),
                row![
                    p2_regular(&order.id).style(theme::text::secondary),
                    Space::new().width(Length::Fill),
                    detail_button(
                        &order.id,
                        "copy",
                        "Copy",
                        theme::button::secondary,
                        Length::Shrink,
                        Some(view::Message::P2P(P2PMessage::CopyOrderId(
                            order.id.clone()
                        ))),
                    ),
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

    // Creator reputation — three-column centered layout
    let reputation_card = card::simple(
        column![
            p2_regular("Creator Reputation").style(theme::text::secondary),
            if let (Some(rating), Some(reviews), Some(days)) = (
                order.seller_rating,
                order.seller_reviews,
                order.seller_days_old
            ) {
                row![
                    column![
                        row![
                            icon::star_fill_icon().style(theme::text::secondary),
                            p1_bold(format!("{:.1}", rating)),
                        ]
                        .spacing(4)
                        .align_y(iced::alignment::Vertical::Center),
                        caption("Rating").style(theme::text::secondary),
                    ]
                    .spacing(4)
                    .align_x(Alignment::Center)
                    .width(Length::Fill),
                    column![
                        row![
                            icon::person_icon().style(theme::text::secondary),
                            p1_bold(format!("{}", reviews)),
                        ]
                        .spacing(4)
                        .align_y(iced::alignment::Vertical::Center),
                        caption("Reviews").style(theme::text::secondary),
                    ]
                    .spacing(4)
                    .align_x(Alignment::Center)
                    .width(Length::Fill),
                    column![
                        row![
                            icon::calendar_icon().style(theme::text::secondary),
                            p1_bold(format!("{}", days)),
                        ]
                        .spacing(4)
                        .align_y(iced::alignment::Vertical::Center),
                        caption("Days").style(theme::text::secondary),
                    ]
                    .spacing(4)
                    .align_x(Alignment::Center)
                    .width(Length::Fill),
                ]
                .spacing(12)
            } else {
                row![p2_regular("No rating available").style(theme::text::secondary)]
            },
        ]
        .spacing(12),
    )
    .width(Length::Fill);

    // Action buttons + info note
    let mut detail_col = column![
        amount_card,
        payment_card,
        time_card,
        order_id_card,
        reputation_card,
    ]
    .spacing(12);

    // Countdown timer for pending orders (24h expiry)
    if expires_at > now {
        let remaining = expires_at - now;
        let hours = remaining / 3600;
        let minutes = (remaining % 3600) / 60;
        let seconds = remaining % 60;
        detail_col = detail_col.push(
            container(
                column![
                    icon::clock_icon().size(24).style(theme::text::secondary),
                    p1_bold(format!("{:02}:{:02}:{:02}", hours, minutes, seconds)),
                    caption("Time remaining").style(theme::text::secondary),
                ]
                .spacing(4)
                .align_x(Alignment::Center),
            )
            .width(Length::Fill)
            .center_x(Length::Fill)
            .padding([12, 0]),
        );
    }

    if order.is_mine {
        detail_col = detail_col.push(
            column![
                p2_regular("This order will be published for 24 hours.")
                    .style(theme::text::secondary),
                p2_regular("You can cancel it anytime before someone takes it.")
                    .style(theme::text::secondary),
            ]
            .spacing(4),
        );

        let close_btn = detail_button(
            &order.id,
            "close",
            "Close",
            theme::button::secondary,
            Length::Fill,
            Some(view::Message::P2P(P2PMessage::CloseOrderDetail)),
        );
        let cancel_btn = detail_button(
            &order.id,
            "cancel",
            "Cancel Order",
            theme::button::destructive,
            Length::Fill,
            Some(view::Message::P2P(P2PMessage::CancelOrder(
                order.id.clone(),
            ))),
        );
        detail_col = detail_col.push(row![close_btn, cancel_btn].spacing(8));
    } else {
        // Is user buying? (taking a sell order)
        let is_buying = order.order_type == OrderType::Sell;

        if let Some(state) = take_state {
            detail_col = append_take_order_form(detail_col, order, state, is_buying);
        } else {
            // Initial state: show Buy/Sell button that activates take order fields
            let close_btn = detail_button(
                &order.id,
                "close",
                "Close",
                theme::button::secondary,
                Length::Fill,
                Some(view::Message::P2P(P2PMessage::CloseOrderDetail)),
            );
            let action_label = if is_buying { "Buy" } else { "Sell" };
            let action_btn = detail_button(
                &order.id,
                "take",
                action_label,
                theme::button::primary,
                Length::Fill,
                Some(view::Message::P2P(P2PMessage::TakeOrder)),
            );
            detail_col = detail_col.push(row![close_btn, action_btn].spacing(8));
        }
    }

    container(detail_col).width(Length::Fill)
}

fn order_amount_card<'a>(order: &'a P2POrder) -> Container<'a, view::Message> {
    let badge_style = match order.order_type {
        OrderType::Buy => theme::pill::success as fn(&_) -> _,
        OrderType::Sell => theme::pill::warning as fn(&_) -> _,
    };

    let heading = if order.is_mine {
        match order.order_type {
            OrderType::Buy => localized_copy("You are buying"),
            OrderType::Sell => localized_copy("You are selling"),
        }
    } else {
        match order.order_type {
            OrderType::Buy => localized_copy("Someone is buying"),
            OrderType::Sell => localized_copy("Someone is selling"),
        }
    };

    // Colored premium
    let premium_style = match order.premium_percent {
        Some(p) if p > 0.0 => theme::text::success as fn(&_) -> _,
        Some(p) if p < 0.0 => theme::text::warning as fn(&_) -> _,
        _ => theme::text::secondary as fn(&_) -> _,
    };

    // Amount section
    card::simple(
        column![
            container(p2_regular(order.order_type_label()))
                .padding([4, 12])
                .style(badge_style),
            p2_regular(heading).style(theme::text::secondary),
            if order.is_range_order() {
                row!(
                    h1(format!(
                        "{} - {}",
                        format_f64_as_string(order.min_amount.unwrap_or(0.0), ",", 0, false),
                        format_f64_as_string(order.max_amount.unwrap_or(0.0), ",", 0, false)
                    ))
                    .style(theme::text::primary),
                    h3(format!(" {}", order.fiat_currency)).style(theme::text::secondary)
                )
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
            } else {
                row!(
                    h1(format_f64_as_string(order.fiat_amount, ",", 2, false))
                        .style(theme::text::primary),
                    h3(format!(" {}", order.fiat_currency)).style(theme::text::secondary)
                )
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
            },
            if order.is_fixed_price() {
                row![
                    p2_regular("for").style(theme::text::secondary),
                    p2_bold(format!(
                        "{} sats",
                        format_with_separators(order.sats_amount.unwrap_or(0))
                    ))
                ]
                .spacing(8)
            } else {
                row![
                    p2_regular("Market Price").style(theme::text::secondary),
                    p2_bold(order.premium_text()).style(premium_style)
                ]
                .spacing(8)
            },
        ]
        .spacing(8),
    )
    .width(Length::Fill)
}

fn append_take_order_form<'a>(
    mut detail_col: Column<'a, view::Message>,
    order: &'a P2POrder,
    state: TakeOrderState<'a>,
    is_buying: bool,
) -> Column<'a, view::Message> {
    // Inline take order fields
    if order.is_range_order() {
        let prior_amount = state.amount.value.clone();
        detail_col = detail_col.push(
            column![
                p2_regular(format!(
                    "Enter fiat amount ({} - {} {})",
                    format_f64_as_string(order.min_amount.unwrap_or(0.0), ",", 0, false),
                    format_f64_as_string(order.max_amount.unwrap_or(0.0), ",", 0, false),
                    order.fiat_currency
                ))
                .style(theme::text::secondary),
                coincube_ui::widget::text_input::TextInput::new(
                    "Fiat amount",
                    &state.amount.value,
                )
                .id(detail_input_id(&order.id, "amount"))
                .on_input(move |value| {
                    // Preserve the Form's non-negative decimal input rule.
                    let trimmed = value.trim_end_matches('.');
                    let valid = value.is_empty()
                        || (!value.starts_with('-')
                            && (trimmed.is_empty() || trimmed.parse::<f64>().is_ok())
                            && value.chars().all(|c| c.is_ascii_digit() || c == '.')
                            && value.matches('.').count() <= 1);
                    view::Message::P2P(P2PMessage::TakeOrderAmountEdited(if valid {
                        value
                    } else {
                        prior_amount.clone()
                    }))
                })
                .padding(10),
            ]
            .spacing(8),
        );
    }

    if is_buying {
        detail_col = detail_col.push(
            column![
                p2_regular("Lightning invoice or address (optional)").style(theme::text::secondary),
                coincube_ui::widget::text_input::TextInput::new(
                    "Enter lightning invoice or address",
                    &state.invoice.value,
                )
                .id(detail_input_id(&order.id, "invoice"))
                .on_input(|value| {
                    view::Message::P2P(P2PMessage::TakeOrderInvoiceEdited(value.trim().to_string()))
                })
                .padding(10),
            ]
            .spacing(8),
        );
    }

    let can_confirm = if order.is_range_order() {
        state
            .amount
            .value
            .parse::<f64>()
            .map(|v| {
                let min = order.min_amount.unwrap_or(0.0);
                let max = order.max_amount.unwrap_or(f64::MAX);
                v >= min && v <= max
            })
            .unwrap_or(false)
    } else {
        true
    };

    let action_label = if is_buying { "Buy" } else { "Sell" };
    let confirm_btn = detail_button(
        &order.id,
        "confirm",
        action_label,
        theme::button::primary,
        Length::Fill,
        (can_confirm && !state.submitting)
            .then_some(view::Message::P2P(P2PMessage::ConfirmTakeOrder)),
    );

    detail_col = detail_col.push(
        row![
            detail_button(
                &order.id,
                "close",
                "Close",
                theme::button::secondary,
                Length::Fill,
                Some(view::Message::P2P(P2PMessage::CloseOrderDetail)),
            ),
            confirm_btn,
        ]
        .spacing(8),
    );
    detail_col
}

#[cfg(test)]
mod book_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    fn order(order_type: OrderType) -> P2POrder {
        P2POrder {
            id: "order-1".into(),
            order_type,
            fiat_amount: 100.0,
            fiat_currency: "EUR".into(),
            min_amount: Some(50.0),
            max_amount: Some(200.0),
            sats_amount: None,
            premium_percent: Some(2.0),
            payment_methods: vec!["SEPA instant".into()],
            seller_rating: None,
            seller_reviews: None,
            seller_days_old: Some(3),
            created_at: String::new(),
            created_at_ts: 0,
            time_ago: String::new(),
            is_mine: false,
        }
    }

    #[test]
    fn book_labels_taker_side_and_only_displays_known_values() {
        let mut sell = order(OrderType::Sell);
        assert_eq!(book_side_label(&sell), "BUYING");
        assert_eq!(book_amount(&sell), "50 – 200 EUR");
        assert_eq!(book_price(&sell), "Market +2%");
        assert_eq!(book_reputation(&sell), "No rating · 3 days");

        let mut buy = order(OrderType::Buy);
        buy.sats_amount = Some(450_000);
        buy.seller_rating = Some(4.8);
        assert_eq!(book_side_label(&buy), "SELLING");
        assert_eq!(book_price(&buy), "Fixed · 450,000 sats");
        assert_eq!(book_reputation(&buy), "★ 4.8 · 3 days");

        buy.is_mine = true;
        assert_eq!(book_side_label(&buy), "YOU ARE BUYING");
        sell.is_mine = true;
        assert_eq!(book_side_label(&sell), "YOU ARE SELLING");
    }

    #[test]
    fn selected_order_detail_actions_support_keyboard_and_pointer() {
        let order = order(OrderType::Sell);
        let size = (1024, 768);
        let take = detail_primary_id(&order);
        assert_eq!(take, detail_action_id(&order.id, "take"));

        let enter = capture::activate(order_detail(&order, None).into(), take.clone(), size, false);
        assert!(matches!(
            enter.as_slice(),
            [view::Message::P2P(P2PMessage::TakeOrder)]
        ));
        let pointer =
            capture::activate(order_detail(&order, None).into(), take.clone(), size, true);
        assert!(matches!(
            pointer.as_slice(),
            [view::Message::FocusAction(id), view::Message::P2P(P2PMessage::TakeOrder)]
                if id == &take
        ));
        let close = capture::activate(
            order_detail(&order, None).into(),
            detail_action_id(&order.id, "close"),
            size,
            false,
        );
        assert!(matches!(
            close.as_slice(),
            [view::Message::P2P(P2PMessage::CloseOrderDetail)]
        ));
        let copy = capture::activate_with_key(
            order_detail(&order, None).into(),
            detail_action_id(&order.id, "copy"),
            size,
            false,
            iced::keyboard::key::Named::Space,
        );
        assert!(matches!(
            copy.as_slice(),
            [view::Message::P2P(P2PMessage::CopyOrderId(id))] if id == &order.id
        ));

        let mut direct = order.clone();
        direct.min_amount = None;
        direct.max_amount = None;
        let amount = form::Value::<String>::default();
        let invoice = form::Value::<String>::default();
        let range_state = || TakeOrderState {
            amount: &amount,
            invoice: &invoice,
            submitting: false,
        };
        for field in ["amount", "invoice"] {
            let messages = capture::activate(
                order_detail(&order, Some(range_state())).into(),
                detail_input_id(&order.id, field).into(),
                size,
                false,
            );
            assert!(
                messages.is_empty(),
                "focusing {field} should not submit",
                field = field
            );
        }
        let confirm = capture::activate(
            order_detail(&direct, Some(range_state())).into(),
            detail_action_id(&direct.id, "confirm"),
            size,
            false,
        );
        assert!(matches!(
            confirm.as_slice(),
            [view::Message::P2P(P2PMessage::ConfirmTakeOrder)]
        ));

        let mut mine = order;
        mine.is_mine = true;
        assert_eq!(
            detail_primary_id(&mine),
            detail_action_id(&mine.id, "close")
        );
        let cancel = capture::activate(
            order_detail(&mine, None).into(),
            detail_action_id(&mine.id, "cancel"),
            size,
            false,
        );
        assert!(matches!(
            cancel.as_slice(),
            [view::Message::P2P(P2PMessage::CancelOrder(id))] if id == &mine.id
        ));
    }
}
