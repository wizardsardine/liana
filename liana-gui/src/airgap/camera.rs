use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Waker},
    thread,
    time::{Duration, Instant},
};

use bwk_bbqr::Progress;
use bwk_camera::{
    capture::{Camera, CameraId, CameraInfo},
    error::Error as CaptureError,
    Config,
};
use bwk_qr::{
    decoder::{Decoded, Decoder},
    image::Image,
};
use crossbeam::channel;
use iced::{futures::Stream, widget::image, Subscription};

const CAPTURE: Config = Config {
    min_fps: 24,
    fallback_fps: Some(10),
    fine_scan: false,
};
const PREVIEW_MAX_SIDE: u32 = 480;
const SCAN_GUIDE_RGBA: [u8; 4] = [0, 255, 102, 255];
const SCAN_TIMEOUT: Duration = Duration::from_secs(300);
const FRAME_WAIT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CameraFailure {
    #[error("Camera permission denied")]
    PermissionDenied,
    #[error("No camera is available")]
    Unavailable,
    #[error("The camera is already in use")]
    Busy,
    #[error("Camera capture failed: {0}")]
    Capture(String),
    #[error("Nothing was scanned in time")]
    TimedOut,
}

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
pub enum CameraEvent {
    Frame { side: u32, rgba: Vec<u8> },
    Progress(Progress),
    Payload(Vec<u8>),
    Failure(CameraFailure),
}

pub async fn request_camera_access() -> Result<Vec<CameraInfo>, CameraFailure> {
    let cameras = tokio::task::spawn_blocking(|| bwk_camera::capture::list().recv())
        .await
        .map_err(|_| CameraFailure::Unavailable)?
        .map_err(|_| CameraFailure::Unavailable)??;
    log::debug!("{} camera(s) available", cameras.len());
    Ok(cameras)
}

pub struct CameraScanner {
    camera: CameraId,
    pub frame: Option<image::Handle>,
    pub progress: Option<Progress>,
}

impl CameraScanner {
    pub fn new(camera: &CameraInfo) -> Self {
        Self {
            camera: camera.id.clone(),
            frame: None,
            progress: None,
        }
    }

    pub fn subscription(&self) -> Subscription<CameraEvent> {
        Subscription::run_with(self.camera.clone(), scan)
    }

    pub fn update(&mut self, event: CameraEvent) -> Option<Result<Vec<u8>, CameraFailure>> {
        match event {
            CameraEvent::Frame { side, rgba } => {
                self.frame = Some(image::Handle::from_rgba(side, side, rgba))
            }
            CameraEvent::Progress(progress) => self.progress = Some(progress),
            CameraEvent::Payload(bytes) => return Some(Ok(bytes)),
            CameraEvent::Failure(failure) => return Some(Err(failure)),
        }
        None
    }
}

type SharedWaker = Arc<Mutex<Option<Waker>>>;

fn scan(camera: &CameraId) -> CameraListener {
    let (sender, receiver) = channel::unbounded();
    let waker = SharedWaker::default();
    let stop = Arc::new(AtomicBool::new(false));
    let sender = CameraSender {
        sender,
        waker: waker.clone(),
    };
    let failure = sender.clone();
    let camera = camera.clone();
    let thread_stop = stop.clone();
    if let Err(error) = thread::Builder::new()
        .name("liana-qr-camera".to_owned())
        .spawn(move || run_camera(&camera, &thread_stop, &sender))
    {
        failure.send(CameraEvent::Failure(CameraFailure::Capture(
            error.to_string(),
        )));
    }
    CameraListener {
        receiver,
        waker,
        stop,
        pending: VecDeque::new(),
    }
}

#[derive(Clone)]
struct CameraSender {
    sender: channel::Sender<CameraEvent>,
    waker: SharedWaker,
}

impl CameraSender {
    fn send(&self, event: CameraEvent) {
        match &event {
            CameraEvent::Payload(payload) => {
                log::debug!("scanned a complete {} byte message", payload.len());
            }
            CameraEvent::Failure(failure) => log::warn!("camera stopped: {failure}"),
            _ => {}
        }
        if self.sender.send(event).is_ok() {
            if let Some(waker) = self.waker.lock().expect("poisoned").clone() {
                waker.wake();
            }
        }
    }
}

struct CameraListener {
    receiver: channel::Receiver<CameraEvent>,
    waker: SharedWaker,
    stop: Arc<AtomicBool>,
    pending: VecDeque<CameraEvent>,
}

impl Stream for CameraListener {
    type Item = CameraEvent;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        *this.waker.lock().expect("poisoned") = Some(cx.waker().clone());
        let disconnected = loop {
            match this.receiver.try_recv() {
                Ok(event) => this.pending.push_back(event),
                Err(channel::TryRecvError::Empty) => break false,
                Err(channel::TryRecvError::Disconnected) => break true,
            }
        };
        drop_duplicate_events(&mut this.pending);
        match this.pending.pop_front() {
            Some(event) => Poll::Ready(Some(event)),
            None if disconnected => Poll::Ready(None),
            None => Poll::Pending,
        }
    }
}

impl Drop for CameraListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn drop_duplicate_events(events: &mut VecDeque<CameraEvent>) {
    let last_frame = events
        .iter()
        .rposition(|event| matches!(event, CameraEvent::Frame { .. }));
    let last_progress = events
        .iter()
        .rposition(|event| matches!(event, CameraEvent::Progress(_)));
    let mut index = 0;
    events.retain(|event| {
        let keep = match event {
            CameraEvent::Frame { .. } => Some(index) == last_frame,
            CameraEvent::Progress(_) => Some(index) == last_progress,
            CameraEvent::Payload(_) | CameraEvent::Failure(_) => true,
        };
        index += 1;
        keep
    });
}

fn run_camera(id: &CameraId, stop: &AtomicBool, sender: &CameraSender) {
    let camera = match Camera::start(id, CAPTURE) {
        Ok(camera) => camera,
        Err(error) => return sender.send(CameraEvent::Failure(error.into())),
    };
    let mut decoder = Decoder::new();
    let started = Instant::now();
    let mut last_progress: Option<Progress> = None;
    let mut last_payload: Option<Vec<u8>> = None;
    while !stop.load(Ordering::Acquire) {
        if started.elapsed() > SCAN_TIMEOUT {
            sender.send(CameraEvent::Failure(CameraFailure::TimedOut));
            break;
        }
        if let Some(error) = camera.failure() {
            sender.send(CameraEvent::Failure(error.into()));
            break;
        }
        let Some(frame) = camera.wait_frame(FRAME_WAIT) else {
            continue;
        };
        let grey = frame.grey();
        match decoder.process(&grey) {
            Ok(decoded) => {
                if let Some(payload) = decoded.and_then(bytes) {
                    if last_payload.as_ref() != Some(&payload) {
                        last_payload = Some(payload.clone());
                        sender.send(CameraEvent::Payload(payload));
                    }
                }
                if let Some(progress) = decoder.progress() {
                    if last_progress.as_ref() != Some(&progress) {
                        log::debug!(
                            "read {} of {} QR frame(s)",
                            progress.seen(),
                            progress.total(),
                        );
                        last_progress = Some(progress.clone());
                        sender.send(CameraEvent::Progress(progress));
                    }
                }
            }
            Err(error) => {
                log::debug!("dropped a frame the scanner refused: {error}");
                decoder.reset();
            }
        }
        let (side, rgba) = preview(&grey);
        sender.send(CameraEvent::Frame { side, rgba });
    }
}

fn bytes(decoded: Decoded) -> Option<Vec<u8>> {
    match decoded {
        Decoded::Bytes(payload) => Some(payload),
        Decoded::Text(_) | Decoded::Binary(_) => None,
    }
}

fn preview(grey: &Image) -> (u32, Vec<u8>) {
    let side = grey.side();
    let step = side.div_ceil(PREVIEW_MAX_SIDE).max(1);
    let preview_side = side.div_ceil(step);
    let mut rgba = Vec::with_capacity((preview_side as usize).pow(2) * 4);
    for preview_y in 0..preview_side {
        let source_y = (preview_y * step).min(side - 1);
        for preview_x in 0..preview_side {
            let source_x = side - 1 - (preview_x * step).min(side - 1);
            let value = grey.pixel(source_x, source_y);
            rgba.extend_from_slice(&[value, value, value, 255]);
        }
    }
    draw_scan_guide(preview_side, &mut rgba);
    (preview_side, rgba)
}

fn draw_scan_guide(side: u32, rgba: &mut [u8]) {
    let guide = side * 3 / 5;
    if guide < 4 {
        return;
    }
    let left = (side - guide) / 2;
    let top = left;
    let right = left + guide - 1;
    let bottom = right;
    let corner = (guide / 5).max(12).min(guide);
    let thickness = 3u32.min(guide);
    let mut paint = |x: u32, y: u32| {
        let offset = ((y as usize * side as usize) + x as usize) * 4;
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
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use bwk_bbqr::Progress;
    use bwk_qr::image::Image;

    use crate::airgap::camera::{
        draw_scan_guide, drop_duplicate_events, preview, CameraEvent, PREVIEW_MAX_SIDE,
    };

    fn frame(side: u32) -> CameraEvent {
        CameraEvent::Frame {
            side,
            rgba: Vec::new(),
        }
    }

    fn progress(last: usize) -> CameraEvent {
        CameraEvent::Progress(Progress {
            received: vec![true; 3],
            last,
        })
    }

    #[test]
    fn a_batch_keeps_every_message_and_the_last_frame_in_order() {
        let mut events = VecDeque::from([
            frame(1),
            progress(0),
            CameraEvent::Payload(vec![1]),
            frame(2),
            progress(1),
            CameraEvent::Payload(vec![2]),
            frame(3),
        ]);
        drop_duplicate_events(&mut events);
        assert_eq!(
            events,
            VecDeque::from([
                CameraEvent::Payload(vec![1]),
                progress(1),
                CameraEvent::Payload(vec![2]),
                frame(3),
            ])
        );
    }

    #[test]
    fn the_preview_is_mirrored_and_bounded() {
        let grey = Image::from_fn(4, |x, y| (y * 4 + x) as u8);
        let (side, rgba) = preview(&grey);
        assert_eq!(side, 4);
        assert_eq!(rgba.len(), 4 * 4 * 4);
        assert_eq!(&rgba[..4], &[3, 3, 3, 255]);

        let (side, _) = preview(&Image::from_fn(1080, |_, _| 0));
        assert!(side <= PREVIEW_MAX_SIDE);
    }

    #[test]
    fn a_tiny_preview_gets_its_guide_without_panicking() {
        for side in [7, 8, 12, 19] {
            let mut rgba = vec![0; side * side * 4];
            draw_scan_guide(side as u32, &mut rgba);
        }
    }
}
