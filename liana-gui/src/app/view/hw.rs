use liana_ui::{
    component::{list::DeviceStatus, modal},
    widget::*,
};

use crate::{app::view::message::*, hw::HardwareWallet, t, view::hw::unusable_device_entry};
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
    let (status, on_press) = if chosen {
        (DeviceStatus::Processing, None)
    } else if matches!(kind, DeviceKind::Specter | DeviceKind::SpecterSimulator) {
        (
            DeviceStatus::Warning(t!("hw-display-address-unavailable")),
            None,
        )
    } else {
        (DeviceStatus::None, Some(Message::SelectHardwareWallet(i)))
    };
    modal::device_entry(
        Some(format!("#{fingerprint}")),
        Some(kind),
        alias.as_ref(),
        status,
        on_press,
    )
}
