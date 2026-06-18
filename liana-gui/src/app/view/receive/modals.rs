use std::collections::HashSet;

use iced::{
    widget::{qr_code, row, Space},
    Length,
};

use liana::miniscript::bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Address,
};

use liana_ui::{
    component::{
        button::btn_show_qr_section,
        form, label,
        modal::{self, modal_no_devices_placeholder, optional_section},
        panels::receive,
        text::text,
    },
    widget::*,
};

use crate::{
    app::{
        error::Error,
        view::{hw, warning::warn},
    },
    hw::HardwareWallet,
};

use crate::app::view::message::{AddressQrSource, LabelMessage, Message, NewAddressMessage};

pub fn verify_address_modal<'a>(
    warning: Option<&Error>,
    hws: &'a [HardwareWallet],
    chosen_hws: &HashSet<Fingerprint>,
    address: &Address,
    derivation_index: ChildNumber,
    qr_section_open: bool,
) -> Element<'a, Message> {
    let mut devices = Column::new().spacing(10);
    if hws.is_empty() {
        devices = devices.push(row![
            Space::fill_width(),
            modal_no_devices_placeholder(),
            Space::fill_width()
        ]);
    } else {
        for (i, hw) in hws.iter().enumerate() {
            devices = devices.push(hw::hw_list_view_verify_address(
                i,
                hw,
                if let HardwareWallet::Supported { fingerprint, .. } = hw {
                    chosen_hws.contains(fingerprint)
                } else {
                    false
                },
            ));
        }
    }
    devices = devices.push(optional_section(
        qr_section_open,
        "Other options".to_string(),
        || Message::ShowQrOptSection(true),
        || Message::ShowQrOptSection(false),
    ));
    if qr_section_open {
        devices = devices.push(btn_show_qr_section(
            Some("For specter DIY devices"),
            Some(Message::ShowAddressQrCode(AddressQrSource::WithIndex(
                address.clone(),
                derivation_index,
            ))),
        ));
    }

    let content = Column::new()
        .push_maybe(warning.map(|w| warn(Some(w))))
        .push(receive::modal::verify_address_modal(
            address,
            &derivation_index,
            Message::Clipboard(address.to_string()),
        ))
        .push(text("Select device to verify address on:").width(Length::Fill))
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
        modal::ModalWidth::L,
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

pub fn new_address_show_modal<'a>(address: &Address, bip21: Option<&str>) -> Element<'a, Message> {
    // For a payjoin address, display and copy the bip21 URI; otherwise the plain address.
    let (display, clipboard): (String, String) = if let Some(bip21) = bip21 {
        (bip21.to_string(), bip21.to_string())
    } else {
        let s = address.to_string();
        (s.clone(), s)
    };
    receive::modal::show_address_modal(
        display,
        Message::NewAddress(NewAddressMessage::Close),
        Message::NewAddress(NewAddressMessage::Verify),
        Message::NewAddress(NewAddressMessage::ShowQr),
        Message::Clipboard(clipboard),
    )
}

pub fn new_address_processing_modal<'a>() -> Element<'a, Message> {
    modal::modal_view(
        Some("Generating address"),
        None,
        None,
        modal::ModalWidth::M,
        Container::new(text("Generating address ...")).center_x(Length::Fill),
    )
}
