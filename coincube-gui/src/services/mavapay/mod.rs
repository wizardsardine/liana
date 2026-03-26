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
    FulfillSellInvoice,
    SellInvoiceFulfilled(breez_sdk_liquid::model::Payment),
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
