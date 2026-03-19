use iced::Color;

pub const BLACK: Color = iced::Color::BLACK;
pub const LIGHT_BLACK: Color = iced::color!(0x161716);
pub const GREY_7: Color = iced::color!(0x3F3F3F);
pub const GREY_6: Color = iced::color!(0x202020);
pub const GREY_5: Color = iced::color!(0x272727);
pub const GREY_4: Color = iced::color!(0x424242);
pub const GREY_3: Color = iced::color!(0x717171);
pub const GREY_2: Color = iced::color!(0xCCCCCC);
pub const GREY_1: Color = iced::color!(0xE6E6E6);

// Toast color palette
pub const GREEN_TOAST: Color = iced::color!(0x2D6A4F);  // Success toast dark green (6.39:1 with white)
pub const DARK_ORANGE: Color = iced::color!(0xB55600);  // Warning toast darker orange (4.89:1 with white)
pub const LIGHT_ORANGE: Color = iced::color!(0xD35400); // Info toast darker orange (4.9:1 with white)
pub const RED_ERROR: Color = iced::color!(0xC0392B);    // Error toast darker red (5.44:1 with white)
pub const GREY_6_DARKER: Color = iced::color!(0x161716); // Darker grey for Debug/Trace hover
pub const GREY_4_DARKER: Color = iced::color!(0x353535); // Darker grey for Debug/Trace hover
pub const GREY_3_DARKER: Color = iced::color!(0x5A5A5A); // Darker grey for Debug/Trace hover

pub const WHITE: Color = iced::Color::WHITE;
pub const TRANSPARENT: Color = iced::Color::TRANSPARENT;

// Legacy colors (keep for backward compatibility)
pub const RED: Color = iced::color!(0xE24E1B);

pub const GREEN: Color = iced::color!(0x00FF66);
pub const TRANSPARENT_GREEN: Color = iced::color!(0x00FF66, 0.3);

pub const ORANGE: Color = iced::color!(0xF7931B);
pub const TRANSPARENT_ORANGE: Color = iced::color!(0xF7931B, 0.3);

pub const BLUE: Color = iced::color!(0x7DD3FC);
