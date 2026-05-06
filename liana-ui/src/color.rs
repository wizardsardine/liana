use iced::Color;

#[macro_export]
macro_rules! color {
    ($name:ident, $hex:expr) => {
        color!($name, $hex, 1.0);
    };
    ($name:ident, $hex:expr, $a:expr) => {
        pub const $name: iced::Color = iced::Color {
            r: (($hex >> 16) & 0xFF) as f32 / 255.0,
            g: (($hex >> 8) & 0xFF) as f32 / 255.0,
            b: ($hex & 0xFF) as f32 / 255.0,
            a: $a,
        };
    };
}

pub const BLACK: Color = iced::Color::BLACK;
pub const TRANSPARENT: Color = iced::Color::TRANSPARENT;
pub const WHITE: Color = iced::Color::WHITE;

color!(LIGHT_BLACK, 0x141414);
color!(BUSINESS_BLACK, 0x0F172A);
color!(GREY_7, 0x3F3F3F);
color!(GREY_6, 0x202020);
color!(GREY_5, 0x272727);
color!(GREY_4, 0x424242);
color!(GREY_3, 0x717171);
color!(GREY_2, 0xCCCCCC);
color!(GREY_1, 0xE6E6E6);
color!(GREEN, 0x00FF66);
color!(SUCCESS_GREEN, 0x4CA55E);
color!(TRANSPARENT_GREEN, 0x00FF66, 0.3);
color!(RED, 0xE24E1B);
color!(ORANGE, 0xFFA700);
color!(BLUE, 0x7DD3FC);
color!(FINGERPRINT_BACKGROUND, 0x162B20);
color!(FINGERPRINT_BORDER, 0x18452B);
color!(FINGERPRINT_TEXT, 0x4ADE80);

// BUSINESS
color!(BUSINESS_BLUE, 0x00BDFF);
color!(BUSINESS_BLUE_DARK, 0x0099CC);
color!(BUSINESS_PILL_SIMPLE, 0x7E889F);
color!(TRANSPARENT_BUSINESS_BLUE, 0x00BFFF, 0.15);
color!(LIGHT_BLUE_TINT, 0xE5F5FF);
color!(SOFT_BLUE, 0x66D4FF);
color!(DARK_GREEN, 0x007A33);
color!(LIGHT_BG, 0xF8F8F8);
color!(LIGHT_BG_SECONDARY, 0xE5E5E5);
color!(LIGHT_BG_TERTIARY, 0xD5D5D5);
color!(DARK_TEXT_PRIMARY, 0x000000);
color!(DARK_TEXT_SECONDARY, 0x101213);
color!(DARK_TEXT_TERTIARY, 0x6B6B6B);
color!(LIGHT_BORDER, 0xA5A5A5);
color!(LIGHT_BORDER_STRONG, 0x7A7A7A);
color!(AMBER, 0xF59F00);
color!(BLACK_15, 0x000000, 0.15);
color!(BLACK_25, 0x000000, 0.25);
color!(BLACK_30, 0x000000, 0.3);
color!(BLACK_80, 0x000000, 0.80);
