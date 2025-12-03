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
    ActiveBuysell {
        buy_or_sell: BuyOrSell,
        country: Country,
        banks: Option<MavapayBanks>,
        amount: u64, // Amount in BTCSAT
        beneficiary: Option<Beneficiary>,
        selected_bank: Option<usize>,
        current_quote: Option<GetQuoteResponse>,
        // TODO: Display BTC price on buysell UI
        current_price: Option<GetPriceResponse>,
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
        let MavapayFlowStep::ActiveBuysell {
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
            // Step 1: Create quote with Mavapay
            async move { client.create_quote(request).await },
            move |result: Result<GetQuoteResponse, MavapayError>| match result {
                Ok(quote) => {
                    // TODO: Save quote to coincube-api, requires rework to the coincube API (Step 2)

                    tracing::info!("[MAVAPAY] Quote created: {}", quote.id);

                    // Step 3: Build quote display URL using quote_id
                    BuySellMessage::WebviewOpenUrl(coincube_client.get_quote_display_url(&quote.id))
                }
                Err(e) => BuySellMessage::SessionError("Unable to create quote", e.to_string()),
            },
        )
    }
}
