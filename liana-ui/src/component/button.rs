use super::{
    modal::BTN_W,
    text::{button_text, panel_title, text},
};
use crate::{
    font::{BOLD, MEDIUM},
    icon::ICON_SIZE_L,
    theme::{self, button::round_icon_btn},
    widget::*,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{container, row},
    Length,
};

const MENU_BTN_PADDING: [u16; 2] = [4 /* Top/Bottom */, 12 /* Left/Right */];
const MENU_TEXT_SIZE: u32 = 22;
const MENU_TEXT_COMPACT_SIZE: u32 = 18;
const MENU_ICON_SIZE: u32 = ICON_SIZE_L as u32;

const ICON_BTN_SIZE: f32 = 40.0;
const ICON_BTN_PADDING: f32 = 10.0;
pub const DEVICE_BTN_H: u32 = 40;

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
}

pub fn menu_active<'a, T: 'a>(
    icon: Option<Text<'a>>,
    t: &'static str,
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

macro_rules! button_helpers {
    ($($entry:tt),* $(,)?) => {
        $( button_helpers!(@one $entry); )*
    };
    (@one ($name:ident, $style:ident)) => {
        pub fn $name<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
            Button::new(content(icon, button_text(t))).style(theme::button::$style)
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

pub fn breadcrumb<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, panel_title(t))).style(theme::button::breadcrumb)
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

fn content<'a, T: 'a>(icon: Option<Text<'a>>, text: Text<'a>) -> Container<'a, T> {
    match icon {
        None => container(text).align_x(Horizontal::Center).padding(5),
        Some(i) => container(row![i, text].spacing(10).align_y(Vertical::Center))
            .align_x(Horizontal::Center)
            .padding(5),
    }
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
}

/// Primary button with preset width.
pub fn btn_primary<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = primary(icon, label).width(Length::Fixed(width as u16 as f32));
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Secondary button with preset width.
pub fn btn_secondary<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = secondary(icon, label).width(Length::Fixed(width as u16 as f32));
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Tertiary button with preset width.
pub fn btn_tertiary<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = tertiary(icon, label).width(Length::Fixed(width as u16 as f32));
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
    let mut btn = destructive(icon, label).width(Length::Fixed(width as u16 as f32));
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
    let mut btn = flat(icon, label).width(Length::Fixed(width as u16 as f32));
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Save button: primary. Width M.
pub fn btn_save<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "Save", BtnWidth::M, msg)
}

/// Cancel button: destructive. Width M.
pub fn btn_cancel<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_destructive(None, "Cancel", BtnWidth::M, msg)
}

/// OK button: primary. Width M.
pub fn btn_ok<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_primary(None, "OK", BtnWidth::M, msg)
}

/// Clear button: secondary. Width M.
pub fn btn_clear<'a, T: Clone + 'a>(msg: Option<T>) -> Button<'a, T> {
    btn_secondary(None, "Clear", BtnWidth::M, msg)
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
