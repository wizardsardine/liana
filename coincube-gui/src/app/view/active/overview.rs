use breez_sdk_liquid::model::PaymentState;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{button, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{Column, Container, Row, Space},
    Alignment, Length,
};

use crate::app::view::{active::RecentTransaction, ActiveOverviewMessage, FiatAmountConverter};

pub fn active_overview_view<'a>(
    btc_balance: Amount,
    fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &Vec<RecentTransaction>,
    error: Option<&'a str>,
) -> Element<'a, ActiveOverviewMessage> {
    let mut content = Column::new()
        .spacing(10)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    let mut balance_section = Column::new().spacing(10).align_x(Alignment::Center).push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(
                text(format!("{:.8}", btc_balance.to_btc()))
                    .size(48)
                    .bold()
                    .color(color::ORANGE),
            )
            .push(text("BTC").size(32).color(color::ORANGE)),
    );

    if let Some(converter) = &fiat_converter {
        let fiat_amount = converter.convert(btc_balance);
        balance_section = balance_section.push(fiat_amount.to_text().size(18).color(color::GREY_3));
    }

    content = content.push(balance_section);

    if recent_transaction.len() > 0 {
        for tx in recent_transaction {
            let row = Row::new()
                .spacing(15)
                .align_y(Alignment::Start)
                .push(
                    Container::new(icon::lightning_icon().size(24).color(color::ORANGE))
                        .padding(10),
                )
                .push(
                    Column::new()
                        .spacing(5)
                        .push(p1_bold(&tx.description).bold())
                        .push(
                            Row::new()
                                .push_maybe(if !matches!(tx.status, PaymentState::Pending) {
                                    Some(p2_regular(&tx.time_ago).style(theme::text::secondary))
                                } else {
                                    None
                                })
                                .push_maybe({
                                    if matches!(tx.status, PaymentState::Pending) {
                                        let (bg, fg) = (color::GREY_3, color::BLACK);
                                        Some(
                                            Container::new(
                                                Row::new()
                                                    .push(icon::warning_icon().size(14).style(
                                                        move |_| iced::widget::text::Style {
                                                            color: Some(fg),
                                                        },
                                                    ))
                                                    .push(text("Pending").bold().size(14).style(
                                                        move |_| iced::widget::text::Style {
                                                            color: Some(fg),
                                                        },
                                                    ))
                                                    .spacing(4),
                                            )
                                            .padding([2, 8])
                                            .style(
                                                move |_| iced::widget::container::Style {
                                                    background: Some(iced::Background::Color(bg)),
                                                    border: iced::Border {
                                                        radius: 12.0.into(),
                                                        ..Default::default()
                                                    },
                                                    ..Default::default()
                                                },
                                            ),
                                        )
                                    } else {
                                        None
                                    }
                                })
                                .spacing(8),
                        ),
                )
                .push(iced::widget::Space::new().width(Length::Fill))
                .push(
                    Column::new()
                        .spacing(5)
                        .align_x(Alignment::End)
                        .push(
                            text(format!("{} {:.8} BTC", tx.sign, tx.amount.to_btc()))
                                .size(16)
                                .color(if tx.is_incoming {
                                    color::GREEN
                                } else {
                                    color::RED
                                }),
                        )
                        .push(if let Some(fiat_amount) = &tx.fiat_amount {
                            text(format!(
                                "about {} {}",
                                fiat_amount.to_rounded_string(),
                                fiat_amount.currency().to_string()
                            ))
                            .size(14)
                            .color(color::GREY_3)
                        } else {
                            text("").size(12)
                        }),
                );
            let tx = Container::new(row)
                .padding(20)
                .style(theme::card::simple)
                .width(Length::Fill)
                .max_width(800);
            content = content.push(tx);
            content = content.push(Space::new().width(Length::Fill).height(5));
        }
    }

    let history_button = button::transparent(Some(icon::history_icon()), "History")
        .on_press(ActiveOverviewMessage::History)
        .width(Length::Fixed(150.0));

    content = content
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            Container::new(history_button)
                .width(Length::Fill)
                .align_x(Alignment::Center),
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
        .push(
            Container::new(buttons_row)
                .width(Length::Fill)
                .max_width(800)
                .align_x(Alignment::Center),
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
