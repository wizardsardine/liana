use iced::widget::overlay::menu::Style as MenuStyle;
use iced::{
    widget::pick_list::{Catalog, Status, Style, StyleFn},
    Border,
};

use super::palette::Menu;
use super::Theme;

const PICK_LIST_RADIUS: f32 = 4.0;

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
    let style = match status {
        Status::Active => theme.colors.buttons.pick_list.active,
        Status::Hovered => theme.colors.buttons.pick_list.hovered,
        Status::Opened => theme.colors.buttons.pick_list.hovered,
    };
    Style {
        text_color: style.text,
        placeholder_color: style.text,
        background: style.background.into(),
        border: if let Some(color) = style.border {
            Border {
                radius: PICK_LIST_RADIUS.into(),
                width: 1.0,
                color,
            }
        } else {
            Border {
                ..Default::default()
            }
        },
        handle_color: style.text,
    }
}

pub fn menu(theme: &Theme) -> MenuStyle {
    theme.colors.menus.pick_list.into()
}

impl From<Menu> for MenuStyle {
    fn from(value: Menu) -> Self {
        MenuStyle {
            background: value.background.into(),
            border: iced::Border {
                color: value.border,
                width: 1.0,
                radius: PICK_LIST_RADIUS.into(),
            },
            text_color: value.text,
            selected_text_color: value.selected_text,
            selected_background: value.selected_background.into(),
        }
    }
}
