use iced::Length;

use coincube_ui::{
    component::hw,
    theme,
    widget::{Button, Column, Element},
};

use crate::{
    app::view::message::*,
    hw::{HardwareWallet, UnsupportedReason},
    hw_advisory,
};
use async_hwi::DeviceKind;

/// Attach the advisory to a rendered device row: an amber badge, and — until
/// the user collapses it — an expandable panel with the tiered copy and a link
/// to the rotation guide.
///
/// The row itself is passed through untouched. Nothing here gates selection,
/// signing or registration; an advisory only ever adds information.
fn with_advisory<'a>(hw: &HardwareWallet, row: Element<'a, Message>) -> Element<'a, Message> {
    let Some(hit) = hw_advisory::view::hit(hw) else {
        return row;
    };
    // Dismissal is keyed by fingerprint, so the control is only offered for
    // devices that reported one; the rest keep the panel expandable.
    let fingerprint = hw.fingerprint();
    let advisory = hw_advisory::view::section(
        &hit,
        fingerprint,
        Message::OpenUrl(hit.url().to_string()),
        fingerprint.map(|fingerprint| Message::DismissHwAdvisory(fingerprint, hit.id())),
    );
    Column::new().push(row).push(advisory).into()
}

pub fn hw_list_view(
    i: usize,
    hw: &HardwareWallet,
    signed: bool,
    signing: bool,
    can_sign: bool,
) -> Element<'_, Message> {
    let mut bttn = Button::new(match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            registered,
            ..
        } => {
            if signing {
                hw::processing_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else if signed {
                hw::sign_success_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                )
            } else if *registered == Some(false) {
                hw::warning_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                    "The wallet descriptor is not registered on the device.\n You can register it in the settings.",
                )
            } else if !can_sign {
                hw::disabled_hardware_wallet(kind, version.as_ref(), fingerprint, "This signing device is not part of this spending path.")
            } else {
                hw::supported_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            }
        }
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => {
                hw::unrelated_hardware_wallet(kind.to_string(), version.as_ref(), fg)
            }
            UnsupportedReason::WrongNetwork => {
                hw::wrong_network_hardware_wallet(kind.to_string(), version.as_ref())
            }
            UnsupportedReason::Version {
                minimal_supported_version,
                note,
            } => hw::unsupported_version_hardware_wallet(
                kind.to_string(),
                version.as_ref(),
                minimal_supported_version,
                *note,
            ),
            _ => hw::unsupported_hardware_wallet(kind.to_string(), version.as_ref()),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .width(Length::Fill);
    // While signing, the row is intentionally not clickable (no `on_press`),
    // but it shouldn't read as disabled/greyed — the user is actively being
    // asked to confirm on the device. Force the active style so the
    // "Processing… / Please check your device" prompt stays legible.
    bttn = if signing {
        bttn.style(|theme, _status| {
            theme::button::secondary(theme, iced::widget::button::Status::Active)
        })
    } else {
        bttn.style(theme::button::secondary)
    };
    if can_sign && !signing {
        if let HardwareWallet::Supported { registered, .. } = hw {
            if *registered != Some(false) {
                bttn = bttn.on_press(Message::SelectHardwareWallet(i));
            }
        }
    }
    with_advisory(hw, bttn.into())
}

pub fn hw_list_view_for_registration(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    registered: bool,
) -> Element<'_, Message> {
    let mut bttn = Button::new(match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            ..
        } => {
            if chosen && processing {
                hw::processing_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else if registered {
                hw::registration_success_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                )
            } else {
                hw::supported_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            }
        }
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => {
                hw::unrelated_hardware_wallet(kind.to_string(), version.as_ref(), fg)
            }
            UnsupportedReason::WrongNetwork => {
                hw::wrong_network_hardware_wallet(kind.to_string(), version.as_ref())
            }
            UnsupportedReason::Version {
                minimal_supported_version,
                note,
            } => hw::unsupported_version_hardware_wallet(
                kind.to_string(),
                version.as_ref(),
                minimal_supported_version,
                *note,
            ),
            _ => hw::unsupported_hardware_wallet(kind.to_string(), version.as_ref()),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .style(theme::button::secondary)
    .width(Length::Fill);
    if !processing && hw.is_supported() {
        bttn = bttn.on_press(Message::SelectHardwareWallet(i));
    }
    with_advisory(hw, bttn.into())
}

pub fn hw_list_view_verify_address(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
) -> Element<'_, Message> {
    let (content, selectable) = match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            ..
        } => {
            if chosen {
                (
                    hw::processing_hardware_wallet(
                        kind,
                        version.as_ref(),
                        fingerprint,
                        alias.as_ref(),
                    ),
                    false,
                )
            } else {
                match kind {
                    DeviceKind::Specter | DeviceKind::SpecterSimulator => {
                        (hw::unimplemented_method_hardware_wallet(
                            kind.to_string(),
                            version.as_ref(),
                            fingerprint,
                            "Tenshu cannot request the device to display the address. \n The verification must be done manually with the device control."
                        ), false)
                    }
                    _ => (hw::supported_hardware_wallet(
                        kind,
                        version.as_ref(),
                        fingerprint,
                        alias.as_ref(),
                    ), true),
                }
            }
        }
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => (
            match reason {
                UnsupportedReason::NotPartOfWallet(fg) => {
                    hw::unrelated_hardware_wallet(kind.to_string(), version.as_ref(), fg)
                }
                UnsupportedReason::WrongNetwork => {
                    hw::wrong_network_hardware_wallet(kind.to_string(), version.as_ref())
                }
                UnsupportedReason::Version {
                    minimal_supported_version,
                    note,
                } => hw::unsupported_version_hardware_wallet(
                    kind.to_string(),
                    version.as_ref(),
                    minimal_supported_version,
                    *note,
                ),
                _ => hw::unsupported_hardware_wallet(kind.to_string(), version.as_ref()),
            },
            false,
        ),
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => (
            hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
            false,
        ),
    };
    let mut bttn = Button::new(content)
        .style(theme::button::secondary)
        .width(Length::Fill);
    if selectable && hw.is_supported() {
        bttn = bttn.on_press(Message::SelectHardwareWallet(i));
    }
    with_advisory(hw, bttn.into())
}
