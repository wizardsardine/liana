use std::convert::TryInto;
use std::sync::Arc;

use breez_sdk_liquid::model::PaymentDetails;
use breez_sdk_liquid::prelude::Payment;
use breez_sdk_liquid::InputType;
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{component::form, widget::*};
use iced::Task;

use crate::app::menu::{ActiveSubMenu, Menu};
use crate::app::settings::unit::BitcoinDisplayUnit;
use crate::app::state::{redirect, State};
use crate::app::view::SendPopupMessage;
use crate::app::{breez::BreezClient, cache::Cache};
use crate::app::{message::Message, view, wallet::Wallet};
use crate::daemon::Daemon;
use crate::utils::format_time_ago;

#[derive(Debug)]
pub enum Modal {
    AmountInput,
    FiatInput {
        fiat_input: form::Value<String>,
        currencies: [crate::services::fiat::Currency; 4],
        selected_currency: crate::services::fiat::Currency,
        converters:
            std::collections::HashMap<crate::services::fiat::Currency, view::FiatAmountConverter>,
    },
    None,
}

#[derive(Debug)]
pub enum ActiveSendFlowState {
    Main { modal: Modal },
    FinalCheck,
    Sent,
}

/// ActiveSend manages the Lightning Network send interface
pub struct ActiveSend {
    breez_client: Arc<BreezClient>,
    btc_balance: Amount,
    amount: Amount,
    amount_input: form::Value<String>,
    recent_transaction: Vec<view::active::RecentTransaction>,
    recent_payments: Vec<Payment>,
    selected_payment: Option<Payment>,
    input: form::Value<String>,
    input_type: Option<InputType>,
    lightning_limits: Option<(u64, u64)>, // (min_sats, max_sats)
    onchain_limits: Option<(u64, u64)>,   // (min_sats, max_sats)
    flow_state: ActiveSendFlowState,
    description: Option<String>,
    comment: Option<String>,
    error: Option<String>,
    prepare_response: Option<breez_sdk_liquid::prelude::PrepareSendResponse>,
    prepare_onchain_response: Option<breez_sdk_liquid::prelude::PreparePayOnchainResponse>,
    is_sending: bool,
}

impl ActiveSend {
    pub fn new(breez_client: Arc<BreezClient>) -> Self {
        Self {
            breez_client,
            btc_balance: Amount::from_sat(0),
            amount: Amount::from_sat(0),
            amount_input: form::Value::default(),
            recent_transaction: Vec::new(),
            recent_payments: Vec::new(),
            selected_payment: None,
            input: form::Value::default(),
            error: None,
            flow_state: ActiveSendFlowState::Main { modal: Modal::None },
            input_type: None,
            lightning_limits: None,
            onchain_limits: None,
            comment: None,
            description: None,
            prepare_response: None,
            prepare_onchain_response: None,
            is_sending: false,
        }
    }

    fn load_balance(&self) -> Task<Message> {
        let breez_client = self.breez_client.clone();

        Task::perform(
            async move {
                let info = breez_client.info().await;
                let payments = breez_client.list_payments(Some(2)).await;

                let balance = info
                    .as_ref()
                    .map(|info| {
                        let balance =
                            info.wallet_info.balance_sat + info.wallet_info.pending_receive_sat;
                        Amount::from_sat(balance)
                    })
                    .unwrap_or(Amount::ZERO);

                let error = match (&info, &payments) {
                    (Err(e1), Err(e2)) => Some(view::ActiveSendError::BalanceAndTransactionsFetch(
                        e1.to_string(),
                        e2.to_string(),
                    )),
                    (Err(e), _) => Some(view::ActiveSendError::BalanceFetch(e.to_string())),
                    (_, Err(e)) => Some(view::ActiveSendError::TransactionsFetch(e.to_string())),
                    _ => None,
                };

                let payments = payments.unwrap_or_default();

                (balance, payments, error)
            },
            |(balance, recent_payment, error)| {
                if let Some(err) = error {
                    Message::View(view::Message::ActiveSend(view::ActiveSendMessage::Error(
                        err,
                    )))
                } else {
                    Message::View(view::Message::ActiveSend(
                        view::ActiveSendMessage::DataLoaded {
                            balance,
                            recent_payment,
                        },
                    ))
                }
            },
        )
    }
}

impl State for ActiveSend {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let fiat_converter = cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());

        if let Some(payment) = &self.selected_payment {
            view::dashboard(
                menu,
                cache,
                None,
                view::active::transaction_detail_view(
                    payment,
                    fiat_converter,
                    cache.bitcoin_unit.into(),
                ),
            )
        } else {
            let comment = self.comment.clone().unwrap_or("".to_string());

            view::active_send_with_flow(view::ActiveSendFlowConfig {
                flow_state: &self.flow_state,
                btc_balance: self.btc_balance,
                fiat_converter,
                recent_transaction: &self.recent_transaction,
                input: &self.input,
                error: None, // Errors now shown via global toast
                amount_input: &self.amount_input,
                comment,
                description: self.description.as_deref(),
                lightning_limits: self.lightning_limits,
                amount: self.amount,
                prepare_response: self.prepare_response.as_ref(),
                is_sending: self.is_sending,
                menu,
                cache,
                input_type: &self.input_type,
                onchain_limits: self.onchain_limits,
                bitcoin_unit: cache.bitcoin_unit.into(),
            })
        }
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        if let Message::View(view::Message::ActiveSend(ref msg)) = message {
            match msg {
                view::ActiveSendMessage::InputEdited(value) => {
                    self.input.value = value.clone();
                    self.error = None;
                    let breez = self.breez_client.clone();
                    let breez_clone = self.breez_client.clone();
                    let breez_client = self.breez_client.clone();
                    let value_owned = value.clone();
                    // TODO: Add some kind of debouncing mechanism here, so that we don't call breez
                    // API again and again
                    let validate_input = Task::perform(
                        async move { breez.validate_input(value_owned).await },
                        |input| {
                            Message::View(view::Message::ActiveSend(
                                view::ActiveSendMessage::InputValidated(input),
                            ))
                        },
                    );

                    // Fetch limits only if not already available
                    if self.lightning_limits.is_none() || self.onchain_limits.is_none() {
                        let fetch_lightning_limits = Task::perform(
                            async move { breez_clone.fetch_lightning_limits().await },
                            |limits| {
                                if let Ok(limits) = limits {
                                    Message::View(view::Message::ActiveSend(
                                        view::ActiveSendMessage::LightningLimitsFetched {
                                            min_sat: limits.send.min_sat,
                                            max_sat: limits.send.max_sat,
                                        },
                                    ))
                                } else {
                                    Message::View(view::Message::ActiveSend(
                                        view::ActiveSendMessage::Error(
                                            view::ActiveSendError::LightningLimitsFetch("Unknown error".to_string())
                                        ),
                                    ))
                                }
                            },
                        );

                        let fetch_onchain_limits = Task::perform(
                            async move { breez_client.fetch_onchain_limits().await },
                            |limits| {
                                if let Ok(limits) = limits {
                                    Message::View(view::Message::ActiveSend(
                                        view::ActiveSendMessage::OnChainLimitsFetched {
                                            min_sat: limits.send.min_sat,
                                            max_sat: limits.send.max_sat,
                                        },
                                    ))
                                } else {
                                    Message::View(view::Message::ActiveSend(
                                        view::ActiveSendMessage::Error(
                                            view::ActiveSendError::OnChainLimitsFetch("Unknown error".to_string())
                                        ),
                                    ))
                                }
                            },
                        );
                        return Task::batch(vec![
                            validate_input,
                            fetch_lightning_limits,
                            fetch_onchain_limits,
                        ]);
                    }
                    return validate_input;
                }
                view::ActiveSendMessage::Send => {
                    let description = if let Some(input_type) = &self.input_type {
                        match input_type {
                            InputType::BitcoinAddress { address } => {
                                format!(
                                    "Sending money to {}",
                                    display_abbreviated(address.address.clone())
                                )
                            }
                            InputType::Bolt11 { invoice } => {
                                if let Some(amt) = invoice.amount_msat {
                                    if let Ok(amount) = Amount::from_str_in(
                                        &amt.to_string(),
                                        breez_sdk_liquid::bitcoin::Denomination::MilliSatoshi,
                                    ) {
                                        self.amount = amount;
                                        self.amount_input.valid = true;
                                        self.amount_input.value = if matches!(
                                            cache.bitcoin_unit,
                                            BitcoinDisplayUnit::BTC
                                        ) {
                                            amount.to_btc().to_string()
                                        } else {
                                            amount.to_sat().to_string()
                                        };
                                    }
                                }
                                if let Some(description) =
                                    invoice.description.as_deref().filter(|d| !d.is_empty())
                                {
                                    description.to_string()
                                } else {
                                    format!(
                                        "Sending money to {}",
                                        display_abbreviated(invoice.bolt11.clone())
                                    )
                                }
                            }
                            InputType::Bolt12Offer {
                                offer,
                                bip353_address,
                            } => {
                                let min_amount = offer.min_amount.clone().unwrap_or(
                                    breez_sdk_liquid::Amount::Bitcoin { amount_msat: 0 },
                                );

                                if let Some((min_limits, max_limits)) = self.lightning_limits {
                                    if let breez_sdk_liquid::Amount::Bitcoin { amount_msat } =
                                        min_amount
                                    {
                                        // convert from millisat to sat
                                        let amount_sat = amount_msat / 1000;
                                        self.lightning_limits = Some((
                                            std::cmp::max(min_limits, amount_sat),
                                            max_limits,
                                        ));
                                    }
                                }

                                if let Some(bip353_address) = bip353_address {
                                    format!("Sending money to {}", bip353_address.clone())
                                } else if let Some(description) = offer.description.clone() {
                                    description
                                } else {
                                    format!(
                                        "Sending money to {}",
                                        display_abbreviated(offer.offer.clone())
                                    )
                                }
                            }

                            InputType::LiquidAddress { address } => address.address.clone(),
                            _ => String::from("Send Payment"),
                        }
                    } else {
                        String::from("")
                    };

                    self.description = if description.is_empty() {
                        None
                    } else {
                        Some(description)
                    };
                    self.flow_state = ActiveSendFlowState::Main {
                        modal: Modal::AmountInput,
                    };
                }
                view::ActiveSendMessage::History => {
                    return redirect(Menu::Active(ActiveSubMenu::Transactions(None)));
                }
                view::ActiveSendMessage::SelectTransaction(idx) => {
                    if let Some(payment) = self.recent_payments.get(*idx).cloned() {
                        self.selected_payment = Some(payment.clone());
                        return Task::batch(vec![
                            redirect(Menu::Active(ActiveSubMenu::Transactions(None))),
                            Task::done(Message::View(view::Message::PreselectPayment(payment))),
                        ]);
                    }
                }
                view::ActiveSendMessage::DataLoaded {
                    balance,
                    recent_payment,
                } => {
                    self.btc_balance = *balance;
                    self.recent_payments = recent_payment.clone();

                    if !recent_payment.is_empty() {
                        let fiat_converter: Option<view::FiatAmountConverter> =
                            cache.fiat_price.as_ref().and_then(|p| p.try_into().ok());
                        let txns = recent_payment
                            .into_iter()
                            .map(|payment| {
                                let amount = Amount::from_sat(payment.amount_sat);
                                let status = payment.status;
                                let time_ago = format_time_ago(payment.timestamp.into());
                                let fiat_amount = fiat_converter
                                    .as_ref()
                                    .map(|c: &view::FiatAmountConverter| c.convert(amount));

                                let desc = match &payment.details {
                                    PaymentDetails::Lightning {
                                        payer_note,
                                        description,
                                        ..
                                    } => payer_note
                                        .as_ref()
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or(description),
                                    PaymentDetails::Liquid {
                                        payer_note,
                                        description,
                                        ..
                                    } => payer_note
                                        .as_ref()
                                        .filter(|s| !s.is_empty())
                                        .unwrap_or(description),

                                    PaymentDetails::Bitcoin { description, .. } => description,
                                };

                                let is_incoming = matches!(
                                    payment.payment_type,
                                    breez_sdk_liquid::prelude::PaymentType::Receive
                                );

                                let fees_sat = Amount::from_sat(payment.fees_sat);

                                let details = payment.details.clone();
                                let sign = if is_incoming { "+" } else { "-" };
                                view::active::RecentTransaction {
                                    description: desc.to_owned(),
                                    time_ago,
                                    amount,
                                    fiat_amount,
                                    is_incoming,
                                    sign,
                                    status,
                                    details,
                                    fees_sat,
                                }
                            })
                            .collect();
                        self.recent_transaction = txns;
                    } else {
                        self.recent_transaction = Vec::new();
                    }
                }
                view::ActiveSendMessage::Error(err) => {
                    self.error = Some(err.to_string());
                    self.is_sending = false; // Reset sending flag on error
                    // Wire to global toast
                    return Task::done(Message::View(view::Message::ShowError(err.to_string())));
                }
                view::ActiveSendMessage::ClearError => {
                    self.error = None;
                }
                view::ActiveSendMessage::InputValidated(input_type) => {
                    self.input.valid = input_type.is_some();
                    self.input_type = input_type.clone();
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::AmountEdited(v)) => {
                    if let ActiveSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &mut self.flow_state
                    {
                        self.amount_input.value = v.clone();

                        if v.is_empty() {
                            self.amount_input.valid = true;
                            self.amount_input.warning = None;
                            self.amount = Amount::from_sat(0);
                        } else if let Ok(amount) = Amount::from_str_in(
                            &v,
                            if matches!(cache.bitcoin_unit, BitcoinDisplayUnit::BTC) {
                                coincube_core::miniscript::bitcoin::Denomination::Bitcoin
                            } else {
                                coincube_core::miniscript::bitcoin::Denomination::Satoshi
                            },
                        ) {
                            self.amount = amount;
                            let amount_sats = amount.to_sat();

                            // Check balance first
                            if amount > self.btc_balance {
                                self.amount_input.valid = false;
                                self.amount_input.warning = Some("Insufficient balance");
                            }
                            // Check limits if available
                            else if let Some((min_sat, max_sat)) = self.lightning_limits {
                                if amount_sats < min_sat {
                                    self.amount_input.valid = false;
                                    self.amount_input.warning = Some("Below minimum limit");
                                } else if amount_sats > max_sat {
                                    self.amount_input.valid = false;
                                    self.amount_input.warning = Some("Exceeds maximum limit");
                                } else {
                                    self.amount_input.valid = true;
                                    self.amount_input.warning = None;
                                }
                            } else {
                                self.amount_input.valid = true;
                                self.amount_input.warning = None;
                            }
                        } else {
                            self.amount_input.valid = false;
                            self.amount_input.warning = Some("Invalid amount format");
                        }
                    }
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::CommentEdited(comment)) => {
                    if let ActiveSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &mut self.flow_state
                    {
                        self.comment = Some(comment.clone());
                    }
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::FiatConvert) => {
                    if let ActiveSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        // Determine default currencies
                        use crate::services::fiat::Currency;
                        let fiat_currency = cache
                            .fiat_price
                            .as_ref()
                            .and_then(|p| TryInto::<view::FiatAmountConverter>::try_into(p).ok())
                            .map(|c| c.currency())
                            .unwrap_or(Currency::USD);

                        let currencies = if fiat_currency == Currency::USD
                            || fiat_currency == Currency::EUR
                            || fiat_currency == Currency::GBP
                            || fiat_currency == Currency::JPY
                        {
                            [Currency::USD, Currency::EUR, Currency::GBP, Currency::JPY]
                        } else {
                            [fiat_currency, Currency::USD, Currency::EUR, Currency::GBP]
                        };

                        // Transition to Fiat Input with empty converters initially
                        self.flow_state = ActiveSendFlowState::Main {
                            modal: Modal::FiatInput {
                                fiat_input: form::Value::default(),
                                currencies,
                                selected_currency: fiat_currency,
                                converters: std::collections::HashMap::new(),
                            },
                        };

                        let price_source = cache
                            .fiat_price
                            .as_ref()
                            .map(|p| p.source())
                            .unwrap_or(crate::services::fiat::PriceSource::CoinGecko);

                        return Task::perform(
                            async move {
                                use crate::app::cache::FiatPriceRequest;

                                let mut tasks = vec![];
                                for currency in currencies.iter() {
                                    let request = FiatPriceRequest::new(price_source, *currency);
                                    tasks.push(async move {
                                        let price = request.send_default().await;
                                        (*currency, price)
                                    });
                                }

                                let mut converters = std::collections::HashMap::new();

                                for task in tasks {
                                    let (currency, price) = task.await;
                                    if let Ok(converter) =
                                        TryInto::<view::FiatAmountConverter>::try_into(&price)
                                    {
                                        converters.insert(currency, converter);
                                    }
                                }

                                converters
                            },
                            |converters| {
                                Message::View(view::Message::ActiveSend(
                                    view::ActiveSendMessage::PopupMessage(
                                        SendPopupMessage::FiatPricesLoaded(converters),
                                    ),
                                ))
                            },
                        );
                    }
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::FiatInputEdited(
                    fiat_input,
                )) => {
                    if let ActiveSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                fiat_input: current_input,
                                selected_currency,
                                converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        current_input.value = fiat_input.clone();
                        current_input.warning = None;

                        // Validate numeric format
                        if fiat_input.is_empty() {
                            current_input.valid = true;
                        } else if fiat_input.parse::<f64>().is_ok() {
                            // Check if converted BTC amount exceeds limits
                            if let Some(converter) = converters.get(selected_currency) {
                                if let Ok(fiat_amount) = view::vault::fiat::FiatAmount::from_str_in(
                                    &fiat_input,
                                    *selected_currency,
                                ) {
                                    if let Ok(btc_amount) = converter.convert_to_btc(&fiat_amount) {
                                        let amount_sats = btc_amount.to_sat();

                                        // Validate against balance and limits
                                        if btc_amount > self.btc_balance {
                                            current_input.valid = false;
                                            current_input.warning = Some("Insufficient balance");
                                        } else if let Some((min_sat, max_sat)) =
                                            self.lightning_limits
                                        {
                                            if amount_sats < min_sat {
                                                current_input.valid = false;
                                                current_input.warning = Some("Below minimum limit");
                                            } else if amount_sats > max_sat {
                                                current_input.valid = false;
                                                current_input.warning =
                                                    Some("Exceeds maximum limit");
                                            } else {
                                                current_input.valid = true;
                                            }
                                        } else {
                                            current_input.valid = true;
                                        }
                                    } else {
                                        // Conversion to BTC failed
                                        current_input.valid = false;
                                        current_input.warning = Some("Unable to convert to BTC");
                                    }
                                } else {
                                    // Invalid fiat amount format
                                    current_input.valid = false;
                                    current_input.warning = Some("Invalid fiat amount");
                                }
                            } else {
                                // Converter not available
                                current_input.valid = false;
                                current_input.warning = Some("Exchange rate unavailable");
                            }
                        } else {
                            current_input.valid = false;
                            current_input.warning = Some("Invalid number format");
                        }
                    }
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::FiatCurrencySelected(
                    currency,
                )) => {
                    if let ActiveSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                selected_currency, ..
                            },
                    } = &mut self.flow_state
                    {
                        *selected_currency = *currency;
                    }
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::FiatPricesLoaded(
                    converters,
                )) => {
                    if let ActiveSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                converters: modal_converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        *modal_converters = converters.clone();
                    }
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::FiatDone) => {
                    if let ActiveSendFlowState::Main {
                        modal:
                            Modal::FiatInput {
                                fiat_input,
                                selected_currency,
                                converters,
                                ..
                            },
                    } = &mut self.flow_state
                    {
                        if let Ok(_fiat_val) = fiat_input.value.parse::<f64>() {
                            // Check if converter is available
                            if let Some(converter) = converters.get(selected_currency) {
                                // Convert fiat to BTC using the converter for selected currency
                                if let Ok(fiat_amount) = view::vault::fiat::FiatAmount::from_str_in(
                                    &fiat_input.value,
                                    *selected_currency,
                                ) {
                                    if let Ok(btc_amount) = converter.convert_to_btc(&fiat_amount) {
                                        self.amount = btc_amount;
                                        let btc_str = if matches!(
                                            cache.bitcoin_unit,
                                            BitcoinDisplayUnit::BTC
                                        ) {
                                            btc_amount.to_btc().to_string()
                                        } else {
                                            btc_amount.to_sat().to_string()
                                        };
                                        let amount_sats = btc_amount.to_sat();

                                        // Validate the converted BTC amount
                                        let (valid, warning) = if btc_amount > self.btc_balance {
                                            (false, Some("Amount exceeds available balance"))
                                        } else {
                                            let limits = if matches!(
                                                self.input_type,
                                                Some(InputType::BitcoinAddress { .. })
                                            ) {
                                                self.onchain_limits
                                            } else {
                                                self.lightning_limits
                                            };

                                            if let Some((min_sat, max_sat)) = limits {
                                                if amount_sats < min_sat {
                                                    (false, Some("Amount is below minimum limit"))
                                                } else if amount_sats > max_sat {
                                                    (false, Some("Amount exceeds maximum limit"))
                                                } else {
                                                    (true, None)
                                                }
                                            } else {
                                                (true, None)
                                            }
                                        };

                                        self.amount_input = form::Value {
                                            value: btc_str,
                                            valid,
                                            warning,
                                        };

                                        // Only close modal on successful conversion
                                        self.flow_state = ActiveSendFlowState::Main {
                                            modal: Modal::AmountInput,
                                        };
                                    } else {
                                        // Conversion to BTC failed - stay in fiat modal with error
                                        fiat_input.valid = false;
                                        fiat_input.warning = Some("Unable to convert to BTC");
                                    }
                                } else {
                                    // Invalid fiat amount - stay in fiat modal with error
                                    fiat_input.valid = false;
                                    fiat_input.warning = Some("Invalid fiat amount");
                                }
                            } else {
                                // Converter not available - stay in fiat modal with error
                                fiat_input.valid = false;
                                fiat_input.warning = Some("Exchange rate unavailable");
                            }
                        }
                    }
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::Done) => {
                    if let ActiveSendFlowState::Main {
                        modal: Modal::AmountInput,
                    } = &self.flow_state
                    {
                        if let Some(input_type) = &self.input_type {
                            let destination = match input_type {
                                InputType::Bolt11 { invoice } => invoice.bolt11.clone(),
                                InputType::Bolt12Offer { offer, .. } => offer.offer.clone(),
                                InputType::BitcoinAddress { address } => address.address.clone(),
                                InputType::LiquidAddress { address } => address.address.clone(),
                                _ => {
                                    self.error = Some("Unsupported payment type".to_string());
                                    return Task::none();
                                }
                            };

                            let breez_client = self.breez_client.clone();
                            let breez_clone = self.breez_client.clone();
                            let amount_sat = self.amount.to_sat();

                            let lightning_send = Task::perform(
                                async move {
                                    breez_client
                                        .prepare_send_payment(
                                            &breez_sdk_liquid::prelude::PrepareSendRequest {
                                                destination,
                                                amount: Some(
                                                    breez_sdk_liquid::prelude::PayAmount::Bitcoin {
                                                        receiver_amount_sat: amount_sat,
                                                    },
                                                ),
                                            },
                                        )
                                        .await
                                },
                                |result| match result {
                                    Ok(prepare_response) => {
                                        Message::View(view::Message::ActiveSend(
                                            view::ActiveSendMessage::PrepareResponseReceived(
                                                prepare_response,
                                            ),
                                        ))
                                    }
                                    Err(e) => Message::View(view::Message::ActiveSend(
                                        view::ActiveSendMessage::Error(
                                            view::ActiveSendError::PrepareSend(e.to_string())
                                        ),
                                    )),
                                },
                            );

                            let onchain_send = Task::perform(
                                async move {
                                    breez_clone
                                        .prepare_pay_onchain(
                                            &breez_sdk_liquid::prelude::PreparePayOnchainRequest {
                                                amount:
                                                    breez_sdk_liquid::prelude::PayAmount::Bitcoin {
                                                        receiver_amount_sat: amount_sat,
                                                    },
                                                fee_rate_sat_per_vbyte: None,
                                            },
                                        )
                                        .await
                                },
                                |result| match result {
                                    Ok(prepare_response) => {
                                        Message::View(view::Message::ActiveSend(
                                            view::ActiveSendMessage::PrepareOnChainResponseReceived(
                                                prepare_response,
                                            ),
                                        ))
                                    }
                                    Err(e) => Message::View(view::Message::ActiveSend(
                                        view::ActiveSendMessage::Error(
                                            view::ActiveSendError::PrepareSend(e.to_string())
                                        ),
                                    )),
                                },
                            );

                            if let InputType::BitcoinAddress { .. } = input_type {
                                return onchain_send;
                            } else {
                                return lightning_send;
                            }
                        }
                    }
                }
                view::ActiveSendMessage::PrepareResponseReceived(prepare_response) => {
                    self.prepare_response = Some(prepare_response.clone());
                    self.flow_state = ActiveSendFlowState::FinalCheck;
                }
                view::ActiveSendMessage::PrepareOnChainResponseReceived(prepare_response) => {
                    self.prepare_onchain_response = Some(prepare_response.clone());
                    self.flow_state = ActiveSendFlowState::FinalCheck;
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::Close) => {
                    self.flow_state = ActiveSendFlowState::Main { modal: Modal::None };
                    self.amount = Amount::ZERO;
                    self.lightning_limits = None;
                    self.description = None;
                    self.comment = None;
                    self.amount_input = form::Value::default();
                    self.input = form::Value::default();
                    self.input_type = None;
                }
                view::ActiveSendMessage::ConfirmSend => {
                    if let ActiveSendFlowState::FinalCheck = &self.flow_state {
                        if self.is_sending {
                            return Task::none();
                        }

                        self.is_sending = true;

                        if let Some(prepare_response) = self.prepare_response.clone() {
                            let breez_client = self.breez_client.clone();
                            let comment = self.comment.clone();

                            return Task::perform(
                                async move {
                                    breez_client
                                        .send_payment(
                                            &breez_sdk_liquid::prelude::SendPaymentRequest {
                                                prepare_response,
                                                payer_note: comment,
                                                use_asset_fees: None,
                                            },
                                        )
                                        .await
                                },
                                |result| match result {
                                    Ok(_send_response) => Message::View(view::Message::ActiveSend(
                                        view::ActiveSendMessage::SendComplete,
                                    )),
                                    Err(e) => Message::View(view::Message::ActiveSend(
                                        view::ActiveSendMessage::Error(
                                            view::ActiveSendError::Send(e.to_string())
                                        ),
                                    )),
                                },
                            );
                        } else if let Some(prepare_onchain_response) =
                            self.prepare_onchain_response.clone()
                        {
                            if let Some(InputType::BitcoinAddress { address }) =
                                self.input_type.clone()
                            {
                                let breez_client = self.breez_client.clone();

                                return Task::perform(
                                    async move {
                                        breez_client
                                            .pay_onchain(
                                                &breez_sdk_liquid::prelude::PayOnchainRequest {
                                                    address: address.address.clone(),
                                                    prepare_response: prepare_onchain_response,
                                                },
                                            )
                                            .await
                                    },
                                    |result| match result {
                                        Ok(_send_response) => {
                                            Message::View(view::Message::ActiveSend(
                                                view::ActiveSendMessage::SendComplete,
                                            ))
                                        }
                                        Err(e) => Message::View(view::Message::ActiveSend(
                                            view::ActiveSendMessage::Error(
                                                view::ActiveSendError::Send(e.to_string())
                                            ),
                                        )),
                                    },
                                );
                            }
                        } else {
                            self.error = Some("No prepare response available".to_string());
                            self.is_sending = false;
                        }
                    }
                }
                view::ActiveSendMessage::SendComplete => {
                    self.flow_state = ActiveSendFlowState::Sent;
                    self.prepare_response = None;
                    self.is_sending = false;
                }
                view::ActiveSendMessage::BackToHome => {
                    self.input = form::Value::default();
                    self.amount = Amount::ZERO;
                    self.amount_input = form::Value::default();
                    self.input_type = None;
                    self.description = None;
                    self.comment = None;
                    self.lightning_limits = None;
                    self.prepare_response = None;
                    self.is_sending = false;
                    self.flow_state = ActiveSendFlowState::Main { modal: Modal::None };
                }
                view::ActiveSendMessage::LightningLimitsFetched { min_sat, max_sat } => {
                    self.lightning_limits = Some((*min_sat, *max_sat));
                }
                view::ActiveSendMessage::OnChainLimitsFetched { min_sat, max_sat } => {
                    self.onchain_limits = Some((*min_sat, *max_sat));
                }
                view::ActiveSendMessage::PopupMessage(SendPopupMessage::FiatClose) => {
                    self.flow_state = ActiveSendFlowState::Main {
                        modal: Modal::AmountInput,
                    }
                }
                view::ActiveSendMessage::RefreshRequested => {
                    return self.load_balance();
                }
            }
        }
        if let Message::View(view::Message::Close) | Message::View(view::Message::Reload) = message
        {
            self.selected_payment = None;
        }
        Task::none()
    }

    fn subscription(&self) -> iced::Subscription<Message> {
        iced::Subscription::none()
    }

    fn reload(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        self.selected_payment = None;
        self.load_balance()
    }
}

fn display_abbreviated(s: String) -> String {
    let formatted = if s.chars().count() > 30 {
        let first: String = s.chars().take(7).collect();
        let last: String = s
            .chars()
            .rev()
            .take(7)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        format!("{first}.....{last}")
    } else {
        s.to_string()
    };
    formatted
}
