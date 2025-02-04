use iced::widget::container::Style;
use iced::{Background, Border};

use super::palette::ContainerPalette;
use super::Theme;

fn pill(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: Border {
            radius: 25.0.into(),
            width: 1.0,
            color: palette.border.unwrap_or_default(),
        },
        ..Default::default()
    }
}

pub fn simple(theme: &Theme) -> Style {
    pill(&theme.colors.pills.simple)
}

pub fn primary(theme: &Theme) -> Style {
    pill(&theme.colors.pills.primary)
}

pub fn success(theme: &Theme) -> Style {
    pill(&theme.colors.pills.success)
}

pub fn warning(theme: &Theme) -> Style {
    pill(&theme.colors.pills.warning)
}
