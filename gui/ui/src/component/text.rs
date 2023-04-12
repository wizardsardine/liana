use crate::{font, theme::Theme};
use std::borrow::Cow;

pub fn h1<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content).font(font::BOLD).size(40)
}

pub fn h2<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content).font(font::BOLD).size(29)
}

pub fn h3<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content).font(font::BOLD).size(24)
}

pub fn h4_bold<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content).font(font::BOLD).size(20)
}

pub fn h4_regular<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(20)
}

pub fn h5_medium<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content).font(font::MEDIUM).size(18)
}

pub fn h5_regular<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(18)
}

pub fn p1_bold<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content).font(font::BOLD).size(16)
}

pub fn p1_medium<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content).font(font::MEDIUM).size(16)
}

pub fn p1_regular<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(16)
}

pub fn p2_medium<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content).font(font::MEDIUM).size(14)
}

pub fn p2_regular<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(14)
}

pub fn caption<'a>(
    content: impl Into<Cow<'a, str>>,
) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(12)
}

pub const TEXT_REGULAR_SIZE: u16 = 25;

pub fn text<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a, iced::Renderer<Theme>> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(TEXT_REGULAR_SIZE)
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
