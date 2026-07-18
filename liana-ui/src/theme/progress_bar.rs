use iced::{
    widget::progress_bar::{Catalog, Style, StyleFn},
    Border,
};

use super::Theme;

const PROGRESS_BAR_RADIUS: f32 = 25.0;

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
        background: theme.colors.progress_bars.background.into(),
        bar: theme.colors.progress_bars.bar.into(),
        border: if let Some(color) = theme.colors.cards.simple.border {
            Border {
                radius: PROGRESS_BAR_RADIUS.into(),
                width: 1.0,
                color,
                ..Default::default()
            }
        } else {
            Border {
                ..Default::default()
            }
        },
    }
}

pub fn error(theme: &Theme) -> Style {
    Style {
        background: crate::color::TRANSPARENT.into(),
        bar: theme.colors.text.error.into(),
        border: Border {
            radius: PROGRESS_BAR_RADIUS.into(),
            ..Default::default()
        },
    }
}
