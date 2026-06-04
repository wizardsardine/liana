use std::collections::HashSet;

use iced::{widget::qr_code, Length};

use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Address,
};

use liana_ui::{
    component::{form, label, modal, panels::receive, text::text},
    widget::*,
};

use crate::{
    app::{
        error::Error,
        view::{hw, warning::warn},
    },
    hw::HardwareWallet,
};

use crate::app::view::message::{LabelMessage, Message, NewAddressMessage};

pub fn verify_address_modal<'a>(
    warning: Option<&Error>,
    hws: &'a [HardwareWallet],
    chosen_hws: &HashSet<Fingerprint>,
    address: &Address,
    derivation_index: &ChildNumber,
) -> Element<'a, Message> {
    let title_row = text("Select device to verify address on:").width(Length::Fill);

    let devices = hws
        .iter()
        .enumerate()
        .fold(Column::new().spacing(10), |col, (i, hw)| {
            col.push(hw::hw_list_view_verify_address(
                i,
                hw,
                if let HardwareWallet::Supported { fingerprint, .. } = hw {
                    chosen_hws.contains(fingerprint)
                } else {
                    false
                },
            ))
        });

    let content = Column::new()
        .push_maybe(warning.map(|w| warn(Some(w))))
        .push(receive::modal::verify_address_modal(
            address,
            derivation_index,
            Message::Clipboard(address.to_string()),
        ))
        .push(title_row)
        .push(devices)
        .spacing(20)
        .width(Length::Fill);
    modal::modal_view(
        Some("Verify address"),
        None,
        Some(Message::Close),
        modal::ModalWidth::XL,
        content,
    )
}

pub fn qr_modal<'a>(qr: &'a qr_code::Data, address: &'a str) -> Element<'a, Message> {
    modal::modal_view(
        Some("Address"),
        None,
        Some(Message::Close),
        modal::ModalWidth::M,
        receive::modal::qr_display(qr, address),
    )
}

pub fn edit_label_modal<'a>(address: &str, value: &'a form::Value<String>) -> Element<'a, Message> {
    let addr = address.to_string();
    let on_change = {
        let addr = addr.clone();
        move |s| Message::Label(vec![addr.clone()], LabelMessage::Edited(s))
    };
    let confirm = Message::Label(vec![addr.clone()], LabelMessage::Confirm);
    let cancel = Message::Label(vec![addr], LabelMessage::Cancel);
    label::edit_label_modal(
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
    label::edit_label_modal(
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
