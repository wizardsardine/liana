use crate::{
    backend::Backend,
    state::{message::Msg, State},
    views::{delete_btn, format_last_edit_info},
};
use iced::{widget::Space, Length};
use liana_connect::ws_business::{
    self, UserRole, WalletStatus, BLOCKS_PER_DAY, BLOCKS_PER_HOUR, BLOCKS_PER_MONTH,
};
use liana_ui::{
    component::list::{self, EntryPathRole},
    widget::*,
};
use std::collections::BTreeMap;

/// Convert a timelock (in blocks) to a human-readable format.
/// Shows "After x hours/days/months" (whichever is most appropriate).
fn format_timelock_human(timelock: &ws_business::Timelock) -> String {
    let blocks = timelock.blocks;

    if blocks == 0 {
        return "No timelock".to_string();
    }

    // Determine the most appropriate unit
    if blocks >= BLOCKS_PER_MONTH {
        let months = blocks / BLOCKS_PER_MONTH;
        if months == 1 {
            "After 1 month".to_string()
        } else {
            format!("After {months} months")
        }
    } else if blocks >= BLOCKS_PER_DAY {
        let days = blocks / BLOCKS_PER_DAY;
        if days == 1 {
            "After 1 day".to_string()
        } else {
            format!("After {days} days")
        }
    } else {
        let hours = blocks / BLOCKS_PER_HOUR;
        if hours <= 1 {
            "After 1 hour".to_string()
        } else {
            format!("After {hours} hours")
        }
    }
}

/// Create a path card displaying key names and timelock information.
/// is_primary: true for primary path, false for secondary paths
/// path_index: None for primary, Some(index) for secondary paths
/// timelock is None for primary path ("Spendable anytime"), Some for secondary paths.
/// is_editable: if true, card is clickable; if false, card is read-only
fn path_card(
    path: &ws_business::SpendingPath,
    keys: &BTreeMap<u8, ws_business::Key>,
    timelock: Option<&ws_business::Timelock>,
    is_primary: bool,
    path_index: Option<usize>,
    is_editable: bool,
    last_edit_info: Option<String>,
) -> Element<'static, Msg> {
    // Get key aliases
    let key_aliases: Vec<String> = path
        .key_ids
        .iter()
        .filter_map(|id| keys.get(id).map(|k| k.alias.clone()))
        .collect();

    let key_count = key_aliases.len();
    let threshold = path.threshold_n as usize;

    let keys_text = if key_aliases.is_empty() {
        "No keys".to_string()
    } else if key_count == 1 {
        // Single key: just show the name
        key_aliases[0].clone()
    } else {
        let names = key_aliases.join(", ");
        if threshold >= key_count {
            format!("All of {names}")
        } else {
            format!("{threshold} of {names}")
        }
    };

    // Determine timelock text
    let timelock_text = match timelock {
        None => "Spendable anytime".to_string(),
        Some(tl) => format_timelock_human(tl),
    };

    let subtitle = match last_edit_info {
        Some(info) => format!("{timelock_text} - {info}"),
        None => timelock_text,
    };

    let message = if is_editable {
        Some(Msg::TemplateEditPath(is_primary, path_index))
    } else {
        None
    };
    let role = if is_primary {
        EntryPathRole::Primary
    } else {
        EntryPathRole::Recovery
    };
    let trailing = if is_editable && !is_primary {
        path_index.map(|index| delete_btn(Some(Msg::TemplateDeleteSecondaryPath(index))).into())
    } else {
        None
    };

    list::entry_path(role, keys_text, Some(subtitle), trailing, message)
}

pub fn template_visualization(state: &State) -> Element<'static, Msg> {
    let primary_path = &state.app.primary_path;
    let secondary_paths = &state.app.secondary_paths;
    let keys = &state.app.keys;
    let current_user_email_lower = state.views.login.email.form.value.to_lowercase();

    // Get current wallet status
    let wallet_status = state
        .app
        .selected_wallet
        .and_then(|id| state.backend.get_wallet(id))
        .map(|w| w.status);

    // Determine if user can edit: WS Admin only, and only when status is Draft
    let is_draft = matches!(
        wallet_status,
        Some(WalletStatus::Created) | Some(WalletStatus::Drafted)
    );
    let is_editable = matches!(
        state.app.current_user_role,
        Some(UserRole::WizardSardineAdmin)
    ) && is_draft;

    let mut column = Column::new()
        .spacing(10)
        .padding(20.0)
        .push(Space::with_height(50));

    let primary_last_edit = format_last_edit_info(
        primary_path.last_edited,
        primary_path.last_editor,
        state,
        &current_user_email_lower,
    );
    column = column.push(path_card(
        primary_path,
        keys,
        None,
        true,
        None,
        is_editable,
        primary_last_edit,
    ));

    for (index, secondary) in secondary_paths.iter().enumerate() {
        let secondary_last_edit = format_last_edit_info(
            secondary.path.last_edited,
            secondary.path.last_editor,
            state,
            &current_user_email_lower,
        );
        column = column.push(path_card(
            &secondary.path,
            keys,
            Some(&secondary.timelock),
            false,
            Some(index),
            is_editable,
            secondary_last_edit,
        ));
    }

    // "Add a recovery path" card - only show if editable (WS Admin)
    if is_editable {
        column = column.push(list::entry_path(
            EntryPathRole::Recovery,
            "+ Add a recovery path",
            None::<String>,
            None,
            Some(Msg::TemplateNewPathModal),
        ));
    }

    Container::new(column)
        .center_x(Length::Shrink)
        .center_y(Length::Shrink)
        .into()
}
