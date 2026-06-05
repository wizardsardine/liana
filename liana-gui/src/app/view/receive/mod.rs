mod modals;
pub use modals::{
    edit_label_modal, new_address_label_modal, new_address_processing_modal,
    new_address_show_modal, qr_modal, verify_address_modal,
};

use std::collections::HashMap;

use iced::{widget::row, Alignment, Length};

use liana::miniscript::bitcoin;

use liana_ui::{
    component::{button, form, list, panels::receive, text::new},
    icon,
    widget::*,
};

use crate::app::menu::Menu;

use super::message::Message;

fn address_card<'a>(
    row_index: usize,
    address: &'a bitcoin::Address,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    let addr = address.to_string();
    let label = labels_editing
        .get(&addr)
        .map(|l| l.value.clone())
        .or_else(|| labels.get(&addr).cloned())
        .unwrap_or_default();
    receive::address_card(
        label,
        address,
        Message::Label(vec![addr.clone()], super::LabelMessage::Edit),
        Message::Clipboard(addr),
        Message::Select(row_index),
        Message::ShowAddressQrCode(super::AddressQrSource::Row(row_index)),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn receive<'a>(
    prev_addresses: &'a [bitcoin::Address],
    prev_labels: &'a HashMap<String, String>,
    show_prev_addresses: bool,
    labels_editing: &'a HashMap<String, form::Value<String>>,
    is_last_page: bool,
    processing: bool,
) -> Element<'a, Message> {
    let title = Container::new(new::d2(Menu::Receive.title())).width(Length::Fill);
    let generate = {
        let (icon, label) = (Some(icon::plus_icon()), "Generate address");
        if prev_addresses.is_empty() {
            button::primary(icon, label)
        } else {
            button::secondary(icon, label)
        }
        .on_press(Message::NextReceiveAddress)
    };
    let header = row![title, generate].align_y(Alignment::Center);

    let description = new::b1("Always generate a new address for each deposit.");

    let prev_header = (!prev_addresses.is_empty()).then_some(receive::previous_addresses_header(
        show_prev_addresses,
        Message::ToggleShowPreviousAddresses,
    ));

    // prev addresses are already ordered in descending order
    let cards = show_prev_addresses.then(|| {
        prev_addresses.iter().enumerate().fold(
            Column::new().spacing(14).width(Length::Fill),
            |col, (i, address)| col.push(address_card(i, address, prev_labels, labels_editing)),
        )
    });

    let see_more =
        (!is_last_page && show_prev_addresses).then(|| list::see_more(processing, Message::Next));

    Column::new()
        .push(header)
        .push(description)
        .push_maybe(prev_header)
        .push_maybe(cards)
        .push_maybe(see_more)
        .spacing(20)
        .into()
}
