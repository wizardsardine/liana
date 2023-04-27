use iced::{
    widget::{
        qr_code::{self, QRCode},
        scrollable, Space,
    },
    Alignment, Length,
};

use liana::miniscript::bitcoin;

use liana_ui::{
    color,
    component::{button, card, text::*},
    icon, theme,
    widget::*,
};

use super::message::Message;

pub fn receive<'a>(
    addresses: &'a [bitcoin::Address],
    qr: Option<&'a qr_code::State>,
) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .align_items(Alignment::Center)
                .push(Container::new(h3("Receive")).width(Length::Fill))
                .push(
                    button::primary(Some(icon::plus_icon()), "Generate address")
                        .on_press(Message::Next),
                ),
        )
        .push(p1_bold("New and never used reception addresses"))
        .push(
            Row::new()
                .spacing(10)
                .push(addresses.iter().rev().fold(
                    Column::new().spacing(10).width(Length::Fill),
                    |col, address| {
                        col.push(
                            card::simple(
                                Row::new()
                                    .push(
                                        Container::new(
                                            scrollable(
                                                Column::new()
                                                    .push(Space::with_height(Length::Units(10)))
                                                    .push(
                                                        p2_regular(address.to_string())
                                                            .small()
                                                            .style(color::GREY_3),
                                                    )
                                                    // Space between the address and the scrollbar
                                                    .push(Space::with_height(Length::Units(10))),
                                            )
                                            .horizontal_scroll(
                                                scrollable::Properties::new().scroller_width(5),
                                            ),
                                        )
                                        .width(Length::Fill),
                                    )
                                    .push(
                                        Button::new(icon::clipboard_icon().style(color::GREY_3))
                                            .on_press(Message::Clipboard(address.to_string()))
                                            .style(theme::Button::TransparentBorder),
                                    )
                                    .align_items(Alignment::Center),
                            )
                            .padding(20),
                        )
                    },
                ))
                .push(if let Some(qr) = qr {
                    Container::new(QRCode::new(qr).cell_size(5))
                        .padding(10)
                        .style(theme::Container::QrCode)
                } else {
                    Container::new(Space::with_width(Length::Fill)).width(Length::Units(200))
                }),
        )
        .spacing(20)
        .into()
}
