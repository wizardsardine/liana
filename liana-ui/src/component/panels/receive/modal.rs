use iced::{
    widget::{
        column,
        qr_code::{self, QRCode},
        row, Button, Space,
    },
    Alignment, Length,
};

use crate::{
    component::text::{text, Text},
    icon, theme,
    widget::*,
};

/// Address and derivation-index rows shown at the top of the verify-address
/// modal, with a copy button for the address.
pub fn verify_address_modal<'a, M: Clone + 'static>(
    address: &bitcoin::Address,
    derivation_index: &bitcoin::bip32::ChildNumber,
    clipboard: M,
) -> Element<'a, M> {
    let address_row = row![
        Container::new(text("Address:").bold()).width(Length::Fill),
        row![
            Container::new(text(address.to_string()).small()),
            Button::new(icon::clipboard_icon())
                .on_press(clipboard)
                .style(theme::button::transparent_border),
        ]
        .align_y(Alignment::Center)
        .width(Length::Shrink),
    ]
    .width(Length::Fill)
    .align_y(Alignment::Center);

    let index_row = row![
        Container::new(text("Derivation index:").bold()).width(Length::Fill),
        Container::new(text(derivation_index.to_string()).small()).width(Length::Shrink),
    ]
    .width(Length::Fill)
    .align_y(Alignment::Center);

    column![address_row, index_row].spacing(5).into()
}

/// QR code for an address, with the address shown below it.
pub fn qr_display<'a, M: 'a>(qr: &'a qr_code::Data, address: &'a str) -> Element<'a, M> {
    column![
        row![
            Space::fill_width(),
            Container::new(QRCode::<theme::Theme>::new(qr).cell_size(8)).padding(10),
            Space::fill_width(),
        ],
        Space::with_height(Length::Fixed(15.0)),
        Container::new(text(address).size(15)).center_x(Length::Fill),
    ]
    .width(Length::Fill)
    .max_width(400)
    .into()
}
