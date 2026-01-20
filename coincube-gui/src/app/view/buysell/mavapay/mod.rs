pub mod ui;

use crate::app::view::buysell::panel::BuyOrSell;
use crate::services::{coincube::*, mavapay::api::*};

#[derive(Debug)]
pub enum MavapayFlowStep {
    Transaction {
        buy_or_sell: BuyOrSell,
        country: Country,
        beneficiary: Option<Beneficiary>,
        sat_amount: u64, // Unit Amount in BTCSAT
        banks: Option<MavapayBanks>,
        selected_bank: Option<usize>,
        transfer_speed: OnchainTransferSpeed,
        btc_price: Option<GetPriceResponse>,
        sending_quote: bool,
    },
    Checkout {
        sat_amount: u64,
        buy_or_sell: BuyOrSell,
        beneficiary: Option<Beneficiary>,
        quote: GetQuoteResponse,
        fulfilled_order: Option<GetOrderResponse>,
        country: Country,
        /// Order ID for SSE transaction status updates
        stream_order_id: Option<String>,
    },
    History {
        transactions: Option<Vec<OrderTransaction>>,
        loading: bool,
        error: Option<String>,
    },
    OrderDetail {
        transaction: OrderTransaction,
        order: Option<GetOrderResponse>,
        loading: bool,
    },
}
