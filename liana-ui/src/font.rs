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

pub const BOLD_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Bold.ttf");
pub const MEDIUM_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Medium.ttf");
pub const REGULAR_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Regular.ttf");

pub const ICONEX_ICONS_BYTES: &[u8] = include_bytes!("../static/icons/iconex/iconex-icons.ttf");
pub const BOOTSTRAP_ICONS_BYTE: &[u8] = include_bytes!("../static/icons/bootstrap-icons.ttf");

pub fn load() -> Vec<Cow<'static, [u8]>> {
    vec![
        BOLD_BYTES.into(),
        MEDIUM_BYTES.into(),
        REGULAR_BYTES.into(),
        ICONEX_ICONS_BYTES.into(),
        BOOTSTRAP_ICONS_BYTE.into(),
    ]
}
