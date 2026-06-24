use std::fmt::Display;

use async_hwi::{DeviceKind, Version};
use iced::{widget::tooltip::Position, Alignment};
use liana::{descriptors::LianaDescriptor, miniscript::bitcoin::bip32::Fingerprint};
use liana_ui::{
    component::{list, modal::DeviceMark, tooltip_custom},
    icon,
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
    let (fingerprint, kind, mark) = match hw {
        HardwareWallet::Supported { .. } => {
            unreachable!("unusable_device_entry called with a Supported device")
        }
        HardwareWallet::Unsupported { kind, reason, .. } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => (
                Some(format!("#{fg}")),
                kind.to_string(),
                DeviceMark::Unrelated,
            ),
            UnsupportedReason::WrongNetwork => (None, kind.to_string(), DeviceMark::WrongNetwork),
            UnsupportedReason::Version {
                minimal_supported_version,
            } => (
                None,
                kind.to_string(),
                DeviceMark::OutdatedFirmware(minimal_supported_version.to_string()),
            ),
            _ => (None, kind.to_string(), DeviceMark::ConnectionError),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => (
            None,
            kind.to_string(),
            DeviceMark::Locked(pairing_code.clone()),
        ),
    };
    device_list_row(
        fingerprint,
        Some(kind),
        None::<&str>,
        Some(mark),
        None,
        None,
    )
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
    let (alias, mark, warning, on_press) = if signing {
        (alias, Some(DeviceMark::Processing), None, None)
    } else if signed {
        (alias, Some(DeviceMark::Signed), None, None)
    } else if registered == Some(false) {
        (
            alias,
            None,
            Some("The wallet descriptor is not registered on the device.\n You can register it in the settings."),
            None,
        )
    } else if !can_sign {
        (None, Some(DeviceMark::NotInPath), None, None)
    } else {
        (alias, None, None, select_msg)
    };
    device_list_row(
        Some(format!("#{fingerprint}")),
        Some(kind),
        alias,
        mark,
        warning,
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
    let taproot_warning =
        not_tapminiscript.then_some("Device firmware version does not support taproot miniscript");
    let (alias, mark, warning, on_press) = if unrelated {
        (None, Some(DeviceMark::Unrelated), None, None)
    } else if chosen && processing {
        (alias, Some(DeviceMark::Processing), None, None)
    } else if complete && has_descriptor {
        (
            alias,
            Some(DeviceMark::Selected),
            taproot_warning,
            select_msg,
        )
    } else if complete {
        (alias, Some(DeviceMark::Registered), None, select_msg)
    } else if not_tapminiscript {
        (alias, None, taproot_warning, select_msg)
    } else {
        (alias, None, None, select_msg)
    };
    device_list_row(
        Some(format!("#{fingerprint}")),
        Some(kind),
        alias,
        mark,
        warning,
        on_press,
    )
}

fn device_list_row<'a, M, F, K, A>(
    fingerprint: Option<F>,
    kind: Option<K>,
    alias: Option<A>,
    mark: Option<DeviceMark>,
    warning: Option<&'static str>,
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
    let warning = warning.or_else(|| mark.as_ref().and_then(DeviceMark::warning));

    let title = alias
        .or_else(|| kind.clone())
        .unwrap_or_else(|| fingerprint.clone().unwrap_or_else(|| " - ".to_string()));
    let subtitle = match (kind, fingerprint) {
        (Some(kind), Some(fingerprint)) => Some(format!("{kind} {fingerprint}")),
        (Some(kind), None) => Some(kind),
        (None, Some(fingerprint)) => Some(fingerprint),
        (None, None) => None,
    };
    let mark = mark.map(|mark| mark.element());
    let warning =
        warning.map(|warning| tooltip_custom(warning, icon::warning_icon(), Position::Bottom));
    let trailing = if mark.is_some() || warning.is_some() {
        Some(
            Row::new()
                .push_maybe(mark)
                .push_maybe(warning)
                .align_y(Alignment::Center)
                .spacing(10)
                .into(),
        )
    } else {
        None
    };

    list::entry_device_list(
        title,
        subtitle,
        trailing,
        liana_ui::component::button::EntryWidth::Fill,
        on_press,
    )
}
