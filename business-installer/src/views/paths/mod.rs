pub mod modal;

use crate::state::{Msg, State};
use iced::{widget::Space, Alignment, Length};
use liana_ui::{
    component::{button, card, text},
    icon,
    widget::*,
};

pub fn paths_view(state: &State) -> Element<'static, Msg> {
    let mut column = Column::new()
        .spacing(20)
        .padding(20.0)
        .width(Length::Fill)
        .align_x(Alignment::Start);

    // Header
    column = column.push(
        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(button::transparent(Some(icon::arrow_back()), "").on_press(Msg::NavigateToHome))
            .push(text::h2("Spending Paths")),
    );

    // Primary spending path
    let primary_card = primary_path_card(state);
    column = column.push(primary_card);

    // Secondary paths
    for (index, (path, timelock)) in state.app.secondary_paths.iter().enumerate() {
        let path = path.clone();
        let timelock = timelock.clone();
        let secondary_card = secondary_path_card(state, index, path, timelock);
        column = column.push(secondary_card);
    }

    // Add secondary path button
    column = column.push(
        button::secondary(Some(icon::plus_icon()), "Add Secondary Path")
            .on_press(Msg::TemplateAddSecondaryPath)
            .width(Length::Fixed(250.0)),
    );

    Container::new(column.width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn primary_path_card(state: &State) -> Element<'static, Msg> {
    let mut content = Column::new().spacing(10);

    // Single row with all path information
    let mut path_row = Row::new().spacing(15).align_y(Alignment::Center);

    // Icon and title
    path_row = path_row.push(icon::key_icon());
    path_row = path_row.push(text::h5_medium("Primary spending option"));

    // Threshold information
    let threshold_n = state.app.primary_path.threshold_n;
    let threshold_m = state.app.primary_path.threshold_m();
    let threshold_text = if threshold_m == 0 {
        "No keys".to_string()
    } else {
        format!("{} of {} keys", threshold_n, threshold_m)
    };
    path_row = path_row.push(text::p2_regular(threshold_text));

    // Keys in path (show aliases or count)
    let primary_path_key_ids = state.app.primary_path.key_ids.clone();
    if !primary_path_key_ids.is_empty() {
        let key_aliases: Vec<String> = primary_path_key_ids
            .iter()
            .filter_map(|id| state.app.keys.get(id).map(|k| k.alias.clone()))
            .collect();
        if !key_aliases.is_empty() {
            path_row = path_row.push(text::p2_regular(format!(
                "Keys: {}",
                key_aliases.join(", ")
            )));
        }
    } else {
        path_row = path_row.push(text::p2_regular("No keys added"));
    }

    // Push buttons to the right side
    path_row = path_row.push(Space::with_width(Length::Fill));
    path_row = path_row.push(
        button::transparent(Some(icon::pencil_icon()), "").on_press(Msg::TemplateEditPath(true, None)),
    );

    content = content.push(path_row);

    // Available keys to add
    let available_keys: Vec<(u8, liana_connect::Key)> = state
        .app
        .keys
        .iter()
        .filter(|(id, _)| !state.app.primary_path.contains_key(**id))
        .map(|(id, key)| (*id, key.clone()))
        .collect();

    if !available_keys.is_empty() {
        let mut add_keys_row = Row::new().spacing(10).align_y(Alignment::Center);
        add_keys_row = add_keys_row.push(text::p2_regular("Add: "));
        for (key_id, key) in available_keys {
            add_keys_row = add_keys_row.push(text::p2_regular(format!("{} ", key.alias)));
            add_keys_row = add_keys_row.push(
                button::transparent(Some(icon::plus_icon()), "")
                    .on_press(Msg::TemplateAddKeyToPrimary(key_id)),
            );
        }
        content = content.push(add_keys_row);
    }

    card::simple(content).into()
}

fn secondary_path_card(
    state: &State,
    path_index: usize,
    path: liana_connect::SpendingPath,
    timelock: liana_connect::Timelock,
) -> Element<'static, Msg> {
    let mut content = Column::new().spacing(10);

    // Single row with all path information
    let mut path_row = Row::new().spacing(15).align_y(Alignment::Center);

    // Icon and title
    path_row = path_row.push(icon::recovery_icon());
    let title = if path_index == 0 {
        "Recovery option #1".to_string()
    } else {
        format!("Recovery option #{}", path_index + 1)
    };
    path_row = path_row.push(text::h5_medium(&title));

    // Timelock
    let timelock_str = if timelock.is_zero() {
        "No timelock".to_string()
    } else {
        format!("Timelock: {}", timelock)
    };
    path_row = path_row.push(text::p2_regular(timelock_str));

    // Threshold information
    let threshold_n = path.threshold_n;
    let threshold_m = path.threshold_m();
    let threshold_text = if threshold_m == 0 {
        "No keys".to_string()
    } else {
        format!("{} of {} keys", threshold_n, threshold_m)
    };
    path_row = path_row.push(text::p2_regular(threshold_text));

    // Keys in path (show aliases or count)
    let path_key_ids = path.key_ids.clone();
    if !path_key_ids.is_empty() {
        let key_aliases: Vec<String> = path_key_ids
            .iter()
            .filter_map(|id| state.app.keys.get(id).map(|k| k.alias.clone()))
            .collect();
        if !key_aliases.is_empty() {
            path_row = path_row.push(text::p2_regular(format!(
                "Keys: {}",
                key_aliases.join(", ")
            )));
        }
    } else {
        path_row = path_row.push(text::p2_regular("No keys added"));
    }

    // Push buttons to the right side
    path_row = path_row.push(Space::with_width(Length::Fill));
    path_row = path_row.push(
        button::transparent(Some(icon::pencil_icon()), "")
            .on_press(Msg::TemplateEditPath(false, Some(path_index))),
    );
    path_row = path_row.push(
        button::transparent(Some(icon::trash_icon()), "")
            .on_press(Msg::TemplateDeleteSecondaryPath(path_index)),
    );

    content = content.push(path_row);

    // Available keys to add
    let available_keys: Vec<(u8, liana_connect::Key)> = state
        .app
        .keys
        .iter()
        .filter(|(id, _)| !path.contains_key(**id))
        .map(|(id, key)| (*id, key.clone()))
        .collect();

    if !available_keys.is_empty() {
        let mut add_keys_row = Row::new().spacing(10).align_y(Alignment::Center);
        add_keys_row = add_keys_row.push(text::p2_regular("Add: "));
        for (key_id, key) in available_keys {
            add_keys_row = add_keys_row.push(text::p2_regular(format!("{} ", key.alias)));
            add_keys_row = add_keys_row.push(
                button::transparent(Some(icon::plus_icon()), "")
                    .on_press(Msg::TemplateAddKeyToSecondary(path_index, key_id)),
            );
        }
        content = content.push(add_keys_row);
    }

    card::simple(content).into()
}
