use iced::widget::radio::{Catalog, Status, Style, StyleFn};

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
        dot_color: theme.colors.radio_buttons.dot,
        text_color: theme.colors.radio_buttons.text.into(),
        background: theme.colors.cards.simple.background.into(),
        border_width: 1.0,
        border_color: theme.colors.radio_buttons.border,
    }
}
