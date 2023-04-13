use iced::{
    application,
    widget::{
        button, checkbox, container, pick_list, progress_bar, radio, scrollable, slider, text,
        text_input,
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
            border_width: 0.0,
            border_radius: 25.0,
            border_color: color::GREY_2,
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
    Card(Card),
    Badge(Badge),
    Pill(Pill),
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
                    background: color::GREY_2.into(),
                    ..container::Appearance::default()
                },
                Container::Foreground => container::Appearance {
                    background: color::GREY_2.into(),
                    ..container::Appearance::default()
                },
                Container::Border => container::Appearance {
                    background: iced::Color::TRANSPARENT.into(),
                    border_width: 1.0,
                    border_color: color::LIGHT_BLACK,
                    ..container::Appearance::default()
                },
                Container::Card(c) => c.appearance(self),
                Container::Badge(c) => c.appearance(self),
                Container::Pill(c) => c.appearance(self),
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
                    border_color: color::GREY_3,
                    ..container::Appearance::default()
                },
                Container::Card(c) => c.appearance(self),
                Container::Badge(c) => c.appearance(self),
                Container::Pill(c) => c.appearance(self),
                Container::Custom(c) => container::Appearance {
                    background: (*c).into(),
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
                    background: color::GREY_2.into(),
                    ..container::Appearance::default()
                },
                Card::Border => container::Appearance {
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 10.0,
                    border_color: color::GREY_2,
                    border_width: 1.0,
                    ..container::Appearance::default()
                },
                Card::Invalid => container::Appearance {
                    background: color::GREY_2.into(),
                    text_color: color::BLACK.into(),
                    border_width: 1.0,
                    border_color: color::RED,
                    ..container::Appearance::default()
                },
                Card::Error => container::Appearance {
                    background: color::GREY_2.into(),
                    text_color: color::RED.into(),
                    border_width: 1.0,
                    border_color: color::RED,
                    ..container::Appearance::default()
                },
                Card::Warning => container::Appearance {
                    background: color::ORANGE.into(),
                    text_color: color::GREY_2.into(),
                    ..container::Appearance::default()
                },
            },
            Theme::Dark => match self {
                Card::Simple => container::Appearance {
                    background: color::GREY_6.into(),
                    border_radius: 25.0,
                    ..container::Appearance::default()
                },
                Card::Border => container::Appearance {
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 25.0,
                    border_color: color::GREY_5,
                    border_width: 1.0,
                    ..container::Appearance::default()
                },
                Card::Invalid => container::Appearance {
                    background: color::LIGHT_BLACK.into(),
                    text_color: color::RED.into(),
                    border_width: 1.0,
                    border_radius: 25.0,
                    border_color: color::RED,
                },
                Card::Error => container::Appearance {
                    background: color::LIGHT_BLACK.into(),
                    text_color: color::RED.into(),
                    border_width: 1.0,
                    border_color: color::RED,
                    ..container::Appearance::default()
                },
                Card::Warning => container::Appearance {
                    background: color::ORANGE.into(),
                    text_color: color::WHITE.into(),
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
                border_radius: 40.0,
                background: color::GREY_4.into(),
                ..container::Appearance::default()
            },
            Self::Bitcoin => container::Appearance {
                border_radius: 40.0,
                background: color::ORANGE.into(),
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
}

impl Pill {
    fn appearance(&self, _theme: &Theme) -> iced::widget::container::Appearance {
        match self {
            Self::Primary => container::Appearance {
                background: color::GREEN.into(),
                border_radius: 25.0,
                text_color: color::LIGHT_BLACK.into(),
                ..container::Appearance::default()
            },
            Self::Success => container::Appearance {
                background: color::GREEN.into(),
                border_radius: 25.0,
                text_color: color::LIGHT_BLACK.into(),
                ..container::Appearance::default()
            },
            Self::Simple => container::Appearance {
                background: color::GREEN.into(),
                border_radius: 25.0,
                text_color: color::LIGHT_BLACK.into(),
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

#[derive(Default, Clone)]
pub struct Scrollable {}
impl scrollable::StyleSheet for Theme {
    type Style = Scrollable;

    fn active(&self, _style: &Self::Style) -> scrollable::Scrollbar {
        scrollable::Scrollbar {
            background: None,
            border_width: 0.0,
            border_color: color::GREY_7,
            border_radius: 10.0,
            scroller: scrollable::Scroller {
                color: color::GREY_7,
                border_radius: 10.0,
                border_width: 0.0,
                border_color: iced::Color::TRANSPARENT,
            },
        }
    }

    fn hovered(&self, style: &Self::Style) -> scrollable::Scrollbar {
        let active = self.active(style);
        scrollable::Scrollbar { ..active }
    }
}

#[derive(Default, Clone)]
pub enum PickList {
    #[default]
    Simple,
    Invalid,
}
impl pick_list::StyleSheet for Theme {
    type Style = PickList;

    fn active(&self, style: &Self::Style) -> pick_list::Appearance {
        match style {
            PickList::Simple => pick_list::Appearance {
                placeholder_color: color::GREY_6,
                handle_color: color::GREY_6,
                background: color::GREEN.into(),
                border_width: 1.0,
                border_color: color::GREY_7,
                border_radius: 25.0,
                text_color: iced::Color::BLACK,
            },
            PickList::Invalid => pick_list::Appearance {
                placeholder_color: color::GREY_6,
                handle_color: color::GREY_6,
                background: color::GREY_6.into(),
                border_width: 1.0,
                border_color: color::RED,
                border_radius: 25.0,
                text_color: iced::Color::BLACK,
            },
        }
    }

    fn hovered(&self, style: &Self::Style) -> pick_list::Appearance {
        let active = self.active(style);
        pick_list::Appearance { ..active }
    }
}

#[derive(Default)]
pub struct CheckBox {}
impl checkbox::StyleSheet for Theme {
    type Style = CheckBox;

    fn active(&self, _style: &Self::Style, _is_selected: bool) -> checkbox::Appearance {
        checkbox::Appearance {
            background: iced::Color::TRANSPARENT.into(),
            border_width: 1.0,
            border_color: color::GREY_7,
            checkmark_color: color::GREEN,
            text_color: None,
            border_radius: 0.0,
        }
    }

    fn hovered(&self, style: &Self::Style, is_selected: bool) -> checkbox::Appearance {
        let active = self.active(style, is_selected);
        checkbox::Appearance { ..active }
    }
}

#[derive(Default)]
pub enum Button {
    #[default]
    Primary,
    Secondary,
    Destructive,
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
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 25.0,
                    border_width: 1.0,
                    border_color: color::GREY_7,
                    text_color: color::GREY_2,
                },
                Button::Secondary | Button::Border => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 25.0,
                    border_width: 1.0,
                    border_color: color::GREY_7,
                    text_color: color::GREY_2,
                },
                Button::Destructive => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 25.0,
                    border_width: 1.0,
                    border_color: color::RED,
                    text_color: color::RED,
                },
                Button::Transparent => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 25.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::GREY_2,
                },
                Button::TransparentBorder => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 25.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::WHITE,
                },
                Button::Menu(active) => {
                    if *active {
                        button::Appearance {
                            shadow_offset: iced::Vector::default(),
                            background: color::LIGHT_BLACK.into(),
                            border_radius: 25.0,
                            border_width: 0.0,
                            border_color: iced::Color::TRANSPARENT,
                            text_color: color::WHITE,
                        }
                    } else {
                        button::Appearance {
                            shadow_offset: iced::Vector::default(),
                            background: iced::Color::TRANSPARENT.into(),
                            border_radius: 25.0,
                            border_width: 0.0,
                            border_color: iced::Color::TRANSPARENT,
                            text_color: color::WHITE,
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
                    background: color::GREEN.into(),
                    border_radius: 25.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Secondary => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::GREEN.into(),
                    border_radius: 25.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Destructive => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::RED.into(),
                    border_radius: 25.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::LIGHT_BLACK,
                },
                Button::Transparent => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::GREY_7.into(),
                    border_radius: 25.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::GREY_2,
                },
                Button::TransparentBorder | Button::Border => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: iced::Color::TRANSPARENT.into(),
                    border_radius: 25.0,
                    border_width: 1.0,
                    border_color: color::GREEN,
                    text_color: color::WHITE,
                },
                Button::Menu(_) => button::Appearance {
                    shadow_offset: iced::Vector::default(),
                    background: color::LIGHT_BLACK.into(),
                    border_radius: 25.0,
                    border_width: 0.0,
                    border_color: iced::Color::TRANSPARENT,
                    text_color: color::WHITE,
                },
            },
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
                background: iced::Background::Color(iced::Color::TRANSPARENT),
                border_radius: 25.0,
                border_width: 1.0,
                border_color: color::GREY_7,
            },
            Form::Invalid => text_input::Appearance {
                background: iced::Background::Color(iced::Color::TRANSPARENT),
                border_radius: 25.0,
                border_width: 1.0,
                border_color: color::RED,
            },
        }
    }

    fn focused(&self, style: &Self::Style) -> text_input::Appearance {
        text_input::Appearance {
            ..self.active(style)
        }
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
            border_radius: 10.0,
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
                border_radius: 4.0,
            },
            color: color::BLACK,
            border_color: color::GREEN,
            border_width: 1.0,
        };
        slider::Appearance {
            rail_colors: (color::GREEN, iced::Color::TRANSPARENT),
            handle,
        }
    }
    fn hovered(&self, _style: &Self::Style) -> slider::Appearance {
        let handle = slider::Handle {
            shape: slider::HandleShape::Rectangle {
                width: 8,
                border_radius: 4.0,
            },
            color: color::GREEN,
            border_color: color::GREEN,
            border_width: 1.0,
        };
        slider::Appearance {
            rail_colors: (color::GREEN, iced::Color::TRANSPARENT),
            handle,
        }
    }
    fn dragging(&self, _style: &Self::Style) -> slider::Appearance {
        let handle = slider::Handle {
            shape: slider::HandleShape::Rectangle {
                width: 8,
                border_radius: 4.0,
            },
            color: color::GREEN,
            border_color: color::GREEN,
            border_width: 1.0,
        };
        slider::Appearance {
            rail_colors: (color::GREEN, iced::Color::TRANSPARENT),
            handle,
        }
    }
}
