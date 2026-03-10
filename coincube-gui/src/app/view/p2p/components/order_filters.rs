use coincube_ui::{component::text::*, theme, widget::*};
use iced::{
    widget::{button, column, container, row},
    Length,
};

use crate::app::view::{self, message::P2PMessage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuySellFilter {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeFilter {
    All,
    Pending,
    Active,
    WaitingPayment,
    WaitingInvoice,
    FiatSent,
    Success,
    Canceled,
    PayingSats,
    Dispute,
}

impl std::fmt::Display for TradeFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeFilter::All => write!(f, "All"),
            TradeFilter::Pending => write!(f, "Pending"),
            TradeFilter::Active => write!(f, "Active"),
            TradeFilter::WaitingPayment => write!(f, "Waiting Payment"),
            TradeFilter::WaitingInvoice => write!(f, "Waiting Invoice"),
            TradeFilter::FiatSent => write!(f, "Fiat Sent"),
            TradeFilter::Success => write!(f, "Success"),
            TradeFilter::Canceled => write!(f, "Canceled"),
            TradeFilter::PayingSats => write!(f, "Paying Sats"),
            TradeFilter::Dispute => write!(f, "Dispute"),
        }
    }
}

pub const ALL_TRADE_FILTERS: &[TradeFilter] = &[
    TradeFilter::All,
    TradeFilter::Pending,
    TradeFilter::Active,
    TradeFilter::WaitingPayment,
    TradeFilter::WaitingInvoice,
    TradeFilter::FiatSent,
    TradeFilter::Success,
    TradeFilter::Canceled,
    TradeFilter::PayingSats,
    TradeFilter::Dispute,
];

pub fn buy_sell_tabs<'a>(
    active: &BuySellFilter,
    buy_count: usize,
    sell_count: usize,
) -> Container<'a, view::Message> {
    let buy_label = format!("BUY BTC ({})", buy_count);
    let sell_label = format!("SELL BTC ({})", sell_count);

    let buy_tab = button(
        container(p1_bold(buy_label))
            .padding([12, 0])
            .align_x(iced::alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .style(if *active == BuySellFilter::Buy {
        theme::button::primary as fn(&_, _) -> _
    } else {
        theme::button::transparent as fn(&_, _) -> _
    })
    .on_press(view::Message::P2P(P2PMessage::BuySellFilterChanged(
        BuySellFilter::Buy,
    )))
    .width(Length::Fill);

    let sell_tab = button(
        container(p1_bold(sell_label))
            .padding([12, 0])
            .align_x(iced::alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .style(if *active == BuySellFilter::Sell {
        theme::button::primary as fn(&_, _) -> _
    } else {
        theme::button::transparent as fn(&_, _) -> _
    })
    .on_press(view::Message::P2P(P2PMessage::BuySellFilterChanged(
        BuySellFilter::Sell,
    )))
    .width(Length::Fill);

    let active_count = match active {
        BuySellFilter::Buy => buy_count,
        BuySellFilter::Sell => sell_count,
    };

    let filter_chip = container(
        p2_regular(format!("FILTER | {} offers", active_count)).style(theme::text::secondary),
    )
    .padding([6, 16])
    .style(theme::pill::simple);

    let tabs_bar = container(row![buy_tab, sell_tab].spacing(4).width(Length::Fill))
        .padding(4)
        .width(Length::Fill)
        .style(theme::container::foreground_rounded);

    container(
        column![tabs_bar, filter_chip]
            .spacing(12)
            .width(Length::Fill),
    )
    .width(Length::Fill)
}

pub fn trade_status_filter<'a>(
    active_filters: &[TradeFilter],
    shown_count: usize,
) -> Container<'a, view::Message> {
    let chips = row(ALL_TRADE_FILTERS.iter().map(|filter| {
        let is_active = active_filters.contains(filter);
        let style: fn(&_, _) -> _ = if is_active {
            theme::button::primary
        } else {
            theme::button::transparent_border
        };

        button(
            container(p2_regular(filter.to_string()))
                .padding([4, 12])
                .align_x(iced::alignment::Horizontal::Center),
        )
        .style(style)
        .on_press(view::Message::P2P(P2PMessage::TradeFilterChanged(*filter)))
        .into()
    }))
    .spacing(6)
    .wrap();

    let count_pill =
        container(p2_regular(format!("{} trades", shown_count)).style(theme::text::secondary))
            .padding([6, 16])
            .style(theme::pill::simple);

    container(column![chips, count_pill].spacing(12).width(Length::Fill)).width(Length::Fill)
}
