//! Registry that owns the app's wallet backends and exposes routing hooks.
//!
//! Holds one [`LiquidBackend`] (always present) and an optional
//! [`SparkBackend`] (present when the cube has a Spark signer and
//! the bridge subprocess spawned successfully).
//! [`WalletRegistry::route_lightning_address`] consults the cube's
//! `default_lightning_backend` setting and returns the backend that
//! should fulfill the next incoming Lightning Address invoice.
//!
//! The registry is the single place the app decides *which* backend
//! handles *which* payment type — keeping that logic in one module
//! means the "Spark default / Liquid advanced" policy is a one-file
//! change.

use std::sync::Arc;

use super::liquid::LiquidBackend;
use super::spark::SparkBackend;
use super::types::WalletKind;

/// Which backend a routing decision picked. Carries the backend handle
/// so callers don't have to re-resolve it from the registry.
///
/// The `Spark` variant only appears when the Spark backend is
/// actually present — callers never see `Spark(None)`.
#[derive(Clone)]
pub enum LightningRoute {
    Liquid(Arc<LiquidBackend>),
    Spark(Arc<SparkBackend>),
}

/// Owns the per-cube wallet backends.
///
/// Cheap to clone — the backends live behind `Arc`s so clones share state.
#[derive(Clone)]
pub struct WalletRegistry {
    liquid: Arc<LiquidBackend>,
    /// `None` if the cube has no Spark signer configured, or if the
    /// bridge subprocess failed to spawn / handshake. Panels code
    /// checks this and shows a "Spark unavailable" placeholder when
    /// absent.
    spark: Option<Arc<SparkBackend>>,
}

impl WalletRegistry {
    pub fn new(liquid: Arc<LiquidBackend>) -> Self {
        Self {
            liquid,
            spark: None,
        }
    }

    pub fn with_spark(liquid: Arc<LiquidBackend>, spark: Option<Arc<SparkBackend>>) -> Self {
        Self { liquid, spark }
    }

    /// Access the Liquid backend.
    pub fn liquid(&self) -> &Arc<LiquidBackend> {
        &self.liquid
    }

    /// Access the Spark backend, if the bridge is up for this cube.
    pub fn spark(&self) -> Option<&Arc<SparkBackend>> {
        self.spark.as_ref()
    }

    /// Returns the backend that should fulfill incoming Lightning Address
    /// requests given the cube's preference.
    ///
    /// Falls back to Liquid when the caller asks for Spark but the
    /// bridge is unavailable (no signer, subprocess crashed, etc.),
    /// so a misconfigured Spark setup never loses invoice requests.
    pub fn route_lightning_address(&self, preferred: WalletKind) -> LightningRoute {
        match resolve_lightning_backend(preferred, self.spark.is_some()) {
            WalletKind::Spark => LightningRoute::Spark(
                self.spark
                    .clone()
                    .expect("resolve_lightning_backend only returns Spark when spark is Some"),
            ),
            WalletKind::Liquid => LightningRoute::Liquid(self.liquid.clone()),
        }
    }
}

/// Pure decision function for [`WalletRegistry::route_lightning_address`].
///
/// Split out from the registry method so it's unit-testable without
/// having to construct real backend handles (the backends wrap SDK
/// clients that require network/process resources).
///
/// Rules: honor `preferred` when Spark is available, fall back to
/// Liquid otherwise. Liquid requests stay on Liquid.
fn resolve_lightning_backend(preferred: WalletKind, spark_available: bool) -> WalletKind {
    match preferred {
        WalletKind::Spark if spark_available => WalletKind::Spark,
        WalletKind::Spark => WalletKind::Liquid,
        WalletKind::Liquid => WalletKind::Liquid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spark_preferred_and_available_returns_spark() {
        assert_eq!(
            resolve_lightning_backend(WalletKind::Spark, true),
            WalletKind::Spark
        );
    }

    #[test]
    fn spark_preferred_but_unavailable_falls_back_to_liquid() {
        // Core Phase 5 guarantee: a misconfigured Spark setup
        // never drops incoming invoice requests.
        assert_eq!(
            resolve_lightning_backend(WalletKind::Spark, false),
            WalletKind::Liquid
        );
    }

    #[test]
    fn liquid_preferred_stays_on_liquid_regardless_of_spark_availability() {
        assert_eq!(
            resolve_lightning_backend(WalletKind::Liquid, true),
            WalletKind::Liquid
        );
        assert_eq!(
            resolve_lightning_backend(WalletKind::Liquid, false),
            WalletKind::Liquid
        );
    }

    #[test]
    fn default_wallet_kind_is_spark() {
        // Phase 5 contract: new cubes land on Spark for Lightning
        // fulfilment. Changing this is a product-level decision,
        // not a refactor — if this test fails, the default flip
        // needs to be deliberate and documented in the release
        // notes.
        assert_eq!(WalletKind::default(), WalletKind::Spark);
    }
}
