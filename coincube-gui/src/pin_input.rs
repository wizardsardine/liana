use coincube_ui::{
    icon,
    widget::{Element, Row},
};
use iced::{
    widget::{operation::focus_next, operation::focus_previous},
    Alignment, Length, Task,
};
use zeroize::Zeroize;

/// Reusable 4-digit PIN input component.
///
/// Handles digit entry with auto-advance/retreat and show/hide toggle.
///
/// The `digits` buffers hold plaintext PIN characters while the user is
/// entering them. `Drop` + `clear()` both `zeroize()` each digit so the
/// heap allocations don't linger on the residual heap after the widget
/// is destroyed or reset. Iced's internal render/event buffers may
/// still hold short-lived copies — eliminating those is framework-side
/// and out of this widget's control.
#[derive(Default)]
pub struct PinInput {
    pub digits: [String; 4],
    pub hidden: bool,
}

#[derive(Debug, Clone)]
pub enum Message {
    DigitChanged(usize, String),
    ToggleShow,
    Submit,
}

impl PinInput {
    pub fn new() -> Self {
        Self {
            digits: [String::new(), String::new(), String::new(), String::new()],
            hidden: true,
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

    /// Clear all digits, scrubbing each heap buffer before it's
    /// reused. Leaves every digit as an empty `String`.
    pub fn clear(&mut self) {
        for d in &mut self.digits {
            d.zeroize();
        }
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
                    if index > 0 {
                        // this deletes the number and also moves the cursor to the previous field
                        // If the field was already empty, also clear the previous field
                        if old_value.is_empty() {
                            self.digits[index - 1] = String::new();
                        }
                        return focus_previous();
                    }
                } else if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
                    // Smart fill logic: determine which field to update
                    if value.len() == 1 {
                        // Simple case: typing in empty field
                        self.digits[index] = value;
                        if index < 3 {
                            return focus_next();
                        }
                    } else if value.len() > old_value.len() {
                        // User typed in a field that already has content
                        // Extract the newly typed character
                        if let Some(last_char) = value.chars().last() {
                            if last_char.is_ascii_digit() {
                                let new_digit = last_char.to_string();

                                // Smart fill: if current field has content and next field is empty, fill next
                                if !old_value.is_empty()
                                    && index < 3
                                    && self.digits[index + 1].is_empty()
                                {
                                    self.digits[index + 1] = new_digit;
                                    return focus_next();
                                } else {
                                    // Otherwise replace current field
                                    self.digits[index] = new_digit;
                                    if index < 3 {
                                        return focus_next();
                                    }
                                }
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::ToggleShow => {
                self.hidden = !self.hidden;
                Task::none()
            }
            Message::Submit => Task::none(),
        }
    }

    /// Render the PIN input row (4 digit fields + eye toggle).
    pub fn view(&self) -> Element<Message> {
        let mut pin_inputs = Row::new().spacing(15).align_y(Alignment::Center);

        for i in 0..4 {
            let input = iced::widget::text_input("", &self.digits[i])
                .size(30)
                .width(Length::Fixed(60.0))
                .align_x(iced::Alignment::Center)
                .padding(15)
                .secure(self.hidden)
                .on_input(move |v| Message::DigitChanged(i, v))
                .on_submit(Message::Submit);

            pin_inputs = pin_inputs.push(input);
        }

        let toggle_button = iced::widget::button(
            if self.hidden {
                icon::eye_icon()
            } else {
                icon::eye_slash_icon()
            }
            .align_x(iced::Alignment::Center)
            .align_y(iced::Alignment::Center),
        )
        .on_press(Message::ToggleShow)
        .width(Length::Fixed(50.0))
        .padding(15);

        Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(pin_inputs)
            .push(toggle_button)
            .into()
    }
}

impl Drop for PinInput {
    fn drop(&mut self) {
        // Scrub every digit buffer before the heap allocations are
        // handed back to the allocator, so a later allocation that
        // reuses the same region can't read the PIN.
        for d in &mut self.digits {
            d.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PinInput;
    use zeroize::Zeroize;

    #[test]
    fn clear_empties_every_digit() {
        let mut pin = PinInput::new();
        pin.digits = [
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string(),
        ];
        assert_eq!(pin.value(), "1234");
        pin.clear();
        for d in &pin.digits {
            assert!(d.is_empty(), "clear() left a digit non-empty: {:?}", d);
        }
        assert_eq!(pin.value(), "");
    }

    #[test]
    fn zeroize_on_digit_clears_underlying_bytes() {
        // Not a true "Drop observed memory" test (we can't look at
        // freed allocations portably), but it pins the invariant that
        // calling `zeroize()` on a digit leaves it empty — the same
        // primitive the `Drop` impl relies on.
        let mut s = "9".to_string();
        s.zeroize();
        assert!(s.is_empty());
    }
}
