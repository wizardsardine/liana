use iced::{
    widget::{
        combo_box::Catalog,
        container,
        overlay::menu::{Style as MenuStyle, StyleFn as MenuStyleFn},
        text_input::{Status as InputStatus, Style as InputStyle, StyleFn as InputStyleFn},
    },
    Border, Shadow, Vector,
};

use super::Theme;
use crate::color;

const MENU_SHADOW: Shadow = Shadow {
    color: color::BLACK_15,
    offset: Vector { x: 0.0, y: 4.0 },
    blur_radius: 10.0,
};

impl Catalog for Theme {
    fn default_input<'a>() -> InputStyleFn<'a, Self> {
        Box::new(input)
    }

    fn default_menu<'a>() -> MenuStyleFn<'a, Self> {
        Box::new(menu)
    }
}

pub fn input(theme: &Theme, status: InputStatus) -> InputStyle {
    let form = super::text_input::form(theme, status);

    InputStyle {
        value: theme.colors.text.primary,
        icon: theme.colors.text.primary,
        background: color::TRANSPARENT.into(),
        border: Border::default(),
        ..form
    }
}

pub fn field(theme: &Theme) -> container::Style {
    let form = super::text_input::form(theme, InputStatus::Active);

    container::Style {
        background: Some(form.background),
        border: form.border,
        ..Default::default()
    }
}

pub fn menu(theme: &Theme) -> MenuStyle {
    MenuStyle {
        selected_background: theme.colors.combobox.selected.into(),
        shadow: MENU_SHADOW,
        ..super::pick_list::menu(theme)
    }
}
