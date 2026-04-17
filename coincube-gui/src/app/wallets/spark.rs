//! Spark-specific backend adapter.
//!
//! [`SparkBackend`] wraps an [`Arc<SparkClient>`] and exposes the same
//! panel-facing read surface as [`LiquidBackend`] (`list_payments`,
//! `get_info`, …), plus a method-forwarding `client()` accessor so
//! write-path callers can reach the subprocess client directly without
//! the wallet abstraction getting in the way.
//!
//! Structurally this is the Spark counterpart to
//! [`crate::app::wallets::liquid::LiquidBackend`]. The two types
//! deliberately don't share a `WalletBackend` trait yet — the Liquid
//! backend's methods are sync/local, Spark's are async/IPC with a real
//! subprocess cost model, and sharing a trait would paper over that
//! difference without earning much. Registry routing (Phase 5 Lightning
//! Address handoff) will use an enum dispatch instead.

use std::sync::Arc;

use coincube_spark_protocol::{
    ClaimDepositOk, GetInfoOk, GetUserSettingsOk, ListPaymentsOk, ListUnclaimedDepositsOk,
    ParseInputOk, PrepareSendOk, ReceivePaymentOk, SendPaymentOk,
};

use crate::app::breez_spark::{SparkClient, SparkClientError, SparkClientEvent};

/// Spark backend: wraps [`SparkClient`] and exposes the domain read API.
///
/// Cheap to clone (internally an `Arc<SparkClient>`). Methods that return
/// domain types are defined inherently on `SparkBackend`; everything
/// else on [`SparkClient`] remains reachable via [`Self::client`].
#[derive(Clone, Debug)]
pub struct SparkBackend {
    client: Arc<SparkClient>,
}

impl SparkBackend {
    pub fn new(client: Arc<SparkClient>) -> Self {
        Self { client }
    }

    /// Access the underlying [`SparkClient`].
    pub fn client(&self) -> &Arc<SparkClient> {
        &self.client
    }

    /// Fetch wallet info (balance + identity pubkey).
    pub async fn get_info(&self) -> Result<GetInfoOk, SparkClientError> {
        self.client.get_info().await
    }

    /// List recent payments.
    pub async fn list_payments(
        &self,
        limit: Option<u32>,
    ) -> Result<ListPaymentsOk, SparkClientError> {
        self.client.list_payments(limit).await
    }

    /// Phase 4e: classify a destination string. The Send panel calls
    /// this before `prepare_send` so it can route LNURL inputs to
    /// `prepare_lnurl_pay` instead.
    pub async fn parse_input(&self, input: String) -> Result<ParseInputOk, SparkClientError> {
        self.client.parse_input(input).await
    }

    /// Phase 4c: parse a destination + compute send preview.
    pub async fn prepare_send(
        &self,
        input: String,
        amount_sat: Option<u64>,
    ) -> Result<PrepareSendOk, SparkClientError> {
        self.client.prepare_send(input, amount_sat).await
    }

    /// Phase 4e: prepare an LNURL-pay / Lightning-address send.
    /// Returns the same shape as `prepare_send` so the Send panel
    /// state machine doesn't need a parallel branch.
    pub async fn prepare_lnurl_pay(
        &self,
        input: String,
        amount_sat: u64,
        comment: Option<String>,
    ) -> Result<PrepareSendOk, SparkClientError> {
        self.client
            .prepare_lnurl_pay(input, amount_sat, comment)
            .await
    }

    /// Phase 4c: execute a previously-prepared send.
    pub async fn send_payment(
        &self,
        prepare_handle: String,
    ) -> Result<SendPaymentOk, SparkClientError> {
        self.client.send_payment(prepare_handle).await
    }

    /// Phase 4c: generate a BOLT11 invoice.
    pub async fn receive_bolt11(
        &self,
        amount_sat: Option<u64>,
        description: String,
        expiry_secs: Option<u32>,
    ) -> Result<ReceivePaymentOk, SparkClientError> {
        self.client
            .receive_bolt11(amount_sat, description, expiry_secs)
            .await
    }

    /// Phase 4c: generate an on-chain deposit address.
    pub async fn receive_onchain(
        &self,
        new_address: Option<bool>,
    ) -> Result<ReceivePaymentOk, SparkClientError> {
        self.client.receive_onchain(new_address).await
    }

    /// Phase 4f: list pending on-chain deposits.
    pub async fn list_unclaimed_deposits(
        &self,
    ) -> Result<ListUnclaimedDepositsOk, SparkClientError> {
        self.client.list_unclaimed_deposits().await
    }

    /// Phase 4f: claim a specific deposit by (txid, vout).
    pub async fn claim_deposit(
        &self,
        txid: String,
        vout: u32,
    ) -> Result<ClaimDepositOk, SparkClientError> {
        self.client.claim_deposit(txid, vout).await
    }

    /// Phase 6: read the Stable Balance toggle state.
    pub async fn get_user_settings(&self) -> Result<GetUserSettingsOk, SparkClientError> {
        self.client.get_user_settings().await
    }

    /// Phase 6: enable or disable Stable Balance.
    pub async fn set_stable_balance(&self, enabled: bool) -> Result<(), SparkClientError> {
        self.client.set_stable_balance(enabled).await
    }

    /// Build an iced [`Subscription`](iced::Subscription) over the
    /// bridge's event stream. Forwards through to
    /// [`SparkClient::event_subscription`]; the app-level runner
    /// wraps the resulting [`SparkClientEvent`] into
    /// [`crate::app::Message::SparkEvent`].
    pub fn event_subscription(&self) -> iced::Subscription<SparkClientEvent> {
        self.client.event_subscription()
    }

    /// Gracefully shut down the bridge subprocess. After this returns
    /// the backend (and any cloned handles) is unusable; further calls
    /// fail with [`SparkClientError::BridgeUnavailable`].
    pub async fn shutdown(&self) -> Result<(), SparkClientError> {
        self.client.shutdown().await
    }
}
