use std::time::Duration;

use super::text::text;
use crate::component::spinner;
use crate::font::MEDIUM;
use crate::theme;
use crate::widget::{Button, Container, Text};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, container, row};

pub fn menu<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content_menu(icon.map(|i| i.style(theme::text::secondary)), t).padding(10))
        .style(theme::button::menu)
}

pub fn menu_active<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content_menu(icon.map(|i| i.style(theme::text::secondary)), t).padding(10))
        .style(theme::button::menu_pressed)
}

pub fn menu_small<'a, T: 'a>(icon: Text<'a>) -> Button<'a, T> {
    Button::new(container(icon.style(theme::text::secondary)).padding(10))
        .style(theme::button::menu)
}

pub fn menu_active_small<'a, T: 'a>(icon: Text<'a>) -> Button<'a, T> {
    Button::new(container(icon.style(theme::text::secondary)).padding(10))
        .style(theme::button::menu_pressed)
}

fn content_menu<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Container<'a, T> {
    match icon {
        None => container(text(t)).padding(5),
        Some(i) => container(row![i, text(t)].spacing(10).align_y(Vertical::Center)).padding(5),
    }
}

pub fn alert<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(icon, text(t))).style(theme::button::destructive)
}

pub fn primary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(
        icon,
        text(t)
            .font(MEDIUM)
            .align_y(iced::Alignment::Center)
            .align_x(iced::Alignment::Center),
    ))
    .style(theme::button::primary)
}

/// Compact primary button - shrinks to content, left-aligned (for action buttons)
pub fn primary_compact<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content_left_aligned(
        icon,
        text(t).font(MEDIUM).align_y(iced::Alignment::Center),
    ))
    .style(theme::button::primary)
}

pub fn transparent<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(
        icon,
        text(t)
            .align_y(iced::Alignment::Center)
            .align_x(iced::Alignment::Center),
    ))
    .style(theme::button::container)
}

pub fn secondary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(
        icon,
        text(t)
            .align_y(iced::Alignment::Center)
            .align_x(iced::Alignment::Center),
    ))
    .style(theme::button::secondary)
}

/// Compact secondary button - shrinks to content, left-aligned (for action buttons like "share xpubs")
pub fn secondary_compact<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content_left_aligned(
        icon,
        text(t).align_y(iced::Alignment::Center),
    ))
    .style(theme::button::secondary)
}

pub fn border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content_left_aligned(
        icon,
        text(t).align_y(iced::Alignment::Center),
    ))
    .style(theme::button::secondary)
}

pub fn transparent_border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button(content_left_aligned(
        icon,
        text(t).align_y(iced::Alignment::Center),
    ))
    .style(theme::button::container_border)
}

/// Transparent bordered button with centered content — for segmented toggles
/// where the button has a fixed/fill width and the label should sit in the
/// middle (e.g. the Spark Receive method picker). Same style as
/// [`transparent_border`], but centered rather than left-aligned.
pub fn transparent_border_centered<'a, T: 'a>(
    icon: Option<Text<'a>>,
    t: &'static str,
) -> Button<'a, T> {
    Button::new(content(
        icon,
        text(t)
            .align_y(iced::Alignment::Center)
            .align_x(iced::Alignment::Center),
    ))
    .style(theme::button::container_border)
}

/// Orange-on-transparent outline button with a solid `DARK_ORANGE`
/// fill + black text on hover. Centered content — use this for
/// "Receive" and similar secondary actions that should read as
/// orange in idle and match the primary button's press feedback
/// on hover.
pub fn orange_outline<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content(
        icon,
        text(t)
            .font(MEDIUM)
            .align_y(iced::Alignment::Center)
            .align_x(iced::Alignment::Center),
    ))
    .style(theme::button::orange_outline)
}

pub fn link<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content_left_aligned(icon, text(t))).style(theme::button::link)
}

/// Primary button that reflects an in-flight async action: while `loading`
/// is `true` it shows the label with an animated typing ellipsis and is
/// inert, so a single press can't be repeated. Otherwise it behaves like
/// `primary(icon, t).on_press_maybe(on_press)` — pass `None` to keep the
/// button disabled (e.g. invalid input) without showing the spinner.
pub fn primary_loading<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    t: &'static str,
    loading: bool,
    on_press: Option<T>,
) -> Button<'a, T> {
    if loading {
        Button::new(content_loading(t)).style(theme::button::primary)
    } else {
        primary(icon, t).on_press_maybe(on_press)
    }
}

/// Secondary variant of [`primary_loading`].
pub fn secondary_loading<'a, T: Clone + 'a>(
    icon: Option<Text<'a>>,
    t: &'static str,
    loading: bool,
    on_press: Option<T>,
) -> Button<'a, T> {
    if loading {
        Button::new(content_loading(t)).style(theme::button::secondary)
    } else {
        secondary(icon, t).on_press_maybe(on_press)
    }
}

// Content for a loading button: centered label followed by an animated
// typing ellipsis. Requires `T: Clone` because the spinner Carousel widget
// clones its message type.
fn content_loading<'a, T: Clone + 'a>(t: &'static str) -> Container<'a, T> {
    container(
        row![
            text(t).font(MEDIUM).align_y(Vertical::Center),
            spinner::typing_text_carousel("...", true, Duration::from_millis(400), |c| {
                text(c).font(MEDIUM).align_y(Vertical::Center)
            }),
        ]
        .spacing(2)
        .align_y(Vertical::Center)
        .width(iced::Length::Shrink),
    )
    .align_x(Horizontal::Center)
    .width(iced::Length::Fill)
    .padding(5)
}

// Content function for centered buttons (primary, secondary, transparent)
fn content<'a, T: 'a>(icon: Option<Text<'a>>, text: Text<'a>) -> Container<'a, T> {
    match icon {
        None => container(text)
            .align_y(Vertical::Center)
            .align_x(Horizontal::Center)
            .width(iced::Length::Fill)
            .padding(5),
        Some(i) => container(
            row![i, text]
                .spacing(10)
                .align_y(Vertical::Center)
                .width(iced::Length::Shrink),
        )
        .align_x(Horizontal::Center)
        .width(iced::Length::Fill)
        .padding(5),
    }
}

// Content function for left-aligned buttons (border, transparent_border, link)
fn content_left_aligned<'a, T: 'a>(icon: Option<Text<'a>>, text: Text<'a>) -> Container<'a, T> {
    match icon {
        None => container(text).align_y(Vertical::Center).padding(5),
        Some(i) => container(row![i, text].spacing(10).align_y(Vertical::Center)).padding(5),
    }
}
