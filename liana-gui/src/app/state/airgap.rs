use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use iced::{
    alignment::Horizontal,
    widget::{image, progress_bar, qr_code, row, Column, Space},
    Alignment, Length, Subscription, Task,
};
use liana_ui::{
    component::{
        button, card,
        text::{p1_bold, p1_regular},
    },
    theme,
    widget::{Container, Element, SpaceExt},
};

use crate::{
    airgap::{
        encode_ur, request_camera_access, AirgappedRequest, AirgappedResponse, AnimatedQr,
        CameraDescriptor, CameraEvent, CameraFailure, CameraScanner, ExpectedResponse, QrDensity,
        ScanLimits, UrPayload,
    },
    app::{message::Message, view},
    export::get_path,
};

const QR_FRAMES_PER_SECOND: u8 = 5;
const QR_DISPLAY_SIZE: f32 = 440.0;
const QR_MODAL_WIDTH: f32 = 860.0;
const DEFAULT_MODAL_WIDTH: f32 = 560.0;
const MAX_JSON_RESPONSE_FILE_BYTES: usize = 24 * 1024;
const MAX_PSBT_RESPONSE_FILE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub enum AirgapAction {
    ShowQr,
    Tick,
    Pause,
    Resume,
    Restart,
    LessDense,
    MoreDense,
    ExportFile,
    FileExported(Result<Option<PathBuf>, String>),
    ImportResponse,
    FileImported(Result<Option<Vec<u8>>, String>),
    ScanResponse,
    Cameras(Result<Vec<CameraDescriptor>, CameraFailure>),
    SelectCamera(usize),
    PollCamera,
    Finish,
    Retry,
    Cancel,
}

#[derive(Debug, Clone)]
pub enum AirgapOutcome {
    Response(AirgappedResponse),
    Exported,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Choose,
    DisplayQr,
    Exported,
    StartingCamera,
    Scanning,
    Done,
}

/// Reusable QR exchange state for registration, address verification, and PSBT
/// signing, with file transport where the signer workflow supports it. It owns
/// and releases the camera and animated QR material.
pub struct AirgapModal {
    title: String,
    request: AirgappedRequest,
    expected: Option<ExpectedResponse>,
    filename: String,
    phase: Phase,
    animation: Option<AnimatedQr>,
    qr_data: Option<qr_code::Data>,
    qr_density: QrDensity,
    cameras: Vec<CameraDescriptor>,
    selected_camera: Option<usize>,
    scanner: Option<CameraScanner>,
    preview: Option<image::Handle>,
    progress: f32,
    detected_frames: u32,
    error: Option<String>,
    exported_path: Option<PathBuf>,
    outcome: Option<AirgapOutcome>,
}

impl AirgapModal {
    pub fn new(
        title: impl Into<String>,
        request: AirgappedRequest,
        filename: impl Into<String>,
    ) -> Self {
        let expected = request.expected_response();
        Self {
            title: title.into(),
            request,
            expected,
            filename: filename.into(),
            phase: Phase::Choose,
            animation: None,
            qr_data: None,
            qr_density: QrDensity::default(),
            cameras: Vec::new(),
            selected_camera: None,
            scanner: None,
            preview: None,
            progress: 0.0,
            detected_frames: 0,
            error: None,
            exported_path: None,
            outcome: None,
        }
    }

    pub fn take_outcome(&mut self) -> Option<AirgapOutcome> {
        self.outcome.take()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let animation = if self.phase == Phase::DisplayQr && self.animation.is_some() {
            iced::time::every(Duration::from_millis(50))
                .map(|_| Message::View(view::Message::Airgap(AirgapAction::Tick)))
        } else {
            Subscription::none()
        };
        let camera = if self.scanner.is_some() {
            iced::time::every(Duration::from_millis(33))
                .map(|_| Message::View(view::Message::Airgap(AirgapAction::PollCamera)))
        } else {
            Subscription::none()
        };
        Subscription::batch(vec![animation, camera])
    }

    pub fn update(&mut self, action: AirgapAction) -> Task<Message> {
        match action {
            AirgapAction::ShowQr => {
                self.rebuild_qr();
            }
            AirgapAction::Tick => self.refresh_qr(),
            AirgapAction::Pause => {
                if let Some(animation) = self.animation.as_mut() {
                    animation.pause();
                }
            }
            AirgapAction::Resume => {
                if let Some(animation) = self.animation.as_mut() {
                    animation.resume();
                }
            }
            AirgapAction::Restart => {
                if let Some(animation) = self.animation.as_mut() {
                    animation.restart();
                }
                self.error = None;
            }
            AirgapAction::LessDense => {
                if let Some(density) = self.qr_density.less_dense() {
                    self.set_qr_density(density);
                }
            }
            AirgapAction::MoreDense => {
                if let Some(density) = self.qr_density.more_dense() {
                    self.set_qr_density(density);
                }
            }
            AirgapAction::ExportFile => {
                if !self.request.supports_file_transport() {
                    self.error = Some("This exchange supports QR codes only".to_owned());
                    return Task::none();
                }
                let filename = self.filename.clone();
                let bytes = match self.request_file_bytes() {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        self.error = Some(error);
                        return Task::none();
                    }
                };
                return Task::perform(
                    async move {
                        let Some(path) = get_path(filename, true).await else {
                            return Ok(None);
                        };
                        fs::write(&path, bytes)
                            .map(|_| Some(path))
                            .map_err(|error| error.to_string())
                    },
                    |result| {
                        Message::View(view::Message::Airgap(AirgapAction::FileExported(result)))
                    },
                );
            }
            AirgapAction::FileExported(result) => match result {
                Ok(Some(path)) => {
                    self.exported_path = Some(path);
                    self.animation = None;
                    self.qr_data = None;
                    self.phase = Phase::Exported;
                    self.error = None;
                }
                Ok(None) => {}
                Err(error) => self.error = Some(error),
            },
            AirgapAction::ImportResponse => {
                if !self.request.supports_file_transport() {
                    self.error = Some("This exchange supports QR codes only".to_owned());
                    return Task::none();
                }
                let Some(expected) = self.expected else {
                    return Task::none();
                };
                let filename = self.response_filename();
                let maximum_bytes = match expected {
                    ExpectedResponse::SignedPsbt => MAX_PSBT_RESPONSE_FILE_BYTES,
                    _ => MAX_JSON_RESPONSE_FILE_BYTES,
                };
                return Task::perform(
                    async move {
                        let Some(path) = get_path(filename, false).await else {
                            return Ok(None);
                        };
                        read_bounded_file(&path, maximum_bytes).map(Some)
                    },
                    |result| {
                        Message::View(view::Message::Airgap(AirgapAction::FileImported(result)))
                    },
                );
            }
            AirgapAction::FileImported(result) => match result {
                Ok(Some(bytes)) => self.accept_file_response(bytes),
                Ok(None) => {}
                Err(error) => self.error = Some(error),
            },
            AirgapAction::ScanResponse => {
                if self.expected.is_none() {
                    return Task::none();
                }
                self.stop_camera();
                self.phase = Phase::StartingCamera;
                self.error = None;
                return Task::perform(request_camera_access(), |result| {
                    Message::View(view::Message::Airgap(AirgapAction::Cameras(result)))
                });
            }
            AirgapAction::Cameras(result) => {
                // Ignore a permission callback that arrived after this exchange
                // was cancelled or returned to the transport chooser.
                if self.phase != Phase::StartingCamera {
                    return Task::none();
                }
                match result {
                    Ok(cameras) if !cameras.is_empty() => {
                        self.cameras = cameras;
                        self.start_camera(0);
                    }
                    Ok(_) => {
                        self.phase = Phase::Choose;
                        self.error = Some("No camera is available".to_owned());
                    }
                    Err(error) => {
                        self.phase = Phase::Choose;
                        self.error = Some(error.to_string());
                    }
                }
            }
            AirgapAction::SelectCamera(index) => self.start_camera(index),
            AirgapAction::PollCamera => self.poll_camera(),
            AirgapAction::Finish => {
                if self.expected.is_none() {
                    self.finish(AirgapOutcome::Exported);
                }
            }
            AirgapAction::Retry => {
                self.stop_camera();
                self.animation = None;
                self.qr_data = None;
                self.error = None;
                self.phase = Phase::Choose;
            }
            AirgapAction::Cancel => self.finish(AirgapOutcome::Cancelled),
        }
        Task::none()
    }

    pub fn view(&self) -> Element<'_, view::Message> {
        let msg = |action| view::Message::Airgap(action);
        let header = row![
            p1_bold(&self.title),
            Space::with_width(Length::Fill),
            button::btn_modal_close(Some(msg(AirgapAction::Cancel)))
        ]
        .align_y(Alignment::Center);
        let mut body = Column::new()
            .push(header)
            .spacing(12)
            .align_x(Horizontal::Center);
        if let Some(error) = &self.error {
            body = body.push(card::error("Air-gapped signer", error.clone()));
        }
        body = match self.phase {
            Phase::Choose => {
                let body = body
                    .push(p1_regular(if self.request.supports_file_transport() {
                        "Choose how to exchange this request with your air-gapped signer."
                    } else {
                        "Exchange this request with your air-gapped signer using QR codes."
                    }))
                    .push(
                        button::primary(None, "Show animated QR code")
                            .width(Length::Fill)
                            .on_press(msg(AirgapAction::ShowQr)),
                    );
                if self.request.supports_file_transport() {
                    body.push(
                        button::secondary(None, "Export to microSD")
                            .width(Length::Fill)
                            .on_press(msg(AirgapAction::ExportFile)),
                    )
                } else {
                    body
                }
            }
            Phase::DisplayQr => {
                let mut controls = Column::new().spacing(12).align_x(Horizontal::Center);
                if let Some(data) = &self.qr_data {
                    body = body.push(
                        row![
                            qr_code::QRCode::<theme::Theme>::new(data).total_size(QR_DISPLAY_SIZE),
                            self.qr_controls(controls, &msg)
                        ]
                        .spacing(20)
                        .align_y(Alignment::Start),
                    );
                    body
                } else {
                    controls = controls.push(p1_regular("Preparing QR code…"));
                    body.push(controls)
                }
            }
            Phase::Exported => {
                let body = body.push(p1_regular(match &self.exported_path {
                    Some(path) => format!("Saved request to {}", path.display()),
                    None => "Request exported".to_owned(),
                }));
                self.response_buttons(body, &msg)
            }
            Phase::StartingCamera => body.push(p1_regular("Requesting camera access…")),
            Phase::Scanning => {
                let mut content = body.push(p1_regular("Scan the response shown by your signer."));
                if self.cameras.len() > 1 {
                    for (index, camera) in self.cameras.iter().enumerate() {
                        let label = if self.selected_camera == Some(index) {
                            format!("{} (active)", camera.name)
                        } else {
                            camera.name.clone()
                        };
                        content = content.push(
                            button::secondary(None, label)
                                .width(Length::Fill)
                                .on_press(msg(AirgapAction::SelectCamera(index))),
                        );
                    }
                }
                if let Some(preview) = &self.preview {
                    content = content.push(
                        image(preview.clone())
                            .width(Length::Fill)
                            .height(Length::Fixed(300.0)),
                    );
                }
                let status = if self.detected_frames == 0 {
                    "Looking for animated QR frames…".to_owned()
                } else {
                    format!(
                        "Scanning: {:.0}% · {} QR frame{} detected",
                        self.progress * 100.0,
                        self.detected_frames,
                        if self.detected_frames == 1 { "" } else { "s" }
                    )
                };
                content = content
                    .push(progress_bar(0.0..=1.0, self.progress.clamp(0.0, 1.0)))
                    .push(p1_regular(status));
                content.push(
                    button::secondary(None, "Back")
                        .width(Length::Fill)
                        .on_press(msg(AirgapAction::Retry)),
                )
            }
            Phase::Done => body,
        };
        let modal_width = if self.phase == Phase::DisplayQr {
            QR_MODAL_WIDTH
        } else {
            DEFAULT_MODAL_WIDTH
        };
        Container::new(body.width(Length::Fixed(modal_width)))
            .padding(20)
            .style(theme::card::modal)
            .into()
    }

    fn qr_controls<'a>(
        &'a self,
        mut controls: Column<'a, view::Message, theme::Theme>,
        msg: &impl Fn(AirgapAction) -> view::Message,
    ) -> Column<'a, view::Message, theme::Theme> {
        if let Some(animation) = &self.animation {
            let state = animation.state();
            if state.total_frames > 1 {
                controls = controls
                    .push(p1_regular(format!(
                        "Frame {} of {}",
                        state.frame + 1,
                        state.total_frames
                    )))
                    .push(if state.paused {
                        button::secondary(None, "Resume").on_press(msg(AirgapAction::Resume))
                    } else {
                        button::secondary(None, "Pause").on_press(msg(AirgapAction::Pause))
                    })
                    .push(
                        button::secondary(None, "Restart QR sequence")
                            .on_press(msg(AirgapAction::Restart)),
                    );
            }
        }
        controls = controls
            .push(p1_regular(format!(
                "QR density: {}",
                self.qr_density.label()
            )))
            .push(
                row![
                    button::secondary(None, "Less dense").on_press_maybe(
                        self.qr_density
                            .less_dense()
                            .map(|_| msg(AirgapAction::LessDense))
                    ),
                    button::secondary(None, "More dense").on_press_maybe(
                        self.qr_density
                            .more_dense()
                            .map(|_| msg(AirgapAction::MoreDense))
                    ),
                ]
                .spacing(10),
            );
        if self.request.supports_file_transport() {
            controls = controls.push(
                button::secondary(None, "Use microSD instead")
                    .width(Length::Fill)
                    .on_press(msg(AirgapAction::ExportFile)),
            );
        }
        self.response_buttons(controls, msg)
    }

    fn response_buttons<'a>(
        &'a self,
        body: Column<'a, view::Message, theme::Theme>,
        msg: &impl Fn(AirgapAction) -> view::Message,
    ) -> Column<'a, view::Message, theme::Theme> {
        if self.expected.is_none() {
            return body.push(
                button::primary(None, "Done")
                    .width(Length::Fill)
                    .on_press(msg(AirgapAction::Finish)),
            );
        }
        let body = body.push(
            button::primary(None, "Scan signer response")
                .width(Length::Fill)
                .on_press(msg(AirgapAction::ScanResponse)),
        );
        if self.request.supports_file_transport() {
            body.push(
                button::secondary(None, "Import response from microSD")
                    .width(Length::Fill)
                    .on_press(msg(AirgapAction::ImportResponse)),
            )
        } else {
            body
        }
    }

    fn refresh_qr(&mut self) {
        if let Some(frame) = self.animation.as_ref().and_then(AnimatedQr::frame) {
            match qr_code::Data::new(frame) {
                Ok(data) => self.qr_data = Some(data),
                Err(error) => self.error = Some(format!("Could not render QR code: {error}")),
            }
        }
    }

    fn rebuild_qr(&mut self) {
        let mut density = self.qr_density;
        loop {
            match self.build_qr(density) {
                Ok((animation, qr_data)) => {
                    self.qr_density = density;
                    self.animation = Some(animation);
                    self.qr_data = Some(qr_data);
                    self.phase = Phase::DisplayQr;
                    self.error = None;
                    return;
                }
                Err(crate::airgap::Error::TooManyFragments { .. }) => {
                    // Preserve QR availability for larger requests by choosing
                    // the least-dense preset that stays within the signer cap.
                    if let Some(denser) = density.more_dense() {
                        density = denser;
                        continue;
                    }
                    self.error = Some(
                        if self.request.supports_file_transport() {
                            "This request needs too many animated QR frames. Export it to microSD instead."
                        } else {
                            "This request needs too many animated QR frames."
                        }
                        .to_owned(),
                    );
                    return;
                }
                Err(crate::airgap::Error::PayloadTooLarge { .. }) => {
                    self.error = Some(
                        if self.request.supports_file_transport() {
                            "This request is too large for animated QR. Export it to microSD instead."
                        } else {
                            "This request is too large for animated QR."
                        }
                        .to_owned(),
                    );
                    return;
                }
                Err(error) => {
                    self.error = Some(error.to_string());
                    return;
                }
            }
        }
    }

    fn set_qr_density(&mut self, density: QrDensity) {
        match self.build_qr(density) {
            Ok((animation, qr_data)) => {
                self.qr_density = density;
                self.animation = Some(animation);
                self.qr_data = Some(qr_data);
                self.error = None;
            }
            Err(crate::airgap::Error::TooManyFragments { .. }) => {
                self.error = Some(
                    if self.request.supports_file_transport() {
                        "This request needs too many frames at that density. Choose a denser setting or use microSD."
                    } else {
                        "This request needs too many frames at that density. Choose a denser setting."
                    }
                    .to_owned(),
                );
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn build_qr(
        &self,
        density: QrDensity,
    ) -> Result<(AnimatedQr, qr_code::Data), crate::airgap::Error> {
        let payload = self.request.encode()?;
        let encoded = encode_ur(&payload, density.fragment_length())?;
        let animation = AnimatedQr::new(encoded, QR_FRAMES_PER_SECOND)?;
        let frame = animation.frame().ok_or(crate::airgap::Error::Empty)?;
        let qr_data = qr_code::Data::new(frame).map_err(|error| {
            crate::airgap::Error::InvalidUr(format!("could not render QR code: {error}"))
        })?;
        Ok((animation, qr_data))
    }

    fn request_file_bytes(&self) -> Result<Vec<u8>, String> {
        self.request
            .encode()
            .map(|payload| payload.data)
            .map_err(|error| error.to_string())
    }

    fn response_filename(&self) -> String {
        match self.expected {
            Some(ExpectedResponse::SignedPsbt) => "signed.psbt".to_owned(),
            _ => "signer-response.json".to_owned(),
        }
    }

    fn accept_file_response(&mut self, bytes: Vec<u8>) {
        let Some(expected) = self.expected else {
            return;
        };
        let payload = UrPayload {
            ur_type: expected.ur_type(),
            data: bytes,
        };
        match expected.decode(payload) {
            Ok(response) => self.finish(AirgapOutcome::Response(response)),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn start_camera(&mut self, index: usize) {
        let fallback_phase = if self.phase == Phase::StartingCamera {
            Phase::Choose
        } else {
            Phase::Scanning
        };
        self.stop_camera();
        self.error = None;
        let Some(camera) = self.cameras.get(index) else {
            self.phase = fallback_phase;
            self.error = Some("Selected camera is no longer available".to_owned());
            return;
        };
        let Some(expected) = self.expected else {
            self.phase = fallback_phase;
            self.error = Some("This request does not expect a response".to_owned());
            return;
        };
        match CameraScanner::start(
            camera.index.clone(),
            expected.ur_type(),
            ScanLimits::default(),
        ) {
            Ok(scanner) => {
                self.selected_camera = Some(index);
                self.scanner = Some(scanner);
                self.phase = Phase::Scanning;
            }
            Err(error) => {
                self.phase = fallback_phase;
                self.error = Some(error.to_string());
            }
        }
    }

    fn poll_camera(&mut self) {
        let mut complete = None;
        let mut failed = false;
        if let Some(scanner) = self.scanner.as_ref() {
            while let Ok(event) = scanner.try_recv() {
                match event {
                    CameraEvent::Preview {
                        width,
                        height,
                        rgba,
                    } => {
                        self.preview = Some(image::Handle::from_rgba(width, height, rgba));
                    }
                    CameraEvent::Progress {
                        estimated,
                        detected_frames,
                    } => {
                        self.progress = estimated;
                        self.detected_frames = detected_frames;
                    }
                    CameraEvent::Rejected(error) => self.error = Some(error),
                    CameraEvent::Complete(payload) => complete = Some(payload),
                    CameraEvent::Failure(error) => {
                        self.error = Some(error.to_string());
                        failed = true;
                    }
                }
            }
        }
        if failed {
            self.stop_camera();
        }
        if let Some(payload) = complete {
            let Some(expected) = self.expected else {
                return;
            };
            match expected.decode(payload) {
                Ok(response) => self.finish(AirgapOutcome::Response(response)),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }

    fn stop_camera(&mut self) {
        self.scanner = None;
        self.preview = None;
        self.progress = 0.0;
        self.detected_frames = 0;
        self.selected_camera = None;
    }

    fn finish(&mut self, outcome: AirgapOutcome) {
        self.stop_camera();
        self.animation = None;
        self.qr_data = None;
        self.phase = Phase::Done;
        self.outcome = Some(outcome);
    }
}

fn read_bounded_file(path: &Path, maximum_bytes: usize) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length > maximum_bytes as u64 {
        return Err(format!(
            "Signer response is too large ({length} bytes; maximum {maximum_bytes})"
        ));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    file.take(maximum_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > maximum_bytes {
        return Err(format!(
            "Signer response is too large (maximum {maximum_bytes} bytes)"
        ));
    }
    Ok(bytes)
}

impl Drop for AirgapModal {
    fn drop(&mut self) {
        self.stop_camera();
        self.animation = None;
        self.qr_data = None;
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    use crate::airgap::{AddressVerificationRequest, PolicyRegistration};

    use super::*;

    #[test]
    fn registration_export_requires_explicit_user_confirmation() {
        let registration = PolicyRegistration::from_json(include_bytes!(
            "../../../test_assets/passport/policy-registration-mainnet.json"
        ))
        .unwrap();
        let mut modal = AirgapModal::new(
            "Register policy",
            AirgappedRequest::RegisterPolicy(registration),
            "liana-policy.json",
        );

        let _ = modal.update(AirgapAction::FileExported(Ok(Some(PathBuf::from(
            "liana-policy.json",
        )))));
        assert!(modal.take_outcome().is_none());

        let _ = modal.update(AirgapAction::Finish);
        assert!(matches!(
            modal.take_outcome(),
            Some(AirgapOutcome::Exported)
        ));
    }

    #[test]
    fn response_file_reads_are_bounded_before_protocol_decoding() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "liana-airgap-response-{}-{unique}",
            std::process::id()
        ));
        fs::write(&path, [1, 2, 3, 4]).unwrap();
        assert_eq!(read_bounded_file(&path, 4).unwrap(), [1, 2, 3, 4]);
        assert!(read_bounded_file(&path, 3)
            .unwrap_err()
            .contains("too large"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn camera_failure_returns_to_the_transport_chooser() {
        let registration = PolicyRegistration::from_json(include_bytes!(
            "../../../test_assets/passport/policy-registration-mainnet.json"
        ))
        .unwrap();
        let mut modal = AirgapModal::new(
            "Register policy",
            AirgappedRequest::RegisterPolicy(registration),
            "liana-policy.json",
        );
        modal.phase = Phase::StartingCamera;

        let _ = modal.update(AirgapAction::Cameras(Err(CameraFailure::PermissionDenied)));

        assert_eq!(modal.phase, Phase::Choose);
        assert_eq!(modal.error.as_deref(), Some("camera permission denied"));
    }

    #[test]
    fn stale_camera_permission_callback_is_ignored() {
        let registration = PolicyRegistration::from_json(include_bytes!(
            "../../../test_assets/passport/policy-registration-mainnet.json"
        ))
        .unwrap();
        let mut modal = AirgapModal::new(
            "Register policy",
            AirgappedRequest::RegisterPolicy(registration),
            "liana-policy.json",
        );

        let _ = modal.update(AirgapAction::Cameras(Err(CameraFailure::PermissionDenied)));

        assert_eq!(modal.phase, Phase::Choose);
        assert!(modal.error.is_none());
    }

    #[test]
    fn address_verification_rejects_file_transport() {
        let registration = PolicyRegistration::from_json(include_bytes!(
            "../../../test_assets/passport/policy-registration-mainnet.json"
        ))
        .unwrap();
        let request = AddressVerificationRequest::new(&registration, 0, 0).unwrap();
        let mut modal = AirgapModal::new(
            "Verify address",
            AirgappedRequest::VerifyAddress(request),
            "liana-address-0.json",
        );

        let _ = modal.update(AirgapAction::ExportFile);

        assert_eq!(modal.phase, Phase::Choose);
        assert_eq!(
            modal.error.as_deref(),
            Some("This exchange supports QR codes only")
        );
    }
}
