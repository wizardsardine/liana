//! View-layer types for the Spark wallet panels.
//!
//! Modules: Overview (balance + "Stable" badge), Send (BOLT11 /
//! BIP21 / LNURL-pay), Receive (BOLT11 invoice, on-chain deposit
//! address with claim lifecycle), Transactions (recent payments),
//! and Settings (Stable Balance toggle, default Lightning backend
//! picker, diagnostics). [`SparkPlaceholderView`] is kept around
//! as a generic "coming soon" slot for future panels.

pub mod last_tx;
pub mod overview;
pub mod receive;
pub mod send;
pub mod settings;
pub mod sideshift_receive;
pub mod transactions;

pub use overview::{SparkOverviewView, SparkPaymentMethod, SparkRecentTransaction, SparkStatus};
pub use receive::{sender_picker_modal, SparkReceiveView};
pub use send::{send_target_picker_modal, SparkSendView};
pub use settings::{SparkSettingsStatus, SparkSettingsView};
pub use sideshift_receive::spark_sideshift_receive_view;
pub use transactions::{SparkTransactionsStatus, SparkTransactionsView};

/// View-level messages for the Spark Overview panel.
#[derive(Debug, Clone)]
pub enum SparkOverviewMessage {
    /// Bridge returned `get_info` + `list_payments` success.
    DataLoaded {
        balance: coincube_core::miniscript::bitcoin::Amount,
        /// USDB holding from `get_info`, when present. Folded into
        /// the unified portfolio total alongside the BTC balance.
        stable_balance: Option<coincube_spark_protocol::StableBalanceSnapshot>,
        recent_payments: Vec<coincube_spark_protocol::PaymentSummary>,
    },
    /// Bridge returned an error response.
    Error(String),
    /// Phase 6: bridge returned the current Stable Balance flag,
    /// fetched alongside `get_info` in `reload`. Drives the
    /// "Stable" badge next to the balance line.
    StableBalanceLoaded(bool),
    /// Sidebar / card actions that navigate to sibling Spark panels.
    SendBtc,
    ReceiveBtc,
    History,
    SelectTransaction(usize),
    /// Forwarded to the top-level handler to flip the global
    /// fiat-native ↔ bitcoin-native display mode.
    FlipDisplayMode,
}

/// View-level messages for the Spark Transactions panel.
#[derive(Debug, Clone)]
pub enum SparkTransactionsMessage {
    /// Bridge returned `list_payments` success. The `u64` is the
    /// fetch-generation token — the panel discards any response whose
    /// token isn't the latest, so a stale pagination response can't
    /// overwrite data from a newer reload.
    DataLoaded(u64, Vec<coincube_spark_protocol::PaymentSummary>),
    /// Bridge returned an error response for `list_payments`. The `u64`
    /// is the fetch-generation token (see [`Self::DataLoaded`]).
    Error(u64, String),
    /// User clicked a row. Opens the detail pane for the payment
    /// at that index in the current `recent_transactions` list.
    Select(usize),
    /// Preselect a specific payment to display in the detail pane —
    /// used by Overview / Send / Receive to hand off a row when the
    /// user taps there. The panel switches into detail mode without
    /// needing an index lookup against its own list.
    Preselect(crate::app::view::spark::SparkRecentTransaction),
    /// Empty-state navigation: "Send sats" button.
    SendBtc,
    /// Empty-state navigation: "Receive sats" button.
    ReceiveBtc,
    /// Go to the previous page in the paginated list.
    PrevPage,
    /// Go to the next page in the paginated list.
    NextPage,
}

/// View-level messages for the Spark Settings panel.
#[derive(Debug, Clone)]
pub enum SparkSettingsMessage {
    /// Bridge `get_info` reload succeeded — the subprocess is
    /// reachable and the SDK is past init. Drives the "Bridge
    /// status" card on the Settings page.
    BridgeReachable,
    /// Bridge `get_info` reload failed. Carries the error string
    /// for the diagnostic card.
    BridgeError(String),
    /// Bridge returned the current Stable Balance + private mode
    /// state. Fired from the panel's `reload` task so the view can
    /// reflect whatever the SDK persisted across restarts.
    UserSettingsLoaded(coincube_spark_protocol::GetUserSettingsOk),
    /// The user flipped the Stable Balance toggle — fires a
    /// `set_stable_balance` RPC on the bridge.
    StableBalanceToggled(bool),
    /// `set_stable_balance` RPC finished. `Ok(enabled)` carries the
    /// new state so the view can update immediately without
    /// re-fetching; `Err` surfaces the SDK error.
    StableBalanceSaved(Result<bool, String>),
}

/// View-level messages for the Phase 4c Spark Send panel. Drives the
/// state machine in [`crate::app::state::spark::send::SparkSend`].
#[derive(Debug, Clone)]
pub enum SparkSendMessage {
    DestinationInputChanged(String),
    AmountInputChanged(String),
    /// Open the "THEY RECEIVE" picker modal (bitcoin rails + USDt/USDC).
    OpenReceivePicker,
    /// Dismiss the "THEY RECEIVE" picker without changing the selection.
    CloseReceivePicker,
    /// Pick what the recipient receives (a bitcoin rail or a stablecoin).
    SetReceiveTarget(crate::app::state::spark::send::SparkSendTarget),
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
    /// The Spark wallet balance for the YOU SEND card — BTC sats plus the Stable
    /// Balance snapshot, folded into a unified total by the handler. `None` when
    /// the `get_info` fetch failed; the card keeps its last value.
    BalanceLoaded(Option<(u64, Option<coincube_spark_protocol::StableBalanceSnapshot>)>),
    /// A `list_payments` RPC completed — used to populate the Last
    /// Transactions section under the Send form.
    PaymentsLoaded(Vec<coincube_spark_protocol::PaymentSummary>),
    /// A `list_payments` RPC failed — silently clears the list.
    PaymentsFailed(String),
    /// User tapped a row in Last Transactions.
    SelectTransaction(usize),
    /// User tapped "View All Transactions".
    History,

    // ── Cross-chain stablecoin send ─────────────────────────────────────
    /// The destination parsed as a cross-chain address and the bridge
    /// returned the routes that can reach it. Moves the panel into
    /// `CrossChainRoutes`, where the user confirms the chain and asset.
    CrossChainRoutesLoaded(coincube_spark_protocol::CrossChainRoutesOk),
    /// User picked one of the offered routes (index into the phase's list).
    CrossChainRouteSelected(usize),
    /// User edited the slippage field (basis points).
    SlippageChanged(String),
    /// User toggled the advanced disclosure that hides the slippage field.
    ToggleAdvanced,
    /// User accepted the chain/asset confirmation — fetch a quote for the
    /// selected route.
    CrossChainQuoteRequested,
    /// The countdown ticked. Recomputes the quote's remaining life and, at
    /// zero, blocks Confirm until the user re-quotes.
    QuoteTick,
    /// User asked for a fresh quote after the previous one expired.
    ReQuoteRequested,
    /// User asked to retry a *failed* cross-chain send.
    ///
    /// Distinct from [`Self::ConfirmRequested`]: a retry has to re-run the
    /// prepare (the bridge consumed the old handle when it executed the failed
    /// send), while deliberately keeping the original idempotency key so the
    /// retry can't double-pay. Only offered when the route's
    /// [`RetryPolicy`](crate::app::state::spark::cross_chain::RetryPolicy)
    /// permits it.
    CrossChainRetryRequested,
    /// A cross-chain send failed. Carries the route's retry policy, so the
    /// panel can tell "safe to retry" apart from "check the payment's state
    /// first, because a retry might pay twice".
    CrossChainSendFailed(String, crate::app::state::spark::cross_chain::RetryPolicy),
}

/// View-level messages for the Phase 4c Spark Receive panel.
#[derive(Debug, Clone)]
pub enum SparkReceiveMessage {
    /// Open the unified "THEY SEND" picker modal (Bitcoin rails + cross-network
    /// assets) that replaces the old Method chips + "Receive from another
    /// network" card.
    OpenSenderPicker,
    /// Dismiss the "THEY SEND" picker without changing the selection.
    CloseSenderPicker,
    /// Picked a Bitcoin rail (Lightning / on-chain / Spark) in the picker —
    /// sets the receive method and closes the modal.
    SelectSenderRail(crate::app::state::spark::receive::SparkReceiveMethod),
    /// Picked a cross-network asset (by `DepositOption` key) in the picker —
    /// launches the SideShift flow pre-selected to that asset. Mainnet only.
    SelectSenderCrossNetwork(String),
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
    /// An Esplora `/tx/<txid>/status` + tip query batch came back with
    /// per-deposit confirmation counts. Map key is `(txid, vout)`,
    /// value is `0` for mempool / unconfirmed and `>= 1` once mined.
    /// Used to render "X / 3 confirmations" on immature rows.
    DepositConfirmationsUpdated(std::collections::HashMap<(String, u32), u32>),
    /// 30s subscription tick while immature deposits are on screen —
    /// fires a fresh Esplora batch so the confirmation counts update
    /// between block arrivals without waiting for a `DepositsChanged`
    /// event (the SDK only re-emits at mature/refund-status boundaries,
    /// not on every new confirmation).
    RefreshConfirmations,
    Reset,
    /// The Spark wallet balance for the YOU RECEIVE card — BTC sats plus the
    /// Stable Balance snapshot, folded into a unified total by the handler.
    /// `None` when `get_info` failed; the card keeps its last value.
    BalanceLoaded(Option<(u64, Option<coincube_spark_protocol::StableBalanceSnapshot>)>),
    /// A `list_payments` RPC completed — used to populate the Last
    /// Transactions section under the Receive form.
    PaymentsLoaded(Vec<coincube_spark_protocol::PaymentSummary>),
    /// A `list_payments` RPC failed — silently clears the list.
    PaymentsFailed(String),
    /// User tapped a row in Last Transactions.
    SelectTransaction(usize),
    /// User tapped "View All Transactions".
    History,
    /// User tapped "Copy" under the generated payment request. Routed
    /// through the panel rather than the app-level `Message::Clipboard`
    /// so the handler can pair the clipboard write with a confirmation
    /// toast naming the rail that was copied — the same pattern Vault
    /// and Liquid Receive use.
    CopyPaymentRequest(String),
}
