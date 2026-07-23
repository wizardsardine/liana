use iced::border::Dash;
use iced::widget::button::{Catalog, Status, Style, StyleFn};
use iced::{Background, Border, Color};

use super::{card::CARD_RADIUS, palette::Button, Theme};

pub const BUTTON_RADIUS: f32 = 12.0;
/// On/off length of the auxiliary button's dashed border, in logical pixels.
const AUXILIARY_DASH: f32 = 6.0;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(primary)
    }

    fn style(&self, class: &Self::Class<'_>, status: Status) -> Style {
        class(self, status)
    }
}

macro_rules! button_styles {
    ($($name:ident),* $(,)?) => {
        $(
            pub fn $name(theme: &Theme, status: Status) -> Style {
                button(&theme.colors.buttons.$name, status, theme.button_border_width)
            }
        )*
    };
}

button_styles!(
    primary,
    secondary,
    feerate,
    feerate_unselected,
    tertiary,
    destructive,
    container,
    container_border,
    menu,
    transparent,
    remove,
    transparent_border,
    clickable_section,
    link,
    link_subtle,
    signing_devices,
    optional_section,
);

pub fn auxiliary(theme: &Theme, status: Status) -> Style {
    let mut style = button(&theme.colors.buttons.auxiliary, status, 1.0);
    style.border.radius = CARD_RADIUS.into();
    style.border = style
        .border
        .dashes(Dash::new(AUXILIARY_DASH, AUXILIARY_DASH));
    style
}

pub fn list_entry(theme: &Theme, status: Status) -> Style {
    let mut style = button(
        &theme.colors.buttons.list_entry,
        status,
        theme.button_border_width,
    );
    if let Some(radius) = theme.colors.buttons.list_entry_radius {
        style.border.radius = radius.into();
    }
    if status == Status::Hovered {
        if let Some(width) = theme.colors.buttons.list_entry_hover_border_width {
            style.border.width = width;
        }
    }
    style
}

pub fn tab_menu(theme: &Theme, status: Status) -> Style {
    let mut style = button(
        &theme.colors.buttons.tab_menu,
        status,
        theme.button_border_width,
    );
    style.border.radius = 3.0.into();
    style.border.width = 1.0;
    style
}

pub fn tab_menu_bottom(theme: &Theme, status: Status) -> Style {
    let mut style = button(
        &theme.colors.buttons.tab_menu,
        status,
        theme.button_border_width,
    );
    style.border.radius = iced::border::Radius {
        top_left: 3.0,
        top_right: 3.0,
        bottom_right: 6.0,
        bottom_left: 6.0,
    };
    style.border.width = 1.0;
    style
}

pub fn signing_devices_non_clickable(theme: &Theme, _status: Status) -> Style {
    button(
        &theme.colors.buttons.signing_devices,
        Status::Active,
        theme.button_border_width,
    )
}

pub fn menu_pressed(theme: &Theme, _status: Status) -> Style {
    button(
        &theme.colors.buttons.menu,
        Status::Pressed,
        theme.button_border_width,
    )
}

pub fn transparent_primary_text(theme: &Theme, status: Status) -> Style {
    let mut style = button(
        &theme.colors.buttons.transparent,
        status,
        theme.button_border_width,
    );
    style.text_color = theme.colors.text.primary;
    style
}

pub fn breadcrumb(theme: &Theme, _status: Status) -> Style {
    let mut style = button(
        &theme.colors.buttons.transparent,
        Status::Active,
        theme.button_border_width,
    );
    style.text_color = theme.colors.text.primary;
    style
}

fn round_button(p: &Button, status: Status, width: f32, radius: f32) -> Style {
    let mut btn = button(p, status, width);
    btn.border.radius = radius.into();
    btn
}

pub fn round_icon_btn(theme: &Theme, status: Status, radius: f32) -> Style {
    round_button(
        &theme.colors.buttons.list_entry,
        status,
        theme.button_border_width,
        radius,
    )
}

fn button(p: &Button, status: Status, width: f32) -> Style {
    match status {
        Status::Active => Style {
            background: Some(Background::Color(p.active.background)),
            text_color: p.active.text,
            border: if let Some(color) = p.active.border {
                Border {
                    radius: BUTTON_RADIUS.into(),
                    width,
                    color,
                    ..Default::default()
                }
            } else {
                Border {
                    ..Default::default()
                }
            },
            shadow: p.active.shadow,
            snap: false,
        },
        Status::Pressed => {
            if let Some(pressed) = p.pressed {
                Style {
                    background: Some(Background::Color(pressed.background)),
                    text_color: pressed.text,
                    border: if let Some(color) = pressed.border {
                        Border {
                            radius: BUTTON_RADIUS.into(),
                            width,
                            color,
                            ..Default::default()
                        }
                    } else {
                        Border {
                            ..Default::default()
                        }
                    },
                    shadow: pressed.shadow,
                    snap: false,
                }
            } else {
                button(p, Status::Active, width)
            }
        }
        Status::Hovered => Style {
            background: Some(Background::Color(p.hovered.background)),
            text_color: p.hovered.text,
            border: if let Some(color) = p.hovered.border {
                Border {
                    radius: BUTTON_RADIUS.into(),
                    width,
                    color,
                    ..Default::default()
                }
            } else {
                Border {
                    ..Default::default()
                }
            },
            shadow: p.hovered.shadow,
            snap: false,
        },
        Status::Disabled => {
            if let Some(disabled) = p.disabled {
                Style {
                    background: Some(Background::Color(disabled.background)),
                    text_color: disabled.text,
                    border: if let Some(color) = disabled.border {
                        Border {
                            radius: BUTTON_RADIUS.into(),
                            width,
                            color,
                            ..Default::default()
                        }
                    } else {
                        Border {
                            ..Default::default()
                        }
                    },
                    shadow: disabled.shadow,
                    snap: false,
                }
            } else {
                let active: Style = button(p, Status::Active, width);

                Style {
                    text_color: Color {
                        a: 0.5,
                        ..active.text_color
                    },
                    ..active
                }
            }
        }
    }
}

pub fn tab(theme: &Theme, status: Status) -> Style {
    let mut style = button(&theme.colors.buttons.tab, status, theme.button_border_width);
    style.border.radius = 0.0.into();
    style.border.width = 0.0;
    style
}

pub fn tab_active(theme: &Theme, _status: Status) -> Style {
    tab(theme, Status::Pressed)
}
