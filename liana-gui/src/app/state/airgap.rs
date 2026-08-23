use std::path::PathBuf;

use bip377::ll::{RequestId, REQUEST_ID_LEN};
use bwk_camera::capture::CameraInfo;
use iced::{
    widget::{column, row, Space},
    window, Length, Size, Subscription, Task,
};
use liana_ui::{
    component::{
        button, card, qr,
        text::new::{b5_medium, caption},
    },
    icon, theme,
    widget::{Element, SpaceExt},
};

use crate::{
    airgap::{
        animation::AnimatedQr,
        camera::{request_camera_access, CameraFailure, CameraScanner},
        display::DisplaySettings,
        error::Error,
        exchange::{Answer, Ask, Exchange},
    },
    app::settings::global::GlobalSettings,
    dir::LianaDirectory,
    t,
};

const SETTINGS_ICON_SIZE: u32 = 12;
/// The share of the window's shorter side an enlarged request takes.
const ENLARGED_SHARE: f32 = 0.8;

#[derive(Debug, Clone)]
pub enum AirgapAction {
    /// Shows the request alone, as large as the window allows, or back again.
    ToggleEnlarged,
    WindowSize(Size),
    ToggleSettings,
    Density(u8),
    FrameRate(u8),
    Brightness(u8),
    /// A slider was let go: the settings are saved for the next exchange.
    SaveSettings,
    Tick,
    Scan,
    Cameras(Result<Vec<CameraInfo>, CameraFailure>),
    SelectCamera(usize),
    PollCamera,
    Back,
    Cancel,
}

#[derive(Debug, Clone)]
pub enum AirgapOutcome {
    Answer(Box<Answer>),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    ShowRequest,
    StartingCamera,
    Scanning,
    Done,
}

/// One QR exchange with an air-gapped signer, shared by every flow: Liana shows
/// the request as animated frames, then scans the signer's answer back.
pub struct AirgapModal {
    title: String,
    exchange: Exchange,
    animation: AnimatedQr,
    phase: Phase,
    scanner: Option<CameraScanner>,
    display: DisplaySettings,
    settings_path: PathBuf,
    settings_expanded: bool,
    enlarged: bool,
    window: Option<Size>,
    error: Option<Error>,
    camera_error: Option<CameraFailure>,
    outcome: Option<AirgapOutcome>,
}

impl AirgapModal {
    /// The display settings are read from, and saved to, the global settings in
    /// `datadir`.
    pub fn new(title: String, ask: Ask, datadir: &LianaDirectory) -> Result<Self, Error> {
        let settings_path = GlobalSettings::path(datadir);
        let display = GlobalSettings::load_airgap_display(&settings_path);
        let exchange = Exchange::new(ask, request_id(), display.frame_size())?;
        Ok(Self {
            title,
            animation: AnimatedQr::new(
                exchange.frames().to_vec(),
                display.brightness(),
                display.frame_interval(),
            )?,
            exchange,
            phase: Phase::ShowRequest,
            scanner: None,
            display,
            settings_path,
            settings_expanded: false,
            enlarged: false,
            window: None,
            error: None,
            camera_error: None,
            outcome: None,
        })
    }

    pub fn take_outcome(&mut self) -> Option<AirgapOutcome> {
        self.outcome.take()
    }

    pub fn subscription(&self) -> Subscription<AirgapAction> {
        let animation = if self.phase == Phase::ShowRequest {
            self.animation.subscription().map(|_| AirgapAction::Tick)
        } else {
            Subscription::none()
        };
        let camera = self
            .scanner
            .as_ref()
            .map_or(Subscription::none(), |scanner| {
                scanner.subscription().map(|_| AirgapAction::PollCamera)
            });
        let window = if self.enlarged {
            window::resize_events().map(|(_, size)| AirgapAction::WindowSize(size))
        } else {
            Subscription::none()
        };
        Subscription::batch([animation, camera, window])
    }

    pub fn update(&mut self, message: AirgapAction) -> Task<AirgapAction> {
        match message {
            AirgapAction::Tick => self.animation.tick(),
            AirgapAction::ToggleEnlarged => {
                self.enlarged = !self.enlarged;
                if self.enlarged {
                    return window::latest()
                        .and_then(window::size)
                        .map(AirgapAction::WindowSize);
                }
            }
            AirgapAction::WindowSize(size) => self.window = Some(size),
            AirgapAction::ToggleSettings => self.settings_expanded = !self.settings_expanded,
            AirgapAction::Density(density) => {
                if density != self.display.density {
                    self.display.density = density;
                    if let Err(error) = self
                        .exchange
                        .reframe(self.display.frame_size())
                        .and_then(|()| self.animation.set_frames(self.exchange.frames().to_vec()))
                    {
                        self.error = Some(error);
                    }
                }
            }
            AirgapAction::FrameRate(frame_rate) => {
                self.display.frame_rate = frame_rate;
                self.animation.set_interval(self.display.frame_interval());
            }
            AirgapAction::Brightness(brightness) => {
                self.display.brightness = brightness;
                self.animation.set_brightness(self.display.brightness());
            }
            AirgapAction::SaveSettings => {
                if let Err(e) =
                    GlobalSettings::update_airgap_display(&self.settings_path, self.display)
                {
                    log::warn!("Air-gap: could not save the display settings: {e}");
                }
            }
            AirgapAction::Scan => {
                self.stop_camera();
                self.phase = Phase::StartingCamera;
                self.clear_errors();
                return Task::perform(request_camera_access(), AirgapAction::Cameras);
            }
            AirgapAction::Cameras(result) => {
                // the permission callback may land after the modal moved on
                if self.phase != Phase::StartingCamera {
                    return Task::none();
                }
                match result.and_then(CameraScanner::start) {
                    Ok(scanner) => {
                        self.scanner = Some(scanner);
                        self.phase = Phase::Scanning;
                    }
                    Err(failure) => self.give_up_on_camera(failure),
                }
            }
            AirgapAction::SelectCamera(index) => {
                self.clear_errors();
                if let Some(Err(failure)) = self.scanner.as_mut().map(|s| s.select(index)) {
                    self.give_up_on_camera(failure);
                }
            }
            AirgapAction::PollCamera => self.poll_camera(),
            AirgapAction::Back => {
                self.stop_camera();
                self.clear_errors();
                self.phase = Phase::ShowRequest;
            }
            AirgapAction::Cancel => self.finish(AirgapOutcome::Cancelled),
        }
        Task::none()
    }

    /// The exchange emits only its own actions; a caller maps them into whatever
    /// message its own flow speaks.
    pub fn view<M: Clone + 'static>(
        &self,
        to_message: impl Fn(AirgapAction) -> M + Clone + 'static,
    ) -> Element<'_, M> {
        let warning = self
            .error
            .as_ref()
            .map(ToString::to_string)
            .or_else(|| self.camera_error.as_ref().map(ToString::to_string))
            .map(|error| card::error(t!("airgap-error-title"), error));

        if let (true, Phase::ShowRequest, Some(window)) = (self.enlarged, self.phase, self.window) {
            let side = window.width.min(window.height) * ENLARGED_SHARE;
            return self
                .animation
                .enlarged(side as u32, to_message(AirgapAction::ToggleEnlarged));
        }

        let body = match self.phase {
            Phase::ShowRequest => self.request_view(to_message.clone()),
            Phase::StartingCamera => caption(t!("airgap-requesting-camera")).into(),
            Phase::Scanning => self.scanning_view(to_message.clone()),
            Phase::Done => Space::fill_width().into(),
        };

        liana_ui::component::modal::modal_view(
            Some(&self.title),
            None,
            Some(to_message(AirgapAction::Cancel)),
            liana_ui::component::modal::ModalWidth::XL,
            column![warning, body].spacing(20).width(Length::Fill),
        )
    }

    fn request_view<M: Clone + 'static>(
        &self,
        to_message: impl Fn(AirgapAction) -> M + Clone + 'static,
    ) -> Element<'_, M> {
        let request = self
            .animation
            .view(to_message(AirgapAction::ToggleEnlarged));

        let display = self.display;
        let density = qr::setting_slider(
            t!("qr-density"),
            DisplaySettings::DENSITIES,
            display.density,
            {
                let to_message = to_message.clone();
                move |density| to_message(AirgapAction::Density(density))
            },
            to_message(AirgapAction::SaveSettings),
        );
        let frame_rate = qr::setting_slider(
            t!("qr-frame-rate"),
            DisplaySettings::FRAME_RATES,
            display.frame_rate,
            {
                let to_message = to_message.clone();
                move |rate| to_message(AirgapAction::FrameRate(rate))
            },
            to_message(AirgapAction::SaveSettings),
        );
        let brightness = qr::setting_slider(
            t!("qr-brightness", percent = display.brightness().percent()),
            qr::Brightness::MIN..=qr::Brightness::MAX,
            display.brightness().percent(),
            {
                let to_message = to_message.clone();
                move |percent| to_message(AirgapAction::Brightness(percent))
            },
            to_message(AirgapAction::SaveSettings),
        );
        let chevron = if self.settings_expanded {
            icon::collapsed_icon()
        } else {
            icon::collapse_icon()
        };
        let settings_toggle = button::btn_optional_section(
            row![
                caption(t!("qr-settings")),
                chevron
                    .size(SETTINGS_ICON_SIZE)
                    .style(theme::text::secondary),
            ]
            .spacing(5)
            .align_y(iced::Alignment::Center),
            to_message(AirgapAction::ToggleSettings),
        );
        let settings = self
            .settings_expanded
            .then(|| column![density, frame_rate, brightness].spacing(15));

        column![
            b5_medium(t!("airgap-show-request")),
            request,
            button::btn_scan_response(Some(to_message(AirgapAction::Scan))),
            settings_toggle,
            settings,
        ]
        .spacing(15)
        .align_x(iced::Alignment::Center)
        .width(Length::Fill)
        .into()
    }

    fn scanning_view<M: Clone + 'static>(
        &self,
        to_message: impl Fn(AirgapAction) -> M + Clone + 'static,
    ) -> Element<'_, M> {
        column![
            b5_medium(t!("airgap-point-camera")),
            self.scanner.as_ref().map(|scanner| {
                let to_message = to_message.clone();
                scanner.view(move |index| to_message(AirgapAction::SelectCamera(index)))
            }),
            button::btn_secondary(
                None,
                t!("airgap-back-to-request"),
                button::BtnWidth::Auto,
                Some(to_message(AirgapAction::Back))
            ),
        ]
        .spacing(15)
        .align_x(iced::Alignment::Center)
        .width(Length::Fill)
        .into()
    }

    fn poll_camera(&mut self) {
        let payload = match self.scanner.as_mut().and_then(CameraScanner::poll) {
            Some(Ok(payload)) => payload,
            Some(Err(failure)) => return self.give_up_on_camera(failure),
            None => return,
        };
        match self.exchange.read(&payload) {
            Ok(answer) => {
                self.stop_camera();
                self.finish(AirgapOutcome::Answer(Box::new(answer)));
            }
            // The answer to an earlier exchange, still on the signer's screen.
            // That is the ordinary way a scan starts, not a failure, so say what
            // it is and keep looking rather than sending the user back a step.
            Err(error @ Error::RequestIdMismatch) => self.error = Some(error),
            Err(error) => {
                self.stop_camera();
                self.error = Some(error);
                self.phase = Phase::ShowRequest;
            }
        }
    }

    /// The camera cannot be used, so send the user back to the request they can
    /// still show, rather than leaving them on a dead scanning screen.
    fn give_up_on_camera(&mut self, failure: CameraFailure) {
        self.stop_camera();
        self.camera_error = Some(failure);
        self.phase = Phase::ShowRequest;
    }

    fn stop_camera(&mut self) {
        self.scanner = None;
    }

    fn clear_errors(&mut self) {
        self.error = None;
        self.camera_error = None;
    }

    fn finish(&mut self, outcome: AirgapOutcome) {
        self.stop_camera();
        self.phase = Phase::Done;
        self.outcome = Some(outcome);
    }
}

/// The signer echoes this back so Liana can tell its own answer from a stale one
/// still on screen from an earlier exchange. Never the id a signer answers an
/// unreadable request with.
fn request_id() -> RequestId {
    loop {
        let mut id = [0u8; REQUEST_ID_LEN];
        getrandom::fill(&mut id).expect("the OS random source is required");
        let id = RequestId(id);
        if id != RequestId::UNREADABLE_REQUEST {
            return id;
        }
    }
}
