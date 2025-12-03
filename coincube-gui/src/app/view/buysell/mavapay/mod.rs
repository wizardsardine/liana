pub mod ui;

use iced::Task;

use crate::app::view::buysell::panel::BuyOrSell;
use crate::app::view::{BuySellMessage, MavapayMessage};
use crate::services::{coincube::*, mavapay::*};

#[derive(Debug)]
pub enum MavapayFlowStep {
    Transaction {
        buy_or_sell: BuyOrSell,
        country: Country,
        beneficiary: Option<Beneficiary>,
        sat_amount: f64, // Unit Amount in BTCSAT
        banks: Option<MavapayBanks>,
        selected_bank: Option<usize>,
        transfer_speed: OnchainTransferSpeed,
        btc_price: Option<GetPriceResponse>,
    },
    Checkout {
        sat_amount: f64,
        buy_or_sell: BuyOrSell,
        beneficiary: Option<Beneficiary>,
        quote: GetQuoteResponse,
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
                sat_amount: 6000.0,
                beneficiary: None,
                transfer_speed: OnchainTransferSpeed::Fast,
                banks: None,
                selected_bank: None,
                btc_price: None,
            },
            client: MavapayClient::new(),
        }
    }
}

impl MavapayState {
    pub fn get_price(&self, country_code: &str) -> Task<BuySellMessage> {
        let client = self.client.clone();
        let currency = match country_code {
            "KE" => MavapayCurrency::KenyanShilling,
            "ZA" => MavapayCurrency::SouthAfricanRand,
            "NG" => MavapayCurrency::NigerianNaira,
            c => unreachable!("Country {:?} is not supported by Mavapay", c),
        };

        Task::perform(
            async move { client.get_price(currency).await },
            |result| match result {
                Ok(price) => BuySellMessage::Mavapay(MavapayMessage::PriceReceived(price)),
                Err(e) => BuySellMessage::SessionError(
                    "Unable to get latest Bitcoin price",
                    e.to_string(),
                ),
            },
        )
    }

    pub fn get_banks(&self, country_code: &str) -> Task<BuySellMessage> {
        let client = self.client.clone();
        let country_code = country_code.to_string();

        Task::perform(
            async move { client.get_banks(&country_code).await },
            |result| match result {
                Ok(banks) => BuySellMessage::Mavapay(MavapayMessage::BanksReceived(banks)),
                Err(e) => BuySellMessage::SessionError(
                    "Unable to fetch supported banks for your country",
                    e.to_string(),
                ),
            },
        )
    }

    pub fn create_quote(&self, coincube_client: CoincubeClient) -> Task<BuySellMessage> {
        let MavapayFlowStep::Transaction {
            country,
            sat_amount: amount,
            beneficiary,
            buy_or_sell,
            transfer_speed,
            ..
        } = &self.step
        else {
            return Task::none();
        };

        let local_currency = match country.code {
            "KE" => MavapayUnitCurrency::KenyanShillingCent,
            "NG" => MavapayUnitCurrency::NigerianNairaKobo,
            "ZA" => MavapayUnitCurrency::SouthAfricanRandCent,
            iso => unreachable!("Country ({}) is unsupported by Mavapay", iso),
        };

        let request = match buy_or_sell {
            super::panel::BuyOrSell::Sell => GetQuoteRequest {
                amount: amount.clone().round() as _,
                source_currency: MavapayUnitCurrency::BitcoinSatoshi,
                target_currency: local_currency,
                // TODO: Mavapay only supports lightning transactions for selling BTC, meaning we are currently blocked by the breeze integration
                payment_method: MavapayPaymentMethod::Lightning,
                payment_currency: MavapayUnitCurrency::BitcoinSatoshi,
                // automatically deposit fiat funds in beneficiary account
                speed: transfer_speed.clone(),
                autopayout: true,
                customer_internal_fee: Some(0),
                beneficiary: beneficiary.clone(),
            },
            super::panel::BuyOrSell::Buy { address } => GetQuoteRequest {
                amount: amount.clone().round() as _,
                source_currency: local_currency,
                target_currency: MavapayUnitCurrency::BitcoinSatoshi,
                // TODO: Currently, Kenyan beneficiaries are not supported by Mavapay, as only BankTransfer is currently supported by `onchain` buy
                payment_method: MavapayPaymentMethod::BankTransfer,
                payment_currency: MavapayUnitCurrency::BitcoinSatoshi,
                speed: transfer_speed.clone(),
                autopayout: true,
                customer_internal_fee: None,
                beneficiary: Some(Beneficiary::Onchain {
                    on_chain_address: address.address.to_string(),
                }),
            },
        };

        // prepare request
        let client = self.client.clone();

        Task::perform(
            async move {
                // Step 1: Create quote with Mavapay
                let quote = client.create_quote(request).await?;

                // Step 2: Save quote to coincube-api
                match coincube_client.save_quote(&quote.id, &quote).await {
                    Ok(save) => log::info!("[COINCUBE] Successfully saved quote: {:?}", save),
                    Err(err) => log::error!("[COINCUBE] Unable to save quote: {:?}", err),
                };

                Ok(quote)
            },
            move |result: Result<GetQuoteResponse, MavapayError>| match result {
                Ok(quote) => BuySellMessage::Mavapay(MavapayMessage::QuoteCreated(quote)),
                Err(e) => BuySellMessage::SessionError("Unable to create quote", e.to_string()),
            },
        )
    }
}
