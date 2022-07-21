use iced::Font;

pub const BOLD: Font = Font::External {
    name: "Bold",
    bytes: include_bytes!("../../static/fonts/OpenSans-Bold.ttf"),
};

pub const REGULAR: Font = Font::External {
    name: "Regular",
    bytes: include_bytes!("../../static/fonts/OpenSans-Regular.ttf"),
};
