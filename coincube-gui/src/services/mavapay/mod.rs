pub mod api;
pub mod client;
pub mod stream;

pub use api::*;
pub use client::MavapayClient;
pub use stream::TransactionUpdate;

#[derive(Debug, Clone)]
pub enum MavapayMessage {
    // TODO: integrate into `MavapayFlowStep` widgets
    NavigateBack,

    // general
    TransferSpeedChanged(OnchainTransferSpeed),
    FiatAmountChanged(f64),
    SatAmountChanged(f64),
    NormalizeAmounts,
    GetPrice,
    PriceReceived(GetPriceResponse),

    // sell input form
    GetBanks,
    BanksReceived(MavapayBanks),
    GetSparkBalance,
    ReceivedSparkBalance(u64),
    VerifyNgnBankDetails,
    VerifiedNgnBankDetails(NgnCustomerDetails),
    BeneficiaryFieldUpdate(&'static str, String),

    // used in both sell and buy modes
    CreateQuote,
    SendQuote(GetQuoteRequest),
    QuoteCreated(GetQuoteResponse),

    // buy input form
    GenerateLightningInvoice,
    WriteInvoiceToClipboard,
    LightningInvoiceReceived(String),

    // transactions history widget
    FetchTransactions,
    TransactionsReceived(Vec<OrderTransaction>),
    SelectTransaction(usize),
    OrderReceived(GetOrderResponse),

    // checkout
    SimulatePayIn,
    /// Pay the sell order's invoice from Spark.
    FulfillSellInvoice,
    // The answers below name the quote they belong to. A checkout drops any
    // answer for a different quote: a late answer for an earlier order must
    // never settle, or fabricate state on, the current one.
    /// Spark prepared the payment and it matches the quote; send it.
    SellInvoicePrepared {
        quote_id: String,
        prepare: coincube_spark_protocol::PrepareSendOk,
    },
    /// Preparing failed. Nothing was sent.
    SellPrepareFailed {
        quote_id: String,
        error: String,
    },
    SellInvoiceFulfilled {
        quote_id: String,
        payment: coincube_spark_protocol::SendPaymentOk,
    },
    /// The bridge answered the send with a rejection. Nothing was paid.
    SellSendFailed {
        quote_id: String,
        error: String,
    },
    /// The send was dispatched and nobody said what happened to it.
    SellInvoiceOutcomeUnknown {
        quote_id: String,
        request_id: u64,
        message: String,
    },
    /// Check again what happened to a send whose outcome is unknown.
    CheckSellPayment,
    SellPaymentChecked {
        quote_id: String,
        outcome: crate::app::state::spark::send::ReconcileOutcome,
    },
    /// Repeat a send whose check found no trace, under the same key.
    RetrySellPayment,
    QuoteFulfilled(GetOrderResponse),

    // SSE stream events
    TransactionUpdated(TransactionUpdate),
    StreamConnected,
    EventSourceDisconnected(String),
    StreamError(String),
}

impl From<MavapayMessage> for crate::app::view::Message {
    fn from(msg: MavapayMessage) -> Self {
        crate::app::view::Message::BuySell(crate::app::view::BuySellMessage::Mavapay(msg))
    }
}

/// Checks if a country ISO code is in the African region (Mavapay supported)
#[inline(always)]
pub fn mavapay_supported(iso_code: &str) -> bool {
    ["NG", "KE", "ZA"].contains(&iso_code)
}
