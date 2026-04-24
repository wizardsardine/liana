use iced::widget::button::{Catalog, Status, Style, StyleFn};
use iced::{Background, Border, Color};

use super::palette::Button;
use super::Theme;

pub const BUTTON_RADIUS: f32 = 16.0;

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
    button(
        &theme.colors.buttons.primary,
        status,
        theme.button_border_width,
    )
}

pub fn secondary(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.secondary,
        status,
        theme.button_border_width,
    )
}

pub fn tertiary(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.tertiary,
        status,
        theme.button_border_width,
    )
}

pub fn destructive(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.destructive,
        status,
        theme.button_border_width,
    )
}

pub fn container(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.container,
        status,
        theme.button_border_width,
    )
}

pub fn container_border(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.container_border,
        status,
        theme.button_border_width,
    )
}

pub fn menu(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.menu,
        status,
        theme.button_border_width,
    )
}

pub fn tab_menu(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.tab_menu,
        status,
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

pub fn transparent(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.transparent,
        status,
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

pub fn transparent_border(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.transparent_border,
        status,
        theme.button_border_width,
    )
}

pub fn clickable_card(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.clickable_card,
        status,
        theme.button_border_width,
    )
}

pub fn link(theme: &Theme, status: Status) -> Style {
    button(
        &theme.colors.buttons.link,
        status,
        theme.button_border_width,
    )
}

fn round_button(p: &Button, status: Status, width: f32, radius: f32) -> Style {
    let mut btn = button(p, status, width);
    btn.border.radius = radius.into();
    btn
}

pub fn round_icon_btn(theme: &Theme, status: Status, radius: f32) -> Style {
    round_button(
        &theme.colors.buttons.clickable_card,
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
                    text_color: Color {
                        a: 0.5,
                        r: disabled.text.r,
                        g: disabled.text.g,
                        b: disabled.text.b,
                    },
                    border: if let Some(color) = disabled.border {
                        Border {
                            radius: BUTTON_RADIUS.into(),
                            width,
                            color,
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
