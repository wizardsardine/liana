use crate::state::{
    views::{ModalStep, XpubEntryModalState},
    Msg, State,
};
use async_hwi::service::SigningDevice;
use iced::{
    alignment::Vertical,
    widget::{container, pick_list, row, Space},
    Alignment, Length,
};
use liana_gui::hw::{is_compatible_with_tapminiscript, min_taproot_version, UnsupportedReason};
use liana_ui::{
    component::{
        button, card, form, modal,
        text::{self, p1_bold},
        tooltip,
    },
    icon, theme,
    widget::*,
};
use miniscript::bitcoin::bip32::ChildNumber;

type FnMsg = fn() -> Msg;

/// Capitalize the first letter of a string
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

/// Render the xpub entry modal if it's open
pub fn xpub_modal_view(state: &State) -> Option<Element<'_, Msg>> {
    let modal_state = state.views.xpub.modal.as_ref()?;
    Some(match modal_state.step {
        ModalStep::Select => select_view(state, modal_state),
        ModalStep::Details => details_view(modal_state),
    })
}

/// Render the Select view - shows device list and other options
fn select_view<'a>(state: &'a State, modal_state: &'a XpubEntryModalState) -> Element<'a, Msg> {
    let header = modal::header(
        Some(format!("Select key source - {}", modal_state.key_alias)),
        None::<FnMsg>,
        Some(|| Msg::XpubCancelModal),
    );

    // Show current xpub status if one exists
    let xpub_status = modal_state.current_xpub.is_some().then_some(
        Container::new(
            Row::new()
                .spacing(10)
                .push(icon::tooltip_icon().size(16))
                .push(
                text::p2_medium(
                    "This key already has an xpub. You can replace it by fetching from a device, \
                    importing from file, or pasting. Use the Clear button to remove it completely.",
                )
                .style(theme::text::primary),
            ),
        )
        .padding(10)
        .style(theme::card::simple)
        .width(Length::Fill),
    );

    // Validation error display - show validation result when input is present
    let validation_error = if !modal_state.xpub_input.is_empty() {
        modal_state.validate().err().map(|error| {
            text::p2_medium(error)
                .style(theme::text::warning)
                .width(Length::Fill)
        })
    } else {
        None
    };

    // Show input field if paste was used or file loaded
    let input_display = (!modal_state.xpub_input.is_empty()).then(|| {
        let input_header = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(text::p2_medium("Current xpub:").style(theme::text::primary))
            .push(Space::with_width(Length::Fill));

        let input_value =
            Container::new(text::p2_medium(&modal_state.xpub_input).style(theme::text::secondary))
                .padding(10)
                .style(theme::card::simple)
                .width(Length::Fill);

        Column::new()
            .push(Space::with_height(5))
            .push(input_header)
            .push(input_value)
    });

    let content = Column::new()
        .push(header)
        .push_maybe(xpub_status)
        .push(hw_section(state))
        .push_maybe(input_display)
        .push(other_options(modal_state))
        .push_maybe(validation_error)
        .push(footer_buttons(modal_state))
        .spacing(15)
        .align_x(Alignment::Center)
        .width(modal::MODAL_WIDTH);

    card::modal(content).into()
}

/// Render the Details view - shows account picker and fetch status
fn details_view(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    let header = modal::header(
        Some(modal_state.key_alias.clone()),
        Some(|| Msg::XpubDeviceBack),
        Some(|| Msg::XpubCancelModal),
    );

    // Account selection picker
    let accounts: Vec<_> = (0..10)
        .map(|i| ChildNumber::from_hardened_idx(i).expect("hardcoded"))
        .collect();

    let pick_enabled = !modal_state.processing;

    let info = "Switch account if you already uses the same hardware in other configurations";
    let account_label = row![p1_bold("Key path account:"), tooltip(info)].align_y(Vertical::Center);

    let account = if pick_enabled {
        container(
            pick_list(
                accounts,
                Some(modal_state.selected_account),
                Msg::XpubUpdateAccount,
            )
            .width(Length::Fill),
        )
    } else {
        container(
            text::p1_medium(format_account(modal_state.selected_account))
                .style(theme::text::primary)
                .width(Length::Fill),
        )
    }
    .width(180);

    let fetching_label = modal_state.processing.then_some(
        Row::new()
            .spacing(10)
            .push(Space::with_width(Length::Fill))
            .push(text::p1_medium("Fetching from device...").style(theme::text::primary))
            .push(Space::with_width(Length::Fill)),
    );

    let error = modal_state.fetch_error.as_ref().map(|error| {
        text::p2_medium(error)
            .style(theme::text::warning)
            .width(Length::Fill)
    });

    // Processing indicator or action buttons
    let btn_row = {
        let mut btn_row = Row::new().spacing(10);

        // Retry button (only if there was an error)
        if modal_state.fetch_error.is_some() {
            btn_row = btn_row.push(
                button::secondary(None, "Retry")
                    .on_press(Msg::XpubRetry)
                    .width(Length::Fixed(100.0)),
            );
        }

        btn_row = btn_row.push(Space::with_width(Length::Fill));

        // Save button (enabled only if we have a valid xpub)
        let can_save =
            modal_state.validate().is_ok() && modal_state.has_changes() && !modal_state.processing;
        let save_button = if can_save {
            button::primary(None, "Save")
                .on_press(Msg::XpubSave)
                .width(Length::Fixed(100.0))
        } else {
            button::secondary(None, "Save").width(Length::Fixed(100.0))
        };
        btn_row = btn_row.push(save_button);
        btn_row
    };

    // Validation error (e.g., wrong network)
    let validation_error = if !modal_state.xpub_input.is_empty() && !modal_state.processing {
        modal_state.validate().err().map(|error| {
            text::p2_medium(error)
                .style(theme::text::warning)
                .width(Length::Fill)
        })
    } else {
        None
    };

    // Show fetched xpub if available and not fetching
    let xpub = (!modal_state.xpub_input.is_empty()
        && modal_state.fetch_error.is_none()
        && !modal_state.processing
        && validation_error.is_none())
    .then_some({
        Container::new(text::p2_medium(&modal_state.xpub_input).style(theme::text::secondary))
            .padding(10)
            .style(theme::card::simple)
            .width(Length::Fill)
    });

    let content = Column::new()
        .push(header)
        .push_maybe(fetching_label)
        .push_maybe(error)
        .push_maybe(xpub)
        .push_maybe(validation_error)
        .push(account_label)
        .push(account)
        .push(btn_row)
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(450.0));

    card::modal(content).into()
}

/// Format account for display (e.g., "Account #0")
fn format_account(account: ChildNumber) -> String {
    let index = account.to_string().replace("'", "");
    format!("Account #{}", index)
}

/// Render the Hardware Wallet section (Select step only)
fn hw_section(state: &State) -> Element<'_, Msg> {
    let devices = state.hw.list();

    let device_list: Element<'_, Msg> = if devices.is_empty() {
        // No devices detected
        Column::new()
            .spacing(10)
            .align_x(Alignment::Center)
            .push(Space::with_height(20))
            .push(icon::usb_icon().size(60))
            .push(
                text::p1_medium("No hardware wallets detected. Connect a device and unlock it.")
                    .style(theme::text::primary),
            )
            .push(Space::with_height(20))
            .width(Length::Fill)
            .into()
    } else {
        // Show device list - extract data to avoid lifetime issues with local BTreeMap
        let device_data: Vec<_> = devices.values().map(extract_device_data).collect();
        let mut list = Column::new()
            .spacing(10)
            .push(text::p1_bold("Detected Devices:"));
        for data in device_data {
            list = list.push(device_card(data));
        }
        list.into()
    };

    Column::new()
        .push(device_list)
        .spacing(10)
        .padding(10)
        .into()
}

/// Extracted device data for rendering (avoids lifetime issues with local BTreeMap)
struct DeviceRenderData {
    kind: async_hwi::DeviceKind,
    version: Option<async_hwi::Version>,
    fingerprint: Option<miniscript::bitcoin::bip32::Fingerprint>,
    state: DeviceState,
}

enum DeviceState {
    Supported,
    Locked {
        pairing_code: Option<String>,
    },
    Unsupported {
        reason: UnsupportedReason,
    },
}

/// Render a device card based on its state (Supported, Locked, or Unsupported)
/// Clicking a supported device opens the Details step
fn device_card(data: DeviceRenderData) -> Element<'static, Msg> {
    let kind_name = capitalize_first(&data.kind.to_string());
    let name = match &data.version {
        Some(v) => format!("{kind_name} {v}"),
        None => kind_name,
    };

    match data.state {
        DeviceState::Supported => {
            let fp = data.fingerprint.expect("supported device has fingerprint");
            modal::key_entry(
                Some(icon::usb_drive_icon()),
                name,
                Some(format!("#{fp}")),
                None,
                None,
                None,
                Some(move || Msg::XpubSelectDevice(fp)),
            )
        }
        DeviceState::Locked { pairing_code } => {
            let message = match pairing_code {
                Some(code) => format!("Locked, check code: {code}"),
                None => "Please unlock the device".to_string(),
            };
            modal::key_entry(
                Some(icon::usb_drive_icon()),
                name,
                None,
                None,
                None,
                Some(message),
                None::<fn() -> Msg>,
            )
        }
        DeviceState::Unsupported { reason } => {
            let message = match &reason {
                UnsupportedReason::NotPartOfWallet(fg) => {
                    format!("Not part of this wallet (#{fg})")
                }
                UnsupportedReason::WrongNetwork => {
                    "Wrong network in device settings".to_string()
                }
                UnsupportedReason::Version {
                    minimal_supported_version,
                } => {
                    format!("Unsupported firmware, upgrade to > {minimal_supported_version}")
                }
                UnsupportedReason::Method(m) => format!("Unsupported method: {m}"),
                UnsupportedReason::AppIsNotOpen => "Please open the app on device".to_string(),
            };
            let fp_str = data.fingerprint.map(|fp| format!("#{fp}"));
            modal::key_entry(
                Some(icon::usb_drive_icon()),
                name,
                fp_str,
                None,
                None,
                Some(message),
                None::<fn() -> Msg>,
            )
        }
    }
}

/// Extract render data from a SigningDevice (copies needed data to avoid lifetime issues)
fn extract_device_data(device: &SigningDevice<Msg>) -> DeviceRenderData {
    let kind = device.kind();
    let fingerprint = device.fingerprint();

    fn translate_reason(reason: &async_hwi::service::UnsupportedReason) -> UnsupportedReason {
        match reason {
            async_hwi::service::UnsupportedReason::Version {
                minimal_supported_version,
            } => UnsupportedReason::Version {
                minimal_supported_version: (*minimal_supported_version).into(),
            },
            async_hwi::service::UnsupportedReason::Method(m) => UnsupportedReason::Method(m),
            async_hwi::service::UnsupportedReason::NotPartOfWallet(fg) => {
                UnsupportedReason::NotPartOfWallet(*fg)
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
                    .map(|v| v.to_string())
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

/// Render the "Other options" collapsible section
fn other_options(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    let collapsed = modal_state.options_collapsed;

    let section_header = modal::optional_section(
        collapsed,
        "Other options".to_string(),
        || Msg::XpubToggleOptions,
        || Msg::XpubToggleOptions,
    );

    let expanded_content = (!collapsed).then(|| {
        let file_button: Element<'_, Msg> = modal::button_entry(
            Some(icon::import_icon()),
            "Import extended public key file",
            None,
            None,
            Some(|| Msg::XpubLoadFromFile),
        );

        let form_xpub = form::Value {
            value: modal_state.xpub_input.clone(),
            warning: None,
            valid: true,
        };
        let paste_input: Element<'_, Msg> = modal::collapsible_input_button(
            modal_state.paste_expanded,
            Some(icon::paste_icon()),
            "Paste an extended public key".to_string(),
            "xpub...".to_string(),
            &form_xpub,
            Some(Msg::XpubUpdateInput),
            Some(|| Msg::XpubPaste),
            || Msg::XpubSelectPaste,
        );

        Column::new()
            .spacing(modal::V_SPACING)
            .push(file_button)
            .push(paste_input)
    });

    Column::new()
        .spacing(modal::V_SPACING)
        .push(section_header)
        .push_maybe(expanded_content)
        .width(modal::BTN_W)
        .into()
}

/// Render the footer action buttons (Select step only)
fn footer_buttons(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    // Cancel button (always enabled)
    let cancel_button = button::secondary(None, "Cancel")
        .on_press(Msg::XpubCancelModal)
        .width(Length::Fixed(120.0));

    // Clear button (enabled only if there's a current xpub)
    let clear_button = if modal_state.current_xpub.is_some() {
        button::secondary(None, "Clear")
            .on_press(Msg::XpubClear)
            .width(Length::Fixed(120.0))
    } else {
        button::secondary(None, "Clear").width(Length::Fixed(120.0))
    };

    // Save button (enabled only if validation passes and there are changes)
    let can_save = modal_state.validate().is_ok() && modal_state.has_changes();
    let save_button = if can_save {
        button::primary(None, "Save")
            .on_press(Msg::XpubSave)
            .width(Length::Fixed(120.0))
    } else {
        button::secondary(None, "Save").width(Length::Fixed(120.0))
    };

    let buttons = Row::new()
        .spacing(10)
        .push(cancel_button)
        .push(clear_button)
        .push(save_button);

    Row::new()
        .push(Space::with_width(Length::Fill))
        .push(buttons)
        .into()
}
