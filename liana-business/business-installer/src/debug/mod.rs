//! Business-installer debug stack.
//!
//! Surfaces every step view in the wizard plus the warning / conflict
//! modals through the debug overlay. Aggregated into [`INSTALLER_STACK`]
//! and re-exported by `liana-business` so its `EXTRA_STACKS` slice picks
//! it up (see `liana_business::debug::EXTRA_STACKS`).
//!
//! The pages are split into one submodule per wizard step so a designer
//! can browse them in their natural order: `login` → `orgs` → `wallets`
//! → `template_creation` → `keys` → `key_registration` (xpub fetch) →
//! `descriptor_registration` (wallet descriptor on devices). Cross-cutting
//! modals (warning / conflict) live at the end of [`INSTALLER_STACK`] and
//! are defined in this file.
//!
//! Each state-based page builds a stripped-down [`State`] via
//! `State::for_debug` (no tokio runtime, no HW bridge thread) and mutates
//! `views.*` to reach the targeted scenario. View functions never read
//! `state.backend`/`state.hw`, so the stub state renders faithfully.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::OnceLock;

use liana::miniscript::descriptor::DescriptorPublicKey;
use liana_connect::ws_business::{Org, Wallet, WalletStatus};
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

pub mod descriptor_registration;
pub mod key_registration;
pub mod keys;
pub mod login;
pub mod orgs;
pub mod template_creation;
pub mod wallets;

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

const SAMPLE_XPUB: &str = "[19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*";

pub(super) fn sample_xpub() -> DescriptorPublicKey {
    DescriptorPublicKey::from_str(SAMPLE_XPUB).expect("sample xpub parses")
}

/// Common helper: install an org + wallet in the backend so the breadcrumb
/// renders org / wallet names instead of placeholders.
pub(super) fn add_sample_org_and_wallet(s: &mut State) {
    let org_id = Uuid::from_u128(0x4000);
    let wallet_id = Uuid::from_u128(0x5000);
    {
        let mut wallets = s.backend.wallets.lock().expect("poisoned");
        wallets.insert(
            wallet_id,
            Wallet {
                alias: "Acme treasury".to_string(),
                org: org_id,
                owner: Uuid::nil(),
                id: wallet_id,
                status: WalletStatus::Drafted,
                template: None,
                last_edited: None,
                last_editor: None,
                descriptor: None,
                devices: None,
            },
        );
    }
    let mut org_wallets = BTreeSet::new();
    org_wallets.insert(wallet_id);
    {
        let mut orgs = s.backend.orgs.lock().expect("poisoned");
        orgs.insert(
            org_id,
            Org {
                name: "Acme Vault".to_string(),
                id: org_id,
                wallets: org_wallets,
                users: BTreeSet::new(),
                owners: Vec::new(),
                last_edited: None,
                last_editor: None,
            },
        );
    }
    s.app.selected_org = Some(org_id);
    s.app.selected_wallet = Some(wallet_id);
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
        // org select
        &orgs::ENTRY_ORG_SELECT_WITH_ORGS,
        // wallet select
        &wallets::ENTRY_WALLET_SELECT_WITH_WALLETS,
        // template creation
        &template_creation::ENTRY_TEMPLATE_BUILDER,
        &template_creation::ENTRY_TEMPLATE_BUILDER_OWNER,
        &template_creation::ENTRY_TEMPLATE_BUILDER_WS_ADMIN,
        &template_creation::ENTRY_TEMPLATE_BUILDER_WS_ADMIN_SINGLE_KEY,
        &template_creation::ENTRY_TEMPLATE_BUILDER_LOCKED,
        &template_creation::ENTRY_PATH_MODAL_PRIMARY,
        &template_creation::ENTRY_PATH_MODAL_PRIMARY_NO_KEYS,
        &template_creation::ENTRY_PATH_MODAL_PRIMARY_THRESHOLD_EMPTY,
        &template_creation::ENTRY_PATH_MODAL_PRIMARY_THRESHOLD_INVALID,
        &template_creation::ENTRY_PATH_MODAL_SECONDARY,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_UNIT_BLOCKS,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_UNIT_HOURS,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_UNIT_DAYS,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_UNIT_MONTHS,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_NO_KEYS,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_NO_KEYS_OTHERS_VALID,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_THRESHOLD_EMPTY,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_THRESHOLD_TOO_HIGH,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_THRESHOLD_NON_NUMERIC,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_EMPTY,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_ZERO,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_TOO_LARGE,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_TOO_LARGE_BLOCKS,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_TOO_LARGE_DAYS,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_TOO_LARGE_MONTHS,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_DUPLICATE,
        &template_creation::ENTRY_PATH_MODAL_RECOVERY_THRESHOLD_AND_TIMELOCK,
        // keys metadata
        &keys::ENTRY_KEYS_EMPTY,
        &keys::ENTRY_KEYS_WITH_BREADCRUMB,
        &keys::ENTRY_KEYS_MANY,
        &keys::ENTRY_KEY_MODAL_NEW_EMPTY,
        &keys::ENTRY_KEY_MODAL_NEW_ALIASED,
        &keys::ENTRY_KEY_MODAL_EXTERNAL,
        &keys::ENTRY_KEY_MODAL_INTERNAL,
        &keys::ENTRY_KEY_MODAL_COSIGNER,
        &keys::ENTRY_KEY_MODAL_SAFETY_NET,
        &keys::ENTRY_KEY_MODAL_INVALID_EMAIL,
        &keys::ENTRY_KEY_MODAL_EMPTY_ALIAS,
        // key registration (xpub fetching)
        &key_registration::ENTRY_XPUB,
        &key_registration::ENTRY_XPUB_PARTIAL,
        &key_registration::ENTRY_XPUB_ALL_SET,
        &key_registration::ENTRY_XPUB_PARTICIPANT_NO_KEYS,
        &key_registration::ENTRY_XPUB_WS_ADMIN,
        &key_registration::ENTRY_XPUB_MODAL_SELECT,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_OPTIONS_EXPANDED,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_PASTE_EXPANDED,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_PASTE_COLLAPSED,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_WITH_CURRENT_XPUB,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_LOCKED_BITBOX,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_LOCKED_JADE,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_VERSION_COLDCARD,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_VERSION_JADE,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_NOT_PART_OF_WALLET,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_WRONG_NETWORK,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_METHOD,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_UNSUPPORTED_APP_NOT_OPEN,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_ONE_DEVICE_OPTIONS_EXPANDED,
        &key_registration::ENTRY_XPUB_MODAL_SELECT_MULTIPLE_DEVICES,
        &key_registration::ENTRY_XPUB_MODAL_DETAILS,
        &key_registration::ENTRY_XPUB_MODAL_DETAILS_FETCHING,
        &key_registration::ENTRY_XPUB_MODAL_DETAILS_FETCH_ERROR,
        &key_registration::ENTRY_XPUB_MODAL_DETAILS_FETCH_SUCCESS,
        &key_registration::ENTRY_XPUB_MODAL_DETAILS_WRONG_NETWORK,
        &key_registration::ENTRY_XPUB_MODAL_DETAILS_ACCOUNT_5,
        // descriptor registration
        &descriptor_registration::ENTRY_REGISTRATION,
        &descriptor_registration::ENTRY_REGISTRATION_WITH_DEVICES,
        &descriptor_registration::ENTRY_REGISTRATION_MODAL_REGISTERING,
        &descriptor_registration::ENTRY_REGISTRATION_MODAL_CONFIRM_COLDCARD,
        &descriptor_registration::ENTRY_REGISTRATION_MODAL_ERROR,
        // cross-cutting modals
        &ENTRY_WARNING_MODAL,
        &ENTRY_CONFLICT_MODAL_INFO,
        &ENTRY_CONFLICT_MODAL_CHOICE,
        &login::ENTRY_LOADING_OK,
        &login::ENTRY_LOADING_ERROR,
    ],
};
