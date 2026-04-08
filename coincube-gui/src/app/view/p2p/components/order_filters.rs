use std::collections::HashSet;

use coincube_ui::{component::text::*, icon, theme, widget::*};
use iced::{
    widget::{button, column, combo_box, container, row, slider},
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

pub struct OrderFilterState<'a> {
    pub buy_sell: &'a BuySellFilter,
    pub buy_count: usize,
    pub sell_count: usize,
    pub filter_currency: &'a String,
    pub currency_combo_state: &'a combo_box::State<String>,
    pub available_payment_methods: &'a [String],
    pub deselected_payment_methods: &'a HashSet<String>,
    pub min_rating: f32,
    pub min_days_active: u32,
    pub filtered_count: usize,
}

/// Left sidebar with buy/sell tabs, currency, payment methods, and reputation filters.
pub fn order_filter_sidebar<'a>(state: OrderFilterState<'a>) -> Container<'a, view::Message> {
    let buy_label = format!("BUY BTC ({})", state.buy_count);
    let sell_label = format!("SELL BTC ({})", state.sell_count);

    let buy_tab = button(
        container(p2_bold(buy_label))
            .padding([8, 0])
            .align_x(iced::alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .style(if *state.buy_sell == BuySellFilter::Buy {
        theme::button::primary as fn(&_, _) -> _
    } else {
        theme::button::transparent as fn(&_, _) -> _
    })
    .on_press(view::Message::P2P(P2PMessage::BuySellFilterChanged(
        BuySellFilter::Buy,
    )))
    .width(Length::Fill);

    let sell_tab = button(
        container(p2_bold(sell_label))
            .padding([8, 0])
            .align_x(iced::alignment::Horizontal::Center)
            .width(Length::Fill),
    )
    .style(if *state.buy_sell == BuySellFilter::Sell {
        theme::button::primary as fn(&_, _) -> _
    } else {
        theme::button::transparent as fn(&_, _) -> _
    })
    .on_press(view::Message::P2P(P2PMessage::BuySellFilterChanged(
        BuySellFilter::Sell,
    )))
    .width(Length::Fill);

    let tabs_bar = container(row![buy_tab, sell_tab].spacing(4).width(Length::Fill))
        .padding(4)
        .width(Length::Fill)
        .style(theme::container::foreground_rounded);

    // --- Currency filter ---
    let currency_combo = combo_box(
        state.currency_combo_state,
        "Search currency...",
        Some(state.filter_currency),
        |selected: String| view::Message::P2P(P2PMessage::FilterCurrencySelected(selected)),
    )
    .padding(10)
    .width(Length::Fill);

    let currency_section = column![
        p2_bold("CURRENCY").style(theme::text::secondary),
        container(currency_combo)
            .style(theme::container::foreground_rounded)
            .width(Length::Fill),
    ]
    .spacing(8);

    // --- Payment methods filter ---
    let payment_methods_section: Element<'_, view::Message> =
        if state.available_payment_methods.is_empty() {
            column![].into()
        } else {
            let chips = row(state.available_payment_methods.iter().map(|method| {
                let is_selected = !state.deselected_payment_methods.contains(method);
                let style: fn(&_, _) -> _ = if is_selected {
                    theme::button::primary
                } else {
                    theme::button::transparent_border
                };
                button(
                    container(p2_regular(method.clone()))
                        .padding([4, 12])
                        .align_x(iced::alignment::Horizontal::Center),
                )
                .style(style)
                .on_press(view::Message::P2P(P2PMessage::FilterPaymentMethodToggled(
                    method.clone(),
                )))
                .into()
            }))
            .spacing(6)
            .wrap();

            column![
                p2_bold("PAYMENT METHODS").style(theme::text::secondary),
                chips,
            ]
            .spacing(8)
            .into()
        };

    // --- Reputation filter ---
    let rating_text = if state.min_rating == 0.0 {
        "Off".to_string()
    } else {
        format!("{:.1}+", state.min_rating)
    };

    let days_text = if state.min_days_active == 0 {
        "Off".to_string()
    } else {
        format!("{}+ days", state.min_days_active)
    };

    let description = if state.min_rating > 0.0 || state.min_days_active > 0 {
        format!(
            "Showing only verified traders with {:.1}+ star ratings and at least {} days active.",
            state.min_rating, state.min_days_active
        )
    } else {
        "Showing all traders regardless of reputation.".to_string()
    };

    let reputation_card = container(
        column![
            row![
                icon::shield_icon().style(theme::text::warning),
                p2_bold("Reputation Filter"),
            ]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center),
            column![
                row![
                    p2_regular("Min rating:"),
                    p2_bold(rating_text).style(theme::text::warning),
                ]
                .spacing(4)
                .align_y(iced::alignment::Vertical::Center),
                slider(0.0..=5.0_f32, state.min_rating, |v: f32| {
                    view::Message::P2P(P2PMessage::FilterMinRatingChanged(v))
                })
                .step(0.5),
            ]
            .spacing(4),
            column![
                row![
                    p2_regular("Min days active:"),
                    p2_bold(days_text).style(theme::text::warning),
                ]
                .spacing(4)
                .align_y(iced::alignment::Vertical::Center),
                slider(0.0..=365.0_f32, state.min_days_active as f32, |v: f32| {
                    view::Message::P2P(P2PMessage::FilterMinDaysActiveChanged(v as u32))
                })
                .step(1.0),
            ]
            .spacing(4),
            caption(description).style(theme::text::secondary),
        ]
        .spacing(12),
    )
    .padding(15)
    .style(theme::container::balance_header)
    .width(Length::Fill);

    // --- Results count ---
    let results_pill = container(
        p2_regular(format!("{} offers", state.filtered_count)).style(theme::text::secondary),
    )
    .padding([6, 16])
    .style(theme::pill::simple);

    container(
        column![
            tabs_bar,
            currency_section,
            payment_methods_section,
            reputation_card,
            results_pill,
        ]
        .spacing(16)
        .width(Length::Fill),
    )
    .padding(20)
    .style(theme::container::balance_header)
    .width(Length::Fixed(420.0))
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
