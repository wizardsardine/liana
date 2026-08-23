use std::path::PathBuf;

use bip377::{
    backend::rust_bitcoin::RustBitcoin,
    ll::{RequestId, REQUEST_ID_LEN},
    request,
};
use bwk_camera::capture::CameraInfo;
use bwk_qr::config::QrDensity;
use iced::{Subscription, Task};
use liana::miniscript::bitcoin::bip32::Fingerprint;
use liana_ui::component::qr;

use crate::{
    airgap::{
        animation::AnimatedQr,
        camera::{request_camera_access, CameraEvent, CameraFailure, CameraScanner},
        device::{Answer, Exchange},
        error::Error,
    },
    app::settings::global::GlobalSettings,
    dir::LianaDirectory,
};

#[derive(Debug, Clone)]
pub enum Message {
    ToggleEnlarged,
    ToggleSettings,
    Density(QrDensity),
    FrameRate(f32), // Frames per second.
    Brightness(qr::Brightness),
    SaveSettings,
    Next,
    Scan,
    Cameras(Result<Vec<CameraInfo>, CameraFailure>),
    SelectCamera(String),
    Camera(CameraEvent),
    Back,
    Cancel,
}

#[derive(Debug, Clone)]
pub enum AirgapOutcome {
    Answer(Box<Answer>),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    ShowRequest,
    Scanning,
    Done,
}

pub struct AirgapModal {
    pub fingerprint: Option<Fingerprint>,
    pub title: String,
    pub exchange: Exchange,
    pub animation: AnimatedQr,
    pub phase: Phase,
    pub scanner: Option<CameraScanner>,
    pub cameras: Result<Vec<CameraInfo>, CameraFailure>,
    camera: Option<String>,
    settings_path: PathBuf,
    pub settings_expanded: bool,
    pub enlarged: bool,
    pub error: Option<Error>,
    pub outcome: Option<AirgapOutcome>,
}

impl AirgapModal {
    pub fn new(
        fingerprint: Option<Fingerprint>,
        title: String,
        request: request::Body<RustBitcoin>,
        datadir: &LianaDirectory,
    ) -> Result<(Self, Task<Message>), Error> {
        let settings_path = GlobalSettings::path(datadir);
        let exchange = Exchange::new(request, request_id());
        let modal = Self {
            fingerprint,
            title,
            animation: AnimatedQr::new(
                exchange.payload()?,
                GlobalSettings::load_airgap_display(&settings_path),
            )?,
            camera: GlobalSettings::load_airgap_camera(&settings_path),
            exchange,
            phase: Phase::ShowRequest,
            scanner: None,
            cameras: Ok(Vec::new()),
            settings_path,
            settings_expanded: false,
            enlarged: false,
            error: None,
            outcome: None,
        };
        Ok((
            modal,
            Task::perform(request_camera_access(), Message::Cameras),
        ))
    }

    pub fn subscription(&self) -> Subscription<Message> {
        match (&self.phase, &self.scanner) {
            (Phase::ShowRequest, _) => self.animation.subscription().map(|_| Message::Next),
            (Phase::Scanning, Some(scanner)) => scanner.subscription().map(Message::Camera),
            _ => Subscription::none(),
        }
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Next => self.animation.next(),
            Message::ToggleEnlarged => self.enlarged = !self.enlarged,
            Message::ToggleSettings => self.settings_expanded = !self.settings_expanded,
            Message::Density(density) => self.animation.set_density(density),
            Message::FrameRate(frame_rate) => self.animation.set_frame_rate(frame_rate),
            Message::Brightness(brightness) => self.animation.set_brightness(brightness),
            Message::SaveSettings => self.save_settings(),
            Message::Scan => self.start_scan(),
            Message::Cameras(cameras) => self.cameras = cameras,
            Message::SelectCamera(name) => self.select_camera(name),
            Message::Camera(event) => self.on_camera_event(event),
            Message::Back => self.back_to_request(),
            Message::Cancel => self.finish(AirgapOutcome::Cancelled),
        }
        Task::none()
    }

    fn on_camera_event(&mut self, event: CameraEvent) {
        let payload = match self
            .scanner
            .as_mut()
            .and_then(|scanner| scanner.update(event))
        {
            Some(Ok(payload)) => payload,
            Some(Err(failure)) => return self.give_up_on_camera_event(failure),
            None => return,
        };
        match self.exchange.read(&payload) {
            Ok(answer) => {
                self.stop_camera();
                self.finish(AirgapOutcome::Answer(Box::new(answer)));
            }
            Err(error @ Error::RequestIdMismatch) => self.error = Some(error),
            Err(error) => {
                self.stop_camera();
                self.error = Some(error);
                self.phase = Phase::ShowRequest;
            }
        }
    }

    fn start_scan(&mut self) {
        self.error = None;
        match self.camera().map(CameraScanner::new) {
            Ok(scanner) => {
                self.scanner = Some(scanner);
                self.phase = Phase::Scanning;
            }
            Err(failure) => self.give_up_on_camera_event(failure),
        }
    }

    fn select_camera(&mut self, name: String) {
        if let Err(e) = GlobalSettings::update_airgap_camera(&self.settings_path, &name) {
            log::warn!("could not save the camera: {e}");
        }
        self.camera = Some(name);
    }

    fn back_to_request(&mut self) {
        self.stop_camera();
        self.error = None;
        self.phase = Phase::ShowRequest;
    }

    fn give_up_on_camera_event(&mut self, failure: CameraFailure) {
        self.stop_camera();
        self.error = Some(Error::Camera(failure));
        self.phase = Phase::ShowRequest;
    }

    pub fn camera(&self) -> Result<&CameraInfo, CameraFailure> {
        let cameras = self.cameras.as_ref().map_err(Clone::clone)?;
        cameras
            .iter()
            .find(|camera| Some(&camera.name) == self.camera.as_ref())
            .or(cameras.first())
            .ok_or(CameraFailure::Unavailable)
    }

    fn save_settings(&self) {
        if let Err(e) =
            GlobalSettings::update_airgap_display(&self.settings_path, self.animation.settings())
        {
            log::warn!("could not save the display settings: {e}");
        }
    }

    fn stop_camera(&mut self) {
        self.scanner = None;
    }

    fn finish(&mut self, outcome: AirgapOutcome) {
        self.stop_camera();
        self.phase = Phase::Done;
        self.outcome = Some(outcome);
    }
}

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
