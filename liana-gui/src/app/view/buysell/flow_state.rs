use iced::Task;

use crate::app::view::buysell::panel::BuyOrSell;
use crate::app::view::{BuySellMessage, MavapayMessage};
use crate::services::{coincube::*, mavapay::*};

#[derive(Debug)]
pub enum MavapayFlowStep {
    Register {
        legal_name: String,
        password1: String,
        password2: String,
        email: String,
    },
    VerifyEmail {
        email: String,
        password: String,
        checking: bool,
    },
    Login {
        email: String,
        password: String,
    },
    PasswordReset {
        email: String,
        sent: bool,
    },
    Transaction {
        buy_or_sell: BuyOrSell,
        country: Country,
        beneficiary: Option<Beneficiary>,
        amount: u64, // Amount in BTCSAT
        banks: Option<MavapayBanks>,
        selected_bank: Option<usize>,
        current_price: Option<GetPriceResponse>,

        // TODO: Should be displayed on a custom `Checkout` UI
        current_quote: Option<GetQuoteResponse>,
    },
}

pub struct MavapayState {
    pub step: MavapayFlowStep,
    pub mavapay_client: MavapayClient,

    // mavapay session information
    pub current_user: Option<User>,
    pub auth_token: Option<String>,
}

impl MavapayState {
    pub fn new() -> Self {
        Self {
            step: MavapayFlowStep::Login {
                email: String::new(),
                password: String::new(),
            },
            mavapay_client: MavapayClient::new(),

            current_user: None,
            auth_token: None,
        }
    }
}

impl MavapayState {
    pub fn get_price(&self, country_iso: Option<&str>) -> Task<BuySellMessage> {
        let client = self.mavapay_client.clone();
        let currency = match country_iso {
            Some("KE") => MavapayCurrency::KenyanShilling,
            Some("ZA") => MavapayCurrency::SouthAfricanRand,
            Some("NG") => MavapayCurrency::NigerianNaira,
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

    pub fn create_quote(&self, coincube_client: CoincubeClient) -> Task<BuySellMessage> {
        let MavapayFlowStep::Transaction {
            country,
            amount,
            beneficiary,
            buy_or_sell,
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
                amount: amount.clone(),
                source_currency: MavapayUnitCurrency::BitcoinSatoshi,
                target_currency: local_currency,
                // TODO: Mavapay only supports lightning transactions for selling BTC, meaning we are blocked by the breeze integration
                payment_method: MavapayPaymentMethod::Lightning,
                payment_currency: MavapayUnitCurrency::BitcoinSatoshi,
                // automatically deposit fiat funds in beneficiary account
                autopayout: true,
                customer_internal_fee: Some(0),
                beneficiary: beneficiary.clone(),
            },
            super::panel::BuyOrSell::Buy { address } => GetQuoteRequest {
                amount: amount.clone(),
                source_currency: local_currency,
                target_currency: MavapayUnitCurrency::BitcoinSatoshi,
                payment_method: MavapayPaymentMethod::BankTransfer,
                payment_currency: MavapayUnitCurrency::BitcoinSatoshi,
                autopayout: true,
                customer_internal_fee: None,
                // TODO: Currently, Kenyan beneficiaries are not supported by Mavapay, as only BankTransfer is currently supported by `onchain` buy
                beneficiary: Some(Beneficiary::Onchain {
                    on_chain_address: address.address.to_string(),
                }),
            },
        };

        // prepare request
        let client = self.mavapay_client.clone();

        Task::perform(
            async move {
                // Step 1: Create quote with Mavapay
                let quote = client.create_quote(request).await?;

                // Step 2: Save quote to coincube-api
                match coincube_client.save_quote(&quote.id, &quote).await {
                    Ok(save) => log::info!("[COINCUBE] Successfully saved quote: {:?}", save),
                    Err(err) => log::error!("[COINCUBE] Unable to saved quote: {:?}", err),
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
