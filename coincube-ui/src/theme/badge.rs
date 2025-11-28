use iced::widget::container::Style;
use iced::{Background, Border};

use super::palette::ContainerPalette;
use super::Theme;

fn badge(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: Border {
            radius: 25.0.into(),
            width: 1.0,
            color: iced::Color::TRANSPARENT,
        },
        ..Default::default()
    }
}

pub fn simple(theme: &Theme) -> Style {
    badge(&theme.colors.badges.simple)
}

pub fn bitcoin(theme: &Theme) -> Style {
    badge(&theme.colors.badges.bitcoin)
}
