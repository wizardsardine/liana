pub mod modal;

use crate::{
    backend::Backend,
    state::{message::Msg, State},
    views::layout_with_scrollable_list,
};
use async_hwi::service::SigningDevice;
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{button, hw, text},
    icon, theme,
    widget::*,
};
use miniscript::bitcoin::bip32::Fingerprint;

use super::{menu_entry, INSTALLER_STEPS, MENU_ENTRY_WIDTH};

/// Main registration view
pub fn registration_view(state: &State) -> Element<'_, Msg> {
    let reg_state = &state.views.registration;

    // Get org name and wallet name from backend
    let org_name = state
        .app
        .selected_org
        .and_then(|org_id| state.backend.get_org(org_id))
        .map(|org| org.name.clone())
        .unwrap_or_else(|| "Organization".to_string());
    let wallet_name = state
        .app
        .selected_wallet
        .and_then(|wallet_id| state.backend.get_wallet(wallet_id))
        .map(|wallet| wallet.alias.clone())
        .unwrap_or_else(|| "Wallet".to_string());
    let breadcrumb = vec![org_name, wallet_name, "Register Devices".to_string()];

    // Get current user email
    let current_user_email = &state.views.login.email.form.value;

    // Header content
    let header_content = Column::new()
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20)
        .push(text::h2("Register Wallet on Devices"))
        .push(
            text::p1_medium(
                "Register the wallet descriptor on each device, or skip if unavailable.",
            )
            .style(theme::text::secondary),
        );
    let header_content = row![
        Space::with_width(Length::Fill),
        header_content,
        Space::with_width(Length::Fill)
    ];

    // List content: device cards or info message
    let list_content = if reg_state.user_devices.is_empty() {
        no_devices_view()
    } else {
        device_list_view(state)
    };

    // Footer with Skip button (only if there are devices to skip)
    let footer_content = if reg_state.user_devices.is_empty() {
        None
    } else {
        let btn_width = 200;
        let spacer = MENU_ENTRY_WIDTH - btn_width;
        let skip_btn = button::secondary(None, "Skip")
            .on_press(Msg::RegistrationSkipAll)
            .width(btn_width);
        let footer = row![
            Space::with_width(Length::Fill),
            Space::with_width(spacer),
            skip_btn,
            Space::with_width(Length::Fill),
        ]
        .align_y(Alignment::Center);

        Some(
            Container::new(footer)
                .padding(20)
                .width(Length::Fill)
                .center_x(Length::Fill)
                .into(),
        )
    };

    layout_with_scrollable_list(
        (5, INSTALLER_STEPS),
        Some(current_user_email),
        None,
        &breadcrumb,
        header_content,
        list_content,
        footer_content,
        true,
        Some(Msg::NavigateToWalletSelect),
    )
}

fn no_devices_view<'a>() -> Element<'a, Msg> {
    Column::new()
        .spacing(20)
        .align_x(Alignment::Center)
        .push(icon::tooltip_icon().size(60))
        .push(text::h3("No devices to register"))
        .push(
            text::p1_medium("You don't have any devices assigned in this wallet.")
                .style(theme::text::secondary),
        )
        .width(Length::Fill)
        .padding([40, 20])
        .into()
}

fn device_list_view(state: &State) -> Element<'_, Msg> {
    let reg_state = &state.views.registration;
    let connected_devices = state.hw.list();

    // Build list of device cards
    let mut cards = Column::new().spacing(10).padding([0, 20]);

    for fingerprint in &reg_state.user_devices {
        // Check if this device is connected
        let connected_device = connected_devices.values().find(|d| match d {
            SigningDevice::Supported(hw) => hw.fingerprint() == fingerprint,
            _ => false,
        });

        let card = match connected_device {
            Some(SigningDevice::Supported(hw)) => {
                // Connected and ready to register - clickable
                clickable_device_card(*fingerprint, *hw.kind())
            }
            Some(_) => {
                // Connected but locked/unsupported
                disconnected_device_card(*fingerprint, "Device locked or unsupported")
            }
            None => {
                // Not connected
                disconnected_device_card(*fingerprint, "Connect device to register")
            }
        };

        cards = cards.push(card);
    }

    cards.into()
}

fn clickable_device_card(
    fingerprint: Fingerprint,
    device_kind: async_hwi::DeviceKind,
) -> Element<'static, Msg> {
    let content = hw::supported_hardware_wallet(
        device_kind,
        None::<&str>,
        fingerprint,
        Some("Ready to register"),
    );

    let message = Some(Msg::RegistrationSelectDevice(fingerprint));
    menu_entry(content.into(), message)
}

fn disconnected_device_card(
    fingerprint: Fingerprint,
    status: &'static str,
) -> Element<'static, Msg> {
    let content = Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(icon::key_icon().size(24).style(theme::text::secondary))
        .push(
            Column::new()
                .spacing(4)
                .push(
                    text::p1_medium(format!("Fingerprint: {}", fingerprint))
                        .style(theme::text::secondary),
                )
                .push(text::caption(status).style(theme::text::secondary)),
        );

    menu_entry(content.into(), None)
}
