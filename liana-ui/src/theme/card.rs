use iced::widget::container::Style;
use iced::{Background, Border, Shadow, Vector};

use super::button::BUTTON_RADIUS;
use super::palette::ContainerPalette;
use super::styles;
use super::Theme;

pub const CARD_RADIUS: f32 = 16.0;
const NOTE_CARD_RADIUS: f32 = 12.0;

pub const CARD_SHADOW: Shadow = Shadow {
    color: crate::color::BLACK_15,
    offset: Vector { x: 0.0, y: 4.0 },
    blur_radius: 4.0,
};

fn card(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: Border {
            radius: CARD_RADIUS.into(),
            width: palette.border.map(|_| 1.0).unwrap_or_default(),
            color: palette.border.unwrap_or_default(),
        },
        ..Default::default()
    }
}

fn card_note(palette: &ContainerPalette) -> Style {
    Style {
        background: Some(Background::Color(palette.background)),
        text_color: palette.text,
        border: Border {
            radius: NOTE_CARD_RADIUS.into(),
            width: palette.border.map(|_| 1.0).unwrap_or_default(),
            color: palette.border.unwrap_or_default(),
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

fn note_or_callout(theme: &Theme, palette: &ContainerPalette) -> Style {
    if theme.is_business() {
        card_note(palette)
    } else {
        let mut c = card(palette);
        c.border.width = theme.button_border_width;
        c
    }
}

pub fn soft_warning(theme: &Theme) -> Style {
    note_or_callout(theme, &theme.colors.cards.soft_warning)
}

pub fn info(theme: &Theme) -> Style {
    note_or_callout(theme, &theme.colors.cards.info)
}

pub fn success(theme: &Theme) -> Style {
    match theme.colors.cards.success {
        Some(palette) => card_note(&palette),
        None => card_with_shadow(&theme.colors.cards.simple, false),
    }
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
        section,
        flat,
    ]
);
