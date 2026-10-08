use iced::widget::overlay::menu::Style as MenuStyle;
use iced::{
    widget::{
        button, container,
        pick_list::{Catalog, Status, Style, StyleFn},
    },
    Background, Border, Shadow,
};

use super::{card::CARD_SHADOW_HOVER, palette::Menu, Theme};

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
        Status::Opened { .. } => theme.colors.buttons.pick_list.hovered,
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
                ..Default::default()
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

/// Pick list menu look for a container, with the hover shadow.
pub fn menu_container(theme: &Theme) -> container::Style {
    let menu = theme.colors.menus.pick_list;
    container::Style {
        background: Some(Background::Color(menu.background)),
        border: Border {
            color: menu.border,
            width: 1.0,
            radius: PICK_LIST_RADIUS.into(),
            ..Default::default()
        },
        shadow: CARD_SHADOW_HOVER,
        ..Default::default()
    }
}

/// Pick list option look for a button.
pub fn option(theme: &Theme, status: button::Status) -> button::Style {
    let menu = theme.colors.menus.pick_list;
    match status {
        button::Status::Active | button::Status::Disabled => button::Style {
            text_color: menu.text,
            ..Default::default()
        },
        button::Status::Hovered | button::Status::Pressed => button::Style {
            background: Some(Background::Color(menu.selected_background)),
            text_color: menu.selected_text,
            ..Default::default()
        },
    }
}

impl From<Menu> for MenuStyle {
    fn from(value: Menu) -> Self {
        MenuStyle {
            background: value.background.into(),
            border: iced::Border {
                color: value.border,
                width: 1.0,
                radius: PICK_LIST_RADIUS.into(),
                ..Default::default()
            },
            text_color: value.text,
            selected_text_color: value.selected_text,
            selected_background: value.selected_background.into(),
            shadow: Shadow::default(),
        }
    }
}
