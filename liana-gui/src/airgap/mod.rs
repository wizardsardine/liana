//! Typed, bounded transport primitives for air-gapped signing methods.
//!
//! Protocol parsing is independent from installer and wallet screens.
//! Untrusted QR/file input is bounded and decoded here before a UI flow sees a
//! protocol value; the camera module feeds that same decoder without persisting
//! frames.

mod animation;
mod camera;
mod device;
mod error;
mod passport;
mod payload;
mod session;
mod ur;

pub use animation::{AnimatedQr, AnimationState};
pub use camera::{
    request_camera_access, CameraDescriptor, CameraEvent, CameraFailure, CameraScanner,
};
pub use device::{AirgappedSignerConfig, AirgappedSignerKind, RegistrationState};
pub use error::Error;
pub use passport::{
    AddressVerificationRequest, PolicyNetwork, PolicyRegistration, VerifiedAddress,
};
pub use payload::{
    validate_and_merge_psbt, AirgappedRequest, AirgappedResponse, ExpectedResponse, PassportAccount,
};
pub use session::{DecodeProgress, ScanLimits, UrDecodeSession};
pub use ur::{encode_ur, EncodedUr, QrDensity, UrPayload, UrType};
