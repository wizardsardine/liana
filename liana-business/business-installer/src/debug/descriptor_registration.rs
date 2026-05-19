//! Final wallet-descriptor registration step: `registration_view` plus
//! the per-device registration modal.

use std::sync::OnceLock;

use async_hwi::DeviceKind;
use iced::widget::Column;
use liana_gui::debug::{installer_chrome, installer_with_modal, DebugMessage, DebugPageEntry};
use liana_ui::widget::Element;
use miniscript::bitcoin::bip32::Fingerprint;

use crate::state::{
    views::registration::{RegistrationModalState, RegistrationModalStep},
    State, View,
};
use crate::views::registration::{
    modal::registration_modal_view, registration_key_entry, registration_view_with_cards,
};
use crate::views::registration_view;

use super::{build_state, StateCell};

const REGISTRATION_PATH: &str = "business_installer::views::registration::registration_view";
const REGISTRATION_MODAL_PATH: &str =
    "business_installer::views::registration::modal::registration_modal_view";

pub static ENTRY_REGISTRATION: DebugPageEntry = DebugPageEntry {
    view: render_registration,
};
pub static ENTRY_REGISTRATION_WITH_DEVICES: DebugPageEntry = DebugPageEntry {
    view: render_registration_with_devices,
};
pub static ENTRY_REGISTRATION_MODAL_REGISTERING: DebugPageEntry = DebugPageEntry {
    view: render_registration_modal_registering,
};
pub static ENTRY_REGISTRATION_MODAL_CONFIRM_COLDCARD: DebugPageEntry = DebugPageEntry {
    view: render_registration_modal_confirm_coldcard,
};
pub static ENTRY_REGISTRATION_MODAL_ERROR: DebugPageEntry = DebugPageEntry {
    view: render_registration_modal_error,
};

// ---- registration view --------------------------------------------------

fn shared_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| StateCell(build_state(|_| {}))).0
}

fn render_registration() -> Element<'static, DebugMessage> {
    let body = registration_view(shared_state()).map(|_| ());
    installer_chrome("Business installer: registration", REGISTRATION_PATH, body)
}

fn registration_with_devices_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Registration;
        }))
    })
    .0
}

fn render_registration_with_devices() -> Element<'static, DebugMessage> {
    let variants: Vec<Element<'_, crate::state::message::Msg>> = vec![
        // kind = Some, alias filled: clickable "Register" with kind+fingerprint.
        registration_key_entry(
            Fingerprint::from([0xAA; 4]),
            Some(DeviceKind::BitBox02),
            true,
            "Alice".to_string(),
        ),
        // kind = Some, alias empty: clickable "Register" with only kind+fingerprint.
        registration_key_entry(
            Fingerprint::from([0xBB; 4]),
            Some(DeviceKind::Ledger),
            true,
            String::new(),
        ),
        // kind = None, connected = true, alias filled: "Device not supported or locked".
        registration_key_entry(
            Fingerprint::from([0xCC; 4]),
            None,
            true,
            "Bobjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjjj".to_string(),
        ),
        // kind = None, connected = true, alias empty.
        registration_key_entry(Fingerprint::from([0xDD; 4]), None, true, String::new()),
        // kind = None, connected = false, alias filled: "Connect the associated device to register".
        registration_key_entry(
            Fingerprint::from([0xEE; 4]),
            None,
            false,
            "Carol".to_string(),
        ),
        // kind = None, connected = false, alias empty.
        registration_key_entry(Fingerprint::from([0xFF; 4]), None, false, String::new()),
    ];
    let cards: Element<'_, crate::state::message::Msg> = Column::with_children(variants)
        .spacing(10)
        .padding([0, 20])
        .into();
    let body =
        registration_view_with_cards(registration_with_devices_state(), cards, true).map(|_| ());
    installer_chrome(
        "Business installer: registration (all key_card variants)",
        REGISTRATION_PATH,
        body,
    )
}

// ---- registration modal -------------------------------------------------

fn registration_modal_state_with(step: RegistrationModalStep, error: Option<String>) -> State {
    build_state(|s| {
        s.current_view = View::Registration;
        s.views.registration.modal = Some(RegistrationModalState {
            fingerprint: Fingerprint::from([0xAA; 4]),
            device_kind: Some(async_hwi::DeviceKind::Ledger),
            step,
            error,
        });
    })
}

fn registration_modal_registering_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(registration_modal_state_with(
            RegistrationModalStep::Registering,
            None,
        ))
    })
    .0
}

fn registration_modal_error_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(registration_modal_state_with(
            RegistrationModalStep::Error,
            Some("Could not reach the device. Reconnect and retry.".to_string()),
        ))
    })
    .0
}

fn registration_modal_confirm_coldcard_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Registration;
            // Coldcard doesn't ack the registration on the wire, so the
            // wizard pops a dedicated Yes/No confirmation step instead of
            // moving straight to "Registered".
            s.views.registration.modal = Some(RegistrationModalState {
                fingerprint: Fingerprint::from([0xCC; 4]),
                device_kind: Some(async_hwi::DeviceKind::Coldcard),
                step: RegistrationModalStep::ConfirmColdcard {
                    hmac: None,
                    wallet_name: "Acme treasury".to_string(),
                },
                error: None,
            });
        }))
    })
    .0
}

fn render_registration_modal_registering() -> Element<'static, DebugMessage> {
    let body = registration_modal_view(registration_modal_registering_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer: registration modal (registering)",
        REGISTRATION_MODAL_PATH,
        body,
    )
}

fn render_registration_modal_error() -> Element<'static, DebugMessage> {
    let body = registration_modal_view(registration_modal_error_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer: registration modal (error)",
        REGISTRATION_MODAL_PATH,
        body,
    )
}

fn render_registration_modal_confirm_coldcard() -> Element<'static, DebugMessage> {
    let body = registration_modal_view(registration_modal_confirm_coldcard_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer: registration modal (Coldcard Yes/No confirmation)",
        REGISTRATION_MODAL_PATH,
        body,
    )
}
