use super::text_roles;
use crate::font;

// Each entry expands to:
//
//     pub const D2_SPEC: TextSpec = TextSpec {
//         size: Some(32),
//         font: font::MANROPE_BOLD,
//     };
//
//     pub fn d2<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
//         apply(content, D2_SPEC)
//     }
#[rustfmt::skip]
text_roles! {
    d2,                     D2_SPEC,                    font::MANROPE_BOLD,     32;
    d3,                     D3_SPEC,                    font::MANROPE_BOLD,     26;
    d4,                     D4_SPEC,                    font::MANROPE_BOLD,     22;
    h1,                     H1_SPEC,                    font::MANROPE_MEDIUM,   24;
    h2,                     H2_SPEC,                    font::MANROPE_MEDIUM,   22;
    h2_semi,                H2_SEMI_SPEC,               font::MANROPE_SEMIBOLD, 22;
    h3,                     H3_SPEC,                    font::MANROPE_MEDIUM,   20;
    h3_semi,                H3_SEMI_SPEC,               font::MANROPE_SEMIBOLD, 20;
    b1,                     B1_SPEC,                    font::REGULAR,          24;
    b1_bold,                B1_BOLD_SPEC,               font::BOLD,             24;
    b2,                     B2_SPEC,                    font::REGULAR,          22;
    b2_medium,              B2_MEDIUM_SPEC,             font::MEDIUM,           22;
    b3,                     B3_SPEC,                    font::REGULAR,          20;
    b3_medium,              B3_MEDIUM_SPEC,             font::MEDIUM,           20;
    b4_medium,              B4_MEDIUM_SPEC,             font::MEDIUM,           18;
    b4_bold,                B4_BOLD_SPEC,               font::BOLD,             18;
    b5_medium,              B5_MEDIUM_SPEC,             font::MEDIUM,           16;
    b5_bold,                B5_BOLD_SPEC,               font::BOLD,             16;
    caption,                CAPTION_SPEC,               font::REGULAR,          16;
    small_caption,          SMALL_CAPTION_SPEC,         font::REGULAR,          13;
    button_text,            BUTTON_TEXT_SPEC,           font::MEDIUM,           17;
    button_text_compact,    BUTTON_TEXT_COMPACT_SPEC,   font::MEDIUM,           14;
}
