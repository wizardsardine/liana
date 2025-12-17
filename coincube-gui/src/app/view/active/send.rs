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

use crate::app::view::ActiveSendMessage;

pub fn active_send_view<'a>(
    btc_balance: f64,
    usd_balance: f64,
    recent_transaction: Option<&'a RecentTransaction>,
    invoice_input: &'a form::Value<String>,
    error: Option<&'a str>,
) -> Element<'a, ActiveSendMessage> {
    let mut content = Column::new()
        .spacing(30)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    // Balance Display Section
    let balance_section = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    text(format!("{:.8}", btc_balance))
                        .size(48)
                        .bold()
                        .color(color::ORANGE),
                )
                .push(text("BTC").size(32).color(color::ORANGE)),
        )
        .push(
            text(format!("$ {:.2}", usd_balance))
                .size(18)
                .color(color::GREY_3),
        );

    content = content.push(balance_section);

    // Recent Transaction (if exists)
    if let Some(tx) = recent_transaction {
        let tx_card = Container::new(
            Row::new()
                .spacing(15)
                .align_y(Alignment::Center)
                .push(
                    Container::new(icon::lightning_icon().size(24).color(color::ORANGE))
                        .padding(10),
                )
                .push(
                    Column::new()
                        .spacing(5)
                        .push(p1_bold(&tx.description))
                        .push(p2_regular(&tx.time_ago).style(theme::text::secondary)),
                )
                .push(Space::with_width(Length::Fill))
                .push(
                    Column::new()
                        .spacing(5)
                        .align_x(Alignment::End)
                        .push(
                            text(format!("{} {:.8} BTC", tx.sign, tx.amount))
                                .size(14)
                                .color(if tx.is_incoming {
                                    color::GREEN
                                } else {
                                    color::RED
                                }),
                        )
                        .push(
                            text(format!("about $ {:.2}", tx.usd_amount))
                                .size(12)
                                .color(color::GREY_3),
                        ),
                ),
        )
        .padding(20)
        .style(theme::card::simple)
        .width(Length::Fill)
        .max_width(800);

        content = content.push(tx_card);
    }

    // History Button
    let history_button = button::secondary(Some(icon::receipt_icon()), "History")
        .on_press(ActiveSendMessage::ViewHistory)
        .width(Length::Fixed(150.0));

    content = content.push(Container::new(history_button).width(Length::Fill));

    content = content.push(Space::with_height(Length::Fixed(40.0)));

    // Input Section
    let input_section = Column::new()
        .spacing(20)
        .width(Length::Fill)
        .max_width(800)
        .push(
            text("Enter Invoice, Lightning Address, or BTC Address")
                .size(16)
                .bold(),
        )
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(
                    form::Form::new(
                        "e.g. satoshi@nakamoto.com",
                        invoice_input,
                        ActiveSendMessage::InvoiceEdited,
                    )
                    .size(16)
                    .padding(15),
                )
                .push(
                    button::primary(Some(icon::arrow_right()), "")
                        .on_press_maybe(
                            if invoice_input.valid && !invoice_input.value.trim().is_empty() {
                                Some(ActiveSendMessage::Send)
                            } else {
                                None
                            },
                        )
                        .width(Length::Fixed(60.0))
                        .padding(15),
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

    Container::new(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

/// Recent transaction display data
pub struct RecentTransaction {
    pub description: String,
    pub time_ago: String,
    pub amount: f64,
    pub usd_amount: f64,
    pub is_incoming: bool,
    pub sign: &'static str,
}
