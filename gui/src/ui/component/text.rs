use crate::ui::font;
use std::borrow::Cow;

pub fn text<'a>(content: impl Into<Cow<'a, str>>) -> iced::widget::Text<'a> {
    iced::widget::Text::new(content)
        .font(font::REGULAR)
        .size(25)
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
