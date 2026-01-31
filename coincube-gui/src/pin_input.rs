use coincube_ui::{
    component::button,
    icon,
    widget::{Element, Row},
};
use iced::{
    widget::{operation::focus_next, operation::focus_previous},
    Alignment, Length, Task,
};

/// Reusable 4-digit PIN input component.
///
/// Handles digit entry with auto-advance/retreat and show/hide toggle.
#[derive(Default)]
pub struct PinInput {
    pub digits: [String; 4],
    pub show: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    DigitChanged(usize, String),
    ToggleShow,
}

impl PinInput {
    pub fn new() -> Self {
        Self {
            digits: [String::new(), String::new(), String::new(), String::new()],
            show: false,
        }
    }

    /// The joined 4-digit PIN value.
    pub fn value(&self) -> String {
        self.digits.join("")
    }

    /// Whether all 4 digits have been entered.
    pub fn is_complete(&self) -> bool {
        !self.digits.iter().any(|d| d.is_empty())
    }

    /// Clear all digits.
    pub fn clear(&mut self) {
        self.digits = [String::new(), String::new(), String::new(), String::new()];
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DigitChanged(index, value) => {
                if index >= self.digits.len() {
                    return Task::none();
                }
                let old_value = self.digits[index].clone();
                if value.is_empty() {
                    self.digits[index] = value;
                    if !old_value.is_empty() && index > 0 {
                        return focus_previous();
                    }
                } else if value.len() == 1 && value.chars().all(|c| c.is_ascii_digit()) {
                    self.digits[index] = value;
                    if index < 3 {
                        return focus_next();
                    }
                }
                Task::none()
            }
            Message::ToggleShow => {
                self.show = !self.show;
                Task::none()
            }
        }
    }

    /// Render the PIN input row (4 digit fields + eye toggle).
    pub fn view(&self) -> Element<Message> {
        let mut pin_inputs = Row::new().spacing(15).align_y(Alignment::Center);

        for i in 0..4 {
            let mut input = iced::widget::text_input("", &self.digits[i])
                .on_input(move |v| Message::DigitChanged(i, v))
                .size(30)
                .width(Length::Fixed(60.0));

            if !self.show {
                input = input
                    .secure(true)
                    .padding(iced::Padding::new(15.0).left(25.0));
            } else {
                input = input.padding(iced::Padding::new(15.0).left(20.0));
            }

            pin_inputs = pin_inputs.push(input);
        }

        let toggle_button = button::secondary(
            Some(if self.show {
                icon::eye_icon()
            } else {
                icon::eye_slash_icon()
            }),
            "",
        )
        .on_press(Message::ToggleShow)
        .width(Length::Fixed(50.0))
        .padding(iced::Padding::new(10.0).left(15.0));

        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(pin_inputs)
            .push(toggle_button)
            .into()
    }
}
