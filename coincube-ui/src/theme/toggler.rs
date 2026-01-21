use iced::widget::toggler::{Catalog, Status, Style, StyleFn};

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> <Self as Catalog>::Class<'a> {
        Box::new(primary)
    }

    fn style(&self, class: &<Self as Catalog>::Class<'_>, status: Status) -> Style {
        class(self, status)
    }
}

pub fn primary(theme: &Theme, status: Status) -> Style {
    match status {
        Status::Active { is_toggled: true } | Status::Hovered { is_toggled: true } => Style {
            background: iced::Background::Color(theme.colors.togglers.on.background),
            background_border_width: 1.0,
            background_border_color: theme.colors.togglers.on.background_border,
            foreground: iced::Background::Color(theme.colors.togglers.on.foreground),
            foreground_border_width: 1.0,
            foreground_border_color: theme.colors.togglers.on.foreground_border,
            text_color: None,
            border_radius: None,
            padding_ratio: 1.0,
        },
        _ => Style {
            background: iced::Background::Color(theme.colors.togglers.off.background),
            background_border_width: 1.0,
            background_border_color: theme.colors.togglers.off.background_border,
            foreground: iced::Background::Color(theme.colors.togglers.off.foreground),
            foreground_border_width: 1.0,
            foreground_border_color: theme.colors.togglers.off.foreground_border,
            text_color: None,
            border_radius: None,
            padding_ratio: 1.0,
        },
    }
}

pub fn orange(_theme: &Theme, status: Status) -> Style {
    let orange = iced::color!(0xF7931B);
    let grey_off = iced::color!(0x424242);
    let white = iced::Color::WHITE;

    match status {
        Status::Active { is_toggled: true } | Status::Hovered { is_toggled: true } => Style {
            background: iced::Background::Color(orange),
            background_border_width: 0.0,
            background_border_color: orange,
            foreground: iced::Background::Color(white),
            foreground_border_width: 0.0,
            foreground_border_color: white,
            text_color: None,
            border_radius: Some(12.0.into()),
            padding_ratio: 0.1,
        },
        _ => Style {
            background: iced::Background::Color(grey_off),
            background_border_width: 0.0,
            background_border_color: grey_off,
            foreground: iced::Background::Color(white),
            foreground_border_width: 0.0,
            foreground_border_color: white,
            text_color: None,
            border_radius: Some(12.0.into()),
            padding_ratio: 0.1,
        },
    }
}
