use std::fmt::Display;

use async_hwi::{DeviceKind, Version};
use liana::{descriptors::LianaDescriptor, miniscript::bitcoin::bip32::Fingerprint};
use liana_ui::{
    component::list::{self, DeviceStatus},
    widget::*,
};

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
    let HardwareWallet::Supported {
        kind,
        version,
        fingerprint,
        alias,
        registered,
        ..
    } = hw
    else {
        return unusable_device_entry(hw);
    };

    let unrelated = match &mode {
        HwRowMode::Registration { descriptor, .. } => descriptor
            .map(|d| !d.contains_fingerprint(*fingerprint))
            .unwrap_or(false),
        HwRowMode::Signing { .. } => false,
    };
    let enabled = match &mode {
        HwRowMode::Signing {
            signing, can_sign, ..
        } => *can_sign && !*signing && *registered != Some(false),
        HwRowMode::Registration { processing, .. } => !*processing && !unrelated,
    };
    let select_msg = enabled.then(make_select);

    match mode {
        HwRowMode::Signing {
            signed,
            signing,
            can_sign,
        } => signing_entry(
            kind,
            fingerprint,
            alias.as_ref(),
            *registered,
            signing,
            signed,
            can_sign,
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
            chosen,
            processing,
            complete,
            descriptor.is_some(),
            device_must_support_taproot,
            unrelated,
            select_msg,
        ),
    }
}

/// Render an entry for a device that cannot be used as-is (unsupported, locked).
/// Callers must have ruled out `Supported` already.
pub fn unusable_device_entry<M: 'static + Clone>(hw: &HardwareWallet) -> Element<'static, M> {
    let (fingerprint, kind, status) = match hw {
        HardwareWallet::Supported { .. } => {
            unreachable!("unusable_device_entry called with a Supported device")
        }
        HardwareWallet::Unsupported { kind, reason, .. } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => (
                Some(format!("#{fg}")),
                kind.to_string(),
                DeviceStatus::Unrelated,
            ),
            UnsupportedReason::WrongNetwork => (None, kind.to_string(), DeviceStatus::WrongNetwork),
            UnsupportedReason::Version {
                minimal_supported_version,
            } => (
                None,
                kind.to_string(),
                DeviceStatus::OutdatedFirmware(minimal_supported_version.to_string()),
            ),
            _ => (None, kind.to_string(), DeviceStatus::ConnectionError),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => (
            None,
            kind.to_string(),
            DeviceStatus::Locked(pairing_code.clone()),
        ),
    };
    device_list_row(fingerprint, Some(kind), None::<&str>, status, None)
}

#[allow(clippy::too_many_arguments)]
fn signing_entry<'a, M: Clone + 'static>(
    kind: &'a DeviceKind,
    fingerprint: &'a Fingerprint,
    alias: Option<&'a String>,
    registered: Option<bool>,
    signing: bool,
    signed: bool,
    can_sign: bool,
    select_msg: Option<M>,
) -> Element<'a, M> {
    let (alias, status, on_press) = if signing {
        (alias, DeviceStatus::Processing, None)
    } else if signed {
        (alias, DeviceStatus::Signed, None)
    } else if registered == Some(false) {
        (
            alias,
            DeviceStatus::Warning("The wallet descriptor is not registered on the device.\n You can register it in the settings."),
            None,
        )
    } else if !can_sign {
        (None, DeviceStatus::NotInPath, None)
    } else {
        (alias, DeviceStatus::None, select_msg)
    };
    device_list_row(
        Some(format!("#{fingerprint}")),
        Some(kind),
        alias,
        status,
        on_press,
    )
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
    let (alias, status, on_press) = if unrelated {
        (None, DeviceStatus::Unrelated, None)
    } else if chosen && processing {
        (alias, DeviceStatus::Processing, None)
    } else if complete && has_descriptor {
        (alias, DeviceStatus::Selected, select_msg)
    } else if complete {
        (alias, DeviceStatus::Registered, select_msg)
    } else if not_tapminiscript {
        (
            alias,
            DeviceStatus::Warning("Device firmware version does not support taproot miniscript"),
            select_msg,
        )
    } else {
        (alias, DeviceStatus::None, select_msg)
    };
    device_list_row(
        Some(format!("#{fingerprint}")),
        Some(kind),
        alias,
        status,
        on_press,
    )
}

fn device_list_row<'a, M, F, K, A>(
    fingerprint: Option<F>,
    kind: Option<K>,
    alias: Option<A>,
    status: DeviceStatus,
    on_press: Option<M>,
) -> Element<'a, M>
where
    M: 'static + Clone,
    F: Display + 'a,
    K: Display + 'a,
    A: Display + 'a,
{
    let fingerprint = fingerprint.map(|fingerprint| fingerprint.to_string());
    let kind = kind.map(|kind| kind.to_string());
    let alias = alias.map(|alias| alias.to_string());

    let title = alias.unwrap_or_else(|| " - ".to_string());
    let subtitle = match (kind, fingerprint) {
        (Some(kind), Some(fingerprint)) => Some(format!("{kind} {fingerprint}")),
        (Some(kind), None) => Some(kind),
        (None, Some(fingerprint)) => Some(fingerprint),
        (None, None) => None,
    };

    list::entry_device_list(
        title,
        subtitle,
        status,
        liana_ui::component::button::EntryWidth::Fill,
        on_press,
    )
}
