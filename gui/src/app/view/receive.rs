use iced::{
    widget::{
        qr_code::{self, QRCode},
        Button, Column, Row,
    },
    Alignment, Element,
};

use liana::miniscript::bitcoin;

use crate::ui::{
    component::{button, card, text::*},
    icon,
};

use super::message::Message;

pub fn receive<'a>(address: &'a bitcoin::Address, qr: &'a qr_code::State) -> Element<'a, Message> {
    card::simple(
        Column::new()
            .push(QRCode::new(qr).cell_size(10))
            .push(
                Row::new()
                    .push(text(address.to_string()).small())
                    .push(
                        Button::new(icon::clipboard_icon())
                            .on_press(Message::Clipboard(address.to_string()))
                            .style(button::Style::TransparentBorder.into()),
                    )
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .spacing(20),
    )
    .into()
}
