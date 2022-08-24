pub mod badge;
pub mod button;
pub mod card;
pub mod form;
pub mod text;

use iced::pure::widget::{container, Column, Container};
use iced::Length;

use crate::ui::color;

pub fn separation<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Column::new().push(iced::Text::new(" ")))
        .style(SepStyle)
        .height(Length::Units(1))
}

pub struct SepStyle;
impl container::StyleSheet for SepStyle {
    fn style(&self) -> container::Style {
        container::Style {
            background: color::SECONDARY.into(),
            ..container::Style::default()
        }
    }
}
