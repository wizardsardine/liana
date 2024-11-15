use crate::{color, theme, widget::*};
use iced::widget::{button, container, row};
use iced::Alignment;

use super::text::text;

pub fn menu<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content_menu(icon.map(|i| i.style(color::GREY_3)), t).padding(10))
        .style(theme::Button::Menu(false))
}

pub fn menu_active<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content_menu(icon.map(|i| i.style(color::GREY_3)), t).padding(10))
        .style(theme::Button::Menu(true))
}

fn content_menu<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Container<'a, T> {
    match icon {
        None => container(text(t)).padding(5),
        Some(i) => {
            container(row![i, text(t)].spacing(10).align_items(Alignment::Center)).padding(5)
        }
    }
}

pub fn alert<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::Button::Destructive)
}

pub fn primary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::Button::Primary)
}

pub fn transparent<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::Button::Transparent)
}

pub fn secondary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::Button::Secondary)
}

pub fn border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t)).style(theme::Button::Border)
}

pub fn transparent_border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button(content(icon, t)).style(theme::Button::TransparentBorder)
}

fn content<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Container<'a, T> {
    match icon {
        None => container(text(t)).center_x().padding(5),
        Some(i) => container(row![i, text(t)].spacing(10).align_items(Alignment::Center))
            .center_x()
            .padding(5),
    }
}
