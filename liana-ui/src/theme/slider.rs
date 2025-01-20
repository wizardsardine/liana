use iced::{
    widget::slider::{Catalog, Handle, HandleShape, Rail, Status, Style, StyleFn},
    Border,
};

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(slider)
    }

    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style {
        class(self, status)
    }
}

pub fn slider(theme: &Theme, _status: Status) -> Style {
    Style {
        rail: Rail {
            backgrounds: (
                theme.colors.sliders.rail_backgrounds.0.into(),
                theme.colors.sliders.rail_backgrounds.1.into(),
            ),
            border: if let Some(color) = theme.colors.sliders.rail_border {
                Border {
                    color,
                    width: 1.0,
                    radius: 4.0.into(),
                }
            } else {
                Border {
                    ..Default::default()
                }
            },
            width: 2.0,
        },
        handle: Handle {
            shape: HandleShape::Rectangle {
                width: 8,
                border_radius: 4.0.into(),
            },
            background: theme.colors.sliders.background.into(),
            border_color: theme.colors.sliders.border,
            border_width: 1.0,
        },
    }
}
