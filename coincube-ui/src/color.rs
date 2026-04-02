use iced::Color;

// === Neutral palette ===
pub const BLACK: Color = iced::Color::BLACK;
pub const INK_BLACK: Color = iced::color!(0x050505);
pub const NEAR_BLACK: Color = iced::color!(0x0A0A0A);
pub const LIGHT_BLACK: Color = iced::color!(0x161716);
pub const DARK_GRAY: Color = iced::color!(0x1A1A1A);
pub const GREY_7: Color = iced::color!(0x3F3F3F);
pub const GREY_6: Color = iced::color!(0x202020);
pub const GREY_5: Color = iced::color!(0x272727);
pub const GREY_4: Color = iced::color!(0x424242);
pub const GREY_3: Color = iced::color!(0x717171);
pub const GREY_2: Color = iced::color!(0xCCCCCC);
pub const GREY_1: Color = iced::color!(0xE6E6E1);
pub const WHITE: Color = iced::Color::WHITE;
pub const WARM_PAPER: Color = iced::color!(0xF5F0E8);
pub const TRANSPARENT: Color = iced::Color::TRANSPARENT;

// === Brand accent colors ===
pub const ORANGE: Color = iced::color!(0xF7931A); // Bitcoin Orange — primary accent
pub const DARK_ORANGE: Color = iced::color!(0xD4770E); // Depth/hover on orange elements
pub const LIGHT_ORANGE: Color = iced::color!(0xFFB347); // Glow halos, highlights
pub const TRANSPARENT_ORANGE: Color = iced::color!(0xF7931A, 0.3);

pub const GREEN: Color = iced::color!(0x00FF66);
pub const DARK_GREEN: Color = iced::color!(0x1B8A4A); // Light-mode friendly green
pub const TRANSPARENT_GREEN: Color = iced::color!(0x00FF66, 0.3);

pub const RED: Color = iced::color!(0xE24E1B);
pub const DARK_RED: Color = iced::color!(0xC43A12); // Light-mode friendly red

pub const BLUE: Color = iced::color!(0x7DD3FC);
pub const LIQUID_TEAL: Color = iced::color!(0x46BEAE); // Liquid Network teal — matches liquid.svg

// === Light-mode surface colors ===
pub const LIGHT_BG: Color = iced::color!(0xFAF9F6); // Main background (warm off-white)
pub const LIGHT_SURFACE: Color = iced::color!(0xFFFFFF); // Card/input surfaces
pub const LIGHT_CARD_BG: Color = iced::color!(0xEDE8E0); // Card backgrounds (warm)
pub const LIGHT_BORDER: Color = iced::color!(0xD5D0C8); // Subtle borders
pub const LIGHT_HOVER: Color = iced::color!(0xE5E0D8); // Hover state backgrounds

// === Toast / notification severity colors (WCAG AA with white text) ===
pub const SUCCESS_GREEN: Color = iced::color!(0x2D6A4F); // 6.39:1
pub const ERROR_RED: Color = iced::color!(0xC0392B); // 5.44:1
pub const WARN_ORANGE: Color = iced::color!(0xD4770E); // ~4.6:1 (same as DARK_ORANGE)
pub const INFO_BLUE: Color = iced::color!(0x2E86C1); // ~5.0:1
