//! Wallet template creation step + per-path edit modal.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use liana_connect::ws_business::{
    Key, KeyIdentity, KeyType, Org, SpendingPath, UserRole, Wallet, WalletStatus,
};
use liana_gui::debug::{installer_chrome, installer_with_modal, DebugMessage, DebugPageEntry};
use liana_ui::widget::Element;
use uuid::Uuid;

use crate::state::{
    views::path::{EditPathModalState, TimelockUnit},
    State, View,
};
use crate::views::{paths::modal::path_modal_view, template_builder_view};

use super::{build_state, StateCell};

const TEMPLATE_PATH: &str = "business_installer::views::template_builder::template_builder_view";
const PATH_MODAL_PATH: &str = "business_installer::views::paths::modal::path_modal_view";

pub static ENTRY_TEMPLATE_BUILDER: DebugPageEntry = DebugPageEntry {
    view: render_template_builder,
};
pub static ENTRY_TEMPLATE_BUILDER_OWNER: DebugPageEntry = DebugPageEntry {
    view: render_template_builder_owner,
};
pub static ENTRY_TEMPLATE_BUILDER_WS_ADMIN: DebugPageEntry = DebugPageEntry {
    view: render_template_builder_ws_admin,
};
pub static ENTRY_TEMPLATE_BUILDER_LOCKED: DebugPageEntry = DebugPageEntry {
    view: render_template_builder_locked,
};
pub static ENTRY_TEMPLATE_BUILDER_WS_ADMIN_SINGLE_KEY: DebugPageEntry = DebugPageEntry {
    view: render_template_builder_ws_admin_single_key,
};
pub static ENTRY_PATH_MODAL_PRIMARY: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_primary,
};
pub static ENTRY_PATH_MODAL_SECONDARY: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_secondary,
};
pub static ENTRY_PATH_MODAL_RECOVERY_NO_KEYS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_no_keys,
};
pub static ENTRY_PATH_MODAL_RECOVERY_THRESHOLD_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_threshold_empty,
};
pub static ENTRY_PATH_MODAL_RECOVERY_THRESHOLD_TOO_HIGH: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_threshold_too_high,
};
pub static ENTRY_PATH_MODAL_RECOVERY_THRESHOLD_NON_NUMERIC: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_threshold_non_numeric,
};
pub static ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_timelock_empty,
};
pub static ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_ZERO: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_timelock_zero,
};
pub static ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_TOO_LARGE: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_timelock_too_large,
};
pub static ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_TOO_LARGE_BLOCKS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_timelock_too_large_blocks,
};
pub static ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_TOO_LARGE_DAYS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_timelock_too_large_days,
};
pub static ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_TOO_LARGE_MONTHS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_timelock_too_large_months,
};
pub static ENTRY_PATH_MODAL_RECOVERY_UNIT_BLOCKS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_unit_blocks,
};
pub static ENTRY_PATH_MODAL_RECOVERY_UNIT_HOURS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_unit_hours,
};
pub static ENTRY_PATH_MODAL_RECOVERY_UNIT_DAYS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_unit_days,
};
pub static ENTRY_PATH_MODAL_RECOVERY_UNIT_MONTHS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_unit_months,
};
pub static ENTRY_PATH_MODAL_RECOVERY_NO_KEYS_OTHERS_VALID: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_no_keys_others_valid,
};
pub static ENTRY_PATH_MODAL_PRIMARY_NO_KEYS: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_primary_no_keys,
};
pub static ENTRY_PATH_MODAL_PRIMARY_THRESHOLD_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_primary_threshold_empty,
};
pub static ENTRY_PATH_MODAL_PRIMARY_THRESHOLD_INVALID: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_primary_threshold_invalid,
};
pub static ENTRY_PATH_MODAL_RECOVERY_TIMELOCK_DUPLICATE: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_timelock_duplicate,
};
pub static ENTRY_PATH_MODAL_RECOVERY_THRESHOLD_AND_TIMELOCK: DebugPageEntry = DebugPageEntry {
    view: render_path_modal_recovery_threshold_and_timelock,
};

fn shared_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| StateCell(build_state(|_| {}))).0
}

fn render_template_builder() -> Element<'static, DebugMessage> {
    let body = template_builder_view(shared_state()).map(|_| ());
    installer_chrome("Business installer — template builder", TEMPLATE_PATH, body)
}

fn template_builder_with_status(role: UserRole, status: WalletStatus) -> State {
    build_state(|s| {
        s.current_view = View::WalletEdit;
        s.app.current_user_role = Some(role);
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
                    status,
                    template: None,
                    last_edited: None,
                    last_editor: None,
                    descriptor: None,
                    devices: None,
                },
            );
        }
        {
            let mut org_wallets = BTreeSet::new();
            org_wallets.insert(wallet_id);
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
    })
}

fn template_builder_owner_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(template_builder_with_status(
            UserRole::WalletManager,
            WalletStatus::Drafted,
        ))
    })
    .0
}

fn render_template_builder_owner() -> Element<'static, DebugMessage> {
    let body = template_builder_view(template_builder_owner_state()).map(|_| ());
    installer_chrome(
        "Business installer — template (wallet manager, draft)",
        TEMPLATE_PATH,
        body,
    )
}

fn template_builder_ws_admin_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(template_builder_with_status(
            UserRole::WizardSardineAdmin,
            WalletStatus::Drafted,
        ))
    })
    .0
}

fn render_template_builder_ws_admin() -> Element<'static, DebugMessage> {
    let body = template_builder_view(template_builder_ws_admin_state()).map(|_| ());
    installer_chrome(
        "Business installer — template (WS admin, draft)",
        TEMPLATE_PATH,
        body,
    )
}

fn template_builder_locked_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(template_builder_with_status(
            UserRole::WalletManager,
            WalletStatus::Locked,
        ))
    })
    .0
}

fn render_template_builder_locked() -> Element<'static, DebugMessage> {
    let body = template_builder_view(template_builder_locked_state()).map(|_| ());
    installer_chrome(
        "Business installer — template (locked)",
        TEMPLATE_PATH,
        body,
    )
}

fn template_builder_ws_admin_single_key_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell({
            let mut s =
                template_builder_with_status(UserRole::WizardSardineAdmin, WalletStatus::Drafted);
            // Single key, primary path present but empty (no key_ids), no
            // recovery paths → `is_template_valid()` returns false, so the
            // "Send for approval" button is rendered disabled.
            s.app.keys.clear();
            s.app.keys.insert(
                0,
                Key {
                    id: 0,
                    alias: "Wallet manager".to_string(),
                    description: String::new(),
                    identity: KeyIdentity::Email("owner@example.com".to_string()),
                    key_type: KeyType::Internal,
                    xpub: None,
                    xpub_source: None,
                    xpub_device_kind: None,
                    xpub_device_version: None,
                    xpub_file_name: None,
                    last_edited: None,
                    last_editor: None,
                },
            );
            s.app.next_key_id = 1;
            s.app.primary_path = SpendingPath::new(true, 1, Vec::new());
            s.app.secondary_paths.clear();
            s
        })
    })
    .0
}

fn render_template_builder_ws_admin_single_key() -> Element<'static, DebugMessage> {
    let body = template_builder_view(template_builder_ws_admin_single_key_state()).map(|_| ());
    installer_chrome(
        "Business installer — template (WS admin, single key, empty primary, send disabled)",
        TEMPLATE_PATH,
        body,
    )
}

// ---- path modal ---------------------------------------------------------

fn path_modal_state(is_primary: bool) -> State {
    build_state(|s| {
        s.views.paths.edit_path_modal = Some(EditPathModalState {
            is_primary,
            path_index: if is_primary { None } else { Some(0) },
            selected_key_ids: if is_primary { vec![0, 1] } else { vec![1, 2] },
            threshold: if is_primary {
                "2".to_string()
            } else {
                "1".to_string()
            },
            timelock_value: if is_primary {
                None
            } else {
                Some("8760".to_string())
            },
            timelock_unit: TimelockUnit::default(),
        });
    })
}

fn path_modal_primary_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| StateCell(path_modal_state(true))).0
}

fn path_modal_secondary_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| StateCell(path_modal_state(false))).0
}

fn render_path_modal_primary() -> Element<'static, DebugMessage> {
    let body = path_modal_view(path_modal_primary_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — path modal (primary path)",
        PATH_MODAL_PATH,
        body,
    )
}

fn render_path_modal_secondary() -> Element<'static, DebugMessage> {
    let body = path_modal_view(path_modal_secondary_state())
        .expect("modal state set")
        .map(|_| ());
    installer_with_modal(
        "Business installer — path modal (recovery / secondary)",
        PATH_MODAL_PATH,
        body,
    )
}

// ---- recovery path: error variants --------------------------------------
//
// `path_modal_view` re-renders all of its validation messages purely from
// `EditPathModalState`, so each variant just builds the modal in the
// targeted error state and lets the production view light it up. The
// surrounding `State` keeps the default `AppState::new` keys (3 keys) and
// secondary paths (8760 / 21900 blocks) so that:
//   * the keys checklist always has rows to choose from;
//   * the duplicate-timelock variant has another path to clash with.

fn recovery_modal_state(modal: EditPathModalState) -> State {
    build_state(|s| {
        s.views.paths.edit_path_modal = Some(modal);
    })
}

fn render_recovery_modal(
    state: &'static State,
    title: &'static str,
) -> Element<'static, DebugMessage> {
    let body = path_modal_view(state).expect("modal state set").map(|_| ());
    installer_with_modal(title, PATH_MODAL_PATH, body)
}

fn path_modal_recovery_no_keys_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            // `None` → "Create New Path" header (rather than "Edit").
            path_index: None,
            selected_key_ids: Vec::new(),
            threshold: String::new(),
            timelock_value: Some(String::new()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_no_keys() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_no_keys_state(),
        "Business installer — path modal (recovery, no keys selected)",
    )
}

fn path_modal_recovery_threshold_empty_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            // Two keys → threshold row visible; empty threshold → save
            // disabled but no warning text.
            selected_key_ids: vec![0, 1],
            threshold: String::new(),
            timelock_value: Some("48".to_string()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_threshold_empty() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_threshold_empty_state(),
        "Business installer — path modal (recovery, empty threshold)",
    )
}

fn path_modal_recovery_threshold_too_high_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![0, 1],
            // 5 > selected_count (2) → "Invalid threshold value".
            threshold: "5".to_string(),
            timelock_value: Some("48".to_string()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_threshold_too_high() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_threshold_too_high_state(),
        "Business installer — path modal (recovery, threshold > selected)",
    )
}

fn path_modal_recovery_threshold_non_numeric_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![0, 1],
            // Non-numeric → parse error → same warning.
            threshold: "abc".to_string(),
            timelock_value: Some("48".to_string()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_threshold_non_numeric() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_threshold_non_numeric_state(),
        "Business installer — path modal (recovery, threshold non-numeric)",
    )
}

fn path_modal_recovery_timelock_empty_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            // Single key → no threshold row; empty timelock → save disabled
            // (no warning row, but the field is marked invalid).
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some(String::new()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_timelock_empty() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_timelock_empty_state(),
        "Business installer — path modal (recovery, empty timelock)",
    )
}

fn path_modal_recovery_timelock_zero_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            // Parses to 0 blocks → "Timelock cannot be zero".
            timelock_value: Some("0".to_string()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_timelock_zero() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_timelock_zero_state(),
        "Business installer — path modal (recovery, timelock = 0)",
    )
}

fn path_modal_recovery_timelock_too_large_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            // Default unit is Hours; max_value(Hours) = 10922. 20000 > max
            // → "Max 10922 hours".
            timelock_value: Some("20000".to_string()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_timelock_too_large() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_timelock_too_large_state(),
        "Business installer — path modal (recovery, timelock > max for unit)",
    )
}

fn path_modal_recovery_timelock_duplicate_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            // Editing existing path index 0 (blocks 8760 in `AppState::new`).
            // 3650 hours × BLOCKS_PER_HOUR(6) = 21900 blocks, which collides
            // with the second seeded path (`AppState::new` uses 21900) → the
            // duplicate-timelock check fires.
            path_index: Some(0),
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some("3650".to_string()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_timelock_duplicate() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_timelock_duplicate_state(),
        "Business installer — path modal (recovery, duplicate timelock)",
    )
}

fn path_modal_recovery_threshold_and_timelock_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            // Two keys + invalid threshold + zero timelock — exercises both
            // error rows simultaneously.
            selected_key_ids: vec![0, 1],
            threshold: "5".to_string(),
            timelock_value: Some("0".to_string()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_threshold_and_timelock() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_threshold_and_timelock_state(),
        "Business installer — path modal (recovery, invalid threshold + zero timelock)",
    )
}

// ---- per-unit too-large warnings ----------------------------------------
//
// `MAX_TIMELOCK_BLOCKS = 65535`. Per unit: Blocks max 65535,
// Hours 10922 (65535/6), Days 455 (65535/144), Months 15 (special-cased).
// Each variant overshoots its unit so the "Max XXX <unit>" warning row
// appears below the timelock input.

fn path_modal_recovery_timelock_too_large_blocks_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some("70000".to_string()),
            timelock_unit: TimelockUnit::Blocks,
        }))
    })
    .0
}

fn render_path_modal_recovery_timelock_too_large_blocks() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_timelock_too_large_blocks_state(),
        "Business installer — path modal (recovery, timelock > max blocks)",
    )
}

fn path_modal_recovery_timelock_too_large_days_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some("500".to_string()),
            timelock_unit: TimelockUnit::Days,
        }))
    })
    .0
}

fn render_path_modal_recovery_timelock_too_large_days() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_timelock_too_large_days_state(),
        "Business installer — path modal (recovery, timelock > max days)",
    )
}

fn path_modal_recovery_timelock_too_large_months_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some("20".to_string()),
            timelock_unit: TimelockUnit::Months,
        }))
    })
    .0
}

fn render_path_modal_recovery_timelock_too_large_months() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_timelock_too_large_months_state(),
        "Business installer — path modal (recovery, timelock > max months)",
    )
}

// ---- per-unit valid variants --------------------------------------------
//
// Same shape (1 key, threshold 1) — only the timelock unit differs so the
// designer can see the unit selector + "Max: X <unit>" hint per unit.

fn path_modal_recovery_unit_blocks_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some("100".to_string()),
            timelock_unit: TimelockUnit::Blocks,
        }))
    })
    .0
}

fn render_path_modal_recovery_unit_blocks() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_unit_blocks_state(),
        "Business installer — path modal (recovery, unit = blocks)",
    )
}

fn path_modal_recovery_unit_hours_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some("48".to_string()),
            timelock_unit: TimelockUnit::Hours,
        }))
    })
    .0
}

fn render_path_modal_recovery_unit_hours() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_unit_hours_state(),
        "Business installer — path modal (recovery, unit = hours)",
    )
}

fn path_modal_recovery_unit_days_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some("30".to_string()),
            timelock_unit: TimelockUnit::Days,
        }))
    })
    .0
}

fn render_path_modal_recovery_unit_days() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_unit_days_state(),
        "Business installer — path modal (recovery, unit = days)",
    )
}

fn path_modal_recovery_unit_months_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: vec![1],
            threshold: "1".to_string(),
            timelock_value: Some("3".to_string()),
            timelock_unit: TimelockUnit::Months,
        }))
    })
    .0
}

fn render_path_modal_recovery_unit_months() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_unit_months_state(),
        "Business installer — path modal (recovery, unit = months)",
    )
}

// ---- save-disabled solely because of missing keys -----------------------
//
// Keys checklist empty, but timelock has a valid value. The threshold row
// is gated by `selected_count > 1`, so it stays hidden — the visible
// demonstration is "everything filled except the checkboxes".

fn path_modal_recovery_no_keys_others_valid_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: false,
            path_index: None,
            selected_key_ids: Vec::new(),
            // Internally valid but threshold row not rendered with 0 keys.
            threshold: "1".to_string(),
            timelock_value: Some("48".to_string()),
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_recovery_no_keys_others_valid() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_recovery_no_keys_others_valid_state(),
        "Business installer — path modal (recovery, save disabled: no keys but timelock valid)",
    )
}

// ---- primary path: error variants ---------------------------------------
//
// Primary paths have no timelock row, so the only blocking inputs are
// the keys checklist and (when 2+ keys selected) the threshold field.

fn path_modal_primary_no_keys_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: true,
            path_index: None,
            selected_key_ids: Vec::new(),
            threshold: String::new(),
            timelock_value: None,
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_primary_no_keys() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_primary_no_keys_state(),
        "Business installer — path modal (primary, no keys selected)",
    )
}

fn path_modal_primary_threshold_empty_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: true,
            path_index: None,
            // Two keys → threshold row visible; empty threshold disables save.
            selected_key_ids: vec![0, 1],
            threshold: String::new(),
            timelock_value: None,
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_primary_threshold_empty() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_primary_threshold_empty_state(),
        "Business installer — path modal (primary, empty threshold)",
    )
}

fn path_modal_primary_threshold_invalid_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(recovery_modal_state(EditPathModalState {
            is_primary: true,
            path_index: None,
            selected_key_ids: vec![0, 1],
            threshold: "5".to_string(),
            timelock_value: None,
            timelock_unit: TimelockUnit::default(),
        }))
    })
    .0
}

fn render_path_modal_primary_threshold_invalid() -> Element<'static, DebugMessage> {
    render_recovery_modal(
        path_modal_primary_threshold_invalid_state(),
        "Business installer — path modal (primary, threshold > selected)",
    )
}
