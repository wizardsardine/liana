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

pub const SEMIBOLD: Font = Font {
    family: Family::Name("IBM Plex Sans"),
    weight: Weight::Semibold,
    style: iced::font::Style::Normal,
    stretch: Stretch::Normal,
};

pub const MONO_REGULAR: Font = Font::with_name("IBM Plex Mono");

pub const MONO_MEDIUM: Font = Font {
    family: Family::Name("IBM Plex Mono"),
    weight: Weight::Medium,
    style: iced::font::Style::Normal,
    stretch: Stretch::Normal,
};

/// IBM Plex Sans 400 italic (`.say { font-style: italic }`), for voice lines.
pub const ITALIC: Font = Font {
    family: Family::Name("IBM Plex Sans"),
    weight: Weight::Normal,
    style: iced::font::Style::Italic,
    stretch: Stretch::Normal,
};

pub const REGULAR: Font = Font::with_name("IBM Plex Sans");

pub const SPACE_GROTESK_BOLD: Font = Font {
    family: Family::Name("Space Grotesk"),
    weight: Weight::Bold,
    style: iced::font::Style::Normal,
    stretch: Stretch::Normal,
};

pub const BOLD_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Bold.ttf");
pub const MEDIUM_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Medium.ttf");
pub const SEMIBOLD_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-SemiBold.ttf");
pub const REGULAR_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Regular.ttf");
pub const ITALIC_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Italic.ttf");
pub const MONO_REGULAR_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexMono-Regular.ttf");
pub const MONO_MEDIUM_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexMono-Medium.ttf");

pub const SPACE_GROTESK_BOLD_BYTES: &[u8] = include_bytes!("../static/fonts/SpaceGrotesk-Bold.ttf");

pub const BOOTSTRAP_ICONS_BYTE: &[u8] = include_bytes!("../static/icons/bootstrap-icons.ttf");
/// Marks Bootstrap Icons lacks, built by `static/icons/build_coincube_icons.py`.
pub const COINCUBE_ICONS_BYTES: &[u8] = include_bytes!("../static/icons/coincube-icons.ttf");

pub fn load() -> Vec<Cow<'static, [u8]>> {
    vec![
        BOLD_BYTES.into(),
        MEDIUM_BYTES.into(),
        SEMIBOLD_BYTES.into(),
        REGULAR_BYTES.into(),
        ITALIC_BYTES.into(),
        MONO_REGULAR_BYTES.into(),
        MONO_MEDIUM_BYTES.into(),
        SPACE_GROTESK_BOLD_BYTES.into(),
        BOOTSTRAP_ICONS_BYTE.into(),
        COINCUBE_ICONS_BYTES.into(),
        // DatePicker navigation, cancel, and confirm use iced_aw's icon font.
        iced_aw::ICED_AW_FONT_BYTES.into(),
    ]
}
