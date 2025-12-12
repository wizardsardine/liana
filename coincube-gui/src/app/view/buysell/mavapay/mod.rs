pub mod ui;

use crate::app::view::buysell::panel::BuyOrSell;
use crate::services::{coincube::*, mavapay::*};

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
        abort: iced::task::Handle,
    },
}

pub struct MavapayState {
    pub step: MavapayFlowStep,
    pub client: MavapayClient,
}

impl MavapayState {
    pub fn new(buy_or_sell: BuyOrSell, country: Country) -> Self {
        Self {
            step: MavapayFlowStep::Transaction {
                buy_or_sell,
                country,
                sat_amount: 6000,
                beneficiary: None,
                transfer_speed: OnchainTransferSpeed::Fast,
                banks: None,
                selected_bank: None,
                btc_price: None,
                sending_quote: false,
            },
            client: MavapayClient::new(),
        }
    }
}
