use iced::{widget::text_input, Length};

use crate::{color, component::text, theme, util::Collection, widget::*};

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
    input: text_input::TextInput<'a, Message, iced::Renderer<theme::Theme>>,
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
            input: text_input::TextInput::new(placeholder, &value.value).on_input(on_change),
            warning: None,
            valid: value.valid,
        }
    }

    /// Creates a new [`Form`] that trims input values before applying the `on_change` function.
    ///
    /// It expects:
    /// - a placeholder
    /// - the current value
    /// - a function that produces a message when the [`Form`] changes
    pub fn new_trimmed<F>(placeholder: &str, value: &Value<String>, on_change: F) -> Self
    where
        F: 'static + Fn(String) -> Message,
    {
        Self {
            input: text_input::TextInput::new(placeholder, &value.value)
                .on_input(move |s| on_change(s.trim().to_string())),
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
                    form.input.style(theme::Form::Invalid)
                } else {
                    form.input
                })
                .push_maybe(if !form.valid {
                    form.warning
                        .map(|message| text::caption(message).style(color::RED))
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
