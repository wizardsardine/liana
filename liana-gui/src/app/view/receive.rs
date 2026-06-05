use std::collections::{HashMap, HashSet};

use iced::{
    alignment::Horizontal,
    widget::{
        qr_code::{self, QRCode},
        Button, Space,
    },
    Alignment, Length,
};

use liana::miniscript::bitcoin::{
    self,
    bip32::{ChildNumber, Fingerprint},
    Address,
};

use liana_ui::{
    component::{
        button, card, form, label as ui_label, panels::receive,
        scrollable,
        text::{self, *},
    },
    icon, theme,
    widget::*,
};

use crate::{
    app::{
        error::Error,
        menu::Menu,
        view::{hw, label, warning::warn},
    },
    hw::HardwareWallet,
};

use super::message::{LabelMessage, Message, NewAddressMessage};

fn address_card<'a>(
    row_index: usize,
    address: &'a bitcoin::Address,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Container<'a, Message> {
    let addr = address.to_string();
    card::simple(
        Column::new()
            .push(if let Some(label) = labels_editing.get(&addr) {
                label::label_editing(vec![addr.clone()], label, text::P1_SIZE)
            } else {
                label::label_editable(vec![addr.clone()], labels.get(&addr), text::P1_SIZE)
            })
            .push(
                Row::new()
                    .push(
                        Container::new(scrollable::horizontal_thin(
                            Column::new()
                                .push(Space::with_height(Length::Fixed(10.0)))
                                .push(p2_regular(address).small().style(theme::text::secondary)),
                        ))
                        .width(Length::Fill),
                    )
                    .push(
                        Button::new(icon::clipboard_icon().style(theme::text::secondary))
                            .on_press(Message::Clipboard(addr))
                            .style(theme::button::transparent_border),
                    )
                    .align_y(Alignment::Center),
            )
            .push(
                Row::new()
                    .push(
                        button::secondary(None, "Verify on hardware device")
                            .on_press(Message::Select(row_index)),
                    )
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::secondary(None, "Show QR Code")
                            .on_press(Message::ShowQrCode(row_index)),
                    ),
            )
            .spacing(10),
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
                .push(Container::new(panel_title(Menu::Receive.title())).width(Length::Fill))
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
        .push(text("Always generate a new address for each deposit."))
        .push_maybe(
            (!prev_addresses.is_empty()).then_some(receive::previous_addresses_header(
                show_prev_addresses,
                Message::ToggleShowPreviousAddresses,
            )),
        )
        .push_maybe(show_prev_addresses.then_some(Row::new().spacing(10).push(
            prev_addresses.iter().enumerate().fold(
                Column::new().spacing(10).width(Length::Fill),
                |col, (i, address)| {
                    col.push(address_card(i, address, prev_labels, labels_editing))
                },
            ),
        )))
        .push_maybe(
            (!is_last_page && show_prev_addresses).then_some(
                Container::new(
                    Button::new(
                        text(if processing {
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

pub fn verify_address_modal<'a>(
    warning: Option<&Error>,
    hws: &'a [HardwareWallet],
    chosen_hws: &HashSet<Fingerprint>,
    address: &Address,
    derivation_index: &ChildNumber,
) -> Element<'a, Message> {
    Column::new()
        .push_maybe(warning.map(|w| warn(Some(w))))
        .push(card::simple(
            Column::new()
                .push(
                    Column::new()
                        .push(
                            Column::new()
                                .push(
                                    Row::new()
                                        .width(Length::Fill)
                                        .align_y(Alignment::Center)
                                        .push(
                                            Container::new(text("Address:").bold())
                                                .width(Length::Fill),
                                        )
                                        .push(
                                            Row::new()
                                                .align_y(Alignment::Center)
                                                .push(Container::new(
                                                    text(address.to_string()).small(),
                                                ))
                                                .push(
                                                    Button::new(icon::clipboard_icon())
                                                        .on_press(Message::Clipboard(
                                                            address.to_string(),
                                                        ))
                                                        .style(theme::button::transparent_border),
                                                )
                                                .width(Length::Shrink),
                                        ),
                                )
                                .push(
                                    Row::new()
                                        .width(Length::Fill)
                                        .align_y(Alignment::Center)
                                        .push(
                                            Container::new(text("Derivation index:").bold())
                                                .width(Length::Fill),
                                        )
                                        .push(
                                            Container::new(
                                                text(derivation_index.to_string()).small(),
                                            )
                                            .width(Length::Shrink),
                                        ),
                                )
                                .spacing(5),
                        )
                        .push(text("Select device to verify address on:").width(Length::Fill))
                        .spacing(10)
                        .push(hws.iter().enumerate().fold(
                            Column::new().spacing(10),
                            |col, (i, hw)| {
                                col.push(hw::hw_list_view_verify_address(
                                    i,
                                    hw,
                                    if let HardwareWallet::Supported { fingerprint, .. } = hw {
                                        chosen_hws.contains(fingerprint)
                                    } else {
                                        false
                                    },
                                ))
                            },
                        ))
                        .width(Length::Fill),
                )
                .spacing(20)
                .width(Length::Fill)
                .align_x(Alignment::Center),
        ))
        .width(Length::Fill)
        .max_width(750)
        .into()
}

pub fn qr_modal<'a>(qr: &'a qr_code::Data, address: &'a String) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .push(Space::with_width(Length::Fill))
                .push(
                    Container::new(QRCode::<liana_ui::theme::Theme>::new(qr).cell_size(8))
                        .padding(10),
                )
                .push(Space::with_width(Length::Fill)),
        )
        .push(Space::with_height(Length::Fixed(15.0)))
        .push(Container::new(text(address).size(15)).center_x(Length::Fill))
        .width(Length::Fill)
        .max_width(400)
        .into()
}

pub fn edit_label_modal<'a>(address: &str, value: &'a form::Value<String>) -> Element<'a, Message> {
    let addr = address.to_string();
    let on_change = {
        let addr = addr.clone();
        move |s| Message::Label(vec![addr.clone()], LabelMessage::Edited(s))
    };
    let confirm = Message::Label(vec![addr.clone()], LabelMessage::Confirm);
    let cancel = Message::Label(vec![addr], LabelMessage::Cancel);
    ui_label::edit_label_modal(
        "Edit label",
        "Enter an address label",
        value,
        on_change,
        confirm,
        cancel,
        false,
    )
}

pub fn new_address_label_modal<'a>(value: &'a form::Value<String>) -> Element<'a, Message> {
    ui_label::edit_label_modal(
        "Label",
        "Enter an address label",
        value,
        |s| Message::NewAddress(NewAddressMessage::LabelEdited(s)),
        Message::NewAddress(NewAddressMessage::Confirm),
        Message::NewAddress(NewAddressMessage::Close),
        true,
    )
}

pub fn new_address_show_modal<'a>(address: &Address) -> Element<'a, Message> {
    receive::modal::show_address_modal(
        address,
        Message::NewAddress(NewAddressMessage::Close),
        Message::NewAddress(NewAddressMessage::Verify),
        Message::NewAddress(NewAddressMessage::ShowQr),
        Message::Clipboard(address.to_string()),
    )
}
