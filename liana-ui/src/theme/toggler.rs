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
            background: theme.colors.togglers.on.background,
            background_border_width: 1.0,
            background_border_color: theme.colors.togglers.on.background_border,
            foreground: theme.colors.togglers.on.foreground,
            foreground_border_width: 1.0,
            foreground_border_color: theme.colors.togglers.on.foreground_border,
        },
        _ => Style {
            background: theme.colors.togglers.off.background,
            background_border_width: 1.0,
            background_border_color: theme.colors.togglers.off.background_border,
            foreground: theme.colors.togglers.off.foreground,
            foreground_border_width: 1.0,
            foreground_border_color: theme.colors.togglers.off.foreground_border,
        },
    }
}
