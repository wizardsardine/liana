use std::collections::HashSet;

use iced::{widget::qr_code, Alignment, Length};

use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Address,
};

use liana_ui::{
    component::{card, panels::receive, text::text},
    widget::*,
};

use crate::{
    app::{
        error::Error,
        view::{hw, warning::warn},
    },
    hw::HardwareWallet,
};

use crate::app::view::message::Message;

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
                        .push(receive::verify_address_modal(
                            address,
                            derivation_index,
                            Message::Clipboard(address.to_string()),
                        ))
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

pub fn qr_modal<'a>(qr: &'a qr_code::Data, address: &'a str) -> Element<'a, Message> {
    receive::qr_display(qr, address)
}
