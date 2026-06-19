use liana_ui::{
    component::modal::{self, DeviceMark},
    widget::*,
};

use crate::{app::view::message::*, hw::HardwareWallet, view::hw::unusable_device_entry};
use async_hwi::DeviceKind;

pub fn hw_list_view_verify_address(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
) -> Element<'_, Message> {
    let HardwareWallet::Supported {
        kind,
        fingerprint,
        alias,
        ..
    } = hw
    else {
        return unusable_device_entry(hw);
    };
    let (mark, warning, on_press) = if chosen {
        (Some(DeviceMark::Processing), None, None)
    } else if matches!(kind, DeviceKind::Specter | DeviceKind::SpecterSimulator) {
        (
            None,
            Some("Liana cannot request the device to display the address. \n Verify it with the QR code in the options below."),
            None,
        )
    } else {
        (None, None, Some(Message::SelectHardwareWallet(i)))
    };
    modal::device_entry(
        Some(format!("#{fingerprint}")),
        Some(kind),
        alias.as_ref(),
        mark,
        warning,
        on_press,
    )
}
