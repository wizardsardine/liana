use iced::{
    widget::checkbox::{Catalog, Status, Style, StyleFn},
    Border,
};

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

pub fn primary(theme: &Theme, _status: Status) -> Style {
    Style {
        icon_color: theme.colors.checkboxes.icon,
        text_color: theme.colors.checkboxes.text.into(),
        background: theme.colors.checkboxes.background.into(),
        border: if let Some(color) = theme.colors.checkboxes.border {
            Border {
                radius: 4.0.into(),
                width: 1.0,
                color,
            }
        } else {
            Border {
                ..Default::default()
            }
        },
    }
}
