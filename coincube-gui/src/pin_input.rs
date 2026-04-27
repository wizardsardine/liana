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

    /// Overwrite `self.digits[i]` with `new`, scrubbing the previous
    /// String's heap bytes first. Needed because plain assignment
    /// (`self.digits[i] = new`) drops the old `String` without
    /// zeroizing, leaving the digit on the heap until the allocator
    /// reuses that region. Callers with a digit character in hand use
    /// this instead of direct assignment.
    fn replace_digit(&mut self, i: usize, new: String) {
        if i >= self.digits.len() {
            return;
        }
        self.digits[i].zeroize();
        self.digits[i] = new;
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DigitChanged(index, value) => {
                if index >= self.digits.len() {
                    return Task::none();
                }
                // Previous digit's state before we overwrite it. Capture
                // just the booleans/lengths we need — cloning the String
                // itself (the prior implementation did `old_value =
                // self.digits[index].clone()`) would copy the plaintext
                // digit onto a throwaway heap buffer that then dropped
                // un-scrubbed at the end of this function.
                let old_was_empty = self.digits[index].is_empty();
                let old_len = self.digits[index].len();
                if value.is_empty() {
                    self.replace_digit(index, value);
                    if index > 0 {
                        // this deletes the number and also moves the cursor to the previous field
                        // If the field was already empty, also clear the previous field
                        if old_was_empty {
                            // Scrub the previous digit in place rather
                            // than reassigning `String::new()`, which
                            // would drop the old plaintext buffer
                            // un-scrubbed.
                            self.digits[index - 1].zeroize();
                        }
                        return focus_previous();
                    }
                } else if !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()) {
                    // Smart fill logic: determine which field to update
                    if value.len() == 1 {
                        // Simple case: typing in empty field
                        self.replace_digit(index, value);
                        if index < 3 {
                            return focus_next();
                        }
                    } else if value.len() > old_len {
                        // User typed in a field that already has content
                        // Extract the newly typed character
                        if let Some(last_char) = value.chars().last() {
                            if last_char.is_ascii_digit() {
                                let new_digit = last_char.to_string();

                                // Smart fill: if current field has content and next field is empty, fill next
                                if !old_was_empty && index < 3 && self.digits[index + 1].is_empty()
                                {
                                    self.replace_digit(index + 1, new_digit);
                                    return focus_next();
                                } else {
                                    // Otherwise replace current field
                                    self.replace_digit(index, new_digit);
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
    use super::{Message, PinInput};
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

    #[test]
    fn replace_digit_scrubs_previous_buffer() {
        let mut pin = PinInput::new();
        pin.digits = ["7".to_string(), String::new(), String::new(), String::new()];
        pin.replace_digit(0, "3".to_string());
        // End state: only the new digit is visible.
        assert_eq!(pin.digits[0], "3");
        // The `zeroize::Zeroize for String` impl scrubs bytes then
        // clears length before we drop the old allocation, so by the
        // time `replace_digit` returns no plaintext "7" is reachable
        // through `self.digits` — the only digit accessible is "3".
        assert_eq!(pin.value(), "3");
    }

    #[test]
    fn digit_changed_smart_fill_scrubs_previous_digit() {
        // Reproduces the `update()` path where the user types into a
        // field that already has a digit AND the next field is empty:
        // the typed character goes into the next slot, leaving the
        // current slot's previous digit in place. Nothing to scrub in
        // this branch directly, but we must not panic or drop data.
        let mut pin = PinInput::new();
        pin.digits[0] = "5".to_string();
        let _ = pin.update(Message::DigitChanged(0, "58".to_string()));
        assert_eq!(pin.digits[0], "5");
        assert_eq!(pin.digits[1], "8");
    }

    #[test]
    fn digit_changed_replace_scrubs_overwritten_digit() {
        // Path: `value.len() > old_len` with next-slot non-empty falls
        // through to "replace current field". Old buffer must be
        // scrubbed before the new one takes its place.
        let mut pin = PinInput::new();
        pin.digits[0] = "5".to_string();
        pin.digits[1] = "9".to_string(); // occupies next slot → no smart-fill
        let _ = pin.update(Message::DigitChanged(0, "52".to_string()));
        // Slot 0 replaced with "2", previous "5" scrubbed before the
        // replacement was assigned.
        assert_eq!(pin.digits[0], "2");
        assert_eq!(pin.digits[1], "9");
    }

    #[test]
    fn digit_changed_backspace_on_empty_clears_prior_slot() {
        // Path: `value.is_empty()` + `old_was_empty` → zeroize prior
        // slot. Before this fix the prior slot was reassigned
        // `String::new()`, dropping its plaintext buffer un-scrubbed.
        let mut pin = PinInput::new();
        pin.digits[0] = "4".to_string();
        pin.digits[1] = String::new();
        let _ = pin.update(Message::DigitChanged(1, String::new()));
        assert!(pin.digits[0].is_empty(), "prior slot should be cleared");
        assert!(pin.digits[1].is_empty());
    }
}
