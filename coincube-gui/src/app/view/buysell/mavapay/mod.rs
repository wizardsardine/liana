pub mod ui;

use crate::app::view;
use crate::app::view::buysell::panel::{self, BuyOrSell};
use crate::services::mavapay::{MavapayClient, MavapayMessage};
use crate::services::{coincube::*, mavapay::api::*};

#[derive(Debug)]
pub enum MavapayState {
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

impl MavapayState {
    pub fn update(
        &mut self,
        msg: MavapayMessage,
        coincube_client: &CoincubeClient,
    ) -> Option<iced::Task<view::Message>> {
        // rust is weird, man
        let mut state = self;

        match (&mut state, msg) {
            // transactions form
            (
                MavapayState::Transaction {
                    sat_amount,
                    btc_price,
                    country,
                    banks,
                    buy_or_sell,
                    beneficiary,
                    transfer_speed,
                    sending_quote,
                    ..
                },
                msg,
            ) => {
                match msg {
                    MavapayMessage::NormalizeAmounts => {
                        *sat_amount = (*sat_amount).clamp(6000, 2_100_000_000_000_000)
                    }
                    MavapayMessage::SatAmountChanged(sats) => *sat_amount = sats.round() as _,
                    MavapayMessage::FiatAmountChanged(fiat) => match btc_price {
                        Some(price) => {
                            let sat_price = price.btc_price_in_unit_currency / 100_000_000.0;
                            *sat_amount = (fiat / sat_price).round() as _
                        }
                        None => log::warn!("Unable to update BTC amount, BTC price is unknown"),
                    },
                    MavapayMessage::TransferSpeedChanged(s) => *transfer_speed = s,

                    // TODO: Beneficiary specific form inputs
                    MavapayMessage::CreateQuote => {
                        *sending_quote = true;
                        let local_currency = match country.code {
                            "KE" => MavapayUnitCurrency::KenyanShillingCent,
                            "NG" => MavapayUnitCurrency::NigerianNairaKobo,
                            "ZA" => MavapayUnitCurrency::SouthAfricanRandCent,
                            iso => unreachable!("Country ({}) is unsupported by Mavapay", iso),
                        };

                        let request = match buy_or_sell {
                            panel::BuyOrSell::Sell => GetQuoteRequest {
                                amount: *sat_amount,
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
                            panel::BuyOrSell::Buy { address } => {
                                GetQuoteRequest {
                                    amount: *sat_amount,
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
                                }
                            }
                        };

                        // prepare request
                        let coincube_client = coincube_client.clone();

                        let task = iced::Task::perform(
                            async move {
                                // Step 1: Create quote with Mavapay
                                let quote = match MavapayClient(&coincube_client)
                                    .create_quote(request)
                                    .await
                                {
                                    MavapayApiResult::Success(q) => q,
                                    MavapayApiResult::Error(e) => return Err(e),
                                };

                                // Step 2: Save quote to coincube-api (fallible)
                                match coincube_client.save_quote(&quote.id, &quote).await {
                                    Ok(_) => log::info!(
                                        "[COINCUBE] Successfully saved quote: {}",
                                        quote.id
                                    ),
                                    Err(err) => {
                                        log::error!("[COINCUBE] Unable to save quote: {:?}", err)
                                    }
                                };

                                Ok(quote)
                            },
                            move |result: Result<GetQuoteResponse, String>| match result {
                                Ok(quote) => view::BuySellMessage::Mavapay(
                                    MavapayMessage::QuoteCreated(quote),
                                ),
                                Err(e) => {
                                    view::BuySellMessage::SessionError("Unable to create quote", e)
                                }
                            },
                        )
                        .map(view::Message::BuySell);

                        return Some(task);
                    }
                    MavapayMessage::QuoteCreated(quote) => {
                        log::info!(
                            "[MAVAPAY] Quote created: {}, Order ID: {:?}",
                            quote.id,
                            quote.order_id
                        );

                        // Set up SSE stream for transaction status updates
                        if let Some(order_id) = quote.order_id.clone() {
                            // switch to checkout
                            *state = MavapayState::Checkout {
                                sat_amount: *sat_amount,
                                buy_or_sell: buy_or_sell.clone(),
                                beneficiary: beneficiary.clone(),
                                quote,
                                fulfilled_order: None,
                                country: country.clone(),
                                stream_order_id: Some(order_id),
                            };
                        } else {
                            *sending_quote = false;
                            return Some(iced::Task::done(view::Message::BuySell(
                                view::BuySellMessage::SessionError(
                                    "Unable to process payment",
                                    "Mavapay Quote created without `order-id`".to_string(),
                                ),
                            )));
                        };
                    }

                    MavapayMessage::GetPrice => {
                        let client = coincube_client.clone();
                        let currency = match country.code {
                            "KE" => MavapayCurrency::KenyanShilling,
                            "ZA" => MavapayCurrency::SouthAfricanRand,
                            "NG" => MavapayCurrency::NigerianNaira,
                            c => unreachable!("Country {:?} is not supported by Mavapay", c),
                        };

                        let task = iced::Task::perform(
                            async move { MavapayClient(&client).get_price(currency).await },
                            |result| match result {
                                MavapayApiResult::Success(price) => view::BuySellMessage::Mavapay(
                                    MavapayMessage::PriceReceived(price),
                                ),
                                MavapayApiResult::Error(e) => view::BuySellMessage::SessionError(
                                    "Unable to get latest Bitcoin price",
                                    e,
                                ),
                            },
                        )
                        .map(view::Message::BuySell);

                        return Some(task);
                    }
                    MavapayMessage::GetBanks => {
                        let code = country.code;
                        let client = coincube_client.clone();

                        let task = iced::Task::perform(
                            async move { MavapayClient(&client).get_banks(code).await },
                            |result| match result {
                                MavapayApiResult::Success(banks) => view::BuySellMessage::Mavapay(
                                    MavapayMessage::BanksReceived(banks),
                                ),
                                MavapayApiResult::Error(e) => view::BuySellMessage::SessionError(
                                    "Unable to fetch supported banks for your country",
                                    e,
                                ),
                            },
                        )
                        .map(view::Message::BuySell);

                        return Some(task);
                    }

                    MavapayMessage::PriceReceived(price) => *btc_price = Some(price),
                    MavapayMessage::BanksReceived(b) => *banks = Some(b),

                    msg => log::warn!("Current {:?} has ignored message: {:?}", *state, msg),
                }
            }
            // checkout form
            (
                MavapayState::Checkout {
                    quote,
                    fulfilled_order,
                    stream_order_id,
                    ..
                },
                msg,
            ) => match msg {
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

                        *stream_order_id = None;

                        let task = iced::Task::perform(
                            async move {
                                // Small delay to allow backend to finalize order
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                MavapayClient(&client).get_order(&order_id).await
                            },
                            |result| match result {
                                MavapayApiResult::Success(order) => {
                                    view::Message::BuySell(view::BuySellMessage::Mavapay(
                                        MavapayMessage::QuoteFulfilled(order),
                                    ))
                                }
                                MavapayApiResult::Error(e) => {
                                    log::error!(
                                        "[MAVAPAY] Failed to fetch order after SSE success: {}",
                                        e
                                    );
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
                        amount: quote.amount_in_source_currency,
                        currency: quote.source_currency.clone().into(),
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

                msg => {
                    log::warn!("Current {:?} has ignored message: {:?}", *state, msg)
                }
            },
            (
                MavapayState::History {
                    transactions,
                    loading,
                    error,
                },
                msg,
            ) => match msg {
                MavapayMessage::FetchTransactions => {
                    *loading = true;
                    *error = None;
                    let client = coincube_client.clone();

                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).get_transactions().await },
                        |result| match result {
                            MavapayApiResult::Success(response) => view::BuySellMessage::Mavapay(
                                MavapayMessage::TransactionsReceived(response.transactions),
                            ),
                            MavapayApiResult::Error(e) => view::BuySellMessage::SessionError(
                                "Failed to fetch transactions",
                                e,
                            ),
                        },
                    )
                    .map(view::Message::BuySell);

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
                    let Some(transaction) = transactions.as_mut().and_then(|t| t.get(idx))
                    else {
                        log::warn!("[MAVAPAY] Selected transaction index is out of bounds");
                        return None;
                    };

                    let order_id = transaction.order_id.clone();
                    let client = coincube_client.clone();

                    let task = iced::Task::perform(
                        async move { MavapayClient(&client).get_order(&order_id).await },
                        |result| match result {
                            MavapayApiResult::Success(order) => {
                                view::BuySellMessage::Mavapay(MavapayMessage::OrderReceived(order))
                            }
                            MavapayApiResult::Error(e) => view::BuySellMessage::SessionError(
                                "Failed to fetch order details",
                                e,
                            ),
                        },
                    )
                    .map(view::Message::BuySell);

                    *state = MavapayState::OrderDetail {
                        transaction: transaction.clone(),
                        order: None,
                        loading: true,
                    };

                    return Some(task);
                }
                msg => {
                    log::warn!("Current {:?} has ignored message: {:?}", *state, msg)
                }
            },
            (MavapayState::OrderDetail { order, loading, .. }, msg) => match msg {
                MavapayMessage::OrderReceived(received_order) => {
                    *order = Some(received_order);
                    *loading = false;
                }
                MavapayMessage::BackToHistory => {
                    *state = MavapayState::History {
                        transactions: None,
                        loading: true,
                        error: None,
                    };

                    return Some(iced::Task::done(view::Message::BuySell(
                        view::BuySellMessage::Mavapay(MavapayMessage::FetchTransactions),
                    )));
                }
                msg => {
                    log::warn!("Current {:?} has ignored message: {:?}", *state, msg)
                }
            },
        }

        None
    }
}
