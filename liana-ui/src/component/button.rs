use std::fmt::Display;

use super::{
    modal::BTN_W,
    text::{
        new::{button_text, button_text_compact, BUTTON_TEXT_COMPACT_SPEC},
        panel_title, text,
    },
    tooltip,
};
use crate::{
    font::{BOLD, MEDIUM},
    icon::{self, ICON_SIZE_L},
    theme::{self, button::round_icon_btn, Theme},
    widget::*,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button::{Status, Style},
        container, row,
    },
    Length,
};
use liana_i18n::t;

const MENU_BTN_PADDING: [u16; 2] = [4 /* Top/Bottom */, 12 /* Left/Right */];
const MENU_TEXT_SIZE: u32 = 22;
const MENU_TEXT_COMPACT_SIZE: u32 = 18;
const MENU_ICON_SIZE: u32 = ICON_SIZE_L as u32;

const ICON_BTN_SIZE: f32 = 40.0;
const ICON_BTN_PADDING: f32 = 10.0;
pub const DEVICE_BTN_H: u32 = 40;

pub fn menu<'a, T: 'a>(icon: Option<Text<'a>>, t: impl Display, compact: bool) -> Button<'a, T> {
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
}

pub fn menu_active<'a, T: 'a>(
    icon: Option<Text<'a>>,
    t: impl Display,
    compact: bool,
) -> Button<'a, T> {
    Button::new(content_menu(icon, t, true, compact).padding(MENU_BTN_PADDING))
        .style(theme::button::menu_pressed)
}

pub fn menu_small<'a, T: 'a>(icon: Text<'a>) -> Button<'a, T> {
    Button::new(
        container(icon.size(MENU_ICON_SIZE).style(theme::text::secondary))
            .padding(MENU_BTN_PADDING)
            .align_x(Horizontal::Center),
    )
    .style(theme::button::menu)
}

pub fn menu_active_small<'a, T: 'a>(icon: Text<'a>) -> Button<'a, T> {
    Button::new(
        container(icon.size(MENU_ICON_SIZE))
            .padding(MENU_BTN_PADDING)
            .align_x(Horizontal::Center),
    )
    .style(theme::button::menu_pressed)
}

fn content_menu<'a, T: 'a>(
    icon: Option<Text<'a>>,
    t: impl Display,
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
    Button::new(content(icon, text, compact)).style(style)
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

pub fn breadcrumb<'a, T: 'a>(icon: Option<Text<'a>>, t: impl Display) -> Button<'a, T> {
    Button::new(content(icon, panel_title(t), false)).style(theme::button::breadcrumb)
}
pub fn clickable_card<'a, M: 'a + Clone, T: Into<Element<'a, M>>>(
    content: T,
    msg: Option<M>,
) -> Button<'a, M> {
    Button::new(content.into())
        .style(theme::button::clickable_card)
        .on_press_maybe(msg)
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

fn content_with_tooltip<'a, T: 'a>(
    icon: Option<Text<'a>>,
    text: Text<'a>,
    tooltip: Option<String>,
    compact: bool,
) -> Container<'a, T> {
    let mut row: Row<'a, T> = Row::new().spacing(10).align_y(Vertical::Center);
    if let Some(icon) = icon {
        row = row.push(icon);
    }
    row = row.push(text);
    if let Some(tt) = tooltip {
        row = row.push(tooltip::tooltip(tt));
    }
    let padding = if compact { 2 } else { 4 };
    container(row).align_x(Horizontal::Center).padding(padding)
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
    /// Short labels (Save, OK, Retry, Skip)
    S = 100,
    /// Standard labels (Cancel, Clear, Unlock)
    M = 120,
    /// Longer labels (Keep my changes, Send token)
    L = 160,
    /// Long labels (Send for approval, Approve Template, Manage Keys)
    XL = 200,
    /// Very long labels (Connect with another email)
    XXL = 260,
    /// Default to Length::Shrink
    Auto,
}

impl From<BtnWidth> for Length {
    fn from(value: BtnWidth) -> Self {
        match value {
            BtnWidth::Auto => Length::Shrink,
            v => (v as u16 as u32).into(),
        }
    }
}

/// Primary button with preset width.
pub fn btn_primary<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: impl Display,
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
    label: impl Display,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = secondary(icon, label).width(width);
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Secondary button with preset width.
pub fn btn_secondary_with_tooltip<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: impl Display,
    tooltip: Option<impl Into<String>>,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    Button::new(content_with_tooltip(
        icon,
        button_text(label),
        tooltip.map(Into::into),
        false,
    ))
    .width(width)
    .style(theme::button::secondary)
    .on_press_maybe(msg)
}

/// Tertiary button with preset width.
pub fn btn_tertiary<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: impl Display,
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
    label: impl Display,
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
    label: impl Display,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = flat(icon, label).width(width);
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Save button: primary. Width M.
pub fn btn_save<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, t!("common-save"), BtnWidth::M, msg)
}

/// Cancel button: destructive. Width M.
pub fn btn_cancel<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_destructive(None, t!("common-cancel"), BtnWidth::M, msg)
}

/// OK button: primary. Width M.
pub fn btn_ok<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, t!("common-ok"), BtnWidth::M, msg)
}

/// Clear button: secondary. Width M.
pub fn btn_clear<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, t!("common-clear"), BtnWidth::M, msg)
}

/// Retry button: secondary. Width M.
pub fn btn_retry<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, t!("common-retry"), BtnWidth::M, msg)
}

/// Yes button: primary. Width S.
pub fn btn_yes<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, t!("common-yes"), BtnWidth::S, msg)
}

/// No button: secondary. Width S.
pub fn btn_no<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, t!("common-no"), BtnWidth::S, msg)
}

pub fn btn_reset_timelock<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(
        Some(icon::reload_icon()),
        t!("common-reset-timelock"),
        BtnWidth::Auto,
        msg,
    )
}

pub fn btn_go_to_rescan<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, t!("common-go-to-rescan"), BtnWidth::XL, msg)
}

pub fn btn_dismiss<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_destructive(
        Some(icon::big_cross_icon()),
        t!("common-dismiss"),
        BtnWidth::L,
        msg,
    )
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
