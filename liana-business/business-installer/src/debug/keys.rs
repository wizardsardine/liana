//! Keys metadata management — `keys_view` + the edit-key modal.

use std::sync::OnceLock;

use liana_connect::ws_business::{Key, KeyIdentity, KeyType as WsKeyType};
use liana_gui::debug::{installer_chrome, installer_with_modal, DebugMessage, DebugPageEntry};
use liana_ui::widget::Element;

use crate::state::{views::keys::EditKeyModalState, State, View};
use crate::views::keys::modal::key_modal_view;
use crate::views::keys_view;

use super::{add_sample_org_and_wallet, build_state, StateCell};

/// Append a Cosigner and a SafetyNet entry to the default key list so the
/// `keys_view` rows exercise every `KeyType` variant (Internal/External
/// already come from `AppState::new`).
fn extend_with_cosigner_and_safety_net(s: &mut State) {
    let cosigner_id = s.app.next_key_id;
    s.app.keys.insert(
        cosigner_id,
        Key {
            id: cosigner_id,
            alias: "Provider cosigner".to_string(),
            description: String::new(),
            identity: KeyIdentity::Token("TKN-COSIG-1234".to_string()),
            key_type: WsKeyType::Cosigner,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    let safety_id = cosigner_id + 1;
    s.app.keys.insert(
        safety_id,
        Key {
            id: safety_id,
            alias: "Safety net".to_string(),
            description: String::new(),
            identity: KeyIdentity::Token("TKN-SAFETY-9999".to_string()),
            key_type: WsKeyType::SafetyNet,
            xpub: None,
            xpub_source: None,
            xpub_device_kind: None,
            xpub_device_version: None,
            xpub_file_name: None,
            last_edited: None,
            last_editor: None,
        },
    );
    s.app.next_key_id = safety_id + 1;
}

/// Build a long key list (mix of types, alphabetised aliases) so the
/// `keys_view` rows overflow the viewport and the inner scroll surfaces.
fn extend_with_many_keys(s: &mut State) {
    s.app.keys.clear();
    let mut next = 0u8;
    let entries: &[(WsKeyType, &str, KeyIdentity)] = &[
        (
            WsKeyType::Internal,
            "Wallet manager",
            KeyIdentity::Email("owner@example.com".to_string()),
        ),
        (
            WsKeyType::External,
            "Alice",
            KeyIdentity::Email("alice@example.com".to_string()),
        ),
        (
            WsKeyType::External,
            "Bob",
            KeyIdentity::Email("bob@example.com".to_string()),
        ),
        (
            WsKeyType::External,
            "Carol",
            KeyIdentity::Email("carol@example.com".to_string()),
        ),
        (
            WsKeyType::External,
            "Dave",
            KeyIdentity::Email("dave@example.com".to_string()),
        ),
        (
            WsKeyType::External,
            "Eve",
            KeyIdentity::Email("eve@example.com".to_string()),
        ),
        (
            WsKeyType::External,
            "Frank",
            KeyIdentity::Email("frank@example.com".to_string()),
        ),
        (
            WsKeyType::External,
            "Grace",
            KeyIdentity::Email("grace@example.com".to_string()),
        ),
        (
            WsKeyType::Cosigner,
            "Provider cosigner",
            KeyIdentity::Token("TKN-COSIG-1234".to_string()),
        ),
        (
            WsKeyType::SafetyNet,
            "Safety net",
            KeyIdentity::Token("TKN-SAFETY-9999".to_string()),
        ),
    ];
    for (key_type, alias, identity) in entries {
        s.app.keys.insert(
            next,
            Key {
                id: next,
                alias: (*alias).to_string(),
                description: String::new(),
                identity: identity.clone(),
                key_type: *key_type,
                xpub: None,
                xpub_source: None,
                xpub_device_kind: None,
                xpub_device_version: None,
                xpub_file_name: None,
                last_edited: None,
                last_editor: None,
            },
        );
        next += 1;
    }
    s.app.next_key_id = next;
}

const KEYS_PATH: &str = "business_installer::views::keys::keys_view";
const KEY_MODAL_PATH: &str = "business_installer::views::keys::modal::edit_key_modal_view";

pub static ENTRY_KEYS_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_keys_empty,
};
pub static ENTRY_KEYS_WITH_BREADCRUMB: DebugPageEntry = DebugPageEntry {
    view: render_keys_with_breadcrumb,
};
pub static ENTRY_KEYS_MANY: DebugPageEntry = DebugPageEntry {
    view: render_keys_many,
};
pub static ENTRY_KEY_MODAL_NEW_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_key_modal_new_empty,
};
pub static ENTRY_KEY_MODAL_NEW_ALIASED: DebugPageEntry = DebugPageEntry {
    view: render_key_modal_new_aliased,
};
pub static ENTRY_KEY_MODAL_EXTERNAL: DebugPageEntry = DebugPageEntry {
    view: render_key_modal_external,
};
pub static ENTRY_KEY_MODAL_INTERNAL: DebugPageEntry = DebugPageEntry {
    view: render_key_modal_internal,
};
pub static ENTRY_KEY_MODAL_COSIGNER: DebugPageEntry = DebugPageEntry {
    view: render_key_modal_cosigner,
};
pub static ENTRY_KEY_MODAL_SAFETY_NET: DebugPageEntry = DebugPageEntry {
    view: render_key_modal_safety_net,
};
pub static ENTRY_KEY_MODAL_INVALID_EMAIL: DebugPageEntry = DebugPageEntry {
    view: render_key_modal_invalid_email,
};
pub static ENTRY_KEY_MODAL_EMPTY_ALIAS: DebugPageEntry = DebugPageEntry {
    view: render_key_modal_empty_alias,
};

// ---- keys view ----------------------------------------------------------

fn keys_empty_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Keys;
            s.app.keys.clear();
        }))
    })
    .0
}

fn render_keys_empty() -> Element<'static, DebugMessage> {
    let body = keys_view(keys_empty_state()).map(|_| ());
    installer_chrome("Business installer — keys (empty)", KEYS_PATH, body)
}

fn keys_with_breadcrumb_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Keys;
            add_sample_org_and_wallet(s);
            extend_with_cosigner_and_safety_net(s);
        }))
    })
    .0
}

fn render_keys_with_breadcrumb() -> Element<'static, DebugMessage> {
    let body = keys_view(keys_with_breadcrumb_state()).map(|_| ());
    installer_chrome(
        "Business installer — keys (with org/wallet breadcrumb)",
        KEYS_PATH,
        body,
    )
}

fn keys_many_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Keys;
            extend_with_many_keys(s);
        }))
    })
    .0
}

fn render_keys_many() -> Element<'static, DebugMessage> {
    let body = keys_view(keys_many_state()).map(|_| ());
    installer_chrome(
        "Business installer — keys (many entries, overflows viewport)",
        KEYS_PATH,
        body,
    )
}

// ---- edit-key modal ----------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn build_key_modal_state(
    is_new: bool,
    key_type: WsKeyType,
    alias: &str,
    description: &str,
    email: &str,
    token: &str,
    token_warning: Option<&'static str>,
) -> State {
    build_state(|s| {
        s.current_view = View::Keys;
        s.views.keys.edit_key_modal = Some(EditKeyModalState {
            key_id: if is_new { 99 } else { 1 },
            alias: alias.to_string(),
            description: description.to_string(),
            key_type,
            is_new,
            email: email.to_string(),
            token: token.to_string(),
            token_warning,
        });
    })
}

fn key_modal_new_empty_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_key_modal_state(
            true,
            WsKeyType::External,
            "",
            "",
            "",
            "",
            None,
        ))
    })
    .0
}

fn render_key_modal_new_empty() -> Element<'static, DebugMessage> {
    let body = key_modal_view(key_modal_new_empty_state())
        .expect("modal state set above")
        .map(|_| ());
    installer_with_modal(
        "Business installer — edit-key (new, empty)",
        KEY_MODAL_PATH,
        body,
    )
}

fn key_modal_new_aliased_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_key_modal_state(
            true,
            WsKeyType::External,
            "Cosigner key",
            "",
            "carol@example.com",
            "",
            None,
        ))
    })
    .0
}

fn render_key_modal_new_aliased() -> Element<'static, DebugMessage> {
    let body = key_modal_view(key_modal_new_aliased_state())
        .expect("modal state set above")
        .map(|_| ());
    installer_with_modal(
        "Business installer — edit-key (new, aliased)",
        KEY_MODAL_PATH,
        body,
    )
}

fn key_modal_external_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_key_modal_state(
            false,
            WsKeyType::External,
            "Bob",
            "",
            "bob@example.com",
            "",
            None,
        ))
    })
    .0
}

fn render_key_modal_external() -> Element<'static, DebugMessage> {
    let body = key_modal_view(key_modal_external_state())
        .expect("modal state set above")
        .map(|_| ());
    installer_with_modal(
        "Business installer — edit-key (existing, External)",
        KEY_MODAL_PATH,
        body,
    )
}

fn key_modal_internal_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_key_modal_state(
            false,
            WsKeyType::Internal,
            "Wallet manager",
            "",
            "owner@example.com",
            "",
            None,
        ))
    })
    .0
}

fn render_key_modal_internal() -> Element<'static, DebugMessage> {
    let body = key_modal_view(key_modal_internal_state())
        .expect("modal state set above")
        .map(|_| ());
    installer_with_modal(
        "Business installer — edit-key (existing, Internal)",
        KEY_MODAL_PATH,
        body,
    )
}

fn key_modal_cosigner_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_key_modal_state(
            true,
            WsKeyType::Cosigner,
            "Cosigner",
            "",
            "",
            // The "TKN-..." string isn't a valid `liana_connect` token, so
            // production's `on_key_update_token` would have stamped this
            // warning. Mirror that here instead of leaving the input
            // silently green.
            "TKN-1234-5678",
            Some("Invalid token!"),
        ))
    })
    .0
}

fn render_key_modal_cosigner() -> Element<'static, DebugMessage> {
    let body = key_modal_view(key_modal_cosigner_state())
        .expect("modal state set above")
        .map(|_| ());
    installer_with_modal(
        "Business installer — edit-key (Cosigner with token)",
        KEY_MODAL_PATH,
        body,
    )
}

fn key_modal_safety_net_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_key_modal_state(
            true,
            WsKeyType::SafetyNet,
            "Safety net",
            "",
            "",
            "TKN-SAFETY-9999",
            // Same as the Cosigner variant — placeholder string, so the
            // real-flow token-format check would have rejected it.
            Some("Invalid token!"),
        ))
    })
    .0
}

fn render_key_modal_safety_net() -> Element<'static, DebugMessage> {
    let body = key_modal_view(key_modal_safety_net_state())
        .expect("modal state set above")
        .map(|_| ());
    installer_with_modal(
        "Business installer — edit-key (SafetyNet with token)",
        KEY_MODAL_PATH,
        body,
    )
}

fn key_modal_invalid_email_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_key_modal_state(
            true,
            WsKeyType::External,
            "Bad email",
            "",
            "not-an-email",
            "",
            None,
        ))
    })
    .0
}

fn render_key_modal_invalid_email() -> Element<'static, DebugMessage> {
    let body = key_modal_view(key_modal_invalid_email_state())
        .expect("modal state set above")
        .map(|_| ());
    installer_with_modal(
        "Business installer — edit-key (invalid email)",
        KEY_MODAL_PATH,
        body,
    )
}

fn key_modal_empty_alias_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_key_modal_state(
            true,
            WsKeyType::External,
            // Alias deliberately empty: email is filled so the
            // alias-required validation is the only thing missing.
            // (Description has no input in the modal, so it stays empty.)
            "",
            "",
            "carol@example.com",
            "",
            None,
        ))
    })
    .0
}

fn render_key_modal_empty_alias() -> Element<'static, DebugMessage> {
    let body = key_modal_view(key_modal_empty_alias_state())
        .expect("modal state set above")
        .map(|_| ());
    installer_with_modal(
        "Business installer — edit-key (empty alias only)",
        KEY_MODAL_PATH,
        body,
    )
}
