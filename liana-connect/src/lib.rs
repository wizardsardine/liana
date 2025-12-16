//! Liana Connect
//!
//! This crate provides shared protocol types and domain models for
//! Liana Connect client/server communication.

pub mod ws_business;

// Re-export ws_business types at crate root for convenience
pub use ws_business::*;
