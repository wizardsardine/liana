//! Wallet backend abstraction layer.
//!
//! This module sits between the SDK-specific wrappers (the Spark bridge
//! client in [`crate::app::breez_spark`]) and the UI panels. It exposes
//! domain types and the concrete [`SparkBackend`] so panels never see
//! SDK-specific payment types.

pub mod registry;
pub mod spark;
pub mod types;

pub use registry::{LightningRoute, WalletRegistry};
pub use spark::SparkBackend;
pub use types::DomainPaymentStatus;
