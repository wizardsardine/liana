use iced::{
    widget::{self, tooltip, Container},
    Element, Length,
};

use crate::ui::{
    color,
    component::{card, text::*},
    icon,
};

pub enum Style {
    Standard,
    Success,
    Warning,
    Bitcoin,
}

impl widget::container::StyleSheet for Style {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> widget::container::Appearance {
        match self {
            Self::Standard => widget::container::Appearance {
                border_radius: 40.0,
                background: color::BACKGROUND.into(),
                ..widget::container::Appearance::default()
            },
            Self::Success => widget::container::Appearance {
                border_radius: 40.0,
                background: color::SUCCESS_LIGHT.into(),
                text_color: color::SUCCESS.into(),
                ..widget::container::Appearance::default()
            },
            Self::Warning => widget::container::Appearance {
                border_radius: 40.0,
                background: color::WARNING_LIGHT.into(),
                text_color: color::WARNING.into(),
                ..widget::container::Appearance::default()
            },
            Self::Bitcoin => widget::container::Appearance {
                border_radius: 40.0,
                background: color::WARNING.into(),
                text_color: iced::Color::WHITE.into(),
                ..widget::container::Appearance::default()
            },
        }
    }
}

impl From<Style> for Box<dyn widget::container::StyleSheet<Style = iced::Theme>> {
    fn from(s: Style) -> Box<dyn widget::container::StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<Style> for iced::theme::Container {
    fn from(i: Style) -> iced::theme::Container {
        iced::theme::Container::Custom(i.into())
    }
}

pub struct Badge {
    icon: widget::Text<'static>,
    style: Style,
}

impl Badge {
    pub fn new(icon: widget::Text<'static>) -> Self {
        Self {
            icon,
            style: Style::Standard,
        }
    }
    pub fn style(self, style: Style) -> Self {
        Self {
            icon: self.icon,
            style,
        }
    }
}

impl<'a, Message: 'a> From<Badge> for Element<'a, Message> {
    fn from(badge: Badge) -> Element<'a, Message> {
        Container::new(badge.icon.width(Length::Units(20)))
            .width(Length::Units(40))
            .height(Length::Units(40))
            .style(badge.style)
            .center_x()
            .center_y()
            .into()
    }
}

pub fn receive<T>() -> widget::container::Container<'static, T> {
    Container::new(icon::receive_icon().width(Length::Units(20)))
        .width(Length::Units(40))
        .height(Length::Units(40))
        .style(Style::Standard)
        .center_x()
        .center_y()
}

pub fn spend<T>() -> widget::container::Container<'static, T> {
    Container::new(icon::send_icon().width(Length::Units(20)))
        .width(Length::Units(40))
        .height(Length::Units(40))
        .style(Style::Standard)
        .center_x()
        .center_y()
}

pub fn coin<T>() -> widget::container::Container<'static, T> {
    Container::new(icon::coin_icon().width(Length::Units(20)))
        .width(Length::Units(40))
        .height(Length::Units(40))
        .style(Style::Standard)
        .center_x()
        .center_y()
}

pub enum PillStyle {
    InversePrimary,
    Primary,
    Success,
    Simple,
}

impl widget::container::StyleSheet for PillStyle {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> widget::container::Appearance {
        match self {
            Self::Primary => widget::container::Appearance {
                background: color::PRIMARY.into(),
                border_radius: 10.0,
                text_color: iced::Color::WHITE.into(),
                ..widget::container::Appearance::default()
            },
            Self::InversePrimary => widget::container::Appearance {
                background: color::FOREGROUND.into(),
                border_radius: 10.0,
                text_color: color::PRIMARY.into(),
                ..widget::container::Appearance::default()
            },
            Self::Success => widget::container::Appearance {
                background: color::SUCCESS.into(),
                border_radius: 10.0,
                text_color: iced::Color::WHITE.into(),
                ..widget::container::Appearance::default()
            },
            Self::Simple => widget::container::Appearance {
                background: color::BACKGROUND.into(),
                border_radius: 10.0,
                text_color: iced::Color::BLACK.into(),
                ..widget::container::Appearance::default()
            },
        }
    }
}

impl From<PillStyle> for Box<dyn widget::container::StyleSheet<Style = iced::Theme>> {
    fn from(s: PillStyle) -> Box<dyn widget::container::StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<PillStyle> for iced::theme::Container {
    fn from(i: PillStyle) -> iced::theme::Container {
        iced::theme::Container::Custom(i.into())
    }
}

pub fn unconfirmed<'a, T: 'a>() -> widget::container::Container<'a, T> {
    Container::new(
        tooltip::Tooltip::new(
            Container::new(text("  Unconfirmed  ").small())
                .padding(3)
                .style(PillStyle::Simple),
            "Do not treat this as a payment until it is confirmed",
            tooltip::Position::Top,
        )
        .style(card::SimpleCardStyle),
    )
}

pub fn deprecated<'a, T: 'a>() -> widget::container::Container<'a, T> {
    Container::new(
        tooltip::Tooltip::new(
            Container::new(text("  Deprecated  ").small())
                .padding(3)
                .style(PillStyle::Simple),
            "This spend cannot be included anymore in the blockchain",
            tooltip::Position::Top,
        )
        .style(card::SimpleCardStyle),
    )
}

pub fn spent<'a, T: 'a>() -> widget::container::Container<'a, T> {
    Container::new(
        tooltip::Tooltip::new(
            Container::new(text("  Spent  ").small())
                .padding(3)
                .style(PillStyle::Simple),
            "The spend transaction was included in the blockchain",
            tooltip::Position::Top,
        )
        .style(card::SimpleCardStyle),
    )
}
