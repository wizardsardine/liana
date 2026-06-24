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

pub fn muted(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.muted),
    }
}

pub fn tertiary(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.muted),
    }
}

pub fn border(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.border),
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

pub fn accent(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.accent),
    }
}

pub fn card_secondary(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.card_secondary),
    }
}

pub fn address(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.address),
    }
}

pub fn address_dimmed(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.address_dimmed),
    }
}

pub fn custom(color: iced::Color) -> Style {
    Style { color: Some(color) }
}
