use crate::{
    backend::Backend,
    state::{message::Msg, State},
    views::format_last_edit_info,
};
use iced::{
    widget::{
        button::{Status, Style},
        Space,
    },
    Alignment, Background, Border, Length,
};
use liana_connect::models::{UserRole, WalletStatus};
use liana_ui::{color, component::text, icon, theme::Theme, widget::*};
use std::collections::BTreeMap;

/// Custom button style for path cards: dark grey border when not hovered, green when hovered
fn path_card_button(_theme: &Theme, status: Status) -> Style {
    bordered_button_style(status, 25.0)
}

/// Custom button style for delete button: dark grey border when not hovered, green when hovered
fn delete_button_style(_theme: &Theme, status: Status) -> Style {
    bordered_button_style(status, 50.0) // Fully round
}

/// Shared bordered button style with configurable border radius
fn bordered_button_style(status: Status, radius: f32) -> Style {
    let grey_border = color::GREY_7;
    let green_border = color::GREEN;

    match status {
        Status::Active => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREY_2,
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: grey_border,
            },
            ..Default::default()
        },
        Status::Hovered => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREEN,
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: green_border,
            },
            ..Default::default()
        },
        Status::Pressed => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREEN,
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: green_border,
            },
            ..Default::default()
        },
        Status::Disabled => Style {
            background: Some(Background::Color(color::TRANSPARENT)),
            text_color: color::GREY_2,
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: grey_border,
            },
            ..Default::default()
        },
    }
}

/// Container style for read-only path cards (matches button border style)
fn path_card_container_style(_theme: &Theme) -> iced::widget::container::Style {
    use iced::widget::container;
    container::Style {
        text_color: Some(color::GREY_2),
        background: Some(Background::Color(color::TRANSPARENT)),
        border: Border {
            radius: 25.0.into(),
            width: 1.0,
            color: color::GREY_7,
        },
        ..Default::default()
    }
}

// Colors for paths
const PRIMARY_COLOR: &str = "#32cd32"; // Green

// Card width constants
const PATH_CARD_WIDTH: f32 = 600.0;
// r_shape icon width that precedes path cards (adds visual offset for "Add recovery" button alignment)
const R_SHAPE_WIDTH: f32 = 60.0;

// Timelock conversion constants (1 block ≈ 10 minutes)
const BLOCKS_PER_HOUR: u64 = 6;
const BLOCKS_PER_DAY: u64 = 144;
const BLOCKS_PER_MONTH: u64 = 4380;

// Generate color from green to blue gradient based on index and total count
// Recovery path 1 should be between green and blue (not the same as primary green)
fn get_secondary_color(index: usize, total_count: usize) -> String {
    if total_count == 0 {
        return "#32cd32".to_string(); // Default to green
    }

    // Calculate interpolation factor (0.0 = mid-green-blue, 1.0 = blue)
    // First recovery path (index 0) should be between green and blue
    // Last recovery path should be blue
    let factor = if total_count == 1 {
        0.5 // Single recovery path: midpoint between green and blue
    } else {
        // Distribute from 0.0 (first) to 1.0 (last)
        // This ensures first path is between green and blue, not pure green
        index as f32 / (total_count - 1) as f32
    };

    // Start color (mid-green-blue): RGB(25, 102, 152) - halfway between green and blue
    // End color (blue): RGB(0, 0, 255) = #0000ff
    // Primary green: RGB(50, 205, 50) = #32cd32
    // Midpoint: RGB(25, 102, 152) = #196698

    let start_r = 25.0;
    let start_g = 102.0;
    let start_b = 152.0;

    let end_r = 0.0;
    let end_g = 0.0;
    let end_b = 255.0;

    // Interpolate from mid-green-blue to blue
    let r = (start_r + (end_r - start_r) * factor) as u8;
    let g = (start_g + (end_g - start_g) * factor) as u8;
    let b = (start_b + (end_b - start_b) * factor) as u8;

    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

/// Generate a single "r" shape SVG as an iced Element.
/// index=0 is primary (green), index>=1 are secondary paths (gradient).
/// count is the total number of paths (including primary).
pub fn r_shape(index: usize, count: usize) -> Element<'static, Msg> {
    let color = if index == 0 {
        PRIMARY_COLOR.to_string()
    } else {
        // Secondary paths: index-1 because get_secondary_color expects 0-based for secondaries
        let secondary_count = count.saturating_sub(1);
        get_secondary_color(index - 1, secondary_count)
    };

    // SVG dimensions - centered "r" shape
    let width = 60.0;
    let height = 60.0;
    let center_x = 30.0;
    let center_y = 30.0;

    // Create the "r" shape SVG
    let thickness = 10.0;
    let stem_top = center_y - 20.0;
    let stem_bottom = center_y + 25.0;
    let radius = 25.0;
    let arc_start_y = center_y + 25.0;
    let arc_end_x = center_x + 25.0;
    let arc_end_y = center_y;

    let svg_content = format!(
        r#"<svg width="{}" height="{}" viewBox="0 0 {} {}" xmlns="http://www.w3.org/2000/svg">
            <line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-width="{}" stroke-linecap="round" />
            <path d="M {} {} A {} {} 0 0 1 {} {}" stroke="{}" stroke-width="{}" fill="none" stroke-linecap="round" />
        </svg>"#,
        width,
        height,
        width,
        height,
        center_x,
        stem_top,
        center_x,
        stem_bottom,
        color,
        thickness,
        center_x,
        arc_start_y,
        radius,
        radius,
        arc_end_x,
        arc_end_y,
        color,
        thickness
    );

    let svg_handle = iced::widget::svg::Handle::from_memory(svg_content.as_bytes().to_vec());
    let svg_widget = liana_ui::widget::Svg::new(svg_handle)
        .width(Length::Fixed(R_SHAPE_WIDTH))
        .height(Length::Fixed(R_SHAPE_WIDTH))
        .content_fit(iced::ContentFit::Contain);

    Container::new(svg_widget)
        .width(Length::Fixed(R_SHAPE_WIDTH))
        .height(Length::Fixed(R_SHAPE_WIDTH))
        .into()
}

/// Convert a timelock (in blocks) to a human-readable format.
/// Shows "After x hours/days/months" (whichever is most appropriate).
fn format_timelock_human(timelock: &liana_connect::Timelock) -> String {
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
            format!("After {} months", months)
        }
    } else if blocks >= BLOCKS_PER_DAY {
        let days = blocks / BLOCKS_PER_DAY;
        if days == 1 {
            "After 1 day".to_string()
        } else {
            format!("After {} days", days)
        }
    } else {
        let hours = blocks / BLOCKS_PER_HOUR;
        if hours <= 1 {
            "After 1 hour".to_string()
        } else {
            format!("After {} hours", hours)
        }
    }
}

/// Create a path card displaying key names and timelock information.
/// is_primary: true for primary path, false for secondary paths
/// path_index: None for primary, Some(index) for secondary paths
/// timelock is None for primary path ("Spendable anytime"), Some for secondary paths.
/// is_editable: if true, card is clickable; if false, card is read-only
fn path_card(
    path: &liana_connect::SpendingPath,
    keys: &BTreeMap<u8, liana_connect::Key>,
    timelock: Option<&liana_connect::Timelock>,
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
            format!("All of {}", names)
        } else {
            format!("{} of {}", threshold, names)
        }
    };

    // Determine timelock text
    let timelock_text = match timelock {
        None => "Spendable anytime".to_string(),
        Some(tl) => format_timelock_human(tl),
    };

    let mut content = Column::new()
        .spacing(5)
        .push(text::p1_regular(keys_text))
        .push(text::p2_regular(timelock_text));

    if let Some(info) = last_edit_info {
        content = content.push(text::caption(info).style(liana_ui::theme::text::secondary));
    }

    // Wrap card content - use Fill width so Button controls the final width
    let card_content = Container::new(content).padding(15).width(Length::Fill);

    // Make clickable only if editable
    if is_editable {
        Button::new(card_content)
            .width(Length::Fixed(PATH_CARD_WIDTH))
            .on_press(Msg::TemplateEditPath(is_primary, path_index))
            .style(path_card_button)
            .into()
    } else {
        // Read-only: display with border styling (no click handler)
        card_content
            .width(Length::Fixed(PATH_CARD_WIDTH))
            .style(path_card_container_style)
            .into()
    }
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
        .map(|w| w.status.clone());

    // Determine if user can edit: WSManager only, and only when status is Draft
    let is_draft = matches!(
        wallet_status,
        Some(WalletStatus::Created) | Some(WalletStatus::Drafted)
    );
    let is_editable = matches!(state.app.current_user_role, Some(UserRole::WSManager)) && is_draft;

    // Total count includes primary + all secondary paths
    let total_count = 1 + secondary_paths.len();

    let mut column = Column::new()
        .spacing(10)
        .padding(20.0)
        .push(Space::with_height(50));

    // Primary path row: [r_shape] [path_card]
    let primary_last_edit = format_last_edit_info(
        primary_path.last_edited,
        primary_path.last_editor,
        state,
        &current_user_email_lower,
    );
    let primary_row = Row::new()
        .spacing(15)
        .align_y(Alignment::Center)
        .push(r_shape(0, total_count))
        .push(path_card(
            primary_path,
            keys,
            None,
            true,
            None,
            is_editable,
            primary_last_edit,
        ));

    column = column.push(primary_row);

    // Secondary path rows (with delete button if editable)
    for (index, (path, timelock)) in secondary_paths.iter().enumerate() {
        let secondary_last_edit = format_last_edit_info(
            path.last_edited,
            path.last_editor,
            state,
            &current_user_email_lower,
        );
        let mut secondary_row = Row::new()
            .spacing(15)
            .align_y(Alignment::Center)
            .push(r_shape(index + 1, total_count))
            .push(path_card(
                path,
                keys,
                Some(timelock),
                false,
                Some(index),
                is_editable,
                secondary_last_edit,
            ));

        // Only show delete button if editable (WSManager)
        if is_editable {
            secondary_row = secondary_row.push(
                Button::new(
                    Container::new(icon::trash_icon())
                        .width(Length::Fixed(20.0))
                        .height(Length::Fixed(20.0))
                        .center_x(Length::Fixed(20.0))
                        .center_y(Length::Fixed(20.0)),
                )
                .padding(10)
                .on_press(Msg::TemplateDeleteSecondaryPath(index))
                .style(delete_button_style),
            );
        }

        column = column.push(secondary_row);
    }

    // "Add a recovery path" card - only show if editable (WSManager)
    if is_editable {
        let add_path_content = Container::new(
            text::p1_regular("+ Add a recovery path").style(liana_ui::theme::text::secondary),
        )
        .padding(15)
        .width(Length::Fill);

        let add_path_card = Button::new(add_path_content)
            .width(Length::Fixed(PATH_CARD_WIDTH))
            .on_press(Msg::TemplateNewPathModal)
            .style(path_card_button);

        // Spacer aligns with r_shape icons above (R_SHAPE_WIDTH) + row spacing (15)
        let add_path_row = Row::new()
            .spacing(15)
            .align_y(Alignment::Center)
            .push(Space::with_width(Length::Fixed(R_SHAPE_WIDTH)))
            .push(add_path_card);

        column = column.push(add_path_row);
    }

    Container::new(column)
        .center_x(Length::Shrink)
        .center_y(Length::Shrink)
        .into()
}
