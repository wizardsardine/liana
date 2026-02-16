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
        button, card, hw,
        text::{self, p1_bold},
        tooltip,
    },
    icon, theme,
    widget::*,
};
use miniscript::bitcoin::bip32::ChildNumber;

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
    // Header with title and close button
    let header = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(text::h3(format!(
            "Select key source - {}",
            modal_state.key_alias
        )))
        .push(Space::with_width(Length::Fill))
        .push(
            button::transparent(Some(icon::cross_icon().size(32)), "")
                .on_press(Msg::XpubCancelModal),
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
        .padding(20.0)
        .width(Length::Fixed(600.0));

    card::modal(content).into()
}

/// Render the Details view - shows account picker and fetch status
fn details_view(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    // Header with back button and close button
    let header = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(button::transparent(Some(icon::previous_icon()), "").on_press(Msg::XpubDeviceBack))
        .push(text::h3(&modal_state.key_alias))
        .push(Space::with_width(Length::Fill))
        .push(
            button::transparent(Some(icon::cross_icon().size(32)), "")
                .on_press(Msg::XpubCancelModal),
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
    fingerprint: Option<miniscript::bitcoin::bip32::Fingerprint>,
    state: DeviceState,
}

enum DeviceState {
    Supported,
    Locked {
        pairing_code: Option<String>,
    },
    Unsupported {
        version: Option<async_hwi::Version>,
        reason: UnsupportedReason,
    },
}

/// Render a device card based on its state (Supported, Locked, or Unsupported)
/// Clicking a supported device opens the Details step
fn device_card(data: DeviceRenderData) -> Element<'static, Msg> {
    let kind = data.kind;

    match data.state {
        DeviceState::Supported => {
            let fp = data.fingerprint.expect("supported device has fingerprint");

            // Build the device card content using liana-ui hw component
            let card_content = hw::supported_hardware_wallet(kind, None::<&str>, fp, None::<&str>);

            // Wrap in a button - clicking opens Details step
            Button::new(card_content)
                .style(theme::button::secondary)
                .width(Length::Fill)
                .on_press(Msg::XpubSelectDevice(fp))
                .into()
        }
        DeviceState::Locked { pairing_code } => {
            // Locked device - show as locked, not clickable
            let card_content = match kind {
                async_hwi::DeviceKind::Jade => hw::taproot_not_supported_device(kind),
                _ => hw::locked_hardware_wallet(kind, pairing_code),
            };

            Button::new(card_content)
                .style(theme::button::secondary)
                .width(Length::Fill)
                .into()
        }
        DeviceState::Unsupported { version, reason } => {
            // Unsupported device - show appropriate message based on reason
            let card_content: Container<'_, Msg> = match &reason {
                UnsupportedReason::NotPartOfWallet(fg) => {
                    hw::unrelated_hardware_wallet(kind, version.as_ref(), fg)
                }
                UnsupportedReason::WrongNetwork => {
                    hw::wrong_network_hardware_wallet(kind, version.as_ref())
                }
                UnsupportedReason::Version {
                    minimal_supported_version,
                } => match kind {
                    async_hwi::DeviceKind::Jade => hw::taproot_not_supported_device(kind),
                    _ => hw::unsupported_version_hardware_wallet(
                        kind,
                        version.as_ref(),
                        minimal_supported_version,
                    ),
                },
                _ => hw::unsupported_hardware_wallet(kind, version.as_ref()),
            };

            Button::new(card_content)
                .style(theme::button::secondary)
                .width(Length::Fill)
                .into()
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

    let state = match device {
        SigningDevice::Supported(hw) => {
            if is_compatible_with_tapminiscript(hw.kind(), hw.version()) {
                DeviceState::Supported
            } else {
                let minimal_supported_version = min_taproot_version(hw.kind())
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                DeviceState::Unsupported {
                    version: hw.version().cloned(),
                    reason: UnsupportedReason::Version {
                        minimal_supported_version,
                    },
                }
            }
        }
        SigningDevice::Locked { pairing_code, .. } => DeviceState::Locked {
            pairing_code: pairing_code.clone(),
        },
        SigningDevice::Unsupported {
            version, reason, ..
        } => DeviceState::Unsupported {
            version: version.clone(),
            reason: translate_reason(reason),
        },
    };

    DeviceRenderData {
        kind: *kind,
        fingerprint,
        state,
    }
}

/// Render the "Other options" collapsible section
fn other_options(modal_state: &XpubEntryModalState) -> Element<'_, Msg> {
    // Collapsible header button
    let header_text = if modal_state.options_collapsed {
        "Other options ▼"
    } else {
        "Other options ▲"
    };
    let header_btn = button::transparent(None, header_text).on_press(Msg::XpubToggleOptions);

    let expanded_content = (!modal_state.options_collapsed).then(|| {
        let file_button =
            button::secondary(Some(icon::import_icon()), "Import extended public key file")
                .on_press(Msg::XpubLoadFromFile)
                .width(Length::Fill);

        let paste_button =
            button::secondary(Some(icon::clipboard_icon()), "Paste extended public key")
                .on_press(Msg::XpubPaste)
                .width(Length::Fill);

        Column::new()
            .spacing(10)
            .push(file_button)
            .push(paste_button)
    });

    Column::new()
        .spacing(10)
        .push(header_btn)
        .push_maybe(expanded_content)
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
