use breez_sdk_liquid::model::PaymentState;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{button, form, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{Column, Container, Row, Space},
    Alignment, Length,
};

use crate::app::view::{vault::fiat::FiatAmount, ActiveSendMessage, FiatAmountConverter};

pub fn active_send_view<'a>(
    btc_balance: Amount,
    fiat_converter: Option<FiatAmountConverter>,
    recent_transaction: &Vec<RecentTransaction>,
    input: &'a form::Value<String>,
    error: Option<&'a str>,
) -> Element<'a, ActiveSendMessage> {
    let mut content = Column::new()
        .spacing(10)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    // Balance Display Section
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

    // Add fiat equivalent if converter is available
    if let Some(converter) = &fiat_converter {
        let fiat_amount = converter.convert(btc_balance);
        balance_section = balance_section.push(fiat_amount.to_text().size(18).color(color::GREY_3));
    }

    content = content.push(balance_section);

    // Recent Transaction (if exists)
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
                                .push_maybe(if tx.status != PaymentState::Pending {
                                    Some(p2_regular(&tx.time_ago).style(theme::text::secondary))
                                } else {
                                    None
                                })
                                .push_maybe({
                                    if let PaymentState::Pending = tx.status {
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
        .on_press(ActiveSendMessage::History)
        .width(Length::Fixed(150.0));

    content = content
        .push(iced::widget::Space::new().height(Length::Fixed(10.0)))
        .push(
            Container::new(history_button)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        );

    // Add spacing before input section
    content = content.push(iced::widget::Space::new().height(Length::Fixed(20.0)));

    // Input Section
    let input_section = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .max_width(800)
        .align_x(Alignment::Center)
        .push(
            Container::new(
                text("Enter Invoice, Lightning Address, or BTC Address")
                    .size(16)
                    .bold(),
            )
            .width(Length::Fill)
            .align_x(Alignment::Center),
        )
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    form::Form::new(
                        "e.g. satoshi@nakamoto.com",
                        input,
                        ActiveSendMessage::InputEdited,
                    )
                    .size(16)
                    .padding(15),
                )
                .push(
                    Container::new(
                        iced::widget::button(
                            Container::new(icon::arrow_right())
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .align_x(Alignment::Center)
                                .align_y(Alignment::Center),
                        )
                        .on_press_maybe(if input.valid && !input.value.trim().is_empty() {
                            Some(ActiveSendMessage::Send)
                        } else {
                            None
                        })
                        .width(Length::Fixed(50.0))
                        .height(Length::Fixed(50.0))
                        .style(theme::button::primary),
                    )
                    .width(Length::Fixed(50.0))
                    .height(Length::Fixed(50.0)),
                ),
        );

    content = content.push(input_section);

    // Error display
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

/// Recent transaction display data
pub struct RecentTransaction {
    pub description: String,
    pub time_ago: String,
    pub amount: Amount,
    pub fiat_amount: Option<FiatAmount>,
    pub is_incoming: bool,
    pub sign: &'static str,
    pub status: PaymentState,
}
