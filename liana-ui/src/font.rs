use iced::{
    font::{Family, Stretch, Weight},
    Font,
};
use std::borrow::Cow;

pub const BOLD: Font = Font {
    family: Family::Name("IBM Plex Sans"),
    weight: Weight::Bold,
    style: iced::font::Style::Normal,
    stretch: Stretch::Normal,
};

pub const MEDIUM: Font = Font {
    family: Family::Name("IBM Plex Sans"),
    weight: Weight::Medium,
    style: iced::font::Style::Normal,
    stretch: Stretch::Normal,
};

pub const REGULAR: Font = Font::with_name("IBM Plex Sans");

pub const MANROPE_BOLD: Font = Font {
    family: Family::Name("Manrope"),
    weight: Weight::Bold,
    style: iced::font::Style::Normal,
    stretch: Stretch::Normal,
};

pub const MANROPE_MEDIUM: Font = Font {
    family: Family::Name("Manrope"),
    weight: Weight::Medium,
    style: iced::font::Style::Normal,
    stretch: Stretch::Normal,
};

pub const MANROPE_REGULAR: Font = Font::with_name("Manrope");

pub const IBM_BOLD_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Bold.ttf");
pub const IBM_MEDIUM_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Medium.ttf");
pub const IBM_REGULAR_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Regular.ttf");

pub const MANROPE_REGULAR_BYTES: &[u8] = include_bytes!("../static/fonts/Manrope-Regular.ttf");
pub const MANROPE_MEDIUM_BYTES: &[u8] = include_bytes!("../static/fonts/Manrope-Medium.ttf");
pub const MANROPE_BOLD_BYTES: &[u8] = include_bytes!("../static/fonts/Manrope-Bold.ttf");

pub const ICONEX_ICONS_BYTES: &[u8] = include_bytes!("../static/icons/iconex/iconex-icons.ttf");
pub const BOOTSTRAP_ICONS_BYTE: &[u8] = include_bytes!("../static/icons/bootstrap-icons.ttf");

pub fn load() -> Vec<Cow<'static, [u8]>> {
    vec![
        IBM_BOLD_BYTES.into(),
        IBM_MEDIUM_BYTES.into(),
        IBM_REGULAR_BYTES.into(),
        MANROPE_REGULAR_BYTES.into(),
        MANROPE_MEDIUM_BYTES.into(),
        MANROPE_BOLD_BYTES.into(),
        ICONEX_ICONS_BYTES.into(),
        BOOTSTRAP_ICONS_BYTE.into(),
    ]
}
