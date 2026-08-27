use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};

use crate::{
    component::{
        self,
        address::{address as address_view, copyable_address},
        button::{btn_copy, btn_show_qr, btn_verify},
        qr,
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
        address_view(address.to_string()),
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
///
/// `content` is what the code carries, usually a BIP-21 URI, while `address` is
/// what is written out underneath for the reader to check by eye.
pub fn qr_display<'a, M: 'a>(content: &str, address: &'a str) -> Element<'a, M> {
    let code = qr::text::<M>(
        theme::qr_code::qr_code(&theme::Theme::default()),
        content,
        None,
    );
    column![
        code,
        Space::with_height(15),
        Container::new(address_view(address)).center_x(Length::Fill),
    ]
    .align_x(Alignment::Center)
    .width(Length::Fill)
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
