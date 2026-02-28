pub mod api;
pub mod client;
pub mod stream;

pub use api::*;
pub use client::MavapayClient;
pub use stream::TransactionUpdate;

#[derive(Debug, Clone)]
pub enum MavapayMessage {
    // transactions
    FiatAmountChanged(f64),
    SatAmountChanged(f64),
    NormalizeAmounts,
    PaymentMethodChanged(MavapayPaymentMethod),
    BankAccountNumberChanged(String),
    BankAccountNameChanged(String),
    BankSelected(usize),
    TransferSpeedChanged(OnchainTransferSpeed),

    GetQuote,
    QuoteCreated(GetQuoteResponse),
    GetPrice,
    GetLightningInvoice,
    LightningInvoiceReceived(String),
    PriceReceived(GetPriceResponse),
    GetBanks,
    BanksReceived(MavapayBanks),

    FetchTransactions,
    TransactionsReceived(Vec<OrderTransaction>),
    SelectTransaction(usize),
    OrderReceived(GetOrderResponse),
    BackToHistory,
    // checkout
    SimulatePayIn,
    QuoteFulfilled(GetOrderResponse),
    // SSE stream events
    TransactionUpdated(TransactionUpdate),
    StreamConnected,
    EventSourceDisconnected(String),
    StreamError(String),
}

/// Checks if a country ISO code is in the African region (Mavapay supported)
#[inline(always)]
pub fn mavapay_supported(iso_code: &str) -> bool {
    ["NG", "KE", "ZA"].contains(&iso_code)
        && matches!(option_env!("ENABLE_MAVAPAY"), Some("1") | Some("true"))
}
