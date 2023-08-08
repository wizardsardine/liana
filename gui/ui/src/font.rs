use iced::{
    font::{Family, Stretch, Weight},
    Command, Font,
};

pub const BOLD: Font = Font {
    family: Family::Name("IBM Plex Sans"),
    weight: Weight::Bold,
    monospaced: false,
    stretch: Stretch::Normal,
};

pub const MEDIUM: Font = Font {
    family: Family::Name("IBM Plex Sans"),
    weight: Weight::Medium,
    monospaced: false,
    stretch: Stretch::Normal,
};

pub const REGULAR: Font = Font::with_name("IBM Plex Sans");

pub const BOLD_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Bold.ttf");
pub const MEDIUM_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Medium.ttf");
pub const REGULAR_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Regular.ttf");

pub const ICONEX_ICONS_BYTES: &[u8] = include_bytes!("../static/icons/iconex/iconex-icons.ttf");
pub const BOOTSTRAP_ICONS_BYTE: &[u8] = include_bytes!("../static/icons/bootstrap-icons.ttf");

pub fn loads<T: From<Result<(), iced::font::Error>> + 'static>() -> Vec<Command<T>> {
    vec![
        iced::font::load(BOLD_BYTES).map(T::from),
        iced::font::load(MEDIUM_BYTES).map(T::from),
        iced::font::load(REGULAR_BYTES).map(T::from),
        iced::font::load(ICONEX_ICONS_BYTES).map(T::from),
        iced::font::load(BOOTSTRAP_ICONS_BYTE).map(T::from),
    ]
}
