use iced::widget::container::Style;
use iced::{Background, Border};

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

pub fn simple(theme: &Theme) -> Style {
    card(&theme.colors.cards.simple)
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
