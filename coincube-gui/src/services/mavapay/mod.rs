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
    GetLiquidWalletBalance,
    ReceivedLiquidWalletBalance(u64),
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
        && matches!(
            std::env::var("ENABLE_MAVAPAY").as_deref(),
            Ok("1") | Ok("true")
        )
}
