use iced::widget::container::Style;
use iced::{Background, Border, Shadow, Vector};

use super::button::BUTTON_RADIUS;
use super::palette::ContainerPalette;
use super::styles;
use super::Theme;

pub const CARD_RADIUS: f32 = 16.0;

pub const CARD_SHADOW: Shadow = Shadow {
    color: crate::color::BLACK_15,
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
        snap: false,
    }
}

pub fn simple(theme: &Theme) -> Style {
    card_with_shadow(&theme.colors.cards.simple, false)
}

pub fn button_simple(theme: &Theme) -> Style {
    card_with_shadow(&theme.colors.cards.simple, true)
}

pub fn soft_warning(theme: &Theme) -> Style {
    let mut c = card(&theme.colors.cards.soft_warning);
    c.border.width = theme.button_border_width;
    c
}

pub fn info(theme: &Theme) -> Style {
    let mut c = card(&theme.colors.cards.info);
    c.border.width = theme.button_border_width;
    c
}

#[rustfmt::skip]
styles!(
    card,
    cards,
    [
        transparent,
        modal,
        border,
        invalid,
        legacy_warning,
        warning,
        error,
    ]
);
