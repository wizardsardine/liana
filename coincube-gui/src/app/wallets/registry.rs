//! Registry that owns the app's wallet backends and exposes routing hooks.
//!
//! Holds one [`LiquidBackend`] (always present) and an optional
//! [`SparkBackend`] (present when the cube has a Spark signer and
//! the bridge subprocess spawned successfully).
//! [`WalletRegistry::route_lightning_address`] returns the backend that
//! should fulfill the next incoming Lightning Address invoice: Spark
//! when available, Liquid otherwise.
//!
//! The registry is the single place the app decides *which* backend
//! handles *which* payment type — keeping that logic in one module
//! means the routing policy is a one-file change.

use std::sync::Arc;

use super::liquid::LiquidBackend;
use super::spark::SparkBackend;

/// Which backend a routing decision picked. Carries the backend handle
/// so callers don't have to re-resolve it from the registry.
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
    /// requests: Spark when the bridge is up, Liquid otherwise.
    ///
    /// Falling back to Liquid keeps invoice requests answerable even when
    /// the Spark setup is broken (no signer, subprocess crashed, etc.).
    pub fn route_lightning_address(&self) -> LightningRoute {
        match self.spark.clone() {
            Some(spark) => LightningRoute::Spark(spark),
            None => LightningRoute::Liquid(self.liquid.clone()),
        }
    }
}
