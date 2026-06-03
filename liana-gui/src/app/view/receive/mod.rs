mod modals;
pub use modals::{
    edit_label_modal, new_address_label_modal, new_address_show_modal, qr_modal,
    verify_address_modal,
};

use std::collections::HashMap;

use iced::{alignment::Horizontal, widget::Button, Alignment, Length};

use liana::miniscript::bitcoin;

use liana_ui::{
    component::{button, form, panels::receive, text::legacy},
    icon, theme,
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
        Message::ShowQrCode(row_index),
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
    Column::new()
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(
                    Container::new(legacy::panel_title(Menu::Receive.title())).width(Length::Fill),
                )
                .push({
                    let (icon, label) = (Some(icon::plus_icon()), "Generate address");
                    if prev_addresses.is_empty() {
                        button::primary(icon, label)
                    } else {
                        button::secondary(icon, label)
                    }
                    .on_press(Message::NextReceiveAddress)
                }),
        )
        .push(legacy::text(
            "Always generate a new address for each deposit.",
        ))
        .push_maybe(
            (!prev_addresses.is_empty()).then_some(receive::previous_addresses_header(
                show_prev_addresses,
                Message::ToggleShowPreviousAddresses,
            )),
        )
        .push_maybe(show_prev_addresses.then_some(Row::new().spacing(10).push(
            prev_addresses.iter().enumerate().fold(
                // prev addresses are already ordered in descending order
                Column::new().spacing(10).width(Length::Fill),
                |col, (i, address)| col.push(address_card(i, address, prev_labels, labels_editing)),
            ),
        )))
        .push_maybe(
            (!is_last_page && show_prev_addresses).then_some(
                Container::new(
                    Button::new(
                        legacy::text(if processing {
                            "Fetching ..."
                        } else {
                            "See more"
                        })
                        .width(Length::Fill)
                        .align_x(Horizontal::Center),
                    )
                    .width(Length::Fill)
                    .padding(15)
                    .style(theme::button::transparent_border)
                    .on_press_maybe((!processing).then_some(Message::Next)),
                )
                .width(Length::Fill)
                .style(theme::card::simple),
            ),
        )
        .spacing(20)
        .into()
}
