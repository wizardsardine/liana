use coincube_ui::{
    component::{button, text::*},
    icon, theme,
    widget::*,
};
use iced::{widget::Space, Alignment, Length, Task};
use std::time::Duration;

use crate::app::settings::CubeSettings;
use crate::pin_input;

pub struct PinEntry {
    cube: CubeSettings,
    pin_input: pin_input::PinInput,
    error: Option<String>,
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
    PinInput(pin_input::Message),
    Submit,
    Back,
    PinVerified,
}

impl PinEntry {
    pub fn new(cube: CubeSettings, on_success: PinEntrySuccess) -> Self {
        Self {
            cube,
            pin_input: pin_input::PinInput::new(),
            error: None,
            loading: false,
            on_success,
        }
    }

    pub fn cube(&self) -> &CubeSettings {
        &self.cube
    }

    pub fn pin(&self) -> String {
        self.pin_input.value()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::PinInput(msg) => {
                self.error = None;
                self.pin_input.update(msg).map(Message::PinInput)
            }
            Message::Submit => {
                if self.loading {
                    return Task::none();
                }

                if !self.pin_input.is_complete() {
                    self.error = Some("Please enter all 4 digits".to_string());
                    return Task::none();
                }

                let pin = self.pin_input.value();

                if self.cube.verify_pin(&pin) {
                    self.loading = true;
                    Task::perform(async {}, |_| Message::PinVerified)
                } else {
                    self.error = Some("Incorrect PIN. Please try again.".to_string());
                    self.pin_input.clear();
                    Task::none()
                }
            }
            Message::Back | Message::PinVerified => Task::none(),
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

        let title = h3(format!("Enter PIN for {}", self.cube.name));

        let mut content = Column::new()
            .spacing(30)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .push(title)
            .push(self.pin_input.view().map(Message::PinInput));

        if let Some(error) = &self.error {
            content = content.push(p1_regular(error).style(theme::text::error));
        }

        let can_submit = !self.loading && self.pin_input.is_complete();

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
