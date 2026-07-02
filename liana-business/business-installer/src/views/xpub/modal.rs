use crate::state::{
    message::HardwareWalletRequestId,
    views::{ModalStep, XpubEntryModalState, XpubInputSource},
    Msg, State,
};
use async_hwi::service::SigningDevice;
use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana_gui::hw::{is_compatible_with_tapminiscript, min_taproot_version, UnsupportedReason};
use liana_ui::{
    component::{
        badge::Tile,
        button::{self, btn_cancel, btn_clear, btn_retry, btn_save},
        card, form, list,
        modal::{self, modal_view, ModalWidth},
        scrollable,
        text::{self, capitalize_first, truncate},
        tooltip,
    },
    icon, theme,
    widget::*,
};

pub fn xpub_modal_view(state: &State) -> Option<Element<'_, Msg>> {
    let modal_state = state.views.xpub.modal.as_ref()?;

    Some(match modal_state.step {
        ModalStep::Select => select_view(state, modal_state),
        ModalStep::Details => {
            let selected_device = modal_state.selected_device?;
            let device_list = state.hw.list();
            let mut connected = false;
            for device in device_list.values() {
                if device.fingerprint() == Some(selected_device) {
                    if matches!(device, SigningDevice::Supported(..)) {
                        connected = true;
                    } else {
                        return None;
                    }
                }
            }
            if !connected {
                return None;
            }
            details_view(modal_state)
        }
    })
}

fn select_view<'a>(state: &'a State, modal_state: &'a XpubEntryModalState) -> Element<'a, Msg> {
    let xpub_status: Option<Element<'_, Msg>> = modal_state.current_xpub.is_some().then_some(
        card::info(
            "This key already has an xpub. You can replace it by fetching from a device, importing from file, or pasting."
        )
        .width(Length::Fill)
        .into(),
    );

    let input_display: Option<Element<'_, Msg>> = (!modal_state.xpub_input.is_empty()).then_some({
        column![
            text::new::b5_bold("Current xpub").style(theme::text::primary),
            xpub_box(&modal_state.xpub_input)
        ]
        .spacing(8)
        .into()
    });

    let validation_error = validation_error(modal_state);

    let detected_devices = row![
        text::new::b5_bold("Detected devices").style(theme::text::primary),
        Space::fill_width()
    ];

    let body = column![
        xpub_status,
        detected_devices,
        hw_section(state),
        input_display,
        other_options(modal_state),
        validation_error,
        select_footer_buttons(modal_state),
    ]
    .spacing(15)
    .align_x(Alignment::Center);

    let alias = truncate(&modal_state.key_alias, 25);
    modal_view(
        Some(format!("Select key source · {alias}")),
        None,
        Some(Msg::XpubCancelModal),
        ModalWidth::L,
        body,
    )
}

fn details_view<'a>(modal_state: &'a XpubEntryModalState) -> Element<'a, Msg> {
    let source = modal_state.input_source.as_ref().and_then(source_line);
    let fetching_label: Option<Element<'_, Msg>> = modal_state.processing.then_some({
        text::new::caption("Fetching from device...")
            .style(theme::text::secondary)
            .width(Length::Fill)
            .into()
    });
    let error: Option<Element<'_, Msg>> = modal_state.fetch_error.as_ref().map(|error| {
        text::new::caption(error)
            .style(theme::text::warning)
            .width(Length::Fill)
            .into()
    });
    let xpub = (!modal_state.xpub_input.is_empty()
        && modal_state.fetch_error.is_none()
        && !modal_state.processing)
        .then_some(xpub_box(&modal_state.xpub_input));
    let validation_error = validation_error(modal_state);
    let account_picker = account_picker(modal_state);

    let body = column![
        source,
        fetching_label,
        error,
        xpub,
        validation_error,
        account_picker,
        details_footer_buttons(modal_state),
    ]
    .spacing(15);

    let alias = truncate(&modal_state.key_alias, 25);
    modal_view(
        Some(alias),
        Some(Msg::XpubDeviceBack),
        Some(Msg::XpubCancelModal),
        ModalWidth::M,
        body,
    )
}

/// Account picker width, shared by the live combobox and the disabled input
/// shown while fetching so the two states keep identical dimensions.
const ACCOUNT_PICKER_WIDTH: f32 = 150.0;

fn account_picker(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    let picker: Element<'_, Msg> = if let Some(fingerprint) = modal_state.selected_device {
        if modal_state.processing {
            // iced has no disabled pick_list, so mirror the combobox with a
            // disabled input of the same width and height (its padding matches
            // the pick_list's vertical DEFAULT_PADDING).
            let value = form::Value {
                value: modal::Account::new(modal_state.selected_account, fingerprint).to_string(),
                warning: None,
                valid: true,
            };
            Container::new(form::Form::new_disabled("Select account", &value).padding(5))
                .width(ACCOUNT_PICKER_WIDTH)
                .into()
        } else {
            modal::account_pick_list(
                fingerprint,
                Some(modal_state.selected_account),
                |account: modal::Account| Msg::XpubUpdateAccount(account.index),
            )
            .width(ACCOUNT_PICKER_WIDTH)
            .into()
        }
    } else {
        Space::with_height(0).into()
    };

    let label = row![
        text::new::b5_bold("Key path account").style(theme::text::primary),
        tooltip::tooltip(
            "The account number in this key's derivation path. Pick a different account to derive an independent key from the same device."
        ),
    ]
    .align_y(Alignment::Center)
    .spacing(5);

    column![label, picker].spacing(8).into()
}

fn validation_error(modal_state: &XpubEntryModalState) -> Option<Element<'_, Msg>> {
    if modal_state.xpub_input.is_empty() || modal_state.processing {
        return None;
    }

    modal_state.validate().err().map(|error| {
        text::new::caption(error)
            .style(theme::text::warning)
            .width(Length::Fill)
            .into()
    })
}

fn xpub_box<'a>(xpub: &'a str) -> Element<'a, Msg> {
    card::flat(
        scrollable::horizontal_thin(text::new::small_caption(xpub).style(theme::text::secondary)),
        10,
    )
    .width(Length::Fill)
    .into()
}

fn source_line(input_source: &XpubInputSource) -> Option<Element<'static, Msg>> {
    match input_source {
        XpubInputSource::Device {
            kind, fingerprint, ..
        } => Some(
            row![
                text::new::caption(format!("Fetched from {kind} #{fingerprint}"))
                    .style(theme::text::secondary),
                icon::check_icon().size(13).style(theme::text::success)
            ]
            .spacing(6)
            .align_y(Alignment::Center)
            .into(),
        ),
        _ => None,
    }
}

fn hw_section(state: &State) -> Element<'_, Msg> {
    let devices = state.hw.list();
    let Some(modal_state) = state.views.xpub.modal.as_ref() else {
        return column![].into();
    };

    if devices.is_empty() {
        return modal::modal_no_devices_placeholder();
    }

    devices
        .values()
        .map(extract_device_data)
        .fold(column![].spacing(12), |column, data| {
            let in_use = state.app.keys().iter().any(|(key_id, key)| {
                *key_id != modal_state.key_id && key.fingerprint() == data.fingerprint
            });
            column.push(device_card(data, in_use))
        })
        .into()
}

struct DeviceRenderData {
    kind: async_hwi::DeviceKind,
    version: Option<async_hwi::Version>,
    fingerprint: Option<miniscript::bitcoin::bip32::Fingerprint>,
    state: DeviceState,
}

enum DeviceState {
    Supported,
    Locked { pairing_code: Option<String> },
    Unsupported { reason: UnsupportedReason },
}

fn device_card(data: DeviceRenderData, in_use: bool) -> Element<'static, Msg> {
    let title = device_title(data.kind, data.version.as_ref());

    if in_use {
        return list::entry_device_list(
            title,
            None::<String>,
            list::DeviceStatus::AlreadyUsed,
            button::EntryWidth::Standard,
            None,
        );
    }

    let fingerprint = data
        .fingerprint
        .map(list::DeviceStatus::Fingerprint)
        .unwrap_or(list::DeviceStatus::None);

    match data.state {
        DeviceState::Supported => {
            let fingerprint = data.fingerprint.expect("supported device has fingerprint");
            list::entry_device_list(
                title,
                None::<String>,
                list::DeviceStatus::Selectable(fingerprint),
                button::EntryWidth::Standard,
                Some(Msg::XpubSelectDevice(fingerprint)),
            )
        }
        DeviceState::Locked { pairing_code } => list::entry_device_list(
            title,
            Some(locked_message(data.kind, pairing_code)),
            fingerprint,
            button::EntryWidth::Standard,
            None,
        ),
        DeviceState::Unsupported { reason } => list::entry_device_list(
            title,
            Some(unsupported_message(data.kind, &reason)),
            fingerprint,
            button::EntryWidth::Standard,
            None,
        ),
    }
}

fn device_title(kind: async_hwi::DeviceKind, version: Option<&async_hwi::Version>) -> String {
    let kind_name = capitalize_first(&kind.to_string());
    match version {
        Some(version) => format!("{kind_name} {version}"),
        None => kind_name,
    }
}

fn locked_message(kind: async_hwi::DeviceKind, pairing_code: Option<String>) -> String {
    match kind {
        async_hwi::DeviceKind::Jade => "This device doesn't support taproot miniscript".to_string(),
        _ => pairing_code
            .map(|code| format!("Pairing code: {code}"))
            .unwrap_or_else(|| "Please unlock the device".to_string()),
    }
}

fn unsupported_message(kind: async_hwi::DeviceKind, reason: &UnsupportedReason) -> String {
    match reason {
        UnsupportedReason::NotPartOfWallet(fingerprint) => {
            format!("Not part of this wallet (#{fingerprint})")
        }
        UnsupportedReason::WrongNetwork => "Wrong network in device settings".to_string(),
        UnsupportedReason::Version {
            minimal_supported_version,
        } => match kind {
            async_hwi::DeviceKind::Jade => {
                "This device doesn't support taproot miniscript".to_string()
            }
            _ => format!(
                "Device version not supported, upgrade to version > {minimal_supported_version}"
            ),
        },
        UnsupportedReason::Method(method) => format!("Unsupported method: {method}"),
        UnsupportedReason::AppIsNotOpen => "Please open the app on device".to_string(),
    }
}

fn extract_device_data(device: &SigningDevice<Msg, HardwareWalletRequestId>) -> DeviceRenderData {
    let kind = device.kind();
    let fingerprint = device.fingerprint();

    fn translate_reason(reason: &async_hwi::service::UnsupportedReason) -> UnsupportedReason {
        match reason {
            async_hwi::service::UnsupportedReason::Version {
                minimal_supported_version,
            } => UnsupportedReason::Version {
                minimal_supported_version: (*minimal_supported_version).into(),
            },
            async_hwi::service::UnsupportedReason::Method(method) => {
                UnsupportedReason::Method(method)
            }
            async_hwi::service::UnsupportedReason::NotPartOfWallet(fingerprint) => {
                UnsupportedReason::NotPartOfWallet(*fingerprint)
            }
            async_hwi::service::UnsupportedReason::WrongNetwork => UnsupportedReason::WrongNetwork,
            async_hwi::service::UnsupportedReason::AppIsNotOpen => UnsupportedReason::AppIsNotOpen,
        }
    }

    let (version, state) = match device {
        SigningDevice::Supported(hw) => {
            let version = hw.version().cloned();
            if is_compatible_with_tapminiscript(hw.kind(), hw.version()) {
                (version, DeviceState::Supported)
            } else {
                let minimal_supported_version = min_taproot_version(hw.kind())
                    .map(|version| version.to_string())
                    .unwrap_or_default();
                (
                    version,
                    DeviceState::Unsupported {
                        reason: UnsupportedReason::Version {
                            minimal_supported_version,
                        },
                    },
                )
            }
        }
        SigningDevice::Locked { pairing_code, .. } => (
            None,
            DeviceState::Locked {
                pairing_code: pairing_code.clone(),
            },
        ),
        SigningDevice::Unsupported {
            version, reason, ..
        } => (
            version.clone(),
            DeviceState::Unsupported {
                reason: translate_reason(reason),
            },
        ),
    };

    DeviceRenderData {
        kind: *kind,
        version,
        fingerprint,
        state,
    }
}

fn other_options(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    let collapsed = modal_state.options_collapsed;
    let section_header = row![
        modal::optional_section(
            // optional_section's flag is "is open" (folded -> right chevron, open -> down).
            !collapsed,
            "Other options".to_string(),
            || Msg::XpubToggleOptions,
            || Msg::XpubToggleOptions,
        ),
        Space::fill_width()
    ];
    let expanded_content = (!collapsed).then_some({
        let file_button = list::entry_action(
            Tile::Import,
            "Import extended public key file",
            None::<String>,
            Some(list::entry_chevron()),
            button::EntryWidth::Standard,
            Some(Msg::XpubLoadFromFile),
        );

        let paste_entry = list::entry_action(
            Tile::Paste,
            "Paste an extended public key",
            None::<String>,
            Some(list::entry_chevron()),
            button::EntryWidth::Standard,
            Some(Msg::XpubSelectPaste),
        );

        let paste_block: Element<'_, Msg> = if modal_state.paste_expanded {
            list::entry_paste_xpub(
                &modal_state.xpub_input,
                Msg::XpubUpdateInput,
                Msg::XpubPaste,
            )
        } else {
            paste_entry
        };

        column![file_button, paste_block].spacing(modal::V_SPACING)
    });

    if let Some(expanded_content) = expanded_content {
        column![section_header, expanded_content]
    } else {
        column![section_header]
    }
    .spacing(modal::V_SPACING)
    .into()
}

fn select_footer_buttons(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    let clear_button = btn_clear(modal_state.current_xpub.is_some().then_some(Msg::XpubClear));
    let can_save = modal_state.validate().is_ok() && modal_state.has_changes();
    let buttons = row![
        btn_cancel(Some(Msg::XpubCancelModal)),
        clear_button,
        btn_save(can_save.then_some(Msg::XpubSave))
    ]
    .spacing(10);

    row![Space::fill_width(), buttons].into()
}

fn details_footer_buttons(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    let retry_button: Option<Element<'_, Msg>> = modal_state
        .fetch_error
        .is_some()
        .then_some(btn_retry(Some(Msg::XpubRetry)).into());

    let can_save =
        modal_state.validate().is_ok() && modal_state.has_changes() && !modal_state.processing;
    let clear_button = btn_clear(modal_state.current_xpub.is_some().then_some(Msg::XpubClear));
    let save_button = btn_save(can_save.then_some(Msg::XpubSave));

    if let Some(retry_button) = retry_button {
        row![retry_button, Space::fill_width(), clear_button, save_button]
    } else {
        row![Space::fill_width(), clear_button, save_button]
    }
    .spacing(10)
    .into()
}
