use iced::{
    widget::qr_code::{self, QRCode},
    Alignment,
};

use liana::miniscript::bitcoin;

use liana_ui::{
    component::{card, text::*},
    icon, theme,
    widget::*,
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
                            .style(theme::Button::TransparentBorder),
                    )
                    .align_items(Alignment::Center),
            )
            .align_items(Alignment::Center)
            .spacing(20),
    )
    .into()
}
