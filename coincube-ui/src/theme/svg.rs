use iced::widget::svg::{Catalog, Status, Style, StyleFn};

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(default)
    }

    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style {
        class(self, status)
    }
}

pub fn default(_theme: &Theme, _status: Status) -> Style {
    Style { color: None }
}

/// Tint for monochrome SVG icons used in the nav rail. Matches the primary
/// text color so the SVG tracks light/dark mode like the Bootstrap font glyphs
/// rendered alongside it.
pub fn nav_icon(theme: &Theme, _status: Status) -> Style {
    Style {
        color: Some(theme.colors.text.primary),
    }
}
