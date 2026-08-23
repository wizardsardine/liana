//! Camera capture for the air-gapped QR exchange.
//!
//! The capture thread owns the QR decoding as well, so a camera frame is never
//! carried to the UI thread to be decoded there. Only the downscaled preview and
//! the reassembled message cross the channel. Frames live in memory for the
//! length of one decode and the stream is released on cancel, drop or failure.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use bwk_bbqr::Progress;
use bwk_camera::{
    capture::{Camera, CameraId, CameraInfo},
    error::Error as CaptureError,
    Config,
};
use bwk_qr::decoder::{Decoded, Decoder};
use iced::{
    widget::{column, image, row},
    Length, Subscription,
};
use liana_ui::{
    component::{button, qr, text::new::caption},
    widget::Element,
};

use crate::t;

/// The frame rate a camera is streamed at, or the fallback for one that cannot
/// reach it. Frames are read at the scan side the decoder reads, never finer.
const CAPTURE: Config = Config {
    min_fps: 24,
    fallback_fps: Some(10),
    fine_scan: false,
};
const PREVIEW_MAX_WIDTH: u32 = 640;
const PREVIEW_MAX_HEIGHT: u32 = 480;
const PREVIEW_INTERVAL: Duration = Duration::from_millis(33);
const DECODE_INTERVAL: Duration = Duration::from_millis(200);
const EVENT_QUEUE: usize = 3;
/// How often the UI takes in what the worker sent.
const POLL_INTERVAL: Duration = Duration::from_millis(33);
const PREVIEW_HEIGHT: u32 = 300;
/// The scan guide corners drawn over the preview, opaque green.
const SCAN_GUIDE_RGBA: [u8; 4] = [0, 255, 102, 255];
/// How long a scan may run before it is abandoned, so a modal left open does not
/// hold the camera forever.
const SCAN_TIMEOUT: Duration = Duration::from_secs(300);
/// How long the capture loop waits before looking for a new frame again.
const FRAME_POLL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraFailure {
    PermissionDenied,
    Unavailable,
    Busy,
    Capture(String),
    InvalidFrame,
    TimedOut,
}

impl std::fmt::Display for CameraFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "Camera permission denied"),
            Self::Unavailable => write!(f, "No camera is available"),
            Self::Busy => write!(f, "The camera is already in use"),
            Self::Capture(error) => write!(f, "Camera capture failed: {error}"),
            Self::InvalidFrame => write!(f, "The camera returned an unreadable frame"),
            Self::TimedOut => write!(f, "Nothing was scanned in time"),
        }
    }
}

impl std::error::Error for CameraFailure {}

impl From<CaptureError> for CameraFailure {
    fn from(error: CaptureError) -> Self {
        match error {
            CaptureError::NoCamera => Self::Unavailable,
            CaptureError::Busy => Self::Busy,
            CaptureError::PermissionDenied => Self::PermissionDenied,
            CaptureError::Capture(error) => Self::Capture(error),
            error @ CaptureError::NoSuitableMode { .. } => Self::Capture(error.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CameraEvent {
    Preview {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Progress(Progress),
    /// A message the signer sent, reassembled from its frames.
    Payload(Vec<u8>),
    Failure(CameraFailure),
}

/// Requests access without blocking the update loop and returns the cameras the
/// user can pick from. On macOS this waits for the user to answer the permission
/// dialog.
pub async fn request_camera_access() -> Result<Vec<CameraInfo>, CameraFailure> {
    let cameras = tokio::task::spawn_blocking(|| bwk_camera::capture::list().recv())
        .await
        .map_err(|_| CameraFailure::Unavailable)?
        .map_err(|_| CameraFailure::Unavailable)??;
    log::debug!("Air-gap: {} camera(s) available", cameras.len());
    Ok(cameras)
}

/// Scans the signer's answer: owns the cameras the user can pick from and the
/// capture worker of the selected one, with what it shows of the scan so far.
/// Dropping it stops the capture loop and closes the native stream.
pub struct CameraScanner {
    cameras: Vec<CameraInfo>,
    selected: usize,
    worker: Worker,
    preview: Option<image::Handle>,
    progress: Option<Progress>,
}

impl CameraScanner {
    /// Starts on the first of `cameras`.
    pub fn start(cameras: Vec<CameraInfo>) -> Result<Self, CameraFailure> {
        let camera = cameras.first().ok_or(CameraFailure::Unavailable)?;
        let worker = Worker::start(&camera.id)?;
        Ok(Self {
            cameras,
            selected: 0,
            worker,
            preview: None,
            progress: None,
        })
    }

    /// Starts the scan over on another camera.
    pub fn select(&mut self, index: usize) -> Result<(), CameraFailure> {
        let camera = self.cameras.get(index).ok_or(CameraFailure::Unavailable)?;
        // the current stream must close before the next one opens the device
        self.worker.cancel();
        self.worker = Worker::start(&camera.id)?;
        self.selected = index;
        self.preview = None;
        self.progress = None;
        Ok(())
    }

    /// Fires while the scan runs, feed each tick to [`Self::poll`].
    pub fn subscription(&self) -> Subscription<Instant> {
        iced::time::every(POLL_INTERVAL)
    }

    /// Takes in what the worker sent since the last poll. Returns a message the
    /// signer sent, or why the camera stopped.
    pub fn poll(&mut self) -> Option<Result<Vec<u8>, CameraFailure>> {
        let mut outcome = None;
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                CameraEvent::Preview {
                    width,
                    height,
                    rgba,
                } => self.preview = Some(image::Handle::from_rgba(width, height, rgba)),
                CameraEvent::Progress(progress) => self.progress = Some(progress),
                CameraEvent::Payload(bytes) => outcome = Some(Ok(bytes)),
                CameraEvent::Failure(failure) => return Some(Err(failure)),
            }
        }
        outcome
    }

    /// The camera picker when there is a choice, the preview, then how many
    /// frames were read. Picking a camera hands back `on_select` of its index.
    pub fn view<'a, M: Clone + 'a>(&'a self, on_select: impl Fn(usize) -> M) -> Element<'a, M> {
        let cameras = (self.cameras.len() > 1).then(|| {
            self.cameras
                .iter()
                .enumerate()
                .fold(row![].spacing(10), |chosen, (index, camera)| {
                    chosen.push(button::btn_secondary(
                        None,
                        &camera.name,
                        button::BtnWidth::Auto,
                        (self.selected != index).then(|| on_select(index)),
                    ))
                })
        });
        let status = match &self.progress {
            Some(progress) => t!(
                "airgap-frames-read",
                seen = progress.seen(),
                total = progress.total()
            ),
            None => t!("airgap-looking-for-qr"),
        };
        column![
            cameras,
            self.preview
                .clone()
                .map(|handle| qr::preview(handle, PREVIEW_HEIGHT)),
            caption(status),
            self.progress.as_ref().and_then(qr::progress),
        ]
        .spacing(15)
        .align_x(iced::Alignment::Center)
        .width(Length::Fill)
        .into()
    }
}

/// The capture thread of one camera. Cancelling or dropping it stops the loop.
struct Worker {
    stop: Arc<AtomicBool>,
    events: Receiver<CameraEvent>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn start(id: &CameraId) -> Result<Self, CameraFailure> {
        let camera = Camera::start(id, CAPTURE)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (sender, events) = mpsc::sync_channel(EVENT_QUEUE);
        let thread = thread::Builder::new()
            .name("liana-qr-camera".to_owned())
            .spawn(move || run_camera(camera, worker_stop, sender))
            .map_err(|error| CameraFailure::Capture(error.to_string()))?;
        Ok(Self {
            stop,
            events,
            thread: Some(thread),
        })
    }

    fn cancel(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run_camera(camera: Camera, stop: Arc<AtomicBool>, sender: SyncSender<CameraEvent>) {
    let mut decoder = Decoder::new();
    let started = Instant::now();
    let mut last_preview = Instant::now() - PREVIEW_INTERVAL;
    let mut last_decode = Instant::now() - DECODE_INTERVAL;
    let mut last_seen = 0;
    let mut last_payload: Option<Vec<u8>> = None;
    while !stop.load(Ordering::Acquire) {
        if started.elapsed() > SCAN_TIMEOUT {
            send_lossless(
                &sender,
                &stop,
                CameraEvent::Failure(CameraFailure::TimedOut),
            );
            break;
        }
        if let Some(error) = camera.failure() {
            send_lossless(&sender, &stop, CameraEvent::Failure(error.into()));
            break;
        }
        // each frame is handed out once, so a frame here is always a new one
        let Some(frame) = camera.frame() else {
            thread::sleep(FRAME_POLL);
            continue;
        };
        let now = Instant::now();
        let decode_due = now.duration_since(last_decode) >= DECODE_INTERVAL;
        let preview_due = now.duration_since(last_preview) >= PREVIEW_INTERVAL;
        if !decode_due && !preview_due {
            continue;
        }
        let grey = frame.grey();
        if decode_due {
            last_decode = now;
            match decoder.process(&grey) {
                // a camera sees unrelated codes too; only framed messages count
                Ok(decoded) => {
                    // Keep scanning after a message: it may be an answer to an
                    // earlier exchange still on the signer's screen, and only the
                    // consumer can tell. The same one reassembles over and over
                    // as the animation loops, so only a change is worth sending.
                    if let Some(payload) = decoded.and_then(bytes) {
                        if last_payload.as_ref() != Some(&payload) {
                            last_payload = Some(payload.clone());
                            send_lossless(&sender, &stop, CameraEvent::Payload(payload));
                        }
                    }
                    if let Some(progress) = decoder.progress() {
                        if progress.seen() != last_seen {
                            last_seen = progress.seen();
                            log::debug!(
                                "Air-gap: read {} of {} QR frame(s)",
                                last_seen,
                                progress.total(),
                            );
                        }
                        let _ = sender.try_send(CameraEvent::Progress(progress));
                    }
                }
                // a conflicting part poisons the joiner, not the scan
                Err(error) => {
                    log::debug!("Air-gap: dropped a frame the scanner refused: {error}");
                    decoder.reset();
                }
            }
        }

        if preview_due && !stop.load(Ordering::Acquire) {
            last_preview = now;
            let Some((preview_width, preview_height, rgba)) =
                luma_to_preview_rgba(grey.side(), grey.side(), grey.data())
            else {
                send_lossless(
                    &sender,
                    &stop,
                    CameraEvent::Failure(CameraFailure::InvalidFrame),
                );
                break;
            };
            let _ = sender.try_send(CameraEvent::Preview {
                width: preview_width,
                height: preview_height,
                rgba,
            });
        }
    }
}

fn bytes(decoded: Decoded) -> Option<Vec<u8>> {
    match decoded {
        Decoded::Bytes(payload) => Some(payload),
        Decoded::Text(_) | Decoded::Binary(_) => None,
    }
}

/// Deliver a payload or a failure without making cancellation wait for a full
/// UI queue. Preview and progress events are deliberately lossy; these two must
/// arrive, so they retry until consumed, disconnected, or the scanner is
/// cancelled.
fn send_lossless(sender: &SyncSender<CameraEvent>, stop: &AtomicBool, mut event: CameraEvent) {
    match &event {
        CameraEvent::Payload(payload) => {
            log::debug!("Air-gap: scanned a complete {} byte message", payload.len());
        }
        CameraEvent::Failure(failure) => log::warn!("Air-gap: camera stopped: {failure}"),
        _ => {}
    }
    loop {
        match sender.try_send(event) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => return,
            Err(TrySendError::Full(returned)) => {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                event = returned;
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// The preview is drawn from the same grayscale frame the decoder reads, so what
/// the user aims with is what the scanner sees.
fn luma_to_preview_rgba(width: u32, height: u32, luma: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    if width == 0 || height == 0 || luma.len() != pixels {
        return None;
    }
    let step = width
        .div_ceil(PREVIEW_MAX_WIDTH)
        .max(height.div_ceil(PREVIEW_MAX_HEIGHT))
        .max(1);
    let preview_width = width.div_ceil(step);
    let preview_height = height.div_ceil(step);
    let mut rgba = Vec::with_capacity(
        (preview_width as usize)
            .checked_mul(preview_height as usize)?
            .checked_mul(4)?,
    );
    for preview_y in 0..preview_height {
        let source_y = (preview_y * step).min(height - 1);
        for preview_x in 0..preview_width {
            // Mirror only the preview so it behaves like a conventional
            // front-facing camera. QR decoding continues to use the original,
            // unmodified frame above.
            let source_x = width - 1 - (preview_x * step).min(width - 1);
            let value = luma[(source_y as usize) * (width as usize) + source_x as usize];
            rgba.extend_from_slice(&[value, value, value, 255]);
        }
    }
    draw_scan_guide(preview_width, preview_height, &mut rgba)?;
    Some((preview_width, preview_height, rgba))
}

fn draw_scan_guide(width: u32, height: u32, rgba: &mut [u8]) -> Option<()> {
    if width == 0 || height == 0 || rgba.len() != width as usize * height as usize * 4 {
        return None;
    }
    let side = width.min(height) * 3 / 5;
    if side < 4 {
        return Some(());
    }
    let left = (width - side) / 2;
    let top = (height - side) / 2;
    let right = left + side - 1;
    let bottom = top + side - 1;
    let corner = (side / 5).max(12).min(side);
    let thickness = 3u32.min(side);
    let mut paint = |x: u32, y: u32| {
        let offset = ((y as usize * width as usize) + x as usize) * 4;
        rgba[offset..offset + 4].copy_from_slice(&SCAN_GUIDE_RGBA);
    };
    for line in 0..thickness {
        for distance in 0..corner {
            paint(left + distance, top + line);
            paint(left + line, top + distance);
            paint(right - distance, top + line);
            paint(right - line, top + distance);
            paint(left + distance, bottom - line);
            paint(left + line, bottom - distance);
            paint(right - distance, bottom - line);
            paint(right - line, bottom - distance);
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use crate::airgap::camera::{draw_scan_guide, luma_to_preview_rgba};

    #[test]
    fn the_preview_is_mirrored_and_bounded() {
        let width = 4;
        let height = 2;
        let luma: Vec<u8> = (0..(width * height) as u8).collect();
        let (preview_width, preview_height, rgba) =
            luma_to_preview_rgba(width, height, &luma).unwrap();
        assert_eq!((preview_width, preview_height), (width, height));
        assert_eq!(rgba.len(), (width * height * 4) as usize);
        // the first preview pixel is the last source pixel of that row
        assert_eq!(&rgba[..4], &[3, 3, 3, 255]);
    }

    #[test]
    fn a_preview_of_the_wrong_length_is_refused() {
        assert!(luma_to_preview_rgba(4, 2, &[0; 7]).is_none());
        assert!(luma_to_preview_rgba(0, 2, &[]).is_none());
    }

    /// The guide's corners are at least 12 pixels long, which a tiny preview
    /// cannot hold, so they must shrink to the guide rather than paint outside it.
    #[test]
    fn a_tiny_preview_gets_its_guide_without_panicking() {
        for side in [7, 8, 12, 19] {
            let mut rgba = vec![0; side * side * 4];
            assert_eq!(
                draw_scan_guide(side as u32, side as u32, &mut rgba),
                Some(())
            );
        }
    }
}
