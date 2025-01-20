use crate::{font, theme::Theme};
use iced::advanced::text::Shaping;
use std::fmt::Display;

pub const H1_SIZE: u16 = 40;
pub const H2_SIZE: u16 = 29;
pub const H3_SIZE: u16 = 24;
pub const H4_SIZE: u16 = 20;
pub const H5_SIZE: u16 = 18;
pub const P1_SIZE: u16 = 16;
pub const P2_SIZE: u16 = 14;
pub const CAPTION_SIZE: u16 = 12;

pub fn h1<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::BOLD)
        .size(H1_SIZE)
}

pub fn h2<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::BOLD)
        .size(H2_SIZE)
}

pub fn h3<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::BOLD)
        .size(H3_SIZE)
}

pub fn h4_bold<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::BOLD)
        .size(H4_SIZE)
}

pub fn h4_regular<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::REGULAR)
        .size(H4_SIZE)
}

pub fn h5_medium<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::MEDIUM)
        .size(H5_SIZE)
}

pub fn h5_regular<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::REGULAR)
        .size(H5_SIZE)
}

pub fn p1_bold<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::BOLD)
        .size(P1_SIZE)
}

pub fn p1_medium<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::MEDIUM)
        .size(P1_SIZE)
}

pub fn p1_regular<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::REGULAR)
        .size(P1_SIZE)
}

pub fn p2_medium<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::MEDIUM)
        .size(P2_SIZE)
}

pub fn p2_regular<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::REGULAR)
        .size(P2_SIZE)
}

pub fn caption<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    iced::widget::text!("{}", content)
        .shaping(Shaping::Advanced)
        .font(font::REGULAR)
        .size(CAPTION_SIZE)
}

pub fn text<'a>(content: impl Display) -> iced::widget::Text<'a, Theme> {
    p1_regular(content)
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
