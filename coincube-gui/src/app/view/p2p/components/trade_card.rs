use coincube_ui::{
    component::{card, text::*},
    theme,
    widget::*,
};
use iced::{
    widget::{column, container, row, Space},
    Length,
};

use super::order_card::OrderType;
use crate::app::view::{self, message::P2PMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeStatus {
    Pending,
    Active,
    WaitingPayment,
    WaitingBuyerInvoice,
    FiatSent,
    SettledHoldInvoice,
    PaymentFailed,
    Success,
    Canceled,
    CooperativelyCanceled,
    Dispute,
    Expired,
}

impl TradeStatus {
    pub fn label(&self) -> &'static str {
        match self {
            TradeStatus::Pending => "Pending",
            TradeStatus::Active => "Active",
            TradeStatus::WaitingPayment => "Waiting Payment",
            TradeStatus::WaitingBuyerInvoice => "Waiting Invoice",
            TradeStatus::FiatSent => "Fiat Sent",
            TradeStatus::SettledHoldInvoice => "Paying Sats",
            TradeStatus::PaymentFailed => "Payment Failed",
            TradeStatus::Success => "Success",
            TradeStatus::Canceled => "Canceled",
            TradeStatus::CooperativelyCanceled => "Canceling",
            TradeStatus::Dispute => "Dispute",
            TradeStatus::Expired => "Expired",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TradeStatus::Success
                | TradeStatus::Canceled
                | TradeStatus::CooperativelyCanceled
                | TradeStatus::Expired
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradeRole {
    Creator,
    Taker,
}

impl TradeRole {
    pub fn label(&self) -> &'static str {
        match self {
            TradeRole::Creator => "Created by You",
            TradeRole::Taker => "Taken by You",
        }
    }
}

#[derive(Debug, Clone)]
pub struct P2PTrade {
    pub id: String,
    /// Buy or Sell from our perspective
    pub order_type: OrderType,
    pub status: TradeStatus,
    /// Whether we created or took this order
    pub role: TradeRole,
    pub fiat_amount: f64,
    pub fiat_currency: String,
    pub sats_amount: Option<u64>,
    pub premium_percent: Option<f64>,
    pub payment_method: String,
    pub counterparty_rating: Option<f32>,
    pub created_at_ts: i64,
    pub created_at: String,
    pub time_ago: String,
    /// Latest DM action from Mostro (e.g. "PayInvoice", "FiatSentOk")
    pub last_dm_action: Option<String>,
    /// Timestamp when the current countdown phase started (PayInvoice, AddInvoice, etc.)
    pub countdown_start_ts: Option<u64>,
    /// Counterparty's trade public key (hex)
    pub counterparty_pubkey: Option<String>,
    /// Admin/solver's trade public key (hex), set when admin takes dispute
    pub admin_pubkey: Option<String>,
}

impl P2PTrade {
    pub fn is_fixed_price(&self) -> bool {
        self.sats_amount.is_some() && self.sats_amount != Some(0)
    }

    pub fn premium_text(&self) -> String {
        super::format_premium(self.premium_percent)
    }

    pub fn order_type_label(&self) -> &'static str {
        match self.order_type {
            OrderType::Buy => "BUY",
            OrderType::Sell => "SELL",
        }
    }
}

pub fn trade_card<'a>(trade: &'a P2PTrade) -> Button<'a, view::Message> {
    let type_badge_style = match trade.order_type {
        OrderType::Buy => theme::pill::success as fn(&_) -> _,
        OrderType::Sell => theme::pill::warning as fn(&_) -> _,
    };

    let status_badge_style = match trade.status {
        TradeStatus::Active | TradeStatus::FiatSent | TradeStatus::SettledHoldInvoice => {
            theme::pill::success as fn(&_) -> _
        }
        TradeStatus::Success => theme::pill::primary as fn(&_) -> _,
        TradeStatus::Pending | TradeStatus::WaitingPayment | TradeStatus::WaitingBuyerInvoice => {
            theme::pill::simple as fn(&_) -> _
        }
        TradeStatus::PaymentFailed
        | TradeStatus::Canceled
        | TradeStatus::CooperativelyCanceled
        | TradeStatus::Dispute
        | TradeStatus::Expired => theme::pill::warning as fn(&_) -> _,
    };

    let content = card::simple(
        column![
            // Header: Order type badge, status badge, and timestamp
            row!(
                container(p2_regular(trade.order_type_label()))
                    .padding([4, 12])
                    .style(type_badge_style),
                container(p2_regular(trade.status.label()))
                    .padding([4, 12])
                    .style(status_badge_style),
                Space::new().width(Length::Fill),
                p2_regular(&trade.time_ago).style(theme::text::secondary)
            )
            .spacing(10)
            .align_y(iced::alignment::Vertical::Center),
            // Amount and currency
            row!(
                h2(format!("{:.2}", trade.fiat_amount)),
                p1_bold(format!(" {}", trade.fiat_currency)).style(theme::text::secondary)
            )
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
            // Sats amount or market price
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
            // Payment method
            container(p2_regular(&trade.payment_method))
                .padding(12)
                .width(Length::Fill)
                .style(theme::container::background),
            // Role and rating
            row![
                container(p2_regular(trade.role.label()))
                    .padding([4, 12])
                    .style(theme::pill::simple as fn(&_) -> _),
                Space::new().width(Length::Fill),
                if let Some(rating) = trade.counterparty_rating {
                    p2_regular(format!("Rating: {:.1}", rating))
                } else {
                    p2_regular("No rating").style(theme::text::secondary)
                }
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
        ]
        .spacing(12),
    )
    .width(Length::Fill);

    Button::new(content)
        .style(theme::button::container)
        .on_press(view::Message::P2P(P2PMessage::SelectTrade(
            trade.id.clone(),
        )))
        .width(Length::Fill)
}
