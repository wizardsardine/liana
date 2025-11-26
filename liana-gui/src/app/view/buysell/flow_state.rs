use iced::Task;
use liana_ui::component::form;

use crate::app::view::{BuySellMessage, MavapayMessage};
use crate::services::{coincube::*, mavapay::*};

#[derive(Debug)]
pub enum MavapayFlowStep {
    Register {
        // TODO: change to normal strings
        first_name: form::Value<String>,
        last_name: form::Value<String>,
        password1: form::Value<String>,
        password2: form::Value<String>,
        email: form::Value<String>,
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
    ActiveBuysell {
        country: Country,
        flow_mode: MavapayFlowMode,
        amount: u64,
        source_currency: Option<MavapayUnitCurrency>,
        target_currency: Option<MavapayUnitCurrency>,
        settlement_currency: Option<MavapayCurrency>,
        payment_method: MavapayPaymentMethod,
        // TODO: replace with banks vector
        bank_account_number: String,
        bank_account_name: String,
        bank_code: String,
        bank_name: String,
        current_quote: Option<GetQuoteResponse>,
        current_price: Option<GetPriceResponse>,
    },
}

/// State specific to Mavapay flow
pub struct MavapayState {
    pub step: MavapayFlowStep,

    // mavapay session information
    pub current_user: Option<User>,
    pub auth_token: Option<String>,

    // API clients
    pub mavapay_client: MavapayClient,
    pub coincube_client: CoincubeClient,
}

impl MavapayState {
    // TODO: load api key from os keychain
    pub fn new() -> Self {
        Self {
            step: MavapayFlowStep::Login {
                email: String::new(),
                password: String::new(),
            },
            current_user: None,
            auth_token: None,
            mavapay_client: MavapayClient::new(),
            coincube_client: crate::services::coincube::CoincubeClient::new(),
        }
    }
}

impl MavapayState {
    pub fn get_price(&self, country_iso: Option<&str>) -> Task<BuySellMessage> {
        let client = self.mavapay_client.clone();
        let currency = match country_iso {
            Some("KE") => MavapayCurrency::KenyanShilling,
            Some("ZA") => MavapayCurrency::SouthAfricanRand,
            _ => MavapayCurrency::NigerianNaira,
        };

        Task::perform(
            async move { client.get_price(currency).await },
            |result| match result {
                Ok(price) => BuySellMessage::Mavapay(MavapayMessage::PriceReceived(price)),
                Err(e) => BuySellMessage::SessionError(e.to_string()),
            },
        )
    }

    pub fn open_payment_link(&self) -> Task<BuySellMessage> {
        let MavapayFlowStep::ActiveBuysell {
            settlement_currency,
            payment_method,
            amount,
            ..
        } = &self.step
        else {
            return Task::none();
        };

        let Some(settlement_currency) = settlement_currency else {
            return Task::none();
        };

        // Get payment method
        let request = CreatePaymentLinkRequest {
            name: format!("Coincube Vault - {}", settlement_currency),
            description: format!(
                "One-time payment of {} {}, made from the Coincube Vault Bitcoin Wallet",
                amount, settlement_currency
            ),
            _type: PaymentLinkType::OneTime,
            add_fee_to_total_cost: false,
            settlement_currency: settlement_currency.clone(),
            payment_methods: [payment_method.clone()],
            amount: *amount,
            callback_url: None, // TODO: Implement callback mechanism for desktop app
        };

        let client = self.mavapay_client.clone();
        Task::perform(
            async move { client.create_payment_link(request).await },
            |result| match result {
                Ok(created) => BuySellMessage::WebviewOpenUrl(created.payment_link),
                Err(e) => BuySellMessage::SessionError(format!("Payment link error: {}", e)),
            },
        )
    }

    pub fn create_quote(&self, country_iso: Option<&str>) -> Task<BuySellMessage> {
        let MavapayFlowStep::ActiveBuysell {
            amount,
            source_currency,
            target_currency,
            bank_account_number,
            bank_account_name,
            bank_code,
            bank_name,
            ..
        } = &self.step
        else {
            return Task::none();
        };

        let Some(source_currency) = source_currency else {
            return Task::none();
        };

        let Some(target_currency) = target_currency else {
            return Task::none();
        };

        let request = match source_currency {
            MavapayUnitCurrency::BitcoinSatoshi => GetQuoteRequest {
                amount: amount.clone(),
                source_currency: source_currency.clone(),
                target_currency: target_currency.clone(),
                payment_method: MavapayPaymentMethod::Lightning,
                payment_currency: source_currency.clone(),
                // automatically deposit fiat funds in beneficiary account
                autopayout: true,
                customer_internal_fee: Some(0),
                beneficiary: match country_iso {
                    Some("KE") => {
                        unimplemented!("Support for Kenyan beneficiaries is incomplete")
                    }
                    Some("ZA") => Some(crate::services::mavapay::Beneficiary::ZAR {
                        name: bank_account_name.clone(),
                        bank_name: bank_name.clone(),
                        bank_account_number: bank_account_number.clone(),
                    }),
                    Some("NG") => Some(crate::services::mavapay::Beneficiary::NGN {
                        bank_account_number: bank_account_number.clone(),
                        bank_account_name: bank_account_name.clone(),
                        bank_code: bank_code.clone(),
                        bank_name: bank_name.clone(),
                    }),
                    iso => unreachable!("Country ({:?}) is not supported by Mavapay", iso),
                },
            },
            fiat => {
                unimplemented!(
                    "Unable to create quote with fiat currency: {fiat}, currently unsupported",
                )
            }
        };

        // prepare request
        let client = self.mavapay_client.clone();
        let coincube_client = self.coincube_client.clone();

        // Get user details
        let user_id = self.current_user.as_ref().map(|user| user.id.to_string());
        let bank_account_number = bank_account_number.clone();
        let bank_account_name = bank_account_name.clone();
        let bank_code = bank_code.clone();
        let bank_name = bank_name.clone();

        Task::perform(
            async move {
                // Step 1: Create quote with Mavapay
                let quote = client.create_quote(request).await?;

                tracing::info!(
                    "[MAVAPAY] Quote created: {}, hash: {:?}",
                    quote.id,
                    quote.hash
                );

                // Step 2: Save quote to coincube-api
                let save_request = SaveQuoteRequest {
                    quote_id: quote.id.clone(),
                    hash: quote.hash.clone(),
                    user_id,
                    amount: quote.amount_in_source_currency,
                    source_currency: quote.source_currency.clone(),
                    target_currency: quote.target_currency.clone(),
                    exchange_rate: quote.exchange_rate,
                    usd_to_target_currency_rate: quote.usd_to_target_currency_rate,
                    transaction_fees_in_source_currency: quote.transaction_fees_in_source_currency,
                    transaction_fees_in_target_currency: quote.transaction_fees_in_target_currency,
                    amount_in_source_currency: quote.amount_in_source_currency,
                    amount_in_target_currency: quote.amount_in_target_currency,
                    total_amount_in_source_currency: quote.total_amount_in_source_currency,
                    total_amount_in_target_currency: quote.total_amount_in_target_currency.or(
                        Some(
                            quote.amount_in_target_currency
                                + quote.transaction_fees_in_target_currency,
                        ),
                    ),
                    bank_account_number: Some(bank_account_number),
                    bank_account_name: Some(bank_account_name),
                    bank_code: Some(bank_code),
                    bank_name: Some(bank_name),
                    payment_method: "bank_transfer".to_string(),
                };

                coincube_client
                    .save_quote(save_request)
                    .await
                    .map_err(|e| {
                        crate::services::mavapay::MavapayError::Http(
                            None,
                            format!("Failed to save quote: {}", e),
                        )
                    })?;

                tracing::info!("✅ [COINCUBE] Quote saved to database");

                // Step 3: Build quote display URL using quote_id
                let url = coincube_client.get_quote_display_url(&quote.id);

                Ok((quote, url))
            },
            |result: Result<
                (crate::services::mavapay::GetQuoteResponse, String),
                crate::services::mavapay::MavapayError,
            >| match result {
                Ok((_, url)) => BuySellMessage::WebviewOpenUrl(url),
                Err(e) => BuySellMessage::SessionError(e.to_string()),
            },
        )
    }
}

/// Mavapay flow mode selection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MavapayFlowMode {
    CreateQuote,
    OneTimePayment,
}

impl MavapayFlowMode {
    pub fn all() -> &'static [MavapayFlowMode] {
        &[
            MavapayFlowMode::CreateQuote,
            MavapayFlowMode::OneTimePayment,
        ]
    }
}

impl std::fmt::Display for MavapayFlowMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MavapayFlowMode::CreateQuote => write!(f, "Create Quote"),
            MavapayFlowMode::OneTimePayment => write!(f, "One-time Payment"),
        }
    }
}
