use iced::Color;
pub const BLACK: Color = iced::Color::BLACK;
pub const TRANSPARENT: Color = iced::Color::TRANSPARENT;
pub const LIGHT_BLACK: Color = Color::from_rgb(
    0x14 as f32 / 255.0,
    0x14 as f32 / 255.0,
    0x14 as f32 / 255.0,
);
pub const GREY_7: Color = Color::from_rgb(
    0x3F as f32 / 255.0,
    0x3F as f32 / 255.0,
    0x3F as f32 / 255.0,
);
pub const GREY_6: Color = Color::from_rgb(
    0x20 as f32 / 255.0,
    0x20 as f32 / 255.0,
    0x20 as f32 / 255.0,
);
pub const GREY_5: Color = Color::from_rgb(
    0x27 as f32 / 255.0,
    0x27 as f32 / 255.0,
    0x27 as f32 / 255.0,
);
pub const GREY_4: Color = Color::from_rgb(
    0x42 as f32 / 255.0,
    0x42 as f32 / 255.0,
    0x42 as f32 / 255.0,
);
pub const GREY_3: Color = Color::from_rgb(
    0x71 as f32 / 255.0,
    0x71 as f32 / 255.0,
    0x71 as f32 / 255.0,
);
pub const GREY_2: Color = Color::from_rgb(
    0xCC as f32 / 255.0,
    0xCC as f32 / 255.0,
    0xCC as f32 / 255.0,
);
pub const GREY_1: Color = Color::from_rgb(
    0xE6 as f32 / 255.0,
    0xE6 as f32 / 255.0,
    0xE6 as f32 / 255.0,
);
pub const WHITE: Color = iced::Color::WHITE;
pub const GREEN: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0xFF as f32 / 255.0,
    0x66 as f32 / 255.0,
);
pub const TRANSPARENT_GREEN: Color = Color::from_rgba(
    0x00 as f32 / 255.0,
    0xFF as f32 / 255.0,
    0x66 as f32 / 255.0,
    0.3,
);
pub const RED: Color = Color::from_rgb(
    0xE2 as f32 / 255.0,
    0x4E as f32 / 255.0,
    0x1B as f32 / 255.0,
);

pub const ORANGE: Color =
    Color::from_rgb(0xFF as f32 / 255.0, 0xa7 as f32 / 255.0, 0x0 as f32 / 255.0);

pub const BLUE: Color = Color::from_rgb(
    0x7D as f32 / 255.0,
    0xD3 as f32 / 255.0,
    0xFC as f32 / 255.0,
);

// =============================================================================
// BUSINESS THEME COLORS (Light Mode with Cyan-Blue accent)
// =============================================================================

// Primary accent: Cyan-Blue from lianawallet.com/business (HSL 196, 100%, 50%)
pub const BUSINESS_BLUE: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0xBF as f32 / 255.0,
    0xFF as f32 / 255.0,
); // #00BFFF

// Darker variant for hover states (HSL 196, 100%, 40%)
pub const BUSINESS_BLUE_DARK: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0x99 as f32 / 255.0,
    0xCC as f32 / 255.0,
); // #0099CC

// Transparent variant for highlights
pub const TRANSPARENT_BUSINESS_BLUE: Color = Color::from_rgba(
    0x00 as f32 / 255.0,
    0xBF as f32 / 255.0,
    0xFF as f32 / 255.0,
    0.15,
);

// Light blue tint for secondary button backgrounds
pub const LIGHT_BLUE_TINT: Color = Color::from_rgb(
    0xE5 as f32 / 255.0,
    0xF5 as f32 / 255.0,
    0xFF as f32 / 255.0,
); // #E5F5FF

// Soft blue for secondary button borders
pub const SOFT_BLUE: Color = Color::from_rgb(
    0x66 as f32 / 255.0,
    0xD4 as f32 / 255.0,
    0xFF as f32 / 255.0,
); // #66D4FF

// Dark green for success text on light backgrounds
pub const DARK_GREEN: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0x7A as f32 / 255.0,
    0x33 as f32 / 255.0,
); // #007A33 - darker forest green

// Light theme backgrounds
pub const LIGHT_BG: Color = Color::from_rgb(
    0xF8 as f32 / 255.0,
    0xF8 as f32 / 255.0,
    0xF8 as f32 / 255.0,
); // #F8F8F8 - soft off-white for reduced glare

pub const LIGHT_BG_SECONDARY: Color = Color::from_rgb(
    0xE5 as f32 / 255.0,
    0xE5 as f32 / 255.0,
    0xE5 as f32 / 255.0,
); // #E5E5E5

pub const LIGHT_BG_TERTIARY: Color = Color::from_rgb(
    0xD5 as f32 / 255.0,
    0xD5 as f32 / 255.0,
    0xD5 as f32 / 255.0,
); // #D5D5D5

// Light theme text colors
pub const DARK_TEXT_PRIMARY: Color = Color::from_rgb(
    0x00 as f32 / 255.0,
    0x00 as f32 / 255.0,
    0x00 as f32 / 255.0,
); // #000000 - true black

pub const DARK_TEXT_SECONDARY: Color = Color::from_rgb(
    0x1A as f32 / 255.0,
    0x1A as f32 / 255.0,
    0x1A as f32 / 255.0,
); // #1A1A1A - very dark grey

pub const DARK_TEXT_TERTIARY: Color = Color::from_rgb(
    0x6B as f32 / 255.0,
    0x6B as f32 / 255.0,
    0x6B as f32 / 255.0,
); // #6B6B6B

// Light theme borders
pub const LIGHT_BORDER: Color = Color::from_rgb(
    0xA5 as f32 / 255.0,
    0xA5 as f32 / 255.0,
    0xA5 as f32 / 255.0,
); // #A5A5A5

pub const LIGHT_BORDER_STRONG: Color = Color::from_rgb(
    0x7A as f32 / 255.0,
    0x7A as f32 / 255.0,
    0x7A as f32 / 255.0,
); // #7A7A7A

pub const AMBER: Color = Color::from_rgb(
    0xFC as f32 / 255.0,
    0xC1 as f32 / 255.0,
    0x07 as f32 / 255.0,
); // #FCC107
