use crate::{theme, widget::*};
use iced::widget::{button, container, row};
use iced::{Alignment, Length};

use super::text::text;

pub fn menu<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t).padding(10)).style(theme::Button::Menu(false))
}

pub fn menu_active<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Button<'a, T> {
    button::Button::new(content(icon, t).padding(10)).style(theme::Button::Menu(true))
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
        None => container(text(t)).width(Length::Fill).center_x().padding(5),
        Some(i) => container(
            row![i, text(t)]
                .spacing(10)
                .width(iced::Length::Fill)
                .align_items(Alignment::Center),
        )
        .width(iced::Length::Fill)
        .center_x()
        .padding(5),
    }
}
