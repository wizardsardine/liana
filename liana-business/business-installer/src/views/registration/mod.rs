pub mod modal;

use crate::{
    backend::Backend,
    state::{message::Msg, State},
    views::layout_with_scrollable_list,
};
use async_hwi::service::{is_compatible_with_tapminiscript, SigningDevice};
use iced::{
    widget::{row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::Wallet;
use liana_ui::{
    component::{
        button::{btn_skip, BtnWidth},
        modal as ui_modal, text,
    },
    icon, theme,
    widget::*,
};
use miniscript::bitcoin::bip32::Fingerprint;

use super::{INSTALLER_STEPS, MENU_ENTRY_WIDTH};

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
        let spacer = MENU_ENTRY_WIDTH - BtnWidth::XL as u32;
        let skip_btn = btn_skip(Some(Msg::RegistrationSkipAll));
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
        false,
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

fn alias_from_fg(wallet: &Wallet, fg: Fingerprint) -> Option<String> {
    for key in wallet.template.as_ref()?.keys.values() {
        if key.fingerprint() == Some(fg) {
            return Some(key.alias.clone());
        }
    }
    None
}

fn device_list_view(state: &State) -> Element<'_, Msg> {
    let reg_state = &state.views.registration;
    let connected_devices = state.hw.list();

    // Build list of device cards
    let mut cards = Column::new().spacing(10).padding([0, 20]);
    let wallet = state.selected_wallet();

    for fingerprint in &reg_state.user_devices {
        let alias = wallet
            .as_ref()
            .and_then(|w| alias_from_fg(w, *fingerprint))
            .unwrap_or_default();

        // Check if this device is connected
        let connected_device = connected_devices.values().find(|d| match d {
            SigningDevice::Supported(hw) => hw.fingerprint() == fingerprint,
            _ => false,
        });

        let card = match connected_device {
            Some(SigningDevice::Supported(hw)) => {
                // Connected and ready to register - clickable
                if is_compatible_with_tapminiscript(hw.kind(), hw.version()) {
                    registration_key_entry(*fingerprint, Some(*hw.kind()), true, alias)
                } else {
                    registration_key_entry(*fingerprint, None, true, alias)
                }
            }
            Some(_) => {
                // Connected but locked/unsupported
                registration_key_entry(*fingerprint, None, true, alias)
            }
            None => {
                // Not connected
                registration_key_entry(*fingerprint, None, false, alias)
            }
        };

        cards = cards.push(card);
    }

    cards.into()
}

fn device_kind(kind: async_hwi::DeviceKind) -> String {
    let binding = kind.to_string();
    let mut chars = binding.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

pub fn registration_key_entry(
    fingerprint: Fingerprint,
    kind: Option<async_hwi::DeviceKind>,
    device_connected: bool,
    alias: String,
) -> Element<'static, Msg> {
    let kind_name = kind.map(device_kind);
    let alias = (!alias.is_empty()).then_some(alias);
    let status = if kind.is_some() {
        "Register"
    } else if device_connected {
        "Device not supported or locked"
    } else {
        "Connect the associated device to register"
    };
    let on_press = kind
        .is_some()
        .then_some(move || Msg::RegistrationSelectDevice(fingerprint));
    ui_modal::registration_key_entry(
        format!("#{fingerprint}"),
        kind_name,
        alias,
        Some(status.to_string()),
        on_press,
    )
}
