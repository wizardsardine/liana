use std::fmt::Display;

use crate::{
    color,
    component::text,
    theme,
    widget::{text_input, *},
};

use bitcoin::Denomination;
use iced::{Length, Padding};

#[derive(Debug, Clone)]
pub struct Value<T> {
    pub value: T,
    pub warning: Option<&'static str>,
    pub valid: bool,
}

impl std::default::Default for Value<String> {
    fn default() -> Self {
        Self {
            value: "".to_string(),
            warning: None,
            valid: true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FormSize {
    Normal,
    Compact,
}

impl FormSize {
    const TEXT_SIZE: f32 = 16.0;
    const LINE_HEIGHT: f32 = 1.3;

    fn height(self) -> f32 {
        match self {
            FormSize::Normal => 40.0,
            FormSize::Compact => 32.0,
        }
    }

    fn padding(self) -> Padding {
        let v = (self.height() - Self::TEXT_SIZE * Self::LINE_HEIGHT) / 2.0;
        Padding {
            top: v,
            bottom: v,
            left: 10.0,
            right: 10.0,
        }
    }
}

fn default_styled<'a, M: 'a + Clone>(input: TextInput<'a, M>) -> TextInput<'a, M> {
    input
        .size(FormSize::TEXT_SIZE as u32)
        .padding(FormSize::Normal.padding())
        .style(theme::text_input::form)
}

pub struct Form<'a, Message> {
    input: TextInput<'a, Message>,
    warning: Option<&'a str>,
    valid: bool,
    label: Option<Element<'a, Message>>,
    fee: bool,
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
    pub fn new<F>(placeholder: impl Display, value: &Value<String>, on_change: F) -> Self
    where
        F: 'static + Fn(String) -> Message,
    {
        Self {
            input: default_styled(
                text_input::TextInput::new(placeholder, &value.value).on_input(on_change),
            ),
            warning: value.warning,
            valid: value.valid,
            label: None,
            fee: false,
        }
    }

    /// Creates a new [`Form`] that has a disabled input.
    ///
    /// It expects:
    /// - a placeholder
    /// - the current value
    pub fn new_disabled(placeholder: &str, value: &Value<String>) -> Self {
        Self {
            input: default_styled(text_input::TextInput::new(placeholder, &value.value)),
            warning: None, // no warning for disabled form
            valid: value.valid,
            label: None,
            fee: false,
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
            input: default_styled(
                text_input::TextInput::new(placeholder, &value.value)
                    .on_input(move |s| on_change(s.trim().to_string())),
            ),
            warning: value.warning,
            valid: value.valid,
            label: None,
            fee: false,
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
        F: 'static + Fn(String) -> Message + Clone,
    {
        let on_change_clone = on_change.clone();
        Self {
            input: default_styled(
                text_input::TextInput::new(placeholder, &value.value)
                    .on_input(move |s| {
                        if bitcoin::Amount::from_str_in(&s, Denomination::Bitcoin).is_ok()
                        || s.is_empty()
                        // In order to allow the user to fix an invalid pasted value, we allow deletion
                        // even if the result is still invalid.
                        // Note that all invalid characters must be deleted before the user can enter
                        // new valid values.
                        || s.chars().count() < value.value.chars().count()
                        {
                            on_change(s)
                        } else {
                            on_change(value.value.clone())
                        }
                    })
                    .on_paste(move |pasted| {
                        // Keep the entire pasted content and perform any required checks or modifications
                        // in the on_change message handler.
                        on_change_clone(pasted)
                    }),
            ),
            warning: value.warning,
            valid: value.valid,
            label: None,
            fee: false,
        }
    }

    pub fn label(mut self, label: impl Display) -> Self {
        self.label = Some(text::new::b3(label).into());
        self
    }

    pub fn component_label(mut self, component: impl Into<Element<'a, Message>>) -> Self {
        self.label = Some(component.into());
        self
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
    pub fn size(mut self, size: u32) -> Self {
        self.input = self.input.size(size);
        self
    }

    /// Switch this form from the default [`FormSize::Normal`] to
    /// [`FormSize::Compact`].
    pub fn compact(mut self) -> Self {
        self.input = self.input.padding(FormSize::Compact.padding());
        self
    }

    /// Apply the fee look: a transparent input wrapped in a tinted, shadowed box.
    /// Combine with [`compact`](Self::compact) for the fee input.
    pub fn fee(mut self) -> Self {
        self.input = self.input.style(theme::text_input::fee);
        self.fee = true;
        self
    }

    /// Sets the message that should be produced when the [`Form`] is
    /// focused and the enter key is pressed.
    pub fn on_submit(mut self, message: Message) -> Self {
        self.input = self.input.on_submit(message);
        self
    }

    /// Sets the message that should be produced when the [`Form`] is
    /// focused and the enter key is pressed, if `Some`.
    pub fn on_submit_maybe(mut self, on_submit: Option<Message>) -> Self {
        self.input = self.input.on_submit_maybe(on_submit);
        self
    }

    /// Sets the [`Id`] of the [`Form`] input.
    pub fn id(mut self, id: impl Into<text_input::Id>) -> Self {
        self.input = self.input.id(id);
        self
    }
}

impl<'a, Message: 'a + Clone> From<Form<'a, Message>> for Element<'a, Message> {
    fn from(form: Form<'a, Message>) -> Element<'a, Message> {
        form.into_container().into()
    }
}

impl<'a, Message: 'a + Clone> Form<'a, Message> {
    /// Converts the [`Form`] into a [`Container`].
    pub fn into_container(self) -> Container<'a, Message> {
        let styled_input = if !self.valid {
            self.input.style(theme::text_input::invalid)
        } else {
            self.input
        };
        let input: Element<'a, Message> = if self.fee {
            Container::new(styled_input)
                .style(theme::container::form_field)
                .width(Length::Fill)
                .into()
        } else {
            styled_input.into()
        };
        Container::new(
            Column::new()
                .push_maybe(self.label)
                .push(input)
                .push_maybe(if !self.valid {
                    self.warning
                        .map(|message| text::caption(message).color(color::RED))
                } else {
                    None
                })
                .width(Length::Fill)
                .spacing(5),
        )
        .width(Length::Fill)
    }
}
