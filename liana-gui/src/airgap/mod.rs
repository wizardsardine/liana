//! Air-gapped signing over QR codes.
//!
//! Liana is always the wallet side of the exchange: it shows a request as
//! animated QR frames and scans the signer's answer back. The wire format is
//! the bwk signing-flow protocol, framed with BBQR; this module holds the
//! Liana-side request building, answer checking and camera capture.

mod animation;
mod camera;
mod device;
mod error;
mod exchange;
mod merge;

pub use animation::{AnimatedQr, AnimationState, FRAMES_PER_SECOND};
pub use camera::{
    request_camera_access, CameraDescriptor, CameraEvent, CameraFailure, CameraScanner,
};
pub use device::{AirgappedSignerConfig, Capabilities, FirmwareVersion, Prerelease, Registration};
pub use error::Error;
pub use exchange::{Answer, Ask, Exchange, Signer, Wallet, ACCOUNTS};
