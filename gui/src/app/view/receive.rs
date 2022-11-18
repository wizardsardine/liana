use iced::{
    pure::{column, row, widget::Button, Element},
    widget::qr_code::{self, QRCode},
    Alignment,
};

use liana::miniscript::bitcoin;

use crate::ui::{
    component::{button, card, text::*},
    icon,
};

use super::message::Message;

pub fn receive<'a>(address: &'a bitcoin::Address, qr: &'a qr_code::State) -> Element<'a, Message> {
    card::simple(
        column()
            .push(QRCode::new(qr).cell_size(10))
            .push(
                row()
                    .push(text(&address.to_string()).small())
                    .push(
                        Button::new(icon::clipboard_icon())
                            .on_press(Message::Clipboard(address.to_string()))
                            .style(button::Style::TransparentBorder),
                    )
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .spacing(20),
    )
    .into()
}
