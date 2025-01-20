use iced::{widget::tooltip, Length};

use crate::{component::text, icon, image, theme, widget::*};

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

pub fn recovery<'a, T: 'a>() -> Container<'a, T> {
    badge_pill("  Recovery  ", "This transaction is using a recovery path")
}

pub fn unconfirmed<'a, T: 'a>() -> Container<'a, T> {
    badge_pill(
        "  Unconfirmed  ",
        "Do not treat this as a payment until it is confirmed",
    )
}

pub fn batch<'a, T: 'a>() -> Container<'a, T> {
    badge_pill("  Batch  ", "This transaction contains multiple payments")
}

pub fn deprecated<'a, T: 'a>() -> Container<'a, T> {
    badge_pill(
        "  Deprecated  ",
        "This transaction cannot be included in the blockchain anymore.",
    )
}

pub fn spent<'a, T: 'a>() -> Container<'a, T> {
    badge_pill(
        "  Spent  ",
        "The transaction was included in the blockchain.",
    )
}

pub fn badge_pill<'a, T: 'a>(label: &'a str, tooltip: &'a str) -> Container<'a, T> {
    Container::new({
        tooltip::Tooltip::new(
            Container::new(text::p2_regular(label))
                .padding(10)
                .center_x(Length::Shrink)
                .style(theme::pill::simple),
            tooltip,
            tooltip::Position::Top,
        )
    })
}
