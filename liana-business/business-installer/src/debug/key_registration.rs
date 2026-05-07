//! Per-key xpub fetching: `xpub_view` + xpub-entry modal (the "Set
//! Keys" wizard step where each key registers an xpub on a device).

use std::sync::{Arc, Mutex, OnceLock};

use async_hwi::service::{SigningDevice, UnsupportedReason};
use async_hwi::{DeviceKind, Version};
use liana_connect::ws_business::{KeyIdentity, UserRole};
use liana_gui::debug::{installer_chrome, installer_with_modal, DebugMessage, DebugPageEntry};
use liana_ui::widget::Element;
use miniscript::bitcoin::bip32::{ChildNumber, Fingerprint};
use miniscript::bitcoin::Network;

use crate::state::message::Message as InstallerMessage;
use crate::state::{
    views::xpub::{ModalStep as XpubModalStep, XpubEntryModalState},
    State, View,
};
use crate::views::xpub::modal::xpub_modal_view;
use crate::views::xpub_view;

use super::{add_sample_org_and_wallet, build_state, sample_xpub, StateCell};

// ---- mock device helpers ------------------------------------------------
//
// The xpub modal renders devices via `state.hw.list()` →
// `async_hwi::service::SigningDevice`. The `Locked` and `Unsupported`
// variants have public, struct-literal-constructible fields, so we can
// fabricate them. The `Supported` variant wraps a private `SupportedDevice`
// whose fields aren't `pub`, so we can't reach that branch from outside
// async-hwi — that case stays unmocked here.

fn insert_locked(s: &mut State, kind: DeviceKind, pairing_code: Option<&str>, id: &str) {
    let device: SigningDevice<InstallerMessage> = SigningDevice::Locked {
        id: id.to_string(),
        device: Arc::new(Mutex::new(None)),
        pairing_code: pairing_code.map(|s| s.to_string()),
        kind,
    };
    s.hw.devices
        .lock()
        .expect("poisoned")
        .insert(id.to_string(), device);
}

fn insert_unsupported(
    s: &mut State,
    kind: DeviceKind,
    version: Option<Version>,
    reason: UnsupportedReason,
    id: &str,
) {
    let device: SigningDevice<InstallerMessage> = SigningDevice::Unsupported {
        id: id.to_string(),
        kind,
        version,
        reason,
    };
    s.hw.devices
        .lock()
        .expect("poisoned")
        .insert(id.to_string(), device);
}

fn make_version(major: u32, minor: u32, patch: u32) -> Version {
    Version {
        major,
        minor,
        patch,
        prerelease: None,
    }
}

const XPUB_VIEW_PATH: &str = "business_installer::views::xpub::view::xpub_view";
const XPUB_MODAL_PATH: &str = "business_installer::views::xpub::modal::xpub_modal_view";

pub static ENTRY_XPUB: DebugPageEntry = DebugPageEntry { view: render_xpub };
pub static ENTRY_XPUB_PARTIAL: DebugPageEntry = DebugPageEntry {
    view: render_xpub_partial,
};
pub static ENTRY_XPUB_ALL_SET: DebugPageEntry = DebugPageEntry {
    view: render_xpub_all_set,
};
pub static ENTRY_XPUB_PARTICIPANT_NO_KEYS: DebugPageEntry = DebugPageEntry {
    view: render_xpub_participant_no_keys,
};
pub static ENTRY_XPUB_WS_ADMIN: DebugPageEntry = DebugPageEntry {
    view: render_xpub_ws_admin,
};
pub static ENTRY_XPUB_MODAL_SELECT: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select,
};
pub static ENTRY_XPUB_MODAL_SELECT_OPTIONS_EXPANDED: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_options_expanded,
};
pub static ENTRY_XPUB_MODAL_SELECT_PASTE_EXPANDED: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_paste_expanded,
};
pub static ENTRY_XPUB_MODAL_SELECT_PASTE_COLLAPSED: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_paste_collapsed,
};
pub static ENTRY_XPUB_MODAL_SELECT_WITH_CURRENT_XPUB: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_with_current_xpub,
};
pub static ENTRY_XPUB_MODAL_SELECT_LOCKED_BITBOX: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_locked_bitbox,
};
pub static ENTRY_XPUB_MODAL_SELECT_LOCKED_JADE: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_locked_jade,
};
pub static ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_VERSION_COLDCARD: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_unsupported_version_coldcard,
};
pub static ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_VERSION_JADE: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_unsupported_version_jade,
};
pub static ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_NOT_PART_OF_WALLET: DebugPageEntry =
    DebugPageEntry {
        view: render_xpub_modal_select_unsupported_not_part_of_wallet,
    };
pub static ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_WRONG_NETWORK: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_unsupported_wrong_network,
};
pub static ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_METHOD: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_unsupported_method,
};
pub static ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_APP_NOT_OPEN: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_unsupported_app_not_open,
};
pub static ENTRY_XPUB_MODAL_SELECT_ONE_DEVICE_OPTIONS_EXPANDED: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_one_device_options_expanded,
};
pub static ENTRY_XPUB_MODAL_SELECT_MULTIPLE_DEVICES: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_select_multiple_devices,
};
pub static ENTRY_XPUB_MODAL_DETAILS: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_details,
};
pub static ENTRY_XPUB_MODAL_DETAILS_FETCHING: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_details_fetching,
};
pub static ENTRY_XPUB_MODAL_DETAILS_FETCH_ERROR: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_details_fetch_error,
};
pub static ENTRY_XPUB_MODAL_DETAILS_FETCH_SUCCESS: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_details_fetch_success,
};
pub static ENTRY_XPUB_MODAL_DETAILS_WRONG_NETWORK: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_details_wrong_network,
};
pub static ENTRY_XPUB_MODAL_DETAILS_ACCOUNT_5: DebugPageEntry = DebugPageEntry {
    view: render_xpub_modal_details_account_5,
};

// ---- xpub view ----------------------------------------------------------

fn shared_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| StateCell(build_state(|_| {}))).0
}

fn render_xpub() -> Element<'static, DebugMessage> {
    let body = xpub_view(shared_state()).map(|_| ());
    installer_chrome(
        "Business installer — xpub (all keys unset)",
        XPUB_VIEW_PATH,
        body,
    )
}

fn xpub_partial_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Xpub;
            s.views
                .login
                .on_update_email("alice@example.com".to_string());
            // Mirror `xpub_all_set_state`: align Alice's identity with the
            // first seeded key so `owned_keys` is non-empty.
            if let Some(k) = s.app.keys.values_mut().next() {
                k.identity = KeyIdentity::Email("alice@example.com".to_string());
            }
            // Set the xpub on the first key only — the remaining seeded
            // keys (Bob, Alice) stay unset, so `all_keys_set` is false and
            // the cards render a mixed-status list.
            if let Some(k) = s.app.keys.values_mut().next() {
                k.xpub = Some(sample_xpub());
            }
        }))
    })
    .0
}

fn render_xpub_partial() -> Element<'static, DebugMessage> {
    let body = xpub_view(xpub_partial_state()).map(|_| ());
    installer_chrome(
        "Business installer — xpub (one key set, others unset)",
        XPUB_VIEW_PATH,
        body,
    )
}

fn xpub_all_set_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Xpub;
            s.views
                .login
                .on_update_email("alice@example.com".to_string());
            for key in s.app.keys.values_mut() {
                key.xpub = Some(sample_xpub());
            }
            // Make alice's identity match an existing key so owned_keys is
            // non-empty.
            if let Some(k) = s.app.keys.values_mut().next() {
                k.identity = KeyIdentity::Email("alice@example.com".to_string());
            }
        }))
    })
    .0
}

fn render_xpub_all_set() -> Element<'static, DebugMessage> {
    let body = xpub_view(xpub_all_set_state()).map(|_| ());
    installer_chrome(
        "Business installer — xpub (all keys set, waiting)",
        XPUB_VIEW_PATH,
        body,
    )
}

fn xpub_participant_no_keys_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Xpub;
            s.app.current_user_role = Some(UserRole::Participant);
            s.views
                .login
                .on_update_email("nobody@example.com".to_string());
        }))
    })
    .0
}

fn render_xpub_participant_no_keys() -> Element<'static, DebugMessage> {
    let body = xpub_view(xpub_participant_no_keys_state()).map(|_| ());
    installer_chrome(
        "Business installer — xpub (participant, no owned keys)",
        XPUB_VIEW_PATH,
        body,
    )
}

fn xpub_ws_admin_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Xpub;
            s.app.current_user_role = Some(UserRole::WizardSardineAdmin);
            s.views
                .login
                .on_update_email("admin@wizardsardine.com".to_string());
            add_sample_org_and_wallet(s);
        }))
    })
    .0
}

fn render_xpub_ws_admin() -> Element<'static, DebugMessage> {
    let body = xpub_view(xpub_ws_admin_state()).map(|_| ());
    installer_chrome(
        "Business installer — xpub (WS admin, breadcrumb)",
        XPUB_VIEW_PATH,
        body,
    )
}

// ---- xpub modal ---------------------------------------------------------

fn xpub_modal_state_with_step(step: XpubModalStep) -> State {
    build_state(|s| {
        s.current_view = View::Xpub;
        let mut modal = XpubEntryModalState::new(1, "Bob".to_string(), None, Network::Bitcoin);
        modal.step = step;
        s.views.xpub.modal = Some(modal);
    })
}

fn xpub_modal_select_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| StateCell(xpub_modal_state_with_step(XpubModalStep::Select)))
        .0
}

fn xpub_modal_details_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| StateCell(xpub_modal_state_with_step(XpubModalStep::Details)))
        .0
}

fn render_xpub_modal_select() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (select source)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn render_xpub_modal_details() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_details_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (details)",
        XPUB_MODAL_PATH,
        body,
    )
}

// ---- Select-step variants -----------------------------------------------

fn build_select_state(
    setup_modal: impl FnOnce(&mut XpubEntryModalState),
    setup_state: impl FnOnce(&mut State),
) -> State {
    build_state(|s| {
        s.current_view = View::Xpub;
        let mut modal = XpubEntryModalState::new(1, "Bob".to_string(), None, Network::Bitcoin);
        modal.step = XpubModalStep::Select;
        setup_modal(&mut modal);
        s.views.xpub.modal = Some(modal);
        setup_state(s);
    })
}

fn xpub_modal_select_options_expanded_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |modal| modal.options_collapsed = false,
            |_| {},
        ))
    })
    .0
}

fn render_xpub_modal_select_options_expanded() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_options_expanded_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (no devices, other options expanded)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_paste_expanded_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |modal| {
                modal.options_collapsed = false;
                // The paste input only renders for WalletManager — set on
                // the State below.
                modal.paste_expanded = true;
            },
            |s| {
                s.app.current_user_role = Some(liana_connect::ws_business::UserRole::WalletManager);
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_paste_expanded() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_paste_expanded_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (wallet manager, paste expanded)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_paste_collapsed_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |modal| {
                modal.options_collapsed = false;
                // Paste card visible, but its input row stays collapsed —
                // only the "Paste an extended public key" button shows
                // until the user clicks it.
                modal.paste_expanded = false;
            },
            |s| {
                s.app.current_user_role = Some(liana_connect::ws_business::UserRole::WalletManager);
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_paste_collapsed() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_paste_collapsed_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (wallet manager, paste card collapsed)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_with_current_xpub_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Xpub;
            // Pre-populate the modal with `current_xpub` so the "this key
            // already has an xpub" banner + Clear button surface.
            let mut modal = XpubEntryModalState::new(
                1,
                "Bob".to_string(),
                Some(sample_xpub()),
                Network::Bitcoin,
            );
            modal.step = XpubModalStep::Select;
            s.views.xpub.modal = Some(modal);
        }))
    })
    .0
}

fn render_xpub_modal_select_with_current_xpub() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_with_current_xpub_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (already has xpub: replace banner + Clear)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_locked_bitbox_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| insert_locked(s, DeviceKind::BitBox02, Some("ABC-123-XYZ"), "bb02-1"),
        ))
    })
    .0
}

fn render_xpub_modal_select_locked_bitbox() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_locked_bitbox_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (locked BitBox02 with pairing code)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_locked_jade_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| insert_locked(s, DeviceKind::Jade, None, "jade-1"),
        ))
    })
    .0
}

fn render_xpub_modal_select_locked_jade() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_locked_jade_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (locked Jade — taproot not supported card)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_unsupported_version_coldcard_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| {
                insert_unsupported(
                    s,
                    DeviceKind::Coldcard,
                    Some(make_version(5, 0, 0)),
                    UnsupportedReason::Version {
                        minimal_supported_version: "5.5.0",
                    },
                    "coldcard-1",
                );
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_unsupported_version_coldcard() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_unsupported_version_coldcard_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (unsupported version, Coldcard 5.0.0 < 5.5.0)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_unsupported_version_jade_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| {
                insert_unsupported(
                    s,
                    DeviceKind::Jade,
                    Some(make_version(1, 0, 30)),
                    // The Jade arm of the unsupported render returns the
                    // taproot-not-supported card, ignoring this version.
                    UnsupportedReason::Version {
                        minimal_supported_version: "1.0.31",
                    },
                    "jade-1",
                );
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_unsupported_version_jade() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_unsupported_version_jade_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (unsupported version, Jade — taproot not supported)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_unsupported_not_part_of_wallet_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| {
                insert_unsupported(
                    s,
                    DeviceKind::Ledger,
                    Some(make_version(2, 1, 3)),
                    UnsupportedReason::NotPartOfWallet(Fingerprint::from([0xCC; 4])),
                    "ledger-1",
                );
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_unsupported_not_part_of_wallet() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_unsupported_not_part_of_wallet_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (Ledger not part of this wallet)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_unsupported_wrong_network_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| {
                insert_unsupported(
                    s,
                    DeviceKind::BitBox02,
                    Some(make_version(9, 14, 0)),
                    UnsupportedReason::WrongNetwork,
                    "bb02-1",
                );
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_unsupported_wrong_network() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_unsupported_wrong_network_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (BitBox02 wrong network)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_unsupported_method_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| {
                insert_unsupported(
                    s,
                    DeviceKind::Specter,
                    None,
                    UnsupportedReason::Method("get_extended_pubkey"),
                    "specter-1",
                );
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_unsupported_method() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_unsupported_method_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (Specter unsupported method)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_unsupported_app_not_open_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| {
                insert_unsupported(
                    s,
                    DeviceKind::Ledger,
                    Some(make_version(2, 1, 3)),
                    UnsupportedReason::AppIsNotOpen,
                    "ledger-1",
                );
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_unsupported_app_not_open() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_unsupported_app_not_open_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (Ledger Bitcoin app not open)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_one_device_options_expanded_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |modal| modal.options_collapsed = false,
            |s| insert_locked(s, DeviceKind::BitBox02, Some("DEF-456-UVW"), "bb02-1"),
        ))
    })
    .0
}

fn render_xpub_modal_select_one_device_options_expanded() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_one_device_options_expanded_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (one locked device + other options expanded)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_select_multiple_devices_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_select_state(
            |_| {},
            |s| {
                insert_locked(s, DeviceKind::BitBox02, Some("GHI-789-RST"), "bb02-1");
                insert_unsupported(
                    s,
                    DeviceKind::Coldcard,
                    Some(make_version(5, 0, 0)),
                    UnsupportedReason::Version {
                        minimal_supported_version: "5.5.0",
                    },
                    "coldcard-1",
                );
                insert_unsupported(
                    s,
                    DeviceKind::Ledger,
                    Some(make_version(2, 1, 3)),
                    UnsupportedReason::AppIsNotOpen,
                    "ledger-1",
                );
            },
        ))
    })
    .0
}

fn render_xpub_modal_select_multiple_devices() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_select_multiple_devices_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub modal (multiple devices, mix of states)",
        XPUB_MODAL_PATH,
        body,
    )
}

// ---- Details-step variants ----------------------------------------------
//
// The Details step is reached after the user selects a hardware device on
// the Select step. Account picker visibility, retry button, and the xpub
// preview are all driven by `processing` / `fetch_error` / `xpub_input`.

fn build_details_state(
    network: Network,
    setup_modal: impl FnOnce(&mut XpubEntryModalState),
) -> State {
    build_state(|s| {
        s.current_view = View::Xpub;
        let mut modal = XpubEntryModalState::new(1, "Bob".to_string(), None, network);
        modal.step = XpubModalStep::Details;
        setup_modal(&mut modal);
        s.views.xpub.modal = Some(modal);
    })
}

fn xpub_modal_details_fetching_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_details_state(Network::Bitcoin, |modal| {
            // Mirrors `select_device`: processing flips to true, account
            // picker becomes a static label.
            modal.processing = true;
        }))
    })
    .0
}

fn render_xpub_modal_details_fetching() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_details_fetching_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub details (fetching from device)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_details_fetch_error_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_details_state(Network::Bitcoin, |modal| {
            modal.fetch_error = Some("Could not reach device. Reconnect and retry.".to_string());
        }))
    })
    .0
}

fn render_xpub_modal_details_fetch_error() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_details_fetch_error_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub details (fetch error + Retry)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_details_fetch_success_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_details_state(Network::Testnet, |modal| {
            // Sample xpub is a tpub, so the Testnet network state validates
            // it cleanly — mirrors the post-fetch "ready to save" UX.
            modal.xpub_input = sample_xpub().to_string();
        }))
    })
    .0
}

fn render_xpub_modal_details_fetch_success() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_details_fetch_success_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub details (fetch success, save enabled)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_details_wrong_network_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_details_state(Network::Bitcoin, |modal| {
            // Bitcoin-network state rejects the testnet sample xpub →
            // validate() emits "Extended public key is not valid for bitcoin".
            modal.xpub_input = sample_xpub().to_string();
        }))
    })
    .0
}

fn render_xpub_modal_details_wrong_network() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_details_wrong_network_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub details (wrong-network validation error)",
        XPUB_MODAL_PATH,
        body,
    )
}

fn xpub_modal_details_account_5_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_details_state(Network::Bitcoin, |modal| {
            modal.selected_account =
                ChildNumber::from_hardened_idx(5).expect("hardcoded valid account index");
        }))
    })
    .0
}

fn render_xpub_modal_details_account_5() -> Element<'static, DebugMessage> {
    let body = xpub_modal_view(xpub_modal_details_account_5_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — xpub details (account #5 selected)",
        XPUB_MODAL_PATH,
        body,
    )
}
