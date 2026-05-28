use iced::{
    widget::{column, row, Button, Space},
    Alignment, Length,
};

use crate::{
    component::{
        button, card, scrollable,
        text::{p1_bold, p2_regular, Text},
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
