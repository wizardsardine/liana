use iced::{
    application,
    widget::{button, container, radio, text},
};

use super::color;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
}

impl application::StyleSheet for Theme {
    type Style = ();

    fn appearance(&self, _style: &Self::Style) -> application::Appearance {
        match self {
            Theme::Light => application::Appearance {
                background_color: color::LIGHT_GREY,
                text_color: color::LIGHT_BLACK,
            },
            Theme::Dark => application::Appearance {
                background_color: color::LIGHT_BLACK,
                text_color: color::LIGHT_GREY,
            },
        }
    }
}

#[derive(Clone, Copy, Default)]
pub enum Text {
    #[default]
    Default,
    Color(iced::Color),
}

impl From<iced::Color> for Text {
    fn from(color: iced::Color) -> Self {
        Text::Color(color)
    }
}

impl text::StyleSheet for Theme {
    type Style = Text;

    fn appearance(&self, style: Self::Style) -> text::Appearance {
        match style {
            Text::Default => Default::default(),
            Text::Color(c) => text::Appearance { color: Some(c) },
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum Container {
    #[default]
    Transparent,
    Background,
    Foreground,
    Border,
    Custom(iced::Color),
}

impl container::StyleSheet for Theme {
    type Style = Container;
    fn appearance(&self, style: &Self::Style) -> iced::widget::container::Appearance {
        match self {
            Theme::Light => match style {
                Container::Transparent => container::Appearance {
                    background: iced::Color::TRANSPARENT.into(),
                    ..container::Appearance::default()
                },
                Container::Background => container::Appearance {
                    background: color::LIGHT_GREY.into(),
                    ..container::Appearance::default()
                },
                Container::Foreground => container::Appearance {
                    background: color::GREY.into(),
                    ..container::Appearance::default()
                },
                Container::Border => container::Appearance {
                    background: iced::Color::TRANSPARENT.into(),
                    border_width: 1.0,
                    border_color: color::LIGHT_BLACK.into(),
                    ..container::Appearance::default()
                },
                Container::Custom(c) => container::Appearance {
                    background: (*c).into(),
                    ..container::Appearance::default()
                },
            },
            Theme::Dark => match style {
                Container::Transparent => container::Appearance {
                    background: iced::Color::TRANSPARENT.into(),
                    ..container::Appearance::default()
                },
                Container::Background => container::Appearance {
                    background: color::LIGHT_BLACK.into(),
                    ..container::Appearance::default()
                },
                Container::Foreground => container::Appearance {
                    background: color::BLACK.into(),
                    ..container::Appearance::default()
                },
                Container::Border => container::Appearance {
                    background: iced::Color::TRANSPARENT.into(),
                    border_width: 1.0,
                    border_color: color::LIGHT_GREY.into(),
                    ..container::Appearance::default()
                },
                Container::Custom(c) => container::Appearance {
                    background: (*c).into(),
                    ..container::Appearance::default()
                },
            },
        }
    }
}

#[derive(Default)]
pub struct Radio {}
impl radio::StyleSheet for Theme {
    type Style = Radio;

    fn active(&self, _style: &Self::Style, _is_selected: bool) -> radio::Appearance {
        radio::Appearance {
            background: iced::Color::TRANSPARENT.into(),
            dot_color: color::GREEN,
            border_width: 1.0,
            border_color: color::GREEN,
            text_color: None,
        }
    }

    fn hovered(&self, style: &Self::Style, is_selected: bool) -> radio::Appearance {
        let active = self.active(style, is_selected);
        radio::Appearance {
            dot_color: color::GREEN,
            background: iced::Color::TRANSPARENT.into(),
            ..active
        }
    }
}

#[derive(Default)]
pub enum Button {
    #[default]
    Primary,
    Secondary,
    Destructive,
    Transparent,
}

impl button::StyleSheet for Theme {
    type Style = Button;

    fn active(&self, style: &Self::Style) -> button::Appearance {
        match self {
            Theme::Light => match style {
                Button::Primary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::LIGHT_BLACK.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_GREY,
                },
                Button::Secondary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 10.0,
                    border_width: 1.0,
                    border_color: color::DARK_GREY,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Destructive => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::RED.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_GREY,
                },
                Button::Transparent => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
            },
            Theme::Dark => match style {
                Button::Primary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::GREY.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Secondary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::LIGHT_BLACK.into(),
                    border_radius: 10.0,
                    border_width: 1.0,
                    border_color: color::LIGHT_GREY,
                    text_color: color::LIGHT_GREY,
                },
                Button::Destructive => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::RED.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Transparent => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_GREY,
                },
            },
        }
    }

    fn hovered(&self, style: &Self::Style) -> button::Appearance {
        match self {
            Theme::Light => match style {
                Button::Primary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::LIGHT_BLACK.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_GREY,
                },
                Button::Secondary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::LIGHT_BLACK.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_GREY,
                },
                Button::Destructive => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::RED.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_GREY,
                },
                Button::Transparent => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::DARK_GREY.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_GREY,
                },
            },
            Theme::Dark => match style {
                Button::Primary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::GREY.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Secondary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::GREY.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Destructive => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::RED.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Transparent => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::DARK_GREY.into(),
                    border_radius: 10.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_GREY,
                },
            },
        }
    }
}
