use crate::state::{Msg, State};
use async_hwi::service::{SigningDevice, UnsupportedReason};
use iced::{
    widget::{pick_list, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{button, card, hw, text},
    icon,
    theme::{self},
    widget::*,
};
use miniscript::bitcoin::bip32::ChildNumber;

/// Render the xpub entry modal if it's open
pub fn render_modal(state: &State) -> Option<Element<'_, Msg>> {
    if let Some(modal_state) = &state.views.xpub.modal {
        return Some(xpub_modal(state, modal_state));
    }
    None
}

/// Main modal rendering function
fn xpub_modal<'a>(
    state: &'a State,
    modal_state: &'a crate::state::views::XpubEntryModalState,
) -> Element<'a, Msg> {
    let mut content = Column::new()
        .spacing(15)
        .padding(20.0)
        .width(Length::Fixed(600.0));

    // Header with title and close button
    content = content.push(
        Row::new()
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
            ),
    );

    // Show current xpub status if one exists
    if modal_state.current_xpub.is_some() {
        content = content.push(
            Container::new(
                Row::new()
                    .spacing(10)
                    .push(icon::tooltip_icon().size(16))
                    .push(text::p2_regular(
                        "This key already has an xpub. You can replace it by fetching from a device, \
                        importing from file, or pasting. Use the Clear button to remove it completely."
                    ))
            )
            .padding(10)
            .style(theme::card::simple)
            .width(Length::Fill)
        );
    }

    // Hardware wallet section (always shown)
    content = content.push(render_hw_section(state, modal_state));

    // "Other options" collapsible section
    content = content.push(render_other_options(modal_state));

    // Validation error display (if present)
    if let Some(error) = &modal_state.validation_error {
        content = content.push(
            text::p2_regular(error)
                .style(theme::text::warning)
                .width(Length::Fill),
        );
    }

    // Footer with action buttons
    if modal_state.processing {
        // Show spinner when processing
        content = content.push(
            Row::new()
                .spacing(10)
                .push(Space::with_width(Length::Fill))
                .push(text::p1_regular("Processing..."))
                .push(Space::with_width(Length::Fill)),
        );
    } else {
        content = content.push(render_footer_buttons(modal_state));
    }

    card::modal(content).into()
}

/// Render the Hardware Wallet section (always visible)
fn render_hw_section<'a>(
    state: &'a State,
    modal_state: &'a crate::state::views::XpubEntryModalState,
) -> Element<'a, Msg> {
    let mut content = Column::new().spacing(10).padding(10);

    let devices = state.hw.list();

    if devices.is_empty() {
        // No devices detected
        content = content
            .push(Space::with_height(20))
            .push(
                Column::new()
                    .spacing(10)
                    .align_x(Alignment::Center)
                    .push(icon::usb_icon().size(60))
                    .push(text::p1_regular(
                        "No hardware wallets detected. Connect a device and unlock it.",
                    ))
                    .width(Length::Fill),
            )
            .push(Space::with_height(20));
    } else {
        // Show device list - extract data to avoid lifetime issues with local BTreeMap
        content = content.push(text::p1_bold("Detected Devices:"));

        let device_data: Vec<_> = devices.values().map(extract_device_data).collect();
        for data in device_data {
            let device_widget = render_device_card(data, modal_state);
            content = content.push(device_widget);
        }
    }

    // Account selection (always visible, but may be disabled)
    let accounts: Vec<_> = (0..10)
        .map(|i| ChildNumber::from_hardened_idx(i).expect("hardcoded"))
        .collect();

    let account_picker = pick_list(
        accounts,
        Some(modal_state.selected_account),
        Msg::XpubUpdateAccount,
    )
    .width(Length::Fixed(150.0));

    content = content.push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(text::p1_regular("Account:"))
            .push(account_picker),
    );

    content.into()
}

/// Extracted device data for rendering (avoids lifetime issues with local BTreeMap)
struct DeviceRenderData {
    kind: async_hwi::DeviceKind,
    fingerprint: Option<miniscript::bitcoin::bip32::Fingerprint>,
    state: DeviceState,
}

enum DeviceState {
    Supported,
    Locked { pairing_code: Option<String> },
    Unsupported { version: Option<async_hwi::Version>, reason: UnsupportedReason },
}

/// Render a device card based on its state (Supported, Locked, or Unsupported)
fn render_device_card(
    data: DeviceRenderData,
    modal_state: &crate::state::views::XpubEntryModalState,
) -> Element<'_, Msg> {
    let kind = data.kind;

    match data.state {
        DeviceState::Supported => {
            let fp = data.fingerprint.expect("supported device has fingerprint");

            // Build the device card content using liana-ui hw component
            let card_content =
                if modal_state.processing && modal_state.selected_device == Some(fp) {
                    hw::processing_hardware_wallet(&kind, None::<&str>, fp, None::<&str>)
                } else {
                    hw::supported_hardware_wallet(&kind, None::<&str>, fp, None::<&str>)
                };

            // Wrap in a button for supported devices (clickable to fetch)
            let mut bttn = Button::new(card_content)
                .style(theme::button::secondary)
                .width(Length::Fill);

            if !modal_state.processing {
                bttn = bttn.on_press(Msg::XpubFetchFromDevice(fp, modal_state.selected_account));
            }

            bttn.into()
        }
        DeviceState::Locked { pairing_code } => {
            // Locked device - show as locked, not clickable
            // Pass owned Option<String> to avoid lifetime issues
            let card_content = hw::locked_hardware_wallet(&kind, pairing_code);

            Button::new(card_content)
                .style(theme::button::secondary)
                .width(Length::Fill)
                .into()
        }
        DeviceState::Unsupported { version, reason } => {
            // Unsupported device - show appropriate message based on reason
            let card_content: Container<'_, Msg> = match &reason {
                UnsupportedReason::NotPartOfWallet(fg) => {
                    hw::unrelated_hardware_wallet(&kind, version.as_ref(), fg)
                }
                UnsupportedReason::WrongNetwork => {
                    hw::wrong_network_hardware_wallet(&kind, version.as_ref())
                }
                UnsupportedReason::Version {
                    minimal_supported_version,
                } => hw::unsupported_version_hardware_wallet(
                    &kind,
                    version.as_ref(),
                    minimal_supported_version,
                ),
                _ => hw::unsupported_hardware_wallet(&kind, version.as_ref()),
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
    let kind = device.kind().clone();
    let fingerprint = device.fingerprint();

    let state = match device {
        SigningDevice::Supported(_) => DeviceState::Supported,
        SigningDevice::Locked { pairing_code, .. } => DeviceState::Locked {
            pairing_code: pairing_code.clone(),
        },
        SigningDevice::Unsupported { version, reason, .. } => DeviceState::Unsupported {
            version: version.clone(),
            reason: reason.clone(),
        },
    };

    DeviceRenderData {
        kind,
        fingerprint,
        state,
    }
}

/// Render the "Other options" collapsible section
fn render_other_options(
    modal_state: &crate::state::views::XpubEntryModalState,
) -> Element<'_, Msg> {
    let mut content = Column::new().spacing(10);

    // Collapsible header button
    let header_text = if modal_state.options_collapsed {
        "Other options ▼"
    } else {
        "Other options ▲"
    };

    let header_btn = button::transparent(None, header_text).on_press(Msg::XpubToggleOptions);

    content = content.push(header_btn);

    // Show options when expanded
    if !modal_state.options_collapsed {
        // Import from file button
        let file_button =
            button::secondary(Some(icon::import_icon()), "Import extended public key file")
                .on_press(Msg::XpubLoadFromFile)
                .width(Length::Fill);
        content = content.push(file_button);

        // Paste xpub button
        let paste_button =
            button::secondary(Some(icon::clipboard_icon()), "Paste extended public key")
                .on_press(Msg::XpubPaste)
                .width(Length::Fill);
        content = content.push(paste_button);

        // Show input field if paste was used or file loaded
        if !modal_state.xpub_input.is_empty() {
            content = content
                .push(Space::with_height(5))
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(text::p2_regular("Current input:"))
                        .push(Space::with_width(Length::Fill))
                        .push(
                            button::transparent(Some(icon::cross_icon().size(16)), "")
                                .on_press(Msg::XpubUpdateInput(String::new())),
                        ),
                )
                .push(
                    Container::new(
                        text::p2_regular(&modal_state.xpub_input).style(theme::text::secondary),
                    )
                    .padding(10)
                    .style(theme::card::simple)
                    .width(Length::Fill),
                );
        }
    }

    content.into()
}

/// Render the footer action buttons
fn render_footer_buttons(
    modal_state: &crate::state::views::XpubEntryModalState,
) -> Element<'_, Msg> {
    let mut row = Row::new().spacing(10);

    // Cancel button (always enabled)
    row = row.push(
        button::secondary(None, "Cancel")
            .on_press(Msg::XpubCancelModal)
            .width(Length::Fixed(120.0)),
    );

    // Clear button (enabled only if there's a current xpub)
    let clear_button = if modal_state.current_xpub.is_some() {
        button::secondary(None, "Clear")
            .on_press(Msg::XpubClear)
            .width(Length::Fixed(120.0))
    } else {
        button::secondary(None, "Clear").width(Length::Fixed(120.0))
    };
    row = row.push(clear_button);

    // Save button (enabled only if validation passes and there are changes)
    let can_save = modal_state.validate().is_ok() && modal_state.has_changes();
    let save_button = if can_save {
        button::primary(None, "Save")
            .on_press(Msg::XpubSave)
            .width(Length::Fixed(120.0))
    } else {
        button::secondary(None, "Save").width(Length::Fixed(120.0))
    };
    row = row.push(save_button);

    Row::new()
        .push(Space::with_width(Length::Fill))
        .push(row)
        .into()
}
