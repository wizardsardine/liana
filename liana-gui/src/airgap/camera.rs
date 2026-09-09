use std::{
    convert::TryFrom,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(target_os = "macos")]
use nokhwa::utils::FrameFormat;
use nokhwa::utils::{ApiBackend, CameraFormat, CameraIndex};
use rxing::{BarcodeFormat, DecodeHints};

#[cfg(not(target_os = "macos"))]
use nokhwa::{
    utils::{RequestedFormat, RequestedFormatType},
    Camera,
};

#[cfg(any(not(target_os = "macos"), test))]
use nokhwa::pixel_format::{FormatDecoder, RgbFormat};

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
    std::ffi::CString,
};

use super::{DecodeProgress, ScanLimits, UrDecodeSession, UrPayload, UrType};

#[cfg(any(not(target_os = "macos"), test))]
const TARGET_CAPTURE_WIDTH: u32 = 1280;
#[cfg(any(not(target_os = "macos"), test))]
const TARGET_CAPTURE_HEIGHT: u32 = 720;
#[cfg(any(not(target_os = "macos"), test))]
const TARGET_CAPTURE_FPS: u32 = 30;
#[cfg(any(not(target_os = "macos"), test))]
const MIN_REALTIME_FPS: u32 = 24;
const PREVIEW_MAX_WIDTH: u32 = 640;
const PREVIEW_MAX_HEIGHT: u32 = 480;
const PREVIEW_INTERVAL: Duration = Duration::from_millis(33);
const DECODE_INTERVAL: Duration = Duration::from_millis(200);
const EVENT_QUEUE: usize = 3;

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
}

impl std::fmt::Display for CameraFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(formatter, "camera permission denied"),
            Self::PermissionTimedOut => write!(formatter, "camera permission request timed out"),
            Self::Unavailable => write!(formatter, "no camera is available"),
            Self::Busy => write!(formatter, "camera is already in use"),
            Self::Capture(error) => write!(formatter, "camera capture failed: {error}"),
            Self::InvalidFrame => write!(formatter, "camera returned an invalid frame"),
        }
    }
}

impl std::error::Error for CameraFailure {}

#[derive(Debug, Clone, PartialEq)]
pub enum CameraEvent {
    Preview {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Progress {
        estimated: f32,
        detected_frames: u32,
    },
    Rejected(String),
    Complete(UrPayload),
    Failure(CameraFailure),
}

/// Requests camera access. On non-macOS platforms the callback completes
/// immediately; macOS uses AVFoundation's permission callback.
fn initialize_camera(callback: impl Fn(bool) + Send + Sync + 'static) {
    nokhwa::nokhwa_initialize(callback);
}

/// Requests access without blocking Iced's update loop and returns the cameras
/// that can be offered to the user. The callback is bounded because some
/// platform backends can fail to answer when their permission service is
/// unavailable.
pub async fn request_camera_access() -> Result<Vec<CameraDescriptor>, CameraFailure> {
    if camera_permission_granted() {
        return list_cameras();
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    initialize_camera(move |granted| {
        if let Some(sender) = sender
            .lock()
            .expect("camera permission lock poisoned")
            .take()
        {
            let _ = sender.send(granted);
        }
    });
    let granted = tokio::time::timeout(Duration::from_secs(30), receiver)
        .await
        .map_err(|_| CameraFailure::PermissionTimedOut)?
        .map_err(|_| CameraFailure::Unavailable)?;
    if !granted {
        return Err(CameraFailure::PermissionDenied);
    }
    list_cameras()
}

fn camera_permission_granted() -> bool {
    nokhwa::nokhwa_check()
}

fn list_cameras() -> Result<Vec<CameraDescriptor>, CameraFailure> {
    nokhwa::query(ApiBackend::Auto)
        .map_err(map_camera_error)
        .map(|cameras| {
            cameras
                .into_iter()
                .map(|camera| CameraDescriptor {
                    index: camera.index().clone(),
                    name: camera.human_name(),
                })
                .collect()
        })
}

/// Owns a camera worker. Dropping or cancelling it stops the capture loop,
/// clears the UR session, and closes the native stream through `Camera::drop`.
pub struct CameraScanner {
    stop: Arc<AtomicBool>,
    events: Receiver<CameraEvent>,
    worker: Option<JoinHandle<()>>,
}

impl CameraScanner {
    pub fn start(
        index: CameraIndex,
        expected: UrType,
        limits: ScanLimits,
    ) -> Result<Self, CameraFailure> {
        if !camera_permission_granted() {
            return Err(CameraFailure::PermissionDenied);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (sender, events) = mpsc::sync_channel(EVENT_QUEUE);
        let worker = thread::Builder::new()
            .name("liana-qr-camera".to_owned())
            .spawn(move || run_camera(index, expected, limits, worker_stop, sender))
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
    expected: UrType,
    limits: ScanLimits,
    stop: Arc<AtomicBool>,
    sender: SyncSender<CameraEvent>,
) {
    let mut camera = match open_camera(index) {
        Ok(camera) => camera,
        Err(failure) => {
            send_terminal_event(&sender, &stop, CameraEvent::Failure(failure));
            return;
        }
    };
    if let Err(failure) = start_camera_stream(&mut camera) {
        send_terminal_event(&sender, &stop, CameraEvent::Failure(failure));
        return;
    }

    let mut ur = UrDecodeSession::new(expected, limits);
    let session_started = Instant::now();
    let mut qr = quircs::Quirc::default();
    let mut last_preview = Instant::now() - PREVIEW_INTERVAL;
    let mut last_decode = Instant::now() - DECODE_INTERVAL;
    let mut detected_frames = 0u32;
    while !stop.load(Ordering::Acquire) {
        if session_started.elapsed() > limits.timeout {
            send_terminal_event(
                &sender,
                &stop,
                CameraEvent::Failure(CameraFailure::Capture(super::Error::TimedOut.to_string())),
            );
            break;
        }
        let (width, height, raw) = match read_camera_frame_rgb(&mut camera, &stop) {
            Ok(frame) => frame,
            Err(failure) => {
                send_terminal_event(&sender, &stop, CameraEvent::Failure(failure));
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
            let Some(luma) = rgb_to_luma(width, height, &raw) else {
                send_terminal_event(
                    &sender,
                    &stop,
                    CameraEvent::Failure(CameraFailure::InvalidFrame),
                );
                break;
            };

            for value in decode_qr_frame(&mut qr, width as usize, height as usize, &luma) {
                let Ok(value) = value else {
                    continue;
                };
                match ur.receive(&value) {
                    Ok(DecodeProgress::Incomplete { estimated }) => {
                        detected_frames = detected_frames.saturating_add(1);
                        let _ = sender.try_send(CameraEvent::Progress {
                            estimated,
                            detected_frames,
                        });
                    }
                    Ok(DecodeProgress::Complete(payload)) => {
                        send_terminal_event(&sender, &stop, CameraEvent::Complete(payload));
                        stop.store(true, Ordering::Release);
                        break;
                    }
                    Err(super::Error::Empty | super::Error::InvalidUr(_)) => {
                        // A camera may see unrelated text or ordinary QR codes.
                        // They are not part of this bounded UR session.
                    }
                    Err(error) => {
                        if matches!(error, super::Error::MixedSession) {
                            ur.restart();
                        }
                        let _ = sender.try_send(CameraEvent::Rejected(error.to_string()));
                    }
                }
            }
        }

        if preview_due && !stop.load(Ordering::Acquire) {
            last_preview = now;
            let Some((preview_width, preview_height, rgba)) =
                rgb_to_preview_rgba(width, height, &raw)
            else {
                send_terminal_event(
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
    ur.cancel();
    stop_camera_stream(&mut camera);
}

/// Deliver completion and failure events without making cancellation wait for
/// a full UI queue. Preview/progress events are deliberately lossy; terminal
/// events retry until consumed, disconnected, or the scanner is cancelled.
fn send_terminal_event(
    sender: &SyncSender<CameraEvent>,
    stop: &AtomicBool,
    mut event: CameraEvent,
) {
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
    // Start with a backend-supported RGB-decodable format, then select an
    // advertised real-time mode close to 720p. Requesting the absolute highest
    // frame rate can also select a multi-megapixel stream whose conversion and
    // QR detection make the preview substantially less responsive.
    let requested = RequestedFormat::new::<RgbFormat>(RequestedFormatType::None);
    let mut camera = Camera::new(index, requested).map_err(map_camera_error)?;
    if let Ok(formats) = camera.compatible_camera_formats() {
        if let Some(format) = preferred_camera_format(&formats) {
            camera
                .set_camera_requset(RequestedFormat::new::<RgbFormat>(
                    RequestedFormatType::Exact(format),
                ))
                .map_err(map_camera_error)?;
        }
    }
    Ok(camera)
}

#[cfg(not(target_os = "macos"))]
fn start_camera_stream(camera: &mut PlatformCamera) -> Result<(), CameraFailure> {
    camera.open_stream().map_err(map_camera_error)
}

#[cfg(not(target_os = "macos"))]
fn read_camera_frame_rgb(
    camera: &mut PlatformCamera,
    _stop: &AtomicBool,
) -> Result<(u32, u32, Vec<u8>), CameraFailure> {
    let image = camera
        .frame()
        .map_err(map_camera_error)?
        .decode_image::<RgbFormat>()
        .map_err(map_camera_error)?;
    let (width, height) = image.dimensions();
    Ok((width, height, image.into_raw()))
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
fn read_camera_frame_rgb(
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
    Ok((width, height, bytes))
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
        RgbFormat::FORMATS.contains(&format.format()) && format.frame_rate() >= MIN_REALTIME_FPS
    });
    formats
        .iter()
        .copied()
        .filter(|format| RgbFormat::FORMATS.contains(&format.format()))
        .filter(|format| !has_realtime_format || format.frame_rate() >= MIN_REALTIME_FPS)
        .min_by_key(|format| {
            let resolution_distance = format.width().abs_diff(TARGET_CAPTURE_WIDTH)
                + format.height().abs_diff(TARGET_CAPTURE_HEIGHT);
            let frame_rate_distance = format.frame_rate().abs_diff(TARGET_CAPTURE_FPS);
            (resolution_distance, frame_rate_distance)
        })
}

fn rgb_to_luma(width: u32, height: u32, rgb: &[u8]) -> Option<Vec<u8>> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    if rgb.len() != pixels.checked_mul(3)? {
        return None;
    }
    Some(
        rgb.chunks_exact(3)
            .map(|pixel| {
                ((u16::from(pixel[0]) * 77 + u16::from(pixel[1]) * 150 + u16::from(pixel[2]) * 29)
                    >> 8) as u8
            })
            .collect(),
    )
}

fn rgb_to_preview_rgba(width: u32, height: u32, rgb: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    if width == 0 || height == 0 || rgb.len() != pixels.checked_mul(3)? {
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
            let source = ((source_y as usize) * (width as usize) + source_x as usize) * 3;
            rgba.extend_from_slice(&[rgb[source], rgb[source + 1], rgb[source + 2], 255]);
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
        rgba[offset..offset + 4].copy_from_slice(&[0, 255, 102, 255]);
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

fn decode_qr_frame(
    decoder: &mut quircs::Quirc,
    width: usize,
    height: usize,
    luma: &[u8],
) -> Vec<Result<String, CameraFailure>> {
    if width.checked_mul(height) != Some(luma.len()) {
        return vec![Err(CameraFailure::InvalidFrame)];
    }
    let mut decoded = decoder
        .identify(width, height, luma)
        .filter_map(|code| code.ok())
        .map(|code| {
            code.decode()
                .map_err(|error| CameraFailure::Capture(error.to_string()))
                .and_then(|data| {
                    String::from_utf8(data.payload)
                        .map_err(|error| CameraFailure::Capture(error.to_string()))
                })
        })
        .collect::<Vec<_>>();
    if decoded.iter().any(Result::is_ok) {
        return decoded;
    }

    if let Some(value) = decode_qr_with_zxing(width, height, luma) {
        decoded.push(Ok(value));
        return decoded;
    }
    if let Some((crop_width, crop_height, crop)) = centered_square_luma(width, height, luma) {
        if let Some(value) = decode_qr_with_zxing(crop_width, crop_height, &crop) {
            decoded.push(Ok(value));
        }
    }
    decoded
}

fn decode_qr_with_zxing(width: usize, height: usize, luma: &[u8]) -> Option<String> {
    let mut hints = DecodeHints {
        TryHarder: Some(true),
        AlsoInverted: Some(true),
        ..DecodeHints::default()
    };
    rxing::helpers::detect_in_luma_with_hints(
        luma.to_vec(),
        u32::try_from(width).ok()?,
        u32::try_from(height).ok()?,
        Some(BarcodeFormat::QR_CODE),
        &mut hints,
    )
    .ok()
    .map(|result| result.getText().to_owned())
}

fn centered_square_luma(
    width: usize,
    height: usize,
    luma: &[u8],
) -> Option<(usize, usize, Vec<u8>)> {
    if width.checked_mul(height) != Some(luma.len()) {
        return None;
    }
    let side = width.min(height);
    let left = (width - side) / 2;
    let top = (height - side) / 2;
    let mut crop = Vec::with_capacity(side.checked_mul(side)?);
    for row in top..top + side {
        let start = row.checked_mul(width)?.checked_add(left)?;
        crop.extend_from_slice(&luma[start..start + side]);
    }
    Some((side, side, crop))
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
    use qrcode::{types::Color, QrCode};

    use super::*;

    fn qr_luma(value: &str, inverted: bool) -> (usize, Vec<u8>) {
        let code = QrCode::new(value).unwrap();
        let modules = code.width();
        let quiet = 4usize;
        let scale = 8usize;
        let side = (modules + quiet * 2) * scale;
        let colors = code.to_colors();
        let (light, dark) = if inverted { (0, 255) } else { (255, 0) };
        let mut pixels = vec![light; side * side];
        for y in 0..modules {
            for x in 0..modules {
                if colors[y * modules + x] == Color::Dark {
                    let left = (x + quiet) * scale;
                    let top = (y + quiet) * scale;
                    for row in top..top + scale {
                        pixels[row * side + left..row * side + left + scale].fill(dark);
                    }
                }
            }
        }
        (side, pixels)
    }

    #[test]
    fn synthetic_qr_frame_decodes_without_persistence() {
        let value = "ur:bytes/hdcxmybgmnkp";
        let (side, pixels) = qr_luma(value, false);
        let mut decoder = quircs::Quirc::default();
        assert_eq!(
            decode_qr_frame(&mut decoder, side, side, &pixels),
            vec![Ok(value.to_owned())]
        );
    }

    #[test]
    fn inverted_qr_frame_decodes_with_robust_fallback() {
        let value = "ur:bytes/hdcxmybgmnkp";
        let (side, pixels) = qr_luma(value, true);
        let mut decoder = quircs::Quirc::default();
        assert!(decode_qr_frame(&mut decoder, side, side, &pixels)
            .into_iter()
            .any(|result| result == Ok(value.to_owned())));
    }

    #[test]
    fn invalid_frame_dimensions_are_rejected() {
        let mut decoder = quircs::Quirc::default();
        assert_eq!(
            decode_qr_frame(&mut decoder, 10, 10, &[0; 99]),
            vec![Err(CameraFailure::InvalidFrame)]
        );
    }

    #[test]
    fn cancelled_scanner_does_not_block_on_a_full_event_queue() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .try_send(CameraEvent::Rejected("queued".to_owned()))
            .unwrap();
        let stop = AtomicBool::new(true);
        send_terminal_event(
            &sender,
            &stop,
            CameraEvent::Failure(CameraFailure::Unavailable),
        );
        assert_eq!(
            receiver.try_recv(),
            Ok(CameraEvent::Rejected("queued".to_owned()))
        );
    }

    #[test]
    fn camera_format_prefers_realtime_720p_without_selecting_4k() {
        use nokhwa::utils::{FrameFormat, Resolution};

        let formats = [
            CameraFormat::new(Resolution::new(3840, 2160), FrameFormat::MJPEG, 60),
            CameraFormat::new(Resolution::new(1280, 720), FrameFormat::MJPEG, 30),
            CameraFormat::new(Resolution::new(640, 480), FrameFormat::MJPEG, 30),
        ];
        assert_eq!(preferred_camera_format(&formats), Some(formats[1]));
    }

    #[test]
    fn camera_format_avoids_slow_modes_when_realtime_is_available() {
        use nokhwa::utils::{FrameFormat, Resolution};

        let formats = [
            CameraFormat::new(Resolution::new(1280, 720), FrameFormat::MJPEG, 5),
            CameraFormat::new(Resolution::new(640, 480), FrameFormat::MJPEG, 30),
        ];
        assert_eq!(preferred_camera_format(&formats), Some(formats[1]));
    }

    #[test]
    fn preview_is_bounded_and_mirrors_sampled_pixels() {
        let width = 1280;
        let height = 720;
        let mut rgb = vec![0; width as usize * height as usize * 3];
        rgb[..3].copy_from_slice(&[10, 20, 30]);
        rgb[(width as usize - 1) * 3..width as usize * 3].copy_from_slice(&[40, 50, 60]);
        let (preview_width, preview_height, rgba) =
            rgb_to_preview_rgba(width, height, &rgb).unwrap();
        assert_eq!((preview_width, preview_height), (640, 360));
        assert_eq!(&rgba[..4], &[40, 50, 60, 255]);
        assert_eq!(
            rgba.len(),
            preview_width as usize * preview_height as usize * 4
        );
        assert!(rgba
            .chunks_exact(4)
            .any(|pixel| pixel == [0, 255, 102, 255]));
    }
}
