use iced::widget::text::{Catalog, Style, StyleFn};

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(default)
    }

    fn style(&self, class: &Self::Class<'_>) -> Style {
        class(self)
    }
}

pub fn default(_theme: &Theme) -> Style {
    Style { color: None }
}

pub fn primary(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.primary),
    }
}

pub fn secondary(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.secondary),
    }
}

pub fn success(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.success),
    }
}

pub fn warning(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.warning),
    }
}

pub fn destructive(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.warning),
    }
}

pub fn error(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.error),
    }
}

pub fn custom(color: iced::Color) -> Style {
    Style { color: Some(color) }
}

/// Map a brand accent colour to a light-mode-friendly shade when the theme is
/// light, so installer diagrams/editors keep contrast in both modes. Unknown
/// colours pass through unchanged.
pub fn adapt_color(base: iced::Color, theme: &Theme) -> iced::Color {
    use crate::color;
    if matches!(theme.mode, super::palette::ThemeMode::Light) {
        if base == color::GREEN {
            color::DARK_GREEN
        } else if base == color::BLUE {
            color::DARK_BLUE
        } else if base == color::ORANGE {
            color::DARK_ORANGE
        } else if base == color::RED {
            color::DARK_RED
        } else if base == color::WHITE {
            // WHITE keys are invisible on light surfaces; use a dark neutral.
            color::GREY_7
        } else {
            base
        }
    } else {
        base
    }
}

/// Text style that renders `base` (a brand accent) with a light-mode-friendly
/// shade when the theme is light. Use for role-coloured key icons/labels.
pub fn adaptive(base: iced::Color) -> impl Fn(&Theme) -> Style {
    move |theme| Style {
        color: Some(adapt_color(base, theme)),
    }
}

/// Green for incoming amounts — darker on light backgrounds.
pub fn incoming(theme: &Theme) -> Style {
    use crate::color;
    Style {
        color: Some(match theme.mode {
            super::palette::ThemeMode::Light => color::DARK_GREEN,
            super::palette::ThemeMode::Dark => color::GREEN,
        }),
    }
}

/// Red for outgoing amounts — darker on light backgrounds.
pub fn outgoing(theme: &Theme) -> Style {
    use crate::color;
    Style {
        color: Some(match theme.mode {
            super::palette::ThemeMode::Light => color::DARK_RED,
            super::palette::ThemeMode::Dark => color::RED,
        }),
    }
}
