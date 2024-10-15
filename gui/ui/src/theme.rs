use iced::{
    application,
    widget::{
        button, checkbox, container, pick_list, progress_bar, qr_code, radio, scrollable, slider,
        svg, text, text_input,
    },
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
                background_color: color::GREY_2,
                text_color: color::LIGHT_BLACK,
            },
            Theme::Dark => application::Appearance {
                background_color: color::LIGHT_BLACK,
                text_color: color::WHITE,
            },
        }
    }
}

#[derive(Clone, Copy, Default)]
pub enum Overlay {
    #[default]
    Default,
}
impl iced::overlay::menu::StyleSheet for Theme {
    type Style = Overlay;

    fn appearance(&self, _style: &Self::Style) -> iced::overlay::menu::Appearance {
        iced::overlay::menu::Appearance {
            text_color: color::GREY_2,
            background: color::GREY_6.into(),
            border: iced::Border {
                color: color::GREY_2,
                width: 0.0,
                radius: 25.0.into(),
            },
            selected_text_color: color::LIGHT_BLACK,
            selected_background: color::GREEN.into(),
        }
    }
}
impl From<PickList> for Overlay {
    fn from(_p: PickList) -> Overlay {
        Overlay::Default
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
    Banner,
    Card(Card),
    Badge(Badge),
    Pill(Pill),
    Custom(iced::Color),
    Notification(Notification),
    QrCode,
}

impl container::StyleSheet for Theme {
    type Style = Container;
    fn appearance(&self, style: &Self::Style) -> iced::widget::container::Appearance {
        match self {
            Theme::Light => match style {
                Container::Transparent => container::Appearance {
                    background: Some(iced::Color::TRANSPARENT.into()),
                    ..container::Appearance::default()
                },
                Container::Background => container::Appearance {
                    background: Some(color::GREY_2.into()),
                    ..container::Appearance::default()
                },
                Container::Foreground => container::Appearance {
                    background: Some(color::GREY_2.into()),
                    ..container::Appearance::default()
                },
                Container::Border => container::Appearance {
                    background: Some(iced::Color::TRANSPARENT.into()),
                    border: iced::Border {
                        color: color::LIGHT_BLACK,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Container::Card(c) => c.appearance(self),
                Container::Badge(c) => c.appearance(self),
                Container::Pill(c) => c.appearance(self),
                Container::Notification(c) => c.appearance(self),
                Container::Custom(c) => container::Appearance {
                    background: Some((*c).into()),
                    ..container::Appearance::default()
                },
                Container::QrCode => container::Appearance {
                    background: Some(color::WHITE.into()),
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Container::Banner => container::Appearance {
                    background: Some(color::WHITE.into()),
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 0.0.into(),
                    },
                    text_color: color::LIGHT_BLACK.into(),
                    ..container::Appearance::default()
                },
            },
            Theme::Dark => match style {
                Container::Transparent => container::Appearance {
                    background: Some(iced::Color::TRANSPARENT.into()),
                    ..container::Appearance::default()
                },
                Container::Background => container::Appearance {
                    background: Some(color::LIGHT_BLACK.into()),
                    ..container::Appearance::default()
                },
                Container::Foreground => container::Appearance {
                    background: Some(color::BLACK.into()),
                    ..container::Appearance::default()
                },
                Container::Border => container::Appearance {
                    background: Some(iced::Color::TRANSPARENT.into()),
                    border: iced::Border {
                        color: color::GREY_3,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Container::Card(c) => c.appearance(self),
                Container::Badge(c) => c.appearance(self),
                Container::Pill(c) => c.appearance(self),
                Container::Notification(c) => c.appearance(self),
                Container::Custom(c) => container::Appearance {
                    background: Some((*c).into()),
                    ..container::Appearance::default()
                },
                Container::QrCode => container::Appearance {
                    background: Some(color::WHITE.into()),
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Container::Banner => container::Appearance {
                    background: Some(color::BLUE.into()),
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 0.0.into(),
                    },
                    text_color: color::LIGHT_BLACK.into(),
                    ..container::Appearance::default()
                },
            },
        }
    }
}

impl From<Card> for Container {
    fn from(c: Card) -> Container {
        Container::Card(c)
    }
}

impl From<Badge> for Container {
    fn from(c: Badge) -> Container {
        Container::Badge(c)
    }
}

impl From<Pill> for Container {
    fn from(c: Pill) -> Container {
        Container::Pill(c)
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum Notification {
    #[default]
    Pending,
    Error,
}

impl Notification {
    fn appearance(&self, theme: &Theme) -> iced::widget::container::Appearance {
        match theme {
            Theme::Light => match self {
                Self::Pending => container::Appearance {
                    background: Some(iced::Background::Color(color::GREEN)),
                    text_color: color::LIGHT_BLACK.into(),
                    border: iced::Border {
                        color: color::GREEN,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Self::Error => container::Appearance {
                    background: Some(iced::Background::Color(color::ORANGE)),
                    text_color: color::LIGHT_BLACK.into(),
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
            },
            Theme::Dark => match self {
                Self::Pending => container::Appearance {
                    background: Some(iced::Background::Color(color::GREEN)),
                    text_color: color::LIGHT_BLACK.into(),
                    border: iced::Border {
                        color: color::GREEN,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Self::Error => container::Appearance {
                    background: Some(iced::Background::Color(color::ORANGE)),
                    text_color: color::LIGHT_BLACK.into(),
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
            },
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum Card {
    #[default]
    Simple,
    Border,
    Invalid,
    Warning,
    Error,
}

impl Card {
    fn appearance(&self, theme: &Theme) -> iced::widget::container::Appearance {
        match theme {
            Theme::Light => match self {
                Card::Simple => container::Appearance {
                    background: Some(color::GREY_2.into()),
                    ..container::Appearance::default()
                },
                Card::Border => container::Appearance {
                    background: Some(iced::Color::TRANSPARENT.into()),
                    border: iced::Border {
                        color: color::GREY_2,
                        width: 1.0,
                        radius: 10.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Card::Invalid => container::Appearance {
                    background: Some(color::GREY_2.into()),
                    text_color: color::BLACK.into(),
                    border: iced::Border {
                        color: color::RED,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Card::Error => container::Appearance {
                    background: Some(color::GREY_2.into()),
                    text_color: color::RED.into(),
                    border: iced::Border {
                        color: color::RED,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Card::Warning => container::Appearance {
                    background: Some(color::ORANGE.into()),
                    text_color: color::GREY_2.into(),
                    ..container::Appearance::default()
                },
            },
            Theme::Dark => match self {
                Card::Simple => container::Appearance {
                    background: Some(color::GREY_6.into()),
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Card::Border => container::Appearance {
                    background: Some(iced::Color::TRANSPARENT.into()),
                    border: iced::Border {
                        color: color::GREY_5,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Card::Invalid => container::Appearance {
                    background: Some(color::LIGHT_BLACK.into()),
                    text_color: color::RED.into(),
                    border: iced::Border {
                        color: color::RED,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Card::Error => container::Appearance {
                    background: Some(color::LIGHT_BLACK.into()),
                    text_color: color::RED.into(),
                    border: iced::Border {
                        color: color::RED,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..container::Appearance::default()
                },
                Card::Warning => container::Appearance {
                    background: Some(color::ORANGE.into()),
                    text_color: color::LIGHT_BLACK.into(),
                    ..container::Appearance::default()
                },
            },
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum Badge {
    #[default]
    Standard,
    Bitcoin,
}

impl Badge {
    fn appearance(&self, _theme: &Theme) -> iced::widget::container::Appearance {
        match self {
            Self::Standard => container::Appearance {
                border: iced::Border {
                    color: color::TRANSPARENT,
                    width: 0.0,
                    radius: 40.0.into(),
                },
                background: Some(color::GREY_4.into()),
                ..container::Appearance::default()
            },
            Self::Bitcoin => container::Appearance {
                border: iced::Border {
                    color: color::TRANSPARENT,
                    width: 0.0,
                    radius: 40.0.into(),
                },
                background: Some(color::ORANGE.into()),
                text_color: iced::Color::WHITE.into(),
                ..container::Appearance::default()
            },
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum Pill {
    #[default]
    Simple,
    Primary,
    Success,
    Warning,
}

impl Pill {
    fn appearance(&self, _theme: &Theme) -> iced::widget::container::Appearance {
        match self {
            Self::Primary => container::Appearance {
                background: Some(color::GREEN.into()),
                border: iced::Border {
                    color: color::TRANSPARENT,
                    width: 0.0,
                    radius: 25.0.into(),
                },
                text_color: color::LIGHT_BLACK.into(),
                ..container::Appearance::default()
            },
            Self::Success => container::Appearance {
                background: Some(color::GREEN.into()),
                border: iced::Border {
                    color: color::TRANSPARENT,
                    width: 0.0,
                    radius: 25.0.into(),
                },
                text_color: color::LIGHT_BLACK.into(),
                ..container::Appearance::default()
            },
            Self::Simple => container::Appearance {
                background: Some(iced::Color::TRANSPARENT.into()),
                border: iced::Border {
                    color: color::GREY_3,
                    width: 1.0,
                    radius: 25.0.into(),
                },
                text_color: color::GREY_3.into(),
                ..container::Appearance::default()
            },
            Self::Warning => container::Appearance {
                background: Some(iced::Color::TRANSPARENT.into()),
                border: iced::Border {
                    color: color::RED,
                    width: 1.0,
                    radius: 25.0.into(),
                },
                text_color: color::RED.into(),
                ..container::Appearance::default()
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
            border_color: color::GREY_7,
            text_color: None,
        }
    }

    fn hovered(&self, style: &Self::Style, is_selected: bool) -> radio::Appearance {
        let active = self.active(style, is_selected);
        radio::Appearance {
            dot_color: color::GREEN,
            border_color: color::GREEN,
            background: iced::Color::TRANSPARENT.into(),
            ..active
        }
    }
}

#[derive(Default, Clone)]
pub struct Scrollable {}
impl scrollable::StyleSheet for Theme {
    type Style = Scrollable;

    fn active(&self, _style: &Self::Style) -> scrollable::Appearance {
        scrollable::Appearance {
            gap: None,
            container: container::Appearance::default(),
            scrollbar: scrollable::Scrollbar {
                background: None,
                border: iced::Border {
                    color: color::GREY_3,
                    width: 0.0,
                    radius: 10.0.into(),
                },
                scroller: scrollable::Scroller {
                    color: color::GREY_7,
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 10.0.into(),
                    },
                },
            },
        }
    }

    fn hovered(&self, style: &Self::Style, _is_hovered: bool) -> scrollable::Appearance {
        let active = self.active(style);
        scrollable::Appearance { ..active }
    }
}

#[derive(Default, Clone)]
pub enum PickList {
    #[default]
    Secondary,
}
impl pick_list::StyleSheet for Theme {
    type Style = PickList;

    fn active(&self, _style: &Self::Style) -> pick_list::Appearance {
        pick_list::Appearance {
            placeholder_color: color::GREY_6,
            handle_color: color::GREY_7,
            background: color::GREY_6.into(),
            border: iced::Border {
                color: color::GREY_7,
                width: 1.0,
                radius: 25.0.into(),
            },
            text_color: color::GREY_2,
        }
    }

    fn hovered(&self, _style: &Self::Style) -> pick_list::Appearance {
        pick_list::Appearance {
            placeholder_color: color::GREY_6,
            handle_color: color::GREEN,
            background: color::GREY_6.into(),
            border: iced::Border {
                color: color::GREEN,
                width: 1.0,
                radius: 25.0.into(),
            },
            text_color: color::GREEN,
        }
    }
}

#[derive(Default)]
pub struct CheckBox {}
impl checkbox::StyleSheet for Theme {
    type Style = CheckBox;

    fn active(&self, _style: &Self::Style, is_selected: bool) -> checkbox::Appearance {
        if is_selected {
            checkbox::Appearance {
                background: color::GREEN.into(),
                icon_color: color::GREY_4,
                text_color: None,
                border: iced::Border {
                    color: color::TRANSPARENT,
                    width: 1.0,
                    radius: 4.0.into(),
                },
            }
        } else {
            checkbox::Appearance {
                background: color::GREY_4.into(),
                icon_color: color::GREEN,
                text_color: None,
                border: iced::Border {
                    color: color::TRANSPARENT,
                    width: 0.0,
                    radius: 4.0.into(),
                },
            }
        }
    }

    fn hovered(&self, style: &Self::Style, is_selected: bool) -> checkbox::Appearance {
        self.active(style, is_selected)
    }
}

#[derive(Default)]
pub enum Button {
    #[default]
    Primary,
    Secondary,
    Destructive,
    SecondaryDestructive,
    Transparent,
    TransparentBorder,
    Border,
    Menu(bool),
}

impl button::StyleSheet for Theme {
    type Style = Button;

    fn active(&self, style: &Self::Style) -> button::Appearance {
        match self {
            Theme::Light => button::Appearance::default(),
            Theme::Dark => match style {
                Button::Primary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(color::GREEN.into()),
                    text_color: color::LIGHT_BLACK,
                    border: iced::Border {
                        color: color::GREEN,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::Secondary | Button::SecondaryDestructive | Button::Border => {
                    button::Appearance {
                        shadow_offset: iced::Vector::default(),
                        background: Some(color::GREY_6.into()),
                        text_color: color::GREY_2,
                        border: iced::Border {
                            color: color::GREY_7,
                            width: 1.0,
                            radius: 25.0.into(),
                        },
                        ..button::Appearance::default()
                    }
                }
                Button::Destructive => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(color::GREY_6.into()),
                    text_color: color::RED,
                    border: iced::Border {
                        color: color::RED,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::Transparent => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(iced::Color::TRANSPARENT.into()),
                    text_color: color::GREY_2,
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::TransparentBorder => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(iced::Color::TRANSPARENT.into()),
                    text_color: color::WHITE,
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::Menu(active) => {
                    if *active {
                        button::Appearance {
                            shadow_offset: iced::Vector::default(),
                            background: Some(color::LIGHT_BLACK.into()),
                            text_color: color::WHITE,
                            border: iced::Border {
                                color: color::TRANSPARENT,
                                width: 0.0,
                                radius: 25.0.into(),
                            },
                            ..button::Appearance::default()
                        }
                    } else {
                        button::Appearance {
                            shadow_offset: iced::Vector::default(),
                            background: Some(iced::Color::TRANSPARENT.into()),
                            text_color: color::WHITE,
                            border: iced::Border {
                                color: color::TRANSPARENT,
                                width: 0.0,
                                radius: 25.0.into(),
                            },
                            ..button::Appearance::default()
                        }
                    }
                }
            },
        }
    }

    fn hovered(&self, style: &Self::Style) -> button::Appearance {
        match self {
            Theme::Light => button::Appearance::default(),
            Theme::Dark => match style {
                Button::Primary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(color::GREEN.into()),
                    text_color: color::LIGHT_BLACK,
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::Secondary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(color::GREY_6.into()),
                    text_color: color::GREEN,
                    border: iced::Border {
                        color: color::GREEN,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::Destructive | Button::SecondaryDestructive => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(color::RED.into()),
                    text_color: color::LIGHT_BLACK,
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::Transparent => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(iced::Color::TRANSPARENT.into()),
                    text_color: color::GREY_2,
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::TransparentBorder | Button::Border => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(color::GREY_6.into()),
                    text_color: color::WHITE,
                    border: iced::Border {
                        color: color::GREEN,
                        width: 1.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
                Button::Menu(_) => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: Some(color::LIGHT_BLACK.into()),
                    text_color: color::WHITE,
                    border: iced::Border {
                        color: color::TRANSPARENT,
                        width: 0.0,
                        radius: 25.0.into(),
                    },
                    ..button::Appearance::default()
                },
            },
        }
    }
    fn disabled(&self, style: &Self::Style) -> button::Appearance {
        let active = self.active(style);

        button::Appearance {
            shadow_offset: iced::Vector::default(),
            background: Some(color::TRANSPARENT.into()),
            text_color: iced::Color {
                a: active.text_color.a * 0.5,
                ..active.text_color
            },
            ..active
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum Form {
    #[default]
    Simple,
    Invalid,
}

impl text_input::StyleSheet for Theme {
    type Style = Form;
    fn active(&self, style: &Self::Style) -> text_input::Appearance {
        match style {
            Form::Simple => text_input::Appearance {
                icon_color: color::GREY_7,
                background: iced::Background::Color(iced::Color::TRANSPARENT),
                border: iced::Border {
                    color: color::GREY_7,
                    width: 1.0,
                    radius: 25.0.into(),
                },
            },
            Form::Invalid => text_input::Appearance {
                icon_color: color::GREY_7,
                background: iced::Background::Color(iced::Color::TRANSPARENT),
                border: iced::Border {
                    color: color::RED,
                    width: 1.0,
                    radius: 25.0.into(),
                },
            },
        }
    }

    fn disabled(&self, style: &Self::Style) -> text_input::Appearance {
        text_input::Appearance {
            ..self.active(style)
        }
    }

    fn focused(&self, style: &Self::Style) -> text_input::Appearance {
        text_input::Appearance {
            ..self.active(style)
        }
    }

    fn disabled_color(&self, _style: &Self::Style) -> iced::Color {
        color::GREY_7
    }

    fn placeholder_color(&self, _style: &Self::Style) -> iced::Color {
        color::GREY_7
    }

    fn value_color(&self, _style: &Self::Style) -> iced::Color {
        color::GREY_2
    }

    fn selection_color(&self, _style: &Self::Style) -> iced::Color {
        color::GREEN
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum ProgressBar {
    #[default]
    Simple,
}

impl progress_bar::StyleSheet for Theme {
    type Style = ProgressBar;
    fn appearance(&self, _style: &Self::Style) -> progress_bar::Appearance {
        progress_bar::Appearance {
            background: color::GREY_6.into(),
            bar: color::GREEN.into(),
            border_radius: 10.0.into(),
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum Slider {
    #[default]
    Simple,
}

impl slider::StyleSheet for Theme {
    type Style = Slider;
    fn active(&self, _style: &Self::Style) -> slider::Appearance {
        let handle = slider::Handle {
            shape: slider::HandleShape::Rectangle {
                width: 8,
                border_radius: 4.0.into(),
            },
            color: color::BLACK,
            border_color: color::GREEN,
            border_width: 1.0,
        };
        slider::Appearance {
            rail: slider::Rail {
                colors: (color::GREEN, iced::Color::TRANSPARENT),
                border_radius: 4.0.into(),
                width: 2.0,
            },
            handle,
        }
    }
    fn hovered(&self, _style: &Self::Style) -> slider::Appearance {
        let handle = slider::Handle {
            shape: slider::HandleShape::Rectangle {
                width: 8,
                border_radius: 4.0.into(),
            },
            color: color::GREEN,
            border_color: color::GREEN,
            border_width: 1.0,
        };
        slider::Appearance {
            rail: slider::Rail {
                colors: (color::GREEN, iced::Color::TRANSPARENT),
                border_radius: 4.0.into(),
                width: 2.0,
            },
            handle,
        }
    }
    fn dragging(&self, _style: &Self::Style) -> slider::Appearance {
        let handle = slider::Handle {
            shape: slider::HandleShape::Rectangle {
                width: 8,
                border_radius: 4.0.into(),
            },
            color: color::GREEN,
            border_color: color::GREEN,
            border_width: 1.0,
        };
        slider::Appearance {
            rail: slider::Rail {
                colors: (color::GREEN, iced::Color::TRANSPARENT),
                border_radius: 4.0.into(),
                width: 2.0,
            },
            handle,
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub enum Svg {
    #[default]
    Simple,
}

impl svg::StyleSheet for Theme {
    type Style = ProgressBar;
    fn appearance(&self, _style: &Self::Style) -> svg::Appearance {
        svg::Appearance::default()
    }
}

impl qr_code::StyleSheet for Theme {
    type Style = ();
    fn appearance(&self, _style: &Self::Style) -> qr_code::Appearance {
        qr_code::Appearance {
            cell: color::BLACK,
            background: color::WHITE,
        }
    }
}
