use crate::{icon, theme, widget::*};

pub fn tooltip<T: 'static>(help: &'static str) -> Container<'static, T> {
    Container::new(
        iced::widget::tooltip::Tooltip::new(
            icon::tooltip_icon(),
            help,
            iced::widget::tooltip::Position::Right,
        )
        .style(theme::card::simple),
    )
}
