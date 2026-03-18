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
    icon::{self, receipt_icon},
    theme,
    widget::*,
};
use iced::{
    widget::{button as iced_button, container, Column, Container, Row},
    Alignment, Background, Length,
};

use crate::app::view::{liquid::RecentTransaction, FiatAmountConverter, LiquidOverviewMessage};

pub fn liquid_overview_view<'a>(
    btc_balance: Amount,
    fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &[RecentTransaction],
    error: Option<&'a str>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, LiquidOverviewMessage> {
    let mut content = Column::new().spacing(20);

    let btc_fiat = fiat_converter.as_ref().map(|c| c.convert(btc_balance));

    let pending_outgoing_sats: u64 = recent_transaction
        .iter()
        .filter(|t| !t.is_incoming && matches!(t.status, PaymentState::Pending))
        .map(|t| (t.amount + t.fees_sat).to_sat())
        .sum();

    let pending_incoming_sats: u64 = recent_transaction
        .iter()
        .filter(|t| t.is_incoming && matches!(t.status, PaymentState::Pending))
        .map(|t| t.amount.to_sat())
        .sum();

    // ── Balance header ────────────────────────────────────────────────────────
    let balance_inner = Column::new()
        .spacing(4)
        .push(amount_with_size_and_unit(
            &btc_balance,
            H2_SIZE,
            bitcoin_unit,
        ))
        .push_maybe(btc_fiat.map(|fiat| -> Element<'_, LiquidOverviewMessage> {
            text(format!("~{} {}", fiat.to_rounded_string(), fiat.currency()))
                .size(P1_SIZE)
                .style(theme::text::secondary)
                .into()
        }))
        .push_maybe(if pending_outgoing_sats > 0 {
            Some(
                Row::new()
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(icon::warning_icon().size(12).style(theme::text::secondary))
                    .push(text("-").size(P2_SIZE).style(theme::text::secondary))
                    .push(amount_with_size_and_unit(
                        &Amount::from_sat(pending_outgoing_sats),
                        P2_SIZE,
                        bitcoin_unit,
                    ))
                    .push(text("pending").size(P2_SIZE).style(theme::text::secondary)),
            )
        } else {
            None
        })
        .push_maybe(if pending_incoming_sats > 0 {
            Some(
                Row::new()
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(icon::warning_icon().size(12).style(theme::text::secondary))
                    .push(text("+").size(P2_SIZE).style(theme::text::secondary))
                    .push(amount_with_size_and_unit(
                        &Amount::from_sat(pending_incoming_sats),
                        P2_SIZE,
                        bitcoin_unit,
                    ))
                    .push(text("pending").size(P2_SIZE).style(theme::text::secondary)),
            )
        } else {
            None
        });

    let balance_col = Column::new()
        .spacing(8)
        .push(h4_bold("Balance"))
        .push(balance_inner);

    let action_buttons = Row::new()
        .spacing(8)
        .push(
            button::primary(None, "Send")
                .on_press(LiquidOverviewMessage::SendLbtc)
                .width(Length::Fixed(120.0)),
        )
        .push(
            iced_button(
                Container::new(text("Receive").size(14))
                    .padding([8, 16])
                    .center_x(Length::Fill),
            )
            .on_press(LiquidOverviewMessage::ReceiveLbtc)
            .width(Length::Fixed(120.0))
            .style(|_, _| iced::widget::button::Style {
                background: Some(Background::Color(iced::Color::TRANSPARENT)),
                text_color: color::ORANGE,
                border: iced::Border {
                    color: color::ORANGE,
                    width: 1.0,
                    radius: 25.0.into(),
                },
                ..Default::default()
            }),
        );

    let header_card = Container::new(
        Row::new()
            .align_y(Alignment::Center)
            .push(balance_col.width(Length::Fill))
            .push(action_buttons),
    )
    .padding(20)
    .width(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(color::GREY_6)),
        border: iced::Border {
            color: color::ORANGE,
            width: 0.2,
            radius: 25.0.into(),
        },
        ..Default::default()
    });

    content = content.push(header_card);

    content = content.push(Column::new().spacing(10).push(h4_bold("Last transactions")));

    if !recent_transaction.is_empty() {
        for (idx, tx) in recent_transaction.iter().enumerate() {
            let direction = if tx.is_incoming {
                TransactionDirection::Incoming
            } else {
                TransactionDirection::Outgoing
            };

            let tx_type = match &tx.details {
                PaymentDetails::Lightning { .. } => TransactionType::Lightning,
                PaymentDetails::Liquid { .. } | PaymentDetails::Bitcoin { .. } => {
                    TransactionType::Bitcoin
                }
            };

            let fiat_str = tx
                .fiat_amount
                .as_ref()
                .map(|fiat| format!("~{} {}", fiat.to_rounded_string(), fiat.currency()));

            let display_amount = if tx.is_incoming {
                tx.amount
            } else {
                tx.amount + tx.fees_sat
            };

            let mut item = TransactionListItem::new(direction, &display_amount, bitcoin_unit)
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

            content = content.push(item.view(LiquidOverviewMessage::SelectTransaction(idx)));
        }
    } else {
        content = content.push(placeholder(
            receipt_icon().size(80),
            "No transactions yet",
            "Your transaction history will appear here once you send or receive coins.",
        ));
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
        .on_press(LiquidOverviewMessage::History)
    };

    if !recent_transaction.is_empty() {
        content = content
            .push(iced::widget::Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(view_transactions_button)
                    .width(Length::Fill)
                    .center_x(Length::Fill),
            );
    }

    if let Some(err) = error {
        content = content.push(
            Container::new(text(err.to_string()).size(14).color(color::RED))
                .padding(10)
                .style(theme::card::invalid)
                .width(Length::Fill)
                .max_width(800),
        );
    }

    content.into()
}

pub fn placeholder<'a, T: Into<Element<'a, LiquidOverviewMessage>>>(
    icon: T,
    title: &'a str,
    subtitle: &'a str,
) -> Element<'a, LiquidOverviewMessage> {
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
        .style(|_| container::Style {
            background: Some(iced::Background::Color(color::GREY_6)),
            border: iced::Border {
                radius: 20.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}
