use async_hwi::{DeviceKind, Version};
use iced::Length;
use liana::{descriptors::LianaDescriptor, miniscript::bitcoin::bip32::Fingerprint};
use liana_ui::{component::hw, theme, widget::*};

use crate::hw::{is_compatible_with_tapminiscript, HardwareWallet, UnsupportedReason};

/// What workflow is asking for a hardware-wallet row, and what state it's in.
pub enum HwRowMode<'a> {
    /// PSBT signing flow.
    Signing {
        signed: bool,
        signing: bool,
        /// Fingerprint participates in the relevant spending path.
        can_sign: bool,
    },
    /// Descriptor registration
    Registration {
        chosen: bool,
        processing: bool,
        complete: bool,
        descriptor: Option<&'a LianaDescriptor>,
        device_must_support_taproot: bool,
    },
}

/// Render one signer entry for the signing or registration flow.
pub fn device_list_entry<'a, M, F>(
    hw: &'a HardwareWallet,
    mode: HwRowMode<'a>,
    make_select: F,
) -> Element<'a, M>
where
    M: Clone + 'static,
    F: FnOnce() -> M + 'a,
{
    let mut unrelated = false;
    let inner = match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            registered,
            ..
        } => match &mode {
            HwRowMode::Signing {
                signing,
                signed,
                can_sign,
            } => signing_entry(
                kind,
                version.as_ref(),
                fingerprint,
                alias.as_ref(),
                *registered,
                *signing,
                *signed,
                *can_sign,
            ),
            HwRowMode::Registration {
                chosen,
                processing,
                complete,
                descriptor,
                device_must_support_taproot,
            } => {
                let device_in_descriptor = descriptor
                    .map(|d| d.contains_fingerprint(*fingerprint))
                    .unwrap_or(true);
                if !device_in_descriptor {
                    unrelated = true;
                }
                registration_entry(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                    *chosen,
                    *processing,
                    *complete,
                    descriptor.is_some(),
                    *device_must_support_taproot,
                    !device_in_descriptor,
                )
            }
        },
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
            } => hw::unsupported_version_hardware_wallet(
                kind.to_string(),
                version.as_ref(),
                minimal_supported_version,
            ),
            _ => hw::unsupported_hardware_wallet(kind.to_string(), version.as_ref()),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    };

    let mut bttn = Button::new(inner)
        .style(theme::button::secondary)
        .width(Length::Fill);

    let enabled = match &mode {
        HwRowMode::Signing {
            signing, can_sign, ..
        } => {
            *can_sign
                && !*signing
                && hw.is_supported()
                && match hw {
                    HardwareWallet::Supported { registered, .. } => *registered != Some(false),
                    _ => true,
                }
        }
        HwRowMode::Registration { processing, .. } => {
            !*processing && hw.is_supported() && !unrelated
        }
    };

    if enabled {
        bttn = bttn.on_press(make_select());
    }
    bttn.into()
}

#[allow(clippy::too_many_arguments)]
fn signing_entry<'a, T: 'static + Clone>(
    kind: &'a DeviceKind,
    version: Option<&'a Version>,
    fingerprint: &'a Fingerprint,
    alias: Option<&'a String>,
    registered: Option<bool>,
    signing: bool,
    signed: bool,
    can_sign: bool,
) -> Container<'a, T> {
    if signing {
        hw::processing_hardware_wallet(kind, version, fingerprint, alias)
    } else if signed {
        hw::sign_success_hardware_wallet(kind, version, fingerprint, alias)
    } else if registered == Some(false) {
        hw::warning_hardware_wallet(
            kind,
            version,
            fingerprint,
            alias,
            "The wallet descriptor is not registered on the device.\n You can register it in the settings.",
        )
    } else if !can_sign {
        hw::disabled_hardware_wallet(
            kind,
            version,
            fingerprint,
            "This signing device is not part of this spending path.",
        )
    } else {
        hw::supported_hardware_wallet(kind, version, fingerprint, alias)
    }
}

#[allow(clippy::too_many_arguments)]
fn registration_entry<'a, T: 'static>(
    kind: &'a DeviceKind,
    version: Option<&'a Version>,
    fingerprint: &'a Fingerprint,
    alias: Option<&'a String>,
    chosen: bool,
    processing: bool,
    complete: bool,
    has_descriptor: bool,
    device_must_support_taproot: bool,
    unrelated: bool,
) -> Container<'a, T> {
    let not_tapminiscript =
        device_must_support_taproot && !is_compatible_with_tapminiscript(kind, version);
    if unrelated {
        hw::unrelated_hardware_wallet(kind.to_string(), version, fingerprint)
    } else if chosen && processing {
        hw::processing_hardware_wallet(kind, version, fingerprint, alias)
    } else if complete {
        if has_descriptor {
            hw::selected_hardware_wallet(
                kind,
                version,
                fingerprint,
                alias,
                if not_tapminiscript {
                    Some("Device firmware version does not support taproot miniscript")
                } else {
                    None
                },
                None,
                false,
            )
        } else {
            hw::registration_success_hardware_wallet(kind, version, fingerprint, alias)
        }
    } else if not_tapminiscript {
        hw::warning_hardware_wallet(
            kind,
            version,
            fingerprint,
            alias,
            "Device firmware version does not support taproot miniscript",
        )
    } else {
        hw::supported_hardware_wallet(kind, version, fingerprint, alias)
    }
}
