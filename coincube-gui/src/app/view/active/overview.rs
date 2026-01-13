use breez_sdk_liquid::model::{PaymentDetails, PaymentState};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{
        amount::*,
        button,
        text::*,
        transaction::{TransactionDirection, TransactionListItem, TransactionType},
    },
    icon, theme,
    widget::*,
};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row},
    Background, Length,
};

use crate::app::view::{active::RecentTransaction, ActiveOverviewMessage, FiatAmountConverter};

pub fn active_overview_view<'a>(
    btc_balance: Amount,
    fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &Vec<RecentTransaction>,
    error: Option<&'a str>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, ActiveOverviewMessage> {
    let mut content = Column::new().spacing(20);

    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(btc_balance));

    content = content.push(h3("Balance")).push(
        Column::new()
            .spacing(5)
            .push(amount_with_size_and_unit(
                &btc_balance,
                H1_SIZE,
                bitcoin_unit,
            ))
            .push_maybe(fiat_balance.map(|fiat| fiat.to_text().size(P2_SIZE).color(color::GREY_2))),
    );

    let buttons_row = Row::new()
        .spacing(10)
        .width(Length::Fill)
        .push(
            button::primary(None, "Send")
                .on_press(ActiveOverviewMessage::Send)
                .width(Length::Fill),
        )
        .push(
            button::secondary(None, "Receive")
                .on_press(ActiveOverviewMessage::Receive)
                .width(Length::Fill)
                .style(|_, _| iced::widget::button::Style {
                    background: Some(iced::Background::Color(iced::Color::TRANSPARENT)),
                    text_color: color::ORANGE,
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..Default::default()
                }),
        );

    content = content
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(Container::new(buttons_row).width(Length::Fill));

    content = content.push(Column::new().spacing(10).push(h4_bold("Last transactions")));

    if !recent_transaction.is_empty() {
        for (idx, tx) in recent_transaction.iter().enumerate() {
            let direction = if tx.is_incoming {
                TransactionDirection::Incoming
            } else {
                TransactionDirection::Outgoing
            };

            let tx_type = if let PaymentDetails::Bitcoin { .. } = tx.details {
                TransactionType::Bitcoin
            } else {
                TransactionType::Lightning
            };

            let fiat_str = tx
                .fiat_amount
                .as_ref()
                .map(|fiat| format!("~{} {}", fiat.to_rounded_string(), fiat.currency()));

            let mut amount = tx.amount.clone();
            if !tx.is_incoming {
                amount = amount + tx.fees_sat;
            }
            let mut item = TransactionListItem::new(direction, &amount, bitcoin_unit)
                .with_type(tx_type)
                .with_label(tx.description.clone())
                .with_time_ago(tx.time_ago.clone());

            if matches!(tx.status, PaymentState::Pending) {
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

            if let Some(fiat) = fiat_str {
                item = item.with_fiat_amount(fiat);
            }

            content = content.push(item.view(ActiveOverviewMessage::SelectTransaction(idx)));
        }
    }

    let view_transactions_button = {
        let icon = icon::history_icon()
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
            .push(icon)
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
        .on_press(ActiveOverviewMessage::History)
    };

    content = content
        .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
        .push(
            Container::new(view_transactions_button)
                .width(Length::Fill)
                .center_x(Length::Fill),
        );

    if let Some(err) = error {
        content = content.push(
            Container::new(text(err).size(14).color(color::RED))
                .padding(10)
                .style(theme::card::invalid)
                .width(Length::Fill)
                .max_width(800),
        );
    }
    content.into()
}
