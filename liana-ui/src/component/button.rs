use std::fmt::Display;

use super::{
    modal::BTN_W,
    text::{
        new::{button_text, button_text_compact, caption, BUTTON_TEXT_COMPACT_SPEC},
        panel_title, text,
    },
    tooltip,
};
use crate::{
    font::{BOLD, MANROPE_SEMIBOLD, MEDIUM},
    icon::{self, ICON_SIZE_L},
    theme::{self, button::round_icon_btn, Theme},
    widget::*,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button::{Status, Style},
        container, row, Space,
    },
    Background, Border, Color, Length, Padding,
};

const MENU_BTN_PADDING: [u16; 2] = [9 /* Top/Bottom */, 22 /* Left/Right */];
const MENU_TEXT_SIZE: u32 = 22;
const MENU_TEXT_COMPACT_SIZE: u32 = 18;
const MENU_ICON_SIZE: u32 = ICON_SIZE_L as u32;
const AUXILIARY_PADDING: [u16; 2] = [14 /* Top/Bottom */, 20 /* Left/Right */];
const LIST_ENTRY_ACCENT_WIDTH: f32 = 4.0;
const LIST_ENTRY_PADDING: [u16; 2] = [14 /* Top/Bottom */, 20 /* Left/Right */];

const ICON_BTN_SIZE: f32 = 40.0;
const ICON_BTN_PADDING: f32 = 10.0;
pub const DEVICE_BTN_H: u32 = 40;

const BTN_PADDING: [u16; 2] = [9 /* Top/Bottom */, 14 /* Left/Right */];
const BTN_PADDING_COMPACT: [u16; 2] = [7 /* Top/Bottom */, 12 /* Left/Right */];

pub type ListEntryAccent = fn(&Theme) -> Color;
pub type ButtonStyle = fn(&theme::Theme, Status) -> Style;

pub fn menu<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str, compact: bool) -> Button<'a, T> {
    Button::new(
        content_menu(
            icon.map(|i| i.style(theme::text::secondary)),
            t,
            false,
            compact,
        )
        .padding(MENU_BTN_PADDING),
    )
    .style(theme::button::menu)
    .padding(0)
}

pub fn menu_active<'a, T: 'a>(
    icon: Option<Text<'a>>,
    t: &'static str,
    compact: bool,
) -> Button<'a, T> {
    Button::new(content_menu(icon, t, true, compact).padding(MENU_BTN_PADDING))
        .style(theme::button::menu_pressed)
        .padding(0)
}

pub fn menu_small<'a, T: 'a>(icon: Text<'a>) -> Button<'a, T> {
    Button::new(
        container(icon.size(MENU_ICON_SIZE).style(theme::text::secondary))
            .padding(MENU_BTN_PADDING)
            .align_x(Horizontal::Center),
    )
    .style(theme::button::menu)
    .padding(0)
}

pub fn menu_active_small<'a, T: 'a>(icon: Text<'a>) -> Button<'a, T> {
    Button::new(
        container(icon.size(MENU_ICON_SIZE))
            .padding(MENU_BTN_PADDING)
            .align_x(Horizontal::Center),
    )
    .style(theme::button::menu_pressed)
    .padding(0)
}

fn content_menu<'a, T: 'a>(
    icon: Option<Text<'a>>,
    t: &'static str,
    active: bool,
    compact: bool,
) -> Container<'a, T> {
    let t = match (active, compact) {
        (true, false) => text(t).size(MENU_TEXT_SIZE).font(BOLD),
        (false, false) => text(t).size(MENU_TEXT_SIZE).font(MEDIUM),
        (true, true) => text(t).size(MENU_TEXT_COMPACT_SIZE).font(BOLD),
        (false, true) => text(t).size(MENU_TEXT_COMPACT_SIZE).font(MEDIUM),
    };

    match icon {
        None => container(t),
        Some(i) => container(
            row![i.size(MENU_ICON_SIZE), t]
                .spacing(10)
                .align_y(Vertical::Center),
        ),
    }
}

pub fn button_with_theme<'a, T: 'a>(
    icon: Option<Text<'a>>,
    text: impl Display,
    style: impl Fn(&Theme, Status) -> Style + 'a,
    compact: bool,
) -> Button<'a, T> {
    let (text, icon) = if compact {
        (
            button_text_compact(text),
            icon.map(|i| i.size(BUTTON_TEXT_COMPACT_SPEC.size.expect("size"))),
        )
    } else {
        (
            button_text(text),
            icon.map(|i| i.size(BUTTON_TEXT_COMPACT_SPEC.size.expect("size"))),
        )
    };
    Button::new(content(icon, text, compact))
        .style(style)
        .padding(0)
}

pub fn button_compact<'a, T: 'a>(
    text: impl Display,
    style: impl Fn(&Theme, Status) -> Style + 'a,
    msg: Option<T>,
) -> Button<'a, T> {
    button_with_theme(None, text, style, true).on_press_maybe(msg)
}

macro_rules! button_helpers {
    ($($entry:tt),* $(,)?) => {
        $( button_helpers!(@one $entry); )*
    };
    (@one ($name:ident, $style:ident)) => {
        pub fn $name<'a, T: 'a>(icon: Option<Text<'a>>, t: impl Display) -> Button<'a, T> {
        button_with_theme(icon, t,theme::button::$style, false)
        }
    };
    (@one $name:ident) => {
        button_helpers!(@one ($name, $name));
    };
}

button_helpers!(
    (alert, destructive),
    destructive,
    primary,
    (transparent, container),
    transparent_primary_text,
    (flat, transparent),
    secondary,
    tertiary,
    (border, secondary),
    (transparent_border, container_border),
    link,
);

pub fn auxiliary<'a, T: 'a + Clone>(
    icon: Option<Text<'a>>,
    t: impl Display,
    msg: Option<T>,
) -> Button<'a, T> {
    Button::new(auxiliary_content(icon, t))
        .style(theme::button::auxiliary)
        .on_press_maybe(msg)
        .width(STANDARD_ENTRY_WIDTH)
        .padding(0)
}

pub fn breadcrumb<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, panel_title(t), false))
        .style(theme::button::breadcrumb)
        .padding(0)
}

pub fn list_entry<'a, M: 'a + Clone, T: Into<Element<'a, M>>>(
    content: T,
    accent: Option<ListEntryAccent>,
    width: EntryWidth,
    msg: Option<M>,
) -> Element<'a, M> {
    list_entry_with_state(content, accent, width, msg.is_some(), msg.is_some(), msg)
}

pub fn list_entry_with_enabled<'a, M: 'a + Clone, T: Into<Element<'a, M>>>(
    content: T,
    accent: Option<ListEntryAccent>,
    width: EntryWidth,
    enabled: bool,
    msg: Option<M>,
) -> Element<'a, M> {
    list_entry_with_state(content, accent, width, enabled, msg.is_some(), msg)
}

pub fn list_entry_with_state<'a, M: 'a + Clone, T: Into<Element<'a, M>>>(
    content: T,
    accent: Option<ListEntryAccent>,
    width: EntryWidth,
    enabled: bool,
    clickable: bool,
    msg: Option<M>,
) -> Element<'a, M> {
    let clickable = enabled && clickable;
    let msg = clickable.then_some(msg).flatten();
    let button = Button::new(
        container(content.into())
            .padding(LIST_ENTRY_PADDING)
            .width(Length::Fill),
    )
    .style(move |theme, status| {
        let status = if !clickable && status == Status::Disabled {
            Status::Active
        } else {
            status
        };
        let mut style = theme::button::list_entry(theme, status);
        if let Some(color) = accent {
            // The accent card behind carries the shadow; keep the inner card flat.
            style.shadow = Default::default();
            if status == Status::Hovered {
                // Hover border matches the entry's accent stripe.
                style.border.color = color(theme);
            }
        }
        style
    })
    .on_press_maybe(msg)
    .padding(0)
    .width(Length::Fill);

    let entry: Element<'a, M> = if let Some(color) = accent {
        let accent_card = Container::new(Space::with_height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme| container::Style {
                background: Some(Background::Color(color(theme))),
                border: Border {
                    radius: theme
                        .colors
                        .buttons
                        .list_entry_radius
                        .unwrap_or(theme::button::BUTTON_RADIUS)
                        .into(),
                    ..Default::default()
                },
                shadow: theme.colors.buttons.list_entry.active.shadow,
                ..Default::default()
            });
        let accent_card = Container::new(accent_card)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding {
                top: 1.0,
                right: 1.0,
                bottom: 1.0,
                left: 0.0,
            });
        // White card inset on the left by the accent width so the accent card behind shows as a
        // stripe that wraps the left rounded corners.
        Stack::new()
            .width(Length::Fill)
            .push(Container::new(button).padding(Padding {
                left: LIST_ENTRY_ACCENT_WIDTH,
                ..Padding::ZERO
            }))
            .push_under(accent_card)
            .into()
    } else {
        button.into()
    };

    container(entry).width(width).into()
}

pub fn clickable_section<'a, M: 'a + Clone, T: Into<Element<'a, M>>>(
    content: T,
    msg: Option<M>,
) -> Button<'a, M> {
    Button::new(content.into())
        .style(theme::button::clickable_section)
        .on_press_maybe(msg)
        .width(Length::Fill)
}

fn content<'a, T: 'a>(icon: Option<Text<'a>>, text: Text<'a>, compact: bool) -> Container<'a, T> {
    content_with_tooltip(icon, text, None, compact)
}

fn auxiliary_content<'a, T: 'a>(icon: Option<Text<'a>>, t: impl Display) -> Container<'a, T> {
    let text = Text::new(t.to_string()).size(16).font(MANROPE_SEMIBOLD);
    container(
        row![icon.map(|icon| icon.size(16)), text]
            .spacing(10)
            .align_y(Vertical::Center),
    )
    .align_x(Horizontal::Center)
    .padding(AUXILIARY_PADDING)
    .width(Length::Fill)
}

fn content_with_tooltip<'a, T: 'a>(
    icon: Option<Text<'a>>,
    text: Text<'a>,
    tooltip: Option<&'a str>,
    compact: bool,
) -> Container<'a, T> {
    let content = row![icon, text, tooltip.map(tooltip::tooltip)]
        .spacing(10)
        .align_y(Vertical::Center);
    let padding = if compact {
        BTN_PADDING_COMPACT
    } else {
        BTN_PADDING
    };
    container(content)
        .align_x(Horizontal::Center)
        .padding(padding)
}

pub fn device<'a, T: 'a + std::clone::Clone, C: Into<Element<'a, T>>>(
    content: C,
    msg: Option<T>,
) -> Element<'a, T> {
    device_with_height(content, msg, DEVICE_BTN_H)
}

pub fn device_with_height<'a, T: 'a + std::clone::Clone, C: Into<Element<'a, T>>>(
    content: C,
    msg: Option<T>,
    height: u32,
) -> Element<'a, T> {
    device_with_height_clickable(content, msg, Some(height), true)
}

pub fn device_with_height_clickable<'a, T: 'a + std::clone::Clone, C: Into<Element<'a, T>>>(
    content: C,
    msg: Option<T>,
    height: Option<u32>,
    clickable: bool,
) -> Element<'a, T> {
    let mut content = Container::new(content).width(BTN_W);
    if let Some(h) = height {
        content = content.center_y(h);
    }
    let style = if clickable {
        theme::button::signing_devices
    } else {
        theme::button::signing_devices_non_clickable
    };
    Button::new(content)
        .style(style)
        .on_press_maybe(msg)
        .padding(10)
        .width(Length::Shrink)
        .height(Length::Shrink)
        .into()
}

/// Button width presets.
#[derive(Debug, Clone, Copy)]
pub enum BtnWidth {
    S = 100,
    M = 140,
    L = 180,
    XL = 230,
    XXL = 330,
    /// Default to Length::Shrink
    Auto,
    Fill,
}

impl From<BtnWidth> for Length {
    fn from(value: BtnWidth) -> Self {
        match value {
            BtnWidth::Auto => Length::Shrink,
            BtnWidth::Fill => Length::Fill,
            v => (v as u16 as u32).into(),
        }
    }
}

pub const STANDARD_ENTRY_WIDTH: f32 = 600.0;
pub const ENTRY_DELETE_SLOT: f32 = 40.0;
pub const ENTRY_DELETE_GAP: f32 = 10.0;

pub enum EntryWidth {
    Standard,
    Deletable,
    Fill,
    Shrink,
    Custom(f32),
}

impl From<EntryWidth> for Length {
    fn from(value: EntryWidth) -> Self {
        match value {
            EntryWidth::Standard => Length::Fixed(STANDARD_ENTRY_WIDTH),
            // A deletable row matches the standard width, reserving room for its delete button.
            EntryWidth::Deletable => {
                Length::Fixed(STANDARD_ENTRY_WIDTH - ENTRY_DELETE_SLOT - ENTRY_DELETE_GAP)
            }
            EntryWidth::Fill => Length::Fill,
            EntryWidth::Shrink => Length::Shrink,
            EntryWidth::Custom(v) => Length::Fixed(v),
        }
    }
}

pub fn icon_btn<'a, T: 'a + Clone>(icon: Text<'a>, message: Option<T>) -> Button<'a, T> {
    let inner = ICON_BTN_SIZE - 2.0 * ICON_BTN_PADDING;
    Button::new(
        Container::new(icon)
            .center_x(Length::Fixed(inner))
            .center_y(Length::Fixed(inner)),
    )
    .padding(ICON_BTN_PADDING)
    .on_press_maybe(message)
    .style(|t, s| round_icon_btn(t, s, ICON_BTN_SIZE / 2.0))
}

pub fn clickable_icon_with_size<'a, T: 'a + Clone>(
    icon: Text<'a>,
    message: Option<T>,
    size: u32,
) -> Button<'a, T> {
    Button::new(icon.size(size))
        .on_press_maybe(message)
        .style(theme::button::transparent)
}

/// Primary button with preset width.
pub fn btn_primary<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = primary(icon, label).width(width);
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Secondary button with preset width.
pub fn btn_secondary<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'a str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = secondary(icon, label).width(width);
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

fn btn_feerate<'a, T: Clone + 'a>(
    label: impl Display,
    selected: bool,
    msg: Option<T>,
) -> Button<'a, T> {
    let btn = if selected {
        button_compact(label, theme::button::feerate, msg)
    } else {
        button_compact(label, theme::button::feerate_unselected, msg)
    };
    btn.width(BtnWidth::S)
}

pub fn btn_low<'a, T: Clone + 'a>(selected: bool, msg: Option<T>) -> Button<'a, T> {
    btn_feerate("Low", selected, msg)
}

pub fn btn_medium<'a, T: Clone + 'a>(selected: bool, msg: Option<T>) -> Button<'a, T> {
    btn_feerate("Medium", selected, msg)
}

pub fn btn_high<'a, T: Clone + 'a>(selected: bool, msg: Option<T>) -> Button<'a, T> {
    btn_feerate("High", selected, msg)
}

fn btn_with_tooltip<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'a str,
    tooltip: Option<&'a str>,
    width: BtnWidth,
    msg: Option<T>,
    style: ButtonStyle,
) -> Button<'a, T> {
    Button::new(content_with_tooltip(
        icon,
        button_text(label),
        tooltip,
        false,
    ))
    .width(width)
    .style(style)
    .on_press_maybe(msg)
    .padding(0)
}

/// Tertiary button with preset width.
pub fn btn_tertiary<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = tertiary(icon, label).width(width);
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Destructive button with preset width.
pub fn btn_destructive<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = destructive(icon, label).width(width);
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Flat button with preset width.
pub fn btn_flat<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = flat(icon, label).width(width);
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Save button: primary or secondary. Width M.
pub fn btn_save<'a, T: Clone + 'a>(msg: Option<T>, primary: bool) -> Button<'a, T> {
    if primary {
        btn_primary(None, "Save", BtnWidth::M, msg)
    } else {
        btn_secondary(None, "Save", BtnWidth::M, msg)
    }
}

/// Cancel button: destructive. Width M.
pub fn btn_cancel<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_destructive(None, "Cancel", BtnWidth::M, msg)
}

/// OK button: primary. Width M.
pub fn btn_ok<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "OK", BtnWidth::M, msg)
}

pub fn btn_email_wizardsardine<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Email WS", BtnWidth::Auto, msg)
}

pub fn btn_modal_close<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    Button::new(icon::cross_icon().size(40))
        .padding(0)
        .style(theme::button::transparent)
        .on_press_maybe(msg)
}

pub fn btn_generate<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Generate", BtnWidth::M, msg)
}

/// Clear button: destructive. Width M.
pub fn btn_clear<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_destructive(None, "Clear", BtnWidth::M, msg)
}

/// Retry button: secondary. Width M.
pub fn btn_retry<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "Retry", BtnWidth::M, msg)
}

/// Yes button: primary. Width S.
pub fn btn_yes<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Yes", BtnWidth::S, msg)
}

/// No button: secondary. Width S.
pub fn btn_no<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "No", BtnWidth::S, msg)
}

pub fn btn_reset_timelock<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(
        Some(icon::reload_icon()),
        "Reset timelock",
        BtnWidth::Auto,
        msg,
    )
}

pub fn btn_go_to_rescan<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "Go to rescan", BtnWidth::XL, msg)
}

pub fn btn_dismiss<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_destructive(Some(icon::big_cross_icon()), "Dismiss", BtnWidth::L, msg)
}

pub fn btn_customize<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(None, "Customize", BtnWidth::M, msg)
}

pub fn btn_clear_all<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(None, "Clear all", BtnWidth::M, msg)
}

pub fn btn_unlock<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "Unlock all", BtnWidth::M, msg)
}

pub fn btn_reload<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Reload", BtnWidth::M, msg)
}

pub fn btn_approve<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Approve", BtnWidth::XL, msg)
}

pub fn btn_send_for_approval<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Send for approval", BtnWidth::XL, msg)
}

pub fn btn_keep_changes<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "Keep my changes", BtnWidth::XL, msg)
}

pub fn btn_send_token<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Send token", BtnWidth::L, msg)
}

pub fn btn_paste_icon<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    clickable_icon_with_size(icon::paste_icon(), msg, 20)
}

pub fn subtle_link<'a, T: Clone + 'a>(label: impl Display, msg: Option<T>) -> Element<'a, T> {
    let link = Button::new(caption(label).size(14))
        .padding(0)
        .style(theme::button::link_subtle)
        .on_press_maybe(msg);
    let underline =
        Container::new(iced::widget::rule::horizontal(1).style(theme::rule::link_underline))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Vertical::Bottom);
    Stack::new()
        .width(Length::Shrink)
        .push(link)
        .push(underline)
        .into()
}

pub fn btn_template_help<'a, T: Clone + 'a>(msg: Option<T>) -> Element<'a, T> {
    subtle_link("Something’s wrong with this template?", msg)
}

pub fn btn_breadcrumb_previous<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_flat(Some(icon::previous_icon()), "Previous", BtnWidth::L, msg)
}

pub fn btn_manage_keys<'a, T: Clone + 'a>(msg: Option<T>, primary: bool) -> Button<'a, T> {
    let width = BtnWidth::XL;
    let label = "Manage Keys";
    let icon = Some(icon::key_icon());
    if primary {
        btn_primary(icon, label, width, msg)
    } else {
        btn_secondary(icon, label, width, msg)
    }
}

pub fn btn_mark_keys_ready<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Mark keys as ready", BtnWidth::XL, msg)
}

pub fn btn_edit_keys<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(Some(icon::edit_icon()), "Edit keys", BtnWidth::L, msg)
}

/// Generate-address button: a plus icon and "Generate address" label.
pub fn btn_generate_address<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    let icon = Some(icon::plus_icon());
    let label = "Generate address";
    btn_primary(icon, label, BtnWidth::Auto, msg)
}

pub fn btn_add_key<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    auxiliary(Some(icon::plus_icon()), "Add a key", msg)
}

pub fn btn_add_recovery_path<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    auxiliary(Some(icon::plus_icon()), "Add a recovery path", msg)
}

pub fn btn_skip<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "Skip", BtnWidth::XL, msg)
}

pub fn btn_skip_registration<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "Skip registration", BtnWidth::XL, msg)
}

pub fn btn_resend_token<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(None, "Resend token", BtnWidth::XL, msg)
}

pub fn btn_change_email<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(
        Some(icon::previous_icon()),
        "Change email",
        BtnWidth::XL,
        msg,
    )
}

pub fn btn_connect_another_email<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    auxiliary(None, "Connect with another email", msg)
}

pub fn btn_verify_compact<'a, T: Clone + 'a>(msg: T) -> Button<'a, T> {
    button_compact(
        "Verify on hardware device",
        theme::button::tertiary,
        Some(msg),
    )
}

pub fn btn_show_qr_compact<'a, T: Clone + 'a>(msg: T) -> Button<'a, T> {
    button_compact("Show QR Code", theme::button::tertiary, Some(msg))
}

pub fn btn_show_qr<'a, T: Clone + 'a>(msg: T) -> Button<'a, T> {
    btn_tertiary(
        Some(icon::qr_icon()),
        "Show QR Code",
        BtnWidth::XL,
        Some(msg),
    )
}

pub fn btn_verify<'a, T: Clone + 'a>(msg: T) -> Button<'a, T> {
    btn_tertiary(
        Some(icon::usb_icon()),
        "Verify on hardware device",
        BtnWidth::XXL,
        Some(msg),
    )
}

pub fn btn_register_on_device<'a, T: Clone + 'a>(msg: T) -> Button<'a, T> {
    btn_tertiary(
        Some(icon::chip_icon()),
        "Register on device",
        BtnWidth::XL,
        Some(msg),
    )
}

pub fn btn_see_transaction_details<'a, T: Clone + 'a>(msg: T) -> Button<'a, T> {
    btn_tertiary(None, "See transaction details", BtnWidth::XL, Some(msg))
}

pub fn btn_export<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(Some(icon::backup_icon()), "Export", BtnWidth::M, msg)
}

pub fn btn_import<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(Some(icon::restore_icon()), "Import", BtnWidth::M, msg)
}

pub fn btn_sign<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Sign", BtnWidth::M, msg)
}

pub fn btn_broadcast<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Broadcast", BtnWidth::M, msg)
}

pub fn btn_new<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(Some(icon::plus_icon()), "New", BtnWidth::M, msg)
}

pub fn btn_processing<'a, T: Clone + 'a>() -> Button<'a, T> {
    btn_tertiary(None, "Processing...", BtnWidth::M, None)
}

pub fn btn_backup_encrypt_descriptor<'a, T: Clone + 'a>(msg: T) -> Button<'a, T> {
    let backup_label = "Back up encrypted descriptor";
    let backup_tooltip = "An encrypted descriptor file (.bed) you can store anywhere. To decrypt it, you need one of your signing devices or xpubs.";
    btn_with_tooltip(
        Some(icon::backup_icon()),
        backup_label,
        Some(backup_tooltip),
        BtnWidth::Auto,
        Some(msg),
        theme::button::tertiary,
    )
}

pub fn btn_update<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    if let Some(msg) = msg {
        btn_tertiary(None, "Update", BtnWidth::L, Some(msg))
    } else {
        btn_tertiary(None, "Updating", BtnWidth::M, None)
    }
}

pub fn btn_edit<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(Some(icon::edit_icon()), "Edit", BtnWidth::S, msg)
}

pub fn btn_set<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(Some(icon::edit_icon()), "Set", BtnWidth::S, msg)
}

pub fn btn_add_recovery_option<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(
        Some(icon::plus_icon()),
        "Add recovery option",
        BtnWidth::XL,
        msg,
    )
}

const SAFETY_NET_DESCRIPTION: &str = "This adds a final recovery option containing keys from professional key agents.\n\nUse this option if you have been provided one or more Safety Net tokens.";

pub fn btn_add_safety_net<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_with_tooltip(
        Some(icon::plus_icon()),
        "Add Safety Net",
        Some(SAFETY_NET_DESCRIPTION),
        BtnWidth::XL,
        msg,
        theme::button::tertiary,
    )
}

pub fn btn_select<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "Select", BtnWidth::M, msg)
}

pub fn btn_share_xpubs<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(None, "Share Xpubs", BtnWidth::M, msg)
}

pub fn btn_add_wallet<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    auxiliary(Some(icon::plus_icon()), "Add wallet", msg)
}

/// Full-width "Show QR Code" button for an optional modal section, with an
/// optional tooltip.
pub fn btn_show_qr_section<'a, M: 'a + 'static>(
    tt: Option<&'static str>,
    msg: Option<M>,
) -> Button<'a, M> {
    let mut btn = Button::new(
        Row::new()
            .push(icon::qr_icon().size(30))
            .push(button_text("Show QR Code"))
            .push_maybe(tt.map(tooltip))
            .spacing(20)
            .align_y(Vertical::Center)
            .padding(15),
    )
    .style(theme::button::tertiary)
    .width(Length::Fill);
    if let Some(msg) = msg {
        btn = btn.on_press(msg);
    }
    btn
}

const CLICKABLE_ICON_SIZE: u32 = 26;

pub fn btn_copy<'a, T: Clone + 'a>(msg: Option<T>) -> BistateButton<'a, T> {
    let size = Length::Fixed(CLICKABLE_ICON_SIZE as f32);
    BistateButton::new(
        Container::new(icon::copy_icon().size(CLICKABLE_ICON_SIZE))
            .center_x(size)
            .center_y(size),
        Container::new(icon::check_mark_icon().size(CLICKABLE_ICON_SIZE))
            .center_x(size)
            .center_y(size),
    )
    .on_press_maybe(msg)
    .style(move |theme, status| {
        let mut button_style = theme::button::transparent(theme, status);
        if status == Status::Hovered {
            button_style.text_color = theme.colors.general.accent;
        }
        button_style
    })
}

pub fn btn_icon_edit<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    clickable_icon_with_size(icon::edit_icon(), msg, CLICKABLE_ICON_SIZE)
}

pub fn btn_remove<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    Button::new(icon::cross_icon().size(CLICKABLE_ICON_SIZE))
        .on_press_maybe(msg)
        .style(theme::button::remove)
}

pub fn btn_delete<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_destructive(None, "Delete", BtnWidth::M, msg)
}

pub fn btn_previous<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(None, "< Previous", BtnWidth::M, msg)
}

pub fn btn_next<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    if msg.is_some() {
        btn_primary(None, "Next", BtnWidth::S, msg)
    } else {
        btn_secondary(None, "Next", BtnWidth::S, msg)
    }
}

pub fn btn_add_payment<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(Some(icon::plus_icon()), "Add payment", BtnWidth::Auto, msg)
}

pub fn btn_add_label<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_tertiary(Some(icon::edit_icon()), "Edit label", BtnWidth::L, msg)
}

pub fn btn_delete_wallet<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    destructive(None, "Delete wallet")
        .width(Length::Fixed(200.0))
        .on_press_maybe(msg)
}

pub fn btn_optional_section<'a, T: Clone + 'a>(content: Row<'a, T>, msg: T) -> Button<'a, T> {
    Button::new(content)
        .style(theme::button::optional_section)
        .on_press(msg)
}

pub fn btn_backup_descriptor<'a, T: Clone + 'a>(msg: Option<T>, primary: bool) -> Button<'a, T> {
    let icon = Some(icon::backup_icon());
    let label = "Back Up Descriptor";
    let width = BtnWidth::XL;
    if primary {
        btn_primary(icon, label, width, msg)
    } else {
        btn_secondary(icon, label, width, msg)
    }
}

pub fn btn_check_connection<'a, T: Clone + 'a>(msg: Option<T>, primary: bool) -> Button<'a, T> {
    let label = "Check connection";
    let width = BtnWidth::L;

    if primary {
        btn_primary(None, label, width, msg)
    } else {
        btn_secondary(None, label, width, msg)
    }
}
