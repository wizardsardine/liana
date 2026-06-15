use iced::widget::{image, Space};
use iced::{Alignment, Length, Task};

use coincube_ui::{
    component::{
        button,
        quote_display::{self, Quote, QuoteDisplayProps},
        text::{h3, p1_regular, text},
    },
    icon, theme,
    widget::{Column, Container, Element, Row},
};

use crate::app::settings::CubeSettings;
use crate::pin_input;

pub struct PinEntry {
    cube: CubeSettings,
    pin_input: pin_input::PinInput,
    error: Option<String>,
    loading: bool,
    // Store what to do after successful PIN entry
    pub on_success: PinEntrySuccess,
    /// This device's enrolled Connect duress account id, captured at
    /// construction so it can be carried explicitly through `DuressDetected`
    /// (Task A.1) rather than re-derived deep inside activation. `None` for a
    /// sovereign (no-Connect) enrollment.
    duress_account_id: Option<String>,
    loading_quote: Quote,
    loading_image_handle: image::Handle,
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
    /// The submitted PIN matched this Cube's **duress** PIN. Bubbles up to the
    /// tab state machine, which delegates to the duress orchestrator (wipe Cube
    /// data + server POST) and locks into the cryptic "Duress Mode Activated"
    /// screen. The parent intercepts this; it is never handled inside
    /// `PinEntry::update`.
    ///
    /// Carries this device's enrolled Connect duress `account_id` (`None` for
    /// sovereign) so the orchestrator receives it explicitly — see Task A.1.
    DuressDetected {
        account_id: Option<String>,
    },
}

/// Classification of a submitted PIN at Cube unlock.
enum PinOutcome {
    Unlock,
    Duress,
    Wrong,
}

impl PinEntry {
    pub fn new(
        cube: CubeSettings,
        on_success: PinEntrySuccess,
        duress_account_id: Option<String>,
    ) -> Self {
        let loading_quote = quote_display::random_quote("loading");
        let loading_image_handle = quote_display::image_handle_for_context("loading");
        Self {
            cube,
            pin_input: pin_input::PinInput::new(),
            error: None,
            loading: false,
            on_success,
            duress_account_id,
            loading_quote,
            loading_image_handle,
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
            Message::PinInput(pin_input::Message::Submit) => {
                // Enter key pressed in a PIN field — trigger submit
                self.update(Message::Submit)
            }
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

                // A Cube without a regular PIN has a permissive `verify_pin`
                // (returns true for ANY input), which would swallow the duress
                // PIN before `verify_duress_pin` ran. So when there's no regular
                // PIN, check duress FIRST. When there IS a regular PIN, check it
                // first (the happy path is never shadowed, and duress vs. wrong
                // both take two argon2 verifies so they're timing-indistinct).
                let outcome = if self.cube.has_pin() {
                    if self.cube.verify_pin(&pin) {
                        PinOutcome::Unlock
                    } else if self.cube.verify_duress_pin(&pin) {
                        PinOutcome::Duress
                    } else {
                        PinOutcome::Wrong
                    }
                } else if self.cube.verify_duress_pin(&pin) {
                    PinOutcome::Duress
                } else {
                    // No regular PIN → any non-duress input unlocks.
                    PinOutcome::Unlock
                };

                match outcome {
                    PinOutcome::Unlock => {
                        self.loading = true;
                        Task::perform(async {}, |_| Message::PinVerified)
                    }
                    PinOutcome::Duress => {
                        // Clear the buffer and bubble up the enrolled account id
                        // so the parent can drive the orchestrator. Show the
                        // neutral loading screen during the brief async
                        // activation gap: it's identical to a normal unlock (so
                        // it reveals nothing to an onlooker) and blocks further
                        // input until we lock into the cryptic screen.
                        self.pin_input.clear();
                        self.loading = true;
                        let account_id = self.duress_account_id.clone();
                        Task::done(Message::DuressDetected { account_id })
                    }
                    PinOutcome::Wrong => {
                        self.error = Some("Incorrect PIN. Please try again.".to_string());
                        self.pin_input.clear();
                        Task::none()
                    }
                }
            }
            // `DuressDetected` is intercepted by the parent (tab state machine);
            // if it ever reaches here it's a no-op.
            Message::Back | Message::PinVerified | Message::DuressDetected { .. } => Task::none(),
        }
    }

    pub fn view(&self) -> Element<Message> {
        if self.loading {
            // Full-screen loading with Kage quote while BreezClient loads
            return Container::new(
                Column::new()
                    .width(Length::Fill)
                    .spacing(20)
                    .align_x(Alignment::Center)
                    .push(Space::new().height(Length::Fill))
                    .push(quote_display::display(&QuoteDisplayProps::new(
                        "loading",
                        &self.loading_quote,
                        &self.loading_image_handle,
                    )))
                    .push(crate::loading::loading_indicator(None))
                    .push(text("Loading your Cube...").style(theme::text::secondary))
                    .push(Space::new().height(Length::Fill)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(50)
            .into();
        }

        let back_button = button::secondary(Some(icon::previous_icon()), "Back")
            .width(Length::Fixed(150.0))
            .on_press(Message::Back);

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

        let can_submit = self.pin_input.is_complete();

        let submit_button = button::primary(None, "Submit")
            .width(Length::Fixed(200.0))
            .on_press_maybe(if can_submit {
                Some(Message::Submit)
            } else {
                None
            });

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
