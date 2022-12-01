use iced::widget::{button, container, Container, Row, Text};
use iced::{Alignment, Color, Length, Vector};

use super::text::text;
use crate::ui::color;

pub fn alert<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> button::Button<'a, T> {
    button::Button::new(content(icon, t)).style(Style::Destructive.into())
}

pub fn primary<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> button::Button<'a, T> {
    button::Button::new(content(icon, t)).style(Style::Primary.into())
}

pub fn transparent<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> button::Button<'a, T> {
    button::Button::new(content(icon, t)).style(Style::Transparent.into())
}

pub fn border<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> button::Button<'a, T> {
    button::Button::new(content(icon, t)).style(Style::Border.into())
}

pub fn transparent_border<'a, T: 'a>(
    icon: Option<Text<'a>>,
    t: &'static str,
) -> button::Button<'a, T> {
    button::Button::new(content(icon, t)).style(Style::TransparentBorder.into())
}

fn content<'a, T: 'a>(icon: Option<Text<'a>>, t: &'static str) -> Container<'a, T> {
    match icon {
        None => container(text(t)).width(Length::Fill).center_x().padding(5),
        Some(i) => container(
            Row::new()
                .push(i)
                .push(text(t))
                .spacing(10)
                .width(iced::Length::Fill)
                .align_items(Alignment::Center),
        )
        .width(iced::Length::Fill)
        .center_x()
        .padding(5),
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Style {
    Primary,
    Transparent,
    TransparentBorder,
    Border,
    Destructive,
}

impl button::StyleSheet for Style {
    type Style = iced::Theme;
    fn active(&self, _style: &Self::Style) -> button::Appearance {
        match self {
            Style::Primary => button::Appearance {
                shadow_offset: Vector::default(),
                background: color::PRIMARY.into(),
                border_radius: 10.0,
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
                text_color: color::FOREGROUND,
            },
            Style::Destructive => button::Appearance {
                shadow_offset: Vector::default(),
                background: color::FOREGROUND.into(),
                border_radius: 10.0,
                border_width: 0.0,
                border_color: color::ALERT,
                text_color: color::ALERT,
            },
            Style::Transparent | Style::TransparentBorder => button::Appearance {
                shadow_offset: Vector::default(),
                background: Color::TRANSPARENT.into(),
                border_radius: 10.0,
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
                text_color: Color::BLACK,
            },
            Style::Border => button::Appearance {
                shadow_offset: Vector::default(),
                background: Color::TRANSPARENT.into(),
                border_radius: 10.0,
                border_width: 1.2,
                border_color: color::BORDER_GREY,
                text_color: Color::BLACK,
            },
        }
    }

    fn hovered(&self, _style: &Self::Style) -> button::Appearance {
        match self {
            Style::Primary => button::Appearance {
                shadow_offset: Vector::default(),
                background: color::PRIMARY.into(),
                border_radius: 10.0,
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
                text_color: color::FOREGROUND,
            },
            Style::Destructive => button::Appearance {
                shadow_offset: Vector::default(),
                background: color::FOREGROUND.into(),
                border_radius: 10.0,
                border_width: 0.0,
                border_color: color::ALERT,
                text_color: color::ALERT,
            },
            Style::Transparent => button::Appearance {
                shadow_offset: Vector::default(),
                background: Color::TRANSPARENT.into(),
                border_radius: 10.0,
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
                text_color: Color::BLACK,
            },
            Style::TransparentBorder => button::Appearance {
                shadow_offset: Vector::default(),
                background: Color::TRANSPARENT.into(),
                border_radius: 10.0,
                border_width: 1.0,
                border_color: Color::BLACK,
                text_color: Color::BLACK,
            },
            Style::Border => button::Appearance {
                shadow_offset: Vector::default(),
                background: Color::TRANSPARENT.into(),
                border_radius: 10.0,
                border_width: 1.0,
                border_color: Color::BLACK,
                text_color: Color::BLACK,
            },
        }
    }
}

impl From<Style> for Box<dyn button::StyleSheet<Style = iced::Theme>> {
    fn from(s: Style) -> Box<dyn button::StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<Style> for iced::theme::Button {
    fn from(i: Style) -> iced::theme::Button {
        iced::theme::Button::Custom(i.into())
    }
}
