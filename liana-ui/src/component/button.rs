use super::text::{button_text, text};
use crate::{
    font::{BOLD, MEDIUM},
    icon::ICON_SIZE_L,
    theme::{self, button::round_icon_btn},
    widget::*,
};
use iced::{
    alignment::{Horizontal, Vertical},
    widget::{button, container, row},
    Length,
};

const MENU_BTN_PADDING: [u16; 2] = [4 /* Top/Bottom */, 12 /* Left/Right */];
const MENU_TEXT_SIZE: u16 = 22;
const MENU_TEXT_COMPACT_SIZE: u16 = 18;
const MENU_ICON_SIZE: u16 = ICON_SIZE_L;

const ICON_BTN_SIZE: f32 = 40.0;
const ICON_BTN_PADDING: f32 = 10.0;

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

pub fn alert<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, button_text(t))).style(theme::button::destructive)
}

pub fn primary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, button_text(t))).style(theme::button::primary)
}

pub fn transparent<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, button_text(t))).style(theme::button::container)
}

pub fn flat<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, button_text(t))).style(theme::button::transparent)
}

pub fn secondary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, button_text(t))).style(theme::button::secondary)
}

pub fn tertiary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, button_text(t))).style(theme::button::tertiary)
}

pub fn border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, button_text(t))).style(theme::button::secondary)
}

pub fn transparent_border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button(content(icon, button_text(t))).style(theme::button::container_border)
}

pub fn link<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, button_text(t))).style(theme::button::link)
}

fn content<'a, T: 'a>(icon: Option<Text<'a>>, text: Text<'a>) -> Container<'a, T> {
    match icon {
        None => container(text).align_x(Horizontal::Center).padding(5),
        Some(i) => container(row![i, text].spacing(10).align_y(Vertical::Center))
            .align_x(Horizontal::Center)
            .padding(5),
    }
}

/// Button width presets.
#[derive(Debug, Clone, Copy)]
pub enum BtnWidth {
    /// Short labels (Save, OK, Retry)
    S = 100,
    /// Standard labels (Cancel, Clear, Delete)
    M = 120,
    /// Longer labels (Keep my changes)
    L = 160,
}

/// Primary button with preset width.
pub fn btn_primary<'a, T: Clone + 'a>(
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = primary(None, label).width(Length::Fixed(width as u16 as f32));
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
}

/// Secondary button with preset width.
pub fn btn_secondary<'a, T: Clone + 'a>(
    label: &'static str,
    width: BtnWidth,
    msg: Option<T>,
) -> Button<'a, T> {
    let mut btn = secondary(None, label).width(Length::Fixed(width as u16 as f32));
    if let Some(m) = msg {
        btn = btn.on_press(m);
    }
    btn
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
