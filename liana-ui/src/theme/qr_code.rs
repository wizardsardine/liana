use iced::{widget::container, Background, Border, Color};

use crate::theme::Theme;

/// The two colours a QR code is painted in.
///
/// Deliberately not the app palette: a scanner needs contrast, not a theme. This
/// is defined here rather than taken from iced because the QR codes are painted
/// as a raster rather than drawn with the `QRCode` widget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub background: Color,
    pub cell: Color,
}

pub fn qr_code(_theme: &Theme) -> Style {
    Style {
        background: Color::WHITE,
        cell: Color::BLACK,
    }
}

const CELL_RADIUS: f32 = 2.0;
/// The outline of a missing cell.
const CELL_BORDER: f32 = 2.0;

/// A cell of the frame grid whose frame was read or shown.
pub fn received_cell(theme: &Theme) -> container::Style {
    filled_cell(theme.colors.progress_bars.bar)
}

/// The frame read last, or the one on screen, so the user sees it move.
pub fn latest_cell(_theme: &Theme) -> container::Style {
    filled_cell(Color::WHITE)
}

/// A missing frame is only outlined, its inside left transparent.
pub fn missing_cell(_theme: &Theme) -> container::Style {
    container::Style {
        border: Border {
            color: Color::WHITE,
            width: CELL_BORDER,
            radius: CELL_RADIUS.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn filled_cell(color: Color) -> container::Style {
    container::Style {
        background: Some(Background::Color(color)),
        border: Border {
            radius: CELL_RADIUS.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}
