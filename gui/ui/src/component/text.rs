use crate::{font, theme::Theme};
use std::borrow::Cow;

// 40 * 1.2
pub const H1_SIZE: u16 = 48;
// 29 * 1.2
pub const H2_SIZE: u16 = 35;
// 24 * 1.2
pub const H3_SIZE: u16 = 29;
// 20 * 1.2
pub const H4_SIZE: u16 = 24;
// 18 * 1.2
pub const H5_SIZE: u16 = 22;
// 16 * 1.2
pub const P1_SIZE: u16 = 20;
// 14 * 1.2
pub const P2_SIZE: u16 = 17;
// 12 * 1.2
pub const CAPTION_SIZE: u16 = 15;

pub fn h1<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::BOLD)
        .size(H1_SIZE)
}

pub fn h2<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::BOLD)
        .size(H2_SIZE)
}

pub fn h3<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::BOLD)
        .size(H3_SIZE)
}

pub fn h4_bold<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::BOLD)
        .size(H4_SIZE)
}

pub fn h4_regular<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(H4_SIZE)
}

pub fn h5_medium<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::MEDIUM)
        .size(H5_SIZE)
}

pub fn h5_regular<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(H5_SIZE)
}

pub fn p1_bold<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::BOLD)
        .size(P1_SIZE)
}

pub fn p1_medium<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::MEDIUM)
        .size(P1_SIZE)
}

pub fn p1_regular<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(P1_SIZE)
}

pub fn p2_medium<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::MEDIUM)
        .size(P2_SIZE)
}

pub fn p2_regular<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(P2_SIZE)
}

pub fn caption<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(CAPTION_SIZE)
}

pub fn text<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    p1_regular(content)
}

pub trait Text {
    fn bold(self) -> Self;
    fn small(self) -> Self;
}

impl Text for iced::widget::Text<'_, iced::Renderer<Theme>> {
    fn bold(self) -> Self {
        self.font(font::BOLD)
    }
    fn small(self) -> Self {
        self.size(20)
    }
}
