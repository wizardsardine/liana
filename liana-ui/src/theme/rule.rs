use iced::widget::rule::{Catalog, FillMode, Style, StyleFn};

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

/// The default styling of a [`Rule`].
pub fn default(theme: &Theme) -> Style {
    Style {
        color: theme.colors.rule,
        width: 2,
        radius: 0.0.into(),
        fill_mode: FillMode::Full,
    }
}
