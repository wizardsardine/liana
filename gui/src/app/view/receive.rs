use std::collections::{HashMap, HashSet};

use iced::{
    widget::{
        qr_code::{self, QRCode},
        scrollable, Space,
    },
    Alignment, Length,
};

use liana::miniscript::bitcoin::{
    self,
    bip32::{ChildNumber, Fingerprint},
    Address,
};

use liana_ui::{
    color,
    component::{
        button, card, form,
        text::{self, *},
    },
    icon, theme,
    util::Collection,
    widget::*,
};

use crate::{
    app::{
        error::Error,
        view::{hw, label, warning::warn},
    },
    hw::HardwareWallet,
};

use super::message::Message;

pub fn receive<'a>(
    addresses: &'a [bitcoin::Address],
    qr: Option<&'a qr_code::State>,
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .align_items(Alignment::Center)
                .push(Container::new(h3("Receive")).width(Length::Fill))
                .push(
                    button::primary(Some(icon::plus_icon()), "Generate address")
                        .on_press(Message::Next),
                ),
        )
        .push(p1_bold("New and never used reception addresses"))
        .push(
            Row::new()
                .spacing(10)
                .push(addresses.iter().enumerate().rev().fold(
                    Column::new().spacing(10).width(Length::Fill),
                    |col, (i, address)| {
                        let addr = address.to_string();
                        col.push(
                            card::simple(
                                Column::new()
                                    .push(if let Some(label) = labels_editing.get(&addr) {
                                        label::label_editing(
                                            vec![addr.clone()],
                                            label,
                                            text::P1_SIZE,
                                        )
                                    } else {
                                        label::label_editable(
                                            vec![addr.clone()],
                                            labels.get(&addr),
                                            text::P1_SIZE,
                                        )
                                    })
                                    .push(
                                        Row::new()
                                            .push(
                                                Container::new(
                                                    scrollable(
                                                        Column::new()
                                                            .push(Space::with_height(
                                                                Length::Fixed(10.0),
                                                            ))
                                                            .push(
                                                                p2_regular(addr)
                                                                    .small()
                                                                    .style(color::GREY_3),
                                                            )
                                                            // Space between the address and the scrollbar
                                                            .push(Space::with_height(
                                                                Length::Fixed(10.0),
                                                            )),
                                                    )
                                                    .horizontal_scroll(
                                                        scrollable::Properties::new()
                                                            .scroller_width(5),
                                                    ),
                                                )
                                                .width(Length::Fill),
                                            )
                                            .push(
                                                Button::new(
                                                    icon::clipboard_icon().style(color::GREY_3),
                                                )
                                                .on_press(Message::Clipboard(address.to_string()))
                                                .style(theme::Button::TransparentBorder),
                                            )
                                            .align_items(Alignment::Center),
                                    )
                                    .push(
                                        button::primary(None, "Verify on hardware device")
                                            .on_press(Message::Select(i)),
                                    )
                                    .spacing(10),
                            )
                            .padding(20),
                        )
                    },
                ))
                .push(if let Some(qr) = qr {
                    Container::new(QRCode::new(qr).cell_size(5))
                        .padding(10)
                        .style(theme::Container::QrCode)
                } else {
                    Container::new(Space::with_width(Length::Fill)).width(Length::Fixed(200.0))
                }),
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
                                        .align_items(Alignment::Center)
                                        .push(
                                            Container::new(text("Address:").bold())
                                                .width(Length::Fill),
                                        )
                                        .push(
                                            Row::new()
                                                .align_items(Alignment::Center)
                                                .push(Container::new(
                                                    text(address.to_string()).small(),
                                                ))
                                                .push(
                                                    Button::new(icon::clipboard_icon())
                                                        .on_press(Message::Clipboard(
                                                            address.to_string(),
                                                        ))
                                                        .style(theme::Button::TransparentBorder),
                                                )
                                                .width(Length::Shrink),
                                        ),
                                )
                                .push(
                                    Row::new()
                                        .width(Length::Fill)
                                        .align_items(Alignment::Center)
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
                .align_items(Alignment::Center),
        ))
        .width(Length::Fill)
        .max_width(750)
        .into()
}
