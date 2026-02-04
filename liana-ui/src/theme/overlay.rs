pub use iced::widget::overlay::menu::Catalog;
use iced::{
    widget::overlay::menu::{Style, StyleFn},
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
        background: theme.colors.cards.simple.background.into(),
        selected_text_color: theme.colors.buttons.menu.hovered.text,
        selected_background: theme.colors.buttons.menu.hovered.background.into(),
        border: if let Some(color) = theme.colors.cards.simple.border {
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
