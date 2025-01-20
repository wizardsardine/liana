use iced::widget::qr_code::{Catalog, Style, StyleFn};

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(qr_code)
    }

    fn style(&self, class: &Self::Class<'_>) -> Style {
        class(self)
    }
}

pub fn qr_code(_theme: &Theme) -> Style {
    Style {
        background: iced::Color::WHITE,
        cell: iced::Color::BLACK,
    }
}
