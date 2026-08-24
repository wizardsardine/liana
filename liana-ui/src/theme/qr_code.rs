use iced::Color;

use super::Theme;

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
