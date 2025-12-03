use iced::widget::container::Style;
use iced::{Background, Border};

use super::palette::ContainerPalette;
use super::Theme;

fn banner(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: if let Some(color) = palette.border {
            Border {
                width: 1.0,
                color,
                ..Default::default()
            }
        } else {
            Border {
                ..Default::default()
            }
        },
        ..Default::default()
    }
}

pub fn network(theme: &Theme) -> Style {
    banner(&theme.colors.banners.network)
}

pub fn warning(theme: &Theme) -> Style {
    banner(&theme.colors.banners.warning)
}
