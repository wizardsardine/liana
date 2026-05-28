use std::fmt::Display;

use iced::{
    widget::{
        column,
        qr_code::{self, QRCode},
        row, Button, Space,
    },
    Alignment, Length,
};

use crate::{
    component::{
        self,
        button::{btn_show_qr_compact, btn_verify_compact},
        card, label,
        text::{new, text, Text},
    },
    icon, theme,
    widget::{Container, Element, SpaceExt},
};

pub fn address_card<'a, M: Clone + 'static>(
    label: impl Display,
    address: &'a bitcoin::Address,
    edit_label: M,
    clipboard: M,
    verify: M,
    show_qr: M,
) -> Element<'a, M> {
    let label = label::editable_label(label, edit_label);
    let address = new::caption(address).style(theme::text::card_secondary);
    let addr_row = row![address, component::button::btn_copy(Some(clipboard))]
        .spacing(12)
        .align_y(Alignment::Center);
    let top = column![label, addr_row].spacing(12);

    let bottom = row![
        btn_verify_compact(verify),
        Space::fill_width(),
        btn_show_qr_compact(show_qr)
    ];

    let content = column![top, bottom].spacing(16);

    card::simple(content).into()
}

/// Collapsible header toggling the list of previously generated addresses.
pub fn previous_addresses_header<'a, M: Clone + 'static>(show: bool, toggle: M) -> Element<'a, M> {
    let chevron = if show {
        icon::collapsed_icon()
    } else {
        icon::collapse_icon()
    };
    let text = new::d3("Previously generated addresses still awaiting deposit");
    let header = row![text, chevron]
        .spacing(14)
        .align_y(Alignment::Center)
        .wrap();

    Button::new(header)
        .style(theme::button::transparent)
        .on_press(toggle)
        .into()
}

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
