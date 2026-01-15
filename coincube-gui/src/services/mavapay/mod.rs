pub mod api;
pub mod client;
pub mod stream;

pub use api::*;
pub use client::MavapayClient;
pub use stream::{
    transaction_stream, TransactionStreamConfig, TransactionStreamEvent, TransactionUpdate,
};

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
    CreateQuote,
    QuoteCreated(GetQuoteResponse),
    GetPrice,
    PriceReceived(GetPriceResponse),
    GetBanks,
    BanksReceived(MavapayBanks),
    FetchTransactions,
    TransactionsReceived(Vec<OrderTransaction>),
    TransactionsFetchFailed(String),
    SelectTransaction(OrderTransaction),
    OrderReceived(GetOrderResponse),
    OrderFetchFailed(String),
    BackToHistory,
    // checkout
    SimulatePayIn,
    QuoteFulfilled(GetOrderResponse),
    // SSE stream events
    StreamEvent(TransactionStreamEvent),
}

/// Checks if a country ISO code is in the African region (Mavapay supported)
#[inline(always)]
pub fn mavapay_supported(iso_code: &str) -> bool {
    ["NG", "KE", "ZA"].contains(&iso_code)
}
