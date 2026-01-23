use coincube_ui::{
    component::{button, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{operation::focus_next, operation::focus_previous, Space},
    Alignment, Length, Task,
};
use std::time::Duration;

use crate::app::settings::CubeSettings;

pub struct PinEntry {
    cube: CubeSettings,
    pin_digits: [String; 4],
    error: Option<String>,
    show_pin: bool,
    loading: bool,
    // Store what to do after successful PIN entry
    pub on_success: PinEntrySuccess,
}

pub enum PinEntrySuccess {
    LoadApp {
        datadir: crate::dir::CoincubeDirectory,
        config: crate::app::Config,
        network: coincube_core::miniscript::bitcoin::Network,
        // Optional Vault wallet loading fields
        internal_bitcoind: Option<crate::node::bitcoind::Bitcoind>,
        backup: Option<crate::backup::Backup>,
        wallet_settings: Option<crate::app::settings::WalletSettings>,
    },
}

#[derive(Debug, Clone)]
pub enum Message {
    DigitChanged(usize, String),
    Submit,
    Back,
    PinVerified,
    ToggleShowPin,
}

impl PinEntry {
    pub fn new(cube: CubeSettings, on_success: PinEntrySuccess) -> Self {
        Self {
            cube,
            pin_digits: [String::new(), String::new(), String::new(), String::new()],
            error: None,
            show_pin: false,
            loading: false,
            on_success,
        }
    }

    pub fn cube(&self) -> &CubeSettings {
        &self.cube
    }

    pub fn pin(&self) -> String {
        self.pin_digits.join("")
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::DigitChanged(index, value) => {
                // Only allow single digit (0-9)
                if value.is_empty() {
                    self.pin_digits[index] = value.clone();
                    self.error = None;

                    // Move to previous input when deleting (either from filled or empty field)
                    if index > 0 {
                        return focus_previous();
                    }
                } else if value.len() == 1 && value.chars().all(|c| c.is_ascii_digit()) {
                    self.pin_digits[index] = value;
                    self.error = None;

                    // Auto-advance to next input when digit is entered
                    if index < 3 {
                        return focus_next();
                    }
                }

                Task::none()
            }
            Message::Submit => {
                if self.loading {
                    return Task::none();
                }

                // Check if all digits are filled
                if self.pin_digits.iter().any(|d| d.is_empty()) {
                    self.error = Some("Please enter all 4 digits".to_string());
                    return Task::none();
                }

                let pin = self.pin_digits.join("");

                // Verify PIN
                if self.cube.verify_pin(&pin) {
                    self.loading = true;
                    Task::perform(async {}, |_| Message::PinVerified)
                } else {
                    self.error = Some("Incorrect PIN. Please try again.".to_string());
                    // Clear PIN on error
                    self.pin_digits = [String::new(), String::new(), String::new(), String::new()];
                    Task::none()
                }
            }
            Message::Back | Message::PinVerified => Task::none(),
            Message::ToggleShowPin => {
                self.show_pin = !self.show_pin;
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<Message> {
        let back_button = button::transparent(Some(icon::previous_icon()), "Previous")
            .on_press_maybe(if self.loading {
                None
            } else {
                Some(Message::Back)
            });

        let header = Row::new()
            .align_y(Alignment::Center)
            .push(Container::new(back_button).center_x(Length::FillPortion(2)))
            .push(Space::new().width(Length::FillPortion(8)))
            .push(Space::new().width(Length::FillPortion(2)));

        // Title with eye button
        let title = h3(format!("Enter PIN for {}", self.cube.name));

        // Small toggle visibility button (icon only) with padding to center icon
        let toggle_icon_button = button::secondary(
            Some(if self.show_pin {
                icon::eye_icon()
            } else {
                icon::eye_slash_icon()
            }),
            "",
        )
        .on_press(Message::ToggleShowPin)
        .width(Length::Fixed(50.0))
        .padding(iced::Padding::new(10.0).left(15.0));

        let title_row = Row::new()
            .spacing(10)
            .align_y(Alignment::Center)
            .push(title)
            .push(toggle_icon_button);

        // PIN input fields with masking
        let mut pin_inputs = Row::new().spacing(15).align_y(Alignment::Center);

        for i in 0..4 {
            let mut input = iced::widget::text_input("", &self.pin_digits[i])
                .on_input(move |v| Message::DigitChanged(i, v))
                .size(30) // Uniform font size for both modes
                .width(Length::Fixed(60.0));

            // Use secure mode when not showing PIN
            if !self.show_pin {
                input = input
                    .secure(true)
                    .padding(iced::Padding::new(15.0).left(25.0)); // More left padding to center asterisks
            } else {
                input = input.padding(iced::Padding::new(15.0).left(20.0)); // Original padding for numbers
            }

            pin_inputs = pin_inputs.push(input);
        }

        let mut content = Column::new()
            .spacing(30)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .push(title_row)
            .push(pin_inputs);

        if let Some(error) = &self.error {
            content = content.push(p1_regular(error).style(theme::text::error));
        }

        let can_submit = !self.loading && !self.pin_digits.iter().any(|d| d.is_empty());

        let submit_button = if self.loading {
            use coincube_ui::component::spinner;

            iced::widget::button(
                Container::new(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .push(text("Loading"))
                        .push(
                            Container::new(spinner::typing_text_carousel(
                                "...",
                                true,
                                Duration::from_millis(500),
                                text,
                            ))
                            .width(Length::Fixed(20.0)),
                        ),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .width(Length::Fixed(200.0))
            .height(Length::Fixed(44.0))
            .style(theme::button::primary)
        } else {
            button::primary(None, "Submit")
                .width(Length::Fixed(200.0))
                .on_press_maybe(if can_submit {
                    Some(Message::Submit)
                } else {
                    None
                })
        };

        content = content.push(submit_button);

        Container::new(
            Column::new()
                .width(Length::Fill)
                .push(Space::new().height(Length::Fixed(100.0)))
                .push(header)
                .push(Space::new().height(Length::Fixed(100.0)))
                .push(Container::new(content).center_x(Length::Fill)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(20)
        .into()
    }
}
