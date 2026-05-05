use crate::{font, theme::Theme};
use iced::advanced::text::Shaping;
use iced::Font;
use std::fmt::Display;

pub const H1_SIZE: u32 = 40;
pub const H2_SIZE: u32 = 29;
pub const H3_SIZE: u32 = 24;
pub const H4_SIZE: u32 = 20;
pub const H5_SIZE: u32 = 18;
pub const P1_SIZE: u32 = 16;
pub const P2_SIZE: u32 = 14;
pub const CAPTION_SIZE: u32 = 12;

/// Per-helper typography spec: the font and (optionally) size that a text
/// helper applies. Each `*_SPEC` constant below is the source of truth for
/// what its matching helper produces; external code (debug galleries, design
/// audits, etc.) can iterate or inspect these constants instead of parsing
/// the helpers' source.
///
/// `size: None` means the helper does not call `.size(...)` — the caller
/// controls the size (used by `button_text`).
#[derive(Debug, Clone, Copy)]
pub struct TextSpec {
    pub size: Option<u32>,
    pub font: Font,
}

#[rustfmt::skip]
pub const PANEL_TITLE_SPEC: TextSpec = TextSpec { size: Some(H2_SIZE),      font: font::MANROPE_BOLD };
#[rustfmt::skip]
pub const H1_SPEC:          TextSpec = TextSpec { size: Some(H1_SIZE),      font: font::BOLD };
#[rustfmt::skip]
pub const H2_SPEC:          TextSpec = TextSpec { size: Some(H2_SIZE),      font: font::BOLD };
#[rustfmt::skip]
pub const H3_SPEC:          TextSpec = TextSpec { size: Some(H3_SIZE),      font: font::BOLD };
#[rustfmt::skip]
pub const H4_BOLD_SPEC:     TextSpec = TextSpec { size: Some(H4_SIZE),      font: font::BOLD };
#[rustfmt::skip]
pub const H4_REGULAR_SPEC:  TextSpec = TextSpec { size: Some(H4_SIZE),      font: font::REGULAR };
#[rustfmt::skip]
pub const H5_MEDIUM_SPEC:   TextSpec = TextSpec { size: Some(H5_SIZE),      font: font::MEDIUM };
#[rustfmt::skip]
pub const H5_REGULAR_SPEC:  TextSpec = TextSpec { size: Some(H5_SIZE),      font: font::REGULAR };
#[rustfmt::skip]
pub const P1_BOLD_SPEC:     TextSpec = TextSpec { size: Some(P1_SIZE),      font: font::BOLD };
#[rustfmt::skip]
pub const P1_MEDIUM_SPEC:   TextSpec = TextSpec { size: Some(P1_SIZE),      font: font::MEDIUM };
#[rustfmt::skip]
pub const P1_REGULAR_SPEC:  TextSpec = TextSpec { size: Some(P1_SIZE),      font: font::REGULAR };
#[rustfmt::skip]
pub const P2_MEDIUM_SPEC:   TextSpec = TextSpec { size: Some(P2_SIZE),      font: font::MEDIUM };
#[rustfmt::skip]
pub const P2_REGULAR_SPEC:  TextSpec = TextSpec { size: Some(P2_SIZE),      font: font::REGULAR };
#[rustfmt::skip]
pub const CAPTION_SPEC:     TextSpec = TextSpec { size: Some(CAPTION_SIZE), font: font::REGULAR };
#[rustfmt::skip]
pub const BUTTON_TEXT_SPEC: TextSpec = TextSpec { size: None,               font: font::MEDIUM };

/// Build a text widget from a [`TextSpec`]. Always sets shaping to
/// [`Shaping::Advanced`] and the font; size is applied only when the spec
/// provides one. Used internally by every helper, and exposed publicly so
/// callers (e.g. design audits, debug galleries) can render arbitrary
/// content for any spec without reaching for a specific helper fn.
pub fn apply<'a>(content: impl Display, spec: TextSpec) -> iced::widget::Text<'a, Theme> {
    let mut t = iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(spec.font);
    if let Some(s) = spec.size {
        t = t.size(s);
    }
    t
}

pub fn panel_title<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, PANEL_TITLE_SPEC)
}

pub fn h1<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, H1_SPEC)
}

pub fn h2<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, H2_SPEC)
}

pub fn h3<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, H3_SPEC)
}

pub fn h4_bold<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, H4_BOLD_SPEC)
}

pub fn h4_regular<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, H4_REGULAR_SPEC)
}

pub fn h5_medium<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, H5_MEDIUM_SPEC)
}

pub fn h5_regular<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, H5_REGULAR_SPEC)
}

pub fn p1_bold<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, P1_BOLD_SPEC)
}

pub fn p1_medium<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, P1_MEDIUM_SPEC)
}

pub fn p1_regular<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, P1_REGULAR_SPEC)
}

pub fn p2_medium<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, P2_MEDIUM_SPEC)
}

pub fn p2_regular<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, P2_REGULAR_SPEC)
}

pub fn caption<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, CAPTION_SPEC)
}

pub fn text<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    p1_regular(content)
}

/// Text for use inside buttons - no color style applied so button can control color
pub fn button_text<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    apply(content, BUTTON_TEXT_SPEC)
}

pub trait Text {
    fn bold(self) -> Self;
    fn small(self) -> Self;
}

impl Text for iced::widget::Text<'_, Theme> {
    fn bold(self) -> Self {
        self.font(font::BOLD)
    }
    fn small(self) -> Self {
        self.size(P1_SIZE)
    }
}
