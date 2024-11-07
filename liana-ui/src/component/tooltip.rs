use crate::{icon, theme, widget::*};

pub fn tooltip<'a, T: 'a>(help: &'static str) -> Container<'a, T> {
    Container::new(
        iced::widget::tooltip::Tooltip::new(
            icon::tooltip_icon(),
            help,
            iced::widget::tooltip::Position::Right,
        )
        .style(theme::Container::Card(theme::Card::Simple)),
    )
}
