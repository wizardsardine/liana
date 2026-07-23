pub mod modal;

use crate::{
    backend::Backend,
    state::{message::Msg, State},
    views::{intro_description, layout_with_scrollable_list, screen_intro},
};
use async_hwi::service::{is_compatible_with_tapminiscript, SigningDevice};
use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana_connect::ws_business::Wallet;
use liana_ui::{
    component::{button::btn_skip_registration, list, pill, text},
    spacing::{HSpacing, VSpacing},
    theme,
    widget::*,
};
use miniscript::bitcoin::bip32::Fingerprint;

use super::INSTALLER_STEPS;

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
    let header_content = screen_intro(
        "Register",
        Some(intro_description(
            "Register the wallet descriptor on each device, or skip if unavailable.",
        )),
        false,
    );

    // List content: device cards or info message
    let list_content = if !reg_state.has_visible_devices() {
        no_devices_view()
    } else {
        device_list_view(state)
    };

    let footer = Some(
        row![
            Space::fill_width(),
            btn_skip_registration(Some(Msg::RegistrationSkipAll)),
            Space::fill_width(),
        ]
        .into(),
    );

    layout_with_scrollable_list(
        (7, INSTALLER_STEPS),
        state.network,
        Some(current_user_email),
        false,
        &breadcrumb,
        Some(header_content),
        list_content,
        None,
        footer,
        Some(Msg::NavigateToWalletSelect),
    )
}

fn no_devices_view<'a>() -> Element<'a, Msg> {
    Container::new(
        text::new::caption("You have no devices to register.")
            .style(theme::text::secondary)
            .align_x(iced::alignment::Horizontal::Center),
    )
    .padding([24, 0])
    .center_x(Length::Fill)
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

    let mut cards = column![]
        .spacing(VSpacing::M)
        .padding([0, 20])
        .align_x(Alignment::Center)
        .width(Length::Fill);
    let wallet = state.selected_wallet();

    for fingerprint in reg_state.visible_devices() {
        let alias = wallet
            .as_ref()
            .and_then(|w| alias_from_fg(w, fingerprint))
            .unwrap_or_default();
        let done = reg_state.is_registered(fingerprint);

        // Check if this device is connected
        let connected_device = connected_devices
            .values()
            .find(|device| device.fingerprint() == Some(fingerprint));

        let card = match connected_device {
            Some(SigningDevice::Supported(hw)) => {
                // Connected and ready to register - clickable
                if is_compatible_with_tapminiscript(hw.kind(), hw.version()) {
                    registration_key_entry(fingerprint, Some(*hw.kind()), true, done, alias)
                } else {
                    registration_key_entry(fingerprint, None, true, done, alias)
                }
            }
            _ => {
                // Locked/Unsupported devices carry no fingerprint, so they can't be tied to a
                // specific key; they fall through to "not connected", matching the legacy view.
                registration_key_entry(fingerprint, None, false, done, alias)
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
    done: bool,
    alias: String,
) -> Element<'static, Msg> {
    let kind_name = kind.map(device_kind);
    let alias = (!alias.is_empty())
        .then_some(alias)
        .or(kind_name)
        .unwrap_or_else(|| format!("#{fingerprint}"));
    let fingerprint_label = format!("#{fingerprint}");
    let can_register = kind.is_some();
    let enabled = can_register || done;
    let status = if done {
        Some("Registered on device")
    } else if can_register {
        None
    } else if device_connected {
        Some("Device locked or unsupported")
    } else {
        Some("Connect the associated device to register")
    };
    let status: Option<Element<'static, Msg>> = status.map(|status| {
        Container::new(text::new::caption(status).style(theme::text::tertiary))
            .padding(iced::Padding {
                top: 4.0,
                ..iced::Padding::ZERO
            })
            .into()
    });
    let heading = row![
        text::new::b5_medium(text::truncate(&alias, 25)),
        pill::fingerprint(fingerprint_label, None::<&str>)
    ]
    .spacing(HSpacing::M)
    .align_y(Alignment::Center)
    .wrap();
    let body = if let Some(status) = status {
        column![heading, status]
    } else {
        column![heading]
    }
    .spacing(0)
    .width(Length::Fill);
    let trailing = if done {
        Some(pill::registered().into())
    } else if can_register {
        Some(list::right_chevron())
    } else {
        Some(text::new::caption("-").style(theme::text::tertiary).into())
    };
    let on_press = (!done && can_register).then_some(Msg::RegistrationSelectDevice(fingerprint));

    list::entry_register(
        if done {
            list::EntryRegisterStatus::Registered
        } else {
            list::EntryRegisterStatus::Unregistered
        },
        body,
        trailing,
        enabled,
        on_press,
    )
}
