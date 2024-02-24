use iced::{widget::tooltip, Length};

use crate::util::Collection;
use crate::{component::text, icon, image, theme, widget::*};

pub struct Badge {
    icon: crate::widget::Text<'static>,
    style: theme::Badge,
}

impl Badge {
    pub fn new(icon: crate::widget::Text<'static>) -> Self {
        Self {
            icon,
            style: theme::Badge::Standard,
        }
    }
    pub fn style(self, style: theme::Badge) -> Self {
        Self {
            icon: self.icon,
            style,
        }
    }
}

impl<'a, Message: 'a> From<Badge> for Element<'a, Message> {
    fn from(badge: Badge) -> Element<'a, Message> {
        Container::new(badge.icon.width(Length::Fixed(20.0)))
            .width(Length::Fixed(40.0))
            .height(Length::Fixed(40.0))
            .style(theme::Container::Badge(badge.style))
            .center_x()
            .center_y()
            .into()
    }
}

pub fn receive<T>() -> Container<'static, T> {
    Container::new(icon::receive_icon().width(Length::Fixed(20.0)))
        .width(Length::Fixed(40.0))
        .height(Length::Fixed(40.0))
        .style(theme::Container::Badge(theme::Badge::Standard))
        .center_x()
        .center_y()
}

pub fn cycle<T>() -> Container<'static, T> {
    Container::new(icon::arrow_repeat().width(Length::Fixed(20.0)))
        .width(Length::Fixed(40.0))
        .height(Length::Fixed(40.0))
        .style(theme::Container::Badge(theme::Badge::Standard))
        .center_x()
        .center_y()
}

pub fn spend<T>() -> Container<'static, T> {
    Container::new(icon::send_icon().width(Length::Fixed(20.0)))
        .width(Length::Fixed(40.0))
        .height(Length::Fixed(40.0))
        .style(theme::Container::Badge(theme::Badge::Standard))
        .center_x()
        .center_y()
}

pub fn coin<T>() -> Container<'static, T> {
    Container::new(
        image::liana_grey_logo()
            .height(Length::Fixed(25.0))
            .width(Length::Fixed(25.0)),
    )
    .width(Length::Fixed(40.0))
    .height(Length::Fixed(40.0))
    .style(theme::Container::Badge(theme::Badge::Standard))
    .center_x()
    .center_y()
}

pub fn recovery<'a, T: 'a>() -> Container<'a, T> {
    badge_pill(
        "  Recovery  ",
        "This transaction is using a recovery path",
        None,
    )
}

pub fn unconfirmed<'a, T: 'a>() -> Container<'a, T> {
    badge_pill(
        "  Unconfirmed  ",
        "Do not treat this as a payment until it is confirmed",
        None,
    )
}

pub fn unconfirmed_sized<'a, T: 'a>(width: f32) -> Container<'a, T> {
    badge_pill(
        "  Unconfirmed  ",
        "Do not treat this as a payment until it is confirmed",
        Some(width),
    )
}

pub fn batch<'a, T: 'a>() -> Container<'a, T> {
    badge_pill(
        "  Batch  ",
        "This transaction contains multiple payments",
        None,
    )
}

pub fn deprecated<'a, T: 'a>() -> Container<'a, T> {
    badge_pill(
        "  Deprecated  ",
        "This transaction cannot be included in the blockchain anymore.",
        None,
    )
}

pub fn deprecated_sized<'a, T: 'a>(width: f32) -> Container<'a, T> {
    badge_pill(
        "  Deprecated  ",
        "This transaction cannot be included in the blockchain anymore.",
        Some(width),
    )
}

pub fn spent<'a, T: 'a>() -> Container<'a, T> {
    badge_pill(
        "  Spent  ",
        "The transaction was included in the blockchain.",
        None,
    )
}

pub fn spent_sized<'a, T: 'a>(width: f32) -> Container<'a, T> {
    badge_pill(
        "  Spent  ",
        "The transaction was included in the blockchain.",
        Some(width),
    )
}

pub fn badge_pill<'a, T: 'a>(
    label: &'a str,
    tooltip: &'a str,
    width: Option<f32>,
) -> Container<'a, T> {
    Container::new({
        let mut pill: Container<'a, T> = Container::new(
            Row::new()
                .push_maybe(width.map(|_| iced::widget::Space::with_width(Length::Fill)))
                .push(text::p2_regular(label))
                .push_maybe(width.map(|_| iced::widget::Space::with_width(Length::Fill))),
        )
        .padding(10)
        .style(theme::Container::Pill(theme::Pill::Simple));
        if let Some(w) = width {
            pill = pill.width(Length::Fixed(w));
        }

        tooltip::Tooltip::new(pill, tooltip, tooltip::Position::Top)
            .style(theme::Container::Card(theme::Card::Simple))
    })
}
