pub mod ui;

use crate::app::{
    breez::BreezClient,
    view::{self, buysell::panel},
};

use crate::services::{
    coincube::*,
    mavapay::{api::*, MavapayClient, MavapayMessage},
};

#[derive(Debug)]
pub enum MavapayFlowStep {
    BuyInputFrom {
        ln_invoice: Option<String>,
        getting_invoice: bool,
        sending_quote: bool,
    },
    SellInputForm {
        liquid_balance: Option<u64>,
        banks: Option<MavapayBanks>,
        beneficiary: Beneficiary,
        sending_quote: bool,
    },
    Checkout {
        quote: GetQuoteResponse,
        fulfilled_order: Option<GetOrderResponse>,
        invoice_qr_code_data: Option<iced::widget::qr_code::Data>,
        liquid_balance: Option<u64>,
        fulfilling_ln_invoice: bool,
        /// Order ID for SSE transaction status updates
        stream_order_id: Option<String>,
    },
    History {
        transactions: Option<Vec<OrderTransaction>>,
        loading: bool,
    },
    OrderDetail {
        transaction: OrderTransaction,
        order: Option<GetOrderResponse>,
        loading: bool,
    },
}

pub struct MavapayState {
    pub buy_or_sell: panel::BuyOrSell,
    pub steps: Vec<MavapayFlowStep>,
    pub country: &'static Country,

    pub breez_client: std::sync::Arc<BreezClient>,

    pub(self) sat_amount: u64,        // Unit Amount in BTCSAT
    pub(self) btc_price: Option<f64>, // btc_price_in_unit_currency
}

impl MavapayState {
    pub fn new(
        buy_or_sell: panel::BuyOrSell,
        base: MavapayFlowStep,
        country: &'static Country,
        breez_client: std::sync::Arc<BreezClient>,
    ) -> MavapayState {
        MavapayState {
            buy_or_sell,
            steps: vec![base],
            country,
            sat_amount: 6000,
            btc_price: None,
            breez_client,
        }
    }

    pub fn update(
        &mut self,
        msg: MavapayMessage,
        coincube_client: &CoincubeClient,
    ) -> Option<iced::Task<view::Message>> {
        match (self.steps.last_mut()?, msg) {
            (_, MavapayMessage::NavigateBack) => {
                match self.steps.len() {
                    1 => {
                        // `MavapayState` must always have at least one FlowStep
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::ResetWidget,
                        )));
                    }
                    _ => self.steps.pop(),
                };
            }

            (_, MavapayMessage::GetPrice) => {
                let client = coincube_client.clone();
                let currency = match self.country.code {
                    "KE" => MavapayCurrency::KenyanShilling,
                    "ZA" => MavapayCurrency::SouthAfricanRand,
                    "NG" => MavapayCurrency::NigerianNaira,
                    c => unreachable!("Country {:?} is not supported by Mavapay", c),
                };

                let task = iced::Task::perform(
                    async move { MavapayClient(&client).get_price(currency).await },
                    |result| match result {
                        MavapayApiResult::Success(price) => {
                            MavapayMessage::PriceReceived(price).into()
                        }
                        MavapayApiResult::Error(e) => {
                            view::Message::BuySell(view::BuySellMessage::SessionError(
                                "Unable to get latest Bitcoin price",
                                e,
                            ))
                        }
                    },
                );

                return Some(task);
            }
            (_, MavapayMessage::PriceReceived(res)) => {
                self.btc_price = Some(res.btc_price_in_unit_currency)
            }

            (_, MavapayMessage::NormalizeAmounts) => {
                self.sat_amount = self.sat_amount.clamp(6000, 2_100_000_000_000_000)
            }
            (_, MavapayMessage::SatAmountChanged(sats)) => self.sat_amount = sats.round() as _,
            (_, MavapayMessage::FiatAmountChanged(fiat)) => match self.btc_price {
                Some(price) => {
                    let sat_price = price / 100_000_000.0;
                    self.sat_amount = (fiat / sat_price).round() as _
                }
                None => log::warn!("Unable to update BTC amount, BTC price is unknown"),
            },
            (_, MavapayMessage::SendQuote(request)) => {
                let coincube_client = coincube_client.clone();

                let task = iced::Task::perform(
                    async move {
                        // Step 1: Create quote with Mavapay
                        let quote =
                            match MavapayClient(&coincube_client).create_quote(request).await {
                                MavapayApiResult::Success(q) => q,
                                MavapayApiResult::Error(e) => return Err(e),
                            };

                        // Step 2: Save quote to coincube-api (fallible)
                        match coincube_client.save_quote(&quote.id, &quote).await {
                            Ok(_) => {
                                log::trace!("[COINCUBE] Successfully saved quote: {}", quote.id)
                            }
                            Err(err) => {
                                log::error!("[COINCUBE] Unable to save quote: {:?}", err)
                            }
                        };

                        Ok(quote)
                    },
                    move |result| match result {
                        Ok(quote) => MavapayMessage::QuoteCreated(quote).into(),
                        Err(e) => view::Message::BuySell(view::BuySellMessage::SessionError(
                            "Unable to create quote",
                            e,
                        )),
                    },
                );

                return Some(task);
            }

            // state specific updates
            (
                MavapayFlowStep::BuyInputFrom {
                    getting_invoice,
                    ln_invoice,
                    sending_quote,
                },
                msg,
            ) => {
                match msg {
                    MavapayMessage::GenerateLightningInvoice => {
                        *getting_invoice = true;

                        let breez_client = self.breez_client.clone();
                        let amount =
                            Some(breez_sdk_liquid::bitcoin::Amount::from_sat(self.sat_amount));
                        let description = format!(
                            "Coincube-Buysell Mavapay Purchase: {} SATS",
                            self.sat_amount
                        );

                        let task = iced::Task::perform(
                            async move {
                                match breez_client
                                    .receive_invoice(amount, Some(description))
                                    .await
                                {
                                    Ok(res) => Ok(res.destination),
                                    Err(e) => Err(e.to_string()),
                                }
                            },
                            |res| match res {
                                Ok(ln_invoice) => {
                                    MavapayMessage::LightningInvoiceReceived(ln_invoice).into()
                                }
                                Err(e) => {
                                    view::Message::BuySell(view::BuySellMessage::SessionError(
                                        "Unable to acquire new invoice",
                                        e.to_string(),
                                    ))
                                }
                            },
                        )
                        .chain(iced::Task::done(MavapayMessage::CreateQuote.into()));

                        return Some(task);
                    }
                    MavapayMessage::LightningInvoiceReceived(invoice) => {
                        *getting_invoice = false;
                        *ln_invoice = Some(invoice);
                    }
                    MavapayMessage::WriteInvoiceToClipboard => {
                        return ln_invoice.as_ref().map(|invoice| {
                            iced::Task::batch([
                                iced::Task::done(view::Message::Clipboard(invoice.clone())),
                                iced::Task::done(view::Message::ShowError(
                                    "Invoice Copied to Clipboard".to_string(),
                                )),
                            ])
                        })
                    }
                    MavapayMessage::CreateQuote => {
                        let Some(invoice) = ln_invoice else {
                            unreachable!()
                        };

                        let beneficiary = Beneficiary::Lightning {
                            ln_invoice: invoice.clone(),
                        };
                        let local_currency = match self.country.code {
                            "KE" => MavapayUnitCurrency::KenyanShillingCent,
                            "NG" => MavapayUnitCurrency::NigerianNairaKobo,
                            "ZA" => MavapayUnitCurrency::SouthAfricanRandCent,
                            iso => unreachable!("Country ({}) is unsupported by Mavapay", iso),
                        };

                        let request = GetQuoteRequest {
                            amount: self.sat_amount,
                            source_currency: local_currency,
                            target_currency: MavapayUnitCurrency::BitcoinSatoshi,
                            // the currency denomination of the `amount` field
                            payment_currency: MavapayUnitCurrency::BitcoinSatoshi,
                            speed: OnchainTransferSpeed::Fast,
                            autopayout: true,
                            customer_internal_fee: None,
                            payment_method: MavapayPaymentMethod::BankTransfer,
                            beneficiary_format: beneficiary.format(),
                            beneficiary,
                        };

                        *sending_quote = true;
                        return Some(iced::Task::done(MavapayMessage::SendQuote(request).into()));
                    }
                    MavapayMessage::QuoteCreated(quote) => {
                        // Set up SSE stream for transaction status updates
                        if let Some(order_id) = quote.order_id.clone() {
                            *sending_quote = false;

                            // proceed to checkout
                            self.steps.push(MavapayFlowStep::Checkout {
                                quote,
                                invoice_qr_code_data: None,
                                fulfilled_order: None,
                                stream_order_id: Some(order_id),
                                liquid_balance: None,
                                fulfilling_ln_invoice: false,
                            });
                        } else {
                            return Some(iced::Task::done(view::Message::BuySell(
                                view::BuySellMessage::SessionError(
                                    "Unable to process payment",
                                    "Mavapay Quote created without `order-id`".to_string(),
                                ),
                            )));
                        };
                    }

                    msg => log::warn!("[MAVAPAY] Message Ignored {:?}", msg),
                }
            }

            (
                MavapayFlowStep::SellInputForm {
                    sending_quote,
                    beneficiary,
                    banks,
                    liquid_balance,
                },
                msg,
            ) => match msg {
                MavapayMessage::GetBanks => {
                    let code = self.country.code;

                    if code == "KE" {
                        // Kenyan customers use mobile money, not bank transfers
                        return None;
                    }

                    let client = coincube_client.clone();
                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).get_banks(code).await },
                        |result| match result {
                            MavapayApiResult::Success(banks) => {
                                MavapayMessage::BanksReceived(banks).into()
                            }
                            MavapayApiResult::Error(e) => {
                                view::Message::BuySell(view::BuySellMessage::SessionError(
                                    "Unable to fetch supported banks for your country",
                                    e,
                                ))
                            }
                        },
                    );

                    return Some(task);
                }
                MavapayMessage::BanksReceived(mut b) => {
                    if cfg!(debug_assertions) {
                        match b {
                            MavapayBanks::Nigerian(ref mut banks) => {
                                banks.sort_by(|a, b| a.nip_bank_code.cmp(&b.nip_bank_code))
                            }
                            MavapayBanks::SouthAfrican(ref mut banks) => banks.sort(),
                        }
                    }

                    let b = banks.insert(b);

                    if match b {
                        MavapayBanks::Nigerian(banks) => banks.is_empty(),
                        MavapayBanks::SouthAfrican(banks) => banks.is_empty(),
                    } {
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::SessionError(
                                "Can't proceed with flow",
                                "Unable to fetch banks from API".to_string(),
                            ),
                        )));
                    };
                }
                MavapayMessage::GetLiquidWalletBalance => {
                    let client = self.breez_client.clone();
                    let task = iced::Task::perform(
                        async move { client.info().await.map(|res| res.wallet_info.balance_sat) },
                        |result| match result {
                            Ok(balance) => {
                                MavapayMessage::ReceivedLiquidWalletBalance(balance).into()
                            }
                            Err(e) => view::Message::BuySell(view::BuySellMessage::SessionError(
                                "Unable to fetch Liquid Wallet balance",
                                e.to_string(),
                            )),
                        },
                    );

                    return Some(task);
                }
                MavapayMessage::ReceivedLiquidWalletBalance(sats) => *liquid_balance = Some(sats),

                MavapayMessage::BeneficiaryFieldUpdate(field, data) => match (field, beneficiary) {
                    // NG
                    (
                        "NGN.bank_account_number",
                        Beneficiary::NGN {
                            bank_account_number,
                            bank_account_name,
                            ..
                        },
                    ) => {
                        *bank_account_number = data;
                        *bank_account_name = None;
                    }
                    (
                        "NGN.bank_code",
                        Beneficiary::NGN {
                            bank_code,
                            bank_name,
                            bank_account_name,
                            ..
                        },
                    ) => {
                        if let Some(MavapayBanks::Nigerian(ngn_banks)) = banks {
                            if let Some(bank) = ngn_banks.iter().find(|b| b.nip_bank_code == data) {
                                *bank_code = bank.nip_bank_code.clone();
                                *bank_name = bank.bank_name.clone();

                                *bank_account_name = None;
                            }
                        }
                    }
                    // ZA
                    (
                        "ZAR.bank_account_number",
                        Beneficiary::ZAR {
                            bank_account_number,
                            ..
                        },
                    ) => *bank_account_number = data,
                    ("ZAR.name", Beneficiary::ZAR { name, .. }) => *name = data,
                    ("ZAR.bank_name", Beneficiary::ZAR { bank_name, .. }) => *bank_name = data,
                    // KE
                    (
                        "KES.account_name",
                        Beneficiary::KES(KenyanBeneficiary::PayToPhone { account_name, .. }),
                    ) => *account_name = data,
                    (
                        "KES.phone_number",
                        Beneficiary::KES(KenyanBeneficiary::PayToPhone { phone_number, .. }),
                    ) => *phone_number = data,

                    (field, ..) => {
                        log::warn!("Field Edit: `{}` ignored for Beneficiary", field)
                    }
                },
                MavapayMessage::VerifyNgnBankDetails => {
                    if let Beneficiary::NGN {
                        bank_account_number,
                        bank_code,
                        ..
                    } = beneficiary
                    {
                        let client = coincube_client.clone();
                        let ban = bank_account_number.clone();
                        let bac = bank_code.clone();

                        let task = iced::Task::perform(
                            async move {
                                MavapayClient(&client)
                                    .ngn_customer_inquiry(&ban, &bac)
                                    .await
                            },
                            |res| match res {
                                MavapayApiResult::Success(details) => {
                                    MavapayMessage::VerifiedNgnBankDetails(details).into()
                                }
                                MavapayApiResult::Error(err) => {
                                    view::Message::BuySell(view::BuySellMessage::SessionError(
                                        "Unable to verify Bank Details",
                                        err,
                                    ))
                                }
                            },
                        );

                        return Some(task);
                    }
                }
                MavapayMessage::VerifiedNgnBankDetails(details) => {
                    if let Beneficiary::NGN {
                        bank_account_name, ..
                    } = beneficiary
                    {
                        *bank_account_name = Some(details.account_name);
                    }
                }

                MavapayMessage::CreateQuote => {
                    *sending_quote = true;

                    let local_currency = match self.country.code {
                        "KE" => MavapayUnitCurrency::KenyanShillingCent,
                        "NG" => MavapayUnitCurrency::NigerianNairaKobo,
                        "ZA" => MavapayUnitCurrency::SouthAfricanRandCent,
                        iso => unreachable!("Country ({}) is unsupported by Mavapay", iso),
                    };

                    let request = GetQuoteRequest {
                        amount: self.sat_amount,
                        source_currency: MavapayUnitCurrency::BitcoinSatoshi,
                        target_currency: local_currency,
                        // the currency denomination of the `amount` field
                        payment_currency: MavapayUnitCurrency::BitcoinSatoshi,
                        speed: OnchainTransferSpeed::Fast,
                        autopayout: true,
                        customer_internal_fee: None,
                        payment_method: MavapayPaymentMethod::Lightning,
                        beneficiary_format: beneficiary.format(),
                        beneficiary: beneficiary.clone(),
                    };

                    return Some(iced::Task::done(MavapayMessage::SendQuote(request).into()));
                }
                MavapayMessage::QuoteCreated(quote) => {
                    if let Some(order_id) = quote.order_id.clone() {
                        *sending_quote = false;

                        let liquid_balance = *liquid_balance;
                        let invoice_qr_code_data =
                            iced::widget::qr_code::Data::new(quote.invoice.as_bytes()).ok();

                        // proceed to checkout
                        self.steps.push(MavapayFlowStep::Checkout {
                            quote,
                            invoice_qr_code_data,
                            fulfilled_order: None,
                            stream_order_id: Some(order_id),
                            liquid_balance,
                            fulfilling_ln_invoice: false,
                        });
                    } else {
                        return Some(iced::Task::done(view::Message::BuySell(
                            view::BuySellMessage::SessionError(
                                "Unable to process payment",
                                "Mavapay Quote created without `order-id`".to_string(),
                            ),
                        )));
                    };
                }
                _ => {}
            },

            (
                MavapayFlowStep::Checkout {
                    quote,
                    fulfilled_order,
                    stream_order_id,
                    fulfilling_ln_invoice,
                    ..
                },
                msg,
            ) => match msg {
                MavapayMessage::FulfillSellInvoice => {
                    if let panel::BuyOrSell::Sell = self.buy_or_sell {
                        let client = self.breez_client.clone();
                        let invoice = quote.invoice.clone();
                        let amount = quote.total_amount_in_source_currency;

                        *fulfilling_ln_invoice = true;

                        return Some(iced::Task::perform(
                            async move { client.pay_invoice(invoice, Some(amount)).await },
                            |res| match res {
                                Ok(res) => MavapayMessage::SellInvoiceFulfilled(res.payment).into(),
                                Err(err) => {
                                    view::Message::BuySell(view::BuySellMessage::SessionError(
                                        "Unable to fulfil invoice",
                                        err.to_string(),
                                    ))
                                }
                            },
                        ));
                    }
                }
                MavapayMessage::SellInvoiceFulfilled(payment) => {
                    *fulfilling_ln_invoice = false;

                    let msg = format!(
                        "[LIQUID] Successfully satisfied invoice for {} SATS, TXID = {:?}",
                        payment.amount_sat, payment.tx_id
                    );

                    return Some(iced::Task::done(view::Message::ShowSuccess(msg)));
                }

                MavapayMessage::WriteInvoiceToClipboard => {
                    return Some(iced::Task::batch([
                        iced::Task::done(view::Message::Clipboard(quote.invoice.clone())),
                        iced::Task::done(view::Message::ShowError(
                            "Invoice Copied to Clipboard".to_string(),
                        )),
                    ]));
                }
                MavapayMessage::TransactionUpdated(update) => {
                    log::info!(
                        "[MAVAPAY SSE] Order {} event={} status={:?}",
                        update.order_id,
                        update.event_type,
                        update.status
                    );

                    if matches!(
                        update.status,
                        TransactionStatus::Paid | TransactionStatus::Success
                    ) {
                        log::info!(
                            "[MAVAPAY] Quote({}) has been fulfilled via SSE, order_id={}",
                            quote.id,
                            update.order_id
                        );

                        let client = coincube_client.clone();
                        let order_id = update.order_id.clone();

                        let task = iced::Task::perform(
                            async move {
                                let mut res = MavapayClient(&client).get_order(&order_id).await;

                                for _ in 0..5 {
                                    match &res {
                                        MavapayApiResult::Success(_) => return res,
                                        MavapayApiResult::Error(_) => {
                                            tokio::time::sleep(std::time::Duration::from_secs(1))
                                                .await
                                        }
                                    }

                                    res = MavapayClient(&client).get_order(&order_id).await;
                                }

                                res
                            },
                            |result| match result {
                                MavapayApiResult::Success(order) => {
                                    MavapayMessage::QuoteFulfilled(order).into()
                                }
                                MavapayApiResult::Error(e) => {
                                    view::Message::BuySell(view::BuySellMessage::SessionError(
                                        "Failed to fetch order details",
                                        e,
                                    ))
                                }
                            },
                        );
                        return Some(task);
                    }
                }

                MavapayMessage::StreamConnected => {
                    log::info!("[MAVAPAY SSE] Connected to transaction stream");
                }
                MavapayMessage::EventSourceDisconnected(msg) => {
                    log::info!("[MAVAPAY SSE] Stream disconnected: {}", msg);
                }
                MavapayMessage::StreamError(err) => {
                    log::error!("[MAVAPAY SSE] Stream error: {}", err);
                }
                MavapayMessage::QuoteFulfilled(order) => {
                    log::info!(
                        "[MAVAPAY] Quote({}) has been fulfilled: Order = {:?}",
                        quote.id,
                        order
                    );

                    *fulfilled_order = Some(order);
                    *stream_order_id = None;
                }

                #[cfg(debug_assertions)]
                MavapayMessage::SimulatePayIn => {
                    let Some(order_id) = quote.order_id.clone() else {
                        log::error!("[MAVAPAY] Cannot simulate Pay-In: Quote has no order_id");
                        return None;
                    };

                    log::info!(
                        "[MAVAPAY] Simulating Pay-In for Quote({}), Order ID({})",
                        quote.id,
                        order_id
                    );

                    let client = coincube_client.clone();
                    let request = SimulatePayInRequest {
                        order_id,
                        amount: matches!(self.buy_or_sell, panel::BuyOrSell::Buy)
                            .then_some(quote.amount_in_source_currency),
                        currency: (&quote.source_currency).into(),
                    };

                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).simulate_pay_in(&request).await },
                        |s| match s {
                            MavapayApiResult::Success(message) => {
                                log::info!("[MAVAPAY] {}", message)
                            }
                            MavapayApiResult::Error(e) => {
                                log::error!("[MAVAPAY] Unable to simulate Pay-In: {}", e)
                            }
                        },
                    );

                    return Some(task.then(|_| iced::Task::none()));
                }

                #[cfg(not(debug_assertions))]
                MavapayMessage::SimulatePayIn => {
                    log::warn!(
                                "[MAVAPAY] Unable to simulate pay-in for Quote({}), app built in release mode",
                                quote.id,
                            );
                }

                msg => log::warn!("[MAVAPAY] Message Ignored {:?}", msg),
            },

            (
                MavapayFlowStep::History {
                    transactions,
                    loading,
                },
                msg,
            ) => match msg {
                MavapayMessage::FetchTransactions => {
                    *loading = true;
                    let client = coincube_client.clone();

                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).get_transactions().await },
                        |result| match result {
                            MavapayApiResult::Success(response) => {
                                MavapayMessage::TransactionsReceived(response.transactions).into()
                            }
                            MavapayApiResult::Error(e) => {
                                view::Message::BuySell(view::BuySellMessage::SessionError(
                                    "Failed to fetch transactions",
                                    e,
                                ))
                            }
                        },
                    );

                    return Some(task);
                }
                MavapayMessage::TransactionsReceived(received_transactions) => {
                    log::info!(
                        "[MAVAPAY] Received {} transactions",
                        received_transactions.len()
                    );
                    *transactions = Some(received_transactions);
                    *loading = false;
                }
                MavapayMessage::SelectTransaction(idx) => {
                    let Some(transaction) = transactions.as_mut().and_then(|t| t.get(idx)) else {
                        log::warn!("[MAVAPAY] Selected transaction index is out of bounds");
                        return None;
                    };

                    let order_id = transaction.order_id.clone();
                    let client = coincube_client.clone();

                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).get_order(&order_id).await },
                        |result| match result {
                            MavapayApiResult::Success(order) => {
                                MavapayMessage::OrderReceived(order).into()
                            }
                            MavapayApiResult::Error(e) => {
                                view::Message::BuySell(view::BuySellMessage::SessionError(
                                    "Failed to fetch order details",
                                    e,
                                ))
                            }
                        },
                    );

                    let transaction = transaction.clone();
                    self.steps.push(MavapayFlowStep::OrderDetail {
                        transaction,
                        order: None,
                        loading: true,
                    });

                    return Some(task);
                }
                msg => log::warn!("[MAVAPAY] Message Ignored {:?}", msg),
            },

            (MavapayFlowStep::OrderDetail { order, loading, .. }, msg) => match msg {
                MavapayMessage::OrderReceived(received_order) => {
                    *order = Some(received_order);
                    *loading = false;
                }
                msg => log::warn!("[MAVAPAY] Message Ignored {:?}", msg),
            },
        }

        None
    }
}
