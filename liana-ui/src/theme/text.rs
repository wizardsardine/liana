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

pub fn payjoin(theme: &Theme) -> Style {
    Style {
        color: Some(theme.colors.text.payjoin),
    }
}
