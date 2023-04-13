use iced::Font;

pub const BOLD: Font = Font::External {
    name: "Bold",
    bytes: include_bytes!("../static/fonts/IBMPlexSans-Bold.ttf"),
};

pub const MEDIUM: Font = Font::External {
    name: "Regular",
    bytes: include_bytes!("../static/fonts/IBMPlexSans-Medium.ttf"),
};

pub const REGULAR_BYTES: &[u8] = include_bytes!("../static/fonts/IBMPlexSans-Regular.ttf");

pub const REGULAR: Font = Font::External {
    name: "Regular",
    bytes: REGULAR_BYTES,
};
