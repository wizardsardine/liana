use super::text_roles;
use crate::{font, theme::Theme};
use std::fmt::Display;

pub const H1_SIZE: u32 = 40;
pub const H2_SIZE: u32 = 29;
pub const H3_SIZE: u32 = 24;
pub const H4_SIZE: u32 = 20;
pub const H5_SIZE: u32 = 18;
pub const P1_SIZE: u32 = 16;
pub const P2_SIZE: u32 = 14;
pub const CAPTION_SIZE: u32 = 12;

// Each entry expands to:
//
//     pub const PANEL_TITLE_SPEC: TextSpec = TextSpec {
//         size: Some(H2_SIZE),
//         font: font::MANROPE_BOLD,
//     };
//
//     pub fn panel_title<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
//         apply(content, PANEL_TITLE_SPEC)
//     }
//
// `button_text` omits the trailing size, so its `*_SPEC` gets `size: None`.
#[rustfmt::skip]
text_roles! {
    panel_title, PANEL_TITLE_SPEC, font::MANROPE_BOLD, H2_SIZE;
    h1,          H1_SPEC,          font::BOLD,         H1_SIZE;
    h2,          H2_SPEC,          font::BOLD,         H2_SIZE;
    h3,          H3_SPEC,          font::BOLD,         H3_SIZE;
    h4_bold,     H4_BOLD_SPEC,     font::BOLD,         H4_SIZE;
    h4_regular,  H4_REGULAR_SPEC,  font::REGULAR,      H4_SIZE;
    h5_medium,   H5_MEDIUM_SPEC,   font::MEDIUM,       H5_SIZE;
    h5_regular,  H5_REGULAR_SPEC,  font::REGULAR,      H5_SIZE;
    p1_bold,     P1_BOLD_SPEC,     font::BOLD,         P1_SIZE;
    p1_medium,   P1_MEDIUM_SPEC,   font::MEDIUM,       P1_SIZE;
    p1_regular,  P1_REGULAR_SPEC,  font::REGULAR,      P1_SIZE;
    p2_medium,   P2_MEDIUM_SPEC,   font::MEDIUM,       P2_SIZE;
    p2_regular,  P2_REGULAR_SPEC,  font::REGULAR,      P2_SIZE;
    caption,     CAPTION_SPEC,     font::REGULAR,      CAPTION_SIZE;
    button_text, BUTTON_TEXT_SPEC, font::MEDIUM;
}

pub fn text<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    p1_regular(content)
}
