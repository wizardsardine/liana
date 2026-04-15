//! Liquid-specific backend adapter.
//!
//! [`LiquidBackend`] wraps an [`Arc<BreezClient>`] and exposes domain-typed
//! read methods ([`DomainPayment`] / [`DomainRefundableSwap`]) on top of the
//! SDK. All other [`BreezClient`] methods remain accessible via `Deref`, so
//! callers that still need raw SDK types (send/receive flows, event handling)
//! don't have to change anything beyond the field type.
//!
//! This is the first concrete wallet backend in the [`crate::app::wallets`]
//! layer. [`SparkBackend`] will land alongside it in Phase 3 and they will
//! share a [`WalletBackend`] trait at that point — introducing the trait now
//! would be guessing at the interface before seeing a second implementor.
//!
//! [`SparkBackend`]: crate::app::wallets
//! [`WalletBackend`]: crate::app::wallets

use std::ops::Deref;
use std::sync::Arc;

use crate::app::breez_liquid::{BreezClient, BreezError};

use super::types::{DomainPayment, DomainRefundableSwap};

/// Liquid backend: wraps [`BreezClient`] and exposes the domain read API.
///
/// Cheap to clone (internally an `Arc<BreezClient>`). Methods that return
/// domain types are defined inherently on `LiquidBackend`; everything else
/// on [`BreezClient`] remains reachable via `Deref`.
#[derive(Clone, Debug)]
pub struct LiquidBackend {
    client: Arc<BreezClient>,
}

impl LiquidBackend {
    pub fn new(client: Arc<BreezClient>) -> Self {
        Self { client }
    }

    /// Access the underlying [`BreezClient`] when you explicitly want the raw
    /// SDK handle (e.g. passing it to code that still expects `Arc<BreezClient>`).
    ///
    /// Most call sites can reach SDK methods directly via `Deref` — prefer
    /// `self.backend.some_sdk_method()` over `self.backend.client().some_sdk_method()`.
    pub fn client(&self) -> &Arc<BreezClient> {
        &self.client
    }

    /// Fetch the payment history and map each record into a [`DomainPayment`].
    ///
    /// `limit` mirrors the underlying [`BreezClient::list_payments`] parameter.
    pub async fn list_payments(
        &self,
        limit: Option<u32>,
    ) -> Result<Vec<DomainPayment>, BreezError> {
        let payments = self.client.list_payments(limit).await?;
        Ok(payments.into_iter().map(DomainPayment::from).collect())
    }

    /// Fetch refundable swaps and map each record into a [`DomainRefundableSwap`].
    pub async fn list_refundables(&self) -> Result<Vec<DomainRefundableSwap>, BreezError> {
        let refundables = self.client.list_refundables().await?;
        Ok(refundables
            .into_iter()
            .map(DomainRefundableSwap::from)
            .collect())
    }
}

impl Deref for LiquidBackend {
    type Target = BreezClient;

    fn deref(&self) -> &BreezClient {
        &self.client
    }
}
