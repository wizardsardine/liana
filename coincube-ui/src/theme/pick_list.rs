use iced::{
    widget::pick_list::{Catalog, Status, Style, StyleFn},
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
        text_color: theme.colors.buttons.secondary.active.text,
        placeholder_color: theme.colors.buttons.secondary.active.text,
        background: theme.colors.buttons.secondary.active.background.into(),
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
        handle_color: theme.colors.buttons.secondary.active.text,
    }
}
