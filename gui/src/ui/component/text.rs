use crate::ui::font;

pub fn text(content: &str) -> iced::pure::widget::Text {
    iced::pure::widget::Text::new(content)
        .font(font::REGULAR)
        .size(25)
}

pub trait Text {
    fn bold(self) -> Self;
    fn small(self) -> Self;
}

impl Text for iced::pure::widget::Text {
    fn bold(self) -> Self {
        self.font(font::BOLD)
    }
    fn small(self) -> Self {
        self.size(20)
    }
}
