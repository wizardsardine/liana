use iced::Length;

use crate::{icon, image, theme, widget::*};

pub fn badge<T>(icon: crate::widget::Text<'static>) -> Container<'static, T> {
    Container::new(icon.width(Length::Fixed(20.0)))
        .style(theme::badge::simple)
        .center_x(Length::Fixed(40.0))
        .center_y(Length::Fixed(40.0))
}

pub fn receive<T>() -> Container<'static, T> {
    Container::new(icon::receive_icon().width(Length::Fixed(20.0)))
        .style(theme::badge::simple)
        .style(theme::badge::simple)
        .center_x(Length::Fixed(40.0))
        .center_y(Length::Fixed(40.0))
}

pub fn cycle<T>() -> Container<'static, T> {
    Container::new(icon::arrow_repeat().width(Length::Fixed(20.0)))
        .style(theme::badge::simple)
        .center_x(Length::Fixed(40.0))
        .center_y(Length::Fixed(40.0))
}

pub fn spend<T>() -> Container<'static, T> {
    Container::new(icon::send_icon().width(Length::Fixed(20.0)))
        .style(theme::badge::simple)
        .center_x(Length::Fixed(40.0))
        .center_y(Length::Fixed(40.0))
}

pub fn coin<T>() -> Container<'static, T> {
    Container::new(
        image::liana_grey_logo()
            .height(Length::Fixed(25.0))
            .width(Length::Fixed(25.0)),
    )
    .style(theme::badge::simple)
    .center_x(Length::Fixed(40.0))
    .center_y(Length::Fixed(40.0))
}
