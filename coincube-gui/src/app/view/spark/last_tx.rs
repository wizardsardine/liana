//! Shared "Last Transactions" section for the Spark Send and Receive
//! panels. Produces the same row layout as the Spark Overview / Spark
//! Transactions pages so users see a consistent list across the wallet.

use coincube_ui::{
    color,
    component::{
        amount::BitcoinDisplayUnit,
        text::*,
        transaction::{TransactionDirection, TransactionListItem},
    },
    icon::{self, receipt_icon},
    image::asset_network_logo,
    theme,
    widget::*,
};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row, Space},
    Alignment, Background, Length,
};

use crate::app::view::spark::{SparkPaymentMethod, SparkRecentTransaction};
use crate::app::wallets::DomainPaymentStatus;

pub fn last_transactions_section<'a, M: 'a + Clone + 'static>(
    recent: &'a [SparkRecentTransaction],
    bitcoin_unit: BitcoinDisplayUnit,
    show_direction_badges: bool,
    on_select: impl Fn(usize) -> M + 'a,
    on_history: M,
) -> Element<'a, M> {
    let mut content = Column::new().spacing(10).push(h4_bold("Last transactions"));

    if recent.is_empty() {
        content = content.push(empty_placeholder(
            receipt_icon().size(80),
            "No transactions yet",
            "Your transaction history will appear here once you send or receive coins.",
        ));
        return content.into();
    }

    for (idx, tx) in recent.iter().enumerate() {
        let direction = if tx.is_incoming {
            TransactionDirection::Incoming
        } else {
            TransactionDirection::Outgoing
        };

        let tx_icon: Element<'_, M> = match tx.method {
            SparkPaymentMethod::Lightning => asset_network_logo("btc", "lightning", 40.0),
            SparkPaymentMethod::OnChainBitcoin => asset_network_logo("btc", "bitcoin", 40.0),
            SparkPaymentMethod::Spark => asset_network_logo("btc", "spark", 40.0),
        };

        let display_amount = if tx.is_incoming {
            tx.amount
        } else {
            tx.amount + tx.fees_sat
        };

        let mut item = TransactionListItem::new(direction, &display_amount, bitcoin_unit)
            .with_custom_icon(tx_icon)
            .with_show_direction_badge(show_direction_badges)
            .with_label(tx.description.clone())
            .with_time_ago(tx.time_ago.clone());

        if let Some(fiat) = tx.fiat_amount.as_ref() {
            item =
                item.with_fiat_amount(format!("~{} {}", fiat.to_rounded_string(), fiat.currency()));
        }

        if matches!(tx.status, DomainPaymentStatus::Pending) {
            let (bg, fg) = (color::GREY_3, color::BLACK);
            let pending_badge = Container::new(
                Row::new()
                    .push(
                        icon::warning_icon()
                            .size(14)
                            .style(move |_| iced::widget::text::Style { color: Some(fg) }),
                    )
                    .push(
                        text("Pending")
                            .bold()
                            .size(14)
                            .style(move |_| iced::widget::text::Style { color: Some(fg) }),
                    )
                    .spacing(4),
            )
            .padding([2, 8])
            .style(move |_| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg)),
                border: iced::Border {
                    radius: 12.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            });
            item = item.with_custom_status(pending_badge.into());
        }

        content = content.push(item.view(on_select(idx)));
    }

    let view_all_button = {
        let the_icon = icon::history_icon()
            .size(18)
            .style(|_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(color::ORANGE),
            });

        let label = text("View All Transactions")
            .size(15)
            .style(|_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(color::ORANGE),
            });

        let button_content = Row::new()
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center)
            .push(the_icon)
            .push(label);

        iced_button(Container::new(button_content).padding([10, 20]).style(
            |_theme: &theme::Theme| container::Style {
                background: Some(Background::Color(color::TRANSPARENT)),
                border: iced::Border {
                    color: color::ORANGE,
                    width: 1.5,
                    radius: 20.0.into(),
                },
                ..Default::default()
            },
        ))
        .style(|_theme: &theme::Theme, _| iced_button::Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::ORANGE,
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .on_press(on_history)
    };

    Column::new()
        .spacing(10)
        .push(content)
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Container::new(view_all_button)
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .push(Space::new().height(Length::Fixed(40.0)))
        .into()
}

fn empty_placeholder<'a, M: 'a + 'static, T: Into<Element<'a, M>>>(
    icon: T,
    title: &'a str,
    subtitle: &'a str,
) -> Element<'a, M> {
    let content = Column::new()
        .push(icon)
        .push(text(title).style(theme::text::secondary).bold())
        .push(
            text(subtitle)
                .size(P2_SIZE)
                .style(theme::text::secondary)
                .align_x(Alignment::Center),
        )
        .spacing(16)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .padding(60)
        .center_x(Length::Fill)
        .style(|t| container::Style {
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}
