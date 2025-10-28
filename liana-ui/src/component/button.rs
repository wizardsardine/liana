use super::text::text;
use crate::font::MEDIUM;
use crate::{theme, widget::*};
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
    Button::new(
        container(icon.style(theme::text::secondary))
            .padding(10)
            .align_x(Horizontal::Center),
    )
    .style(theme::button::menu)
}

pub fn menu_active_small<'a, T: 'a>(icon: Text<'a>) -> Button<'a, T> {
    Button::new(
        container(icon.style(theme::text::secondary))
            .padding(10)
            .align_x(Horizontal::Center),
    )
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

pub fn link<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    Button::new(content_left_aligned(icon, text(t))).style(theme::button::link)
}

/// Xpubs button - compact button specifically for hardware wallet xpubs actions
/// Uses completely minimal layout to ensure it never stretches
pub fn xpubs_button<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    // Minimal content - no width constraints, just natural padding
    let content = match icon {
        None => container(text(t).align_y(iced::Alignment::Center)).padding(5),
        Some(i) => container(
            row![i, text(t)]
                .spacing(10)
                .align_y(iced::alignment::Vertical::Center),
        )
        .padding(5),
    };

    Button::new(content).style(theme::button::secondary)
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
