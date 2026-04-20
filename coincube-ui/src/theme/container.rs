use iced::widget::container::{transparent, Catalog, Style, StyleFn};
use iced::Background;

use super::Theme;

impl Catalog for Theme {
    type Class<'a> = StyleFn<'a, Self>;

    fn default<'a>() -> Self::Class<'a> {
        Box::new(transparent)
    }

    fn style(&self, class: &Self::Class<'_>) -> Style {
        class(self)
    }
}

pub fn background(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        ..Default::default()
    }
}

#[allow(unused)]
pub fn debug(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(iced::Color::WHITE)),
        border: iced::Border::default().color(iced::color!(0xFF0000)),
        ..Default::default()
    }
}

pub fn foreground(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.foreground)),
        ..Default::default()
    }
}

pub fn foreground_rounded(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.foreground)),
        border: iced::Border {
            radius: 25.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn border(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        ..Default::default()
    }
}

pub fn custom(color: iced::Color) -> Box<dyn Fn(&Theme) -> Style> {
    Box::new(move |_theme: &Theme| Style {
        background: Some(Background::Color(color)),
        ..Default::default()
    })
}

/// Balance header card: card background with orange border and rounded corners.
pub fn balance_header(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.cards.simple.background)),
        border: iced::Border {
            color: crate::color::ORANGE,
            width: 0.2,
            radius: 25.0.into(),
        },
        ..Default::default()
    }
}

pub fn border_grey(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        border: iced::Border {
            color: theme.colors.text.secondary,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// Primary (72px) left rail — darker tone so it visually reads as the
/// app's navigation spine, distinct from the content area.
pub fn sidebar_primary(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.foreground)),
        ..Default::default()
    }
}

/// Secondary (188px) left rail — same tone as the content area so
/// the secondary rail blends with content visually.
pub fn sidebar_secondary(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.general.background)),
        ..Default::default()
    }
}

/// Tertiary (~72px) left rail — appears only when the current route
/// has a third level. A step lighter than the primary/secondary rails
/// (uses the card background) so the deeper nav level reads as
/// slightly elevated without breaking the dark palette.
pub fn sidebar_tertiary(theme: &Theme) -> Style {
    Style {
        background: Some(Background::Color(theme.colors.cards.simple.background)),
        ..Default::default()
    }
}
