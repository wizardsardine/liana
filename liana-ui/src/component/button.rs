use crate::{theme, widget::*};
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, container, row};

use super::text::text;

pub fn menu<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content_menu(icon.map(|i| i.style(theme::text::secondary)), t).padding(10))
        .style(theme::button::menu)
}

pub fn menu_active<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content_menu(icon.map(|i| i.style(theme::text::secondary)), t).padding(10))
        .style(theme::button::menu_pressed)
}

fn content_menu<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Container<'a, T> {
    match icon {
        None => container(text(t)).padding(5),
        Some(i) => container(row![i, text(t)].spacing(10).align_y(Vertical::Center)).padding(5),
    }
}

pub fn alert<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::button::destructive)
}

pub fn primary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::button::primary)
}

pub fn transparent<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::button::container)
}

pub fn secondary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::button::secondary)
}

pub fn border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::button::secondary)
}

pub fn transparent_border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button(content(icon, t)).style(theme::button::container_border)
}

fn content<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Container<'a, T> {
    match icon {
        None => container(text(t)).align_x(Horizontal::Center).padding(5),
        Some(i) => container(row![i, text(t)].spacing(10).align_y(Vertical::Center))
            .align_x(Horizontal::Center)
            .padding(5),
    }
}
