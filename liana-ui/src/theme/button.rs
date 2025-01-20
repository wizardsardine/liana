use iced::widget::button::{Catalog, Status, Style, StyleFn};
use iced::{Background, Border, Color};

use super::palette::Button;
use super::Theme;

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
    button(&theme.colors.buttons.primary, status)
}

pub fn secondary(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.secondary, status)
}

pub fn destructive(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.destructive, status)
}

pub fn container(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.container, status)
}

pub fn container_border(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.container_border, status)
}

pub fn menu(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.menu, status)
}

pub fn menu_pressed(theme: &Theme, _status: Status) -> Style {
    button(&theme.colors.buttons.menu, Status::Pressed)
}

pub fn transparent(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.transparent, status)
}

pub fn transparent_border(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.transparent_border, status)
}

fn button(p: &Button, status: Status) -> Style {
    match status {
        Status::Active => Style {
            background: Some(Background::Color(p.active.background)),
            text_color: p.active.text,
            border: if let Some(color) = p.active.border {
                Border {
                    radius: 25.0.into(),
                    width: 1.0,
                    color,
                }
            } else {
                Border {
                    ..Default::default()
                }
            },
            ..Default::default()
        },
        Status::Pressed => {
            if let Some(pressed) = p.pressed {
                Style {
                    background: Some(Background::Color(pressed.background)),
                    text_color: pressed.text,
                    border: if let Some(color) = pressed.border {
                        Border {
                            radius: 25.0.into(),
                            width: 1.0,
                            color,
                        }
                    } else {
                        Border {
                            ..Default::default()
                        }
                    },
                    ..Default::default()
                }
            } else {
                button(p, Status::Active)
            }
        }
        Status::Hovered => Style {
            background: Some(Background::Color(p.hovered.background)),
            text_color: p.hovered.text,
            border: if let Some(color) = p.hovered.border {
                Border {
                    radius: 25.0.into(),
                    width: 1.0,
                    color,
                }
            } else {
                Border {
                    ..Default::default()
                }
            },
            ..Default::default()
        },
        Status::Disabled => {
            if let Some(disabled) = p.disabled {
                Style {
                    background: Some(Background::Color(disabled.background)),
                    text_color: Color {
                        a: 0.2,
                        r: disabled.text.r,
                        g: disabled.text.g,
                        b: disabled.text.b,
                    },
                    border: if let Some(color) = disabled.border {
                        Border {
                            radius: 25.0.into(),
                            width: 1.0,
                            color,
                        }
                    } else {
                        Border {
                            ..Default::default()
                        }
                    },
                    ..Default::default()
                }
            } else {
                let active: Style = button(p, Status::Active);

                Style {
                    text_color: Color {
                        a: 0.2,
                        ..active.text_color
                    },
                    ..active
                }
            }
        }
    }
}
