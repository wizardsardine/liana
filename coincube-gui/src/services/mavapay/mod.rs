pub mod api;
pub mod client;

pub use api::*;
pub use client::MavapayClient;

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
    // checkout
    SimulatePayIn,
    QuoteFulfilled(GetOrderResponse),
}


/// Checks if a country ISO code is in the African region (Mavapay supported)
#[inline(always)]
pub fn mavapay_supported(iso_code: &str) -> bool {
    ["NG", "KE", "ZA"].contains(&iso_code)
}
