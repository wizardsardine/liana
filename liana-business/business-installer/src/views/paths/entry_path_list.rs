use std::collections::BTreeMap;

use crate::{
    state::{message::Msg, State},
    views::entry_key_kind,
};
use iced::{widget::column, Length};
use liana_connect::ws_business::{self, BLOCKS_PER_DAY, BLOCKS_PER_HOUR, BLOCKS_PER_MONTH};
use liana_ui::{
    component::{
        list::{self, EntryPathRole},
        pill,
    },
    spacing::VSpacing,
    widget::*,
};

fn format_timelock_human(timelock: &ws_business::Timelock) -> String {
    let blocks = timelock.blocks;

    if blocks == 0 {
        return "No timelock".to_string();
    }

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

fn key_pills<'a>(
    path: &'a ws_business::SpendingPath,
    keys: &'a BTreeMap<u8, ws_business::Key>,
) -> Vec<Element<'a, Msg>> {
    path.key_ids
        .iter()
        .filter_map(|key_id| keys.get(key_id))
        .map(|key| pill::key_kind(entry_key_kind(&key.key_type), key.alias.as_str()).into())
        .collect()
}

fn summary(path: &ws_business::SpendingPath) -> String {
    let key_count = path.key_ids.len();
    let threshold = usize::min(path.threshold_n as usize, key_count);
    format!("{threshold} of {key_count} keys required to spend")
}

fn availability_pill(timelock: Option<&ws_business::Timelock>) -> Element<'static, Msg> {
    match timelock {
        Some(timelock) => pill::path_timelock(format_timelock_human(timelock)).into(),
        None => pill::path_always_available().into(),
    }
}

fn path_title(is_primary: bool, path_index: Option<usize>) -> String {
    match (is_primary, path_index) {
        (true, _) => "Primary path".to_string(),
        (false, Some(index)) => format!("Recovery path {}", index + 1),
        (false, None) => "Recovery path".to_string(),
    }
}

fn entry_path<'a>(
    path: &'a ws_business::SpendingPath,
    keys: &'a BTreeMap<u8, ws_business::Key>,
    timelock: Option<&'a ws_business::Timelock>,
    is_primary: bool,
    path_index: Option<usize>,
    editable: bool,
    has_delete: bool,
) -> Element<'a, Msg> {
    let role = if is_primary {
        EntryPathRole::Primary
    } else {
        EntryPathRole::Recovery
    };
    let on_delete = if editable && !is_primary {
        path_index.map(Msg::TemplateDeleteSecondaryPath)
    } else {
        None
    };
    let message = editable.then_some(Msg::TemplateEditPath(is_primary, path_index));

    list::entry_path(
        role,
        path_title(is_primary, path_index),
        summary(path),
        availability_pill(timelock),
        key_pills(path, keys),
        has_delete,
        on_delete,
        message,
    )
}

pub fn path_list<'a>(state: &'a State, editable: bool) -> Element<'a, Msg> {
    let keys = state.app.keys();
    // The deletable width is only reserved when at least one recovery path shows a delete button,
    // so a lone primary lines up at the standard width.
    let has_delete = editable && !state.app.secondary_paths().is_empty();

    let mut list = column![entry_path(
        state.app.primary_path(),
        keys,
        None,
        true,
        None,
        editable,
        has_delete,
    )]
    .spacing(VSpacing::M)
    .width(Length::Fill);

    for (index, secondary) in state.app.secondary_paths().iter().enumerate() {
        list = list.push(entry_path(
            &secondary.path,
            keys,
            Some(&secondary.timelock),
            false,
            Some(index),
            editable,
            has_delete,
        ));
    }

    list.into()
}
