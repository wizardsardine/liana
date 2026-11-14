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

pub fn link(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.link, status)
}

pub fn orange_outline(theme: &Theme, status: Status) -> Style {
    button(&theme.colors.buttons.orange_outline, status)
}

/// Bordered buttons take the Tenshu `.btn` corner (8px), not a pill: the
/// artboards keep pills only for wallet actions such as Send and Receive.
fn button(p: &Button, status: Status) -> Style {
    match status {
        Status::Active => palette_style(&p.active, p.active.text),
        Status::Pressed => {
            if let Some(pressed) = p.pressed {
                palette_style(&pressed, pressed.text)
            } else {
                button(p, Status::Active)
            }
        }
        Status::Hovered => palette_style(&p.hovered, p.hovered.text),
        Status::Disabled => {
            if let Some(disabled) = p.disabled {
                palette_style(
                    &disabled,
                    Color {
                        a: 0.2,
                        r: disabled.text.r,
                        g: disabled.text.g,
                        b: disabled.text.b,
                    },
                )
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

fn palette_style(palette: &super::palette::ButtonPalette, text_color: Color) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color,
        border: if let Some(color) = palette.border {
            Border {
                radius: super::tenshu::radius::CONTROL.into(),
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
}

pub fn tab(theme: &Theme, status: Status) -> Style {
    let mut style = button(&theme.colors.buttons.tab, status);
    style.border.radius = 0.0.into();
    style.border.width = 0.0;
    style
}

/// Text-first rows of the Tenshu sidebar and Cube switcher.
pub fn shell_nav(theme: &Theme, status: Status) -> Style {
    shell_nav_with_state(theme, status, false)
}

pub fn shell_nav_active(theme: &Theme, status: Status) -> Style {
    shell_nav_with_state(theme, status, true)
}

fn shell_nav_with_state(theme: &Theme, status: Status, selected: bool) -> Style {
    let colors = &theme.colors.shell;
    let background = if matches!(status, Status::Hovered | Status::Pressed) {
        colors.hover
    } else if selected {
        colors.selected
    } else {
        Color::TRANSPARENT
    };
    Style {
        background: Some(Background::Color(background)),
        text_color: if matches!(status, Status::Disabled) {
            colors.muted
        } else {
            colors.ink
        },
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Inactive primary-rail item. Square corners, no border, transparent
/// background with the standard menu text color — hover darkens the
/// background to the content-area tone to give click feedback.
pub fn rail(theme: &Theme, status: Status) -> Style {
    let mut style = button(&theme.colors.buttons.menu, status);
    style.border.radius = 0.0.into();
    style.border.width = 0.0;
    style
}

/// Active primary-rail item. Same square corners as `rail` but forced
/// into the pressed state so the active section reads as "selected"
/// even when not hovered.
pub fn rail_active(theme: &Theme, _status: Status) -> Style {
    let mut style = button(&theme.colors.buttons.menu, Status::Pressed);
    style.border.radius = 0.0.into();
    style.border.width = 0.0;
    style
}

/// Disabled rail item — network-gated features that are greyed out with
/// a hover popover instead of being hidden. Forces the disabled
/// styling (muted text + icon) regardless of hover so the item reads as
/// inert. Used by both the primary and secondary rails.
pub fn rail_disabled(theme: &Theme, _status: Status) -> Style {
    let mut style = button(&theme.colors.buttons.menu, Status::Disabled);
    style.border.radius = 0.0.into();
    style.border.width = 0.0;
    // The default disabled alpha (0.2) is nearly invisible on the dark
    // rail. Lift it so the greyed icon + label stay legible while still
    // reading clearly as inert next to the full-strength active items.
    style.text_color.a = 0.55;
    style
}

pub fn tab_liquid(theme: &Theme, _status: Status) -> Style {
    tab(theme, Status::Pressed)
}
