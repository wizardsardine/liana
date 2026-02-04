use iced::widget::container::Style;
use iced::{Background, Border, Color, Shadow, Vector};

use super::palette::ContainerPalette;
use super::Theme;

fn card(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: if let Some(color) = palette.border {
            Border {
                radius: 25.0.into(),
                width: 1.0,
                color,
            }
        } else {
            Border {
                ..Default::default()
            }
        },
        ..Default::default()
    }
}

fn card_with_shadow(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: if let Some(color) = palette.border {
            Border {
                radius: 25.0.into(),
                width: 1.0,
                color,
            }
        } else {
            Border {
                radius: 25.0.into(),
                ..Default::default()
            }
        },
        shadow: Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.15),
            offset: Vector::new(0.0, 2.0),
            blur_radius: 8.0,
        },
    }
}

pub fn simple(theme: &Theme) -> Style {
    card_with_shadow(&theme.colors.cards.simple)
}

pub fn modal(theme: &Theme) -> Style {
    card(&theme.colors.cards.modal)
}

pub fn border(theme: &Theme) -> Style {
    card(&theme.colors.cards.border)
}

pub fn invalid(theme: &Theme) -> Style {
    card(&theme.colors.cards.invalid)
}

pub fn warning(theme: &Theme) -> Style {
    card(&theme.colors.cards.warning)
}

pub fn error(theme: &Theme) -> Style {
    card(&theme.colors.cards.error)
}
