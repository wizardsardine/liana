use crate::widget::text_input;
use bitcoin::Denomination;
use iced::Length;

use crate::{color, component::text, theme, widget::*};

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
            input: text_input::TextInput::new(placeholder, &value.value).on_input(on_change),
            warning: None,
            valid: value.valid,
        }
    }

    /// Creates a new [`Form`] that has a disabled input.
    ///
    /// It expects:
    /// - a placeholder
    /// - the current value
    pub fn new_disabled(placeholder: &str, value: &Value<String>) -> Self {
        Self {
            input: text_input::TextInput::new(placeholder, &value.value),
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

    /// Creates a new [`Form`] that restrict input values to valid btc amount before applying the
    /// `on_change` function.
    /// It expects:
    /// - a placeholder
    /// - the current value
    /// - a function that produces a message when the [`Form`] changes
    pub fn new_amount_btc<F>(placeholder: &str, value: &'a Value<String>, on_change: F) -> Self
    where
        F: 'static + Fn(String) -> Message,
    {
        Self {
            input: text_input::TextInput::new(placeholder, &value.value).on_input(move |s| {
                if bitcoin::Amount::from_str_in(&s, Denomination::Bitcoin).is_ok() || s.is_empty() {
                    on_change(s)
                } else {
                    on_change(value.value.clone())
                }
            }),
            warning: None,
            valid: value.valid,
        }
    }

    /// Sets the [`Form`] with a warning message
    pub fn warning(mut self, warning: &'a str) -> Self {
        self.warning = Some(warning);
        self
    }

    /// Sets the [`Form`] with a warning message
    pub fn maybe_warning(mut self, warning: Option<&'a str>) -> Self {
        self.warning = warning;
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
                    form.input.style(theme::text_input::invalid)
                } else {
                    form.input
                })
                .push_maybe(if !form.valid {
                    form.warning
                        .map(|message| text::caption(message).color(color::RED))
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
