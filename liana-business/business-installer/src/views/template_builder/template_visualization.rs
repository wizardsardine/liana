use crate::{
    backend::Backend,
    state::{message::Msg, State},
    views::{card_entry, format_last_edit_info},
};
use iced::{
    widget::{
        button::{Status, Style},
        Space,
    },
    Alignment, Background, Border, Length,
};
use liana_connect::ws_business::{
    self, UserRole, WalletStatus, BLOCKS_PER_DAY, BLOCKS_PER_HOUR, BLOCKS_PER_MONTH,
};
use liana_ui::{color, component::text, icon, theme, theme::Theme, widget::*};
use std::collections::BTreeMap;

/// Custom button style for delete button: circular with grey background and shadow
fn delete_button_style(_theme: &Theme, status: Status) -> Style {
    use iced::{Color, Shadow, Vector};

    let background = color::LIGHT_BG_SECONDARY;
    let border_active = color::BUSINESS_BLUE_DARK;
    let shadow = Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.15),
        offset: Vector::new(0.0, 2.0),
        blur_radius: 8.0,
    };

    match status {
        Status::Active => Style {
            background: Some(Background::Color(background)),
            text_color: color::DARK_TEXT_SECONDARY,
            border: Border {
                radius: 50.0.into(),
                width: 1.0,
                color: color::TRANSPARENT,
            },
            shadow,
        },
        Status::Hovered | Status::Pressed => Style {
            background: Some(Background::Color(background)),
            text_color: color::BUSINESS_BLUE_DARK,
            border: Border {
                radius: 50.0.into(),
                width: 1.0,
                color: border_active,
            },
            shadow,
        },
        Status::Disabled => Style {
            background: Some(Background::Color(background)),
            text_color: color::DARK_TEXT_TERTIARY,
            border: Border {
                radius: 50.0.into(),
                width: 1.0,
                color: color::TRANSPARENT,
            },
            shadow,
        },
    }
}

// Colors for paths
const PRIMARY_COLOR: &str = "#00BFFF"; // Business Blue

// Card width constants
const PATH_CARD_WIDTH: f32 = 600.0;
// r_shape icon width that precedes path cards (adds visual offset for "Add recovery" button alignment)
const R_SHAPE_WIDTH: f32 = 60.0;

// Generate color from blue to purple gradient based on index and total count
// Recovery path 1 should be between blue and purple (not the same as primary blue)
fn get_secondary_color(index: usize, total_count: usize) -> String {
    if total_count == 0 {
        return "#00BFFF".to_string(); // Default to business blue
    }

    // Calculate interpolation factor (0.0 = mid-blue-purple, 1.0 = purple)
    // First recovery path (index 0) should be between blue and purple
    // Last recovery path should be purple
    let factor = if total_count == 1 {
        0.5 // Single recovery path: midpoint between blue and purple
    } else {
        // Distribute from 0.0 (first) to 1.0 (last)
        // This ensures first path is between blue and purple, not pure blue
        index as f32 / (total_count - 1) as f32
    };

    // Start color (mid-blue-purple): RGB(64, 96, 255) - halfway between blue and purple
    // End color (purple): RGB(128, 0, 255) = #8000FF
    // Primary blue: RGB(0, 191, 255) = #00BFFF
    // Midpoint: RGB(64, 96, 255) = #4060FF

    let start_r = 64.0;
    let start_g = 96.0;
    let start_b = 255.0;

    let end_r = 128.0;
    let end_g = 0.0;
    let end_b = 255.0;

    // Interpolate from mid-blue-purple to purple
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

    let last_edit_info =
        last_edit_info.map(|info| text::caption(info).style(liana_ui::theme::text::secondary));

    let content = Column::new()
        .push(text::h3(keys_text).style(theme::text::primary))
        .push(text::p2_medium(timelock_text).style(theme::text::primary))
        .push_maybe(last_edit_info)
        .spacing(5);

    // Use card_entry with optional message for editable vs read-only
    let message = if is_editable {
        Some(Msg::TemplateEditPath(is_primary, path_index))
    } else {
        None
    };

    card_entry(content.into(), message, PATH_CARD_WIDTH)
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
    for (index, secondary) in secondary_paths.iter().enumerate() {
        let secondary_last_edit = format_last_edit_info(
            secondary.path.last_edited,
            secondary.path.last_editor,
            state,
            &current_user_email_lower,
        );
        let mut secondary_row = Row::new()
            .spacing(15)
            .align_y(Alignment::Center)
            .push(r_shape(index + 1, total_count))
            .push(path_card(
                &secondary.path,
                keys,
                Some(&secondary.timelock),
                false,
                Some(index),
                is_editable,
                secondary_last_edit,
            ));

        // Only show delete button if editable (WS Admin)
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

    // "Add a recovery path" card - only show if editable (WS Admin)
    if is_editable {
        let add_path_content =
            text::p1_medium("+ Add a recovery path").style(liana_ui::theme::text::secondary);

        let add_path_card = card_entry(
            add_path_content.into(),
            Some(Msg::TemplateNewPathModal),
            PATH_CARD_WIDTH,
        );

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
