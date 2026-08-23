//! Air-gapped signing over QR codes.
//!
//! Liana is always the wallet side of the exchange: it shows a request as
//! animated QR frames and scans the signer's answer back. The wire format is
//! the signing-flow protocol encoded by `bip377`, framed with BBQR; this module
//! holds the Liana side of the exchange.

pub mod animation;
pub mod camera;
pub mod device;
pub mod display;
pub mod error;
pub mod exchange;
mod merge;
