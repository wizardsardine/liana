use breez_sdk_liquid::model::{PaymentDetails, PaymentState};
use breez_sdk_liquid::prelude::{Payment, PaymentType};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    component::{amount::amount, amount::DisplayAmount, badge, button, card, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{Column, Container, Row, Space},
    Alignment, Length,
};

use crate::app::menu::Menu;
use crate::app::view::message::Message;
use crate::app::view::FiatAmountConverter;
use crate::export::ImportExportMessage;
use crate::utils::{format_time_ago, format_timestamp};

pub fn active_transactions_view<'a>(
    payments: &'a [Payment],
    _balance: &'a Amount,
    fiat_converter: Option<FiatAmountConverter>,
    _loading: bool,
) -> Element<'a, Message> {
    let mut content = Column::new().spacing(20).width(Length::Fill);

    // Header row with Transactions heading and Export button (matching Vault style)
    content = content.push(
        Row::new()
            .push(Container::new(h3("Transactions")))
            .push(Space::new().width(Length::Fill))
            .push(
                button::secondary(Some(icon::backup_icon()), "Export")
                    .on_press(ImportExportMessage::Open.into()),
            ),
    );

    if payments.is_empty() {
        // Empty state
        content = content.push(
            Column::new()
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .push(Space::new().height(Length::Fixed(100.0)))
                .push(h2("No transactions yet").style(theme::text::primary))
                .push(
                    text("Your Lightning wallet is ready. Once you send or receive\nsats, they'll show up here.")
                        .size(16)
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::Word)
                        .align_x(iced::alignment::Horizontal::Center),
                )
                .push(Space::new().height(Length::Fixed(20.0)))
                .push(
                    Row::new()
                        .spacing(15)
                        .push(
                            button::primary(None, "Send sats")
                                .on_press(Message::Menu(Menu::Active(crate::app::menu::ActiveSubMenu::Send)))
                                .padding(15)
                                .width(Length::Fixed(150.0)),
                        )
                        .push(
                            button::transparent_border(None, "Receive sats")
                                .on_press(Message::Menu(Menu::Active(crate::app::menu::ActiveSubMenu::Receive)))
                                .padding(15)
                                .width(Length::Fixed(150.0)),
                        ),
                ),
        );
    } else {
        // Transaction list
        content = content.push(
            Column::new().spacing(10).push(
                payments
                    .iter()
                    .enumerate()
                    .fold(Column::new().spacing(10), |col, (i, payment)| {
                        col.push(transaction_row(i, payment, fiat_converter))
                    }),
            ),
        );
    }

    content.into()
}

fn transaction_row<'a>(
    i: usize,
    payment: &'a Payment,
    fiat_converter: Option<FiatAmountConverter>,
) -> Element<'a, Message> {
    let is_receive = matches!(payment.payment_type, PaymentType::Receive);

    // Format timestamp
    let time_text = format_time_ago(payment.timestamp as u32);

    // Extract description from payment details
    let description = match &payment.details {
        PaymentDetails::Lightning {
            payer_note,
            description,
            ..
        } => payer_note
            .as_ref()
            .filter(|s| !s.is_empty())
            .unwrap_or(description),
        PaymentDetails::Liquid {
            payer_note,
            description,
            ..
        } => payer_note
            .as_ref()
            .filter(|s| !s.is_empty())
            .unwrap_or(description),
        PaymentDetails::Bitcoin { description, .. } => description,
    };

    let btc_amount = Amount::from_sat(payment.amount_sat);

    Container::new(
        Button::new(
            Row::new()
                .push(
                    Row::new()
                        .push(if is_receive {
                            badge::receive()
                        } else {
                            badge::spend()
                        })
                        .push(
                            Column::new()
                                .push(p1_regular(description))
                                .push(text(time_text).style(theme::text::secondary).small()),
                        )
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(
                    Column::new()
                        .spacing(5)
                        .align_x(Alignment::End)
                        .push(if is_receive {
                            Row::new()
                                .spacing(5)
                                .push(text("+"))
                                .push(amount(&btc_amount))
                                .align_y(Alignment::Center)
                        } else {
                            Row::new()
                                .spacing(5)
                                .push(text("-"))
                                .push(amount(&btc_amount))
                                .align_y(Alignment::Center)
                        })
                        .push_maybe(fiat_converter.map(|converter| {
                            let fiat = converter.convert(btc_amount);
                            fiat.to_text().size(14).style(theme::text::secondary)
                        })),
                )
                .align_y(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .on_press(Message::Select(i))
        .style(theme::button::transparent_border),
    )
    .style(theme::card::simple)
    .into()
}

pub fn payment_detail_view<'a>(
    payment: &'a Payment,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let is_receive = matches!(payment.payment_type, PaymentType::Receive);
    let btc_amount = Amount::from_sat(payment.amount_sat);

    // Format full date/time
    let date_text = format_timestamp(payment.timestamp as u64);

    // Extract description from payment details
    let description = match &payment.details {
        PaymentDetails::Lightning {
            payer_note,
            description,
            ..
        } => payer_note
            .as_ref()
            .filter(|s| !s.is_empty())
            .unwrap_or(description),
        PaymentDetails::Liquid {
            payer_note,
            description,
            ..
        } => payer_note
            .as_ref()
            .filter(|s| !s.is_empty())
            .unwrap_or(description),
        PaymentDetails::Bitcoin { description, .. } => description,
    };

    Column::new()
        .spacing(20)
        .push(if is_receive {
            Container::new(h3("Incoming payment")).width(Length::Fill)
        } else {
            Container::new(h3("Outgoing payment")).width(Length::Fill)
        })
        .push(Column::new().push(p1_regular(description)).spacing(10))
        .push(
            Column::new().spacing(20).push(
                Column::new()
                    .push(if is_receive {
                        Container::new(coincube_ui::component::amount::amount_with_size_and_unit(
                            &btc_amount,
                            H1_SIZE,
                            bitcoin_unit,
                        ))
                    } else {
                        Container::new(Row::new().spacing(5).push(text("-").size(H1_SIZE)).push(
                            coincube_ui::component::amount::amount_with_size_and_unit(
                                &btc_amount,
                                H1_SIZE,
                                bitcoin_unit,
                            ),
                        ))
                    })
                    .push_maybe(fiat_converter.map(|converter| {
                        let fiat = converter.convert(btc_amount);
                        Row::new().align_y(Alignment::Center).push(
                            fiat.to_text()
                                .size(H2_SIZE)
                                .color(coincube_ui::color::GREY_2),
                        )
                    })),
            ),
        )
        .push(card::simple(
            Column::new()
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Date").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(date_text)),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Status").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(match payment.status {
                                    PaymentState::Complete => text("Complete").style(theme::text::success),
                                    PaymentState::Pending => text("Pending").style(theme::text::secondary),
                                    PaymentState::Failed => text("Failed").style(theme::text::danger),
                                }),
                        )
                        .spacing(20),
                )
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .width(Length::FillPortion(1))
                                .push(text("Amount").bold()),
                        )
                        .push(
                            Column::new()
                                .width(Length::FillPortion(2))
                                .push(text(btc_amount.to_formatted_string_with_unit(bitcoin_unit))),
                        )
                        .spacing(20),
                )
                .spacing(15),
        ))
        .into()
}
