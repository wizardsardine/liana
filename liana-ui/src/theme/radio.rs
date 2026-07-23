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

pub fn primary(theme: &Theme, status: Status) -> Style {
    if !theme.is_business() {
        return Style {
            dot_color: theme.colors.radio_buttons.dot,
            text_color: theme.colors.radio_buttons.text.into(),
            background: theme.colors.cards.simple.background.into(),
            border_width: 1.0,
            border_color: theme.colors.radio_buttons.border,
        };
    }

    let is_selected = match status {
        Status::Active { is_selected } | Status::Hovered { is_selected } => is_selected,
    };
    let p = &theme.colors.radio_buttons;
    if is_selected {
        Style {
            dot_color: crate::color::WHITE,
            text_color: p.text.into(),
            background: p.dot.into(),
            border_width: 0.0,
            border_color: crate::color::TRANSPARENT,
        }
    } else {
        Style {
            dot_color: p.dot,
            text_color: p.text.into(),
            background: crate::color::TRANSPARENT.into(),
            border_width: 1.0,
            border_color: p.border,
        }
    }
}
