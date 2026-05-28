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
        button, card, scrollable,
        text::{p1_bold, p2_regular, text, Text},
    },
    icon, theme,
    widget::{Container, Element, SpaceExt},
};

/// A single receive address: its label, the address with a copy button, and
/// the verify / show-QR actions.
pub fn address_card<'a, M: Clone + 'static>(
    label: impl Into<Element<'a, M>>,
    address: &'a bitcoin::Address,
    clipboard: M,
    verify: M,
    show_qr: M,
) -> Element<'a, M> {
    let address_row = row![
        Container::new(scrollable::horizontal_thin(column![
            Space::with_height(Length::Fixed(10.0)),
            p2_regular(address).small().style(theme::text::secondary),
        ]))
        .width(Length::Fill),
        Button::new(icon::clipboard_icon().style(theme::text::secondary))
            .on_press(clipboard)
            .style(theme::button::transparent_border),
    ]
    .align_y(Alignment::Center);

    let buttons = row![
        button::secondary(None, "Verify on hardware device").on_press(verify),
        Space::fill_width(),
        button::secondary(None, "Show QR Code").on_press(show_qr),
    ];

    card::simple(column![label.into(), address_row, buttons].spacing(10)).into()
}

/// Collapsible header toggling the list of previously generated addresses.
pub fn previous_addresses_header<'a, M: Clone + 'static>(show: bool, toggle: M) -> Element<'a, M> {
    let chevron = if show {
        icon::collapsed_icon()
    } else {
        icon::collapse_icon()
    };
    let header = Button::new(
        row![
            p1_bold("Previously generated addresses still awaiting deposit").width(Length::Fill),
            chevron,
        ]
        .align_y(Alignment::Center),
    )
    .on_press(toggle)
    .padding(20)
    .width(Length::Fill)
    .style(theme::button::transparent_border);

    Container::new(header)
        .style(theme::card::button_simple)
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
