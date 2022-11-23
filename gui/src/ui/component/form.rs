use iced::{
    widget::{
        text_input::{Appearance, StyleSheet, TextInput},
        Column, Container,
    },
    Element, Length,
};

use crate::ui::{color, component::text::*, util::Collection};

#[derive(Debug, Clone)]
pub struct Value<T> {
    pub value: T,
    pub valid: bool,
}

impl std::default::Default for Value<String> {
    fn default() -> Self {
        Self {
            value: "".to_string(),
            valid: true,
        }
    }
}

pub struct Form<'a, Message> {
    input: TextInput<'a, Message>,
    warning: Option<&'a str>,
    valid: bool,
}

impl<'a, Message: 'a> Form<'a, Message>
where
    Message: Clone,
{
    /// Creates a new [`Form`].
    ///
    /// It expects:
    /// - a placeholder
    /// - the current value
    /// - a function that produces a message when the [`Form`] changes
    pub fn new<F>(placeholder: &str, value: &Value<String>, on_change: F) -> Self
    where
        F: 'static + Fn(String) -> Message,
    {
        Self {
            input: TextInput::new(placeholder, &value.value, on_change),
            warning: None,
            valid: value.valid,
        }
    }

    /// Sets the [`Form`] with a warning message
    pub fn warning(mut self, warning: &'a str) -> Self {
        self.warning = Some(warning);
        self
    }

    /// Sets the padding of the [`Form`].
    pub fn padding(mut self, units: u16) -> Self {
        self.input = self.input.padding(units);
        self
    }

    /// Sets the [`Form`] with a text size
    pub fn size(mut self, size: u16) -> Self {
        self.input = self.input.size(size);
        self
    }
}

impl<'a, Message: 'a + Clone> From<Form<'a, Message>> for Element<'a, Message> {
    fn from(form: Form<'a, Message>) -> Element<'a, Message> {
        Container::new(
            Column::new()
                .push(if !form.valid {
                    form.input.style(InvalidFormStyle)
                } else {
                    form.input
                })
                .push_maybe(if !form.valid {
                    form.warning
                        .map(|message| text(message).style(color::ALERT).small())
                } else {
                    None
                })
                .width(Length::Fill)
                .spacing(5),
        )
        .width(Length::Fill)
        .into()
    }
}

struct InvalidFormStyle;
impl StyleSheet for InvalidFormStyle {
    type Style = iced::Theme;
    fn active(&self, _style: &Self::Style) -> Appearance {
        Appearance {
            background: iced::Background::Color(color::FOREGROUND),
            border_radius: 5.0,
            border_width: 1.0,
            border_color: color::ALERT,
        }
    }

    fn focused(&self, style: &Self::Style) -> Appearance {
        Appearance {
            border_color: color::ALERT,
            ..self.active(style)
        }
    }

    fn placeholder_color(&self, _style: &Self::Style) -> iced::Color {
        iced::Color::from_rgb(0.7, 0.7, 0.7)
    }

    fn value_color(&self, _style: &Self::Style) -> iced::Color {
        iced::Color::from_rgb(0.3, 0.3, 0.3)
    }

    fn selection_color(&self, _style: &Self::Style) -> iced::Color {
        iced::Color::from_rgb(0.8, 0.8, 1.0)
    }
}

impl From<InvalidFormStyle> for Box<dyn StyleSheet<Style = iced::Theme>> {
    fn from(s: InvalidFormStyle) -> Box<dyn StyleSheet<Style = iced::Theme>> {
        Box::new(s)
    }
}

impl From<InvalidFormStyle> for iced::theme::TextInput {
    fn from(i: InvalidFormStyle) -> iced::theme::TextInput {
        iced::theme::TextInput::Custom(i.into())
    }
}
