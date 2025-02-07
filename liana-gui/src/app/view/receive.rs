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
    component::{
        button, card, form,
        text::{self, *},
    },
    icon, theme,
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
    labels: &'a HashMap<String, String>,
    labels_editing: &'a HashMap<String, form::Value<String>>,
) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(Container::new(h3("Receive")).width(Length::Fill))
                .push(
                    button::secondary(Some(icon::plus_icon()), "Generate address")
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
                                                                    .style(theme::text::secondary),
                                                            )
                                                            // Space between the address and the scrollbar
                                                            .push(Space::with_height(
                                                                Length::Fixed(10.0),
                                                            )),
                                                    )
                                                    .direction(scrollable::Direction::Horizontal(
                                                        scrollable::Scrollbar::new()
                                                            .width(2)
                                                            .scroller_width(2),
                                                    )),
                                                )
                                                .width(Length::Fill),
                                            )
                                            .push(
                                                Button::new(
                                                    icon::clipboard_icon()
                                                        .style(theme::text::secondary),
                                                )
                                                .on_press(Message::Clipboard(address.to_string()))
                                                .style(theme::button::transparent_border),
                                            )
                                            .align_y(Alignment::Center),
                                    )
                                    .push(
                                        Row::new()
                                            .push(
                                                button::secondary(
                                                    None,
                                                    "Verify on hardware device",
                                                )
                                                .on_press(Message::Select(i)),
                                            )
                                            .push(Space::with_width(Length::Fill))
                                            .push(
                                                button::secondary(None, "Show QR Code")
                                                    .on_press(Message::ShowQrCode(i)),
                                            ),
                                    )
                                    .spacing(10),
                            )
                            .padding(20),
                        )
                    },
                )),
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
