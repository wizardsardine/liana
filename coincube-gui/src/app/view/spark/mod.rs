//! View-layer types for the Spark wallet panels.
//!
//! Modules: Overview (balance + "Stable" badge), Send (BOLT11 /
//! BIP21 / LNURL-pay), Receive (BOLT11 invoice, on-chain deposit
//! address with claim lifecycle), Transactions (recent payments),
//! and Settings (Stable Balance toggle, default Lightning backend
//! picker, diagnostics). [`SparkPlaceholderView`] is kept around
//! as a generic "coming soon" slot for future panels.

pub mod overview;
pub mod receive;
pub mod send;
pub mod settings;
pub mod transactions;

pub use overview::{SparkOverviewView, SparkStatus};
pub use receive::SparkReceiveView;
pub use send::SparkSendView;
pub use settings::{SparkSettingsStatus, SparkSettingsView};
pub use transactions::{SparkTransactionsStatus, SparkTransactionsView};

/// View-level messages for the Spark Overview panel.
#[derive(Debug, Clone)]
pub enum SparkOverviewMessage {
    /// Bridge returned `get_info` success — carries a snapshot.
    DataLoaded(crate::app::state::spark::overview::SparkBalanceSnapshot),
    /// Bridge returned an error response for `get_info`.
    Error(String),
    /// Phase 6: bridge returned the current Stable Balance flag,
    /// fetched alongside `get_info` in `reload`. Drives the
    /// "Stable" badge next to the balance line.
    StableBalanceLoaded(bool),
}

/// View-level messages for the Phase 4b Spark Transactions panel.
#[derive(Debug, Clone)]
pub enum SparkTransactionsMessage {
    /// Bridge returned `list_payments` success — carries the page.
    DataLoaded(Vec<coincube_spark_protocol::PaymentSummary>),
    /// Bridge returned an error response for `list_payments`.
    Error(String),
}

/// View-level messages for the Phase 4b Spark Settings panel.
#[derive(Debug, Clone)]
pub enum SparkSettingsMessage {
    /// Bridge returned `get_info` success — carries a snapshot used
    /// for the read-only diagnostics view.
    DataLoaded(crate::app::state::spark::settings::SparkSettingsSnapshot),
    /// Bridge returned an error response.
    Error(String),
    /// Phase 5: user clicked a radio in the "Default Lightning backend"
    /// picker. The state panel persists the new value to the cube
    /// settings file and emits `Message::SettingsSaved` on success.
    DefaultLightningBackendChanged(crate::app::wallets::WalletKind),
    /// Phase 5: the background save task finished. Carries an error
    /// string if persistence failed, else `None`.
    DefaultLightningBackendSaved(Option<String>),
    /// Phase 6: bridge returned the current Stable Balance + private
    /// mode state. Fired from the panel's `reload` task so the view
    /// can reflect whatever the SDK persisted across restarts.
    UserSettingsLoaded(coincube_spark_protocol::GetUserSettingsOk),
    /// Phase 6: the user flipped the Stable Balance toggle — fires a
    /// `set_stable_balance` RPC on the bridge.
    StableBalanceToggled(bool),
    /// Phase 6: `set_stable_balance` RPC finished. `Ok(enabled)`
    /// carries the new state so the view can update immediately
    /// without re-fetching; `Err` surfaces the SDK error.
    StableBalanceSaved(Result<bool, String>),
}

/// View-level messages for the Phase 4c Spark Send panel. Drives the
/// state machine in [`crate::app::state::spark::send::SparkSend`].
#[derive(Debug, Clone)]
pub enum SparkSendMessage {
    DestinationInputChanged(String),
    AmountInputChanged(String),
    PrepareRequested,
    PrepareSucceeded(coincube_spark_protocol::PrepareSendOk),
    PrepareFailed(String),
    ConfirmRequested,
    SendSucceeded(coincube_spark_protocol::SendPaymentOk),
    SendFailed(String),
    /// Reset back to the `Idle` phase, clearing inputs and any
    /// prepared/sent state. Fired from the "Send another" / "Try
    /// again" / "Cancel" buttons.
    Reset,
}

/// View-level messages for the Phase 4c Spark Receive panel.
#[derive(Debug, Clone)]
pub enum SparkReceiveMessage {
    MethodSelected(crate::app::state::spark::receive::SparkReceiveMethod),
    AmountInputChanged(String),
    DescriptionInputChanged(String),
    GenerateRequested,
    GenerateSucceeded(coincube_spark_protocol::ReceivePaymentOk),
    GenerateFailed(String),
    /// Forwarded from the app-level Spark event handler when a
    /// `PaymentSucceeded` event arrives. Carries the payment's
    /// amount (signed sats — positive for incoming) and an optional
    /// BOLT11 string from the SDK Payment's details. Phase 4f
    /// uses the BOLT11 to correlate against the panel's currently
    /// displayed invoice — events for unrelated payments are
    /// ignored. Pre-Phase-4f BOLT11-less events (Spark-native /
    /// on-chain / token) still trigger the auto-advance.
    PaymentReceived {
        amount_sat: i64,
        bolt11: Option<String>,
    },
    /// Phase 4f: a `Method::ListUnclaimedDeposits` RPC came back with
    /// a fresh deposit list.
    PendingDepositsLoaded(Vec<coincube_spark_protocol::DepositInfo>),
    /// Phase 4f: a `Method::ListUnclaimedDeposits` RPC failed. We
    /// log + clear the displayed list rather than surface a hard
    /// error in the UI — the panel's primary purpose is generating
    /// invoices, not managing deposits, so a deposits-list failure
    /// shouldn't block the rest of the panel.
    PendingDepositsFailed(String),
    /// Phase 4f: user clicked "Claim" on a specific (txid, vout).
    ClaimDepositRequested {
        txid: String,
        vout: u32,
    },
    /// Phase 4f: a `claim_deposit` RPC succeeded. Triggers a deposits
    /// reload so the row disappears.
    ClaimDepositSucceeded(coincube_spark_protocol::ClaimDepositOk),
    /// Phase 4f: a `claim_deposit` RPC failed. Surface the SDK error
    /// in the panel and keep the row.
    ClaimDepositFailed(String),
    /// Phase 4f: app-level signal that the bridge emitted a
    /// `DepositsChanged` event. The panel re-fetches the list.
    DepositsChanged,
    Reset,
}
