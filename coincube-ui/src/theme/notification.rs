use iced::widget::container::Style;
use iced::{Background, Border};

use super::palette::ContainerPalette;
use super::Theme;

fn notification(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: if let Some(color) = palette.border {
            Border {
                width: 1.0,
                color,
                radius: 25.0.into(),
            }
        } else {
            Border {
                ..Default::default()
            }
        },
        ..Default::default()
    }
}

pub fn pending(theme: &Theme) -> Style {
    notification(&theme.colors.notifications.pending)
}

pub fn error(theme: &Theme) -> Style {
    notification(&theme.colors.notifications.error)
}
