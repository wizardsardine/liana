use iced::widget::container::{transparent, Catalog, Style, StyleFn};
use iced::Background;

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(transparent)
    }

    fn style(&self, class: &Self::Class<'_>) -> Style {
        class(self)
    }
}

pub fn background(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        ..Default::default()
    }
}

#[allow(unused)]
pub fn debug(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(iced::Color::WHITE)),
        border: iced::Border::default().color(iced::color!(0xFF0000)),
        ..Default::default()
    }
}

pub fn foreground(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.foreground)),
        ..Default::default()
    }
}

pub fn border(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        ..Default::default()
    }
}

pub fn custom(color: iced::Color) -> Box<dyn Fn(&Theme) -> Style> {
    Box::new(move |_theme: &Theme| Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    })
}

pub fn border_orange(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        border: iced::Border {
            color: crate::color::ORANGE,
            width: 2.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

pub fn border_grey(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        border: iced::Border {
            color: theme.colors.text.secondary,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}
