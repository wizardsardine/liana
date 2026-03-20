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

pub fn success(theme: &Theme) -> Style {
    notification(&theme.colors.notifications.success)
}

pub fn warning(theme: &Theme) -> Style {
    notification(&theme.colors.notifications.warning)
}

pub fn info(theme: &Theme) -> Style {
    notification(&theme.colors.notifications.info)
}

/// Map log::Level to the notification palette for consistent styling
pub fn palette_for_level<'a>(level: &log::Level, theme: &'a Theme) -> &'a ContainerPalette {
    match level {
        log::Level::Error => &theme.colors.notifications.error,
        log::Level::Warn => &theme.colors.notifications.warning,
        log::Level::Info => &theme.colors.notifications.info,
        log::Level::Debug => &theme.colors.notifications.debug,
        log::Level::Trace => &theme.colors.notifications.debug,
    }
}
