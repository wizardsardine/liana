use iced::widget::container::Style;
use iced::{Background, Border, Color, Shadow, Vector};

use super::button::BUTTON_RADIUS;
use super::palette::ContainerPalette;
use super::Theme;

pub const CARD_RADIUS: f32 = 16.0;

pub const CARD_SHADOW: Shadow = Shadow {
    color: Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.15,
    },
    offset: Vector { x: 0.0, y: 4.0 },
    blur_radius: 4.0,
};

fn card(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: if let Some(color) = palette.border {
            Border {
                radius: CARD_RADIUS.into(),
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

fn card_with_shadow(palette: &ContainerPalette, btn: bool) -> Style {
    let radius = if btn { BUTTON_RADIUS } else { CARD_RADIUS }.into();
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: if let Some(color) = palette.border {
            Border {
                radius,
                width: 1.0,
                color,
            }
        } else {
            Border {
                radius,
                ..Default::default()
            }
        },
        shadow: CARD_SHADOW,
    }
}

pub fn simple(theme: &Theme) -> Style {
    card_with_shadow(&theme.colors.cards.simple, false)
}

pub fn button_simple(theme: &Theme) -> Style {
    card_with_shadow(&theme.colors.cards.simple, true)
}

pub fn transparent(theme: &Theme) -> Style {
    let palette = &theme.colors.cards.transparent;
    card(palette)
}

pub fn modal(theme: &Theme) -> Style {
    card(&theme.colors.cards.modal)
}

pub fn border(theme: &Theme) -> Style {
    card(&theme.colors.cards.border)
}

pub fn invalid(theme: &Theme) -> Style {
    card(&theme.colors.cards.invalid)
}

pub fn warning(theme: &Theme) -> Style {
    card(&theme.colors.cards.warning)
}

pub fn warning_banner(theme: &Theme) -> Style {
    card(&theme.colors.cards.warning_banner)
}

pub fn home_hint(theme: &Theme) -> Style {
    card(&theme.colors.cards.home_hint)
}

pub fn error(theme: &Theme) -> Style {
    card(&theme.colors.cards.error)
}
