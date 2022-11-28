use crate::ui::color;
use iced::widget::container;

pub enum Style {
    Sidebar,
    Background,
}

impl container::StyleSheet for Style {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> container::Appearance {
        match self {
            Self::Background => container::Appearance {
                background: color::BACKGROUND.into(),
                ..container::Appearance::default()
            },
            Self::Sidebar => container::Appearance {
                background: color::FOREGROUND.into(),
                border_width: 1.0,
                border_color: color::SECONDARY,
                ..container::Appearance::default()
            },
        }
    }
}

impl From<Style> for Box<dyn container::StyleSheet<Style = iced::Theme>> {
    fn from(s: Style) -> Box<dyn container::StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<Style> for iced::theme::Container {
    fn from(i: Style) -> iced::theme::Container {
        iced::theme::Container::Custom(i.into())
    }
}
