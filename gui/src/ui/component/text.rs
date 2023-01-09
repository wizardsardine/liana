use crate::ui::font;
use std::borrow::Cow;

pub const TEXT_REGULAR_SIZE: u16 = 25;

pub fn text<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(TEXT_REGULAR_SIZE)
}

pub trait Text {
    fn bold(self) -> Self;
    fn small(self) -> Self;
}

impl Text for iced::widget::Text<'_> {
    fn bold(self) -> Self {
        self.font(font::BOLD)
    }
    fn small(self) -> Self {
        self.size(20)
    }
}
