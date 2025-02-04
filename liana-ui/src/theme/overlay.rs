use iced::{
    widget::overlay::menu::{Catalog, Style, StyleFn},
    Border,
};

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> <Self as Catalog>::Class<'a> {
        Box::new(primary)
    }

    fn style(&self, class: &<Self as Catalog>::Class<'_>) -> Style {
        class(self)
    }
}

pub fn primary(theme: &Theme) -> Style {
    Style {
        text_color: theme.colors.text.primary,
        background: theme.colors.buttons.secondary.active.background.into(),
        selected_text_color: theme.colors.buttons.primary.active.text,
        selected_background: theme.colors.buttons.primary.active.background.into(),
        border: if let Some(color) = theme.colors.buttons.secondary.active.border {
            Border {
                radius: 25.0.into(),
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
