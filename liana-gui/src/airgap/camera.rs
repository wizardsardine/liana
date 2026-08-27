//! Camera capture for the air-gapped QR exchange.
//!
//! The capture thread owns the QR decoding as well, so a 720p frame is never
//! carried to the UI thread to be decoded there. Only the downscaled preview and
//! the reassembled message cross the channel. Frames live in memory for the
//! length of one decode and the stream is released on cancel, drop or failure.

use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use bwk_qr::{
    scan::{Frame, Quircs, Scanner},
    Config, Decoded, Decoder, Image, Progress,
};
use nokhwa::utils::{ApiBackend, CameraFormat, CameraIndex, FrameFormat};

#[cfg(not(target_os = "macos"))]
use nokhwa::{
    utils::{RequestedFormat, RequestedFormatType},
    Camera,
};

#[cfg(any(not(target_os = "macos"), test))]
use nokhwa::pixel_format::LumaFormat;

#[cfg(target_os = "macos")]
use {
    flume::{Receiver as FrameReceiver, Sender as FrameSender},
    nokhwa_bindings_macos::{
        AVCaptureDevice, AVCaptureDeviceInput, AVCaptureSession, AVCaptureVideoCallback,
        AVCaptureVideoDataOutput,
    },
    objc::{
        msg_send,
        runtime::{Object, BOOL, YES},
        sel, sel_impl,
    },
    std::{ffi::CString, sync::Mutex},
};

#[cfg(any(not(target_os = "macos"), test))]
const TARGET_CAPTURE_WIDTH: u32 = 1280;
#[cfg(any(not(target_os = "macos"), test))]
const TARGET_CAPTURE_HEIGHT: u32 = 720;
#[cfg(any(not(target_os = "macos"), test))]
const TARGET_CAPTURE_FPS: u32 = 30;
#[cfg(any(not(target_os = "macos"), test))]
const MIN_REALTIME_FPS: u32 = 24;
/// Frame formats we can turn into grayscale ourselves. MJPEG is deliberately
/// absent: decoding it would mean linking a JPEG library, and every webcam that
/// offers MJPEG offers an uncompressed mode as well.
#[cfg(any(not(target_os = "macos"), test))]
const CONVERTIBLE_FORMATS: [FrameFormat; 5] = [
    FrameFormat::YUYV,
    FrameFormat::NV12,
    FrameFormat::GRAY,
    FrameFormat::RAWRGB,
    FrameFormat::RAWBGR,
];
const PREVIEW_MAX_WIDTH: u32 = 640;
const PREVIEW_MAX_HEIGHT: u32 = 480;
const PREVIEW_INTERVAL: Duration = Duration::from_millis(33);
const DECODE_INTERVAL: Duration = Duration::from_millis(200);
const EVENT_QUEUE: usize = 3;
/// The scan guide corners drawn over the preview, opaque green.
const SCAN_GUIDE_RGBA: [u8; 4] = [0, 255, 102, 255];
/// How long a scan may run before it is abandoned, so a modal left open does not
/// hold the camera forever.
const SCAN_TIMEOUT: Duration = Duration::from_secs(300);
const PERMISSION_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVCaptureSessionPreset1280x720: *mut Object;
}

#[cfg(target_os = "macos")]
type NativeFrame = (Vec<u8>, FrameFormat, Option<Duration>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDescriptor {
    pub index: CameraIndex,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraFailure {
    PermissionDenied,
    PermissionTimedOut,
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
            Self::PermissionTimedOut => write!(f, "Camera permission request timed out"),
            Self::Unavailable => write!(f, "No camera is available"),
            Self::Busy => write!(f, "The camera is already in use"),
            Self::Capture(error) => write!(f, "Camera capture failed: {error}"),
            Self::InvalidFrame => write!(f, "The camera returned an unreadable frame"),
            Self::TimedOut => write!(f, "Nothing was scanned in time"),
        }
    }
}

impl std::error::Error for CameraFailure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraEvent {
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
/// user can pick from. The permission callback is bounded because some platform
/// backends never answer when their permission service is unavailable.
pub async fn request_camera_access() -> Result<Vec<CameraDescriptor>, CameraFailure> {
    if nokhwa::nokhwa_check() {
        return list_cameras();
    }
    tokio::task::spawn_blocking(|| {
        let (sender, receiver) = mpsc::sync_channel(1);
        nokhwa::nokhwa_initialize(move |granted| {
            let _ = sender.try_send(granted);
        });
        match receiver.recv_timeout(PERMISSION_TIMEOUT) {
            Ok(true) => list_cameras(),
            Ok(false) => Err(CameraFailure::PermissionDenied),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(CameraFailure::PermissionTimedOut),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(CameraFailure::Unavailable),
        }
    })
    .await
    .map_err(|_| CameraFailure::Unavailable)?
}

fn list_cameras() -> Result<Vec<CameraDescriptor>, CameraFailure> {
    let mut cameras = nokhwa::query(ApiBackend::Auto).map_err(map_camera_error)?;
    cameras.retain(|camera| {
        let usable = can_capture(camera);
        log::debug!(
            "Air-gap: camera {:?} ({}) {}",
            camera.index(),
            camera.description(),
            if usable {
                "offered"
            } else {
                "cannot capture, skipped"
            },
        );
        usable
    });
    // the backend hands them back in no particular order, so pick a stable one
    // rather than letting which camera is offered first vary between runs
    cameras.sort_by_key(|camera| camera.index().as_index().unwrap_or(u32::MAX));
    log::info!("Air-gap: {} camera(s) available", cameras.len());
    Ok(cameras
        .into_iter()
        .map(|camera| CameraDescriptor {
            index: camera.index().clone(),
            name: camera.human_name(),
        })
        .collect())
}

/// V4L2 splits one USB camera into a capture node and a metadata node and
/// enumerates both, so an unfiltered list offers a device that cannot produce a
/// frame, and often offers it first. Opening it is the only way to tell the two
/// apart through nokhwa; it reads the format list without starting the stream.
#[cfg(target_os = "linux")]
fn can_capture(camera: &nokhwa::utils::CameraInfo) -> bool {
    Camera::new(
        camera.index().clone(),
        RequestedFormat::new::<LumaFormat>(RequestedFormatType::None),
    )
    .and_then(|mut camera| camera.compatible_camera_formats())
    .is_ok_and(|formats| !formats.is_empty())
}

#[cfg(not(target_os = "linux"))]
fn can_capture(_: &nokhwa::utils::CameraInfo) -> bool {
    true
}

/// Owns a camera worker. Cancelling or dropping it stops the capture loop and
/// closes the native stream.
pub struct CameraScanner {
    stop: Arc<AtomicBool>,
    events: Receiver<CameraEvent>,
    worker: Option<JoinHandle<()>>,
}

impl CameraScanner {
    pub fn start(index: CameraIndex, config: Config) -> Result<Self, CameraFailure> {
        if !nokhwa::nokhwa_check() {
            return Err(CameraFailure::PermissionDenied);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (sender, events) = mpsc::sync_channel(EVENT_QUEUE);
        let worker = thread::Builder::new()
            .name("liana-qr-camera".to_owned())
            .spawn(move || run_camera(index, config, worker_stop, sender))
            .map_err(|error| CameraFailure::Capture(error.to_string()))?;
        Ok(Self {
            stop,
            events,
            worker: Some(worker),
        })
    }

    pub fn try_recv(&self) -> Result<CameraEvent, TryRecvError> {
        self.events.try_recv()
    }

    pub fn cancel(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for CameraScanner {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run_camera(
    index: CameraIndex,
    config: Config,
    stop: Arc<AtomicBool>,
    sender: SyncSender<CameraEvent>,
) {
    let mut decoder = match Decoder::new(config) {
        Ok(decoder) => decoder,
        Err(error) => {
            send_lossless(
                &sender,
                &stop,
                CameraEvent::Failure(CameraFailure::Capture(error.to_string())),
            );
            return;
        }
    };
    let mut camera = match open_camera(index) {
        Ok(camera) => camera,
        Err(failure) => {
            send_lossless(&sender, &stop, CameraEvent::Failure(failure));
            return;
        }
    };
    if let Err(failure) = start_camera_stream(&mut camera) {
        send_lossless(&sender, &stop, CameraEvent::Failure(failure));
        return;
    }

    let started = Instant::now();
    let mut last_preview = Instant::now() - PREVIEW_INTERVAL;
    let mut last_decode = Instant::now() - DECODE_INTERVAL;
    let mut last_seen = 0;
    let mut last_payload: Option<Vec<u8>> = None;
    let mut logged = FrameLog::default();
    while !stop.load(Ordering::Acquire) {
        if started.elapsed() > SCAN_TIMEOUT {
            send_lossless(
                &sender,
                &stop,
                CameraEvent::Failure(CameraFailure::TimedOut),
            );
            break;
        }
        let (width, height, luma) = match read_camera_frame_luma(&mut camera, &stop) {
            Ok(frame) => frame,
            Err(failure) => {
                send_lossless(&sender, &stop, CameraEvent::Failure(failure));
                break;
            }
        };
        let now = Instant::now();
        let decode_due = now.duration_since(last_decode) >= DECODE_INTERVAL;
        let preview_due = now.duration_since(last_preview) >= PREVIEW_INTERVAL;
        if !decode_due && !preview_due {
            continue;
        }
        if decode_due {
            last_decode = now;
            let frame = Image {
                data: luma.clone(),
                width,
                height,
            };
            logged.record(&frame);
            match decoder.process(&frame) {
                // a camera sees unrelated codes too; only framed messages count
                Ok(decoded) => {
                    // Keep scanning after a message: it may be an answer to an
                    // earlier exchange still on the signer's screen, and only the
                    // consumer can tell. The same one reassembles over and over
                    // as the animation loops, so only a change is worth sending.
                    if let Some(payload) = decoded.into_iter().find_map(bytes) {
                        if last_payload.as_ref() != Some(&payload) {
                            last_payload = Some(payload.clone());
                            send_lossless(&sender, &stop, CameraEvent::Payload(payload));
                        }
                    }
                    if let Some(progress) = decoder.progress() {
                        if progress.seen != last_seen {
                            last_seen = progress.seen;
                            log::debug!(
                                "Air-gap: read {} of {} QR frame(s)",
                                progress.seen,
                                progress.total,
                            );
                        }
                        let _ = sender.try_send(CameraEvent::Progress(progress));
                    }
                }
                // a conflicting or oversized part poisons the joiner, not the scan
                Err(error) => {
                    log::debug!("Air-gap: dropped a frame the scanner refused: {error}");
                    decoder.reset();
                }
            }
        }

        if preview_due && !stop.load(Ordering::Acquire) {
            last_preview = now;
            let Some((preview_width, preview_height, rgba)) =
                luma_to_preview_rgba(width, height, &luma)
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
    stop_camera_stream(&mut camera);
}

/// Logs every distinct QR the camera reads, before the joiner swallows it.
///
/// The decoder only ever hands back a message once every part has arrived, so a
/// scan that stalls would otherwise leave no trace of what was actually read.
/// This costs a second scan of each frame, so it only runs at the debug level,
/// and it skips the inverted rescan the decoder does: a frame missing here may
/// still have reached the decoder.
#[derive(Default)]
struct FrameLog {
    scanner: Quircs,
    seen: HashSet<Vec<u8>>,
}

impl FrameLog {
    fn record(&mut self, image: &Image) {
        if !log::log_enabled!(log::Level::Debug) {
            return;
        }
        for payload in self.unseen(image) {
            log::debug!(
                "Air-gap: read QR {} of the answer: {}",
                self.seen.len(),
                String::from_utf8_lossy(&payload),
            );
        }
    }

    /// The QR payloads in this frame that have not been read before. The camera
    /// sees the same frame many times over as the signer's animation loops, and
    /// only the first sighting is worth a line.
    fn unseen(&mut self, image: &Image) -> Vec<Vec<u8>> {
        let Ok(payloads) = self.scanner.scan(Frame {
            data: &image.data,
            width: image.width,
            height: image.height,
        }) else {
            return Vec::new();
        };
        payloads
            .into_iter()
            .filter(|payload| self.seen.insert(payload.clone()))
            .collect()
    }
}

fn bytes(decoded: Decoded) -> Option<Vec<u8>> {
    match decoded {
        Decoded::Bytes(payload) => Some(payload),
        Decoded::Text(_) => None,
    }
}

/// Deliver a payload or a failure without making cancellation wait for a full
/// UI queue. Preview and progress events are deliberately lossy; these two must
/// arrive, so they retry until consumed, disconnected, or the scanner is
/// cancelled.
fn send_lossless(sender: &SyncSender<CameraEvent>, stop: &AtomicBool, mut event: CameraEvent) {
    match &event {
        CameraEvent::Payload(payload) => {
            log::info!("Air-gap: scanned a complete {} byte message", payload.len());
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

#[cfg(not(target_os = "macos"))]
type PlatformCamera = Camera;

#[cfg(not(target_os = "macos"))]
fn open_camera(index: CameraIndex) -> Result<PlatformCamera, CameraFailure> {
    // Open with whatever the backend defaults to, then move it to an advertised
    // real-time mode close to 720p that we can convert. Requesting the absolute
    // highest frame rate can select a multi-megapixel stream whose conversion
    // and QR detection make the preview substantially less responsive.
    let requested = RequestedFormat::new::<LumaFormat>(RequestedFormatType::None);
    let mut camera = Camera::new(index, requested).map_err(map_camera_error)?;
    let formats = camera
        .compatible_camera_formats()
        .map_err(map_camera_error)?;
    let format = preferred_camera_format(&formats).ok_or_else(|| {
        CameraFailure::Capture(
            "this camera only offers compressed frames, which Liana cannot read".to_owned(),
        )
    })?;
    camera
        .set_camera_requset(RequestedFormat::new::<LumaFormat>(
            RequestedFormatType::Exact(format),
        ))
        .map_err(map_camera_error)?;
    log::info!(
        "Air-gap: capturing at {}x{} {} @{}fps",
        format.width(),
        format.height(),
        format.format(),
        format.frame_rate(),
    );
    Ok(camera)
}

#[cfg(not(target_os = "macos"))]
fn start_camera_stream(camera: &mut PlatformCamera) -> Result<(), CameraFailure> {
    camera.open_stream().map_err(map_camera_error)
}

#[cfg(not(target_os = "macos"))]
fn read_camera_frame_luma(
    camera: &mut PlatformCamera,
    _stop: &AtomicBool,
) -> Result<(u32, u32, Vec<u8>), CameraFailure> {
    let buffer = camera.frame().map_err(map_camera_error)?;
    let resolution = buffer.resolution();
    let luma = to_luma(
        buffer.source_frame_format(),
        resolution.width(),
        resolution.height(),
        buffer.buffer(),
    )
    .ok_or(CameraFailure::InvalidFrame)?;
    Ok((resolution.width(), resolution.height(), luma))
}

#[cfg(not(target_os = "macos"))]
fn stop_camera_stream(camera: &mut PlatformCamera) {
    let _ = camera.stop_stream();
}

/// AVFoundation capture path for macOS.
///
/// Nokhwa configures the device format both while constructing and opening a
/// camera. Some built-in Mac cameras reject that exclusive configuration lock
/// even though they are available for capture. A 720p session preset lets
/// AVFoundation negotiate a processed capture mode without locking the device
/// directly, while still giving the QR decoder enough spatial detail.
#[cfg(target_os = "macos")]
struct PlatformCamera {
    device: AVCaptureDevice,
    format: CameraFormat,
    buffer_name: CString,
    receiver: Arc<FrameReceiver<NativeFrame>>,
    sender: Arc<FrameSender<NativeFrame>>,
    input: Option<AVCaptureDeviceInput>,
    session: Option<AVCaptureSession>,
    output: Option<AVCaptureVideoDataOutput>,
    callback: Option<AVCaptureVideoCallback>,
}

#[cfg(target_os = "macos")]
fn open_camera(index: CameraIndex) -> Result<PlatformCamera, CameraFailure> {
    let device = AVCaptureDevice::new(&index).map_err(map_camera_error)?;
    let active = device.active_format().map_err(map_camera_error)?;
    let format = CameraFormat::new(
        active.resolution(),
        FrameFormat::RAWRGB,
        active.frame_rate(),
    );
    let buffer_name = CString::new(format!("liana-qr-camera-{index}"))
        .map_err(|error| CameraFailure::Capture(error.to_string()))?;
    let (sender, receiver) = flume::unbounded();
    Ok(PlatformCamera {
        device,
        format,
        buffer_name,
        receiver: Arc::new(receiver),
        sender: Arc::new(sender),
        input: None,
        session: None,
        output: None,
        callback: None,
    })
}

#[cfg(target_os = "macos")]
fn start_camera_stream(camera: &mut PlatformCamera) -> Result<(), CameraFailure> {
    let input = AVCaptureDeviceInput::new(&camera.device).map_err(map_camera_error)?;
    let session = AVCaptureSession::new();
    session.begin_configuration();
    session.add_input(&input).map_err(map_camera_error)?;
    set_720p_session_preset(&session)?;
    let callback = AVCaptureVideoCallback::new(&camera.buffer_name, &camera.sender)
        .map_err(map_camera_error)?;
    let output = AVCaptureVideoDataOutput::new();
    output.add_delegate(&callback).map_err(map_camera_error)?;
    output
        .set_frame_format(FrameFormat::RAWRGB)
        .map_err(map_camera_error)?;
    session.add_output(&output).map_err(map_camera_error)?;
    session.commit_configuration();
    session.start().map_err(map_camera_error)?;
    let active = camera.device.active_format().map_err(map_camera_error)?;
    camera.format = CameraFormat::new(
        active.resolution(),
        FrameFormat::RAWRGB,
        active.frame_rate(),
    );
    camera.input = Some(input);
    camera.session = Some(session);
    camera.output = Some(output);
    camera.callback = Some(callback);
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn set_720p_session_preset(session: &AVCaptureSession) -> Result<(), CameraFailure> {
    // SAFETY: AVFoundation exports this process-lifetime NSString constant.
    let preset = unsafe { AVCaptureSessionPreset1280x720 };
    // SAFETY: `session.inner()` and `preset` are valid Objective-C objects for
    // the duration of these synchronous messages, and both selectors return
    // the declared Objective-C types.
    let supported: BOOL = unsafe { msg_send![session.inner(), canSetSessionPreset: preset] };
    if supported != YES {
        return Err(CameraFailure::Capture(
            "camera does not support a 720p capture session".to_owned(),
        ));
    }
    // SAFETY: The preceding query confirmed this session accepts the preset.
    let _: () = unsafe { msg_send![session.inner(), setSessionPreset: preset] };
    Ok(())
}

#[cfg(target_os = "macos")]
fn read_camera_frame_luma(
    camera: &mut PlatformCamera,
    stop: &AtomicBool,
) -> Result<(u32, u32, Vec<u8>), CameraFailure> {
    let (bytes, _, _) = loop {
        match camera.receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => break frame,
            Err(flume::RecvTimeoutError::Timeout) if !stop.load(Ordering::Acquire) => continue,
            Err(flume::RecvTimeoutError::Timeout) => {
                return Err(CameraFailure::Capture(
                    "camera capture cancelled".to_owned(),
                ))
            }
            Err(flume::RecvTimeoutError::Disconnected) => {
                return Err(CameraFailure::Capture(
                    "camera capture channel disconnected".to_owned(),
                ))
            }
        }
    };
    let width = camera.format.width();
    let height = camera.format.height();
    let row_bytes = (width as usize)
        .checked_mul(3)
        .ok_or(CameraFailure::InvalidFrame)?;
    let expected = row_bytes
        .checked_mul(height as usize)
        .ok_or(CameraFailure::InvalidFrame)?;
    let bytes = if bytes.len() == expected {
        bytes
    } else if height != 0 && bytes.len() % height as usize == 0 {
        let source_stride = bytes.len() / height as usize;
        if source_stride < row_bytes {
            return Err(CameraFailure::InvalidFrame);
        }
        let mut packed = Vec::with_capacity(expected);
        for row in bytes.chunks_exact(source_stride) {
            packed.extend_from_slice(&row[..row_bytes]);
        }
        packed
    } else {
        return Err(CameraFailure::InvalidFrame);
    };
    let _ = camera.receiver.drain();
    let luma =
        to_luma(FrameFormat::RAWRGB, width, height, &bytes).ok_or(CameraFailure::InvalidFrame)?;
    Ok((width, height, luma))
}

#[cfg(target_os = "macos")]
fn stop_camera_stream(camera: &mut PlatformCamera) {
    if let Some(session) = camera.session.take() {
        if let Some(output) = camera.output.take() {
            session.remove_output(&output);
        }
        if let Some(input) = camera.input.take() {
            session.remove_input(&input);
        }
        session.stop();
    }
    camera.callback = None;
    let _ = camera.receiver.drain();
}

#[cfg(target_os = "macos")]
impl Drop for PlatformCamera {
    fn drop(&mut self) {
        stop_camera_stream(self);
    }
}

#[cfg(any(not(target_os = "macos"), test))]
fn preferred_camera_format(formats: &[CameraFormat]) -> Option<CameraFormat> {
    let has_realtime_format = formats.iter().any(|format| {
        CONVERTIBLE_FORMATS.contains(&format.format()) && format.frame_rate() >= MIN_REALTIME_FPS
    });
    formats
        .iter()
        .copied()
        .filter(|format| CONVERTIBLE_FORMATS.contains(&format.format()))
        .filter(|format| !has_realtime_format || format.frame_rate() >= MIN_REALTIME_FPS)
        .min_by_key(|format| {
            let resolution_distance = format.width().abs_diff(TARGET_CAPTURE_WIDTH)
                + format.height().abs_diff(TARGET_CAPTURE_HEIGHT);
            let frame_rate_distance = format.frame_rate().abs_diff(TARGET_CAPTURE_FPS);
            (resolution_distance, frame_rate_distance)
        })
}

/// Grayscale is all a QR decoder needs, and every format below carries luminance
/// already or is one multiply away from it, so no colour buffer is ever built.
fn to_luma(format: FrameFormat, width: u32, height: u32, frame: &[u8]) -> Option<Vec<u8>> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    match format {
        // the Y of each pixel pair is already the luminance
        FrameFormat::YUYV => (frame.len() == pixels.checked_mul(2)?)
            .then(|| frame.iter().step_by(2).copied().collect()),
        // the Y plane comes first and is exactly one byte per pixel
        FrameFormat::NV12 => (frame.len() >= pixels).then(|| frame[..pixels].to_vec()),
        FrameFormat::GRAY => (frame.len() == pixels).then(|| frame.to_vec()),
        FrameFormat::RAWRGB => rgb_to_luma(pixels, frame, 0, 2),
        FrameFormat::RAWBGR => rgb_to_luma(pixels, frame, 2, 0),
        // decoding this would mean linking a JPEG library, see CONVERTIBLE_FORMATS
        FrameFormat::MJPEG => None,
    }
}

/// Rec. 601 luma, in fixed point so the hot path stays integer only.
fn rgb_to_luma(pixels: usize, frame: &[u8], red: usize, blue: usize) -> Option<Vec<u8>> {
    if frame.len() != pixels.checked_mul(3)? {
        return None;
    }
    Some(
        frame
            .chunks_exact(3)
            .map(|pixel| {
                ((u16::from(pixel[red]) * 77
                    + u16::from(pixel[1]) * 150
                    + u16::from(pixel[blue]) * 29)
                    >> 8) as u8
            })
            .collect(),
    )
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
    let corner = (side / 5).max(12);
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

fn map_camera_error(error: nokhwa::NokhwaError) -> CameraFailure {
    let text = error.to_string();
    let lowercase = text.to_ascii_lowercase();
    if lowercase.contains("permission") || lowercase.contains("denied") {
        CameraFailure::PermissionDenied
    } else if lowercase.contains("busy") || lowercase.contains("in use") {
        CameraFailure::Busy
    } else if lowercase.contains("not found") || lowercase.contains("no camera") {
        CameraFailure::Unavailable
    } else {
        CameraFailure::Capture(text)
    }
}

#[cfg(test)]
mod tests {
    use bwk_qr::Encoder;

    use super::*;

    fn format(frame_format: FrameFormat, width: u32, height: u32, fps: u32) -> CameraFormat {
        CameraFormat::new_from(width, height, frame_format, fps)
    }

    /// The Y byte of each pixel pair is the luminance, so it is taken as is.
    #[test]
    fn yuyv_takes_the_luma_plane() {
        // two pixels: Y0 U Y1 V
        let frame = [10, 128, 20, 128, 30, 128, 40, 128];
        assert_eq!(
            to_luma(FrameFormat::YUYV, 4, 1, &frame),
            Some(vec![10, 20, 30, 40])
        );
    }

    #[test]
    fn nv12_takes_the_leading_luma_plane() {
        let mut frame = vec![1, 2, 3, 4];
        frame.extend_from_slice(&[128, 128]);
        assert_eq!(
            to_luma(FrameFormat::NV12, 2, 2, &frame),
            Some(vec![1, 2, 3, 4])
        );
    }

    #[test]
    fn gray_is_taken_unchanged() {
        assert_eq!(to_luma(FrameFormat::GRAY, 2, 1, &[7, 9]), Some(vec![7, 9]));
    }

    /// The two raw orders differ only in which end of the pixel is red, so a
    /// pure red pixel has to land on the red weight in both.
    #[test]
    fn raw_orders_weight_the_right_channel() {
        let red_rgb = [255, 0, 0];
        let red_bgr = [0, 0, 255];
        assert_eq!(
            to_luma(FrameFormat::RAWRGB, 1, 1, &red_rgb),
            to_luma(FrameFormat::RAWBGR, 1, 1, &red_bgr)
        );
        // green weighs heaviest, blue least
        let green = to_luma(FrameFormat::RAWRGB, 1, 1, &[0, 255, 0]).unwrap()[0];
        let blue = to_luma(FrameFormat::RAWRGB, 1, 1, &[0, 0, 255]).unwrap()[0];
        let red = to_luma(FrameFormat::RAWRGB, 1, 1, &red_rgb).unwrap()[0];
        assert!(green > red && red > blue);
    }

    /// Decoding MJPEG would mean linking a JPEG library, so it is refused rather
    /// than silently producing a frame the scanner cannot read.
    #[test]
    fn mjpeg_is_refused() {
        assert_eq!(to_luma(FrameFormat::MJPEG, 1, 1, &[0; 64]), None);
    }

    #[test]
    fn a_frame_of_the_wrong_length_is_refused() {
        assert_eq!(to_luma(FrameFormat::YUYV, 4, 1, &[0; 7]), None);
        assert_eq!(to_luma(FrameFormat::GRAY, 4, 1, &[0; 3]), None);
        assert_eq!(to_luma(FrameFormat::RAWRGB, 4, 1, &[0; 11]), None);
    }

    /// MJPEG is usually advertised first and at the highest frame rate, so the
    /// negotiation has to skip it even when it looks like the best mode.
    #[test]
    fn a_compressed_mode_is_never_negotiated() {
        let formats = [
            format(FrameFormat::MJPEG, 1280, 720, 60),
            format(FrameFormat::YUYV, 640, 480, 30),
        ];
        let chosen = preferred_camera_format(&formats).unwrap();
        assert_eq!(chosen.format(), FrameFormat::YUYV);
    }

    #[test]
    fn a_camera_with_only_compressed_modes_negotiates_nothing() {
        let formats = [format(FrameFormat::MJPEG, 1280, 720, 30)];
        assert!(preferred_camera_format(&formats).is_none());
    }

    /// Closest to 720p at a real-time frame rate, not simply the largest.
    #[test]
    fn the_closest_realtime_mode_wins() {
        let formats = [
            format(FrameFormat::YUYV, 320, 240, 30),
            format(FrameFormat::YUYV, 1280, 720, 30),
            format(FrameFormat::YUYV, 1920, 1080, 6),
        ];
        let chosen = preferred_camera_format(&formats).unwrap();
        assert_eq!((chosen.width(), chosen.height()), (1280, 720));
    }

    /// A camera that cannot reach a real-time rate at all still has to be
    /// usable, just slowly.
    #[test]
    fn a_slow_camera_still_negotiates() {
        let formats = [format(FrameFormat::YUYV, 1280, 720, 6)];
        assert!(preferred_camera_format(&formats).is_some());
    }

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

    /// A stalled scan must still leave every frame it did read in the log, so
    /// each distinct QR is reported once no matter how long it stays on camera.
    #[test]
    fn every_distinct_frame_is_reported_once() {
        let encoder = Encoder::new(Config {
            max_qr_version: 11,
            bbqr_part_bytes: 200,
            ..Config::default()
        })
        .unwrap();
        let frames = encoder.encode_bytes(&vec![7u8; 500]).unwrap();
        assert!(frames.len() > 1, "this payload should need several frames");

        let mut log = FrameLog::default();
        let first: Vec<Vec<u8>> = frames.iter().flat_map(|f| log.unseen(f)).collect();
        assert_eq!(first.len(), frames.len());
        assert!(first.iter().all(|payload| payload.starts_with(b"B$HB")));

        // the animation loops, and a frame already read is not reported again
        let second: Vec<Vec<u8>> = frames.iter().flat_map(|f| log.unseen(f)).collect();
        assert!(second.is_empty());
    }

    #[test]
    fn an_unreadable_frame_reports_nothing() {
        let mut log = FrameLog::default();
        let blank = Image {
            data: vec![255; 64 * 64],
            width: 64,
            height: 64,
        };
        assert!(log.unseen(&blank).is_empty());
    }
}
