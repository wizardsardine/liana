pub mod modal;

use std::fmt::Display;

use iced::{
    widget::{column, row, Button, Space},
    Alignment,
};

use crate::{
    component::{
        address::copyable_address_with_style,
        button::{btn_show_qr_compact, btn_verify_compact},
        card, label,
        text::new,
    },
    icon, theme,
    widget::{Element, SpaceExt},
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
    let addr_row = copyable_address_with_style(address, clipboard, theme::button::secondary);
    let top = column![label, addr_row];

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
