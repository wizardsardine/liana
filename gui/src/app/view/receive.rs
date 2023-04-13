use iced::{
    widget::qr_code::{self, QRCode},
    Alignment, Length,
};

use liana::miniscript::bitcoin;

use liana_ui::{
    component::{button, card, text::*},
    icon, theme,
    widget::*,
};

use super::message::Message;

pub fn receive<'a>(address: &'a bitcoin::Address, qr: &'a qr_code::State) -> Element<'a, Message> {
    Column::new()
        .push(card::simple(
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
        ))
        .push(
            Column::new().push(
                button::primary(None, "Generate new")
                    .on_press(Message::Next)
                    .width(Length::Units(150)),
            ),
        )
        .spacing(20)
        .align_items(Alignment::Center)
        .into()
}
