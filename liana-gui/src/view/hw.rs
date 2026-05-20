use async_hwi::{DeviceKind, Version};
use liana::{descriptors::LianaDescriptor, miniscript::bitcoin::bip32::Fingerprint};
use liana_ui::{component::modal::legacy, widget::*};

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
    let unrelated = match (&mode, hw) {
        (
            HwRowMode::Registration { descriptor, .. },
            HardwareWallet::Supported { fingerprint, .. },
        ) => descriptor
            .map(|d| !d.contains_fingerprint(*fingerprint))
            .unwrap_or(false),
        _ => false,
    };
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
    let select_msg = if enabled { Some(make_select()) } else { None };

    match hw {
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
                select_msg,
            ),
            HwRowMode::Registration {
                chosen,
                processing,
                complete,
                descriptor,
                device_must_support_taproot,
            } => registration_entry(
                kind,
                version.as_ref(),
                fingerprint,
                alias.as_ref(),
                *chosen,
                *processing,
                *complete,
                descriptor.is_some(),
                *device_must_support_taproot,
                unrelated,
                select_msg,
            ),
        },
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => {
                legacy::unrelated_device(kind.to_string(), version.as_ref(), fg, None)
            }
            UnsupportedReason::WrongNetwork => {
                legacy::wrong_network_device(kind.to_string(), version.as_ref(), None)
            }
            UnsupportedReason::Version {
                minimal_supported_version,
            } => legacy::unsupported_version_device(
                kind.to_string(),
                version.as_ref(),
                minimal_supported_version,
                None,
            ),
            _ => legacy::unsupported_device(kind.to_string(), version.as_ref(), None),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => legacy::locked_device(kind, pairing_code.as_ref(), None),
    }
}

#[allow(clippy::too_many_arguments)]
fn signing_entry<'a, M: Clone + 'static>(
    kind: &'a DeviceKind,
    version: Option<&'a Version>,
    fingerprint: &'a Fingerprint,
    alias: Option<&'a String>,
    registered: Option<bool>,
    signing: bool,
    signed: bool,
    can_sign: bool,
    select_msg: Option<M>,
) -> Element<'a, M> {
    if signing {
        legacy::processing_device(kind, version, fingerprint, alias, None)
    } else if signed {
        legacy::signed_device(kind, version, fingerprint, alias, None)
    } else if registered == Some(false) {
        legacy::warning_device(
            kind,
            version,
            fingerprint,
            alias,
            "The wallet descriptor is not registered on the device.\n You can register it in the settings.",
            None,
        )
    } else if !can_sign {
        legacy::disabled_device(
            kind,
            version,
            fingerprint,
            "This signing device is not part of this spending path.",
            None,
        )
    } else {
        legacy::supported_device(kind, version, fingerprint, alias, select_msg)
    }
}

#[allow(clippy::too_many_arguments)]
fn registration_entry<'a, M: Clone + 'static>(
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
    select_msg: Option<M>,
) -> Element<'a, M> {
    let not_tapminiscript =
        device_must_support_taproot && !is_compatible_with_tapminiscript(kind, version);
    if unrelated {
        legacy::unrelated_device(kind.to_string(), version, fingerprint, None)
    } else if chosen && processing {
        legacy::processing_device(kind, version, fingerprint, alias, None)
    } else if complete {
        if has_descriptor {
            legacy::selected_device(
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
                select_msg,
            )
        } else {
            legacy::registered_device(kind, version, fingerprint, alias, select_msg)
        }
    } else if not_tapminiscript {
        legacy::warning_device(
            kind,
            version,
            fingerprint,
            alias,
            "Device firmware version does not support taproot miniscript",
            select_msg,
        )
    } else {
        legacy::supported_device(kind, version, fingerprint, alias, select_msg)
    }
}
