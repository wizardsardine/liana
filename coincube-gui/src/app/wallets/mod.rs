//! Wallet backend abstraction layer.
//!
//! This module sits between the SDK-specific wrappers (e.g.
//! [`crate::app::breez_liquid`]) and the UI panels. It exposes domain types
//! and a concrete [`LiquidBackend`] so panels can be ported to additional
//! backends (Spark next) without leaking SDK-specific payment types into the
//! UI code.
//!
//! The read surface is complete: panels consume [`DomainPayment`] and
//! [`DomainRefundableSwap`] everywhere. The write surface (send/receive,
//! refunds, swaps) is still Liquid-specific and reachable via
//! [`LiquidBackend::client`] or `Deref` — that layer will grow when the Spark
//! backend lands and we extract a shared `WalletBackend` trait from the
//! concrete implementations.

pub mod liquid;
pub mod registry;
pub mod spark;
pub mod types;

pub use liquid::LiquidBackend;
pub use registry::{LightningRoute, WalletRegistry};
pub use spark::SparkBackend;
pub use types::{
    DomainLiquidAssetInfo, DomainPayment, DomainPaymentDetails, DomainPaymentDirection,
    DomainPaymentStatus, DomainRefundableSwap, WalletKind,
};
