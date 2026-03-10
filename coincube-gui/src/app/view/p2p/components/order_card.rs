use coincube_ui::{
    component::{button, card, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{column, container, row, Space},
    Alignment, Length,
};

use crate::app::view::{self, message::P2PMessage};

/// Format a u64 with thousand separators (e.g. 1234567 → "1,234,567").
fn format_sats(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

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
        if let Some(premium) = self.premium_percent {
            if premium == 0.0 {
                "(0%)".to_string()
            } else if premium > 0.0 {
                format!("(+{}%)", premium)
            } else {
                format!("({}%)", premium)
            }
        } else {
            "(0%)".to_string()
        }
    }

    pub fn order_type_label(&self) -> &'static str {
        match self.order_type {
            OrderType::Buy => "BUYING",
            OrderType::Sell => "SELLING",
        }
    }
}

pub fn order_card<'a>(order: &'a P2POrder) -> Button<'a, view::Message> {
    let badge_style = match order.order_type {
        OrderType::Buy => theme::pill::success as fn(&_) -> _,
        OrderType::Sell => theme::pill::warning as fn(&_) -> _,
    };

    // Colored premium text
    let premium_style = match order.premium_percent {
        Some(p) if p > 0.0 => theme::text::success as fn(&_) -> _,
        Some(p) if p < 0.0 => theme::text::warning as fn(&_) -> _,
        _ => theme::text::secondary as fn(&_) -> _,
    };

    let pill_label = if order.is_mine {
        match order.order_type {
            OrderType::Buy => "YOU ARE BUYING",
            OrderType::Sell => "YOU ARE SELLING",
        }
    } else {
        order.order_type_label()
    };

    let content = card::simple(
        column![
            // Header: Order type badge and timestamp
            row!(
                container(p2_regular(pill_label))
                    .padding([4, 12])
                    .style(badge_style),
                Space::new().width(Length::Fill),
                p2_regular(&order.time_ago).style(theme::text::secondary)
            )
            .spacing(10)
            .align_y(iced::alignment::Vertical::Center),
            // Amount and currency — prominent h1
            if order.is_range_order() {
                row!(
                    h1(format!(
                        "{:.0} - {:.0}",
                        order.min_amount.unwrap_or(0.0),
                        order.max_amount.unwrap_or(0.0)
                    )),
                    h3(format!(" {}", order.fiat_currency)).style(theme::text::secondary)
                )
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
            } else {
                row!(
                    h1(format!("{:.2}", order.fiat_amount)),
                    h3(format!(" {}", order.fiat_currency)).style(theme::text::secondary)
                )
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
            },
            // Sats amount or market price
            if order.is_fixed_price() {
                row![
                    p2_regular("for").style(theme::text::secondary),
                    p2_bold(format!(
                        "{} sats",
                        format_sats(order.sats_amount.unwrap_or(0))
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
            // Payment methods with icon
            container(
                row![
                    icon::cash_icon().style(theme::text::secondary),
                    p2_regular(order.payment_methods.join(", "))
                ]
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
            )
            .padding(12)
            .width(Length::Fill)
            .style(theme::container::background),
            // Rating with icons
            if let (Some(rating), Some(reviews), Some(days)) = (
                order.seller_rating,
                order.seller_reviews,
                order.seller_days_old
            ) {
                row![
                    row![
                        icon::star_fill_icon().style(theme::text::secondary),
                        p2_bold(format!("{:.1}", rating)),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center),
                    row![
                        icon::person_icon().style(theme::text::secondary),
                        p2_regular(format!("{}", reviews)).style(theme::text::secondary),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center),
                    row![
                        icon::calendar_icon().style(theme::text::secondary),
                        p2_regular(format!("{}", days)).style(theme::text::secondary),
                    ]
                    .spacing(4)
                    .align_y(iced::alignment::Vertical::Center),
                ]
                .spacing(16)
            } else {
                row![p2_regular("No rating").style(theme::text::secondary)].spacing(8)
            },
        ]
        .spacing(12),
    )
    .width(Length::Fill);

    Button::new(content)
        .style(theme::button::container)
        .on_press(view::Message::P2P(P2PMessage::SelectOrder(
            order.id.clone(),
        )))
        .width(Length::Fill)
}

/// Detail view for a selected order.
pub fn order_detail<'a>(order: &'a P2POrder) -> Container<'a, view::Message> {
    let badge_style = match order.order_type {
        OrderType::Buy => theme::pill::success as fn(&_) -> _,
        OrderType::Sell => theme::pill::warning as fn(&_) -> _,
    };

    let heading = if order.is_mine {
        match order.order_type {
            OrderType::Buy => "You are buying",
            OrderType::Sell => "You are selling",
        }
    } else {
        match order.order_type {
            OrderType::Buy => "Someone is buying",
            OrderType::Sell => "Someone is selling",
        }
    };

    // Colored premium
    let premium_style = match order.premium_percent {
        Some(p) if p > 0.0 => theme::text::success as fn(&_) -> _,
        Some(p) if p < 0.0 => theme::text::warning as fn(&_) -> _,
        _ => theme::text::secondary as fn(&_) -> _,
    };

    // Amount section
    let amount_card = card::simple(
        column![
            container(p2_regular(order.order_type_label()))
                .padding([4, 12])
                .style(badge_style),
            p2_regular(heading).style(theme::text::secondary),
            if order.is_range_order() {
                row!(
                    h1(format!(
                        "{:.0} - {:.0}",
                        order.min_amount.unwrap_or(0.0),
                        order.max_amount.unwrap_or(0.0)
                    )),
                    h3(format!(" {}", order.fiat_currency)).style(theme::text::secondary)
                )
                .spacing(8)
                .align_y(iced::alignment::Vertical::Center)
            } else {
                row!(
                    h1(format!("{:.2}", order.fiat_amount)),
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
                        format_sats(order.sats_amount.unwrap_or(0))
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
    .width(Length::Fill);

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
    let time_left = if expires_at > now {
        let remaining = expires_at - now;
        let hours = remaining / 3600;
        let minutes = (remaining % 3600) / 60;
        if hours > 0 {
            format!("{}h {}m left", hours, minutes)
        } else {
            format!("{}m left", minutes)
        }
    } else {
        "Expired".to_string()
    };

    let time_card = card::simple(
        row![
            icon::clock_icon().style(theme::text::secondary),
            column![
                p2_regular("Created").style(theme::text::secondary),
                p1_bold(&order.time_ago),
                p2_regular(time_left).style(theme::text::secondary),
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
                    button::secondary_compact(None, "Copy").on_press(view::Message::P2P(
                        P2PMessage::CopyOrderId(order.id.clone(),)
                    )),
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

        let close_btn = button::secondary(None, "Close")
            .on_press(view::Message::P2P(P2PMessage::CloseOrderDetail))
            .width(Length::Fill);
        let cancel_btn = button::alert(None, "Cancel Order")
            .on_press(view::Message::P2P(P2PMessage::CancelOrder(
                order.id.clone(),
            )))
            .width(Length::Fill);
        detail_col = detail_col.push(row![close_btn, cancel_btn].spacing(8));
    } else {
        let close_btn = button::secondary(None, "Close")
            .on_press(view::Message::P2P(P2PMessage::CloseOrderDetail))
            .width(Length::Fill);
        let action_label = match order.order_type {
            OrderType::Buy => "Sell",
            OrderType::Sell => "Buy",
        };
        let action_btn = button::primary(None, action_label)
            .on_press(view::Message::P2P(P2PMessage::TakeOrder))
            .width(Length::Fill);
        detail_col = detail_col.push(row![close_btn, action_btn].spacing(8));
    }

    container(detail_col).width(Length::Fill)
}
