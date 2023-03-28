pub mod badge;
pub mod button;
pub mod card;
pub mod collapse;
pub mod form;
pub mod modal;
pub mod notification;
pub mod text;
pub mod tooltip;

pub use tooltip::tooltip;

use iced::Length;

use crate::{theme, widget::*};

pub fn separation<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Column::new().push(Text::new(" ")))
        .style(theme::Container::Border)
        .height(Length::Units(1))
}
