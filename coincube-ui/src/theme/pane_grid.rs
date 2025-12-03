use iced::widget::container;
use iced::widget::pane_grid::{Catalog, Highlight, Line, Style, StyleFn};
use iced::Border;

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
        hovered_region: Highlight {
            background: theme.colors.pane_grid.highlight_background.into(),
            border: Border {
                color: theme.colors.pane_grid.highlight_border,
                width: 1.0,
                radius: 0.0.into(),
            },
        },
        picked_split: Line {
            color: theme.colors.pane_grid.picked_split,
            width: 2.0,
        },
        hovered_split: Line {
            color: theme.colors.pane_grid.hovered_split,
            width: 2.0,
        },
    }
}

pub fn pane_grid_background(theme: &Theme) -> container::Style {
    container::Style {
        background: Some(theme.colors.pane_grid.background.into()),
        ..Default::default()
    }
}
