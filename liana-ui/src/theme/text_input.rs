use iced::{
    widget::text_input::{Catalog, Status, Style, StyleFn},
    Background, Border,
};

use super::{palette::TextInput, Theme};

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(primary)
    }

    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style {
        class(self, status)
    }
}

pub fn primary(theme: &Theme, status: Status) -> Style {
    text_input(&theme.colors.text_inputs.primary, status)
}

pub fn invalid(theme: &Theme, status: Status) -> Style {
    text_input(&theme.colors.text_inputs.invalid, status)
}

fn text_input(c: &TextInput, status: Status) -> Style {
    let active = Style {
        background: Background::Color(c.active.background),
        border: if let Some(color) = c.active.border {
            Border {
                radius: 25.0.into(),
                width: 1.0,
                color,
            }
        } else {
            Border::default()
        },
        icon: c.active.icon,
        placeholder: c.active.placeholder,
        value: c.active.value,
        selection: c.active.selection,
    };

    match status {
        Status::Active | Status::Hovered | Status::Focused { .. } => active,
        Status::Disabled => Style {
            background: Background::Color(c.disabled.background),
            border: if let Some(color) = c.disabled.border {
                Border {
                    radius: 25.0.into(),
                    width: 1.0,
                    color,
                }
            } else {
                Border::default()
            },
            icon: c.disabled.icon,
            placeholder: c.disabled.placeholder,
            value: c.disabled.value,
            selection: c.disabled.selection,
        },
    }
}
