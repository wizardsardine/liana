use iced::{
    widget::{
        column,
        qr_code::{self, QRCode},
        row, Space,
    },
    Alignment, Length,
};

use crate::{
    component::{
        self,
        address::copyable_address,
        button::{btn_copy, btn_show_qr, btn_verify},
        text::{text, Text},
    },
    theme,
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
        text("Address:").bold(),
        text(address.to_string()).small(),
        btn_copy(Some(clipboard)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let index_row = row![
        text("Derivation index:").bold(),
        text(derivation_index.to_string()).small(),
    ]
    .spacing(10)
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

pub fn show_address_modal<'a, M: 'a + Clone>(
    address: &bitcoin::Address,
    close: M,
    verify: M,
    show_qr: M,
    clipboard: M,
) -> Element<'a, M> {
    let addr_row = copyable_address(address, clipboard);
    let btn_row = row![
        btn_verify(verify),
        Space::fill_width(),
        btn_show_qr(show_qr)
    ];
    let content = column![addr_row, btn_row].spacing(28);
    component::modal::modal_view(
        Some("Address"),
        None,
        Some(close),
        component::modal::ModalWidth::XL,
        content,
    )
}
