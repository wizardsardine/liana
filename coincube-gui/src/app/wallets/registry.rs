//! Registry that owns the app's wallet backends and exposes routing hooks.
//!
//! Holds an optional [`SparkBackend`], present when the cube has a Spark
//! signer and the bridge subprocess spawned successfully.
//! [`WalletRegistry::route_lightning_address`] returns the backend that
//! should fulfill the next incoming Lightning Address invoice: Spark when
//! available, or no route for Vault-only Cubes.
//!
//! The registry is the single place the app decides *which* backend
//! handles *which* payment type — keeping that logic in one module
//! means the routing policy is a one-file change.

use std::sync::Arc;

use super::spark::SparkBackend;

/// Which backend a routing decision picked. Carries the backend handle
/// so callers don't have to re-resolve it from the registry.
#[derive(Clone)]
pub enum LightningRoute {
    Spark(Arc<SparkBackend>),
}

/// Owns the per-cube wallet backends.
///
/// Cheap to clone — the backends live behind `Arc`s so clones share state.
#[derive(Clone, Default)]
pub struct WalletRegistry {
    /// `None` if the cube has no Spark signer configured, or if the
    /// bridge subprocess failed to spawn / handshake. Panels code
    /// checks this and shows a "Spark unavailable" placeholder when
    /// absent.
    spark: Option<Arc<SparkBackend>>,
}

impl WalletRegistry {
    /// Builds the registry from the Cube's Spark backend, if any. A Cube
    /// without one (Vault-only, or a Bitcoin Blake2b Cube) has no
    /// Lightning route.
    pub fn new(spark: Option<Arc<SparkBackend>>) -> Self {
        Self { spark }
    }

    /// Access the Spark backend, if the bridge is up for this cube.
    pub fn spark(&self) -> Option<&Arc<SparkBackend>> {
        self.spark.as_ref()
    }

    /// Returns the backend that should fulfill incoming Lightning Address
    /// requests: Spark when the bridge is up, otherwise none.
    pub fn route_lightning_address(&self) -> Option<LightningRoute> {
        self.spark.clone().map(LightningRoute::Spark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_spark_there_is_no_backend_or_lightning_route() {
        let registry = WalletRegistry::new(None);
        assert!(registry.spark().is_none());
        assert!(registry.route_lightning_address().is_none());
    }

    /// Spark no longer rides on a Liquid client: a registry built from a
    /// Spark backend alone exposes it and routes Lightning to it.
    #[test]
    fn spark_backend_alone_is_exposed_and_routed() {
        let spark = Arc::new(SparkBackend::new(Arc::new(
            crate::app::breez_spark::SparkClient::disconnected_for_test(),
        )));
        let registry = WalletRegistry::new(Some(spark.clone()));
        assert!(registry.spark().is_some_and(|s| Arc::ptr_eq(s, &spark)));
        match registry.route_lightning_address() {
            Some(LightningRoute::Spark(routed)) => assert!(Arc::ptr_eq(&routed, &spark)),
            None => panic!("a Spark backend must receive Lightning Address payments"),
        }
    }
}
