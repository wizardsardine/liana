//! Business-installer debug stack.
//!
//! Surfaces every step view in the wizard plus the warning / conflict
//! modals through the debug overlay. Aggregated into [`INSTALLER_STACK`]
//! and re-exported by `liana-business` so its `EXTRA_STACKS` slice picks
//! it up (see `liana_business::debug::EXTRA_STACKS`).
//!
//! This commit lays down the scaffold and the cross-cutting modal pages.
//! Per-step submodules (`login`, `orgs`, `wallets`, …) are added in
//! follow-up commits.

use std::path::PathBuf;
use std::sync::OnceLock;

use liana_gui::{
    debug::{installer_with_modal, DebugMessage, DebugPageEntry, DebugStack},
    dir::LianaDirectory,
    services::connect::client::auth::AccessTokenResponse,
};
use liana_ui::widget::Element;
use miniscript::bitcoin::Network;
use uuid::Uuid;

use crate::state::{
    views::modals::{ConflictModalState, ConflictType, WarningModalState},
    State,
};
use crate::views::{modals::conflict::conflict_modal_view, modals::warning::warning_modal_view};

pub mod login;

/// SAFETY: iced renders on the main thread; debug-overlay state is only
/// read during rendering.
pub(super) struct StateCell<T>(pub(super) T);
unsafe impl<T> Sync for StateCell<T> {}

pub(super) fn datadir() -> LianaDirectory {
    LianaDirectory::new(PathBuf::new())
}

pub(super) fn build_state(setup: impl FnOnce(&mut State)) -> State {
    let mut s = State::for_debug(Network::Bitcoin, datadir());
    setup(&mut s);
    s
}

pub(super) fn stub_tokens() -> AccessTokenResponse {
    AccessTokenResponse {
        access_token: String::new(),
        expires_at: 0,
        refresh_token: String::new(),
    }
}

// ---- cross-cutting modals -----------------------------------------------

const WARNING_MODAL_PATH: &str = "business_installer::views::modals::warning::warning_modal_view";
const CONFLICT_MODAL_PATH: &str =
    "business_installer::views::modals::conflict::conflict_modal_view";

pub static ENTRY_WARNING_MODAL: DebugPageEntry = DebugPageEntry {
    view: render_warning_modal,
};
pub static ENTRY_CONFLICT_MODAL_INFO: DebugPageEntry = DebugPageEntry {
    view: render_conflict_modal_info,
};
pub static ENTRY_CONFLICT_MODAL_CHOICE: DebugPageEntry = DebugPageEntry {
    view: render_conflict_modal_choice,
};

fn warning_state() -> &'static WarningModalState {
    static S: OnceLock<WarningModalState> = OnceLock::new();
    S.get_or_init(|| {
        WarningModalState::new(
            "Wallet not registered".to_string(),
            "The wallet descriptor is not registered on the device.\nYou can register it in the settings.".to_string(),
        )
    })
}

fn render_warning_modal() -> Element<'static, DebugMessage> {
    let body = warning_modal_view(warning_state()).map(|_| ());
    installer_with_modal(
        "Business installer — warning modal",
        WARNING_MODAL_PATH,
        body,
    )
}

fn conflict_info_state() -> &'static ConflictModalState {
    static S: OnceLock<ConflictModalState> = OnceLock::new();
    S.get_or_init(|| ConflictModalState {
        conflict_type: ConflictType::KeyDeleted,
        title: "Key deleted".to_string(),
        message: "The key you were editing was deleted by another user.".to_string(),
    })
}

fn render_conflict_modal_info() -> Element<'static, DebugMessage> {
    let body = conflict_modal_view(conflict_info_state()).map(|_| ());
    installer_with_modal(
        "Business installer — conflict modal (info-only)",
        CONFLICT_MODAL_PATH,
        body,
    )
}

fn conflict_choice_state() -> &'static ConflictModalState {
    static S: OnceLock<ConflictModalState> = OnceLock::new();
    S.get_or_init(|| ConflictModalState {
        conflict_type: ConflictType::KeyModified {
            key_id: 1,
            wallet_id: Uuid::nil(),
        },
        title: "Key modified".to_string(),
        message: "The key you were editing has been modified by another user.\nReload to see the latest changes, or keep your edits.".to_string(),
    })
}

fn render_conflict_modal_choice() -> Element<'static, DebugMessage> {
    let body = conflict_modal_view(conflict_choice_state()).map(|_| ());
    installer_with_modal(
        "Business installer — conflict modal (choice)",
        CONFLICT_MODAL_PATH,
        body,
    )
}

// ---- aggregated stack ---------------------------------------------------

pub const INSTALLER_STACK: DebugStack = DebugStack {
    name: "Business installer",
    menu: None,
    pages: &[
        // Login
        &login::ENTRY_EMAIL_EMPTY,
        &login::ENTRY_EMAIL,
        &login::ENTRY_EMAIL_INVALID,
        &login::ENTRY_CODE_INVALID,
        &login::ENTRY_CODE_EMPTY,
        &login::ENTRY_CODE,
        // Select account
        &login::ENTRY_ACCOUNT_SELECT,
        &login::ENTRY_ACCOUNT_SELECT_MANY,
        &login::ENTRY_ACCOUNT_SELECT_PROCESSING,
        // cross-cutting modals
        &ENTRY_WARNING_MODAL,
        &ENTRY_CONFLICT_MODAL_INFO,
        &ENTRY_CONFLICT_MODAL_CHOICE,
        &login::ENTRY_LOADING_OK,
        &login::ENTRY_LOADING_ERROR,
    ],
};
